use crate::assets::ImageHandle;
use crate::platform::{Antialiasing, Color, SharedPlatformState, lock_platform_state};
use crate::renderer::{self, DrawCommand, Rect, SharedRenderState, TextureFilter, Vec2};
use bytemuck::{Pod, Zeroable};
use image::RgbaImage;
use naga::back::spv;
use naga::front::glsl;
use naga::valid::{Capabilities, ValidationFlags, Validator};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use vulkano::buffer::{Buffer, BufferContents, BufferCreateInfo, BufferUsage};
use vulkano::command_buffer::allocator::StandardCommandBufferAllocator;
use vulkano::command_buffer::{
    AutoCommandBufferBuilder, CommandBufferUsage, PrimaryAutoCommandBuffer, RenderPassBeginInfo,
    SubpassBeginInfo, SubpassContents, SubpassEndInfo,
};
use vulkano::descriptor_set::allocator::StandardDescriptorSetAllocator;
use vulkano::descriptor_set::{PersistentDescriptorSet, WriteDescriptorSet};
use vulkano::device::{Device, Queue};
use vulkano::format::{ClearValue, Format};
use vulkano::image::sampler::{Filter, Sampler, SamplerAddressMode, SamplerCreateInfo};
use vulkano::image::view::ImageView;
use vulkano::image::{Image, ImageCreateInfo, ImageLayout, ImageUsage, SampleCount, SampleCounts};
use vulkano::memory::allocator::{AllocationCreateInfo, MemoryTypeFilter, StandardMemoryAllocator};
use vulkano::pipeline::graphics::color_blend::{
    AttachmentBlend, ColorBlendAttachmentState, ColorBlendState, ColorComponents,
};
use vulkano::pipeline::graphics::depth_stencil::{CompareOp, DepthState, DepthStencilState};
use vulkano::pipeline::graphics::input_assembly::{InputAssemblyState, PrimitiveTopology};
use vulkano::pipeline::graphics::multisample::MultisampleState;
use vulkano::pipeline::graphics::rasterization::RasterizationState;
use vulkano::pipeline::graphics::subpass::PipelineSubpassType;
use vulkano::pipeline::graphics::vertex_input::{
    Vertex, VertexInputAttributeDescription, VertexInputBindingDescription, VertexInputState,
};
use vulkano::pipeline::graphics::viewport::{Viewport, ViewportState};
use vulkano::pipeline::layout::PipelineDescriptorSetLayoutCreateInfo;
use vulkano::pipeline::{
    DynamicState, GraphicsPipeline, Pipeline, PipelineBindPoint, PipelineLayout,
    PipelineShaderStageCreateInfo,
};
use vulkano::render_pass::{Framebuffer, FramebufferCreateInfo, RenderPass, Subpass};
use vulkano::shader::{ShaderModule, ShaderModuleCreateInfo};
use vulkano::swapchain::{
    self, PresentMode, Surface, Swapchain, SwapchainCreateInfo, SwapchainPresentInfo,
};
use vulkano::sync::{self, GpuFuture};
use vulkano::{Validated, VulkanError};
use vulkano::{Version, VulkanLibrary, single_pass_renderpass};
use winit::event_loop::EventLoop;
use winit::window::Window;

const BUILTIN_VERTEX_SHADER: &str = r#"#version 450
layout(location = 0) in vec4 position;
layout(location = 1) in vec4 color;
layout(location = 2) in vec2 uv;

layout(location = 0) out vec4 v_color;
layout(location = 1) out vec2 v_uv;

void main() {
    gl_Position = position;
    v_color = color;
    v_uv = uv;
}
"#;

const BUILTIN_FRAGMENT_SHADER: &str = r#"#version 450
layout(binding = 0) uniform texture2D Texture;
layout(binding = 1) uniform sampler TextureSampler;

layout(location = 0) in vec4 color;
layout(location = 1) in vec2 uv;
layout(location = 0) out vec4 f_color;

void main() {
    f_color = texture(sampler2D(Texture, TextureSampler), uv) * color;
}
"#;

const EQUIRECTANGULAR_ENVIRONMENT_FRAGMENT_SHADER: &str = r#"#version 450
layout(binding = 0) uniform texture2D Texture;
layout(binding = 1) uniform sampler TextureSampler;
layout(binding = 2) uniform EnvironmentUniforms {
    vec4 slots[16];
};

layout(location = 0) in vec4 color;
layout(location = 1) in vec2 uv;
layout(location = 0) out vec4 f_color;

void main() {
    vec2 plane = vec2(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    vec3 direction = normalize(
        slots[2].xyz + slots[0].xyz * plane.x * slots[3].x
        + slots[1].xyz * plane.y * slots[3].y
    );
    float yaw_sin = slots[3].z;
    float yaw_cos = slots[3].w;
    direction = vec3(
        direction.x * yaw_cos - direction.z * yaw_sin,
        direction.y,
        direction.x * yaw_sin + direction.z * yaw_cos
    );
    float panorama_u = fract(atan(direction.z, direction.x) / 6.28318530718 + 0.5);
    float panorama_v = clamp(0.5 - asin(clamp(direction.y, -1.0, 1.0)) / 3.14159265359, 0.0, 1.0);
    vec4 sampled = texture(sampler2D(Texture, TextureSampler), vec2(panorama_u, panorama_v));
    f_color = vec4(sampled.rgb * max(slots[4].x, 0.0), sampled.a);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Zeroable, Pod, Vertex)]
