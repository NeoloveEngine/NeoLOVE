use crate::assets::{CubemapHandle, ImageHandle};
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
use vulkano::buffer::{Buffer, BufferContents, BufferCreateInfo, BufferUsage, Subbuffer};
use vulkano::command_buffer::allocator::StandardCommandBufferAllocator;
use vulkano::command_buffer::{
    AutoCommandBufferBuilder, CommandBufferUsage, CopyBufferInfo, CopyImageToBufferInfo,
    PrimaryAutoCommandBuffer, RenderPassBeginInfo, SubpassBeginInfo, SubpassContents,
    SubpassEndInfo,
};
use vulkano::descriptor_set::allocator::StandardDescriptorSetAllocator;
use vulkano::descriptor_set::{PersistentDescriptorSet, WriteDescriptorSet};
use vulkano::device::{Device, Queue};
use vulkano::format::{ClearValue, Format};
use vulkano::image::sampler::{Filter, Sampler, SamplerAddressMode, SamplerCreateInfo};
use vulkano::image::view::{ImageView, ImageViewCreateInfo, ImageViewType};
use vulkano::image::{
    Image, ImageCreateFlags, ImageCreateInfo, ImageLayout, ImageUsage, SampleCount, SampleCounts,
};
use vulkano::memory::allocator::{AllocationCreateInfo, MemoryTypeFilter, StandardMemoryAllocator};
use vulkano::pipeline::graphics::color_blend::{
    AttachmentBlend, ColorBlendAttachmentState, ColorBlendState, ColorComponents,
};
use vulkano::pipeline::graphics::depth_stencil::{CompareOp, DepthState, DepthStencilState};
use vulkano::pipeline::graphics::input_assembly::{InputAssemblyState, PrimitiveTopology};
use vulkano::pipeline::graphics::multisample::MultisampleState;
use vulkano::pipeline::graphics::rasterization::{CullMode, FrontFace, RasterizationState};
use vulkano::pipeline::graphics::subpass::PipelineSubpassType;
use vulkano::pipeline::graphics::vertex_input::{
    Vertex, VertexInputAttributeDescription, VertexInputBindingDescription, VertexInputRate,
    VertexInputState,
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
    vec4 sampled = texture(sampler2D(Texture, TextureSampler), uv);
    vec3 linear_sample = pow(max(sampled.rgb, vec3(0.0)), vec3(2.2));
    vec3 linear_tint = pow(max(color.rgb, vec3(0.0)), vec3(2.2));
    f_color = vec4(linear_sample * linear_tint, sampled.a * color.a);
}
"#;

// Light maps store linear multipliers, unlike ordinary authored RGBA images.
// Decoding them as sRGB would make the multiply composite far too dark.
const LIGHT_COMPOSITE_FRAGMENT_SHADER: &str = r#"#version 450
layout(binding = 0) uniform texture2D Texture;
layout(binding = 1) uniform sampler TextureSampler;

layout(location = 0) in vec4 color;
layout(location = 1) in vec2 uv;
layout(location = 0) out vec4 f_color;

void main() {
    f_color = texture(sampler2D(Texture, TextureSampler), uv) * color;
}
"#;

const HDR_SCENE_FORMAT: Format = Format::R16G16B16A16_SFLOAT;

const TONEMAP_FRAGMENT_SHADER: &str = r#"#version 450
layout(binding = 0) uniform texture2D HdrScene;
layout(binding = 1) uniform sampler HdrSampler;
layout(binding = 2) uniform TonemapUniforms {
    vec4 settings;
};
layout(binding = 3) uniform texture2D BloomScene;
layout(binding = 4) uniform sampler BloomSampler;

layout(location = 0) in vec4 color;
layout(location = 1) in vec2 uv;
layout(location = 0) out vec4 f_color;

void main() {
    vec4 hdr = texture(sampler2D(HdrScene, HdrSampler), uv);
    vec3 bloom = texture(sampler2D(BloomScene, BloomSampler), uv).rgb;
    vec3 mapped = max(hdr.rgb + bloom * max(settings.w, 0.0), vec3(0.0))
        * exp2(clamp(settings.x, -24.0, 24.0));
    int tonemap_operator = int(settings.y + 0.5);
    if (tonemap_operator == 1) {
        mapped = mapped / (vec3(1.0) + mapped);
    } else if (tonemap_operator == 2) {
        mapped = clamp(
            (mapped * (2.51 * mapped + vec3(0.03)))
                / (mapped * (2.43 * mapped + vec3(0.59)) + vec3(0.14)),
            vec3(0.0), vec3(1.0)
        );
    }
    float gamma = clamp(settings.z, 0.1, 8.0);
    f_color = vec4(pow(mapped, vec3(1.0 / gamma)), hdr.a);
}
"#;

const BLOOM_EXTRACT_FRAGMENT_SHADER: &str = r#"#version 450
layout(binding = 0) uniform texture2D Source;
layout(binding = 1) uniform sampler SourceSampler;
layout(binding = 2) uniform BloomUniforms {
    vec4 settings;
};

layout(location = 0) in vec4 color;
layout(location = 1) in vec2 uv;
layout(location = 0) out vec4 f_color;

void main() {
    vec2 texel = 1.0 / vec2(textureSize(sampler2D(Source, SourceSampler), 0));
    vec3 source = (
        texture(sampler2D(Source, SourceSampler), uv + texel * vec2(-0.5, -0.5)).rgb
        + texture(sampler2D(Source, SourceSampler), uv + texel * vec2(0.5, -0.5)).rgb
        + texture(sampler2D(Source, SourceSampler), uv + texel * vec2(-0.5, 0.5)).rgb
        + texture(sampler2D(Source, SourceSampler), uv + texel * vec2(0.5, 0.5)).rgb
    ) * 0.25;
    float luminance = dot(source, vec3(0.2126, 0.7152, 0.0722));
    float threshold = clamp(settings.x, 0.0, 1.0);
    float contribution = clamp(
        (luminance - threshold) / max(1.0 - threshold, 1.0 / 255.0),
        0.0,
        1.0
    );
    f_color = vec4(source * contribution, 1.0);
}
"#;

const BLOOM_BLUR_FRAGMENT_SHADER: &str = r#"#version 450
layout(binding = 0) uniform texture2D Source;
layout(binding = 1) uniform sampler SourceSampler;
layout(binding = 2) uniform BloomUniforms {
    vec4 settings;
};

layout(location = 0) in vec4 color;
layout(location = 1) in vec2 uv;
layout(location = 0) out vec4 f_color;

void main() {
    vec2 texel = 1.0 / vec2(textureSize(sampler2D(Source, SourceSampler), 0));
    vec2 direction = settings.xy * texel;
    int radius = int(clamp(settings.z, 1.0, 32.0) + 0.5);
    vec3 total = vec3(0.0);
    float weight = 0.0;
    for (int offset = -32; offset <= 32; ++offset) {
        if (abs(offset) > radius) {
            continue;
        }
        // Tent weights avoid the hard edge of a box kernel while preserving
        // the bounded radius and separability of the reference implementation.
        float sample_weight = float(radius + 1 - abs(offset));
        total += texture(
            sampler2D(Source, SourceSampler), uv + direction * float(offset)
        ).rgb * sample_weight;
        weight += sample_weight;
    }
    f_color = vec4(total / max(weight, 1.0), 1.0);
}
"#;

const MAX_NATIVE_MESH_LIGHTS: usize = 64;
const NATIVE_MESH_LIGHT_BASE_SLOT: usize = 16;
const MAX_NATIVE_SKIN_JOINTS: usize = 256;
const NATIVE_SHADOW_MAP_SIZE: u32 = 2048;
const NATIVE_MESH_SKIN_BASE_SLOT: usize =
    NATIVE_MESH_LIGHT_BASE_SLOT + MAX_NATIVE_MESH_LIGHTS * 4;
const NATIVE_MESH_FOG_BASE_SLOT: usize =
    NATIVE_MESH_SKIN_BASE_SLOT + MAX_NATIVE_SKIN_JOINTS * 4;
const NATIVE_MESH_AO_CONFIG_SLOT: usize = NATIVE_MESH_FOG_BASE_SLOT + 2;
const NATIVE_MESH_AO_OCCLUDER_BASE_SLOT: usize = NATIVE_MESH_AO_CONFIG_SLOT + 1;
const NATIVE_MESH_REFLECTION_PROBE_SLOT: usize =
    NATIVE_MESH_AO_OCCLUDER_BASE_SLOT + crate::render3d::MAX_AMBIENT_OCCLUDERS_3D * 2;
const NATIVE_MESH_UNIFORM_SLOTS: usize = NATIVE_MESH_REFLECTION_PROBE_SLOT + 1;

const NATIVE_MESH_VERTEX_SHADER: &str = r#"#version 450
layout(location = 0) in vec3 position;
layout(location = 1) in vec3 normal;
layout(location = 2) in vec2 uv;
layout(location = 3) in vec4 tangent;
layout(location = 4) in uvec4 joints;
layout(location = 5) in vec4 weights;
layout(location = 6) in vec4 instance_model_0;
layout(location = 7) in vec4 instance_model_1;
layout(location = 8) in vec4 instance_model_2;
layout(location = 9) in vec4 instance_model_3;
layout(location = 10) in vec4 instance_normal_0;
layout(location = 11) in vec4 instance_normal_1;
layout(location = 12) in vec4 instance_normal_2;
layout(location = 13) in vec4 instance_tint;

layout(binding = 2) uniform MeshUniforms {
    vec4 mesh_slots[1364];
};

layout(location = 0) out vec3 v_world_position;
layout(location = 1) out vec3 v_world_normal;
layout(location = 2) out vec3 v_world_tangent;
layout(location = 3) out float v_tangent_sign;
layout(location = 4) out vec2 v_uv;
layout(location = 5) out vec4 v_tint;

mat4 skin_matrix(uint joint) {
    int base = 272 + int(min(joint, 255u)) * 4;
    return mat4(
        mesh_slots[base], mesh_slots[base + 1],
        mesh_slots[base + 2], mesh_slots[base + 3]
    );
}

void main() {
    mat4 model = mat4(
        instance_model_0, instance_model_1, instance_model_2, instance_model_3
    );
    mat4 view_projection = mat4(
        mesh_slots[4], mesh_slots[5], mesh_slots[6], mesh_slots[7]
    );
    mat3 normal_matrix = mat3(
        instance_normal_0.xyz, instance_normal_1.xyz, instance_normal_2.xyz
    );
    vec3 local_position = position;
    vec3 local_normal = normal;
    vec3 local_tangent = tangent.xyz;
    float weight_sum = weights.x + weights.y + weights.z + weights.w;
    if (mesh_slots[15].y > 0.5 && weight_sum > 0.000001) {
        mat4 skin = skin_matrix(joints.x) * weights.x
            + skin_matrix(joints.y) * weights.y
            + skin_matrix(joints.z) * weights.z
            + skin_matrix(joints.w) * weights.w;
        local_position = (skin * vec4(position, 1.0)).xyz;
        local_normal = mat3(skin) * normal;
        local_tangent = mat3(skin) * tangent.xyz;
    }
    vec3 world_position = (model * vec4(local_position, 1.0)).xyz;
    vec3 transformed_normal = normal_matrix * local_normal;
    vec3 world_normal = transformed_normal
        * inversesqrt(max(dot(transformed_normal, transformed_normal), 0.00000001));
    vec3 transformed_tangent = normal_matrix * local_tangent;
    v_world_position = world_position;
    v_world_normal = world_normal;
    v_world_tangent = transformed_tangent
        * inversesqrt(max(dot(transformed_tangent, transformed_tangent), 0.00000001));
    v_tangent_sign = tangent.w;
    v_uv = uv;
    v_tint = instance_tint;
    gl_Position = view_projection * model * vec4(local_position, 1.0);
}
"#;

const NATIVE_MESH_FRAGMENT_SHADER: &str = r#"#version 450
layout(binding = 0) uniform texture2D BaseColorTexture;
layout(binding = 1) uniform sampler BaseColorSampler;
layout(binding = 2) uniform MeshUniforms {
    vec4 mesh_slots[1364];
};
layout(binding = 3) uniform texture2D NormalTexture;
layout(binding = 4) uniform sampler NormalSampler;
layout(binding = 5) uniform texture2D MetallicRoughnessTexture;
layout(binding = 6) uniform sampler MetallicRoughnessSampler;
layout(binding = 7) uniform texture2D EmissiveTexture;
layout(binding = 8) uniform sampler EmissiveSampler;
layout(binding = 9) uniform texture2D ShadowMap;
layout(binding = 10) uniform sampler ShadowSampler;
layout(binding = 11) uniform texture2D EnvironmentMap;
layout(binding = 12) uniform sampler EnvironmentSampler;
layout(binding = 13) uniform textureCube EnvironmentCubemap;
layout(binding = 14) uniform sampler EnvironmentCubemapSampler;
layout(binding = 15) uniform textureCube ReflectionProbeCubemap;
layout(binding = 16) uniform sampler ReflectionProbeSampler;

layout(location = 0) in vec3 world_position;
layout(location = 1) in vec3 world_normal_input;
layout(location = 2) in vec3 world_tangent_input;
layout(location = 3) in float tangent_sign;
layout(location = 4) in vec2 uv;
layout(location = 5) in vec4 tint;
layout(location = 0) out vec4 f_color;

vec3 sample_environment(vec3 input_direction) {
    vec3 direction = normalize(input_direction);
    float yaw = mesh_slots[15].w;
    float yaw_sin = sin(yaw);
    float yaw_cos = cos(yaw);
    direction = vec3(
        direction.x * yaw_cos - direction.z * yaw_sin,
        direction.y,
        direction.x * yaw_sin + direction.z * yaw_cos
    );
    vec3 encoded;
    if (mesh_slots[12].z > 1.5) {
        encoded = texture(
            samplerCube(EnvironmentCubemap, EnvironmentCubemapSampler), direction
        ).rgb;
    } else {
        vec2 panorama_uv = vec2(
            fract(atan(direction.z, direction.x) / 6.28318530718 + 0.5),
            clamp(0.5 - asin(clamp(direction.y, -1.0, 1.0)) / 3.14159265359, 0.0, 1.0)
        );
        encoded = texture(
            sampler2D(EnvironmentMap, EnvironmentSampler), panorama_uv
        ).rgb;
    }
    vec3 global_environment = pow(max(encoded, vec3(0.0)), vec3(2.2))
        * max(mesh_slots[12].w, 0.0);
    vec4 probe = mesh_slots[1363];
    if (probe.w < 0.5 || probe.x <= 0.0) {
        return global_environment;
    }
    vec3 probe_direction = normalize(input_direction);
    float probe_sin = sin(probe.z);
    float probe_cos = cos(probe.z);
    probe_direction = vec3(
        probe_direction.x * probe_cos - probe_direction.z * probe_sin,
        probe_direction.y,
        probe_direction.x * probe_sin + probe_direction.z * probe_cos
    );
    vec3 local_environment = pow(max(texture(
        samplerCube(ReflectionProbeCubemap, ReflectionProbeSampler), probe_direction
    ).rgb, vec3(0.0)), vec3(2.2)) * max(probe.y, 0.0);
    return mix(global_environment, local_environment, clamp(probe.x, 0.0, 1.0));
}

vec3 sample_environment_lobe(vec3 direction, float spread) {
    direction = normalize(direction);
    vec3 helper = abs(direction.y) < 0.95 ? vec3(0.0, 1.0, 0.0) : vec3(1.0, 0.0, 0.0);
    vec3 tangent = normalize(cross(helper, direction));
    vec3 bitangent = cross(direction, tangent);
    float bounded_spread = clamp(spread, 0.0, 1.0);
    vec3 total = sample_environment(direction) * 4.0;
    total += sample_environment(normalize(direction + tangent * bounded_spread));
    total += sample_environment(normalize(direction - tangent * bounded_spread));
    total += sample_environment(normalize(direction + bitangent * bounded_spread));
    total += sample_environment(normalize(direction - bitangent * bounded_spread));
    return total * 0.125;
}

float ambient_visibility(vec3 normal_value) {
    int count = int(clamp(mesh_slots[1298].x, 0.0, 32.0));
    float intensity = clamp(mesh_slots[1298].y, 0.0, 1.0);
    float radius = max(mesh_slots[1298].z, 0.001);
    float bias = max(mesh_slots[1298].w, 0.0);
    float visibility = 1.0;
    for (int index = 0; index < 32; ++index) {
        if (index >= count) {
            break;
        }
        int base = 1299 + index * 2;
        vec3 minimum = mesh_slots[base].xyz;
        vec3 maximum = mesh_slots[base + 1].xyz;
        vec3 center = (minimum + maximum) * 0.5;
        vec3 closest_delta = clamp(world_position, minimum, maximum) - world_position;
        float distance_to_bounds = length(closest_delta);
        if (distance_to_bounds > radius) {
            continue;
        }
        vec3 center_delta = center - world_position;
        float center_distance = length(center_delta);
        vec3 direction = distance_to_bounds > max(bias, 0.000001)
            ? closest_delta / distance_to_bounds
            : center_delta / max(center_distance, 0.000001);
        float alignment = max(dot(normal_value, direction), 0.0);
        if (alignment <= 0.0) {
            continue;
        }
        float extent = max(length((maximum - minimum) * 0.5), 0.0001);
        float angular_size = clamp(
            extent / (extent + max(distance_to_bounds, bias)), 0.0, 1.0
        );
        float proximity = clamp(
            1.0 - max(distance_to_bounds - bias, 0.0) / max(radius - bias, 0.0001),
            0.0,
            1.0
        );
        float occlusion = alignment * proximity * proximity * angular_size * intensity;
        visibility *= 1.0 - clamp(occlusion, 0.0, intensity);
    }
    return clamp(max(visibility, 1.0 - intensity), 0.0, 1.0);
}

