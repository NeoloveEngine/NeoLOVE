use crate::assets::ImageHandle;
use crate::platform::{Color, SharedPlatformState};
use crate::renderer::{self, DrawCommand, Rect, SharedRenderState, TextureFilter, Vec2};
use bytemuck::{Pod, Zeroable};
use image::RgbaImage;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use vulkano::buffer::{Buffer, BufferCreateInfo, BufferUsage};
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
use vulkano::pipeline::graphics::input_assembly::{InputAssemblyState, PrimitiveTopology};
use vulkano::pipeline::graphics::multisample::MultisampleState;
use vulkano::pipeline::graphics::rasterization::RasterizationState;
use vulkano::pipeline::graphics::subpass::PipelineSubpassType;
use vulkano::pipeline::graphics::vertex_input::{Vertex, VertexDefinition};
use vulkano::pipeline::graphics::viewport::{Viewport, ViewportState};
use vulkano::pipeline::layout::PipelineDescriptorSetLayoutCreateInfo;
use vulkano::pipeline::{
    DynamicState, GraphicsPipeline, Pipeline, PipelineBindPoint, PipelineLayout,
    PipelineShaderStageCreateInfo,
};
use vulkano::render_pass::{Framebuffer, FramebufferCreateInfo, RenderPass, Subpass};
use vulkano::swapchain::{
    self, PresentMode, Surface, Swapchain, SwapchainCreateInfo, SwapchainPresentInfo,
};
use vulkano::sync::{self, GpuFuture};
use vulkano::{Validated, VulkanError};
use vulkano::{Version, VulkanLibrary, single_pass_renderpass};
use winit::event_loop::EventLoop;
use winit::window::Window;

mod vs {
    vulkano_shaders::shader! {
        ty: "vertex",
        src: r#"
            #version 450
            layout(location = 0) in vec2 position;
            layout(location = 1) in vec4 color;
            layout(location = 2) in vec2 uv;

            layout(location = 0) out vec4 v_color;
            layout(location = 1) out vec2 v_uv;

            void main() {
                gl_Position = vec4(position, 0.0, 1.0);
                v_color = color;
                v_uv = uv;
            }
        "#,
    }
}

mod fs {
    vulkano_shaders::shader! {
        ty: "fragment",
        src: r#"
            #version 450
            layout(set = 0, binding = 0) uniform sampler2D tex;

            layout(location = 0) in vec4 v_color;
            layout(location = 1) in vec2 v_uv;
            layout(location = 0) out vec4 f_color;

            void main() {
                f_color = texture(tex, v_uv) * v_color;
            }
        "#,
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Zeroable, Pod, Vertex)]
struct GpuVertex {
    #[format(R32G32_SFLOAT)]
    position: [f32; 2],
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
}

struct CachedTexture {
    revision: u64,
    descriptor_nearest: Arc<PersistentDescriptorSet>,
    descriptor_linear: Arc<PersistentDescriptorSet>,
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
    recreate_swapchain: bool,
    nearest_sampler: Arc<Sampler>,
    linear_sampler: Arc<Sampler>,
    msaa_samples: SampleCount,
    white_texture: TextureKey,
    texture_cache: HashMap<TextureKey, CachedTexture>,
    image_cache_keys: HashMap<usize, TextureKey>,
    text_cache: HashMap<u64, TextureKey>,
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