struct GpuVertex {
    #[format(R32G32B32A32_SFLOAT)]
    position: [f32; 4],
    #[format(R32G32B32A32_SFLOAT)]
    color: [f32; 4],
    #[format(R32G32_SFLOAT)]
    uv: [f32; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct TextureKey(u64);

struct TextureBatch {
    texture: TextureKey,
    filter: TextureFilter,
    vertices: Vec<GpuVertex>,
    shader: BatchShaderState,
}

struct CachedTexture {
    revision: u64,
    view: Arc<ImageView>,
    descriptor_nearest: Arc<PersistentDescriptorSet>,
    descriptor_linear: Arc<PersistentDescriptorSet>,
}

fn environment_scaled_color(color: Color, intensity: f32) -> Color {
    let intensity = if intensity.is_finite() {
        intensity.max(0.0)
    } else {
        1.0
    };
    Color::rgba(
        (color.r as f32 * intensity).clamp(0.0, 255.0).round() as u8,
        (color.g as f32 * intensity).clamp(0.0, 255.0).round() as u8,
        (color.b as f32 * intensity).clamp(0.0, 255.0).round() as u8,
        color.a,
    )
}

fn environment_clear_color(
    environment: &crate::environment3d::Environment3D,
    fallback: Color,
) -> Color {
    if environment.enabled
        && environment.mode == crate::environment3d::EnvironmentMode3D::Solid
    {
        environment_scaled_color(environment.solid, environment.intensity)
    } else {
        fallback
    }
}

/// A prepared light-map composite: the multiply pipeline, the uploaded light-map
/// texture descriptor, and a fullscreen quad. Drawn last, it multiplies the
/// light over the finished scene.
#[derive(Clone)]
struct LightComposite {
    pipeline: Arc<GraphicsPipeline>,
    descriptor: Arc<PersistentDescriptorSet>,
    vertex_buffer: vulkano::buffer::Subbuffer<[GpuVertex]>,
}

struct CachedLightComposite {
    generation: u64,
    composite: LightComposite,
}

// Generated images (camera frames, procedural images, and rasterized text)
// must not pin Vulkan textures forever after their Luau handles disappear.
// Keep a generous working set, but retire entries that have not participated
// in a frame for several seconds.
const GPU_TEXTURE_IDLE_FRAMES: u64 = 300;
const GPU_IMAGE_CACHE_LIMIT: usize = 256;
const GPU_TEXT_CACHE_LIMIT: usize = 256;

/// Log a GPU-composite failure once. Lighting then falls back to unlit rather
/// than failing the frame.
fn warn_light_composite_once(error: &str) {
    use std::sync::Once;
    static WARN: Once = Once::new();
    WARN.call_once(|| {
        eprintln!("lighting: GPU composite unavailable, rendering scene unlit ({error})");
    });
}

#[derive(Clone, Debug, PartialEq)]
struct BatchShaderState {
    pipeline_key: u64,
    fragment_source: Option<String>,
    uses_uniform_buffer: bool,
    uniform_slots: [[f32; 4]; crate::shader::MAX_SHADER_FLOAT_UNIFORMS],
    extra_textures: Vec<(u32, TextureKey)>,
}

impl BatchShaderState {
    fn default_pipeline() -> Self {
        Self {
            pipeline_key: 0,
            fragment_source: None,
            uses_uniform_buffer: false,
            uniform_slots: [[0.0; 4]; crate::shader::MAX_SHADER_FLOAT_UNIFORMS],
            extra_textures: Vec::new(),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, BufferContents)]
struct ShaderUniformBuffer {
    slots: [[f32; 4]; crate::shader::MAX_SHADER_FLOAT_UNIFORMS],
}

pub(crate) struct VulkanPresenter {
    device: Arc<Device>,
    queue: Arc<Queue>,
    swapchain: Arc<Swapchain>,
    images: Vec<Arc<Image>>,
    memory_allocator: Arc<StandardMemoryAllocator>,
    command_buffer_allocator: StandardCommandBufferAllocator,
    descriptor_set_allocator: StandardDescriptorSetAllocator,
    previous_frame_end: Option<Box<dyn GpuFuture>>,
    render_pass: Arc<RenderPass>,
    framebuffers: Vec<Arc<Framebuffer>>,
    pipeline: Arc<GraphicsPipeline>,
    /// Lazily-created multiply-blend pipeline for the light-map composite.
    composite_pipeline: Option<Arc<GraphicsPipeline>>,
    light_map_cache: crate::lighting::LightMapCache,
    light_composite_cache: Option<CachedLightComposite>,
    recreate_swapchain: bool,
    nearest_sampler: Arc<Sampler>,
    linear_sampler: Arc<Sampler>,
    supported_samples: SampleCounts,
    msaa_samples: SampleCount,
    white_texture: TextureKey,
    texture_cache: HashMap<TextureKey, CachedTexture>,
    shader_cache: HashMap<u64, Arc<GraphicsPipeline>>,
    image_cache_keys: HashMap<usize, TextureKey>,
    image_cache_last_used: HashMap<usize, u64>,
    text_cache: HashMap<u64, TextureKey>,
    text_cache_last_used: HashMap<u64, u64>,
    frame_serial: u64,
    next_texture_key: u64,
}

impl VulkanPresenter {
    pub(crate) fn new(
        event_loop: &EventLoop<()>,
        window: Arc<Window>,
    ) -> Result<(Self, Arc<Surface>), String> {
        let library = VulkanLibrary::new().map_err(|e| e.to_string())?;
        let instance = vulkano::instance::Instance::new(
            library,
            vulkano::instance::InstanceCreateInfo {
                enabled_extensions: Surface::required_extensions(event_loop),
                max_api_version: Some(Version::V1_1),
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
        let surface = Surface::from_window(instance.clone(), window).map_err(|e| e.to_string())?;

        let device_extensions = vulkano::device::DeviceExtensions {
            khr_swapchain: true,
            ..Default::default()
        };

        let (physical, queue_family_index) = instance
            .enumerate_physical_devices()
            .map_err(|e| e.to_string())?
            .filter(|physical| physical.supported_extensions().contains(&device_extensions))
            .filter_map(|physical| {
                physical
                    .queue_family_properties()
                    .iter()
                    .enumerate()
                    .position(|(index, family)| {
                        family
                            .queue_flags
                            .intersects(vulkano::device::QueueFlags::GRAPHICS)
                            && physical
                                .surface_support(index as u32, &surface)
                                .map_err(Validated::unwrap)
                                .unwrap_or(false)
                    })
                    .map(|index| (physical, index as u32))
            })
            .min_by_key(|(physical, _)| match physical.properties().device_type {
                vulkano::device::physical::PhysicalDeviceType::DiscreteGpu => 0,
                vulkano::device::physical::PhysicalDeviceType::IntegratedGpu => 1,
                vulkano::device::physical::PhysicalDeviceType::VirtualGpu => 2,
                vulkano::device::physical::PhysicalDeviceType::Cpu => 3,
                _ => 4,
            })
            .ok_or_else(|| "no suitable Vulkan physical device found".to_string())?;

        let mut supported_samples = physical.properties().framebuffer_color_sample_counts
            & physical.properties().framebuffer_depth_sample_counts;
        let mut msaa_samples = preferred_sample_count(Antialiasing::High, supported_samples);

        let (device, mut queues) = Device::new(
            physical.clone(),
            vulkano::device::DeviceCreateInfo {
                enabled_extensions: device_extensions,
                queue_create_infos: vec![vulkano::device::QueueCreateInfo {
                    queue_family_index,
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
        let queue = queues
            .next()
            .ok_or_else(|| "failed to create Vulkan queue".to_string())?;

        let surface_caps = physical
            .surface_capabilities(&surface, Default::default())
            .map_err(|e| e.to_string())?;
        let surface_formats = physical
            .surface_formats(&surface, Default::default())
            .map_err(|e| e.to_string())?;
        let present_modes: Vec<_> = physical
            .surface_present_modes(&surface, Default::default())
            .map_err(|e| e.to_string())?
            .collect();
        let image_format = surface_formats
            .iter()
            .find(|(format, _)| *format == Format::B8G8R8A8_UNORM)
            .map(|(format, _)| *format)
            .unwrap_or(surface_formats[0].0);
        let size = surface
            .object()
            .and_then(|object| object.downcast_ref::<Window>())
            .map(|window| window.inner_size())
            .ok_or_else(|| "surface window missing".to_string())?;
        let min_image_count = surface_caps
            .max_image_count
            .map(|limit| limit.min(surface_caps.min_image_count.max(2)))
            .unwrap_or(surface_caps.min_image_count.max(2));
        let image_usage = surface_caps.supported_usage_flags
            & (ImageUsage::COLOR_ATTACHMENT | ImageUsage::TRANSFER_DST);
        if !image_usage.intersects(ImageUsage::COLOR_ATTACHMENT) {
            return Err(format!(
                "surface does not support color-attachment swapchain images; supported usage flags: {:?}",
                surface_caps.supported_usage_flags
            ));
        }
        if !image_usage.intersects(ImageUsage::TRANSFER_DST) {
            supported_samples = SampleCounts::SAMPLE_1;
            msaa_samples = SampleCount::Sample1;
        }
        let present_mode = if present_modes.contains(&PresentMode::Immediate) {
            PresentMode::Immediate
        } else {
            PresentMode::Fifo
        };

        let (swapchain, images) = Swapchain::new(
            device.clone(),
            surface.clone(),
            SwapchainCreateInfo {
                min_image_count,
                image_format,
                image_extent: [size.width.max(1), size.height.max(1)],
                image_usage,
                composite_alpha: surface_caps
                    .supported_composite_alpha
                    .into_iter()
                    .next()
                    .ok_or_else(|| "no supported composite alpha".to_string())?,
                present_mode,
                pre_transform: surface_caps.current_transform,
                ..Default::default()
            },
        )
        .map_err(|e| {
            format!(
                "swapchain creation failed: {e}; usage={image_usage:?}; present_mode={present_mode:?}; supported_present_modes={present_modes:?}"
            )
        })?;

        let memory_allocator = Arc::new(StandardMemoryAllocator::new_default(device.clone()));
        let command_buffer_allocator =
            StandardCommandBufferAllocator::new(device.clone(), Default::default());
        let descriptor_set_allocator =
            StandardDescriptorSetAllocator::new(device.clone(), Default::default());
        let render_pass =
            Self::create_render_pass(device.clone(), swapchain.image_format(), msaa_samples)?;
        let framebuffers = Self::create_framebuffers(
            &images,
            render_pass.clone(),
            memory_allocator.clone(),
            swapchain.image_format(),
            msaa_samples,
        )?;
        let pipeline = Self::create_pipeline(
            device.clone(),
            render_pass.clone(),
            size.width,
            size.height,
            msaa_samples,
        )?;

        let nearest_sampler = Sampler::new(
            device.clone(),
            SamplerCreateInfo {
                mag_filter: Filter::Nearest,
                min_filter: Filter::Nearest,
                address_mode: [SamplerAddressMode::ClampToEdge; 3],
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
        let linear_sampler = Sampler::new(
            device.clone(),
            SamplerCreateInfo {
                mag_filter: Filter::Linear,
                min_filter: Filter::Linear,
                address_mode: [SamplerAddressMode::ClampToEdge; 3],
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;

        let mut presenter = Self {
            device: device.clone(),
            queue,
            swapchain,
            images,
            memory_allocator,
            command_buffer_allocator,
            descriptor_set_allocator,
            previous_frame_end: Some(sync::now(device).boxed()),
            render_pass,
            framebuffers,
            pipeline,
            composite_pipeline: None,
            light_map_cache: crate::lighting::LightMapCache::default(),
            light_composite_cache: None,
            recreate_swapchain: false,
            nearest_sampler,
            linear_sampler,
            supported_samples,
            msaa_samples,
            white_texture: TextureKey(0),
            texture_cache: HashMap::new(),
            shader_cache: HashMap::new(),
            image_cache_keys: HashMap::new(),
            image_cache_last_used: HashMap::new(),
            text_cache: HashMap::new(),
            text_cache_last_used: HashMap::new(),
            frame_serial: 0,
            next_texture_key: 1,
        };
        presenter.init_white_texture()?;

        Ok((presenter, surface))
    }

    fn create_render_pass(
        device: Arc<Device>,
        image_format: Format,
        msaa_samples: SampleCount,
    ) -> Result<Arc<RenderPass>, String> {
        if msaa_samples == SampleCount::Sample1 {
            return single_pass_renderpass!(
                device,
                attachments: {
                    color: {
                        format: image_format,
                        samples: 1,
                        load_op: Clear,
                        store_op: Store,
                        final_layout: ImageLayout::PresentSrc,
                    },
                    depth: {
                        format: Format::D16_UNORM,
                        samples: 1,
                        load_op: Clear,
                        store_op: DontCare,
                    }
                },
                pass: {
                    color: [color],
                    depth_stencil: {depth}
                }
            )
            .map_err(|e| e.to_string());
        }

        single_pass_renderpass!(
            device,
            attachments: {
                color_msaa: {
                    format: image_format,
                    samples: u32::from(msaa_samples),
                    load_op: Clear,
                    store_op: DontCare,
                },
                color_resolve: {
                    format: image_format,
                    samples: 1,
                    load_op: DontCare,
                    store_op: Store,
                    final_layout: ImageLayout::PresentSrc,
                },
                depth: {
                    format: Format::D16_UNORM,
                    samples: u32::from(msaa_samples),
                    load_op: Clear,
                    store_op: DontCare,
                }
            },
            pass: {
                color: [color_msaa],
                color_resolve: [color_resolve],
                depth_stencil: {depth}
            }
        )
        .map_err(|e| e.to_string())
    }

    fn create_framebuffers(
        images: &[Arc<Image>],
        render_pass: Arc<RenderPass>,
        memory_allocator: Arc<StandardMemoryAllocator>,
        image_format: Format,
        msaa_samples: SampleCount,
    ) -> Result<Vec<Arc<Framebuffer>>, String> {
        images
            .iter()
            .map(|image| {
                let swapchain_view =
                    ImageView::new_default(image.clone()).map_err(|e| e.to_string())?;
                let depth_image = Image::new(
                    memory_allocator.clone(),
                    ImageCreateInfo {
                        format: Format::D16_UNORM,
                        extent: image.extent(),
                        usage: ImageUsage::DEPTH_STENCIL_ATTACHMENT,
                        samples: msaa_samples,
                        ..Default::default()
                    },
                    AllocationCreateInfo {
                        memory_type_filter: MemoryTypeFilter::PREFER_DEVICE,
                        ..Default::default()
                    },
                )
                .map_err(|e| e.to_string())?;
                let depth_view = ImageView::new_default(depth_image).map_err(|e| e.to_string())?;
                let attachments = if msaa_samples == SampleCount::Sample1 {
                    vec![swapchain_view, depth_view]
                } else {
                    let msaa_image = Image::new(
                        memory_allocator.clone(),
                        ImageCreateInfo {
                            format: image_format,
                            extent: image.extent(),
                            usage: ImageUsage::COLOR_ATTACHMENT,
                            samples: msaa_samples,
                            ..Default::default()
                        },
                        AllocationCreateInfo {
                            memory_type_filter: MemoryTypeFilter::PREFER_DEVICE,
                            ..Default::default()
                        },
                    )
                    .map_err(|e| e.to_string())?;
                    let msaa_view =
                        ImageView::new_default(msaa_image).map_err(|e| e.to_string())?;
                    vec![msaa_view, swapchain_view, depth_view]
                };

                Framebuffer::new(
                    render_pass.clone(),
                    FramebufferCreateInfo {
                        attachments,
                        ..Default::default()
                    },
                )
                .map_err(|e| e.to_string())
            })
            .collect()
    }

    fn create_pipeline(
        device: Arc<Device>,
        render_pass: Arc<RenderPass>,
        width: u32,
        height: u32,
        msaa_samples: SampleCount,
    ) -> Result<Arc<GraphicsPipeline>, String> {
        Self::create_pipeline_with_sources(
            device,
            render_pass,
            width,
            height,
            msaa_samples,
            BUILTIN_VERTEX_SHADER,
            BUILTIN_FRAGMENT_SHADER,
            AttachmentBlend::alpha(),
        )
    }

    fn compile_shader_module(
        device: Arc<Device>,
        source: &str,
        stage: naga::ShaderStage,
        label: &str,
    ) -> Result<Arc<ShaderModule>, String> {
        let mut frontend = glsl::Frontend::default();
        let options = glsl::Options::from(stage);
        let module = frontend
            .parse(&options, source)
            .map_err(|e| format!("failed to parse {label}: {e}"))?;
        let module_info = Validator::new(ValidationFlags::all(), Capabilities::all())
            .validate(&module)
            .map_err(|e| format!("failed to validate {label}: {e}"))?;
        let pipeline_options = spv::PipelineOptions {
            shader_stage: stage,
            entry_point: "main".to_string(),
        };
        let words = spv::write_vec(
            &module,
            &module_info,
            &spv::Options::default(),
            Some(&pipeline_options),
        )
        .map_err(|e| format!("failed to generate SPIR-V for {label}: {e}"))?;
        unsafe { ShaderModule::new(device, ShaderModuleCreateInfo::new(&words)) }
            .map_err(|e| e.to_string())
    }

    fn create_pipeline_with_sources(
        device: Arc<Device>,
        render_pass: Arc<RenderPass>,
        width: u32,
        height: u32,
        msaa_samples: SampleCount,
        vertex_source: &str,
        fragment_source: &str,
        blend: AttachmentBlend,
    ) -> Result<Arc<GraphicsPipeline>, String> {
        let vs = Self::compile_shader_module(
            device.clone(),
            vertex_source,
            naga::ShaderStage::Vertex,
            "neolove_builtin_vertex.glsl",
        )?;
        let fs = Self::compile_shader_module(
            device.clone(),
            fragment_source,
            naga::ShaderStage::Fragment,
            "neolove_fragment.glsl",
        )?;
        let vs_entry = vs
            .entry_point("main")
            .ok_or_else(|| "missing vertex shader entry point".to_string())?;
        let fs_entry = fs
            .entry_point("main")
            .ok_or_else(|| "missing fragment shader entry point".to_string())?;
        let stages = [
            PipelineShaderStageCreateInfo::new(vs_entry.clone()),
            PipelineShaderStageCreateInfo::new(fs_entry.clone()),
        ];
        let layout = PipelineLayout::new(
            device.clone(),
            PipelineDescriptorSetLayoutCreateInfo::from_stages(&stages)
                .into_pipeline_layout_create_info(device.clone())
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let vertex_description = GpuVertex::per_vertex();
        let position = vertex_description
            .members
            .get("position")
            .ok_or_else(|| "vertex layout missing position field".to_string())?;
        let color = vertex_description
            .members
            .get("color")
            .ok_or_else(|| "vertex layout missing color field".to_string())?;
        let uv = vertex_description
            .members
            .get("uv")
            .ok_or_else(|| "vertex layout missing uv field".to_string())?;
        let vertex_input_state = VertexInputState::new()
            .binding(
                0,
                VertexInputBindingDescription {
                    stride: vertex_description.stride,
                    input_rate: vertex_description.input_rate,
                },
            )
            .attribute(
                0,
                VertexInputAttributeDescription {
                    binding: 0,
                    format: position.format,
                    offset: position.offset as u32,
                },
            )
            .attribute(
                1,
                VertexInputAttributeDescription {
                    binding: 0,
                    format: color.format,
                    offset: color.offset as u32,
                },
            )
            .attribute(
                2,
                VertexInputAttributeDescription {
                    binding: 0,
                    format: uv.format,
                    offset: uv.offset as u32,
                },
            );
        let subpass = Subpass::from(render_pass.clone(), 0)
            .ok_or_else(|| "missing render subpass".to_string())?;

        GraphicsPipeline::new(
            device,
            None,
            vulkano::pipeline::graphics::GraphicsPipelineCreateInfo {
                stages: stages.into_iter().collect(),
                vertex_input_state: Some(vertex_input_state),
                input_assembly_state: Some(InputAssemblyState {
                    topology: PrimitiveTopology::TriangleList,
                    ..Default::default()
                }),
                viewport_state: Some({
                    let mut state = ViewportState::default();
                    state.viewports[0] = Viewport {
                        offset: [0.0, 0.0],
                        extent: [width.max(1) as f32, height.max(1) as f32],
                        depth_range: 0.0..=1.0,
                    };
                    state
                }),
                rasterization_state: Some(RasterizationState::default()),
                multisample_state: Some(MultisampleState {
                    rasterization_samples: msaa_samples,
                    ..Default::default()
                }),
                depth_stencil_state: Some(DepthStencilState {
                    depth: Some(DepthState {
                        write_enable: true,
                        compare_op: CompareOp::LessOrEqual,
                    }),
                    ..Default::default()
                }),
                color_blend_state: Some(ColorBlendState::with_attachment_states(
                    1,
                    ColorBlendAttachmentState {
                        blend: Some(blend),
                        color_write_mask: ColorComponents::all(),
                        color_write_enable: true,
                    },
                )),
                dynamic_state: [DynamicState::Viewport].into_iter().collect(),
                subpass: Some(PipelineSubpassType::BeginRenderPass(subpass)),
                ..vulkano::pipeline::graphics::GraphicsPipelineCreateInfo::layout(layout)
            },
        )
        .map_err(|e| e.to_string())
    }

    fn create_pipeline_with_fragment_source(
        device: Arc<Device>,
        render_pass: Arc<RenderPass>,
        width: u32,
        height: u32,
        msaa_samples: SampleCount,
        fragment_source: &str,
    ) -> Result<Arc<GraphicsPipeline>, String> {
        Self::create_pipeline_with_sources(
            device,
            render_pass,
            width,
            height,
            msaa_samples,
            BUILTIN_VERTEX_SHADER,
            fragment_source,
            AttachmentBlend::alpha(),
        )
    }

    /// The multiply blend used to composite the light map: `result = src * dst`,
    /// with the destination alpha preserved.
    fn multiply_blend() -> AttachmentBlend {
        use vulkano::pipeline::graphics::color_blend::{BlendFactor, BlendOp};
        AttachmentBlend {
            src_color_blend_factor: BlendFactor::DstColor,
            dst_color_blend_factor: BlendFactor::Zero,
            color_blend_op: BlendOp::Add,
            src_alpha_blend_factor: BlendFactor::Zero,
            dst_alpha_blend_factor: BlendFactor::One,
            alpha_blend_op: BlendOp::Add,
        }
    }

    fn pipeline_for_batch(
        &mut self,
        shader: &BatchShaderState,
        width: u32,
        height: u32,
    ) -> Result<Arc<GraphicsPipeline>, String> {
        if shader.pipeline_key == 0 {
            return Ok(self.pipeline.clone());
        }
        if let Some(pipeline) = self.shader_cache.get(&shader.pipeline_key) {
            return Ok(pipeline.clone());
        }

        let pipeline = Self::create_pipeline_with_fragment_source(
            self.device.clone(),
            self.render_pass.clone(),
            width,
            height,
            self.msaa_samples,
            shader
                .fragment_source
                .as_deref()
                .ok_or_else(|| "missing fragment source for custom shader batch".to_string())?,
        )?;
        self.shader_cache
            .insert(shader.pipeline_key, pipeline.clone());
        Ok(pipeline)
    }

    fn init_white_texture(&mut self) -> Result<(), String> {
        let white = RgbaImage::from_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        let key = self.upload_rgba_texture(TextureKey(0), 0, &white)?;
        self.white_texture = key;
        Ok(())
    }

    fn recreate(&mut self, width: u32, height: u32) -> Result<(), String> {
        let (swapchain, images) = self
            .swapchain
            .recreate(SwapchainCreateInfo {
                image_extent: [width.max(1), height.max(1)],
                ..self.swapchain.create_info()
            })
            .map_err(|e| e.to_string())?;
        self.swapchain = swapchain;
        self.images = images;
        self.framebuffers = Self::create_framebuffers(
            &self.images,
            self.render_pass.clone(),
            self.memory_allocator.clone(),
            self.swapchain.image_format(),
            self.msaa_samples,
        )?;
        self.pipeline = Self::create_pipeline(
            self.device.clone(),
            self.render_pass.clone(),
            width.max(1),
            height.max(1),
            self.msaa_samples,
        )?;
        self.shader_cache.clear();
        self.composite_pipeline = None;
        self.light_composite_cache = None;
        self.recreate_swapchain = false;
        Ok(())
    }

    fn set_antialiasing(
        &mut self,
        antialiasing: Antialiasing,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let samples = preferred_sample_count(antialiasing, self.supported_samples);
        if samples == self.msaa_samples {
            return Ok(());
        }
        self.msaa_samples = samples;
        self.render_pass =
            Self::create_render_pass(self.device.clone(), self.swapchain.image_format(), samples)?;
        self.framebuffers = Self::create_framebuffers(
            &self.images,
            self.render_pass.clone(),
            self.memory_allocator.clone(),
            self.swapchain.image_format(),
            samples,
        )?;
        self.pipeline = Self::create_pipeline(
            self.device.clone(),
            self.render_pass.clone(),
            width.max(1),
            height.max(1),
            samples,
        )?;
        self.shader_cache.clear();
        self.composite_pipeline = None;
        self.light_composite_cache = None;
        Ok(())
    }

    pub(crate) fn render(
        &mut self,
        platform: &SharedPlatformState,
        render_state: &SharedRenderState,
        surface_width: u32,
        surface_height: u32,
        logical_width: u32,
        logical_height: u32,
    ) -> Result<(), String> {
        if let Some(previous) = self.previous_frame_end.as_mut() {
            previous.cleanup_finished();
        }

        if self.recreate_swapchain {
            self.recreate(surface_width, surface_height)?;
        }

        let antialiasing = lock_platform_state(platform).antialiasing();
        self.set_antialiasing(antialiasing, surface_width, surface_height)?;

        self.frame_serial = self.frame_serial.wrapping_add(1);

        let commands = renderer::drain_commands_without_remembering(render_state)?;
        let platform_clear_color = lock_platform_state(platform).clear_color();
        let (config, lights, occluders, lights_3d, environment, camera_3d) = {
            let mut state = render_state
                .lock()
                .map_err(|_| "render state lock poisoned".to_string())?;
            let (config, lights, occluders) = state.take_lighting();
            let lights_3d = state.take_lights_3d();
            let environment = state.environment_3d();
            let camera_3d = state.camera_3d();
            (
                config,
                lights,
                occluders,
                lights_3d,
                environment,
                camera_3d,
            )
        };
        let clear_color = environment_clear_color(&environment, platform_clear_color);
        let batches = self.build_batches(
            &commands,
            logical_width.max(1),
            logical_height.max(1),
            &lights_3d,
            &environment,
            camera_3d,
        )?;
        self.prune_dynamic_texture_caches();
        renderer::remember_last_frame_commands(render_state, commands)?;

        // Build the per-pixel light map and prepare its GPU composite. Any
        // failure here is non-fatal: the scene simply renders unlit rather than
        // taking down the frame.
        let light_map = crate::lighting::render_light_map_cached(
            logical_width.max(1),
            logical_height.max(1),
            &config,
            &lights,
            &occluders,
            &mut self.light_map_cache,
        );
        let light_composite = if let Some((generation, map)) = light_map {
            if let Some(cached) = self
                .light_composite_cache
                .as_ref()
                .filter(|cached| cached.generation == generation)
            {
                Some(cached.composite.clone())
            } else {
                match self.prepare_light_composite(&map) {
                    Ok(composite) => {
                        self.light_composite_cache = Some(CachedLightComposite {
                            generation,
                            composite: composite.clone(),
                        });
                        Some(composite)
                    }
                    Err(error) => {
                        warn_light_composite_once(&error);
                        None
                    }
                }
            }
        } else {
            None
        };

        let (image_index, suboptimal, acquire_future) =
            match swapchain::acquire_next_image(self.swapchain.clone(), None)
                .map_err(Validated::unwrap)
            {
                Ok(result) => result,
                Err(VulkanError::OutOfDate) => {
                    self.recreate_swapchain = true;
                    return Ok(());
                }
                Err(error) => return Err(error.to_string()),
            };

        let command_buffer = self.build_command_buffer(
            image_index as usize,
            surface_width.max(1),
            surface_height.max(1),
            clear_color,
            batches,
            light_composite,
        )?;

        let previous = self
            .previous_frame_end
            .take()
            .unwrap_or_else(|| sync::now(self.device.clone()).boxed());
        let future = previous
            .join(acquire_future)
            .then_execute(self.queue.clone(), command_buffer)
            .map_err(|e| e.to_string())?
            .then_swapchain_present(
                self.queue.clone(),
                SwapchainPresentInfo::swapchain_image_index(self.swapchain.clone(), image_index),
            )
            .then_signal_fence_and_flush();

        match future.map_err(Validated::unwrap) {
            Ok(future) => {
                // Serialize frame submission until we add per-image future tracking. This keeps
                // swapchain image/framebuffer usage ordered and avoids Vulkano validation failures.
                future.wait(None).map_err(|e| e.to_string())?;
                self.previous_frame_end = Some(sync::now(self.device.clone()).boxed());
            }
            Err(VulkanError::OutOfDate) => {
                self.recreate_swapchain = true;
                self.previous_frame_end = Some(sync::now(self.device.clone()).boxed());
            }
            Err(error) => return Err(error.to_string()),
        }

        if suboptimal {
            self.recreate_swapchain = true;
        }

        Ok(())
    }

    pub(crate) fn request_swapchain_recreate(&mut self) {
        self.recreate_swapchain = true;
    }

    /// Upload the light map and assemble everything needed to composite it: the
    /// multiply pipeline (created once), a fresh texture descriptor, and a
    /// fullscreen quad. Returns `Err` on any Vulkan failure so the caller can
    /// fall back to an unlit frame.
    fn prepare_light_composite(
        &mut self,
        map: &crate::lighting::LightMapImage,
    ) -> Result<LightComposite, String> {
        if self.composite_pipeline.is_none() {
            self.composite_pipeline = Some(Self::create_pipeline_with_sources(
                self.device.clone(),
                self.render_pass.clone(),
                1,
                1,
                self.msaa_samples,
                BUILTIN_VERTEX_SHADER,
                BUILTIN_FRAGMENT_SHADER,
                Self::multiply_blend(),
            )?);
        }
        let pipeline = self
            .composite_pipeline
            .clone()
            .ok_or_else(|| "light composite pipeline was not initialized".to_string())?;

        // Upload the light map as a texture (synchronous, like other uploads).
        let image = Image::new(
            self.memory_allocator.clone(),
            ImageCreateInfo {
                format: Format::R8G8B8A8_UNORM,
                extent: [map.width.max(1), map.height.max(1), 1],
                usage: ImageUsage::TRANSFER_DST | ImageUsage::SAMPLED,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::PREFER_DEVICE,
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
        let upload = Buffer::from_iter(
            self.memory_allocator.clone(),
            BufferCreateInfo {
                usage: BufferUsage::TRANSFER_SRC,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::PREFER_HOST
                    | MemoryTypeFilter::HOST_SEQUENTIAL_WRITE,
                ..Default::default()
            },
            map.rgba.iter().copied(),
        )
        .map_err(|e| e.to_string())?;
        let mut builder = AutoCommandBufferBuilder::primary(
            &self.command_buffer_allocator,
            self.queue.queue_family_index(),
            CommandBufferUsage::OneTimeSubmit,
        )
        .map_err(|e| e.to_string())?;
        builder
            .copy_buffer_to_image(
                vulkano::command_buffer::CopyBufferToImageInfo::buffer_image(upload, image.clone()),
            )
            .map_err(|e| e.to_string())?;
        let command_buffer = builder.build().map_err(|e| e.to_string())?;
        sync::now(self.device.clone())
            .then_execute(self.queue.clone(), command_buffer)
            .map_err(|e| e.to_string())?
            .then_signal_fence_and_flush()
            .map_err(Validated::unwrap)
            .map_err(|e| e.to_string())?
            .wait(None)
            .map_err(|e| e.to_string())?;

        let view = ImageView::new_default(image).map_err(|e| e.to_string())?;
        let layout = pipeline
            .layout()
            .set_layouts()
            .first()
            .cloned()
            .ok_or_else(|| "composite pipeline missing descriptor set layout".to_string())?;
        let descriptor = PersistentDescriptorSet::new(
            &self.descriptor_set_allocator,
            layout,
            [
                WriteDescriptorSet::image_view(0, view),
                WriteDescriptorSet::sampler(1, self.linear_sampler.clone()),
            ],
            [],
        )
        .map_err(|e| e.to_string())?;

        // Fullscreen quad. Positions follow the same convention as the scene
        // (NDC +1 is screen top); UVs map the light map top-left to screen
        // top-left. A linear sampler upscales the downsampled map smoothly.
        let white = [1.0f32; 4];
        let verts = [
            GpuVertex {
                position: [-1.0, 1.0, 0.0, 1.0],
                color: white,
                uv: [0.0, 0.0],
            },
            GpuVertex {
                position: [1.0, 1.0, 0.0, 1.0],
                color: white,
                uv: [1.0, 0.0],
            },
            GpuVertex {
                position: [1.0, -1.0, 0.0, 1.0],
                color: white,
                uv: [1.0, 1.0],
            },
            GpuVertex {
                position: [-1.0, 1.0, 0.0, 1.0],
                color: white,
                uv: [0.0, 0.0],
            },
            GpuVertex {
                position: [1.0, -1.0, 0.0, 1.0],
                color: white,
                uv: [1.0, 1.0],
            },
            GpuVertex {
                position: [-1.0, -1.0, 0.0, 1.0],
                color: white,
                uv: [0.0, 1.0],
            },
        ];
        let vertex_buffer = Buffer::from_iter(
            self.memory_allocator.clone(),
            BufferCreateInfo {
                usage: BufferUsage::VERTEX_BUFFER,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::PREFER_HOST
                    | MemoryTypeFilter::HOST_SEQUENTIAL_WRITE,
                ..Default::default()
            },
            verts,
        )
        .map_err(|e| e.to_string())?;

        Ok(LightComposite {
            pipeline,
            descriptor,
            vertex_buffer,
        })
    }

    fn build_command_buffer(
        &mut self,
        image_index: usize,
        width: u32,
        height: u32,
        clear: Color,
        batches: Vec<TextureBatch>,
        light_composite: Option<LightComposite>,
    ) -> Result<Arc<PrimaryAutoCommandBuffer>, String> {
        let mut builder = AutoCommandBufferBuilder::primary(
            &self.command_buffer_allocator,
            self.queue.queue_family_index(),
            CommandBufferUsage::OneTimeSubmit,
        )
        .map_err(|e| e.to_string())?;

        let clear_values = if self.msaa_samples == SampleCount::Sample1 {
            vec![
                Some(ClearValue::Float([
                    clear.r as f32 / 255.0,
                    clear.g as f32 / 255.0,
                    clear.b as f32 / 255.0,
                    clear.a as f32 / 255.0,
                ])),
                Some(ClearValue::Depth(1.0)),
            ]
        } else {
            vec![
                Some(ClearValue::Float([
                    clear.r as f32 / 255.0,
                    clear.g as f32 / 255.0,
                    clear.b as f32 / 255.0,
                    clear.a as f32 / 255.0,
                ])),
                None,
                Some(ClearValue::Depth(1.0)),
            ]
        };

        builder
            .begin_render_pass(
                RenderPassBeginInfo {
                    clear_values,
                    ..RenderPassBeginInfo::framebuffer(self.framebuffers[image_index].clone())
                },
                SubpassBeginInfo {
                    contents: SubpassContents::Inline,
                    ..Default::default()
                },
            )
            .map_err(|e| e.to_string())?;

        builder
            .set_viewport(
                0,
                [Viewport {
                    offset: [0.0, 0.0],
                    extent: [width as f32, height as f32],
                    depth_range: 0.0..=1.0,
                }]
                .into_iter()
                .collect(),
            )
            .map_err(|e| e.to_string())?;

        for batch in batches {
            if batch.vertices.is_empty() {
                continue;
            }
            let pipeline = self.pipeline_for_batch(&batch.shader, width, height)?;
            let descriptor = self.descriptor_for_batch(
                pipeline.clone(),
                batch.texture,
                batch.filter,
                &batch.shader,
            )?;
            let vertex_count = batch.vertices.len() as u32;
            let vertex_buffer = Buffer::from_iter(
                self.memory_allocator.clone(),
                BufferCreateInfo {
                    usage: BufferUsage::VERTEX_BUFFER,
                    ..Default::default()
                },
                AllocationCreateInfo {
                    memory_type_filter: MemoryTypeFilter::PREFER_HOST
                        | MemoryTypeFilter::HOST_SEQUENTIAL_WRITE,
                    ..Default::default()
                },
                batch.vertices,
            )
            .map_err(|e| e.to_string())?;

            builder
                .bind_pipeline_graphics(pipeline.clone())
                .map_err(|e| e.to_string())?
                .bind_descriptor_sets(
                    PipelineBindPoint::Graphics,
                    pipeline.layout().clone(),
                    0,
                    descriptor,
                )
                .map_err(|e| e.to_string())?
                .bind_vertex_buffers(0, vertex_buffer)
                .map_err(|e| e.to_string())?
                .draw(vertex_count, 1, 0, 0)
                .map_err(|e| e.to_string())?;
        }

        // Composite the light map over the finished scene (multiply blend).
        if let Some(composite) = light_composite {
            builder
                .bind_pipeline_graphics(composite.pipeline.clone())
                .map_err(|e| e.to_string())?
                .bind_descriptor_sets(
                    PipelineBindPoint::Graphics,
                    composite.pipeline.layout().clone(),
                    0,
                    composite.descriptor.clone(),
                )
                .map_err(|e| e.to_string())?
                .bind_vertex_buffers(0, composite.vertex_buffer.clone())
                .map_err(|e| e.to_string())?
                .draw(6, 1, 0, 0)
                .map_err(|e| e.to_string())?;
        }

        builder
            .end_render_pass(SubpassEndInfo::default())
            .map_err(|e| e.to_string())?;
        builder.build().map_err(|e| e.to_string())
    }

    fn descriptor_for_batch(
        &self,
        pipeline: Arc<GraphicsPipeline>,
        texture: TextureKey,
        filter: TextureFilter,
        shader: &BatchShaderState,
    ) -> Result<Arc<PersistentDescriptorSet>, String> {
        if shader.pipeline_key == 0 {
            return self
                .descriptor_for(texture, filter)
                .ok_or_else(|| "missing cached texture descriptor".to_string());
        }

        let layout = pipeline
            .layout()
            .set_layouts()
            .first()
            .cloned()
            .ok_or_else(|| "pipeline missing descriptor set layout".to_string())?;
        let sampler = match filter {
            TextureFilter::Nearest => self.nearest_sampler.clone(),
            TextureFilter::Linear => self.linear_sampler.clone(),
        };
        let base_texture = self
            .texture_cache
            .get(&texture)
            .ok_or_else(|| "missing cached base texture".to_string())?;

        let mut writes = vec![
            WriteDescriptorSet::image_view(0, base_texture.view.clone()),
            WriteDescriptorSet::sampler(1, sampler.clone()),
        ];

        if shader.uses_uniform_buffer {
            let uniform_buffer = Buffer::from_data(
                self.memory_allocator.clone(),
                BufferCreateInfo {
                    usage: BufferUsage::UNIFORM_BUFFER,
                    ..Default::default()
                },
                AllocationCreateInfo {
                    memory_type_filter: MemoryTypeFilter::PREFER_HOST
                        | MemoryTypeFilter::HOST_SEQUENTIAL_WRITE,
                    ..Default::default()
                },
                ShaderUniformBuffer {
                    slots: shader.uniform_slots,
                },
            )
            .map_err(|e| e.to_string())?;
            writes.push(WriteDescriptorSet::buffer(2, uniform_buffer));
        }

        for (binding, texture_key) in &shader.extra_textures {
            let cached = self
                .texture_cache
                .get(texture_key)
                .ok_or_else(|| format!("missing cached texture for shader binding {binding}"))?;
            writes.push(WriteDescriptorSet::image_view(
                *binding,
                cached.view.clone(),
            ));
            writes.push(WriteDescriptorSet::sampler(*binding + 1, sampler.clone()));
        }

        PersistentDescriptorSet::new(&self.descriptor_set_allocator, layout, writes, [])
            .map_err(|e| e.to_string())
    }

    fn descriptor_for(
        &self,
        texture: TextureKey,
        filter: TextureFilter,
    ) -> Option<Arc<PersistentDescriptorSet>> {
        let cached = self.texture_cache.get(&texture)?;
        Some(match filter {
            TextureFilter::Nearest => cached.descriptor_nearest.clone(),
            TextureFilter::Linear => cached.descriptor_linear.clone(),
        })
    }

    fn build_batches(
        &mut self,
        commands: &[DrawCommand],
        width: u32,
        height: u32,
        lights_3d: &[crate::render3d::Light3D],
        environment: &crate::environment3d::Environment3D,
        camera_3d: crate::render3d::Camera3D,
    ) -> Result<Vec<TextureBatch>, String> {
        let mut batches = Vec::with_capacity(commands.len().min(64));
        let mut current: Option<TextureBatch> = None;

        if let Some(background) = self.environment_batch(environment, camera_3d, width, height) {
            batches.push(background);
        }

        for command in commands {
            if !renderer::command_intersects_viewport(&command, width, height) {
                continue;
            }
            match command {
                DrawCommand::Rect {
                    x,
                    y,
                    w,
                    h,
                    rotation,
                    offset,
                    color,
                    shader,
                } => {
                    let (x, y, w, h, rotation, offset, color) =
                        (*x, *y, *w, *h, *rotation, *offset, *color);
                    let pivot_x = x + w * offset.x;
                    let pivot_y = y + h * offset.y;
                    let verts = quad_vertices(
                        width,
                        height,
                        [
                            world_point(x, y, pivot_x, pivot_y, rotation),
                            world_point(x + w, y, pivot_x, pivot_y, rotation),
                            world_point(x + w, y + h, pivot_x, pivot_y, rotation),
                            world_point(x, y + h, pivot_x, pivot_y, rotation),
                        ],
                        [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                        color,
                    );
                    let shader = self.batch_shader_for_command(shader.as_ref())?;
                    push_vertices(
                        &mut current,
                        &mut batches,
                        self.white_texture,
                        TextureFilter::Nearest,
                        shader,
                        verts,
                    );
                }
                DrawCommand::Triangle {
                    a,
                    b,
                    c,
                    color,
                    shader,
                } => {
                    let shader = self.batch_shader_for_command(shader.as_ref())?;
                    let color = *color;
                    push_vertices(
                        &mut current,
                        &mut batches,
                        self.white_texture,
                        TextureFilter::Nearest,
                        shader,
                        [
                            vertex_from_point(width, height, *a, color, [0.0, 0.0]),
                            vertex_from_point(width, height, *b, color, [1.0, 0.0]),
                            vertex_from_point(width, height, *c, color, [0.5, 1.0]),
                        ],
                    );
                }
                DrawCommand::Circle {
                    center,
                    radius,
                    color,
                    shader,
                } => {
                    let (center, radius, color) = (*center, *radius, *color);
                    let segments =
                        ((radius * std::f32::consts::TAU / 4.0).ceil() as usize).clamp(24, 128);
                    let mut verts = Vec::with_capacity(segments * 3);
                    for index in 0..segments {
                        let a0 = index as f32 / segments as f32 * std::f32::consts::TAU;
                        let a1 = (index + 1) as f32 / segments as f32 * std::f32::consts::TAU;
                        let p0 = center;
                        let p1 = Vec2 {
                            x: center.x + a0.cos() * radius,
                            y: center.y + a0.sin() * radius,
                        };
                        let p2 = Vec2 {
                            x: center.x + a1.cos() * radius,
                            y: center.y + a1.sin() * radius,
                        };
                        verts.push(vertex_from_point(width, height, p0, color, [0.5, 0.5]));
                        verts.push(vertex_from_point(width, height, p1, color, [1.0, 0.0]));
                        verts.push(vertex_from_point(width, height, p2, color, [0.0, 1.0]));
                    }
                    let shader = self.batch_shader_for_command(shader.as_ref())?;
                    push_vertices(
                        &mut current,
                        &mut batches,
                        self.white_texture,
                        TextureFilter::Nearest,
                        shader,
                        verts,
                    );
                }
                DrawCommand::Image {
                    image,
                    dest,
                    source,
                    rotation,
                    pivot,
                    tint,
                    filter,
                    shader,
                } => {
                    let texture = self.texture_for_image(image)?;
                    let uv = image_uvs(image, *source)?;
                    let corners = image_corners(*dest, *rotation, *pivot);
                    let verts = quad_vertices(width, height, corners, uv, *tint);
                    let shader = self.batch_shader_for_command(shader.as_ref())?;
                    push_vertices(&mut current, &mut batches, texture, *filter, shader, verts);
                }
                DrawCommand::Mesh3D(command) => {
                    let texture = match command.texture.as_ref() {
                        Some(image) => self.texture_for_image(image)?,
                        None => self.white_texture,
                    };
                    let filter = if command.texture.is_some() {
                        TextureFilter::Linear
                    } else {
                        TextureFilter::Nearest
                    };
                    let shader = self.batch_shader_for_command(command.shader.as_ref())?;
                    let triangles = crate::render3d::project_mesh(command, lights_3d)?;
                    let vertices = triangles.into_iter().flat_map(|triangle| {
                        triangle.vertices.into_iter().map(|vertex| GpuVertex {
                            position: vertex.clip_position,
                            color: vertex.color,
                            uv: vertex.uv,
                        })
                    });
                    push_vertices(
                        &mut current,
                        &mut batches,
                        texture,
                        filter,
                        shader,
                        vertices,
                    );
                }
                DrawCommand::Particles3D(command) => {
                    let texture = match command.texture.as_ref() {
                        Some(image) => self.texture_for_image(image)?,
                        None => self.white_texture,
                    };
                    let vertices = crate::render3d::project_particles(command)?
                        .into_iter()
                        .flat_map(|triangle| {
                            triangle.vertices.into_iter().map(|vertex| GpuVertex {
                                position: vertex.clip_position,
                                color: vertex.color,
                                uv: vertex.uv,
                            })
                        });
                    push_vertices(
                        &mut current,
                        &mut batches,
                        texture,
                        command.filter,
                        BatchShaderState::default_pipeline(),
                        vertices,
                    );
                }
                DrawCommand::Text(request) => {
                    let Some(sprite) = renderer::rasterize_text_sprite(request) else {
                        continue;
                    };
                    let texture = self.texture_for_text(request, sprite.image.as_ref())?;
                    let corners = image_corners(sprite.dest, sprite.rotation, sprite.pivot);
                    let verts = quad_vertices(
                        width,
                        height,
                        corners,
                        [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                        Color::WHITE,
                    );
                    push_vertices(
                        &mut current,
                        &mut batches,
                        texture,
                        sprite.filter,
                        BatchShaderState::default_pipeline(),
                        verts,
                    );
                }
            }
        }

        if let Some(batch) = current.take() {
            batches.push(batch);
        }
        Ok(batches)
    }

    fn environment_batch(
        &mut self,
        environment: &crate::environment3d::Environment3D,
        camera: crate::render3d::Camera3D,
        width: u32,
        height: u32,
    ) -> Option<TextureBatch> {
        use crate::environment3d::EnvironmentMode3D;

        if !environment.enabled || environment.mode == EnvironmentMode3D::Solid {
            return None;
        }
        let rgba = |color: Color| {
            let color = environment_scaled_color(color, environment.intensity);
            [
                color.r as f32 / 255.0,
                color.g as f32 / 255.0,
                color.b as f32 / 255.0,
                color.a as f32 / 255.0,
            ]
        };
        let fullscreen = |top: [f32; 4], bottom: [f32; 4]| {
            vec![
                GpuVertex {
                    position: [-1.0, 1.0, 1.0, 1.0],
                    color: top,
                    uv: [0.0, 0.0],
                },
                GpuVertex {
                    position: [1.0, 1.0, 1.0, 1.0],
                    color: top,
                    uv: [1.0, 0.0],
                },
                GpuVertex {
                    position: [1.0, -1.0, 1.0, 1.0],
                    color: bottom,
                    uv: [1.0, 1.0],
                },
                GpuVertex {
                    position: [-1.0, 1.0, 1.0, 1.0],
                    color: top,
                    uv: [0.0, 0.0],
                },
                GpuVertex {
                    position: [1.0, -1.0, 1.0, 1.0],
                    color: bottom,
                    uv: [1.0, 1.0],
                },
                GpuVertex {
                    position: [-1.0, -1.0, 1.0, 1.0],
                    color: bottom,
                    uv: [0.0, 1.0],
                },
            ]
        };
        let white_texture = self.white_texture;
        let gradient = || TextureBatch {
            texture: white_texture,
            filter: TextureFilter::Linear,
            vertices: fullscreen(rgba(environment.top), rgba(environment.bottom)),
            shader: BatchShaderState::default_pipeline(),
        };

        if environment.mode != EnvironmentMode3D::Equirectangular {
            return Some(gradient());
        }
        let Some(image) = environment.equirectangular.as_ref() else {
            return Some(gradient());
        };
        let Ok(texture) = self.texture_for_image(image) else {
            return Some(gradient());
        };
        let rotation = crate::render3d::Mat4::rotation_euler_degrees(camera.euler);
        let right = rotation.transform_direction(crate::render3d::Vec3::new(1.0, 0.0, 0.0));
        let up = rotation.transform_direction(crate::render3d::Vec3::new(0.0, 1.0, 0.0));
        let forward = rotation.transform_direction(crate::render3d::Vec3::new(0.0, 0.0, -1.0));
        let aspect = width.max(1) as f32 / height.max(1) as f32;
        let half_height = (camera.fov.clamp(1.0, 179.0).to_radians() * 0.5).tan();
        let yaw = environment.rotation_degrees.to_radians();
        let (yaw_sin, yaw_cos) = yaw.sin_cos();
        let mut uniform_slots = [[0.0; 4]; crate::shader::MAX_SHADER_FLOAT_UNIFORMS];
        uniform_slots[0] = [right.x, right.y, right.z, 0.0];
        uniform_slots[1] = [up.x, up.y, up.z, 0.0];
        uniform_slots[2] = [forward.x, forward.y, forward.z, 0.0];
        uniform_slots[3] = [half_height * aspect, half_height, yaw_sin, yaw_cos];
        uniform_slots[4][0] = environment.intensity.max(0.0);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        EQUIRECTANGULAR_ENVIRONMENT_FRAGMENT_SHADER.hash(&mut hasher);
        Some(TextureBatch {
            texture,
            filter: TextureFilter::Linear,
            vertices: fullscreen([1.0; 4], [1.0; 4]),
            shader: BatchShaderState {
                pipeline_key: hasher.finish(),
                fragment_source: Some(
                    EQUIRECTANGULAR_ENVIRONMENT_FRAGMENT_SHADER.to_string(),
                ),
                uses_uniform_buffer: true,
                uniform_slots,
                extra_textures: Vec::new(),
            },
        })
    }

    fn batch_shader_for_command(
        &mut self,
        shader: Option<&crate::shader::ShaderHandle>,
    ) -> Result<BatchShaderState, String> {
        let Some(shader) = shader else {
            return Ok(BatchShaderState::default_pipeline());
        };

        let snapshot = shader.snapshot_for_runtime()?;
        let mut extra_textures = Vec::with_capacity(snapshot.texture_bindings.len());
        for (binding, image) in snapshot.texture_bindings {
            let texture = self.texture_for_image(&image)?;
            extra_textures.push((binding, texture));
        }

        Ok(BatchShaderState {
            pipeline_key: snapshot.pipeline_key,
            fragment_source: Some(snapshot.fragment_source),
            uses_uniform_buffer: snapshot.uses_uniform_buffer,
            uniform_slots: snapshot.uniform_slots,
            extra_textures,
        })
    }

    fn texture_for_image(&mut self, image: &ImageHandle) -> Result<TextureKey, String> {
        let id = image.id().map_err(|e| e.to_string())?;
        self.image_cache_last_used.insert(id, self.frame_serial);
        let revision = image.revision().map_err(|e| e.to_string())?;
        if let Some(key) = self.image_cache_keys.get(&id).copied()
            && let Some(cached) = self.texture_cache.get(&key)
            && cached.revision == revision
        {
            return Ok(key);
        }

        let key = self
            .image_cache_keys
            .get(&id)
            .copied()
            .unwrap_or_else(|| self.allocate_texture_key());
        let rgba = image.clone_rgba_image().map_err(|e| e.to_string())?;
        let key = self.upload_rgba_texture(key, revision, &rgba)?;
        self.image_cache_keys.insert(id, key);
        Ok(key)
    }

    fn texture_for_text(
        &mut self,
        request: &renderer::TextRenderRequest,
        rgba: &RgbaImage,
    ) -> Result<TextureKey, String> {
        let hash = renderer::text_render_request_cache_id(request);
        self.text_cache_last_used.insert(hash, self.frame_serial);
        if let Some(key) = self.text_cache.get(&hash).copied() {
            return Ok(key);
        }
        let key = self.allocate_texture_key();
        let key = self.upload_rgba_texture(key, 0, rgba)?;
        self.text_cache.insert(hash, key);
        Ok(key)
    }

    fn prune_dynamic_texture_caches(&mut self) {
        let frame = self.frame_serial;

        let mut stale_images: Vec<(usize, u64)> = self
            .image_cache_last_used
            .iter()
            .filter_map(|(&id, &last_used)| {
                (frame.wrapping_sub(last_used) > GPU_TEXTURE_IDLE_FRAMES).then_some((id, last_used))
            })
            .collect();
        if self
            .image_cache_keys
            .len()
            .saturating_sub(stale_images.len())
            > GPU_IMAGE_CACHE_LIMIT
        {
            let mut remaining: Vec<(usize, u64)> = self
                .image_cache_last_used
                .iter()
                .filter_map(|(&id, &last_used)| {
                    (last_used != frame
                        && !stale_images.iter().any(|(stale_id, _)| *stale_id == id))
                    .then_some((id, last_used))
                })
                .collect();
            remaining.sort_unstable_by_key(|(_, last_used)| *last_used);
            let excess = self
                .image_cache_keys
                .len()
                .saturating_sub(stale_images.len())
                .saturating_sub(GPU_IMAGE_CACHE_LIMIT);
            stale_images.extend(remaining.into_iter().take(excess));
        }
        for (id, _) in stale_images {
            self.image_cache_last_used.remove(&id);
            if let Some(key) = self.image_cache_keys.remove(&id) {
                self.texture_cache.remove(&key);
            }
        }

        let mut stale_text: Vec<(u64, u64)> = self
            .text_cache_last_used
            .iter()
            .filter_map(|(&hash, &last_used)| {
                (frame.wrapping_sub(last_used) > GPU_TEXTURE_IDLE_FRAMES)
                    .then_some((hash, last_used))
            })
            .collect();
        if self.text_cache.len().saturating_sub(stale_text.len()) > GPU_TEXT_CACHE_LIMIT {
            let mut remaining: Vec<(u64, u64)> = self
                .text_cache_last_used
                .iter()
                .filter_map(|(&hash, &last_used)| {
                    (last_used != frame
                        && !stale_text.iter().any(|(stale_hash, _)| *stale_hash == hash))
                    .then_some((hash, last_used))
                })
                .collect();
            remaining.sort_unstable_by_key(|(_, last_used)| *last_used);
            let excess = self
                .text_cache
                .len()
                .saturating_sub(stale_text.len())
                .saturating_sub(GPU_TEXT_CACHE_LIMIT);
            stale_text.extend(remaining.into_iter().take(excess));
        }
        for (hash, _) in stale_text {
            self.text_cache_last_used.remove(&hash);
            if let Some(key) = self.text_cache.remove(&hash) {
                self.texture_cache.remove(&key);
            }
        }
    }

    fn allocate_texture_key(&mut self) -> TextureKey {
        let key = TextureKey(self.next_texture_key);
        self.next_texture_key = self.next_texture_key.wrapping_add(1);
        key
    }

    fn upload_rgba_texture(
        &mut self,
        key: TextureKey,
        revision: u64,
        rgba: &RgbaImage,
    ) -> Result<TextureKey, String> {
        let image = Image::new(
            self.memory_allocator.clone(),
            ImageCreateInfo {
                format: Format::R8G8B8A8_UNORM,
                extent: [rgba.width().max(1), rgba.height().max(1), 1],
                usage: ImageUsage::TRANSFER_DST | ImageUsage::SAMPLED,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::PREFER_DEVICE,
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
        let upload = Buffer::from_iter(
            self.memory_allocator.clone(),
            BufferCreateInfo {
                usage: BufferUsage::TRANSFER_SRC,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::PREFER_HOST
                    | MemoryTypeFilter::HOST_SEQUENTIAL_WRITE,
                ..Default::default()
            },
            rgba.as_raw().iter().copied(),
        )
        .map_err(|e| e.to_string())?;
        let mut builder = AutoCommandBufferBuilder::primary(
            &self.command_buffer_allocator,
            self.queue.queue_family_index(),
            CommandBufferUsage::OneTimeSubmit,
        )
        .map_err(|e| e.to_string())?;
        builder
            .copy_buffer_to_image(
                vulkano::command_buffer::CopyBufferToImageInfo::buffer_image(upload, image.clone()),
            )
            .map_err(|e| e.to_string())?;
        let command_buffer = builder.build().map_err(|e| e.to_string())?;
        sync::now(self.device.clone())
            .then_execute(self.queue.clone(), command_buffer)
            .map_err(|e| e.to_string())?
            .then_signal_fence_and_flush()
            .map_err(Validated::unwrap)
            .map_err(|e| e.to_string())?
            .wait(None)
            .map_err(|e| e.to_string())?;

        let view = ImageView::new_default(image).map_err(|e| e.to_string())?;
        let layout = self
            .pipeline
            .layout()
            .set_layouts()
            .first()
            .cloned()
            .ok_or_else(|| "pipeline missing descriptor set layout".to_string())?;
        let descriptor_nearest = PersistentDescriptorSet::new(
            &self.descriptor_set_allocator,
            layout.clone(),
            [
                WriteDescriptorSet::image_view(0, view.clone()),
                WriteDescriptorSet::sampler(1, self.nearest_sampler.clone()),
            ],
            [],
        )
        .map_err(|e| e.to_string())?;
        let descriptor_linear = PersistentDescriptorSet::new(
            &self.descriptor_set_allocator,
            layout,
            [
                WriteDescriptorSet::image_view(0, view.clone()),
                WriteDescriptorSet::sampler(1, self.linear_sampler.clone()),
            ],
            [],
        )
        .map_err(|e| e.to_string())?;
        self.texture_cache.insert(
            key,
            CachedTexture {
                revision,
                view,
                descriptor_nearest,
                descriptor_linear,
            },
        );
        Ok(key)
    }
}

fn preferred_sample_count(antialiasing: Antialiasing, supported: SampleCounts) -> SampleCount {
    match antialiasing {
        Antialiasing::Off => SampleCount::Sample1,
        Antialiasing::Standard if supported.intersects(SampleCounts::SAMPLE_2) => {
            SampleCount::Sample2
        }
        Antialiasing::High if supported.intersects(SampleCounts::SAMPLE_4) => SampleCount::Sample4,
        Antialiasing::High if supported.intersects(SampleCounts::SAMPLE_2) => SampleCount::Sample2,
        Antialiasing::Standard | Antialiasing::High => SampleCount::Sample1,
    }
}

fn push_vertices(
    current: &mut Option<TextureBatch>,
    batches: &mut Vec<TextureBatch>,
    texture: TextureKey,
    filter: TextureFilter,
    shader: BatchShaderState,
    vertices: impl IntoIterator<Item = GpuVertex>,
) {
    match current {
        Some(batch)
            if batch.texture == texture && batch.filter == filter && batch.shader == shader =>
        {
            batch.vertices.extend(vertices);
        }
        Some(_) => {
            let finished_batch = current
                .take()
                .expect("current batch must exist before starting a new one");
            batches.push(finished_batch);
            *current = Some(TextureBatch {
                texture,
                filter,
                vertices: vertices.into_iter().collect(),
                shader,
            });
        }
        None => {
            *current = Some(TextureBatch {
                texture,
                filter,
                vertices: vertices.into_iter().collect(),
                shader,
            });
        }
    }
}

fn world_point(x: f32, y: f32, pivot_x: f32, pivot_y: f32, rotation: f32) -> Vec2 {
    let local_x = x - pivot_x;
    let local_y = y - pivot_y;
    let cos_r = rotation.cos();
    let sin_r = rotation.sin();
    Vec2 {
        x: pivot_x + local_x * cos_r - local_y * sin_r,
        y: pivot_y + local_x * sin_r + local_y * cos_r,
    }
}

fn image_corners(dest: Rect, rotation: f32, pivot: Vec2) -> [Vec2; 4] {
    [
        world_point(dest.x, dest.y, pivot.x, pivot.y, rotation),
        world_point(dest.x + dest.w, dest.y, pivot.x, pivot.y, rotation),
        world_point(dest.x + dest.w, dest.y + dest.h, pivot.x, pivot.y, rotation),
        world_point(dest.x, dest.y + dest.h, pivot.x, pivot.y, rotation),
    ]
}

fn quad_vertices(
    width: u32,
    height: u32,
    corners: [Vec2; 4],
    uv: [[f32; 2]; 4],
    color: Color,
) -> [GpuVertex; 6] {
    [
        vertex_from_point(width, height, corners[0], color, uv[0]),
        vertex_from_point(width, height, corners[1], color, uv[1]),
        vertex_from_point(width, height, corners[2], color, uv[2]),
        vertex_from_point(width, height, corners[0], color, uv[0]),
        vertex_from_point(width, height, corners[2], color, uv[2]),
        vertex_from_point(width, height, corners[3], color, uv[3]),
    ]
}

fn vertex_from_point(
    width: u32,
    height: u32,
    point: Vec2,
    color: Color,
    uv: [f32; 2],
) -> GpuVertex {
    let width = width.max(1) as f32;
    let height = height.max(1) as f32;
    GpuVertex {
        position: [
            point.x / width * 2.0 - 1.0,
            1.0 - point.y / height * 2.0,
            0.0,
            1.0,
        ],
        color: [
            color.r as f32 / 255.0,
            color.g as f32 / 255.0,
            color.b as f32 / 255.0,
            color.a as f32 / 255.0,
        ],
        uv,
    }
}

fn image_uvs(image: &ImageHandle, source: Option<Rect>) -> Result<[[f32; 2]; 4], String> {
    let (img_w, img_h) = image.dimensions().map_err(|e| e.to_string())?;
    let source = source.unwrap_or(Rect {
        x: 0.0,
        y: 0.0,
        w: img_w as f32,
        h: img_h as f32,
    });
    let u0 = source.x / img_w.max(1) as f32;
    let v0 = source.y / img_h.max(1) as f32;
    let u1 = (source.x + source.w) / img_w.max(1) as f32;
    let v1 = (source.y + source.h) / img_h.max(1) as f32;
    Ok([[u0, v0], [u1, v0], [u1, v1], [u0, v1]])
}

#[cfg(test)]
mod environment_shader_tests {
    use super::*;

    #[test]
    fn global_antialiasing_selects_supported_vulkan_msaa_samples() {
        let supported = SampleCounts::SAMPLE_1
            | SampleCounts::SAMPLE_2
            | SampleCounts::SAMPLE_4
            | SampleCounts::SAMPLE_8;
        assert_eq!(
            preferred_sample_count(Antialiasing::Off, supported),
            SampleCount::Sample1
        );
        assert_eq!(
            preferred_sample_count(Antialiasing::Standard, supported),
            SampleCount::Sample2
        );
        assert_eq!(
            preferred_sample_count(Antialiasing::High, supported),
            SampleCount::Sample4
        );
        assert_eq!(
            preferred_sample_count(Antialiasing::High, SampleCounts::SAMPLE_1),
            SampleCount::Sample1
        );
    }

    #[test]
    fn equirectangular_environment_shader_parses_and_validates() {
        let mut frontend = glsl::Frontend::default();
        let module = frontend
            .parse(
                &glsl::Options::from(naga::ShaderStage::Fragment),
                EQUIRECTANGULAR_ENVIRONMENT_FRAGMENT_SHADER,
            )
            .expect("environment fragment shader should parse");
        Validator::new(ValidationFlags::all(), Capabilities::all())
            .validate(&module)
            .expect("environment fragment shader should validate");
    }
}