void main() {
    vec4 sampled_base = texture(sampler2D(BaseColorTexture, BaseColorSampler), uv);
    sampled_base.rgb = pow(max(sampled_base.rgb, vec3(0.0)), vec3(2.2));
    // MeshRenderer3D tint is a final renderer modulation, matching Scene View
    // and the software runtime. Keep it out of the dielectric Fresnel term so
    // white specular energy cannot bypass a pure-red (or otherwise channel-
    // constrained) authored tint under bright lights.
    vec4 base = sampled_base * mesh_slots[9];
    base.a *= tint.a;
    float alpha_mode = mesh_slots[12].y;
    if (alpha_mode > 0.5 && alpha_mode < 1.5 && base.a < mesh_slots[11].y) {
        discard;
    }

    vec3 normal_value = world_normal_input
        * inversesqrt(max(dot(world_normal_input, world_normal_input), 0.00000001));
    vec3 tangent_value = world_tangent_input
        - normal_value * dot(normal_value, world_tangent_input);
    tangent_value *= inversesqrt(max(dot(tangent_value, tangent_value), 0.00000001));
    if (!gl_FrontFacing) {
        normal_value = -normal_value;
        tangent_value = -tangent_value;
    }
    if (mesh_slots[11].z > 0.5) {
        vec3 mapped = texture(sampler2D(NormalTexture, NormalSampler), uv).xyz * 2.0 - 1.0;
        mapped *= inversesqrt(max(dot(mapped, mapped), 0.00000001));
        vec3 bitangent = cross(normal_value, tangent_value) * tangent_sign;
        normal_value = mat3(tangent_value, bitangent, normal_value) * mapped;
        normal_value *= inversesqrt(max(dot(normal_value, normal_value), 0.00000001));
    }

    float metallic = clamp(mesh_slots[10].w, 0.0, 1.0);
    float roughness = clamp(mesh_slots[11].x, 0.045, 1.0);
    if (mesh_slots[11].w > 0.5) {
        vec4 packed = texture(
            sampler2D(MetallicRoughnessTexture, MetallicRoughnessSampler), uv
        );
        roughness = clamp(roughness * packed.g, 0.045, 1.0);
        metallic = clamp(metallic * packed.b, 0.0, 1.0);
    }

    vec3 view_delta = mesh_slots[13].xyz - world_position;
    vec3 view_direction = view_delta
        * inversesqrt(max(dot(view_delta, view_delta), 0.00000001));
    float n_dot_v = max(dot(normal_value, view_direction), 0.0001);
    vec3 f0 = mix(vec3(0.04), base.rgb, metallic);
    vec3 outgoing = mesh_slots[12].z > 0.5
        ? vec3(0.0)
        : vec3(0.03) * base.rgb * (1.0 - metallic);
    int light_count = int(clamp(mesh_slots[15].x, 0.0, 64.0));

    if (light_count == 0 && mesh_slots[12].z < 0.5) {
        vec3 headlight_direction = normalize(vec3(0.25, 0.45, 1.0));
        float headlight = max(dot(normal_value, headlight_direction), 0.0);
        outgoing += base.rgb * (0.35 + headlight * 0.53);
    }

    for (int light_index = 0; light_index < 64; ++light_index) {
        if (light_index >= light_count) {
            break;
        }
        int light_base = 16 + light_index * 4;
        vec4 position_kind = mesh_slots[light_base];
        vec4 direction_intensity = mesh_slots[light_base + 1];
        vec4 color_range = mesh_slots[light_base + 2];
        vec4 spot = mesh_slots[light_base + 3];
        vec3 light_direction;
        float attenuation = 1.0;
        if (position_kind.w < 0.5) {
            vec3 raw_direction = -direction_intensity.xyz;
            light_direction = raw_direction
                * inversesqrt(max(dot(raw_direction, raw_direction), 0.00000001));
        } else {
            vec3 delta = position_kind.xyz - world_position;
            float distance_squared = dot(delta, delta);
            float range = max(color_range.w, 0.0001);
            float normalized_distance = clamp(sqrt(distance_squared) / range, 0.0, 1.0);
            attenuation = pow(1.0 - normalized_distance * normalized_distance, 2.0);
            light_direction = delta
                * inversesqrt(max(distance_squared, 0.00000001));
        }
        if (position_kind.w > 1.5) {
            vec3 from_light = -light_direction;
            vec3 raw_spot_direction = direction_intensity.xyz;
            vec3 spot_direction = raw_spot_direction
                * inversesqrt(max(dot(raw_spot_direction, raw_spot_direction), 0.00000001));
            float alignment = dot(spot_direction, from_light);
            float outer = cos(max(spot.x, 0.001) * 0.5);
            float softness = clamp(spot.y, 0.0, 0.999);
            float inner = min(outer + (1.0 - outer) * (1.0 - softness), 1.0);
            float cone = inner <= outer + 0.000001
                ? (alignment >= outer ? 1.0 : 0.0)
                : clamp((alignment - outer) / (inner - outer), 0.0, 1.0);
            attenuation *= cone;
        }

        float n_dot_l = max(dot(normal_value, light_direction), 0.0);
        vec3 half_delta = view_direction + light_direction;
        vec3 half_direction = half_delta
            * inversesqrt(max(dot(half_delta, half_delta), 0.00000001));
        float n_dot_h = max(dot(normal_value, half_direction), 0.0);
        float h_dot_v = max(dot(half_direction, view_direction), 0.0);
        float alpha = roughness * roughness;
        float alpha_squared = alpha * alpha;
        float distribution_denominator =
            n_dot_h * n_dot_h * (alpha_squared - 1.0) + 1.0;
        float distribution = alpha_squared / max(
            3.14159265359 * distribution_denominator * distribution_denominator,
            0.000001
        );
        float geometry_k = (roughness + 1.0) * (roughness + 1.0) * 0.125;
        float geometry_view = n_dot_v / (n_dot_v * (1.0 - geometry_k) + geometry_k);
        float geometry_light = n_dot_l / (n_dot_l * (1.0 - geometry_k) + geometry_k);
        vec3 fresnel = f0 + (vec3(1.0) - f0) * pow(1.0 - h_dot_v, 5.0);
        vec3 specular = distribution * geometry_view * geometry_light * fresnel
            / max(4.0 * n_dot_v * n_dot_l, 0.0001);
        vec3 diffuse_weight = (vec3(1.0) - fresnel) * (1.0 - metallic);
        float visibility = 1.0;
        if (mesh_slots[14].x > 0.5 && mesh_slots[14].w > 0.5
                && light_index == int(mesh_slots[14].y + 0.5)) {
            mat4 shadow_view_projection = mat4(
                mesh_slots[0], mesh_slots[1], mesh_slots[2], mesh_slots[3]
            );
            vec4 shadow_clip = shadow_view_projection * vec4(world_position, 1.0);
            vec3 shadow_ndc = shadow_clip.xyz / max(shadow_clip.w, 0.000001);
            vec2 shadow_uv = shadow_ndc.xy * 0.5 + 0.5;
            if (shadow_ndc.z >= 0.0 && shadow_ndc.z <= 1.0
                    && all(greaterThanEqual(shadow_uv, vec2(0.0)))
                    && all(lessThanEqual(shadow_uv, vec2(1.0)))) {
                float receiver_depth = shadow_ndc.z - max(mesh_slots[14].z, 0.0);
                vec2 texel = 1.0 / vec2(textureSize(sampler2D(ShadowMap, ShadowSampler), 0));
                visibility = 0.0;
                for (int shadow_y = -1; shadow_y <= 1; ++shadow_y) {
                    for (int shadow_x = -1; shadow_x <= 1; ++shadow_x) {
                        float stored_depth = texture(
                            sampler2D(ShadowMap, ShadowSampler),
                            shadow_uv + vec2(shadow_x, shadow_y) * texel
                        ).r;
                        visibility += receiver_depth <= stored_depth ? 1.0 : 0.0;
                    }
                }
                visibility /= 9.0;
            }
        }
        vec3 radiance = color_range.rgb * max(direction_intensity.w, 0.0)
            * attenuation * visibility;
        outgoing += (diffuse_weight * base.rgb / 3.14159265359 + specular)
            * radiance * n_dot_l;
    }

    if (mesh_slots[12].z > 0.5) {
        vec3 diffuse_environment = sample_environment_lobe(normal_value, 0.8);
        vec3 reflection_direction = reflect(-view_direction, normal_value);
        vec3 specular_environment = sample_environment_lobe(
            reflection_direction, roughness * roughness
        );
        vec3 environment_fresnel = f0 + (vec3(1.0) - f0) * pow(1.0 - n_dot_v, 5.0);
        outgoing += diffuse_environment * base.rgb * (1.0 - metallic) * 0.35;
        outgoing += specular_environment * environment_fresnel;
    }

    outgoing *= ambient_visibility(normal_value);
    vec3 emissive = mesh_slots[10].xyz;
    if (mesh_slots[12].x > 0.5) {
        vec3 sampled_emissive = texture(
            sampler2D(EmissiveTexture, EmissiveSampler), uv
        ).rgb;
        emissive *= pow(max(sampled_emissive, vec3(0.0)), vec3(2.2));
    }
    vec3 final_color = max(outgoing + emissive, vec3(0.0)) * tint.rgb;
    if (mesh_slots[1296].w > 0.5) {
        float fog_distance = length(view_delta);
        float fog_mode = mesh_slots[1297].w;
        float fog_amount;
        if (fog_mode < 0.5) {
            float fog_start = mesh_slots[1297].x;
            float fog_end = max(mesh_slots[1297].y, fog_start + 0.0001);
            fog_amount = clamp((fog_distance - fog_start) / (fog_end - fog_start), 0.0, 1.0);
        } else if (fog_mode < 1.5) {
            fog_amount = 1.0 - exp(-mesh_slots[1297].z * fog_distance);
        } else {
            float scaled = mesh_slots[1297].z * fog_distance;
            fog_amount = 1.0 - exp(-(scaled * scaled));
        }
        final_color = mix(final_color, mesh_slots[1296].rgb, clamp(fog_amount, 0.0, 1.0));
    }
    f_color = vec4(final_color, base.a);
}
"#;

const NATIVE_SHADOW_VERTEX_SHADER: &str = r#"#version 450
layout(location = 0) in vec3 position;
layout(location = 4) in uvec4 joints;
layout(location = 5) in vec4 weights;
layout(location = 6) in vec4 instance_model_0;
layout(location = 7) in vec4 instance_model_1;
layout(location = 8) in vec4 instance_model_2;
layout(location = 9) in vec4 instance_model_3;

layout(binding = 0) uniform ShadowUniforms {
    vec4 mesh_slots[1364];
};

mat4 skin_matrix(uint joint) {
    int base = 272 + int(min(joint, 255u)) * 4;
    return mat4(
        mesh_slots[base], mesh_slots[base + 1],
        mesh_slots[base + 2], mesh_slots[base + 3]
    );
}

void main() {
    mat4 model = mat4(
        instance_model_0, instance_model_1, instance_model_2, instance_model_3
    );
    vec3 local_position = position;
    float weight_sum = weights.x + weights.y + weights.z + weights.w;
    if (mesh_slots[15].y > 0.5 && weight_sum > 0.000001) {
        mat4 skin = skin_matrix(joints.x) * weights.x
            + skin_matrix(joints.y) * weights.y
            + skin_matrix(joints.z) * weights.z
            + skin_matrix(joints.w) * weights.w;
        local_position = (skin * vec4(position, 1.0)).xyz;
    }
    mat4 light_view_projection = mat4(
        mesh_slots[4], mesh_slots[5], mesh_slots[6], mesh_slots[7]
    );
    gl_Position = light_view_projection * model * vec4(local_position, 1.0);
}
"#;

const NATIVE_SHADOW_FRAGMENT_SHADER: &str = r#"#version 450
void main() {}
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
    vec3 linear_sample = pow(max(sampled.rgb, vec3(0.0)), vec3(2.2));
    f_color = vec4(linear_sample * max(slots[4].x, 0.0), sampled.a);
}
"#;

const CUBEMAP_ENVIRONMENT_FRAGMENT_SHADER: &str = r#"#version 450
layout(binding = 0) uniform texture2D FallbackTexture;
layout(binding = 1) uniform sampler FallbackSampler;
layout(binding = 2) uniform EnvironmentUniforms {
    vec4 slots[16];
};
layout(binding = 3) uniform textureCube CubemapTexture;
layout(binding = 4) uniform sampler CubemapSampler;

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
    vec4 sampled = texture(samplerCube(CubemapTexture, CubemapSampler), direction);
    // The shared transient descriptor always binds its base texture at 0/1.
    // Consume the white fallback alpha so shader reflection retains those
    // bindings instead of optimizing them out of the descriptor-set layout.
    float fallback_alpha = texture(sampler2D(FallbackTexture, FallbackSampler), uv).a;
    vec3 linear_sample = pow(max(sampled.rgb, vec3(0.0)), vec3(2.2));
    f_color = vec4(
        linear_sample * max(slots[4].x, 0.0),
        sampled.a * fallback_alpha
    );
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

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Zeroable, Pod, Vertex)]
struct NativeMeshVertex {
    #[format(R32G32B32_SFLOAT)]
    position: [f32; 3],
    #[format(R32G32B32_SFLOAT)]
    normal: [f32; 3],
    #[format(R32G32_SFLOAT)]
    uv: [f32; 2],
    #[format(R32G32B32A32_SFLOAT)]
    tangent: [f32; 4],
    #[format(R32G32B32A32_UINT)]
    joints: [u32; 4],
    #[format(R32G32B32A32_SFLOAT)]
    weights: [f32; 4],
}

