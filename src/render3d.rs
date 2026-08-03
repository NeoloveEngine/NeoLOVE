//! Backend-neutral 3D camera, lighting, transform, and mesh preparation.
//!
//! The editor and script runtime expose Euler angles because they are easy to
//! author. Rendering converts them to matrices once per entity. The resulting
//! projected triangles are shared by the Vulkan, software, and web presenters,
//! which keeps imported meshes deterministic on every supported platform.

use crate::assets::ImageHandle;
use crate::mesh::{MeshBounds, MeshHandle, MeshMaterial};
use crate::platform::Color;

const CLIP_EPSILON: f32 = 1.0e-5;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub(crate) const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    pub(crate) const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    fn from_array(value: [f32; 3]) -> Self {
        Self::new(value[0], value[1], value[2])
    }

    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }

    fn scale(self, amount: f32) -> Self {
        Self::new(self.x * amount, self.y * amount, self.z * amount)
    }

    fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    fn length_squared(self) -> f32 {
        self.dot(self)
    }

    fn normalized(self) -> Self {
        let length_squared = self.length_squared();
        if length_squared <= f32::EPSILON || !length_squared.is_finite() {
            Self::ZERO
        } else {
            self.scale(length_squared.sqrt().recip())
        }
    }
}

/// Row-major matrix multiplied by column vectors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Mat4 {
    pub values: [[f32; 4]; 4],
}

impl Default for Mat4 {
    fn default() -> Self {
        Self::identity()
    }
}