        let msaa_samples = if physical
            .properties()
            .framebuffer_color_sample_counts
            .intersects(SampleCounts::SAMPLE_4)
        {
            SampleCount::Sample4
        } else if physical
            .properties()
            .framebuffer_color_sample_counts
            .intersects(SampleCounts::SAMPLE_2)
        {
            SampleCount::Sample2
        } else {
            SampleCount::Sample1
        };

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
        let msaa_samples = if image_usage.intersects(ImageUsage::TRANSFER_DST) {
            msaa_samples
        } else {
            SampleCount::Sample1
        };
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
            recreate_swapchain: false,
            nearest_sampler,
            linear_sampler,
            msaa_samples,
            white_texture: TextureKey(0),
            texture_cache: HashMap::new(),
            image_cache_keys: HashMap::new(),
            text_cache: HashMap::new(),
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
                    }
                },
                pass: {
                    color: [color],
                    depth_stencil: {}
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
                }
            },
            pass: {
                color: [color_msaa],
                color_resolve: [color_resolve],
                depth_stencil: {}
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
                let attachments = if msaa_samples == SampleCount::Sample1 {
                    vec![swapchain_view]
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
                    vec![msaa_view, swapchain_view]
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
        let vs = vs::load(device.clone()).map_err(|e| e.to_string())?;
        let fs = fs::load(device.clone()).map_err(|e| e.to_string())?;
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
        let vertex_input_state = GpuVertex::per_vertex()
            .definition(&vs_entry.info().input_interface)
            .map_err(|e| e.to_string())?;
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
                color_blend_state: Some(ColorBlendState::with_attachment_states(
                    1,
                    ColorBlendAttachmentState {
                        blend: Some(AttachmentBlend::alpha()),
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
        self.recreate_swapchain = false;
        Ok(())
    }

    pub(crate) fn render(
        &mut self,
        platform: &SharedPlatformState,
        render_state: &SharedRenderState,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        if let Some(previous) = self.previous_frame_end.as_mut() {
            previous.cleanup_finished();
        }

        if self.recreate_swapchain {
            self.recreate(width, height)?;
        }

        let commands = renderer::drain_commands(render_state)?;
        let clear_color = platform
            .lock()
            .map_err(|_| "platform lock poisoned".to_string())?
            .clear_color();
        let batches = self.build_batches(commands, width.max(1), height.max(1))?;

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
            width.max(1),
            height.max(1),
            clear_color,
            batches,
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

    fn build_command_buffer(
        &self,
        image_index: usize,
        width: u32,
        height: u32,
        clear: Color,
        batches: Vec<TextureBatch>,
    ) -> Result<Arc<PrimaryAutoCommandBuffer>, String> {
        let mut builder = AutoCommandBufferBuilder::primary(
            &self.command_buffer_allocator,
            self.queue.queue_family_index(),
            CommandBufferUsage::OneTimeSubmit,
        )
        .map_err(|e| e.to_string())?;

        let clear_values = if self.msaa_samples == SampleCount::Sample1 {
            vec![Some(ClearValue::Float([
                clear.r as f32 / 255.0,
                clear.g as f32 / 255.0,
                clear.b as f32 / 255.0,
                clear.a as f32 / 255.0,
            ]))]
        } else {
            vec![
                Some(ClearValue::Float([
                    clear.r as f32 / 255.0,
                    clear.g as f32 / 255.0,
                    clear.b as f32 / 255.0,
                    clear.a as f32 / 255.0,
                ])),
                None,
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
        builder
            .bind_pipeline_graphics(self.pipeline.clone())
            .map_err(|e| e.to_string())?;

        for batch in batches {
            if batch.vertices.is_empty() {
                continue;
            }
            let descriptor = self
                .descriptor_for(batch.texture, batch.filter)
                .ok_or_else(|| "missing cached texture descriptor".to_string())?;
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
                batch.vertices.into_iter(),
            )
            .map_err(|e| e.to_string())?;

            builder
                .bind_descriptor_sets(
                    PipelineBindPoint::Graphics,
                    self.pipeline.layout().clone(),
                    0,
                    descriptor,
                )
                .map_err(|e| e.to_string())?
                .bind_vertex_buffers(0, vertex_buffer)
                .map_err(|e| e.to_string())?
                .draw(vertex_count, 1, 0, 0)
                .map_err(|e| e.to_string())?;
        }

        builder
            .end_render_pass(SubpassEndInfo::default())
            .map_err(|e| e.to_string())?;
        builder.build().map_err(|e| e.to_string())
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
        commands: Vec<DrawCommand>,
        width: u32,
        height: u32,
    ) -> Result<Vec<TextureBatch>, String> {
        let mut batches = Vec::new();
        let mut current: Option<TextureBatch> = None;

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
                } => {
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
                    push_vertices(
                        &mut current,
                        &mut batches,
                        self.white_texture,
                        TextureFilter::Nearest,
                        verts,
                    );
                }
                DrawCommand::Triangle { a, b, c, color } => {
                    push_vertices(
                        &mut current,
                        &mut batches,
                        self.white_texture,
                        TextureFilter::Nearest,
                        vec![
                            vertex_from_point(width, height, a, color, [0.0, 0.0]),
                            vertex_from_point(width, height, b, color, [1.0, 0.0]),
                            vertex_from_point(width, height, c, color, [0.5, 1.0]),
                        ],
                    );
                }
                DrawCommand::Circle {
                    center,
                    radius,
                    color,
                } => {
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
                    push_vertices(
                        &mut current,
                        &mut batches,
                        self.white_texture,
                        TextureFilter::Nearest,
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
                } => {
                    let texture = self.texture_for_image(&image)?;
                    let uv = image_uvs(&image, source)?;
                    let corners = image_corners(dest, rotation, pivot);
                    let verts = quad_vertices(width, height, corners, uv, tint);
                    push_vertices(&mut current, &mut batches, texture, filter, verts);
                }
                DrawCommand::Text(request) => {
                    let Some(sprite) = renderer::rasterize_text_sprite(&request) else {
                        continue;
                    };
                    let texture = self.texture_for_text(&request, &sprite.image)?;
                    let corners = image_corners(sprite.dest, sprite.rotation, sprite.pivot);
                    let verts = quad_vertices(
                        width,
                        height,
                        corners,
                        [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                        Color::WHITE,
                    );
                    push_vertices(&mut current, &mut batches, texture, sprite.filter, verts);
                }
            }
        }

        if let Some(batch) = current.take() {
            batches.push(batch);
        }
        Ok(batches)
    }

    fn texture_for_image(&mut self, image: &ImageHandle) -> Result<TextureKey, String> {
        let id = image.id();
        let revision = image.revision().map_err(|e| e.to_string())?;
        if let Some(key) = self.image_cache_keys.get(&id).copied() {
            if let Some(cached) = self.texture_cache.get(&key) {
                if cached.revision == revision {
                    return Ok(key);
                }
            }
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
        let mut hasher = DefaultHasher::new();
        request.text.hash(&mut hasher);
        match &request.font {
            renderer::FontHandle::Default => "__neolove_default_font__".hash(&mut hasher),
            renderer::FontHandle::Path(path) => path.hash(&mut hasher),
        }
        request.scale.to_bits().hash(&mut hasher);
        request.min_scale.to_bits().hash(&mut hasher);
        match request.text_scale {
            renderer::TextScaleMode::None => 0u8,
            renderer::TextScaleMode::Fit => 1u8,
            renderer::TextScaleMode::FitWidth => 2u8,
            renderer::TextScaleMode::FitHeight => 3u8,
        }
        .hash(&mut hasher);
        match request.align_x {
            renderer::TextAlignX::Left => 0u8,
            renderer::TextAlignX::Center => 1u8,
            renderer::TextAlignX::Right => 2u8,
        }
        .hash(&mut hasher);
        match request.align_y {
            renderer::TextAlignY::Top => 0u8,
            renderer::TextAlignY::Center => 1u8,
            renderer::TextAlignY::Bottom => 2u8,
        }
        .hash(&mut hasher);
        match request.wrap {
            renderer::TextWrapMode::None => 0u8,
            renderer::TextWrapMode::Word => 1u8,
            renderer::TextWrapMode::Char => 2u8,
        }
        .hash(&mut hasher);
        request.bounds.x.to_bits().hash(&mut hasher);
        request.bounds.y.to_bits().hash(&mut hasher);
        request.bounds.w.to_bits().hash(&mut hasher);
        request.bounds.h.to_bits().hash(&mut hasher);
        request.padding_x.to_bits().hash(&mut hasher);
        request.padding_y.to_bits().hash(&mut hasher);
        request.line_spacing.to_bits().hash(&mut hasher);
        request.letter_spacing.to_bits().hash(&mut hasher);
        request.stretch_width.to_bits().hash(&mut hasher);
        request.stretch_height.to_bits().hash(&mut hasher);
        [
            request.color.r,
            request.color.g,
            request.color.b,
            request.color.a,
        ]
        .hash(&mut hasher);
        let hash = hasher.finish();
        if let Some(key) = self.text_cache.get(&hash).copied() {
            return Ok(key);
        }
        let key = self.allocate_texture_key();
        let key = self.upload_rgba_texture(key, 0, rgba)?;
        self.text_cache.insert(hash, key);
        Ok(key)
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
            [WriteDescriptorSet::image_view_sampler(
                0,
                view.clone(),
                self.nearest_sampler.clone(),
            )],
            [],
        )
        .map_err(|e| e.to_string())?;
        let descriptor_linear = PersistentDescriptorSet::new(
            &self.descriptor_set_allocator,
            layout,
            [WriteDescriptorSet::image_view_sampler(
                0,
                view.clone(),
                self.linear_sampler.clone(),
            )],
            [],
        )
        .map_err(|e| e.to_string())?;
        self.texture_cache.insert(
            key,
            CachedTexture {
                revision,
                descriptor_nearest,
                descriptor_linear,
            },
        );
        Ok(key)
    }
}

fn push_vertices(
    current: &mut Option<TextureBatch>,
    batches: &mut Vec<TextureBatch>,
    texture: TextureKey,
    filter: TextureFilter,
    vertices: Vec<GpuVertex>,
) {
    match current {
        Some(batch) if batch.texture == texture && batch.filter == filter => {
            batch.vertices.extend(vertices);
        }
        Some(_) => {
            batches.push(current.take().unwrap());
            *current = Some(TextureBatch {
                texture,
                filter,
                vertices,
            });
        }
        None => {
            *current = Some(TextureBatch {
                texture,
                filter,
                vertices,
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
) -> Vec<GpuVertex> {
    vec![
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
        position: [point.x / width * 2.0 - 1.0, point.y / height * 2.0 - 1.0],
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