impl NativeMeshVertex {
    fn new(
        vertex: crate::mesh::Vertex,
        influences: Option<crate::mesh::SkinWeights>,
    ) -> Self {
        let influences = influences.unwrap_or_default();
        Self {
            position: vertex.position,
            normal: vertex.normal,
            uv: vertex.uv,
            tangent: vertex.tangent,
            joints: influences.joints.map(u32::from),
            weights: influences.weights,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Zeroable, Pod, Vertex)]
struct NativeMeshInstance {
    #[format(R32G32B32A32_SFLOAT)]
    model_0: [f32; 4],
    #[format(R32G32B32A32_SFLOAT)]
    model_1: [f32; 4],
    #[format(R32G32B32A32_SFLOAT)]
    model_2: [f32; 4],
    #[format(R32G32B32A32_SFLOAT)]
    model_3: [f32; 4],
    #[format(R32G32B32A32_SFLOAT)]
    normal_0: [f32; 4],
    #[format(R32G32B32A32_SFLOAT)]
    normal_1: [f32; 4],
    #[format(R32G32B32A32_SFLOAT)]
    normal_2: [f32; 4],
    #[format(R32G32B32A32_SFLOAT)]
    tint: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct TextureKey(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct CubemapTextureKey(u64);

struct TextureBatch {
    texture: TextureKey,
    filter: TextureFilter,
    vertices: Vec<GpuVertex>,
    shader: BatchShaderState,
}

#[repr(C)]
#[derive(Clone, Copy, BufferContents)]
struct NativeMeshUniformBuffer {
    slots: [[f32; 4]; NATIVE_MESH_UNIFORM_SLOTS],
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct NativeMeshBatchKey {
    mesh_identity: usize,
    mesh_revision: u64,
    first_index: u32,
    index_count: u32,
    material: Option<usize>,
    material_override: Option<(usize, u64)>,
    texture: TextureKey,
    normal_texture: TextureKey,
    metallic_roughness_texture: TextureKey,
    emissive_texture: TextureKey,
    environment_texture: TextureKey,
    environment_cubemap: CubemapTextureKey,
    reflection_probe_cubemap: CubemapTextureKey,
    double_sided: bool,
    receives_shadows: bool,
    view_projection_bits: [u32; 16],
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct NativeMeshReuseKey {
    mesh_identity: usize,
    mesh_revision: u64,
    material_overrides: Vec<Option<(usize, u64)>>,
    double_sided: bool,
    receives_shadows: bool,
    view_projection_bits: [u32; 16],
}

#[derive(Clone)]
struct NativeMeshBatch {
    key: NativeMeshBatchKey,
    vertex_buffer: Subbuffer<[NativeMeshVertex]>,
    index_buffer: Subbuffer<[u32]>,
    first_index: u32,
    index_count: u32,
    texture: TextureKey,
    normal_texture: TextureKey,
    metallic_roughness_texture: TextureKey,
    emissive_texture: TextureKey,
    environment_texture: TextureKey,
    environment_cubemap: CubemapTextureKey,
    reflection_probe_cubemap: CubemapTextureKey,
    filter: TextureFilter,
    uniforms: NativeMeshUniformBuffer,
    double_sided: bool,
    instances: Vec<NativeMeshInstance>,
    instancing_allowed: bool,
}

#[derive(Clone)]
struct NativeShadowBatch {
    vertex_buffer: Subbuffer<[NativeMeshVertex]>,
    index_buffer: Subbuffer<[u32]>,
    first_index: u32,
    index_count: u32,
    uniforms: NativeMeshUniformBuffer,
    instances: Vec<NativeMeshInstance>,
}

#[derive(Clone, Copy, Debug)]
struct NativeShadowConfig {
    light_index: usize,
    view_projection: crate::render3d::Mat4,
    bias: f32,
}

#[derive(Clone, Copy, Debug)]
struct NativeEnvironmentLighting {
    panorama_texture: TextureKey,
    cubemap_texture: CubemapTextureKey,
    mode: f32,
    intensity: f32,
    rotation_radians: f32,
    fog: Option<crate::environment3d::Fog3D>,
    reflection_probe: Option<NativeReflectionProbeLighting>,
}

#[derive(Clone, Copy, Debug)]
struct NativeReflectionProbeLighting {
    cubemap_texture: CubemapTextureKey,
    intensity: f32,
    rotation_radians: f32,
    blend_weight: f32,
}

#[derive(Clone, Debug)]
struct NativeReflectionProbe {
    probe: crate::render3d::ReflectionProbe3D,
    cubemap_texture: CubemapTextureKey,
}

#[derive(Clone, Copy)]
struct NativeAmbientOcclusion<'a> {
    settings: crate::environment3d::AmbientOcclusion3D,
    occluders: &'a [crate::render3d::AmbientOccluder3D],
}

#[derive(Clone, Copy, Default)]
struct NativeMaterialTextureKeys {
    base_color: Option<TextureKey>,
    normal: Option<TextureKey>,
    metallic_roughness: Option<TextureKey>,
    emissive: Option<TextureKey>,
}

enum RenderBatch {
    Transient(TextureBatch),
    NativeMesh(NativeMeshBatch),
}

#[derive(Clone)]
struct CachedGpuMesh {
    revision: u64,
    vertex_buffer: Subbuffer<[NativeMeshVertex]>,
    index_buffer: Subbuffer<[u32]>,
    bytes: usize,
    last_used: u64,
}

struct CachedTexture {
    revision: u64,
    view: Arc<ImageView>,
    descriptor_nearest: Arc<PersistentDescriptorSet>,
    descriptor_linear: Arc<PersistentDescriptorSet>,
}

struct CachedCubemap {
    revisions: [u64; 6],
    view: Arc<ImageView>,
    last_used: u64,
}

fn native_gpu_skinning(snapshot: &crate::mesh::MeshSnapshot) -> bool {
    snapshot.geometry_revision == 0 && snapshot.mesh.armature.as_ref().is_some_and(|armature| {
        !armature.joints.is_empty()
            && armature.joints.len() <= MAX_NATIVE_SKIN_JOINTS
            && armature.pose_palette.len() == armature.joints.len()
    })
}

fn native_mesh_upload_revision(snapshot: &crate::mesh::MeshSnapshot) -> u64 {
    if native_gpu_skinning(snapshot) {
        snapshot.geometry_revision
    } else {
        snapshot.revision
    }
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

fn native_tonemap_settings(
    stack: &crate::post_process::PostProcessStack,
) -> NativeTonemapSettings {
    if !stack.enabled {
        return NativeTonemapSettings::default();
    }
    stack
        .effects
        .iter()
        .rev()
        .find_map(|pass| {
            if !pass.enabled {
                return None;
            }
            let crate::post_process::Effect::ExposureTonemap(config) = pass.effect else {
                return None;
            };
            Some(NativeTonemapSettings {
                exposure: if config.exposure.is_finite() {
                    config.exposure.clamp(-24.0, 24.0)
                } else {
                    0.0
                },
                operator: match config.operator {
                    crate::post_process::TonemapOperator::None => 0.0,
                    crate::post_process::TonemapOperator::Reinhard => 1.0,
                    crate::post_process::TonemapOperator::Aces => 2.0,
                },
                gamma: if config.gamma.is_finite() {
                    config.gamma.clamp(0.1, 8.0)
                } else {
                    2.2
                },
            })
        })
        .unwrap_or_default()
}

fn native_bloom_settings(
    stack: &crate::post_process::PostProcessStack,
) -> Option<NativeBloomSettings> {
    if !stack.enabled {
        return None;
    }
    stack.effects.iter().rev().find_map(|pass| {
        if !pass.enabled {
            return None;
        }
        let crate::post_process::Effect::Bloom(config) = pass.effect else {
            return None;
        };
        let intensity = if config.intensity.is_finite() {
            config.intensity.clamp(0.0, 64.0)
        } else {
            0.0
        };
        let radius = config.radius.min(64);
        if intensity == 0.0 || radius == 0 {
            return None;
        }
        Some(NativeBloomSettings {
            threshold: if config.threshold.is_finite() {
                config.threshold.clamp(0.0, 1.0)
            } else {
                0.0
            },
            intensity,
            // Targets are half resolution, so retain the authored full-size
            // footprint with a bounded 1..32 sample radius per axis.
            radius: radius.div_ceil(2) as f32,
        })
    })
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
const GPU_CUBEMAP_CACHE_LIMIT: usize = 32;
const GPU_TEXT_CACHE_LIMIT: usize = 256;
const GPU_MESH_IDLE_FRAMES: u64 = 600;
const GPU_MESH_CACHE_LIMIT: usize = 512;
const GPU_MESH_CACHE_BYTE_LIMIT: usize = 512 * 1024 * 1024;

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
    extra_cubemaps: Vec<(u32, CubemapTextureKey)>,
}

impl BatchShaderState {
    fn default_pipeline() -> Self {
        Self {
            pipeline_key: 0,
            fragment_source: None,
            uses_uniform_buffer: false,
            uniform_slots: [[0.0; 4]; crate::shader::MAX_SHADER_FLOAT_UNIFORMS],
            extra_textures: Vec::new(),
            extra_cubemaps: Vec::new(),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, BufferContents)]
struct ShaderUniformBuffer {
    slots: [[f32; 4]; crate::shader::MAX_SHADER_FLOAT_UNIFORMS],
}

#[repr(C)]
#[derive(Clone, Copy, BufferContents)]
struct TonemapUniformBuffer {
    /// exposure stops, operator (0 none / 1 Reinhard / 2 ACES), gamma, unused
    settings: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct NativeTonemapSettings {
    exposure: f32,
    operator: f32,
    gamma: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct NativeBloomSettings {
    threshold: f32,
    intensity: f32,
    radius: f32,
}

impl Default for NativeTonemapSettings {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            operator: 0.0,
            gamma: 2.2,
        }
    }
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
    /// Linear floating-point scene pass. It is intentionally singular while
    /// frame submission is serialized; move to per-frame targets with multiple
    /// in-flight frames.
    render_pass: Arc<RenderPass>,
    hdr_framebuffer: Arc<Framebuffer>,
    hdr_view: Arc<ImageView>,
    present_render_pass: Arc<RenderPass>,
    present_framebuffers: Vec<Arc<Framebuffer>>,
    present_pipeline: Arc<GraphicsPipeline>,
    present_vertex_buffer: Subbuffer<[GpuVertex]>,
    /// Optional RGBA8 tonemapped target and host buffer used only by embedded
    /// Game View/readback tests. Normal windowed Vulkan pays no copy cost.
    capture_enabled: bool,
    capture_render_pass: Option<Arc<RenderPass>>,
    capture_framebuffer: Option<Arc<Framebuffer>>,
    capture_view: Option<Arc<ImageView>>,
    capture_pipeline: Option<Arc<GraphicsPipeline>>,
    capture_buffer: Option<Subbuffer<[u8]>>,
    capture_pixels: Vec<u8>,
    capture_extent: [u32; 2],
    bloom_render_pass: Arc<RenderPass>,
    bloom_framebuffers: [Arc<Framebuffer>; 2],
    bloom_views: [Arc<ImageView>; 2],
    bloom_extract_pipeline: Arc<GraphicsPipeline>,
    bloom_blur_pipeline: Arc<GraphicsPipeline>,
    bloom_extent: [u32; 2],
    pipeline: Arc<GraphicsPipeline>,
    native_mesh_pipeline: Arc<GraphicsPipeline>,
    native_mesh_double_sided_pipeline: Arc<GraphicsPipeline>,
    shadow_framebuffer: Arc<Framebuffer>,
    shadow_view: Arc<ImageView>,
    shadow_pipeline: Arc<GraphicsPipeline>,
    shadow_initialized: bool,
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
    white_cubemap: CubemapTextureKey,
    texture_cache: HashMap<TextureKey, CachedTexture>,
    cubemap_cache: HashMap<CubemapTextureKey, CachedCubemap>,
    cubemap_cache_keys: HashMap<[usize; 6], CubemapTextureKey>,
    shader_cache: HashMap<u64, Arc<GraphicsPipeline>>,
    image_cache_keys: HashMap<usize, TextureKey>,
    image_cache_last_used: HashMap<usize, u64>,
    text_cache: HashMap<u64, TextureKey>,
    text_cache_last_used: HashMap<u64, u64>,
    mesh_cache: HashMap<u64, CachedGpuMesh>,
    mesh_cache_bytes: usize,
    frame_serial: u64,
    next_texture_key: u64,
    next_cubemap_key: u64,
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

        let supported_samples = physical.properties().framebuffer_color_sample_counts
            & physical.properties().framebuffer_depth_sample_counts;
        let msaa_samples = preferred_sample_count(Antialiasing::High, supported_samples);

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
        let render_pass = Self::create_render_pass(device.clone(), msaa_samples)?;
        let (hdr_framebuffer, hdr_view) = Self::create_hdr_framebuffer(
            render_pass.clone(),
            memory_allocator.clone(),
            [size.width.max(1), size.height.max(1), 1],
            msaa_samples,
        )?;
        let present_render_pass =
            Self::create_present_render_pass(device.clone(), swapchain.image_format())?;
        let present_framebuffers =
            Self::create_present_framebuffers(&images, present_render_pass.clone())?;
        let bloom_render_pass = Self::create_bloom_render_pass(device.clone())?;
        let bloom_extent = Self::bloom_extent(size.width, size.height);
        let (bloom_framebuffers, bloom_views) = Self::create_bloom_targets(
            bloom_render_pass.clone(),
            memory_allocator.clone(),
            bloom_extent,
        )?;
        let pipeline = Self::create_pipeline(
            device.clone(),
            render_pass.clone(),
            size.width,
            size.height,
            msaa_samples,
        )?;
        let native_mesh_pipeline = Self::create_native_mesh_pipeline(
            device.clone(),
            render_pass.clone(),
            size.width,
            size.height,
            msaa_samples,
            false,
        )?;
        let native_mesh_double_sided_pipeline = Self::create_native_mesh_pipeline(
            device.clone(),
            render_pass.clone(),
            size.width,
            size.height,
            msaa_samples,
            true,
        )?;
        let present_pipeline = Self::create_present_pipeline(
            device.clone(),
            present_render_pass.clone(),
            size.width,
            size.height,
        )?;
        let present_vertex_buffer = Self::create_fullscreen_quad(memory_allocator.clone())?;
        let bloom_extract_pipeline = Self::create_bloom_pipeline(
            device.clone(),
            bloom_render_pass.clone(),
            bloom_extent,
            BLOOM_EXTRACT_FRAGMENT_SHADER,
        )?;
        let bloom_blur_pipeline = Self::create_bloom_pipeline(
            device.clone(),
            bloom_render_pass.clone(),
            bloom_extent,
            BLOOM_BLUR_FRAGMENT_SHADER,
        )?;
        let (shadow_render_pass, shadow_framebuffer, shadow_view) =
            Self::create_shadow_target(device.clone(), memory_allocator.clone())?;
        let shadow_pipeline = Self::create_shadow_pipeline(
            device.clone(),
            shadow_render_pass.clone(),
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
            hdr_framebuffer,
            hdr_view,
            present_render_pass,
            present_framebuffers,
            present_pipeline,
            present_vertex_buffer,
            capture_enabled: false,
            capture_render_pass: None,
            capture_framebuffer: None,
            capture_view: None,
            capture_pipeline: None,
            capture_buffer: None,
            capture_pixels: Vec::new(),
            capture_extent: [0, 0],
            bloom_render_pass,
            bloom_framebuffers,
            bloom_views,
            bloom_extract_pipeline,
            bloom_blur_pipeline,
            bloom_extent,
            pipeline,
            native_mesh_pipeline,
            native_mesh_double_sided_pipeline,
            shadow_framebuffer,
            shadow_view,
            shadow_pipeline,
            shadow_initialized: false,
            composite_pipeline: None,
            light_map_cache: crate::lighting::LightMapCache::default(),
            light_composite_cache: None,
            recreate_swapchain: false,
            nearest_sampler,
            linear_sampler,
            supported_samples,
            msaa_samples,
            white_texture: TextureKey(0),
            white_cubemap: CubemapTextureKey(0),
            texture_cache: HashMap::new(),
            cubemap_cache: HashMap::new(),
            cubemap_cache_keys: HashMap::new(),
            shader_cache: HashMap::new(),
            image_cache_keys: HashMap::new(),
            image_cache_last_used: HashMap::new(),
            text_cache: HashMap::new(),
            text_cache_last_used: HashMap::new(),
            mesh_cache: HashMap::new(),
            mesh_cache_bytes: 0,
            frame_serial: 0,
            next_texture_key: 1,
            next_cubemap_key: 1,
        };
        presenter.init_white_texture()?;
        presenter.init_white_cubemap()?;

        Ok((presenter, surface))
    }

    fn create_render_pass(
        device: Arc<Device>,
        msaa_samples: SampleCount,
    ) -> Result<Arc<RenderPass>, String> {
        if msaa_samples == SampleCount::Sample1 {
            return single_pass_renderpass!(
                device,
                attachments: {
                    color: {
                        format: HDR_SCENE_FORMAT,
                        samples: 1,
                        load_op: Clear,
                        store_op: Store,
                        final_layout: ImageLayout::ShaderReadOnlyOptimal,
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
                    format: HDR_SCENE_FORMAT,
                    samples: u32::from(msaa_samples),
                    load_op: Clear,
                    store_op: DontCare,
                },
                color_resolve: {
                    format: HDR_SCENE_FORMAT,
                    samples: 1,
                    load_op: DontCare,
                    store_op: Store,
                    final_layout: ImageLayout::ShaderReadOnlyOptimal,
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

    fn create_hdr_framebuffer(
        render_pass: Arc<RenderPass>,
        memory_allocator: Arc<StandardMemoryAllocator>,
        extent: [u32; 3],
        msaa_samples: SampleCount,
    ) -> Result<(Arc<Framebuffer>, Arc<ImageView>), String> {
        let hdr_image = Image::new(
            memory_allocator.clone(),
            ImageCreateInfo {
                format: HDR_SCENE_FORMAT,
                extent,
                // Vulkano's multisample resolve path may use a transfer layout
                // for the single-sample destination, so advertise both roles.
                usage: ImageUsage::COLOR_ATTACHMENT
                    | ImageUsage::SAMPLED
                    | ImageUsage::TRANSFER_DST,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::PREFER_DEVICE,
                ..Default::default()
            },
        )
        .map_err(|e| format!("HDR scene image creation failed: {e}"))?;
        let hdr_view = ImageView::new_default(hdr_image).map_err(|e| e.to_string())?;
        let depth_image = Image::new(
            memory_allocator.clone(),
            ImageCreateInfo {
                format: Format::D16_UNORM,
                extent,
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
            vec![hdr_view.clone(), depth_view]
        } else {
            let msaa_image = Image::new(
                memory_allocator,
                ImageCreateInfo {
                    format: HDR_SCENE_FORMAT,
                    extent,
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
            let msaa_view = ImageView::new_default(msaa_image).map_err(|e| e.to_string())?;
            vec![msaa_view, hdr_view.clone(), depth_view]
        };
        let framebuffer = Framebuffer::new(
            render_pass,
            FramebufferCreateInfo {
                attachments,
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
        Ok((framebuffer, hdr_view))
    }

    fn create_present_render_pass(
        device: Arc<Device>,
        image_format: Format,
    ) -> Result<Arc<RenderPass>, String> {
        single_pass_renderpass!(
            device,
            attachments: {
                output: {
                    format: image_format,
                    samples: 1,
                    load_op: DontCare,
                    store_op: Store,
                    final_layout: ImageLayout::PresentSrc,
                }
            },
            pass: {
                color: [output],
                depth_stencil: {}
            }
        )
        .map_err(|e| e.to_string())
    }

    fn create_capture_render_pass(device: Arc<Device>) -> Result<Arc<RenderPass>, String> {
        single_pass_renderpass!(
            device,
            attachments: {
                output: {
                    format: Format::R8G8B8A8_UNORM,
                    samples: 1,
                    load_op: DontCare,
                    store_op: Store,
                    final_layout: ImageLayout::TransferSrcOptimal,
                }
            },
            pass: {
                color: [output],
                depth_stencil: {}
            }
        )
        .map_err(|error| format!("capture render pass creation failed: {error}"))
    }

    fn ensure_capture_resources(&mut self, width: u32, height: u32) -> Result<(), String> {
        if !self.capture_enabled {
            return Ok(());
        }
        let extent = [width.max(1), height.max(1)];
        if self.capture_extent == extent
            && self.capture_framebuffer.is_some()
            && self.capture_view.is_some()
            && self.capture_pipeline.is_some()
            && self.capture_buffer.is_some()
        {
            return Ok(());
        }
        let render_pass = match self.capture_render_pass.clone() {
            Some(render_pass) => render_pass,
            None => {
                let render_pass = Self::create_capture_render_pass(self.device.clone())?;
                self.capture_render_pass = Some(render_pass.clone());
                render_pass
            }
        };
        let image = Image::new(
            self.memory_allocator.clone(),
            ImageCreateInfo {
                format: Format::R8G8B8A8_UNORM,
                extent: [extent[0], extent[1], 1],
                usage: ImageUsage::COLOR_ATTACHMENT | ImageUsage::TRANSFER_SRC,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::PREFER_DEVICE,
                ..Default::default()
            },
        )
        .map_err(|error| format!("capture image creation failed: {error}"))?;
        let view = ImageView::new_default(image)
            .map_err(|error| format!("capture image view creation failed: {error}"))?;
        let framebuffer = Framebuffer::new(
            render_pass.clone(),
            FramebufferCreateInfo {
                attachments: vec![view.clone()],
                ..Default::default()
            },
        )
        .map_err(|error| format!("capture framebuffer creation failed: {error}"))?;
        let pipeline = Self::create_present_pipeline(
            self.device.clone(),
            render_pass,
            extent[0],
            extent[1],
        )?;
        let byte_len = u64::from(extent[0])
            .checked_mul(u64::from(extent[1]))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| "capture framebuffer byte size overflow".to_string())?;
        let buffer = Buffer::new_slice::<u8>(
            self.memory_allocator.clone(),
            BufferCreateInfo {
                usage: BufferUsage::TRANSFER_DST,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::PREFER_HOST
                    | MemoryTypeFilter::HOST_RANDOM_ACCESS,
                ..Default::default()
            },
            byte_len,
        )
        .map_err(|error| format!("capture readback buffer creation failed: {error}"))?;
        self.capture_framebuffer = Some(framebuffer);
        self.capture_view = Some(view);
        self.capture_pipeline = Some(pipeline);
        self.capture_buffer = Some(buffer);
        self.capture_pixels.resize(byte_len as usize, 0);
        self.capture_extent = extent;
        Ok(())
    }

    fn create_present_framebuffers(
        images: &[Arc<Image>],
        render_pass: Arc<RenderPass>,
    ) -> Result<Vec<Arc<Framebuffer>>, String> {
        images
            .iter()
            .map(|image| {
                let view = ImageView::new_default(image.clone()).map_err(|e| e.to_string())?;
                Framebuffer::new(
                    render_pass.clone(),
                    FramebufferCreateInfo {
                        attachments: vec![view],
                        ..Default::default()
                    },
                )
                .map_err(|e| e.to_string())
            })
            .collect()
    }

    fn create_bloom_render_pass(device: Arc<Device>) -> Result<Arc<RenderPass>, String> {
        single_pass_renderpass!(
            device,
            attachments: {
                bloom: {
                    format: HDR_SCENE_FORMAT,
                    samples: 1,
                    load_op: DontCare,
                    store_op: Store,
                    final_layout: ImageLayout::ShaderReadOnlyOptimal,
                }
            },
            pass: {
                color: [bloom],
                depth_stencil: {}
            }
        )
        .map_err(|e| e.to_string())
    }

    fn bloom_extent(width: u32, height: u32) -> [u32; 2] {
        [
            width.max(1).saturating_add(1) / 2,
            height.max(1).saturating_add(1) / 2,
        ]
    }

    fn create_bloom_targets(
        render_pass: Arc<RenderPass>,
        memory_allocator: Arc<StandardMemoryAllocator>,
        extent: [u32; 2],
    ) -> Result<([Arc<Framebuffer>; 2], [Arc<ImageView>; 2]), String> {
        let mut framebuffers = Vec::with_capacity(2);
        let mut views = Vec::with_capacity(2);
        for _ in 0..2 {
            let image = Image::new(
                memory_allocator.clone(),
                ImageCreateInfo {
                    format: HDR_SCENE_FORMAT,
                    extent: [extent[0].max(1), extent[1].max(1), 1],
                    usage: ImageUsage::COLOR_ATTACHMENT | ImageUsage::SAMPLED,
                    ..Default::default()
                },
                AllocationCreateInfo {
                    memory_type_filter: MemoryTypeFilter::PREFER_DEVICE,
                    ..Default::default()
                },
            )
            .map_err(|e| format!("bloom image creation failed: {e}"))?;
            let view = ImageView::new_default(image).map_err(|e| e.to_string())?;
            let framebuffer = Framebuffer::new(
                render_pass.clone(),
                FramebufferCreateInfo {
                    attachments: vec![view.clone()],
                    ..Default::default()
                },
            )
            .map_err(|e| e.to_string())?;
            views.push(view);
            framebuffers.push(framebuffer);
        }
        Ok((
            framebuffers
                .try_into()
                .map_err(|_| "expected two bloom framebuffers".to_string())?,
            views
                .try_into()
                .map_err(|_| "expected two bloom image views".to_string())?,
        ))
    }

    fn create_shadow_target(
        device: Arc<Device>,
        memory_allocator: Arc<StandardMemoryAllocator>,
    ) -> Result<(Arc<RenderPass>, Arc<Framebuffer>, Arc<ImageView>), String> {
        let render_pass = single_pass_renderpass!(
            device,
            attachments: {
                shadow_depth: {
                    format: Format::D16_UNORM,
                    samples: 1,
                    load_op: Clear,
                    store_op: Store,
                    final_layout: ImageLayout::ShaderReadOnlyOptimal,
                }
            },
            pass: {
                color: [],
                depth_stencil: {shadow_depth}
            }
        )
        .map_err(|error| format!("shadow render pass creation failed: {error:?}"))?;
        let image = Image::new(
            memory_allocator,
            ImageCreateInfo {
                format: Format::D16_UNORM,
                extent: [NATIVE_SHADOW_MAP_SIZE, NATIVE_SHADOW_MAP_SIZE, 1],
                usage: ImageUsage::DEPTH_STENCIL_ATTACHMENT | ImageUsage::SAMPLED,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::PREFER_DEVICE,
                ..Default::default()
            },
        )
        .map_err(|error| format!("shadow image creation failed: {error:?}"))?;
        let view = ImageView::new_default(image)
            .map_err(|error| format!("shadow image view creation failed: {error:?}"))?;
        let framebuffer = Framebuffer::new(
            render_pass.clone(),
            FramebufferCreateInfo {
                attachments: vec![view.clone()],
                ..Default::default()
            },
        )
        .map_err(|error| format!("shadow framebuffer creation failed: {error:?}"))?;
        Ok((render_pass, framebuffer, view))
    }

    fn create_shadow_pipeline(
        device: Arc<Device>,
        render_pass: Arc<RenderPass>,
    ) -> Result<Arc<GraphicsPipeline>, String> {
        let vertex = Self::compile_shader_module(
            device.clone(),
            NATIVE_SHADOW_VERTEX_SHADER,
            naga::ShaderStage::Vertex,
            "neolove_shadow_vertex.glsl",
        )?;
        let vertex_entry = vertex
            .entry_point("main")
            .ok_or_else(|| "missing shadow vertex shader entry point".to_string())?;
        let fragment = Self::compile_shader_module(
            device.clone(),
            NATIVE_SHADOW_FRAGMENT_SHADER,
            naga::ShaderStage::Fragment,
            "neolove_shadow_fragment.glsl",
        )?;
        let fragment_entry = fragment
            .entry_point("main")
            .ok_or_else(|| "missing shadow fragment shader entry point".to_string())?;
        let stages = [
            PipelineShaderStageCreateInfo::new(vertex_entry),
            PipelineShaderStageCreateInfo::new(fragment_entry),
        ];
        let layout = PipelineLayout::new(
            device.clone(),
            PipelineDescriptorSetLayoutCreateInfo::from_stages(&stages)
                .into_pipeline_layout_create_info(device.clone())
                .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        let subpass = Subpass::from(render_pass, 0)
            .ok_or_else(|| "missing shadow render subpass".to_string())?;
        GraphicsPipeline::new(
            device,
            None,
            vulkano::pipeline::graphics::GraphicsPipelineCreateInfo {
                stages: stages.into_iter().collect(),
                vertex_input_state: Some(Self::native_mesh_vertex_input_state()?),
                input_assembly_state: Some(InputAssemblyState {
                    topology: PrimitiveTopology::TriangleList,
                    ..Default::default()
                }),
                viewport_state: Some({
                    let mut state = ViewportState::default();
                    state.viewports[0] = Viewport {
                        offset: [0.0, 0.0],
                        extent: [NATIVE_SHADOW_MAP_SIZE as f32; 2],
                        depth_range: 0.0..=1.0,
                    };
                    state
                }),
                rasterization_state: Some(RasterizationState {
                    cull_mode: CullMode::None,
                    front_face: FrontFace::Clockwise,
                    ..RasterizationState::default()
                }),
                multisample_state: Some(MultisampleState {
                    rasterization_samples: SampleCount::Sample1,
                    ..Default::default()
                }),
                depth_stencil_state: Some(DepthStencilState {
                    depth: Some(DepthState {
                        write_enable: true,
                        compare_op: CompareOp::LessOrEqual,
                    }),
                    ..Default::default()
                }),
                color_blend_state: None,
                dynamic_state: [DynamicState::Viewport].into_iter().collect(),
                subpass: Some(PipelineSubpassType::BeginRenderPass(subpass)),
                ..vulkano::pipeline::graphics::GraphicsPipelineCreateInfo::layout(layout)
            },
        )
        .map_err(|error| format!("shadow graphics pipeline creation failed: {error:?}"))
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

    fn create_present_pipeline(
        device: Arc<Device>,
        render_pass: Arc<RenderPass>,
        width: u32,
        height: u32,
    ) -> Result<Arc<GraphicsPipeline>, String> {
        Self::create_pipeline_with_sources_and_state(
            device,
            render_pass,
            width,
            height,
            SampleCount::Sample1,
            BUILTIN_VERTEX_SHADER,
            TONEMAP_FRAGMENT_SHADER,
            None,
            Self::gpu_vertex_input_state()?,
            RasterizationState::default(),
            false,
        )
    }

    fn create_bloom_pipeline(
        device: Arc<Device>,
        render_pass: Arc<RenderPass>,
        extent: [u32; 2],
        fragment_source: &str,
    ) -> Result<Arc<GraphicsPipeline>, String> {
        Self::create_pipeline_with_sources_and_state(
            device,
            render_pass,
            extent[0],
            extent[1],
            SampleCount::Sample1,
            BUILTIN_VERTEX_SHADER,
            fragment_source,
            None,
            Self::gpu_vertex_input_state()?,
            RasterizationState::default(),
            false,
        )
    }

    fn create_fullscreen_quad(
        memory_allocator: Arc<StandardMemoryAllocator>,
    ) -> Result<Subbuffer<[GpuVertex]>, String> {
        let white = [1.0; 4];
        Buffer::from_iter(
            memory_allocator,
            BufferCreateInfo {
                usage: BufferUsage::VERTEX_BUFFER,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::PREFER_HOST
                    | MemoryTypeFilter::HOST_SEQUENTIAL_WRITE,
                ..Default::default()
            },
            [
                GpuVertex { position: [-1.0, 1.0, 0.0, 1.0], color: white, uv: [0.0, 0.0] },
                GpuVertex { position: [1.0, 1.0, 0.0, 1.0], color: white, uv: [1.0, 0.0] },
                GpuVertex { position: [1.0, -1.0, 0.0, 1.0], color: white, uv: [1.0, 1.0] },
                GpuVertex { position: [-1.0, 1.0, 0.0, 1.0], color: white, uv: [0.0, 0.0] },
                GpuVertex { position: [1.0, -1.0, 0.0, 1.0], color: white, uv: [1.0, 1.0] },
                GpuVertex { position: [-1.0, -1.0, 0.0, 1.0], color: white, uv: [0.0, 1.0] },
            ],
        )
        .map_err(|e| e.to_string())
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

    fn gpu_vertex_input_state() -> Result<VertexInputState, String> {
        let description = GpuVertex::per_vertex();
        let position = description
            .members
            .get("position")
            .ok_or_else(|| "vertex layout missing position field".to_string())?;
        let color = description
            .members
            .get("color")
            .ok_or_else(|| "vertex layout missing color field".to_string())?;
        let uv = description
            .members
            .get("uv")
            .ok_or_else(|| "vertex layout missing uv field".to_string())?;
        Ok(VertexInputState::new()
            .binding(
                0,
                VertexInputBindingDescription {
                    stride: description.stride,
                    input_rate: description.input_rate,
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
            ))
    }

    fn native_mesh_vertex_input_state() -> Result<VertexInputState, String> {
        let description = NativeMeshVertex::per_vertex();
        let instance = NativeMeshInstance::per_vertex();
        let position = description
            .members
            .get("position")
            .ok_or_else(|| "native mesh layout missing position field".to_string())?;
        let normal = description
            .members
            .get("normal")
            .ok_or_else(|| "native mesh layout missing normal field".to_string())?;
        let uv = description
            .members
            .get("uv")
            .ok_or_else(|| "native mesh layout missing uv field".to_string())?;
        let tangent = description
            .members
            .get("tangent")
            .ok_or_else(|| "native mesh layout missing tangent field".to_string())?;
        let joints = description
            .members
            .get("joints")
            .ok_or_else(|| "native mesh layout missing joints field".to_string())?;
        let weights = description
            .members
            .get("weights")
            .ok_or_else(|| "native mesh layout missing weights field".to_string())?;
        let mut state = VertexInputState::new()
            .binding(
                0,
                VertexInputBindingDescription {
                    stride: description.stride,
                    input_rate: description.input_rate,
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
                    format: normal.format,
                    offset: normal.offset as u32,
                },
            )
            .attribute(
                2,
                VertexInputAttributeDescription {
                    binding: 0,
                    format: uv.format,
                    offset: uv.offset as u32,
                },
            )
            .attribute(
                3,
                VertexInputAttributeDescription {
                    binding: 0,
                    format: tangent.format,
                    offset: tangent.offset as u32,
                },
            )
            .attribute(
                4,
                VertexInputAttributeDescription {
                    binding: 0,
                    format: joints.format,
                    offset: joints.offset as u32,
                },
            )
            .attribute(
                5,
                VertexInputAttributeDescription {
                    binding: 0,
                    format: weights.format,
                    offset: weights.offset as u32,
                },
            )
            .binding(
                1,
                VertexInputBindingDescription {
                    stride: instance.stride,
                    input_rate: VertexInputRate::Instance { divisor: 1 },
                },
            );
        for (location, name) in [
            "model_0", "model_1", "model_2", "model_3", "normal_0", "normal_1", "normal_2",
            "tint",
        ]
        .into_iter()
        .enumerate()
        {
            let attribute = instance
                .members
                .get(name)
                .ok_or_else(|| format!("native instance layout missing {name} field"))?;
            state = state.attribute(
                location as u32 + 6,
                VertexInputAttributeDescription {
                    binding: 1,
                    format: attribute.format,
                    offset: attribute.offset as u32,
                },
            );
        }
        Ok(state)
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
        Self::create_pipeline_with_sources_and_state(
            device,
            render_pass,
            width,
            height,
            msaa_samples,
            vertex_source,
            fragment_source,
            Some(blend),
            Self::gpu_vertex_input_state()?,
            RasterizationState::default(),
            true,
        )
    }

    fn create_native_mesh_pipeline(
        device: Arc<Device>,
        render_pass: Arc<RenderPass>,
        width: u32,
        height: u32,
        msaa_samples: SampleCount,
        double_sided: bool,
    ) -> Result<Arc<GraphicsPipeline>, String> {
        let rasterization_state = native_mesh_rasterization_state(double_sided);
        Self::create_pipeline_with_sources_and_state(
            device,
            render_pass,
            width,
            height,
            msaa_samples,
            NATIVE_MESH_VERTEX_SHADER,
            NATIVE_MESH_FRAGMENT_SHADER,
            Some(AttachmentBlend::alpha()),
            Self::native_mesh_vertex_input_state()?,
            rasterization_state,
            true,
        )
    }

    fn create_pipeline_with_sources_and_state(
        device: Arc<Device>,
        render_pass: Arc<RenderPass>,
        width: u32,
        height: u32,
        msaa_samples: SampleCount,
        vertex_source: &str,
        fragment_source: &str,
        blend: Option<AttachmentBlend>,
        vertex_input_state: VertexInputState,
        rasterization_state: RasterizationState,
        depth_enabled: bool,
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
                .map_err(|error| format!("pipeline layout derivation failed: {error:?}"))?,
        )
        .map_err(|error| format!("pipeline layout creation failed: {error:?}"))?;
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
                rasterization_state: Some(rasterization_state),
                multisample_state: Some(MultisampleState {
                    rasterization_samples: msaa_samples,
                    ..Default::default()
                }),
                depth_stencil_state: depth_enabled.then(|| DepthStencilState {
                    depth: Some(DepthState {
                        write_enable: true,
                        compare_op: CompareOp::LessOrEqual,
                    }),
                    ..Default::default()
                }),
                color_blend_state: Some(ColorBlendState::with_attachment_states(
                    1,
                    ColorBlendAttachmentState {
                        blend,
                        color_write_mask: ColorComponents::all(),
                        color_write_enable: true,
                    },
                )),
                dynamic_state: [DynamicState::Viewport].into_iter().collect(),
                subpass: Some(PipelineSubpassType::BeginRenderPass(subpass)),
                ..vulkano::pipeline::graphics::GraphicsPipelineCreateInfo::layout(layout)
            },
        )
        .map_err(|error| format!("graphics pipeline creation failed: {error:?}"))
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

    fn init_white_cubemap(&mut self) -> Result<(), String> {
        let faces = std::array::from_fn(|_| {
            Arc::new(RgbaImage::from_pixel(
                1,
                1,
                image::Rgba([255, 255, 255, 255]),
            ))
        });
        let key = self.upload_cubemap_texture(CubemapTextureKey(0), [0; 6], &faces, 1)?;
        self.white_cubemap = key;
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
        (self.hdr_framebuffer, self.hdr_view) = Self::create_hdr_framebuffer(
            self.render_pass.clone(),
            self.memory_allocator.clone(),
            [width.max(1), height.max(1), 1],
            self.msaa_samples,
        )?;
        self.bloom_extent = Self::bloom_extent(width, height);
        (self.bloom_framebuffers, self.bloom_views) = Self::create_bloom_targets(
            self.bloom_render_pass.clone(),
            self.memory_allocator.clone(),
            self.bloom_extent,
        )?;
        self.present_framebuffers =
            Self::create_present_framebuffers(&self.images, self.present_render_pass.clone())?;
        self.pipeline = Self::create_pipeline(
            self.device.clone(),
            self.render_pass.clone(),
            width.max(1),
            height.max(1),
            self.msaa_samples,
        )?;
        self.native_mesh_pipeline = Self::create_native_mesh_pipeline(
            self.device.clone(),
            self.render_pass.clone(),
            width.max(1),
            height.max(1),
            self.msaa_samples,
            false,
        )?;
        self.native_mesh_double_sided_pipeline = Self::create_native_mesh_pipeline(
            self.device.clone(),
            self.render_pass.clone(),
            width.max(1),
            height.max(1),
            self.msaa_samples,
            true,
        )?;
        self.present_pipeline = Self::create_present_pipeline(
            self.device.clone(),
            self.present_render_pass.clone(),
            width.max(1),
            height.max(1),
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
        self.render_pass = Self::create_render_pass(self.device.clone(), samples)?;
        (self.hdr_framebuffer, self.hdr_view) = Self::create_hdr_framebuffer(
            self.render_pass.clone(),
            self.memory_allocator.clone(),
            [width.max(1), height.max(1), 1],
            samples,
        )?;
        self.pipeline = Self::create_pipeline(
            self.device.clone(),
            self.render_pass.clone(),
            width.max(1),
            height.max(1),
            samples,
        )?;
        self.native_mesh_pipeline = Self::create_native_mesh_pipeline(
            self.device.clone(),
            self.render_pass.clone(),
            width.max(1),
            height.max(1),
            samples,
            false,
        )?;
        self.native_mesh_double_sided_pipeline = Self::create_native_mesh_pipeline(
            self.device.clone(),
            self.render_pass.clone(),
            width.max(1),
            height.max(1),
            samples,
            true,
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
        self.ensure_capture_resources(surface_width, surface_height)?;

        self.frame_serial = self.frame_serial.wrapping_add(1);

        let commands = renderer::drain_commands_without_remembering(render_state)?;
        let platform_clear_color = lock_platform_state(platform).clear_color();
        let (
            config,
            lights,
            occluders,
            lights_3d,
            reflection_probes_3d,
            environment,
            camera_3d,
            tonemap,
            bloom,
        ) = {
            let mut state = render_state
                .lock()
                .map_err(|_| "render state lock poisoned".to_string())?;
            let (config, lights, occluders) = state.take_lighting();
            let lights_3d = state.take_lights_3d();
            let reflection_probes_3d = state.take_reflection_probes_3d();
            let environment = state.environment_3d();
            let camera_3d = state.camera_3d();
            let tonemap = native_tonemap_settings(state.post_process());
            let bloom = native_bloom_settings(state.post_process());
            (
                config,
                lights,
                occluders,
                lights_3d,
                reflection_probes_3d,
                environment,
                camera_3d,
                tonemap,
                bloom,
            )
        };
        #[cfg(not(neolove_2d))]
        let clear_color = environment_clear_color(&environment, platform_clear_color);
        #[cfg(neolove_2d)]
        let clear_color = platform_clear_color;
        #[cfg(not(neolove_2d))]
        let shadow = native_shadow_config(
            &lights_3d,
            camera_3d,
            logical_width.max(1) as f32 / logical_height.max(1) as f32,
        );
        #[cfg(not(neolove_2d))]
        let batches = self.build_batches(
            &commands,
            logical_width.max(1),
            logical_height.max(1),
            &lights_3d,
            &reflection_probes_3d,
            &environment,
            camera_3d,
            shadow,
        )
        .map_err(|error| format!("3D batch preparation failed: {error}"))?;
        #[cfg(neolove_2d)]
        let batches = self.build_batches_2d(
            &commands,
            logical_width.max(1),
            logical_height.max(1),
        )?;
        #[cfg(not(neolove_2d))]
        let shadow_batches = self
            .build_shadow_batches(&commands, shadow)
            .map_err(|error| format!("shadow batch preparation failed: {error}"))?;
        #[cfg(neolove_2d)]
        let shadow_batches = Vec::new();
        #[cfg(not(neolove_2d))]
        let update_shadow_map = shadow.is_some() || !self.shadow_initialized;
        #[cfg(neolove_2d)]
        let update_shadow_map = false;
        self.prune_gpu_mesh_cache();
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
            update_shadow_map,
            shadow_batches,
            batches,
            light_composite,
            tonemap,
            bloom,
        )
        .map_err(|error| format!("frame command recording failed: {error}"))?;

        let previous = self
            .previous_frame_end
            .take()
            .unwrap_or_else(|| sync::now(self.device.clone()).boxed());
        let future = previous
            .join(acquire_future)
            .then_execute(self.queue.clone(), command_buffer)
            .map_err(|error| format!("frame queue submission failed: {error:?}"))?
            .then_swapchain_present(
                self.queue.clone(),
                SwapchainPresentInfo::swapchain_image_index(self.swapchain.clone(), image_index),
            )
            .then_signal_fence_and_flush();

        match future.map_err(Validated::unwrap) {
            Ok(future) => {
                if self.capture_enabled {
                    // Readback is the one path that genuinely needs the CPU to
                    // wait. Normal windowed frames remain chained on the GPU
                    // and can overlap CPU simulation/command preparation.
                    future.wait(None).map_err(|e| e.to_string())?;
                    if let Some(buffer) = self.capture_buffer.as_ref() {
                        let bytes = buffer
                            .read()
                            .map_err(|error| format!("capture readback map failed: {error}"))?;
                        self.capture_pixels.clear();
                        self.capture_pixels.extend_from_slice(&bytes);
                    }
                    self.previous_frame_end = Some(sync::now(self.device.clone()).boxed());
                } else {
                    // The chained future preserves ordering for the shared HDR,
                    // bloom, and depth targets without stalling the CPU after
                    // every present. cleanup_finished() retires resources once
                    // the queue has completed them.
                    self.previous_frame_end = Some(future.boxed());
                }
                self.shadow_initialized = true;
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

    pub(crate) fn set_frame_capture_enabled(&mut self, enabled: bool) {
        self.capture_enabled = enabled;
        if !enabled {
            self.capture_pixels.clear();
        }
    }

    pub(crate) fn captured_pixels(&self) -> Option<(u32, u32, &[u8])> {
        let [width, height] = self.capture_extent;
        let expected = width as usize * height as usize * 4;
        (self.capture_enabled && expected > 0 && self.capture_pixels.len() == expected)
            .then_some((width, height, self.capture_pixels.as_slice()))
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
                LIGHT_COMPOSITE_FRAGMENT_SHADER,
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
        update_shadow_map: bool,
        shadow_batches: Vec<NativeShadowBatch>,
        batches: Vec<RenderBatch>,
        light_composite: Option<LightComposite>,
        tonemap: NativeTonemapSettings,
        bloom: Option<NativeBloomSettings>,
    ) -> Result<Arc<PrimaryAutoCommandBuffer>, String> {
        let mut builder = AutoCommandBufferBuilder::primary(
            &self.command_buffer_allocator,
            self.queue.queue_family_index(),
            CommandBufferUsage::OneTimeSubmit,
        )
        .map_err(|e| e.to_string())?;

        if update_shadow_map {
            builder
                .begin_render_pass(
                RenderPassBeginInfo {
                    clear_values: vec![Some(ClearValue::Depth(1.0))],
                    ..RenderPassBeginInfo::framebuffer(self.shadow_framebuffer.clone())
                },
                SubpassBeginInfo {
                    contents: SubpassContents::Inline,
                    ..Default::default()
                },
                )
                .map_err(|error| error.to_string())?;
            builder
                .set_viewport(
                0,
                [Viewport {
                    offset: [0.0, 0.0],
                    extent: [NATIVE_SHADOW_MAP_SIZE as f32; 2],
                    depth_range: 0.0..=1.0,
                }]
                .into_iter()
                .collect(),
                )
                .map_err(|error| error.to_string())?;
            for batch in shadow_batches {
                if batch.index_count == 0 || batch.instances.is_empty() {
                    continue;
                }
                let descriptor = self.descriptor_for_shadow(batch.uniforms)?;
                let instance_count = u32::try_from(batch.instances.len())
                    .map_err(|_| "shadow instance count exceeds u32".to_string())?;
                let instance_buffer = Buffer::from_iter(
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
                batch.instances,
                )
                .map_err(|error| error.to_string())?;
                builder
                    .bind_pipeline_graphics(self.shadow_pipeline.clone())
                .map_err(|error| error.to_string())?
                .bind_descriptor_sets(
                    PipelineBindPoint::Graphics,
                    self.shadow_pipeline.layout().clone(),
                    0,
                    descriptor,
                )
                .map_err(|error| error.to_string())?
                .bind_vertex_buffers(0, (batch.vertex_buffer, instance_buffer))
                .map_err(|error| error.to_string())?
                .bind_index_buffer(batch.index_buffer)
                .map_err(|error| error.to_string())?
                    .draw_indexed(batch.index_count, instance_count, batch.first_index, 0, 0)
                    .map_err(|error| error.to_string())?;
            }
            builder
                .end_render_pass(SubpassEndInfo::default())
                .map_err(|error| error.to_string())?;
        }

        let clear_values = if self.msaa_samples == SampleCount::Sample1 {
            vec![
                Some(ClearValue::Float([
                    (clear.r as f32 / 255.0).powf(2.2),
                    (clear.g as f32 / 255.0).powf(2.2),
                    (clear.b as f32 / 255.0).powf(2.2),
                    clear.a as f32 / 255.0,
                ])),
                Some(ClearValue::Depth(1.0)),
            ]
        } else {
            vec![
                Some(ClearValue::Float([
                    (clear.r as f32 / 255.0).powf(2.2),
                    (clear.g as f32 / 255.0).powf(2.2),
                    (clear.b as f32 / 255.0).powf(2.2),
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
                    ..RenderPassBeginInfo::framebuffer(self.hdr_framebuffer.clone())
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

        // Upload all transient 2D/text vertices through one host allocation.
        // Texture changes can create many batches in sprite-heavy games; one
        // allocation per batch made allocator/validation overhead dominate the
        // actual draws even though every batch uses the same vertex layout.
        let transient_vertex_count = batches
            .iter()
            .map(|batch| match batch {
                RenderBatch::Transient(batch) => batch.vertices.len() as u64,
                RenderBatch::NativeMesh(_) => 0,
            })
            .sum::<u64>();
        let transient_vertex_buffer = if transient_vertex_count == 0 {
            None
        } else {
            let buffer = Buffer::new_slice::<GpuVertex>(
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
                transient_vertex_count,
            )
            .map_err(|error| format!("transient vertex allocation failed: {error}"))?;
            {
                let mut mapped = buffer
                    .write()
                    .map_err(|error| format!("transient vertex upload failed: {error}"))?;
                let mut offset = 0;
                for batch in &batches {
                    let RenderBatch::Transient(batch) = batch else {
                        continue;
                    };
                    let end = offset + batch.vertices.len();
                    mapped[offset..end].copy_from_slice(&batch.vertices);
                    offset = end;
                }
            }
            Some(buffer)
        };
        let mut transient_vertex_offset = 0_u64;

        for batch in batches {
            match batch {
                RenderBatch::Transient(batch) => {
                    if batch.vertices.is_empty() {
                        continue;
                    }
                    let pipeline = self
                        .pipeline_for_batch(&batch.shader, width, height)
                        .map_err(|error| format!("transient pipeline creation failed: {error}"))?;
                    let descriptor = self.descriptor_for_batch(
                        pipeline.clone(),
                        batch.texture,
                        batch.filter,
                        &batch.shader,
                    )
                    .map_err(|error| format!("transient descriptor creation failed: {error}"))?;
                    let vertex_count = batch.vertices.len() as u32;
                    let vertex_end = transient_vertex_offset + vertex_count as u64;
                    let vertex_buffer = transient_vertex_buffer
                        .as_ref()
                        .expect("a non-empty transient batch has a shared vertex buffer")
                        .clone()
                        .slice(transient_vertex_offset..vertex_end);
                    transient_vertex_offset = vertex_end;

                    builder
                        .bind_pipeline_graphics(pipeline.clone())
                        .map_err(|error| {
                            format!("transient pipeline bind failed: {error:?}")
                        })?
                        .bind_descriptor_sets(
                            PipelineBindPoint::Graphics,
                            pipeline.layout().clone(),
                            0,
                            descriptor,
                        )
                        .map_err(|error| {
                            format!("transient descriptor bind failed: {error:?}")
                        })?
                        .bind_vertex_buffers(0, vertex_buffer)
                        .map_err(|error| {
                            format!("transient vertex-buffer bind failed: {error:?}")
                        })?
                        .draw(vertex_count, 1, 0, 0)
                        .map_err(|error| format!("transient draw failed: {error:?}"))?;
                }
                RenderBatch::NativeMesh(batch) => {
                    if batch.index_count == 0 || batch.instances.is_empty() {
                        continue;
                    }
                    let pipeline = if batch.double_sided {
                        self.native_mesh_double_sided_pipeline.clone()
                    } else {
                        self.native_mesh_pipeline.clone()
                    };
                    let descriptor = self.descriptor_for_native_mesh(
                        pipeline.clone(),
                        batch.texture,
                        batch.normal_texture,
                        batch.metallic_roughness_texture,
                        batch.emissive_texture,
                        batch.environment_texture,
                        batch.environment_cubemap,
                        batch.reflection_probe_cubemap,
                        batch.filter,
                        batch.uniforms,
                    )?;
                    let instance_count = u32::try_from(batch.instances.len())
                        .map_err(|_| "native mesh instance count exceeds u32".to_string())?;
                    let instance_buffer = Buffer::from_iter(
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
                        batch.instances,
                    )
                    .map_err(|error| error.to_string())?;
                    builder
                        .bind_pipeline_graphics(pipeline.clone())
                        .map_err(|error| format!("native mesh pipeline bind failed: {error:?}"))?
                        .bind_descriptor_sets(
                            PipelineBindPoint::Graphics,
                            pipeline.layout().clone(),
                            0,
                            descriptor,
                        )
                        .map_err(|error| {
                            format!("native mesh descriptor bind failed: {error:?}")
                        })?
                        .bind_vertex_buffers(0, (batch.vertex_buffer, instance_buffer))
                        .map_err(|error| {
                            format!("native mesh vertex-buffer bind failed: {error:?}")
                        })?
                        .bind_index_buffer(batch.index_buffer)
                        .map_err(|error| {
                            format!("native mesh index-buffer bind failed: {error:?}")
                        })?
                        .draw_indexed(batch.index_count, instance_count, batch.first_index, 0, 0)
                        .map_err(|error| format!("native mesh draw failed: {error:?}"))?;
                }
            }
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

        let bloom_output = if let Some(settings) = bloom {
            let extract_descriptor = self.descriptor_for_post_process(
                self.bloom_extract_pipeline.clone(),
                self.hdr_view.clone(),
                [settings.threshold, 0.0, 0.0, 0.0],
            )?;
            builder
                .begin_render_pass(
                    RenderPassBeginInfo {
                        clear_values: vec![None],
                        ..RenderPassBeginInfo::framebuffer(self.bloom_framebuffers[0].clone())
                    },
                    SubpassBeginInfo {
                        contents: SubpassContents::Inline,
                        ..Default::default()
                    },
                )
                .map_err(|e| e.to_string())?
                .set_viewport(
                    0,
                    [Viewport {
                        offset: [0.0, 0.0],
                        extent: [self.bloom_extent[0] as f32, self.bloom_extent[1] as f32],
                        depth_range: 0.0..=1.0,
                    }]
                    .into_iter()
                    .collect(),
                )
                .map_err(|e| e.to_string())?
                .bind_pipeline_graphics(self.bloom_extract_pipeline.clone())
                .map_err(|e| e.to_string())?
                .bind_descriptor_sets(
                    PipelineBindPoint::Graphics,
                    self.bloom_extract_pipeline.layout().clone(),
                    0,
                    extract_descriptor,
                )
                .map_err(|e| e.to_string())?
                .bind_vertex_buffers(0, self.present_vertex_buffer.clone())
                .map_err(|e| e.to_string())?
                .draw(6, 1, 0, 0)
                .map_err(|e| e.to_string())?;
            builder
                .end_render_pass(SubpassEndInfo::default())
                .map_err(|e| e.to_string())?;

            let horizontal_descriptor = self.descriptor_for_post_process(
                self.bloom_blur_pipeline.clone(),
                self.bloom_views[0].clone(),
                [1.0, 0.0, settings.radius, 0.0],
            )?;
            builder
                .begin_render_pass(
                    RenderPassBeginInfo {
                        clear_values: vec![None],
                        ..RenderPassBeginInfo::framebuffer(self.bloom_framebuffers[1].clone())
                    },
                    SubpassBeginInfo {
                        contents: SubpassContents::Inline,
                        ..Default::default()
                    },
                )
                .map_err(|e| e.to_string())?
                .set_viewport(
                    0,
                    [Viewport {
                        offset: [0.0, 0.0],
                        extent: [self.bloom_extent[0] as f32, self.bloom_extent[1] as f32],
                        depth_range: 0.0..=1.0,
                    }]
                    .into_iter()
                    .collect(),
                )
                .map_err(|e| e.to_string())?
                .bind_pipeline_graphics(self.bloom_blur_pipeline.clone())
                .map_err(|e| e.to_string())?
                .bind_descriptor_sets(
                    PipelineBindPoint::Graphics,
                    self.bloom_blur_pipeline.layout().clone(),
                    0,
                    horizontal_descriptor,
                )
                .map_err(|e| e.to_string())?
                .bind_vertex_buffers(0, self.present_vertex_buffer.clone())
                .map_err(|e| e.to_string())?
                .draw(6, 1, 0, 0)
                .map_err(|e| e.to_string())?;
            builder
                .end_render_pass(SubpassEndInfo::default())
                .map_err(|e| e.to_string())?;

            let vertical_descriptor = self.descriptor_for_post_process(
                self.bloom_blur_pipeline.clone(),
                self.bloom_views[1].clone(),
                [0.0, 1.0, settings.radius, 0.0],
            )?;
            builder
                .begin_render_pass(
                    RenderPassBeginInfo {
                        clear_values: vec![None],
                        ..RenderPassBeginInfo::framebuffer(self.bloom_framebuffers[0].clone())
                    },
                    SubpassBeginInfo {
                        contents: SubpassContents::Inline,
                        ..Default::default()
                    },
                )
                .map_err(|e| e.to_string())?
                .set_viewport(
                    0,
                    [Viewport {
                        offset: [0.0, 0.0],
                        extent: [self.bloom_extent[0] as f32, self.bloom_extent[1] as f32],
                        depth_range: 0.0..=1.0,
                    }]
                    .into_iter()
                    .collect(),
                )
                .map_err(|e| e.to_string())?
                .bind_pipeline_graphics(self.bloom_blur_pipeline.clone())
                .map_err(|e| e.to_string())?
                .bind_descriptor_sets(
                    PipelineBindPoint::Graphics,
                    self.bloom_blur_pipeline.layout().clone(),
                    0,
                    vertical_descriptor,
                )
                .map_err(|e| e.to_string())?
                .bind_vertex_buffers(0, self.present_vertex_buffer.clone())
                .map_err(|e| e.to_string())?
                .draw(6, 1, 0, 0)
                .map_err(|e| e.to_string())?;
            builder
                .end_render_pass(SubpassEndInfo::default())
                .map_err(|e| e.to_string())?;
            Some((self.bloom_views[0].clone(), settings.intensity))
        } else {
            None
        };

        let tonemap_descriptor = self.descriptor_for_tonemap(
            self.present_pipeline.clone(),
            tonemap,
            bloom_output.as_ref(),
        )?;
        builder
            .begin_render_pass(
                RenderPassBeginInfo {
                    clear_values: vec![None],
                    ..RenderPassBeginInfo::framebuffer(
                        self.present_framebuffers[image_index].clone(),
                    )
                },
                SubpassBeginInfo {
                    contents: SubpassContents::Inline,
                    ..Default::default()
                },
            )
            .map_err(|e| e.to_string())?
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
            .map_err(|e| e.to_string())?
            .bind_pipeline_graphics(self.present_pipeline.clone())
            .map_err(|e| e.to_string())?
            .bind_descriptor_sets(
                PipelineBindPoint::Graphics,
                self.present_pipeline.layout().clone(),
                0,
                tonemap_descriptor,
            )
            .map_err(|e| e.to_string())?
            .bind_vertex_buffers(0, self.present_vertex_buffer.clone())
            .map_err(|e| e.to_string())?
            .draw(6, 1, 0, 0)
            .map_err(|e| e.to_string())?;
        builder
            .end_render_pass(SubpassEndInfo::default())
            .map_err(|e| e.to_string())?;

        if self.capture_enabled
            && let (Some(framebuffer), Some(view), Some(pipeline), Some(buffer)) = (
                self.capture_framebuffer.clone(),
                self.capture_view.clone(),
                self.capture_pipeline.clone(),
                self.capture_buffer.clone(),
            )
        {
            let descriptor = self.descriptor_for_tonemap(
                pipeline.clone(),
                tonemap,
                bloom_output.as_ref(),
            )?;
            builder
                .begin_render_pass(
                    RenderPassBeginInfo {
                        clear_values: vec![None],
                        ..RenderPassBeginInfo::framebuffer(framebuffer)
                    },
                    SubpassBeginInfo {
                        contents: SubpassContents::Inline,
                        ..Default::default()
                    },
                )
                .map_err(|error| format!("begin capture render pass: {error}"))?
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
                .map_err(|error| format!("set capture viewport: {error}"))?
                .bind_pipeline_graphics(pipeline.clone())
                .map_err(|error| format!("bind capture pipeline: {error}"))?
                .bind_descriptor_sets(
                    PipelineBindPoint::Graphics,
                    pipeline.layout().clone(),
                    0,
                    descriptor,
                )
                .map_err(|error| format!("bind capture descriptors: {error}"))?
                .bind_vertex_buffers(0, self.present_vertex_buffer.clone())
                .map_err(|error| format!("bind capture quad: {error}"))?
                .draw(6, 1, 0, 0)
                .map_err(|error| format!("draw capture tonemap pass: {error}"))?
                .end_render_pass(SubpassEndInfo::default())
                .map_err(|error| format!("end capture render pass: {error}"))?
                .copy_image_to_buffer(CopyImageToBufferInfo::image_buffer(
                    view.image().clone(),
                    buffer,
                ))
                .map_err(|error| format!("copy capture image to host buffer: {error}"))?;
        }
        builder.build().map_err(|e| e.to_string())
    }

    fn descriptor_for_tonemap(
        &self,
        pipeline: Arc<GraphicsPipeline>,
        settings: NativeTonemapSettings,
        bloom: Option<&(Arc<ImageView>, f32)>,
    ) -> Result<Arc<PersistentDescriptorSet>, String> {
        let layout = pipeline
            .layout()
            .set_layouts()
            .first()
            .cloned()
            .ok_or_else(|| "tone-map pipeline missing descriptor layout".to_string())?;
        let uniform = Buffer::from_data(
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
            TonemapUniformBuffer {
                settings: [
                    settings.exposure,
                    settings.operator,
                    settings.gamma,
                    bloom.map(|(_, intensity)| *intensity).unwrap_or(0.0),
                ],
            },
        )
        .map_err(|e| e.to_string())?;
        let fallback_bloom = self
            .texture_cache
            .get(&self.white_texture)
            .ok_or_else(|| "white fallback texture is missing".to_string())?;
        PersistentDescriptorSet::new(
            &self.descriptor_set_allocator,
            layout,
            [
                WriteDescriptorSet::image_view(0, self.hdr_view.clone()),
                WriteDescriptorSet::sampler(1, self.linear_sampler.clone()),
                WriteDescriptorSet::buffer(2, uniform),
                WriteDescriptorSet::image_view(
                    3,
                    bloom
                        .map(|(view, _)| view.clone())
                        .unwrap_or_else(|| fallback_bloom.view.clone()),
                ),
                WriteDescriptorSet::sampler(4, self.linear_sampler.clone()),
            ],
            [],
        )
        .map_err(|e| e.to_string())
    }

    fn descriptor_for_post_process(
        &self,
        pipeline: Arc<GraphicsPipeline>,
        source: Arc<ImageView>,
        settings: [f32; 4],
    ) -> Result<Arc<PersistentDescriptorSet>, String> {
        let layout = pipeline
            .layout()
            .set_layouts()
            .first()
            .cloned()
            .ok_or_else(|| "post-process pipeline missing descriptor layout".to_string())?;
        let uniform = Buffer::from_data(
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
            TonemapUniformBuffer { settings },
        )
        .map_err(|e| e.to_string())?;
        PersistentDescriptorSet::new(
            &self.descriptor_set_allocator,
            layout,
            [
                WriteDescriptorSet::image_view(0, source),
                WriteDescriptorSet::sampler(1, self.linear_sampler.clone()),
                WriteDescriptorSet::buffer(2, uniform),
            ],
            [],
        )
        .map_err(|e| e.to_string())
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
        for (binding, cubemap_key) in &shader.extra_cubemaps {
            let cached = self.cubemap_cache.get(cubemap_key).ok_or_else(|| {
                format!("missing cached cubemap for shader binding {binding}")
            })?;
            writes.push(WriteDescriptorSet::image_view(
                *binding,
                cached.view.clone(),
            ));
            writes.push(WriteDescriptorSet::sampler(
                *binding + 1,
                sampler.clone(),
            ));
        }

        PersistentDescriptorSet::new(&self.descriptor_set_allocator, layout, writes, [])
            .map_err(|error| format!("transient descriptor validation failed: {error:?}"))
    }

    fn descriptor_for_native_mesh(
        &self,
        pipeline: Arc<GraphicsPipeline>,
        texture: TextureKey,
        normal_texture: TextureKey,
        metallic_roughness_texture: TextureKey,
        emissive_texture: TextureKey,
        environment_texture: TextureKey,
        environment_cubemap: CubemapTextureKey,
        reflection_probe_cubemap: CubemapTextureKey,
        filter: TextureFilter,
        uniforms: NativeMeshUniformBuffer,
    ) -> Result<Arc<PersistentDescriptorSet>, String> {
        let layout = pipeline
            .layout()
            .set_layouts()
            .first()
            .cloned()
            .ok_or_else(|| "native mesh pipeline missing descriptor layout".to_string())?;
        let cached = self
            .texture_cache
            .get(&texture)
            .ok_or_else(|| "native mesh texture is missing from the GPU cache".to_string())?;
        let normal = self
            .texture_cache
            .get(&normal_texture)
            .ok_or_else(|| "native mesh normal texture is missing from the GPU cache".to_string())?;
        let metallic_roughness = self
            .texture_cache
            .get(&metallic_roughness_texture)
            .ok_or_else(|| {
                "native mesh metallic/roughness texture is missing from the GPU cache".to_string()
            })?;
        let emissive = self
            .texture_cache
            .get(&emissive_texture)
            .ok_or_else(|| "native mesh emissive texture is missing from the GPU cache".to_string())?;
        let environment = self.texture_cache.get(&environment_texture).ok_or_else(|| {
            "native mesh environment texture is missing from the GPU cache".to_string()
        })?;
        let environment_cubemap = self
            .cubemap_cache
            .get(&environment_cubemap)
            .ok_or_else(|| "native mesh environment cubemap is missing from the GPU cache".to_string())?;
        let reflection_probe_cubemap = self
            .cubemap_cache
            .get(&reflection_probe_cubemap)
            .ok_or_else(|| {
                "native mesh reflection-probe cubemap is missing from the GPU cache".to_string()
            })?;
        let sampler = match filter {
            TextureFilter::Nearest => self.nearest_sampler.clone(),
            TextureFilter::Linear => self.linear_sampler.clone(),
        };
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
            uniforms,
        )
        .map_err(|error| error.to_string())?;
        PersistentDescriptorSet::new(
            &self.descriptor_set_allocator,
            layout,
            [
                WriteDescriptorSet::image_view(0, cached.view.clone()),
                WriteDescriptorSet::sampler(1, sampler.clone()),
                WriteDescriptorSet::buffer(2, uniform_buffer),
                WriteDescriptorSet::image_view(3, normal.view.clone()),
                WriteDescriptorSet::sampler(4, sampler.clone()),
                WriteDescriptorSet::image_view(5, metallic_roughness.view.clone()),
                WriteDescriptorSet::sampler(6, sampler.clone()),
                WriteDescriptorSet::image_view(7, emissive.view.clone()),
                WriteDescriptorSet::sampler(8, sampler),
                WriteDescriptorSet::image_view(9, self.shadow_view.clone()),
                WriteDescriptorSet::sampler(10, self.nearest_sampler.clone()),
                WriteDescriptorSet::image_view(11, environment.view.clone()),
                WriteDescriptorSet::sampler(12, self.linear_sampler.clone()),
                WriteDescriptorSet::image_view(13, environment_cubemap.view.clone()),
                WriteDescriptorSet::sampler(14, self.linear_sampler.clone()),
                WriteDescriptorSet::image_view(15, reflection_probe_cubemap.view.clone()),
                WriteDescriptorSet::sampler(16, self.linear_sampler.clone()),
            ],
            [],
        )
        .map_err(|error| format!("native mesh descriptor creation failed: {error:?}"))
    }

    fn descriptor_for_shadow(
        &self,
        uniforms: NativeMeshUniformBuffer,
    ) -> Result<Arc<PersistentDescriptorSet>, String> {
        let layout = self
            .shadow_pipeline
            .layout()
            .set_layouts()
            .first()
            .cloned()
            .ok_or_else(|| "shadow pipeline missing descriptor layout".to_string())?;
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
            uniforms,
        )
        .map_err(|error| error.to_string())?;
        PersistentDescriptorSet::new(
            &self.descriptor_set_allocator,
            layout,
            [WriteDescriptorSet::buffer(0, uniform_buffer)],
            [],
        )
        .map_err(|error| error.to_string())
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

    fn ensure_gpu_mesh(
        &mut self,
        identity: u64,
        upload_revision: u64,
        mesh: &crate::mesh::MeshData,
        gpu_skinning: bool,
    ) -> Result<CachedGpuMesh, String> {
        if let Some(cached) = self.mesh_cache.get_mut(&identity)
            && cached.revision == upload_revision
        {
            cached.last_used = self.frame_serial;
            return Ok(cached.clone());
        }
        if mesh.vertices.is_empty() || mesh.indices.is_empty() {
            return Err("cannot upload an empty indexed mesh".to_string());
        }

        let vertices = if gpu_skinning {
            let armature = mesh
                .armature
                .as_ref()
                .ok_or_else(|| "GPU-skinned mesh has no armature".to_string())?;
            armature
                .bind_vertices
                .iter()
                .copied()
                .zip(armature.vertex_weights.iter().copied())
                .map(|(vertex, weights)| NativeMeshVertex::new(vertex, Some(weights)))
                .collect::<Vec<_>>()
        } else {
            mesh.vertices
                .iter()
                .copied()
                .map(|vertex| NativeMeshVertex::new(vertex, None))
                .collect::<Vec<_>>()
        };
        let vertex_upload = Buffer::from_iter(
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
            vertices,
        )
        .map_err(|error| error.to_string())?;
        let index_upload = Buffer::from_iter(
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
            mesh.indices.iter().copied(),
        )
        .map_err(|error| error.to_string())?;
        let vertex_buffer = Buffer::new_slice::<NativeMeshVertex>(
            self.memory_allocator.clone(),
            BufferCreateInfo {
                usage: BufferUsage::TRANSFER_DST | BufferUsage::VERTEX_BUFFER,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::PREFER_DEVICE,
                ..Default::default()
            },
            mesh.vertices.len() as u64,
        )
        .map_err(|error| error.to_string())?;
        let index_buffer = Buffer::new_slice::<u32>(
            self.memory_allocator.clone(),
            BufferCreateInfo {
                usage: BufferUsage::TRANSFER_DST | BufferUsage::INDEX_BUFFER,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::PREFER_DEVICE,
                ..Default::default()
            },
            mesh.indices.len() as u64,
        )
        .map_err(|error| error.to_string())?;
        let mut builder = AutoCommandBufferBuilder::primary(
            &self.command_buffer_allocator,
            self.queue.queue_family_index(),
            CommandBufferUsage::OneTimeSubmit,
        )
        .map_err(|error| error.to_string())?;
        builder
            .copy_buffer(CopyBufferInfo::buffers(
                vertex_upload,
                vertex_buffer.clone(),
            ))
            .map_err(|error| error.to_string())?
            .copy_buffer(CopyBufferInfo::buffers(index_upload, index_buffer.clone()))
            .map_err(|error| error.to_string())?;
        let command_buffer = builder.build().map_err(|error| error.to_string())?;
        sync::now(self.device.clone())
            .then_execute(self.queue.clone(), command_buffer)
            .map_err(|error| error.to_string())?
            .then_signal_fence_and_flush()
            .map_err(Validated::unwrap)
            .map_err(|error| error.to_string())?
            .wait(None)
            .map_err(|error| error.to_string())?;

        let bytes = mesh
            .vertices
            .len()
            .saturating_mul(std::mem::size_of::<NativeMeshVertex>())
            .saturating_add(
                mesh.indices
                    .len()
                    .saturating_mul(std::mem::size_of::<u32>()),
            );
        let cached = CachedGpuMesh {
            revision: upload_revision,
            vertex_buffer,
            index_buffer,
            bytes,
            last_used: self.frame_serial,
        };
        if let Some(previous) = self.mesh_cache.insert(identity, cached.clone()) {
            self.mesh_cache_bytes = self.mesh_cache_bytes.saturating_sub(previous.bytes);
        }
        self.mesh_cache_bytes = self.mesh_cache_bytes.saturating_add(bytes);
        Ok(cached)
    }

    fn native_mesh_batches(
        &mut self,
        command: &crate::render3d::Mesh3DCommand,
        lights: &[crate::render3d::Light3D],
        camera: crate::render3d::Camera3D,
        shadow: Option<NativeShadowConfig>,
        environment: Option<NativeEnvironmentLighting>,
        ambient_occlusion: Option<crate::environment3d::AmbientOcclusion3D>,
        ambient_occluders: &[crate::render3d::AmbientOccluder3D],
        source_index: usize,
        snapshot: &crate::mesh::MeshSnapshot,
    ) -> Result<Vec<NativeMeshBatch>, String> {
        let mesh = &snapshot.mesh;
        if mesh.indices.is_empty()
            || !crate::render3d::bounds_visible(
                mesh.bounds,
                command.view_projection.mul(command.model),
            )
        {
            return Ok(Vec::new());
        }

        let explicit_texture = command
            .texture
            .as_ref()
            .map(|image| self.texture_for_image(image))
            .transpose()?;
        let material_overrides = command.material_override_snapshots()?;
        let material_count = mesh.materials.len().max(material_overrides.len());
        let mut resolved_materials = Vec::with_capacity(material_count);
        let mut material_textures = Vec::with_capacity(material_count);
        for index in 0..material_count {
            let material = material_overrides
                .get(index)
                .and_then(Option::as_ref)
                .map(|snapshot| snapshot.material.clone())
                .or_else(|| mesh.materials.get(index).cloned().map(Arc::new));
            let Some(material) = material else {
                resolved_materials.push(None);
                material_textures.push(NativeMaterialTextureKeys::default());
                continue;
            };
            let base_color = if explicit_texture.is_none() {
                material
                    .base_color_texture
                    .as_ref()
                    .and_then(|binding| binding.image.as_ref())
                    .map(|image| self.texture_for_image(image))
                    .transpose()?
            } else {
                None
            };
            let normal = material
                .normal_texture
                .as_ref()
                .and_then(|binding| binding.image.as_ref())
                .map(|image| self.texture_for_image(image))
                .transpose()?;
            let metallic_roughness = material
                .metallic_roughness_texture
                .as_ref()
                .and_then(|binding| binding.image.as_ref())
                .map(|image| self.texture_for_image(image))
                .transpose()?;
            let emissive = material
                .emissive_texture
                .as_ref()
                .and_then(|binding| binding.image.as_ref())
                .map(|image| self.texture_for_image(image))
                .transpose()?;
            material_textures.push(NativeMaterialTextureKeys {
                base_color,
                normal,
                metallic_roughness,
                emissive,
            });
            resolved_materials.push(Some(material));
        }
        let gpu_skinning = native_gpu_skinning(snapshot);
        let selected_ambient_occluders = if command.receives_shadows {
            ambient_occlusion
                .and_then(|_| crate::render3d::mesh_world_bounds_3d(command).ok())
                .map(|receiver| {
                    crate::render3d::select_ambient_occluders_3d(
                        source_index,
                        receiver,
                        ambient_occluders,
                    )
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let native_ambient_occlusion = ambient_occlusion
            .filter(|_| !selected_ambient_occluders.is_empty())
            .map(|settings| NativeAmbientOcclusion {
                settings,
                occluders: &selected_ambient_occluders,
            });
        let upload_revision = native_mesh_upload_revision(snapshot);
        let cached = self.ensure_gpu_mesh(
            snapshot.geometry_identity,
            upload_revision,
            mesh,
            gpu_skinning,
        )?;
        let mut batches = Vec::with_capacity(mesh.submeshes.len());
        for submesh in &mesh.submeshes {
            let material_slot = submesh.material.or_else(|| {
                material_overrides
                    .first()
                    .and_then(Option::as_ref)
                    .map(|_| 0)
            });
            let material = material_slot
                .and_then(|index| resolved_materials.get(index))
                .and_then(Option::as_deref);
            let material_override = material_slot.and_then(|index| {
                command.materials.get(index).and_then(Option::as_ref).map(
                    |material| {
                        (
                            material.identity(),
                            material_overrides[index]
                                .as_ref()
                                .expect("material handle snapshot must exist")
                                .revision,
                        )
                    },
                )
            });
            let material_texture = material_slot
                .and_then(|index| material_textures.get(index))
                .copied()
                .unwrap_or_default();
            let texture = explicit_texture
                .or(material_texture.base_color)
                .unwrap_or(self.white_texture);
            let normal_texture = material_texture.normal.unwrap_or(self.white_texture);
            let metallic_roughness_texture = material_texture
                .metallic_roughness
                .unwrap_or(self.white_texture);
            let emissive_texture = material_texture.emissive.unwrap_or(self.white_texture);
            let environment_texture = environment
                .map(|environment| environment.panorama_texture)
                .unwrap_or(self.white_texture);
            let environment_cubemap = environment
                .map(|environment| environment.cubemap_texture)
                .unwrap_or(self.white_cubemap);
            let reflection_probe_cubemap = environment
                .and_then(|environment| environment.reflection_probe)
                .map(|probe| probe.cubemap_texture)
                .unwrap_or(self.white_cubemap);
            let double_sided = command.double_sided
                || material
                    .map(|material| material.double_sided)
                    .unwrap_or(false);
            batches.push(NativeMeshBatch {
                key: NativeMeshBatchKey {
                    mesh_identity: command.mesh.identity(),
                    mesh_revision: snapshot.revision,
                    first_index: submesh.first_index,
                    index_count: submesh.index_count,
                    material: material_slot,
                    material_override,
                    texture,
                    normal_texture,
                    metallic_roughness_texture,
                    emissive_texture,
                    environment_texture,
                    environment_cubemap,
                    reflection_probe_cubemap,
                    double_sided,
                    receives_shadows: command.receives_shadows,
                    view_projection_bits: matrix_bits(command.view_projection),
                },
                vertex_buffer: cached.vertex_buffer.clone(),
                index_buffer: cached.index_buffer.clone(),
                first_index: submesh.first_index,
                index_count: submesh.index_count,
                texture,
                normal_texture,
                metallic_roughness_texture,
                emissive_texture,
                environment_texture,
                environment_cubemap,
                reflection_probe_cubemap,
                filter: if texture == self.white_texture {
                    TextureFilter::Nearest
                } else {
                    TextureFilter::Linear
                },
                uniforms: native_mesh_uniforms(
                    command,
                    material,
                    lights,
                    camera,
                    shadow,
                    environment,
                    gpu_skinning
                        .then(|| mesh.armature.as_ref().map(|armature| armature.pose_palette.as_slice()))
                        .flatten(),
                    native_ambient_occlusion,
                ),
                double_sided,
                instances: vec![native_mesh_instance(command)],
                instancing_allowed: !gpu_skinning
                    && native_ambient_occlusion.is_none()
                    && environment.is_none_or(|environment| environment.reflection_probe.is_none())
                    && material.is_none_or(|material| {
                        !matches!(material.alpha_mode, crate::mesh::AlphaMode::Blend)
                    }),
            });
        }
        Ok(batches)
    }

    fn build_shadow_batches(
        &mut self,
        commands: &[DrawCommand],
        shadow: Option<NativeShadowConfig>,
    ) -> Result<Vec<NativeShadowBatch>, String> {
        let Some(shadow) = shadow else {
            return Ok(Vec::new());
        };
        let mut snapshots = HashMap::<usize, crate::mesh::MeshSnapshot>::new();
        let mut batches = Vec::new();
        for command in commands {
            let DrawCommand::Mesh3D(command) = command else {
                continue;
            };
            if !command.casts_shadows {
                continue;
            }
            let identity = command.mesh.identity();
            let snapshot = match snapshots.get(&identity) {
                Some(snapshot) => snapshot.clone(),
                None => {
                    let snapshot = command.mesh.snapshot().map_err(|error| error.to_string())?;
                    snapshots.insert(identity, snapshot.clone());
                    snapshot
                }
            };
            if snapshot.mesh.indices.is_empty()
                || !crate::render3d::bounds_visible(
                    snapshot.mesh.bounds,
                    shadow.view_projection.mul(command.model),
                )
            {
                continue;
            }
            let gpu_skinning = native_gpu_skinning(&snapshot);
            let cached = self.ensure_gpu_mesh(
                snapshot.geometry_identity,
                native_mesh_upload_revision(&snapshot),
                &snapshot.mesh,
                gpu_skinning,
            )?;
            let palette = gpu_skinning
                .then(|| {
                    snapshot
                        .mesh
                        .armature
                        .as_ref()
                        .map(|armature| armature.pose_palette.as_slice())
                })
                .flatten();
            let uniforms = native_shadow_uniforms(shadow.view_projection, palette);
            let instance = native_mesh_instance(command);
            for submesh in &snapshot.mesh.submeshes {
                if submesh.index_count == 0 {
                    continue;
                }
                batches.push(NativeShadowBatch {
                    vertex_buffer: cached.vertex_buffer.clone(),
                    index_buffer: cached.index_buffer.clone(),
                    first_index: submesh.first_index,
                    index_count: submesh.index_count,
                    uniforms,
                    instances: vec![instance],
                });
            }
        }
        Ok(batches)
    }

    fn prune_gpu_mesh_cache(&mut self) {
        let idle_before = self.frame_serial.saturating_sub(GPU_MESH_IDLE_FRAMES);
        let mut entries = self
            .mesh_cache
            .iter()
            .map(|(identity, cached)| (*identity, cached.last_used, cached.bytes))
            .collect::<Vec<_>>();
        entries.sort_unstable_by_key(|entry| entry.1);

        let mut remove = entries
            .iter()
            .filter(|entry| entry.1 < idle_before)
            .map(|entry| entry.0)
            .collect::<std::collections::HashSet<_>>();
        let mut retained_count = self.mesh_cache.len().saturating_sub(remove.len());
        let mut retained_bytes = self
            .mesh_cache_bytes
            .saturating_sub(
                entries
                    .iter()
                    .filter(|entry| remove.contains(&entry.0))
                    .map(|entry| entry.2)
                    .sum(),
            );
        for (identity, _, bytes) in entries {
            if retained_count <= GPU_MESH_CACHE_LIMIT
                && retained_bytes <= GPU_MESH_CACHE_BYTE_LIMIT
            {
                break;
            }
            if remove.insert(identity) {
                retained_count = retained_count.saturating_sub(1);
                retained_bytes = retained_bytes.saturating_sub(bytes);
            }
        }
        for identity in remove {
            if let Some(cached) = self.mesh_cache.remove(&identity) {
                self.mesh_cache_bytes = self.mesh_cache_bytes.saturating_sub(cached.bytes);
            }
        }
    }

    /// Lean batch preparation used by packaged 2D games. Keeping this separate
    /// from the universal editor/3D path avoids per-frame 3D material, probe,
    /// shadow, mesh-reuse, and environment setup and lets LTO discard it from
    /// the specialized executable.
    #[cfg(neolove_2d)]
    fn build_batches_2d(
        &mut self,
        commands: &[DrawCommand],
        width: u32,
        height: u32,
    ) -> Result<Vec<RenderBatch>, String> {
        let mut batches = Vec::with_capacity(commands.len().min(64));
        let mut current: Option<TextureBatch> = None;

        for command in commands {
            if !renderer::command_intersects_viewport(command, width, height) {
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
                    let pivot_x = x + w * offset.x;
                    let pivot_y = y + h * offset.y;
                    let vertices = quad_vertices(
                        width,
                        height,
                        [
                            world_point(*x, *y, pivot_x, pivot_y, *rotation),
                            world_point(*x + *w, *y, pivot_x, pivot_y, *rotation),
                            world_point(*x + *w, *y + *h, pivot_x, pivot_y, *rotation),
                            world_point(*x, *y + *h, pivot_x, pivot_y, *rotation),
                        ],
                        [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                        *color,
                    );
                    let shader = self.batch_shader_for_command(shader.as_ref())?;
                    push_vertices(
                        &mut current,
                        &mut batches,
                        self.white_texture,
                        TextureFilter::Nearest,
                        shader,
                        vertices,
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
                    push_vertices(
                        &mut current,
                        &mut batches,
                        self.white_texture,
                        TextureFilter::Nearest,
                        shader,
                        [
                            vertex_from_point(width, height, *a, *color, [0.0, 0.0]),
                            vertex_from_point(width, height, *b, *color, [1.0, 0.0]),
                            vertex_from_point(width, height, *c, *color, [0.5, 1.0]),
                        ],
                    );
                }
                DrawCommand::Circle {
                    center,
                    radius,
                    color,
                    shader,
                } => {
                    let segments = ((radius * std::f32::consts::TAU / 4.0).ceil() as usize)
                        .clamp(24, 128);
                    let mut vertices = Vec::with_capacity(segments * 3);
                    for index in 0..segments {
                        let a0 = index as f32 / segments as f32 * std::f32::consts::TAU;
                        let a1 = (index + 1) as f32 / segments as f32 * std::f32::consts::TAU;
                        vertices.push(vertex_from_point(
                            width,
                            height,
                            *center,
                            *color,
                            [0.5, 0.5],
                        ));
                        vertices.push(vertex_from_point(
                            width,
                            height,
                            Vec2 {
                                x: center.x + a0.cos() * radius,
                                y: center.y + a0.sin() * radius,
                            },
                            *color,
                            [1.0, 0.0],
                        ));
                        vertices.push(vertex_from_point(
                            width,
                            height,
                            Vec2 {
                                x: center.x + a1.cos() * radius,
                                y: center.y + a1.sin() * radius,
                            },
                            *color,
                            [0.0, 1.0],
                        ));
                    }
                    let shader = self.batch_shader_for_command(shader.as_ref())?;
                    push_vertices(
                        &mut current,
                        &mut batches,
                        self.white_texture,
                        TextureFilter::Nearest,
                        shader,
                        vertices,
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
                    let vertices = quad_vertices(
                        width,
                        height,
                        image_corners(*dest, *rotation, *pivot),
                        image_uvs(image, *source)?,
                        *tint,
                    );
                    let shader = self.batch_shader_for_command(shader.as_ref())?;
                    push_vertices(
                        &mut current,
                        &mut batches,
                        texture,
                        *filter,
                        shader,
                        vertices,
                    );
                }
                DrawCommand::Text(request) => {
                    let Some(sprite) = renderer::rasterize_text_sprite(request) else {
                        continue;
                    };
                    let texture = self.texture_for_text(request, sprite.image.as_ref())?;
                    let vertices = quad_vertices(
                        width,
                        height,
                        image_corners(sprite.dest, sprite.rotation, sprite.pivot),
                        [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                        Color::WHITE,
                    );
                    push_vertices(
                        &mut current,
                        &mut batches,
                        texture,
                        sprite.filter,
                        BatchShaderState::default_pipeline(),
                        vertices,
                    );
                }
                // These variants cannot be emitted because their component and
                // asset APIs are absent from a packaged 2D runtime.
                DrawCommand::Mesh3D(_) | DrawCommand::Particles3D(_) => {}
            }
        }

        if let Some(batch) = current {
            batches.push(RenderBatch::Transient(batch));
        }
        Ok(batches)
    }

    fn build_batches(
        &mut self,
        commands: &[DrawCommand],
        width: u32,
        height: u32,
        lights_3d: &[crate::render3d::Light3D],
        reflection_probes_3d: &[crate::render3d::ReflectionProbe3D],
        environment: &crate::environment3d::Environment3D,
        camera_3d: crate::render3d::Camera3D,
        shadow: Option<NativeShadowConfig>,
    ) -> Result<Vec<RenderBatch>, String> {
        let mut batches = Vec::with_capacity(commands.len().min(64));
        let mut current: Option<TextureBatch> = None;
        let mut native_batch_indices = HashMap::<NativeMeshBatchKey, usize>::new();
        let mut native_mesh_snapshots =
            HashMap::<usize, crate::mesh::MeshSnapshot>::new();
        let mut native_mesh_reuse =
            HashMap::<NativeMeshReuseKey, Vec<NativeMeshBatchKey>>::new();

        let environment_intensity = if environment.intensity.is_finite() {
            environment.intensity.max(0.0)
        } else {
            1.0
        };
        let environment_rotation = if environment.rotation_degrees.is_finite() {
            environment.rotation_degrees.to_radians()
        } else {
            0.0
        };
        let fog = (environment.enabled && environment.fog.enabled)
            .then(|| environment.fog.sanitized());
        let ambient_occlusion = (environment.enabled && environment.ambient_occlusion.enabled)
            .then(|| environment.ambient_occlusion.sanitized());
        let ambient_occluders = ambient_occlusion
            .map(|_| crate::render3d::gather_ambient_occluders_3d(commands.iter()))
            .unwrap_or_default();
        let mut native_reflection_probes = Vec::with_capacity(reflection_probes_3d.len());
        for probe in reflection_probes_3d {
            native_reflection_probes.push(NativeReflectionProbe {
                probe: probe.clone().sanitized(),
                cubemap_texture: self.cubemap_for_handle(&probe.cubemap)?,
            });
        }
        let environment_lighting = if environment.enabled {
            match environment.mode {
                crate::environment3d::EnvironmentMode3D::Equirectangular => environment
                    .equirectangular
                    .as_ref()
                    .and_then(|image| self.texture_for_image(image).ok())
                    .map(|texture| NativeEnvironmentLighting {
                        panorama_texture: texture,
                        cubemap_texture: self.white_cubemap,
                        mode: 1.0,
                        intensity: environment_intensity,
                        rotation_radians: environment_rotation,
                        fog,
                        reflection_probe: None,
                    }),
                crate::environment3d::EnvironmentMode3D::Cubemap => environment
                    .cubemap
                    .as_ref()
                    .and_then(|cubemap| self.cubemap_for_handle(cubemap).ok())
                    .map(|cubemap| NativeEnvironmentLighting {
                        panorama_texture: self.white_texture,
                        cubemap_texture: cubemap,
                        mode: 2.0,
                        intensity: environment_intensity,
                        rotation_radians: environment_rotation,
                        fog,
                        reflection_probe: None,
                    }),
                _ => None,
            }
        } else {
            None
        }
        .or_else(|| {
            fog.map(|fog| NativeEnvironmentLighting {
                panorama_texture: self.white_texture,
                cubemap_texture: self.white_cubemap,
                mode: 0.0,
                intensity: 0.0,
                rotation_radians: 0.0,
                fog: Some(fog),
                reflection_probe: None,
            })
        });

        if let Some(background) = self.environment_batch(environment, camera_3d, width, height) {
            batches.push(RenderBatch::Transient(background));
        }

        for (source_index, command) in commands.iter().enumerate() {
            if !renderer::command_intersects_viewport(&command, width, height) {
                continue;
            }
            if !matches!(command, DrawCommand::Mesh3D(command) if command.shader.is_none()) {
                native_batch_indices.clear();
                native_mesh_reuse.clear();
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
                    if command.shader.is_none() {
                        if let Some(batch) = current.take() {
                            batches.push(RenderBatch::Transient(batch));
                        }
                        let mesh_identity = command.mesh.identity();
                        let snapshot = match native_mesh_snapshots.get(&mesh_identity) {
                            Some(snapshot) => snapshot.clone(),
                            None => {
                                let snapshot = command
                                    .mesh
                                    .snapshot()
                                    .map_err(|error| error.to_string())?;
                                native_mesh_snapshots.insert(mesh_identity, snapshot.clone());
                                snapshot
                            }
                        };
                        let reuse_key = NativeMeshReuseKey {
                            mesh_identity,
                            mesh_revision: snapshot.revision,
                            material_overrides: command
                                .materials
                                .iter()
                                .map(|material| {
                                    material
                                        .as_ref()
                                        .map(|material| {
                                            material
                                                .revision()
                                                .map(|revision| (material.identity(), revision))
                                        })
                                        .transpose()
                                })
                                .collect::<Result<Vec<_>, _>>()
                                .map_err(|error| error.to_string())?,
                            double_sided: command.double_sided,
                            receives_shadows: command.receives_shadows,
                            view_projection_bits: matrix_bits(command.view_projection),
                        };
                        let selected_reflection_probe =
                            crate::render3d::mesh_world_bounds_3d(command)
                                .ok()
                                .and_then(|receiver| {
                                    crate::render3d::select_reflection_probe_3d(
                                        receiver.center,
                                        reflection_probes_3d,
                                    )
                                });
                        let command_environment_lighting = selected_reflection_probe
                            .map(|selection| {
                                let selected = &native_reflection_probes[selection.index];
                                let mut lighting = environment_lighting.unwrap_or(
                                    NativeEnvironmentLighting {
                                        panorama_texture: self.white_texture,
                                        cubemap_texture: self.white_cubemap,
                                        mode: 0.0,
                                        intensity: 0.0,
                                        rotation_radians: 0.0,
                                        fog,
                                        reflection_probe: None,
                                    },
                                );
                                lighting.reflection_probe = Some(NativeReflectionProbeLighting {
                                    cubemap_texture: selected.cubemap_texture,
                                    intensity: selected.probe.intensity,
                                    rotation_radians: selected.probe.rotation_degrees.to_radians(),
                                    blend_weight: selection.weight,
                                });
                                lighting
                            })
                            .or(environment_lighting);
                        if command.texture.is_none()
                            && selected_reflection_probe.is_none()
                            && !snapshot.mesh.indices.is_empty()
                            && crate::render3d::bounds_visible(
                                snapshot.mesh.bounds,
                                command.view_projection.mul(command.model),
                            )
                            && let Some(keys) = native_mesh_reuse.get(&reuse_key)
                        {
                            let instance = native_mesh_instance(command);
                            for key in keys {
                                if let Some(index) = native_batch_indices.get(key).copied()
                                    && let Some(RenderBatch::NativeMesh(existing)) =
                                        batches.get_mut(index)
                                {
                                    existing.instances.push(instance);
                                }
                            }
                            continue;
                        }

                        let mut reusable_keys = Vec::new();
                        let mut all_instancing_allowed = command.texture.is_none();
                        for mut candidate in
                            self.native_mesh_batches(
                                command,
                                lights_3d,
                                camera_3d,
                                shadow,
                                command_environment_lighting,
                                ambient_occlusion,
                                &ambient_occluders,
                                source_index,
                                &snapshot,
                            )?
                        {
                            all_instancing_allowed &= candidate.instancing_allowed;
                            reusable_keys.push(candidate.key.clone());
                            if candidate.instancing_allowed
                                && let Some(index) = native_batch_indices.get(&candidate.key).copied()
                                && let Some(RenderBatch::NativeMesh(existing)) =
                                    batches.get_mut(index)
                            {
                                existing.instances.append(&mut candidate.instances);
                                continue;
                            }
                            let index = batches.len();
                            if candidate.instancing_allowed {
                                native_batch_indices.insert(candidate.key.clone(), index);
                            }
                            batches.push(RenderBatch::NativeMesh(candidate));
                        }
                        if selected_reflection_probe.is_none()
                            && all_instancing_allowed
                            && !reusable_keys.is_empty()
                        {
                            native_mesh_reuse.insert(reuse_key, reusable_keys);
                        }
                        continue;
                    }
                    let explicit_texture = command
                        .texture
                        .as_ref()
                        .map(|image| self.texture_for_image(image))
                        .transpose()?;
                    let material_textures = if explicit_texture.is_some() {
                        Vec::new()
                    } else {
                        command
                            .material_base_color_textures()?
                            .into_iter()
                            .map(|image| {
                                image
                                    .as_ref()
                                    .map(|image| self.texture_for_image(image))
                                    .transpose()
                            })
                            .collect::<Result<Vec<_>, String>>()?
                    };
                    let shader = self.batch_shader_for_command(command.shader.as_ref())?;
                    let mut triangles = crate::render3d::project_mesh(command, lights_3d)?;
                    if command.receives_shadows
                        && let Some(settings) = ambient_occlusion
                        && let Ok(receiver) = crate::render3d::mesh_world_bounds_3d(command)
                    {
                        let selected = crate::render3d::select_ambient_occluders_3d(
                            source_index,
                            receiver,
                            &ambient_occluders,
                        );
                        crate::render3d::apply_ambient_occlusion_to_projected_triangles(
                            &mut triangles,
                            settings,
                            &selected,
                        );
                    }
                    if let Some(fog) = fog {
                        crate::render3d::apply_fog_to_projected_triangles(
                            &mut triangles,
                            command.camera_position,
                            fog,
                        );
                    }
                    let mut group_texture = None;
                    let mut group_vertices = Vec::new();
                    for triangle in triangles {
                        let texture = explicit_texture
                            .or_else(|| {
                                triangle
                                    .material
                                    .and_then(|index| material_textures.get(index))
                                    .copied()
                                    .flatten()
                            })
                            .unwrap_or(self.white_texture);
                        if group_texture.is_some_and(|current| current != texture) {
                            let previous = group_texture.expect("material group has a texture");
                            let filter = if previous == self.white_texture {
                                TextureFilter::Nearest
                            } else {
                                TextureFilter::Linear
                            };
                            push_vertices(
                                &mut current,
                                &mut batches,
                                previous,
                                filter,
                                shader.clone(),
                                std::mem::take(&mut group_vertices),
                            );
                        }
                        group_texture = Some(texture);
                        group_vertices.extend(triangle.vertices.into_iter().map(|vertex| GpuVertex {
                            position: vertex.clip_position,
                            color: vertex.color,
                            uv: vertex.uv,
                        }));
                    }
                    if let Some(texture) = group_texture {
                        let filter = if texture == self.white_texture {
                            TextureFilter::Nearest
                        } else {
                            TextureFilter::Linear
                        };
                        push_vertices(
                            &mut current,
                            &mut batches,
                            texture,
                            filter,
                            shader,
                            group_vertices,
                        );
                    }
                }
                DrawCommand::Particles3D(command) => {
                    let texture = match command.texture.as_ref() {
                        Some(image) => self.texture_for_image(image)?,
                        None => self.white_texture,
                    };
                    let mut triangles = crate::render3d::project_particles(command)?;
                    if let Some(fog) = fog {
                        crate::render3d::apply_fog_to_projected_triangles(
                            &mut triangles,
                            command.camera_position,
                            fog,
                        );
                    }
                    let vertices = triangles.into_iter()
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
            batches.push(RenderBatch::Transient(batch));
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
        let (texture, fragment_source, extra_cubemaps) = match environment.mode {
            EnvironmentMode3D::Equirectangular => {
                let Some(image) = environment.equirectangular.as_ref() else {
                    return Some(gradient());
                };
                let Ok(texture) = self.texture_for_image(image) else {
                    return Some(gradient());
                };
                (
                    texture,
                    EQUIRECTANGULAR_ENVIRONMENT_FRAGMENT_SHADER,
                    Vec::new(),
                )
            }
            EnvironmentMode3D::Cubemap => {
                let Some(cubemap) = environment.cubemap.as_ref() else {
                    return Some(gradient());
                };
                let Ok(cubemap) = self.cubemap_for_handle(cubemap) else {
                    return Some(gradient());
                };
                (
                    self.white_texture,
                    CUBEMAP_ENVIRONMENT_FRAGMENT_SHADER,
                    vec![(3, cubemap)],
                )
            }
            _ => return Some(gradient()),
        };
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        fragment_source.hash(&mut hasher);
        Some(TextureBatch {
            texture,
            filter: TextureFilter::Linear,
            vertices: fullscreen([1.0; 4], [1.0; 4]),
            shader: BatchShaderState {
                pipeline_key: hasher.finish(),
                fragment_source: Some(fragment_source.to_string()),
                uses_uniform_buffer: true,
                uniform_slots,
                extra_textures: Vec::new(),
                extra_cubemaps,
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
            extra_cubemaps: Vec::new(),
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

        let mut stale_cubemaps = self
            .cubemap_cache
            .iter()
            .filter_map(|(&key, cached)| {
                (key != self.white_cubemap
                    && frame.wrapping_sub(cached.last_used) > GPU_TEXTURE_IDLE_FRAMES)
                    .then_some((key, cached.last_used))
            })
            .collect::<Vec<_>>();
        let retained_cubemaps = self
            .cubemap_cache
            .len()
            .saturating_sub(1)
            .saturating_sub(stale_cubemaps.len());
        if retained_cubemaps > GPU_CUBEMAP_CACHE_LIMIT {
            let mut remaining = self
                .cubemap_cache
                .iter()
                .filter_map(|(&key, cached)| {
                    (key != self.white_cubemap
                        && cached.last_used != frame
                        && !stale_cubemaps.iter().any(|(stale, _)| *stale == key))
                        .then_some((key, cached.last_used))
                })
                .collect::<Vec<_>>();
            remaining.sort_unstable_by_key(|(_, last_used)| *last_used);
            stale_cubemaps.extend(
                remaining
                    .into_iter()
                    .take(retained_cubemaps - GPU_CUBEMAP_CACHE_LIMIT),
            );
        }
        for (key, _) in stale_cubemaps {
            self.cubemap_cache.remove(&key);
            self.cubemap_cache_keys
                .retain(|_, cached_key| *cached_key != key);
        }

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

    fn allocate_cubemap_key(&mut self) -> CubemapTextureKey {
        let key = CubemapTextureKey(self.next_cubemap_key);
        self.next_cubemap_key = self.next_cubemap_key.wrapping_add(1);
        key
    }

    fn cubemap_for_handle(
        &mut self,
        cubemap: &CubemapHandle,
    ) -> Result<CubemapTextureKey, String> {
        let snapshot = cubemap.snapshot().map_err(|error| error.to_string())?;
        if let Some(key) = self.cubemap_cache_keys.get(&snapshot.identities).copied()
            && let Some(cached) = self.cubemap_cache.get_mut(&key)
            && cached.revisions == snapshot.revisions
        {
            cached.last_used = self.frame_serial;
            return Ok(key);
        }
        let key = self
            .cubemap_cache_keys
            .get(&snapshot.identities)
            .copied()
            .unwrap_or_else(|| self.allocate_cubemap_key());
        self.upload_cubemap_texture(key, snapshot.revisions, &snapshot.faces, snapshot.size)?;
        self.cubemap_cache_keys.insert(snapshot.identities, key);
        Ok(key)
    }

    fn upload_cubemap_texture(
        &mut self,
        key: CubemapTextureKey,
        revisions: [u64; 6],
        faces: &[Arc<RgbaImage>; 6],
        size: u32,
    ) -> Result<CubemapTextureKey, String> {
        let image = Image::new(
            self.memory_allocator.clone(),
            ImageCreateInfo {
                flags: ImageCreateFlags::CUBE_COMPATIBLE,
                format: Format::R8G8B8A8_UNORM,
                extent: [size.max(1), size.max(1), 1],
                array_layers: 6,
                usage: ImageUsage::TRANSFER_DST | ImageUsage::SAMPLED,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::PREFER_DEVICE,
                ..Default::default()
            },
        )
        .map_err(|error| format!("cubemap image creation failed: {error:?}"))?;
        let bytes = faces
            .iter()
            .flat_map(|face| face.as_raw().iter().copied())
            .collect::<Vec<_>>();
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
            bytes,
        )
        .map_err(|error| format!("cubemap staging allocation failed: {error:?}"))?;
        let mut builder = AutoCommandBufferBuilder::primary(
            &self.command_buffer_allocator,
            self.queue.queue_family_index(),
            CommandBufferUsage::OneTimeSubmit,
        )
        .map_err(|error| format!("cubemap command allocation failed: {error:?}"))?;
        builder
            .copy_buffer_to_image(
                vulkano::command_buffer::CopyBufferToImageInfo::buffer_image(
                    upload,
                    image.clone(),
                ),
            )
            .map_err(|error| format!("cubemap buffer-to-image copy failed: {error:?}"))?;
        let command_buffer = builder
            .build()
            .map_err(|error| format!("cubemap upload command build failed: {error:?}"))?;
        sync::now(self.device.clone())
            .then_execute(self.queue.clone(), command_buffer)
            .map_err(|error| format!("cubemap upload submit failed: {error:?}"))?
            .then_signal_fence_and_flush()
            .map_err(Validated::unwrap)
            .map_err(|error| format!("cubemap upload flush failed: {error:?}"))?
            .wait(None)
            .map_err(|error| format!("cubemap upload wait failed: {error:?}"))?;
        let mut view_info = ImageViewCreateInfo::from_image(&image);
        view_info.view_type = ImageViewType::Cube;
        let view = ImageView::new(image, view_info)
            .map_err(|error| format!("cubemap view creation failed: {error:?}"))?;
        self.cubemap_cache.insert(
            key,
            CachedCubemap {
                revisions,
                view,
                last_used: self.frame_serial,
            },
        );
        Ok(key)
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

fn native_mesh_rasterization_state(double_sided: bool) -> RasterizationState {
    RasterizationState {
        cull_mode: if double_sided {
            CullMode::None
        } else {
            CullMode::Back
        },
        // NeoLOVE and the software renderer treat positive signed clip-space
        // area as the front shell. Vulkan preserves that CCW classification
        // for this viewport; declaring clockwise here culls the front shell
        // and exposes the back of closed meshes instead. Both variants must
        // agree because the fragment shader also uses gl_FrontFacing to orient
        // two-sided normals when culling is disabled.
        front_face: FrontFace::CounterClockwise,
        ..RasterizationState::default()
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

fn write_matrix_columns(
    slots: &mut [[f32; 4]; NATIVE_MESH_UNIFORM_SLOTS],
    first_slot: usize,
    matrix: crate::render3d::Mat4,
) {
    for (column, values) in matrix_columns(matrix).into_iter().enumerate() {
        slots[first_slot + column] = values;
    }
}

fn matrix_columns(matrix: crate::render3d::Mat4) -> [[f32; 4]; 4] {
    std::array::from_fn(|column| {
        [
            matrix.values[0][column],
            matrix.values[1][column],
            matrix.values[2][column],
            matrix.values[3][column],
        ]
    })
}

fn matrix_bits(matrix: crate::render3d::Mat4) -> [u32; 16] {
    let mut bits = [0; 16];
    for (index, value) in matrix.values.into_iter().flatten().enumerate() {
        bits[index] = value.to_bits();
    }
    bits
}

fn native_shadow_config(
    lights: &[crate::render3d::Light3D],
    camera: crate::render3d::Camera3D,
    aspect: f32,
) -> Option<NativeShadowConfig> {
    let (light_index, light) = lights
        .iter()
        .enumerate()
        .find(|(_, light)| {
            light.casts_shadows && light.kind == crate::render3d::LightKind3D::Directional
        })
        .or_else(|| {
            lights.iter().enumerate().find(|(_, light)| {
                light.casts_shadows && light.kind == crate::render3d::LightKind3D::Spot
            })
        })?;
    let view_projection = crate::render3d::shadow_projection_3d(*light, camera, aspect)?
        .view_projection;
    Some(NativeShadowConfig {
        light_index,
        view_projection,
        bias: light.shadow_bias.clamp(0.0, 0.1),
    })
}

fn native_mesh_instance(command: &crate::render3d::Mesh3DCommand) -> NativeMeshInstance {
    let model = matrix_columns(command.model);
    let normal = crate::render3d::NormalMatrix::from_model(command.model).values();
    NativeMeshInstance {
        model_0: model[0],
        model_1: model[1],
        model_2: model[2],
        model_3: model[3],
        normal_0: [normal[0][0], normal[1][0], normal[2][0], 0.0],
        normal_1: [normal[0][1], normal[1][1], normal[2][1], 0.0],
        normal_2: [normal[0][2], normal[1][2], normal[2][2], 0.0],
        tint: [
            command.tint.r as f32 / 255.0,
            command.tint.g as f32 / 255.0,
            command.tint.b as f32 / 255.0,
            command.tint.a as f32 / 255.0,
        ],
    }
}

fn native_mesh_uniforms(
    command: &crate::render3d::Mesh3DCommand,
    material: Option<&crate::mesh::MeshMaterial>,
    lights: &[crate::render3d::Light3D],
    camera: crate::render3d::Camera3D,
    shadow: Option<NativeShadowConfig>,
    environment: Option<NativeEnvironmentLighting>,
    skin_palette: Option<&[[f32; 16]]>,
    ambient_occlusion: Option<NativeAmbientOcclusion<'_>>,
) -> NativeMeshUniformBuffer {
    let mut slots = [[0.0; 4]; NATIVE_MESH_UNIFORM_SLOTS];
    write_matrix_columns(&mut slots, 0, command.model);
    write_matrix_columns(&mut slots, 4, command.view_projection);
    slots[8] = [
        command.tint.r as f32 / 255.0,
        command.tint.g as f32 / 255.0,
        command.tint.b as f32 / 255.0,
        command.tint.a as f32 / 255.0,
    ];
    slots[9] = material
        .map(|material| material.base_color)
        .unwrap_or([1.0; 4]);
    let emissive = material
        .map(|material| material.emissive)
        .unwrap_or([0.0; 3]);
    let metallic = material.map(|material| material.metallic).unwrap_or(0.0);
    let roughness = material.map(|material| material.roughness).unwrap_or(1.0);
    let alpha_cutoff = material
        .map(|material| material.alpha_cutoff)
        .unwrap_or(0.5);
    let has_normal_map = material.is_some_and(|material| {
        material
            .normal_texture
            .as_ref()
            .is_some_and(|binding| binding.tex_coord == 0 && binding.image.is_some())
    });
    let has_metallic_roughness_map = material.is_some_and(|material| {
        material
            .metallic_roughness_texture
            .as_ref()
            .is_some_and(|binding| binding.tex_coord == 0 && binding.image.is_some())
    });
    let has_emissive_map = material.is_some_and(|material| {
        material
            .emissive_texture
            .as_ref()
            .is_some_and(|binding| binding.tex_coord == 0 && binding.image.is_some())
    });
    let alpha_mode = material.map_or(0.0, |material| match material.alpha_mode {
        crate::mesh::AlphaMode::Opaque => 0.0,
        crate::mesh::AlphaMode::Mask => 1.0,
        crate::mesh::AlphaMode::Blend => 2.0,
    });
    slots[10] = [emissive[0], emissive[1], emissive[2], metallic];
    slots[11] = [
        roughness,
        alpha_cutoff,
        has_normal_map as u8 as f32,
        has_metallic_roughness_map as u8 as f32,
    ];
    slots[12] = [has_emissive_map as u8 as f32, alpha_mode, 0.0, 0.0];
    if let Some(environment) = environment {
        slots[12][2] = environment.mode;
        slots[12][3] = environment.intensity;
        slots[15][3] = environment.rotation_radians;
        if let Some(fog) = environment.fog {
            let fog = fog.sanitized();
            let encoded = fog.color_channels();
            slots[NATIVE_MESH_FOG_BASE_SLOT] = [
                encoded[0].powf(2.2),
                encoded[1].powf(2.2),
                encoded[2].powf(2.2),
                1.0,
            ];
            slots[NATIVE_MESH_FOG_BASE_SLOT + 1] = [
                fog.start_distance,
                fog.end_distance,
                fog.density,
                match fog.mode {
                    crate::environment3d::FogMode3D::Linear => 0.0,
                    crate::environment3d::FogMode3D::Exponential => 1.0,
                    crate::environment3d::FogMode3D::ExponentialSquared => 2.0,
                },
            ];
        }
    }
    if let Some(ambient_occlusion) = ambient_occlusion {
        let settings = ambient_occlusion.settings.sanitized();
        let count = ambient_occlusion
            .occluders
            .len()
            .min(crate::render3d::MAX_AMBIENT_OCCLUDERS_3D);
        slots[NATIVE_MESH_AO_CONFIG_SLOT] = [
            count as f32,
            settings.intensity,
            settings.radius,
            settings.bias,
        ];
        for (index, occluder) in ambient_occlusion.occluders.iter().take(count).enumerate() {
            let base = NATIVE_MESH_AO_OCCLUDER_BASE_SLOT + index * 2;
            slots[base] = [occluder.min.x, occluder.min.y, occluder.min.z, 0.0];
            slots[base + 1] = [occluder.max.x, occluder.max.y, occluder.max.z, 0.0];
        }
    }
    if let Some(probe) = environment.and_then(|environment| environment.reflection_probe) {
        slots[NATIVE_MESH_REFLECTION_PROBE_SLOT] = [
            probe.blend_weight.clamp(0.0, 1.0),
            probe.intensity.max(0.0),
            probe.rotation_radians,
            1.0,
        ];
    }
    slots[13] = [camera.position.x, camera.position.y, camera.position.z, 1.0];
    if let Some(shadow) = shadow {
        write_matrix_columns(&mut slots, 0, shadow.view_projection);
        slots[14] = [
            1.0,
            shadow.light_index as f32,
            shadow.bias,
            command.receives_shadows as u8 as f32,
        ];
    }

    let light_count = lights.len().min(MAX_NATIVE_MESH_LIGHTS);
    slots[15][0] = light_count as f32;
    if let Some(palette) = skin_palette.filter(|palette| palette.len() <= MAX_NATIVE_SKIN_JOINTS) {
        slots[15][1] = 1.0;
        slots[15][2] = palette.len() as f32;
        for (joint, matrix) in palette.iter().enumerate() {
            let base = NATIVE_MESH_SKIN_BASE_SLOT + joint * 4;
            for column in 0..4 {
                slots[base + column].copy_from_slice(&matrix[column * 4..column * 4 + 4]);
            }
        }
    }
    for (index, light) in lights.iter().take(light_count).enumerate() {
        let base = NATIVE_MESH_LIGHT_BASE_SLOT + index * 4;
        let kind = match light.kind {
            crate::render3d::LightKind3D::Directional => 0.0,
            crate::render3d::LightKind3D::Point => 1.0,
            crate::render3d::LightKind3D::Spot => 2.0,
        };
        slots[base] = [light.position.x, light.position.y, light.position.z, kind];
        slots[base + 1] = [
            light.direction.x,
            light.direction.y,
            light.direction.z,
            light.intensity,
        ];
        slots[base + 2] = [
            light.color.r as f32 / 255.0,
            light.color.g as f32 / 255.0,
            light.color.b as f32 / 255.0,
            light.range,
        ];
        slots[base + 3] = [light.spot_angle_radians, light.spot_softness, 0.0, 0.0];
    }
    NativeMeshUniformBuffer { slots }
}

fn native_shadow_uniforms(
    view_projection: crate::render3d::Mat4,
    skin_palette: Option<&[[f32; 16]]>,
) -> NativeMeshUniformBuffer {
    let mut slots = [[0.0; 4]; NATIVE_MESH_UNIFORM_SLOTS];
    write_matrix_columns(&mut slots, 4, view_projection);
    if let Some(palette) = skin_palette.filter(|palette| palette.len() <= MAX_NATIVE_SKIN_JOINTS) {
        slots[15][1] = 1.0;
        slots[15][2] = palette.len() as f32;
        for (joint, matrix) in palette.iter().enumerate() {
            let base = NATIVE_MESH_SKIN_BASE_SLOT + joint * 4;
            for column in 0..4 {
                slots[base + column].copy_from_slice(&matrix[column * 4..column * 4 + 4]);
            }
        }
    }
    NativeMeshUniformBuffer { slots }
}

fn push_vertices(
    current: &mut Option<TextureBatch>,
    batches: &mut Vec<RenderBatch>,
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
            batches.push(RenderBatch::Transient(finished_batch));
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
    fn native_mesh_front_face_is_stable_when_double_sided_disables_culling() {
        let single_sided = native_mesh_rasterization_state(false);
        let double_sided = native_mesh_rasterization_state(true);

        assert_eq!(single_sided.cull_mode, CullMode::Back);
        assert_eq!(double_sided.cull_mode, CullMode::None);
        assert_eq!(single_sided.front_face, FrontFace::CounterClockwise);
        assert_eq!(double_sided.front_face, FrontFace::CounterClockwise);
    }

    #[test]
    fn environment_shaders_parse_and_validate() {
        for (name, source) in [
            (
                "equirectangular environment",
                EQUIRECTANGULAR_ENVIRONMENT_FRAGMENT_SHADER,
            ),
            ("cubemap environment", CUBEMAP_ENVIRONMENT_FRAGMENT_SHADER),
        ] {
            let mut frontend = glsl::Frontend::default();
            let module = frontend
                .parse(&glsl::Options::from(naga::ShaderStage::Fragment), source)
                .unwrap_or_else(|error| panic!("{name} shader should parse: {error}"));
            Validator::new(ValidationFlags::all(), Capabilities::all())
                .validate(&module)
                .unwrap_or_else(|error| panic!("{name} shader should validate: {error}"));
        }
    }

    #[test]
    fn linear_hdr_and_tonemap_shaders_parse_and_validate() {
        for (name, source) in [
            ("built-in linear fragment", BUILTIN_FRAGMENT_SHADER),
            ("light composite fragment", LIGHT_COMPOSITE_FRAGMENT_SHADER),
            ("tone-map fragment", TONEMAP_FRAGMENT_SHADER),
            ("bloom extract fragment", BLOOM_EXTRACT_FRAGMENT_SHADER),
            ("bloom blur fragment", BLOOM_BLUR_FRAGMENT_SHADER),
        ] {
            let mut frontend = glsl::Frontend::default();
            let module = frontend
                .parse(&glsl::Options::from(naga::ShaderStage::Fragment), source)
                .unwrap_or_else(|error| panic!("{name} should parse: {error}"));
            Validator::new(ValidationFlags::all(), Capabilities::all())
                .validate(&module)
                .unwrap_or_else(|error| panic!("{name} should validate: {error}"));
        }
        assert_eq!(HDR_SCENE_FORMAT, Format::R16G16B16A16_SFLOAT);
        assert!(!NATIVE_MESH_FRAGMENT_SHADER.contains("1.0 / 2.2"));
    }

    #[test]
    fn native_tonemap_uses_last_enabled_exposure_pass_and_sanitizes_values() {
        use crate::post_process::{
            Effect, EffectPass, ExposureTonemapConfig, PostProcessStack, TonemapOperator,
        };
        let mut stack = PostProcessStack::default();
        stack.effects = vec![
            EffectPass::new(Effect::ExposureTonemap(ExposureTonemapConfig {
                exposure: 1.0,
                operator: TonemapOperator::Reinhard,
                gamma: 2.0,
            })),
            EffectPass {
                enabled: false,
                effect: Effect::ExposureTonemap(ExposureTonemapConfig {
                    exposure: 8.0,
                    operator: TonemapOperator::None,
                    gamma: 1.0,
                }),
            },
            EffectPass::new(Effect::ExposureTonemap(ExposureTonemapConfig {
                exposure: f32::INFINITY,
                operator: TonemapOperator::Aces,
                gamma: 99.0,
            })),
        ];
        assert_eq!(
            native_tonemap_settings(&stack),
            NativeTonemapSettings {
                exposure: 0.0,
                operator: 2.0,
                gamma: 8.0,
            }
        );
        stack.enabled = false;
        assert_eq!(native_tonemap_settings(&stack), NativeTonemapSettings::default());
    }

    #[test]
    fn native_bloom_is_bounded_and_skips_disabled_or_zero_work() {
        use crate::post_process::{BloomConfig, Effect, EffectPass, PostProcessStack};
        let mut stack = PostProcessStack::default();
        stack.effects = vec![EffectPass::new(Effect::Bloom(BloomConfig {
            threshold: -3.0,
            intensity: 1000.0,
            radius: 63,
        }))];
        assert_eq!(
            native_bloom_settings(&stack),
            Some(NativeBloomSettings {
                threshold: 0.0,
                intensity: 64.0,
                radius: 32.0,
            })
        );
        stack.effects[0].enabled = false;
        assert_eq!(native_bloom_settings(&stack), None);
        stack.effects[0] = EffectPass::new(Effect::Bloom(BloomConfig {
            threshold: 0.8,
            intensity: 1.0,
            radius: 0,
        }));
        assert_eq!(native_bloom_settings(&stack), None);
    }

    #[test]
    fn native_mesh_vertex_shader_parses_and_validates() {
        let mut frontend = glsl::Frontend::default();
        let module = frontend
            .parse(
                &glsl::Options::from(naga::ShaderStage::Vertex),
                NATIVE_MESH_VERTEX_SHADER,
            )
            .expect("native mesh vertex shader should parse");
        Validator::new(ValidationFlags::all(), Capabilities::all())
            .validate(&module)
            .expect("native mesh vertex shader should validate");
    }

    #[test]
    fn native_skin_palette_uses_bind_vertices_and_stable_geometry_revision() {
        let identity = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            1.0,
        ];
        let mut translated = identity;
        translated[12] = 2.5;
        let source = crate::mesh::primitive_mesh(
            "cube",
            crate::mesh::PrimitiveOptions::default(),
        )
        .expect("cube mesh")
        .snapshot()
        .expect("cube snapshot");
        let mut mesh = source.mesh.as_ref().clone();
        mesh.armature = Some(crate::mesh::Armature {
            nodes: vec![crate::mesh::ArmatureNode {
                name: "joint".into(),
                parent: None,
                translation: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
            }],
            joints: vec![0],
            inverse_bind_matrices: vec![identity],
            pose_palette: vec![translated],
            vertex_weights: vec![crate::mesh::SkinWeights::default(); mesh.vertices.len()],
            bind_vertices: mesh.vertices.clone(),
        });
        let first = crate::mesh::MeshSnapshot {
            revision: 12,
            geometry_revision: 0,
            geometry_identity: 41,
            mesh: Arc::new(mesh),
        };
        let second = crate::mesh::MeshSnapshot {
            revision: 13,
            geometry_revision: 0,
            geometry_identity: 41,
            mesh: first.mesh.clone(),
        };
        assert!(native_gpu_skinning(&first));
        assert_eq!(native_mesh_upload_revision(&first), 0);
        assert_eq!(native_mesh_upload_revision(&second), 0);
        let manually_edited = crate::mesh::MeshSnapshot {
            revision: 14,
            geometry_revision: 1,
            geometry_identity: 42,
            mesh: first.mesh.clone(),
        };
        assert!(!native_gpu_skinning(&manually_edited));
        assert_eq!(native_mesh_upload_revision(&manually_edited), 14);

        let influences = crate::mesh::SkinWeights {
            joints: [7, 5, 3, 1],
            weights: [0.4, 0.3, 0.2, 0.1],
        };
        let vertex = NativeMeshVertex::new(first.mesh.vertices[0], Some(influences));
        assert_eq!(vertex.joints, [7, 5, 3, 1]);
        assert_eq!(vertex.weights, influences.weights);
    }

    #[test]
    fn native_mesh_pbr_fragment_shader_parses_and_validates() {
        let mut frontend = glsl::Frontend::default();
        let module = frontend
            .parse(
                &glsl::Options::from(naga::ShaderStage::Fragment),
                NATIVE_MESH_FRAGMENT_SHADER,
            )
            .expect("native mesh PBR fragment shader should parse");
        let mut bindings = module
            .global_variables
            .iter()
            .filter_map(|(_, variable)| variable.binding.as_ref().map(|binding| binding.binding))
            .collect::<Vec<_>>();
        bindings.sort_unstable();
        assert_eq!(bindings, (0..=14).collect::<Vec<_>>());
        Validator::new(ValidationFlags::all(), Capabilities::all())
            .validate(&module)
            .expect("native mesh PBR fragment shader should validate");
    }

    #[test]
    fn native_shadow_shader_and_configuration_validate() {
        let mut frontend = glsl::Frontend::default();
        let module = frontend
            .parse(
                &glsl::Options::from(naga::ShaderStage::Vertex),
                NATIVE_SHADOW_VERTEX_SHADER,
            )
            .expect("native shadow vertex shader should parse");
        Validator::new(ValidationFlags::all(), Capabilities::all())
            .validate(&module)
            .expect("native shadow vertex shader should validate");

        let mut point = crate::render3d::Light3D {
            kind: crate::render3d::LightKind3D::Point,
            position: crate::render3d::Vec3::new(0.0, 4.0, 0.0),
            direction: crate::render3d::Vec3::new(0.0, -1.0, 0.0),
            color: Color::WHITE,
            intensity: 1.0,
            range: 20.0,
            spot_angle_radians: 1.0,
            spot_softness: 0.1,
            casts_shadows: true,
            shadow_bias: 0.007,
        };
        assert!(native_shadow_config(&[point], crate::render3d::Camera3D::default(), 1.0).is_none());
        point.kind = crate::render3d::LightKind3D::Spot;
        let spot = native_shadow_config(&[point], crate::render3d::Camera3D::default(), 1.0)
            .expect("shadow-casting spot light");
        assert_eq!(spot.light_index, 0);
        assert_eq!(spot.bias, 0.007);
        assert!(spot
            .view_projection
            .values
            .into_iter()
            .flatten()
            .all(f32::is_finite));

        let mut disabled = point;
        disabled.casts_shadows = false;
        let directional = crate::render3d::Light3D {
            kind: crate::render3d::LightKind3D::Directional,
            direction: crate::render3d::Vec3::new(-0.5, -1.0, -0.25),
            casts_shadows: true,
            shadow_bias: 0.003,
            ..point
        };
        let selected = native_shadow_config(
            &[disabled, point, directional],
            crate::render3d::Camera3D::default(),
            16.0 / 9.0,
        )
        .expect("directional shadow light should be preferred");
        assert_eq!(selected.light_index, 2);
        assert_eq!(selected.bias, 0.003);
        assert!(selected
            .view_projection
            .values
            .into_iter()
            .flatten()
            .all(f32::is_finite));
    }

    #[test]
    fn native_mesh_uniforms_preserve_transforms_materials_and_light_limit() {
        let mesh = crate::mesh::primitive_mesh(
            "cube",
            crate::mesh::PrimitiveOptions::default(),
        )
        .expect("cube mesh");
        let mut command = crate::render3d::Mesh3DCommand {
            mesh,
            model: crate::render3d::Mat4::trs(
                crate::render3d::Vec3::new(3.0, 4.0, 5.0),
                crate::render3d::Vec3::ZERO,
                crate::render3d::Vec3::new(2.0, 1.0, 0.5),
            ),
            view_projection: crate::render3d::Mat4::identity(),
            camera_position: crate::render3d::Vec3::new(0.0, 0.0, 5.0),
            tint: Color::rgba(128, 64, 32, 255),
            texture: None,
            materials: Vec::new(),
            shader: None,
            double_sided: false,
            casts_shadows: true,
            receives_shadows: true,
        };
        let mut material = crate::mesh::MeshMaterial::named("test");
        material.base_color = [0.25, 0.5, 0.75, 0.8];
        material.emissive = [0.1, 0.2, 0.3];
        material.metallic = 0.7;
        material.roughness = 0.35;
        material.alpha_mode = crate::mesh::AlphaMode::Mask;
        material.alpha_cutoff = 0.42;
        let map = crate::assets::ImageHandle::from_rgba_image(RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([128, 128, 255, 255]),
        ));
        material.normal_texture = Some(crate::mesh::TextureBinding {
            source: "normal.png".to_string(),
            tex_coord: 0,
            image: Some(map.clone()),
        });
        material.metallic_roughness_texture = Some(crate::mesh::TextureBinding {
            source: "metallic-roughness.png".to_string(),
            tex_coord: 0,
            image: Some(map.clone()),
        });
        material.emissive_texture = Some(crate::mesh::TextureBinding {
            source: "emissive.png".to_string(),
            tex_coord: 0,
            image: Some(map),
        });
        let lights = (0..MAX_NATIVE_MESH_LIGHTS + 7)
            .map(|index| crate::render3d::Light3D {
                kind: crate::render3d::LightKind3D::Point,
                position: crate::render3d::Vec3::new(index as f32, 2.0, 3.0),
                direction: crate::render3d::Vec3::new(0.0, -1.0, 0.0),
                color: Color::rgba(10, 20, 30, 255),
                intensity: 2.0,
                range: 12.0,
                spot_angle_radians: 0.7,
                spot_softness: 0.2,
                casts_shadows: false,
                shadow_bias: 0.005,
            })
            .collect::<Vec<_>>();
        let camera = crate::render3d::Camera3D {
            position: crate::render3d::Vec3::new(8.0, 9.0, 10.0),
            ..crate::render3d::Camera3D::default()
        };
        let uniforms = native_mesh_uniforms(
            &command,
            Some(&material),
            &lights,
            camera,
            None,
            None,
            None,
            None,
        );

        assert_eq!(uniforms.slots[3], [3.0, 4.0, 5.0, 1.0]);
        assert_eq!(uniforms.slots[9], material.base_color);
        assert_eq!(uniforms.slots[10], [0.1, 0.2, 0.3, 0.7]);
        assert_eq!(uniforms.slots[11], [0.35, 0.42, 1.0, 1.0]);
        assert_eq!(uniforms.slots[12], [1.0, 1.0, 0.0, 0.0]);
        assert_eq!(uniforms.slots[13], [8.0, 9.0, 10.0, 1.0]);
        assert_eq!(uniforms.slots[15][0], MAX_NATIVE_MESH_LIGHTS as f32);
        assert_eq!(uniforms.slots[NATIVE_MESH_LIGHT_BASE_SLOT][0], 0.0);
        assert_eq!(
            uniforms.slots[NATIVE_MESH_LIGHT_BASE_SLOT + (MAX_NATIVE_MESH_LIGHTS - 1) * 4][0],
            (MAX_NATIVE_MESH_LIGHTS - 1) as f32
        );

        let ao_occluders = [crate::render3d::AmbientOccluder3D {
            source_index: 8,
            min: crate::render3d::Vec3::new(-2.0, -0.5, -3.0),
            max: crate::render3d::Vec3::new(2.0, 0.5, 3.0),
            center: crate::render3d::Vec3::ZERO,
        }];
        let environment_lit = native_mesh_uniforms(
            &command,
            Some(&material),
            &lights,
            camera,
            None,
            Some(NativeEnvironmentLighting {
                panorama_texture: TextureKey(99),
                cubemap_texture: CubemapTextureKey(77),
                mode: 1.0,
                intensity: 2.5,
                rotation_radians: 0.75,
                fog: Some(crate::environment3d::Fog3D {
                    enabled: true,
                    mode: crate::environment3d::FogMode3D::ExponentialSquared,
                    color: Color::rgba(128, 64, 32, 255),
                    start_distance: 4.0,
                    end_distance: 80.0,
                    density: 0.03,
                }),
                reflection_probe: Some(NativeReflectionProbeLighting {
                    cubemap_texture: CubemapTextureKey(88),
                    intensity: 1.8,
                    rotation_radians: 0.25,
                    blend_weight: 0.6,
                }),
            }),
            None,
            Some(NativeAmbientOcclusion {
                settings: crate::environment3d::AmbientOcclusion3D {
                    enabled: true,
                    radius: 3.5,
                    intensity: 0.7,
                    bias: 0.04,
                },
                occluders: &ao_occluders,
            }),
        );
        assert_eq!(environment_lit.slots[12][2..], [1.0, 2.5]);
        assert_eq!(environment_lit.slots[15][3], 0.75);
        assert_eq!(
            environment_lit.slots[NATIVE_MESH_FOG_BASE_SLOT + 1],
            [4.0, 80.0, 0.03, 2.0]
        );
        assert_eq!(environment_lit.slots[NATIVE_MESH_FOG_BASE_SLOT][3], 1.0);
        assert_eq!(
            environment_lit.slots[NATIVE_MESH_AO_CONFIG_SLOT],
            [1.0, 0.7, 3.5, 0.04]
        );
        assert_eq!(
            environment_lit.slots[NATIVE_MESH_AO_OCCLUDER_BASE_SLOT],
            [-2.0, -0.5, -3.0, 0.0]
        );
        assert_eq!(
            environment_lit.slots[NATIVE_MESH_AO_OCCLUDER_BASE_SLOT + 1],
            [2.0, 0.5, 3.0, 0.0]
        );
        assert_eq!(
            environment_lit.slots[NATIVE_MESH_REFLECTION_PROBE_SLOT],
            [0.6, 1.8, 0.25, 1.0]
        );

        let mut palette = [[0.0; 16]; 2];
        palette[0][0] = 1.0;
        palette[0][5] = 1.0;
        palette[0][10] = 1.0;
        palette[0][15] = 1.0;
        palette[1] = palette[0];
        palette[1][12] = 4.0;
        let skinned = native_mesh_uniforms(
            &command,
            Some(&material),
            &lights,
            camera,
            None,
            None,
            Some(&palette),
            None,
        );
        assert_eq!(skinned.slots[15][1], 1.0);
        assert_eq!(skinned.slots[15][2], 2.0);
        assert_eq!(skinned.slots[NATIVE_MESH_SKIN_BASE_SLOT], [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(
            skinned.slots[NATIVE_MESH_SKIN_BASE_SLOT + 7],
            [4.0, 0.0, 0.0, 1.0]
        );

        let shadow_matrix = crate::render3d::Mat4::translation(
            crate::render3d::Vec3::new(1.0, 2.0, 3.0),
        );
        let shadowed = native_mesh_uniforms(
            &command,
            Some(&material),
            &lights,
            camera,
            Some(NativeShadowConfig {
                light_index: 3,
                view_projection: shadow_matrix,
                bias: 0.006,
            }),
            None,
            None,
            None,
        );
        assert_eq!(shadowed.slots[3], [1.0, 2.0, 3.0, 1.0]);
        assert_eq!(shadowed.slots[14], [1.0, 3.0, 0.006, 1.0]);
        command.receives_shadows = false;
        let non_receiver = native_mesh_uniforms(
            &command,
            Some(&material),
            &lights,
            camera,
            Some(NativeShadowConfig {
                light_index: 3,
                view_projection: shadow_matrix,
                bias: 0.006,
            }),
            None,
            None,
            None,
        );
        assert_eq!(non_receiver.slots[14][3], 0.0);
    }
}