impl Mat4 {
    pub(crate) const fn identity() -> Self {
        Self {
            values: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }

    pub(crate) fn translation(value: Vec3) -> Self {
        let mut out = Self::identity();
        out.values[0][3] = value.x;
        out.values[1][3] = value.y;
        out.values[2][3] = value.z;
        out
    }

    pub(crate) fn scale(value: Vec3) -> Self {
        Self {
            values: [
                [value.x, 0.0, 0.0, 0.0],
                [0.0, value.y, 0.0, 0.0],
                [0.0, 0.0, value.z, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }

    /// Build an XYZ Euler rotation. Angles are authored in degrees; the
    /// resulting matrix applies X, then Y, then Z rotations.
    pub(crate) fn rotation_euler_degrees(euler: Vec3) -> Self {
        let x = euler.x.to_radians();
        let y = euler.y.to_radians();
        let z = euler.z.to_radians();
        let (sx, cx) = x.sin_cos();
        let (sy, cy) = y.sin_cos();
        let (sz, cz) = z.sin_cos();
        let rx = Self {
            values: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, cx, -sx, 0.0],
                [0.0, sx, cx, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        };
        let ry = Self {
            values: [
                [cy, 0.0, sy, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [-sy, 0.0, cy, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        };
        let rz = Self {
            values: [
                [cz, -sz, 0.0, 0.0],
                [sz, cz, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        };
        rz.mul(ry).mul(rx)
    }

    pub(crate) fn trs(position: Vec3, euler: Vec3, scale: Vec3) -> Self {
        Self::translation(position)
            .mul(Self::rotation_euler_degrees(euler))
            .mul(Self::scale(scale))
    }

    pub(crate) fn mul(self, rhs: Self) -> Self {
        let mut values = [[0.0; 4]; 4];
        for (row, output_row) in values.iter_mut().enumerate() {
            for (column, output) in output_row.iter_mut().enumerate() {
                *output = (0..4)
                    .map(|index| self.values[row][index] * rhs.values[index][column])
                    .sum();
            }
        }
        Self { values }
    }

    pub(crate) fn transform_vec4(self, value: [f32; 4]) -> [f32; 4] {
        let mut output = [0.0; 4];
        for (row, destination) in output.iter_mut().enumerate() {
            *destination = self.values[row][0] * value[0]
                + self.values[row][1] * value[1]
                + self.values[row][2] * value[2]
                + self.values[row][3] * value[3];
        }
        output
    }

    pub(crate) fn transform_point(self, value: Vec3) -> Vec3 {
        let transformed = self.transform_vec4([value.x, value.y, value.z, 1.0]);
        Vec3::new(transformed[0], transformed[1], transformed[2])
    }

    pub(crate) fn transform_direction(self, value: Vec3) -> Vec3 {
        let transformed = self.transform_vec4([value.x, value.y, value.z, 0.0]);
        Vec3::new(transformed[0], transformed[1], transformed[2])
    }

    /// Camera scale is intentionally ignored. This is the inverse of a
    /// translation followed by the authored Euler rotation.
    pub(crate) fn view(position: Vec3, euler: Vec3) -> Self {
        let rotation = Self::rotation_euler_degrees(euler);
        let mut inverse_rotation = Self::identity();
        for row in 0..3 {
            for column in 0..3 {
                inverse_rotation.values[row][column] = rotation.values[column][row];
            }
        }
        inverse_rotation.mul(Self::translation(position.scale(-1.0)))
    }

    /// Right-handed perspective matrix with Vulkan/WebGPU's zero-to-one depth.
    pub(crate) fn perspective(fov_degrees: f32, aspect: f32, near: f32, far: f32) -> Self {
        let near = near.max(0.0001);
        let far = far.max(near + 0.0001);
        let aspect = aspect.max(0.0001);
        let f = (fov_degrees.clamp(1.0, 179.0).to_radians() * 0.5)
            .tan()
            .recip();
        Self {
            values: [
                [f / aspect, 0.0, 0.0, 0.0],
                [0.0, f, 0.0, 0.0],
                [0.0, 0.0, far / (near - far), far * near / (near - far)],
                [0.0, 0.0, -1.0, 0.0],
            ],
        }
    }

    pub(crate) fn orthographic(size: f32, aspect: f32, near: f32, far: f32) -> Self {
        let half_height = size.max(0.0001);
        let half_width = half_height * aspect.max(0.0001);
        let near = near.max(0.0001);
        let far = far.max(near + 0.0001);
        Self {
            values: [
                [half_width.recip(), 0.0, 0.0, 0.0],
                [0.0, half_height.recip(), 0.0, 0.0],
                [0.0, 0.0, (near - far).recip(), near / (near - far)],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }
}

/// Inverse-transpose of a model matrix's upper-left 3x3. Normals cannot use
/// `transform_direction` when an entity has non-uniform scale: doing so tilts
/// them toward the largest scale axis and produces visibly incorrect light.
#[derive(Clone, Copy, Debug)]
struct NormalMatrix {
    values: [[f32; 3]; 3],
}

impl NormalMatrix {
    fn from_model(model: Mat4) -> Self {
        let m = model.values;
        // Cofactor matrix. Dividing it by det(A) produces inverse-transpose(A).
        let cofactors = [
            [
                m[1][1] * m[2][2] - m[1][2] * m[2][1],
                m[1][2] * m[2][0] - m[1][0] * m[2][2],
                m[1][0] * m[2][1] - m[1][1] * m[2][0],
            ],
            [
                m[0][2] * m[2][1] - m[0][1] * m[2][2],
                m[0][0] * m[2][2] - m[0][2] * m[2][0],
                m[0][1] * m[2][0] - m[0][0] * m[2][1],
            ],
            [
                m[0][1] * m[1][2] - m[0][2] * m[1][1],
                m[0][2] * m[1][0] - m[0][0] * m[1][2],
                m[0][0] * m[1][1] - m[0][1] * m[1][0],
            ],
        ];
        let determinant =
            m[0][0] * cofactors[0][0] + m[0][1] * cofactors[0][1] + m[0][2] * cofactors[0][2];
        if determinant.abs() <= f32::EPSILON || !determinant.is_finite() {
            // A zero-scale transform has no mathematically valid normal
            // matrix. Falling back to the model's direction transform is
            // deterministic and keeps the remaining non-zero axes useful.
            return Self {
                values: [
                    [m[0][0], m[0][1], m[0][2]],
                    [m[1][0], m[1][1], m[1][2]],
                    [m[2][0], m[2][1], m[2][2]],
                ],
            };
        }
        let inverse_determinant = determinant.recip();
        let mut values = cofactors;
        for row in &mut values {
            for value in row {
                *value *= inverse_determinant;
            }
        }
        Self { values }
    }

    fn transform(self, normal: Vec3) -> Vec3 {
        Vec3::new(
            self.values[0][0] * normal.x
                + self.values[0][1] * normal.y
                + self.values[0][2] * normal.z,
            self.values[1][0] * normal.x
                + self.values[1][1] * normal.y
                + self.values[1][2] * normal.z,
            self.values[2][0] * normal.x
                + self.values[2][1] * normal.y
                + self.values[2][2] * normal.z,
        )
        .normalized()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Projection3D {
    Perspective,
    Orthographic,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Camera3D {
    pub position: Vec3,
    pub euler: Vec3,
    pub projection: Projection3D,
    pub fov: f32,
    pub orthographic_size: f32,
    pub near_clip: f32,
    pub far_clip: f32,
}

impl Default for Camera3D {
    fn default() -> Self {
        Self {
            position: Vec3::new(0.0, 0.0, 5.0),
            euler: Vec3::ZERO,
            projection: Projection3D::Perspective,
            fov: 60.0,
            orthographic_size: 10.0,
            near_clip: 0.1,
            far_clip: 1000.0,
        }
    }
}

impl Camera3D {
    pub(crate) fn view_projection(self, aspect: f32) -> Mat4 {
        let projection = match self.projection {
            Projection3D::Perspective => {
                Mat4::perspective(self.fov, aspect, self.near_clip, self.far_clip)
            }
            Projection3D::Orthographic => Mat4::orthographic(
                self.orthographic_size,
                aspect,
                self.near_clip,
                self.far_clip,
            ),
        };
        projection.mul(Mat4::view(self.position, self.euler))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LightKind3D {
    Directional,
    Point,
    Spot,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Light3D {
    pub kind: LightKind3D,
    pub position: Vec3,
    /// World-space direction in which the light points.
    pub direction: Vec3,
    pub color: Color,
    pub intensity: f32,
    pub range: f32,
    pub spot_angle_radians: f32,
    pub spot_softness: f32,
    pub casts_shadows: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Mesh3DCommand {
    pub mesh: MeshHandle,
    pub model: Mat4,
    pub view_projection: Mat4,
    pub tint: Color,
    pub texture: Option<ImageHandle>,
    pub shader: Option<crate::shader::ShaderHandle>,
    pub double_sided: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProjectedVertex {
    /// Homogeneous clip-space position. Keeping W is required for
    /// perspective-correct attributes in software and by the GPU rasterizer.
    pub clip_position: [f32; 4],
    /// Normalized device coordinates; Z uses the zero-to-one depth range.
    pub ndc: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProjectedTriangle {
    pub vertices: [ProjectedVertex; 3],
    pub depth: f32,
}

fn color_channels(color: Color) -> [f32; 4] {
    [
        color.r as f32 / 255.0,
        color.g as f32 / 255.0,
        color.b as f32 / 255.0,
        color.a as f32 / 255.0,
    ]
}

fn light_vertex(position: Vec3, normal: Vec3, lights: &[Light3D]) -> [f32; 3] {
    let normal = normal.normalized();
    let mut illumination = [0.12; 3];

    // A mesh remains readable before the user adds a light, which is especially
    // useful in the editor and for imported-asset inspection.
    if lights.is_empty() {
        let headlight = normal.dot(Vec3::new(0.25, 0.45, 1.0).normalized()).max(0.0);
        for channel in &mut illumination {
            *channel += 0.35 + headlight * 0.53;
        }
    }

    for light in lights {
        let light_color = color_channels(light.color);
        let (direction_to_light, attenuation) = match light.kind {
            LightKind3D::Directional => (light.direction.scale(-1.0).normalized(), 1.0),
            LightKind3D::Point | LightKind3D::Spot => {
                let delta = light.position.sub(position);
                let distance_squared = delta.length_squared();
                let range = light.range.max(0.0001);
                let distance = distance_squared.sqrt();
                let normalized_distance = (distance / range).clamp(0.0, 1.0);
                let attenuation = (1.0 - normalized_distance * normalized_distance).powi(2);
                (delta.normalized(), attenuation)
            }
        };
        let mut contribution = normal.dot(direction_to_light).max(0.0) * attenuation;
        if light.kind == LightKind3D::Spot {
            let from_light = direction_to_light.scale(-1.0);
            let alignment = light.direction.normalized().dot(from_light);
            let outer = (light.spot_angle_radians.max(0.001) * 0.5).cos();
            let softness = light.spot_softness.clamp(0.0, 0.999);
            let inner = (outer + (1.0 - outer) * (1.0 - softness)).min(1.0);
            let spot = if inner <= outer + f32::EPSILON {
                (alignment >= outer) as u8 as f32
            } else {
                ((alignment - outer) / (inner - outer)).clamp(0.0, 1.0)
            };
            contribution *= spot;
        }
        contribution *= light.intensity.max(0.0);
        for channel in 0..3 {
            illumination[channel] += light_color[channel] * contribution;
        }
    }

    illumination
}

fn shade_material(
    illumination: [f32; 3],
    tint: [f32; 4],
    material: Option<&MeshMaterial>,
) -> [f32; 4] {
    let material_color = material.map(|value| value.base_color).unwrap_or([1.0; 4]);
    let emissive = material.map(|value| value.emissive).unwrap_or([0.0; 3]);
    [
        (tint[0] * material_color[0] * (illumination[0] + emissive[0])).clamp(0.0, 1.0),
        (tint[1] * material_color[1] * (illumination[1] + emissive[1])).clamp(0.0, 1.0),
        (tint[2] * material_color[2] * (illumination[2] + emissive[2])).clamp(0.0, 1.0),
        (tint[3] * material_color[3]).clamp(0.0, 1.0),
    ]
}

const MAX_CLIPPED_VERTICES: usize = 12;

#[derive(Clone, Copy, Debug)]
struct ClipVertex {
    clip_position: [f32; 4],
    uv: [f32; 2],
    color: [f32; 4],
}

impl ClipVertex {
    const ZERO: Self = Self {
        clip_position: [0.0; 4],
        uv: [0.0; 2],
        color: [0.0; 4],
    };

    fn interpolate(self, other: Self, amount: f32) -> Self {
        let amount = amount.clamp(0.0, 1.0);
        let mut result = Self::ZERO;
        for (channel, value) in result.clip_position.iter_mut().enumerate() {
            *value = self.clip_position[channel]
                + (other.clip_position[channel] - self.clip_position[channel]) * amount;
        }
        for (channel, value) in result.uv.iter_mut().enumerate() {
            *value = self.uv[channel] + (other.uv[channel] - self.uv[channel]) * amount;
        }
        for (channel, value) in result.color.iter_mut().enumerate() {
            *value = self.color[channel] + (other.color[channel] - self.color[channel]) * amount;
        }
        result
    }
}

fn clip_w(position: [f32; 4]) -> f32 {
    position[3] - CLIP_EPSILON
}

fn clip_left(position: [f32; 4]) -> f32 {
    position[0] + position[3]
}

fn clip_right(position: [f32; 4]) -> f32 {
    position[3] - position[0]
}

fn clip_bottom(position: [f32; 4]) -> f32 {
    position[1] + position[3]
}

fn clip_top(position: [f32; 4]) -> f32 {
    position[3] - position[1]
}

fn clip_near(position: [f32; 4]) -> f32 {
    position[2]
}

fn clip_far(position: [f32; 4]) -> f32 {
    position[3] - position[2]
}

const CLIP_PLANES: [fn([f32; 4]) -> f32; 7] = [
    clip_w,
    clip_left,
    clip_right,
    clip_bottom,
    clip_top,
    clip_near,
    clip_far,
];

fn clip_outcode(position: [f32; 4]) -> u8 {
    let mut outcode = 0;
    if clip_w(position) < 0.0 {
        outcode |= 1 << 0;
    }
    if clip_left(position) < 0.0 {
        outcode |= 1 << 1;
    }
    if clip_right(position) < 0.0 {
        outcode |= 1 << 2;
    }
    if clip_bottom(position) < 0.0 {
        outcode |= 1 << 3;
    }
    if clip_top(position) < 0.0 {
        outcode |= 1 << 4;
    }
    if clip_near(position) < 0.0 {
        outcode |= 1 << 5;
    }
    if clip_far(position) < 0.0 {
        outcode |= 1 << 6;
    }
    outcode
}

/// Clip a convex polygon against one homogeneous frustum plane without heap
/// allocation. A triangle can gain at most one vertex for each of the seven
/// planes, so the fixed scratch capacity is conservative.
fn clip_polygon_against_plane(
    input: &[ClipVertex; MAX_CLIPPED_VERTICES],
    input_len: usize,
    output: &mut [ClipVertex; MAX_CLIPPED_VERTICES],
    plane: fn([f32; 4]) -> f32,
) -> usize {
    if input_len == 0 {
        return 0;
    }

    let mut output_len = 0;
    let mut previous = input[input_len - 1];
    let mut previous_distance = plane(previous.clip_position);
    let mut previous_inside = previous_distance >= 0.0;
    for current in input.iter().take(input_len).copied() {
        let current_distance = plane(current.clip_position);
        let current_inside = current_distance >= 0.0;
        if current_inside != previous_inside {
            let denominator = previous_distance - current_distance;
            let amount = if denominator.abs() <= f32::EPSILON {
                0.0
            } else {
                previous_distance / denominator
            };
            debug_assert!(output_len < MAX_CLIPPED_VERTICES);
            output[output_len] = previous.interpolate(current, amount);
            output_len += 1;
        }
        if current_inside {
            debug_assert!(output_len < MAX_CLIPPED_VERTICES);
            output[output_len] = current;
            output_len += 1;
        }
        previous = current;
        previous_distance = current_distance;
        previous_inside = current_inside;
    }
    output_len
}

fn push_ready_triangle(
    projected: [ProjectedVertex; 3],
    double_sided: bool,
    output: &mut Vec<ProjectedTriangle>,
) {
    let signed_area = (projected[1].ndc[0] - projected[0].ndc[0])
        * (projected[2].ndc[1] - projected[0].ndc[1])
        - (projected[1].ndc[1] - projected[0].ndc[1]) * (projected[2].ndc[0] - projected[0].ndc[0]);
    if !double_sided && signed_area <= 0.0 {
        return;
    }
    output.push(ProjectedTriangle {
        vertices: projected,
        depth: (projected[0].ndc[2] + projected[1].ndc[2] + projected[2].ndc[2]) / 3.0,
    });
}

fn push_projected_triangle(
    clipped: [ClipVertex; 3],
    double_sided: bool,
    output: &mut Vec<ProjectedTriangle>,
) {
    let projected = clipped.map(|vertex| {
        let inverse_w = vertex.clip_position[3].recip();
        ProjectedVertex {
            clip_position: vertex.clip_position,
            ndc: [
                vertex.clip_position[0] * inverse_w,
                vertex.clip_position[1] * inverse_w,
                vertex.clip_position[2] * inverse_w,
            ],
            uv: vertex.uv,
            color: vertex.color,
        }
    });
    push_ready_triangle(projected, double_sided, output);
}

fn push_clipped_triangle(
    original: [ClipVertex; 3],
    double_sided: bool,
    output: &mut Vec<ProjectedTriangle>,
) {
    if original
        .iter()
        .any(|vertex| !vertex.clip_position.iter().all(|value| value.is_finite()))
    {
        return;
    }
    let outcodes = original.map(|vertex| clip_outcode(vertex.clip_position));
    let any_outcode = outcodes[0] | outcodes[1] | outcodes[2];
    if outcodes[0] & outcodes[1] & outcodes[2] != 0 {
        return;
    }
    if any_outcode == 0 {
        push_projected_triangle(original, double_sided, output);
        return;
    }

    let mut polygon_a = [ClipVertex::ZERO; MAX_CLIPPED_VERTICES];
    let mut polygon_b = [ClipVertex::ZERO; MAX_CLIPPED_VERTICES];
    polygon_a[..3].copy_from_slice(&original);
    let mut polygon_len = 3;
    for (plane_index, plane) in CLIP_PLANES.iter().copied().enumerate() {
        if any_outcode & (1 << plane_index) == 0 {
            continue;
        }
        polygon_len = clip_polygon_against_plane(&polygon_a, polygon_len, &mut polygon_b, plane);
        if polygon_len < 3 {
            return;
        }
        std::mem::swap(&mut polygon_a, &mut polygon_b);
    }
    for fan_index in 1..polygon_len.saturating_sub(1) {
        push_projected_triangle(
            [polygon_a[0], polygon_a[fan_index], polygon_a[fan_index + 1]],
            double_sided,
            output,
        );
    }
}

/// Expand a fixed-capacity emitter into camera-facing quads. All particles in
/// one component share the same texture and view-projection, so backends can
/// upload/draw the resulting vertices as one batch.
pub(crate) fn project_particles(
    command: &crate::particles3d::ParticleSystem3DCommand,
) -> Result<Vec<ProjectedTriangle>, String> {
    let particles = command.emitter.render_particles()?;
    let mut output = Vec::with_capacity(particles.len().saturating_mul(2));
    let camera_rotation = Mat4::rotation_euler_degrees(command.camera_euler);
    let camera_right = camera_rotation.transform_direction(Vec3::new(1.0, 0.0, 0.0));
    let camera_up = camera_rotation.transform_direction(Vec3::new(0.0, 1.0, 0.0));

    for particle in particles {
        if particle.size <= 0.0 || !particle.size.is_finite() || particle.color.a == 0 {
            continue;
        }
        let (sin, cos) = particle.rotation_degrees.to_radians().sin_cos();
        let half = particle.size * 0.5;
        let right = Vec3::new(
            (camera_right.x * cos + camera_up.x * sin) * half,
            (camera_right.y * cos + camera_up.y * sin) * half,
            (camera_right.z * cos + camera_up.z * sin) * half,
        );
        let up = Vec3::new(
            (-camera_right.x * sin + camera_up.x * cos) * half,
            (-camera_right.y * sin + camera_up.y * cos) * half,
            (-camera_right.z * sin + camera_up.z * cos) * half,
        );
        let corner = |right_amount: f32, up_amount: f32| {
            let position = Vec3::new(
                particle.position.x + right.x * right_amount + up.x * up_amount,
                particle.position.y + right.y * right_amount + up.y * up_amount,
                particle.position.z + right.z * right_amount + up.z * up_amount,
            );
            command
                .view_projection
                .transform_vec4([position.x, position.y, position.z, 1.0])
        };
        let color = color_channels(particle.color);
        let vertices = [
            ClipVertex {
                clip_position: corner(-1.0, 1.0),
                uv: [0.0, 0.0],
                color,
            },
            ClipVertex {
                clip_position: corner(-1.0, -1.0),
                uv: [0.0, 1.0],
                color,
            },
            ClipVertex {
                clip_position: corner(1.0, -1.0),
                uv: [1.0, 1.0],
                color,
            },
            ClipVertex {
                clip_position: corner(1.0, 1.0),
                uv: [1.0, 0.0],
                color,
            },
        ];
        push_clipped_triangle([vertices[0], vertices[1], vertices[2]], true, &mut output);
        push_clipped_triangle([vertices[0], vertices[2], vertices[3]], true, &mut output);
    }
    output.sort_by(|left, right| right.depth.total_cmp(&left.depth));
    Ok(output)
}

/// Conservatively reject an entire mesh before transforming or lighting its
/// vertices. Testing the eight cached AABB corners in homogeneous space works
/// for both perspective and orthographic cameras and does not reject meshes
/// that cross a frustum plane.
fn bounds_outside_clip(bounds: MeshBounds, model_view_projection: Mat4) -> bool {
    let mut corners = [[0.0; 4]; 8];
    let mut index = 0;
    for x in [bounds.min[0], bounds.max[0]] {
        for y in [bounds.min[1], bounds.max[1]] {
            for z in [bounds.min[2], bounds.max[2]] {
                let clip = model_view_projection.transform_vec4([x, y, z, 1.0]);
                if !clip.iter().all(|value| value.is_finite()) {
                    // Invalid user transforms should be handled by the
                    // per-vertex validation, never used for an unsafe cull.
                    return false;
                }
                corners[index] = clip;
                index += 1;
            }
        }
    }

    let outside_plane =
        |plane: fn([f32; 4]) -> f32| corners.iter().all(|corner| plane(*corner) < -CLIP_EPSILON);
    outside_plane(|clip| clip[0] + clip[3])
        || outside_plane(|clip| clip[3] - clip[0])
        || outside_plane(|clip| clip[1] + clip[3])
        || outside_plane(|clip| clip[3] - clip[1])
        || outside_plane(|clip| clip[2])
        || outside_plane(|clip| clip[3] - clip[2])
}

#[derive(Clone, Copy, Debug)]
struct PreparedVertex {
    clip_position: [f32; 4],
    ndc: [f32; 3],
    uv: [f32; 2],
    illumination: [f32; 3],
    finite: bool,
    outcode: u8,
}

/// Transform, light, cull, and project a mesh for any rendering backend.
pub(crate) fn project_mesh(
    command: &Mesh3DCommand,
    lights: &[Light3D],
) -> Result<Vec<ProjectedTriangle>, String> {
    let snapshot = command.mesh.snapshot().map_err(|error| error.to_string())?;
    let mesh = snapshot.mesh;
    let model_view_projection = command.view_projection.mul(command.model);
    if bounds_outside_clip(mesh.bounds, model_view_projection) {
        return Ok(Vec::new());
    }
    let tint = color_channels(command.tint);
    let mut output = Vec::with_capacity(mesh.indices.len() / 3);

    // Imported meshes heavily share indexed vertices. Prepare every unique
    // vertex once per entity instead of repeating transforms and every light
    // evaluation for each triangle corner.
    let normal_matrix = NormalMatrix::from_model(command.model);
    let mut prepared = Vec::with_capacity(mesh.vertices.len());
    for vertex in &mesh.vertices {
        let local_position = Vec3::from_array(vertex.position);
        let world_position = command.model.transform_point(local_position);
        let world_normal = normal_matrix.transform(Vec3::from_array(vertex.normal));
        let clip_position = model_view_projection.transform_vec4([
            local_position.x,
            local_position.y,
            local_position.z,
            1.0,
        ]);
        let finite = clip_position.iter().all(|value| value.is_finite());
        let outcode = if finite {
            clip_outcode(clip_position)
        } else {
            u8::MAX
        };
        let ndc = if outcode == 0 {
            let inverse_w = clip_position[3].recip();
            [
                clip_position[0] * inverse_w,
                clip_position[1] * inverse_w,
                clip_position[2] * inverse_w,
            ]
        } else {
            [0.0; 3]
        };
        prepared.push(PreparedVertex {
            clip_position,
            ndc,
            uv: vertex.uv,
            illumination: light_vertex(world_position, world_normal, lights),
            finite,
            outcode,
        });
    }

    for submesh in &mesh.submeshes {
        let first = submesh.first_index as usize;
        let end = first.saturating_add(submesh.index_count as usize);
        let Some(indices) = mesh.indices.get(first..end) else {
            return Err("mesh submesh range changed after validation".to_string());
        };
        // Resolve a submesh's material once. Per-vertex lighting above is
        // material-independent, so shared vertices never repeat light work.
        let material = submesh
            .material
            .and_then(|material_index| mesh.materials.get(material_index));
        let double_sided =
            command.double_sided || material.map(|value| value.double_sided).unwrap_or(false);

        for triangle in indices.chunks_exact(3) {
            let mut original = [ClipVertex::ZERO; 3];
            let mut original_ndc = [[0.0; 3]; 3];
            let mut any_outcode = 0;
            let mut common_outcode = u8::MAX;
            let mut invalid = false;
            for corner in 0..3 {
                let vertex_index = triangle[corner] as usize;
                let Some(vertex) = prepared.get(vertex_index) else {
                    return Err(format!(
                        "mesh triangle references missing vertex {vertex_index}"
                    ));
                };
                if !vertex.finite {
                    invalid = true;
                    break;
                }
                let outcode = vertex.outcode;
                any_outcode |= outcode;
                common_outcode &= outcode;
                original_ndc[corner] = vertex.ndc;
                original[corner] = ClipVertex {
                    clip_position: vertex.clip_position,
                    uv: vertex.uv,
                    color: shade_material(vertex.illumination, tint, material),
                };
            }
            if invalid || common_outcode != 0 {
                continue;
            }
            if any_outcode == 0 {
                let projected = std::array::from_fn(|corner| ProjectedVertex {
                    clip_position: original[corner].clip_position,
                    ndc: original_ndc[corner],
                    uv: original[corner].uv,
                    color: original[corner].color,
                });
                push_ready_triangle(projected, double_sided, &mut output);
                continue;
            }

            let mut polygon_a = [ClipVertex::ZERO; MAX_CLIPPED_VERTICES];
            let mut polygon_b = [ClipVertex::ZERO; MAX_CLIPPED_VERTICES];
            polygon_a[..3].copy_from_slice(&original);
            let mut polygon_len = 3;
            for (plane_index, plane) in CLIP_PLANES.iter().copied().enumerate() {
                if any_outcode & (1 << plane_index) == 0 {
                    continue;
                }
                polygon_len =
                    clip_polygon_against_plane(&polygon_a, polygon_len, &mut polygon_b, plane);
                if polygon_len < 3 {
                    break;
                }
                std::mem::swap(&mut polygon_a, &mut polygon_b);
            }

            for fan_index in 1..polygon_len.saturating_sub(1) {
                push_projected_triangle(
                    [polygon_a[0], polygon_a[fan_index], polygon_a[fan_index + 1]],
                    double_sided,
                    &mut output,
                );
            }
        }
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{MeshData, Submesh, Vertex};

    fn triangle_mesh() -> MeshHandle {
        let vertices = vec![
            Vertex::from_position([-0.5, -0.5, 0.0]),
            Vertex::from_position([0.5, -0.5, 0.0]),
            Vertex::from_position([0.0, 0.5, 0.0]),
        ];
        let mesh = MeshData::new(
            "triangle",
            vertices,
            vec![0, 1, 2],
            vec![Submesh {
                name: "triangle".into(),
                first_index: 0,
                index_count: 3,
                material: None,
            }],
            Vec::new(),
            true,
        )
        .expect("triangle mesh");
        MeshHandle::new(mesh).expect("mesh handle")
    }

    #[test]
    fn euler_trs_transforms_points_in_authored_axes() {
        let model = Mat4::trs(
            Vec3::new(2.0, 3.0, 4.0),
            Vec3::new(0.0, 0.0, 90.0),
            Vec3::new(2.0, 1.0, 1.0),
        );
        let point = model.transform_point(Vec3::new(1.0, 0.0, 0.0));
        assert!((point.x - 2.0).abs() < 0.0001);
        assert!((point.y - 5.0).abs() < 0.0001);
        assert!((point.z - 4.0).abs() < 0.0001);
    }

    #[test]
    fn perspective_maps_near_and_far_planes_to_zero_and_one() {
        let projection = Mat4::perspective(60.0, 16.0 / 9.0, 0.1, 100.0);
        for (z, expected) in [(-0.1, 0.0), (-100.0, 1.0)] {
            let clip = projection.transform_vec4([0.0, 0.0, z, 1.0]);
            let depth = clip[2] / clip[3];
            assert!((depth - expected).abs() < 0.0001, "depth={depth}");
        }
    }

    #[test]
    fn mesh_projection_produces_finite_visible_triangle() {
        let camera = Camera3D::default();
        let command = Mesh3DCommand {
            mesh: triangle_mesh(),
            model: Mat4::identity(),
            view_projection: camera.view_projection(16.0 / 9.0),
            tint: Color::WHITE,
            texture: None,
            shader: None,
            double_sided: true,
        };
        let triangles = project_mesh(&command, &[]).expect("project mesh");
        assert_eq!(triangles.len(), 1);
        assert!(
            triangles[0]
                .vertices
                .iter()
                .flat_map(|vertex| vertex.ndc)
                .all(f32::is_finite)
        );
        assert!(
            triangles[0]
                .vertices
                .iter()
                .all(|vertex| vertex.clip_position[3] > 0.0)
        );
    }

    #[test]
    fn particle_billboards_project_as_one_batched_quad() {
        let emitter = crate::particles3d::ParticleEmitterHandle::new(4, 11);
        emitter.emit(1).expect("emit particle");
        let config = crate::particles3d::ParticleConfig3D {
            max_particles: 4,
            playing: false,
            lifetime_min: 10.0,
            lifetime_max: 10.0,
            speed_min: 0.0,
            speed_max: 0.0,
            gravity: Vec3::ZERO,
            start_size: 1.0,
            end_size: 1.0,
            ..crate::particles3d::ParticleConfig3D::default()
        };
        emitter
            .step(0.0, Vec3::ZERO, Vec3::ZERO, &config)
            .expect("spawn particle");
        let camera = Camera3D::default();
        let triangles = project_particles(&crate::particles3d::ParticleSystem3DCommand {
            emitter,
            view_projection: camera.view_projection(1.0),
            camera_euler: camera.euler,
            texture: None,
            filter: crate::renderer::TextureFilter::Linear,
        })
        .expect("project particle batch");
        assert_eq!(triangles.len(), 2);
        assert!(
            triangles
                .iter()
                .flat_map(|triangle| triangle.vertices)
                .all(|vertex| vertex.ndc.iter().all(|value| value.is_finite()))
        );
    }

    #[test]
    fn triangle_crossing_near_plane_is_clipped_instead_of_disappearing() {
        let mut vertices = vec![
            Vertex::from_position([-0.02, -0.02, 4.95]),
            Vertex::from_position([0.3, -0.3, 4.0]),
            Vertex::from_position([0.0, 0.3, 4.0]),
        ];
        vertices[0].uv = [0.0, 0.0];
        vertices[1].uv = [1.0, 0.0];
        vertices[2].uv = [0.5, 1.0];
        let mesh = MeshHandle::new(
            MeshData::new(
                "near-plane triangle",
                vertices,
                vec![0, 1, 2],
                Vec::new(),
                Vec::new(),
                true,
            )
            .expect("near-plane mesh"),
        )
        .expect("near-plane handle");
        let command = Mesh3DCommand {
            mesh,
            model: Mat4::identity(),
            view_projection: Camera3D::default().view_projection(1.0),
            tint: Color::WHITE,
            texture: None,
            shader: None,
            double_sided: true,
        };

        let triangles = project_mesh(&command, &[]).expect("clip near-plane triangle");
        assert_eq!(
            triangles.len(),
            2,
            "a clipped quad should triangulate twice"
        );
        for vertex in triangles.iter().flat_map(|triangle| triangle.vertices) {
            assert!(vertex.clip_position[3] >= CLIP_EPSILON);
            assert!((-0.0001..=1.0001).contains(&vertex.ndc[2]));
            assert!(vertex.ndc.iter().all(|value| value.is_finite()));
            assert!(vertex.uv.iter().all(|value| value.is_finite()));
        }
    }

    #[test]
    fn mesh_bounds_reject_wholly_off_frustum_entity() {
        let camera = Camera3D::default();
        let command = Mesh3DCommand {
            mesh: triangle_mesh(),
            model: Mat4::translation(Vec3::new(10_000.0, 0.0, 0.0)),
            view_projection: camera.view_projection(1.0),
            tint: Color::WHITE,
            texture: None,
            shader: None,
            double_sided: true,
        };
        assert!(
            project_mesh(&command, &[])
                .expect("project offscreen mesh")
                .is_empty()
        );
    }

    #[test]
    fn nonuniform_scale_uses_inverse_transpose_normals_for_lighting() {
        let authored_normal = Vec3::new(1.0, 1.0, 0.0).normalized();
        let mut vertices = vec![
            Vertex::from_position([-0.5, -0.5, 0.0]),
            Vertex::from_position([0.5, -0.5, 0.0]),
            Vertex::from_position([0.0, 0.5, 0.0]),
        ];
        for vertex in &mut vertices {
            vertex.normal = [authored_normal.x, authored_normal.y, authored_normal.z];
        }
        let mesh = MeshHandle::new(
            MeshData::new(
                "nonuniform normal",
                vertices,
                vec![0, 1, 2],
                vec![Submesh {
                    name: "triangle".into(),
                    first_index: 0,
                    index_count: 3,
                    material: None,
                }],
                Vec::new(),
                false,
            )
            .expect("normal mesh"),
        )
        .expect("normal mesh handle");
        let light = Light3D {
            kind: LightKind3D::Directional,
            position: Vec3::ZERO,
            direction: Vec3::new(0.0, -1.0, 0.0),
            color: Color::WHITE,
            intensity: 0.5,
            range: 1.0,
            spot_angle_radians: 1.0,
            spot_softness: 0.0,
            casts_shadows: false,
        };
        let command = Mesh3DCommand {
            mesh,
            model: Mat4::scale(Vec3::new(2.0, 1.0, 1.0)),
            view_projection: Camera3D::default().view_projection(1.0),
            tint: Color::WHITE,
            texture: None,
            shader: None,
            double_sided: true,
        };
        let triangles = project_mesh(&command, &[light]).expect("project lit mesh");
        let red = triangles[0].vertices[0].color[0];
        // inverse-transpose(scale(2, 1, 1)) maps (1, 1, 0) to
        // normalize(0.5, 1, 0), whose Y contribution is ~0.894.
        assert!((red - 0.5672).abs() < 0.002, "unexpected light value {red}");
    }

    #[test]
    fn projection_preserves_distinct_clip_w_values() {
        let mesh = MeshHandle::new(
            MeshData::new(
                "depth varying triangle",
                vec![
                    Vertex::from_position([-0.5, -0.5, 0.0]),
                    Vertex::from_position([0.5, -0.5, -2.0]),
                    Vertex::from_position([0.0, 0.5, -1.0]),
                ],
                vec![0, 1, 2],
                vec![Submesh {
                    name: "triangle".into(),
                    first_index: 0,
                    index_count: 3,
                    material: None,
                }],
                Vec::new(),
                true,
            )
            .expect("varying depth mesh"),
        )
        .expect("varying depth handle");
        let command = Mesh3DCommand {
            mesh,
            model: Mat4::identity(),
            view_projection: Camera3D::default().view_projection(1.0),
            tint: Color::WHITE,
            texture: None,
            shader: None,
            double_sided: true,
        };
        let triangles = project_mesh(&command, &[]).expect("project varying depth mesh");
        let vertices = triangles[0].vertices;
        assert!((vertices[0].clip_position[3] - 5.0).abs() < 0.0001);
        assert!((vertices[1].clip_position[3] - 7.0).abs() < 0.0001);
        assert!((vertices[2].clip_position[3] - 6.0).abs() < 0.0001);
        for vertex in vertices {
            assert!(
                (vertex.ndc[0] - vertex.clip_position[0] / vertex.clip_position[3]).abs() < 1e-6
            );
        }
    }

    #[test]
    #[ignore = "release-only 3D preparation diagnostic"]
    fn benchmark_shared_index_mesh_many_entities() {
        use std::time::Instant;

        const CELLS: usize = 48;
        const ENTITIES: usize = 48;
        let mut shared_vertices = Vec::with_capacity((CELLS + 1) * (CELLS + 1));
        for y in 0..=CELLS {
            for x in 0..=CELLS {
                let mut vertex = Vertex::from_position([
                    x as f32 / CELLS as f32 * 2.0 - 1.0,
                    y as f32 / CELLS as f32 * 2.0 - 1.0,
                    0.0,
                ]);
                vertex.normal = [0.0, 0.0, 1.0];
                shared_vertices.push(vertex);
            }
        }
        let mut shared_indices = Vec::with_capacity(CELLS * CELLS * 6);
        for y in 0..CELLS {
            for x in 0..CELLS {
                let top_left = (y * (CELLS + 1) + x) as u32;
                let top_right = top_left + 1;
                let bottom_left = top_left + (CELLS + 1) as u32;
                let bottom_right = bottom_left + 1;
                shared_indices.extend_from_slice(&[
                    top_left,
                    top_right,
                    bottom_right,
                    top_left,
                    bottom_right,
                    bottom_left,
                ]);
            }
        }
        let expanded_vertices: Vec<Vertex> = shared_indices
            .iter()
            .map(|index| shared_vertices[*index as usize])
            .collect();
        let expanded_indices: Vec<u32> = (0..expanded_vertices.len() as u32).collect();
        let make_mesh = |name: &str, vertices, indices: Vec<u32>| {
            MeshHandle::new(
                MeshData::new(name, vertices, indices, Vec::new(), Vec::new(), false)
                    .expect("benchmark mesh"),
            )
            .expect("benchmark mesh handle")
        };
        let shared = make_mesh("shared grid", shared_vertices, shared_indices);
        let expanded = make_mesh("expanded grid", expanded_vertices, expanded_indices);
        let lights: Vec<Light3D> = (0..8)
            .map(|index| Light3D {
                kind: LightKind3D::Directional,
                position: Vec3::ZERO,
                direction: Vec3::new(index as f32 * 0.05, -0.25, -1.0),
                color: Color::WHITE,
                intensity: 0.1,
                range: 10.0,
                spot_angle_radians: 1.0,
                spot_softness: 0.0,
                casts_shadows: false,
            })
            .collect();
        let camera = Camera3D::default().view_projection(1.0);
        let run = |mesh: &MeshHandle| {
            let started = Instant::now();
            let mut triangles = 0;
            for entity in 0..ENTITIES {
                let command = Mesh3DCommand {
                    mesh: mesh.clone(),
                    model: Mat4::translation(Vec3::new(
                        (entity % 3) as f32 * 0.002,
                        (entity % 5) as f32 * 0.002,
                        0.0,
                    )),
                    view_projection: camera,
                    tint: Color::WHITE,
                    texture: None,
                    shader: None,
                    double_sided: true,
                };
                triangles += project_mesh(&command, &lights)
                    .expect("benchmark projection")
                    .len();
            }
            (started.elapsed(), triangles)
        };
        let (shared_time, shared_triangles) = run(&shared);
        let (expanded_time, expanded_triangles) = run(&expanded);
        assert_eq!(shared_triangles, expanded_triangles);
        eprintln!(
            "3D preparation, {ENTITIES} entities / {shared_triangles} triangles: shared-index={shared_time:?}, expanded={expanded_time:?}, speedup={:.2}x",
            expanded_time.as_secs_f64() / shared_time.as_secs_f64()
        );
    }
}
