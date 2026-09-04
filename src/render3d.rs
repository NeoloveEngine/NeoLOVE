//! Backend-neutral 3D camera, lighting, transform, and mesh preparation.
//!
//! The editor and script runtime expose Euler angles because they are easy to
//! author. Rendering converts them to matrices once per entity. The resulting
//! projected triangles are shared by the Vulkan, software, and web presenters,
//! which keeps imported meshes deterministic on every supported platform.

use crate::assets::ImageHandle;
use crate::mesh::{MaterialHandle, MaterialSnapshot, MeshBounds, MeshHandle, MeshMaterial};
use crate::platform::Color;
use image::RgbaImage;
use std::sync::Arc;

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

    pub(crate) fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }

    pub(crate) fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }

    pub(crate) fn scale(self, amount: f32) -> Self {
        Self::new(self.x * amount, self.y * amount, self.z * amount)
    }

    pub(crate) fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    pub(crate) fn cross(self, other: Self) -> Self {
        Self::new(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        )
    }

    pub(crate) fn length_squared(self) -> f32 {
        self.dot(self)
    }

    pub(crate) fn normalized(self) -> Self {
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
pub(crate) struct NormalMatrix {
    values: [[f32; 3]; 3],
}

impl NormalMatrix {
    pub(crate) fn from_model(model: Mat4) -> Self {
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

    pub(crate) fn values(self) -> [[f32; 3]; 3] {
        self.values
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
    /// Bit mask used to accept or reject authored RenderLayer3D components.
    /// The editor uses the same value when previewing through an authored
    /// camera, while its free Scene View camera defaults to every layer.
    pub render_mask: u32,
}

/// The editor exposes masks as signed 32-bit inspector integers, so reserve
/// the sign bit and provide 31 stable render layers on every target/Luau VM.
pub(crate) const ALL_RENDER_LAYERS_3D: u32 = i32::MAX as u32;

pub(crate) fn sanitize_render_mask_3d(value: i64) -> u32 {
    (value as u64 & ALL_RENDER_LAYERS_3D as u64) as u32
}

pub(crate) fn render_layers_intersect_3d(camera_mask: u32, entity_mask: u32) -> bool {
    camera_mask & entity_mask & ALL_RENDER_LAYERS_3D != 0
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
            render_mask: ALL_RENDER_LAYERS_3D,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LodForce3D {
    #[default]
    Automatic,
    Level(usize),
    Culled,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LodDistances3D {
    pub lod1: f32,
    pub lod2: f32,
    pub cull: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LodMeshSelection3D<'a> {
    pub requested_level: usize,
    pub active_level: usize,
    pub mesh_path: &'a str,
}

pub(crate) fn parse_lod_force_3d(value: &str) -> LodForce3D {
    match value.trim().to_ascii_lowercase().as_str() {
        "lod0" | "0" => LodForce3D::Level(0),
        "lod1" | "1" => LodForce3D::Level(1),
        "lod2" | "2" => LodForce3D::Level(2),
        "culled" | "cull" | "off" => LodForce3D::Culled,
        _ => LodForce3D::Automatic,
    }
}

pub(crate) fn lod_distances_3d(
    lod1_distance: f32,
    lod2_distance: f32,
    cull_distance: f32,
) -> LodDistances3D {
    let lod1 = if lod1_distance.is_finite() {
        lod1_distance.max(0.0)
    } else {
        20.0
    };
    let lod2 = if lod2_distance.is_finite() {
        lod2_distance.max(lod1)
    } else {
        50.0f32.max(lod1)
    };
    let cull = if cull_distance.is_finite() {
        cull_distance.max(lod2)
    } else {
        100.0f32.max(lod2)
    };
    LodDistances3D { lod1, lod2, cull }
}

/// Resolves the requested LOD to the nearest populated lower-detail source.
/// An empty LOD 0 path inherits MeshRenderer3D's base mesh path, preserving
/// existing scenes when an LOD group is added after the renderer.
pub(crate) fn resolve_lod_mesh_path_3d<'a>(
    base_mesh_path: &'a str,
    lod_mesh_paths: [&'a str; 3],
    requested_level: usize,
) -> LodMeshSelection3D<'a> {
    let requested_level = requested_level.min(2);
    for level in (0..=requested_level).rev() {
        let candidate = if level == 0 && lod_mesh_paths[0].trim().is_empty() {
            base_mesh_path
        } else {
            lod_mesh_paths[level]
        };
        if !candidate.trim().is_empty() {
            return LodMeshSelection3D {
                requested_level,
                active_level: level,
                mesh_path: candidate,
            };
        }
    }
    LodMeshSelection3D {
        requested_level,
        active_level: 0,
        mesh_path: base_mesh_path,
    }
}

pub(crate) fn select_lod_level_3d(
    distance: f32,
    lod1_distance: f32,
    lod2_distance: f32,
    cull_distance: f32,
    force: LodForce3D,
) -> Option<usize> {
    match force {
        LodForce3D::Level(level) => return Some(level.min(2)),
        LodForce3D::Culled => return None,
        LodForce3D::Automatic => {}
    }
    if !distance.is_finite() {
        return None;
    }
    let distances = lod_distances_3d(lod1_distance, lod2_distance, cull_distance);
    if distance >= distances.cull {
        None
    } else if distance >= distances.lod2 {
        Some(2)
    } else if distance >= distances.lod1 {
        Some(1)
    } else {
        Some(0)
    }
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
    pub shadow_bias: f32,
}

/// A local image-based-lighting volume. Reflection probes are queued by their
/// runtime components every frame, just like lights, so scene unloads and
/// visibility changes cannot leave stale renderer state behind.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReflectionProbe3D {
    pub source_id: usize,
    pub cubemap: crate::assets::CubemapHandle,
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    pub priority: i32,
    pub intensity: f32,
    pub rotation_degrees: f32,
    pub blend_distance: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ReflectionProbeSelection3D {
    pub index: usize,
    pub weight: f32,
}

impl ReflectionProbe3D {
    pub(crate) fn sanitized(mut self) -> Self {
        let fallback_min = Vec3::new(-5.0, -5.0, -5.0);
        let fallback_max = Vec3::new(5.0, 5.0, 5.0);
        if ![self.bounds_min.x, self.bounds_min.y, self.bounds_min.z]
            .into_iter()
            .all(f32::is_finite)
        {
            self.bounds_min = fallback_min;
        }
        if ![self.bounds_max.x, self.bounds_max.y, self.bounds_max.z]
            .into_iter()
            .all(f32::is_finite)
        {
            self.bounds_max = fallback_max;
        }
        let minimum = Vec3::new(
            self.bounds_min.x.min(self.bounds_max.x),
            self.bounds_min.y.min(self.bounds_max.y),
            self.bounds_min.z.min(self.bounds_max.z),
        );
        let maximum = Vec3::new(
            self.bounds_min.x.max(self.bounds_max.x),
            self.bounds_min.y.max(self.bounds_max.y),
            self.bounds_min.z.max(self.bounds_max.z),
        );
        self.bounds_min = minimum;
        self.bounds_max = maximum;
        self.intensity = if self.intensity.is_finite() {
            self.intensity.clamp(0.0, 64.0)
        } else {
            1.0
        };
        self.rotation_degrees = if self.rotation_degrees.is_finite() {
            self.rotation_degrees.rem_euclid(360.0)
        } else {
            0.0
        };
        let smallest_extent = maximum
            .sub(minimum)
            .scale(0.5)
            .x
            .min(maximum.sub(minimum).scale(0.5).y)
            .min(maximum.sub(minimum).scale(0.5).z)
            .max(0.0);
        self.blend_distance = if self.blend_distance.is_finite() {
            self.blend_distance.clamp(0.0, smallest_extent)
        } else {
            smallest_extent.min(1.0)
        };
        self
    }

    pub(crate) fn center(&self) -> Vec3 {
        self.bounds_min.add(self.bounds_max).scale(0.5)
    }
}

/// Select the strongest local probe at a receiver point. Higher priority wins;
/// ties prefer the volume with the greatest interior blend weight, then the
/// nearest center and stable source id. A point on a blended boundary has zero
/// local weight and therefore falls back cleanly to the global environment.
pub(crate) fn select_reflection_probe_3d(
    position: Vec3,
    probes: &[ReflectionProbe3D],
) -> Option<ReflectionProbeSelection3D> {
    let mut selected: Option<(usize, i32, f32, f32, usize)> = None;
    // RenderState sanitizes probes when they are queued. Keep this hot
    // per-mesh selection path allocation-free: cloning a probe also clones
    // its six reference-counted face handles for every probe/receiver pair.
    for (index, probe) in probes.iter().enumerate() {
        if position.x < probe.bounds_min.x
            || position.y < probe.bounds_min.y
            || position.z < probe.bounds_min.z
            || position.x > probe.bounds_max.x
            || position.y > probe.bounds_max.y
            || position.z > probe.bounds_max.z
        {
            continue;
        }
        let interior = (position.x - probe.bounds_min.x)
            .min(probe.bounds_max.x - position.x)
            .min(position.y - probe.bounds_min.y)
            .min(probe.bounds_max.y - position.y)
            .min(position.z - probe.bounds_min.z)
            .min(probe.bounds_max.z - position.z)
            .max(0.0);
        let weight = if probe.blend_distance <= f32::EPSILON {
            1.0
        } else {
            (interior / probe.blend_distance).clamp(0.0, 1.0)
        };
        if weight <= 0.0 {
            continue;
        }
        let distance = probe.center().sub(position).length_squared();
        let candidate = (index, probe.priority, weight, distance, probe.source_id);
        let replace = selected.is_none_or(|current| {
            candidate.1 > current.1
                || (candidate.1 == current.1
                    && (candidate.2 > current.2
                        || (candidate.2 == current.2
                            && (candidate.3 < current.3
                                || (candidate.3 == current.3 && candidate.4 < current.4)))))
        });
        if replace {
            selected = Some(candidate);
        }
    }
    selected.map(|(index, _, weight, _, _)| ReflectionProbeSelection3D { index, weight })
}

/// Runtime shadow projection plus its exact world-space frustum corners. The
/// editor consumes the corners for diagnostics while Vulkan consumes the
/// matrix, keeping both views tied to one calculation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ShadowProjection3D {
    pub view_projection: Mat4,
    pub corners: [Vec3; 8],
}

fn shadow_basis(direction: Vec3) -> (Vec3, Vec3, Vec3) {
    let mut forward = direction.normalized();
    if forward.length_squared() <= f32::EPSILON {
        forward = Vec3::new(0.0, 0.0, -1.0);
    }
    let up_seed = if forward.y.abs() > 0.95 {
        Vec3::new(1.0, 0.0, 0.0)
    } else {
        Vec3::new(0.0, 1.0, 0.0)
    };
    let right = forward.cross(up_seed).normalized();
    let up = right.cross(forward);
    (right, up, forward)
}

fn shadow_view_from_basis(position: Vec3, right: Vec3, up: Vec3, forward: Vec3) -> Mat4 {
    Mat4 {
        values: [
            [right.x, right.y, right.z, -right.dot(position)],
            [up.x, up.y, up.z, -up.dot(position)],
            [-forward.x, -forward.y, -forward.z, forward.dot(position)],
            [0.0, 0.0, 0.0, 1.0],
        ],
    }
}

fn shadow_frustum_corners(
    position: Vec3,
    right: Vec3,
    up: Vec3,
    forward: Vec3,
    near: f32,
    far: f32,
    near_half: f32,
    far_half: f32,
) -> [Vec3; 8] {
    let plane = |distance: f32, half: f32| {
        let center = position.add(forward.scale(distance));
        [
            center.add(right.scale(-half)).add(up.scale(-half)),
            center.add(right.scale(half)).add(up.scale(-half)),
            center.add(right.scale(half)).add(up.scale(half)),
            center.add(right.scale(-half)).add(up.scale(half)),
        ]
    };
    let near_plane = plane(near, near_half);
    let far_plane = plane(far, far_half);
    [
        near_plane[0],
        near_plane[1],
        near_plane[2],
        near_plane[3],
        far_plane[0],
        far_plane[1],
        far_plane[2],
        far_plane[3],
    ]
}

pub(crate) fn shadow_projection_3d(
    light: Light3D,
    camera: Camera3D,
    aspect: f32,
) -> Option<ShadowProjection3D> {
    let (right, up, direction) = shadow_basis(light.direction);
    match light.kind {
        LightKind3D::Point => None,
        LightKind3D::Spot => {
            let near = 0.05;
            let far = light.range.max(0.1);
            let half_tangent = (light
                .spot_angle_radians
                .clamp(0.1f32.to_radians(), 179.0f32.to_radians())
                * 0.5)
                .tan();
            Some(ShadowProjection3D {
                view_projection: Mat4::perspective(
                    light.spot_angle_radians.to_degrees(),
                    1.0,
                    near,
                    far,
                )
                .mul(shadow_view_from_basis(light.position, right, up, direction)),
                corners: shadow_frustum_corners(
                    light.position,
                    right,
                    up,
                    direction,
                    near,
                    far,
                    half_tangent * near,
                    half_tangent * far,
                ),
            })
        }
        LightKind3D::Directional => {
            let camera_rotation = Mat4::rotation_euler_degrees(camera.euler);
            let camera_right = camera_rotation.transform_direction(Vec3::new(1.0, 0.0, 0.0));
            let camera_up = camera_rotation.transform_direction(Vec3::new(0.0, 1.0, 0.0));
            let camera_forward = camera_rotation.transform_direction(Vec3::new(0.0, 0.0, -1.0));
            let camera_near = camera.near_clip.max(0.05);
            let camera_far = camera.far_clip.min(100.0).max(camera_near + 1.0);
            let mut camera_corners = Vec::with_capacity(8);
            for depth in [camera_near, camera_far] {
                let (half_width, half_height) = match camera.projection {
                    Projection3D::Perspective => {
                        let half_height =
                            (camera.fov.clamp(1.0, 179.0).to_radians() * 0.5).tan() * depth;
                        (half_height * aspect.max(0.0001), half_height)
                    }
                    Projection3D::Orthographic => {
                        let half_height = camera.orthographic_size.max(0.0001);
                        (half_height * aspect.max(0.0001), half_height)
                    }
                };
                let center = camera.position.add(camera_forward.scale(depth));
                for x in [-1.0, 1.0] {
                    for y in [-1.0, 1.0] {
                        camera_corners.push(
                            center
                                .add(camera_right.scale(half_width * x))
                                .add(camera_up.scale(half_height * y)),
                        );
                    }
                }
            }
            let center = camera_corners
                .iter()
                .copied()
                .fold(Vec3::ZERO, Vec3::add)
                .scale(1.0 / camera_corners.len() as f32);
            let radius = camera_corners
                .iter()
                .map(|corner| corner.sub(center).length_squared())
                .fold(1.0f32, f32::max)
                .sqrt()
                .max(1.0);
            let padding = radius * 0.1 + 1.0;
            let position = center.sub(direction.scale(radius + padding));
            let near = 0.05;
            let far = radius * 2.0 + padding * 2.0;
            let half = radius + padding;
            Some(ShadowProjection3D {
                view_projection: Mat4::orthographic(half, 1.0, near, far)
                    .mul(shadow_view_from_basis(position, right, up, direction)),
                corners: shadow_frustum_corners(
                    position, right, up, direction, near, far, half, half,
                ),
            })
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Mesh3DCommand {
    pub mesh: MeshHandle,
    pub model: Mat4,
    pub view_projection: Mat4,
    pub camera_position: Vec3,
    pub tint: Color,
    pub texture: Option<ImageHandle>,
    /// Optional reusable material overrides by zero-based imported material
    /// slot. `None` entries retain the mesh-embedded material.
    pub materials: Vec<Option<MaterialHandle>>,
    pub shader: Option<crate::shader::ShaderHandle>,
    pub double_sided: bool,
    pub casts_shadows: bool,
    pub receives_shadows: bool,
}

impl Mesh3DCommand {
    pub(crate) fn material_override_snapshots(
        &self,
    ) -> Result<Vec<Option<MaterialSnapshot>>, String> {
        self.materials
            .iter()
            .map(|material| {
                material
                    .as_ref()
                    .map(|material| material.snapshot().map_err(|error| error.to_string()))
                    .transpose()
            })
            .collect()
    }

    /// Resolve imported base-color images once per draw command. The explicit
    /// component texture remains an override and is handled by each backend.
    pub(crate) fn material_base_color_textures(&self) -> Result<Vec<Option<ImageHandle>>, String> {
        let overrides = self.material_override_snapshots()?;
        self.mesh
            .with_read(|mesh, _| {
                let count = mesh.materials.len().max(overrides.len());
                (0..count)
                    .map(|index| {
                        overrides
                            .get(index)
                            .and_then(Option::as_ref)
                            .map(|snapshot| snapshot.material.as_ref())
                            .or_else(|| mesh.materials.get(index))
                            .and_then(|material| material.base_color_texture.as_ref())
                            .and_then(|binding| binding.image.clone())
                    })
                    .collect()
            })
            .map_err(|error| error.to_string())
    }

    pub(crate) fn resolved_materials(
        &self,
    ) -> Result<Vec<Option<std::sync::Arc<MeshMaterial>>>, String> {
        let overrides = self.material_override_snapshots()?;
        self.mesh
            .with_read(|mesh, _| {
                let count = mesh.materials.len().max(overrides.len());
                (0..count)
                    .map(|index| {
                        overrides
                            .get(index)
                            .and_then(Option::as_ref)
                            .map(|snapshot| snapshot.material.clone())
                            .or_else(|| mesh.materials.get(index).cloned().map(std::sync::Arc::new))
                    })
                    .collect()
            })
            .map_err(|error| error.to_string())
    }
}

pub(crate) const MAX_AMBIENT_OCCLUDERS_3D: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AmbientOccluder3D {
    pub source_index: usize,
    pub min: Vec3,
    pub max: Vec3,
    pub center: Vec3,
}

pub(crate) fn mesh_world_bounds_3d(command: &Mesh3DCommand) -> Result<AmbientOccluder3D, String> {
    let snapshot = command.mesh.snapshot().map_err(|error| error.to_string())?;
    let bounds = snapshot.mesh.bounds;
    let mut min = Vec3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
    let mut max = Vec3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for x in [bounds.min[0], bounds.max[0]] {
        for y in [bounds.min[1], bounds.max[1]] {
            for z in [bounds.min[2], bounds.max[2]] {
                let point = command.model.transform_point(Vec3::new(x, y, z));
                min.x = min.x.min(point.x);
                min.y = min.y.min(point.y);
                min.z = min.z.min(point.z);
                max.x = max.x.max(point.x);
                max.y = max.y.max(point.y);
                max.z = max.z.max(point.z);
            }
        }
    }
    if ![min.x, min.y, min.z, max.x, max.y, max.z]
        .into_iter()
        .all(f32::is_finite)
    {
        return Err("mesh world bounds contain a non-finite transform".to_string());
    }
    Ok(AmbientOccluder3D {
        source_index: 0,
        min,
        max,
        center: min.add(max).scale(0.5),
    })
}

pub(crate) fn gather_ambient_occluders_3d<'a>(
    commands: impl IntoIterator<Item = &'a crate::renderer::DrawCommand>,
) -> Vec<AmbientOccluder3D> {
    commands
        .into_iter()
        .enumerate()
        .filter_map(|(source_index, command)| {
            let crate::renderer::DrawCommand::Mesh3D(command) = command else {
                return None;
            };
            if !command.casts_shadows {
                return None;
            }
            mesh_world_bounds_3d(command).ok().map(|mut bounds| {
                bounds.source_index = source_index;
                bounds
            })
        })
        .collect()
}

pub(crate) fn select_ambient_occluders_3d(
    source_index: usize,
    receiver: AmbientOccluder3D,
    occluders: &[AmbientOccluder3D],
) -> Vec<AmbientOccluder3D> {
    let mut selected = Vec::with_capacity(MAX_AMBIENT_OCCLUDERS_3D);
    for occluder in occluders
        .iter()
        .copied()
        .filter(|occluder| occluder.source_index != source_index)
    {
        let distance = occluder.center.sub(receiver.center).length_squared();
        let insertion = selected
            .binary_search_by(|candidate: &AmbientOccluder3D| {
                candidate
                    .center
                    .sub(receiver.center)
                    .length_squared()
                    .total_cmp(&distance)
                    .then_with(|| candidate.source_index.cmp(&occluder.source_index))
            })
            .unwrap_or_else(|index| index);
        if insertion < MAX_AMBIENT_OCCLUDERS_3D {
            selected.insert(insertion, occluder);
            selected.truncate(MAX_AMBIENT_OCCLUDERS_3D);
        }
    }
    selected
}

pub(crate) fn ambient_occlusion_visibility_3d(
    settings: crate::environment3d::AmbientOcclusion3D,
    world_position: Vec3,
    world_normal: Vec3,
    occluders: &[AmbientOccluder3D],
) -> f32 {
    if !settings.enabled || occluders.is_empty() {
        return 1.0;
    }
    let settings = settings.sanitized();
    let normal = world_normal.normalized();
    if normal.length_squared() <= f32::EPSILON {
        return 1.0;
    }
    let mut visibility = 1.0;
    for occluder in occluders.iter().take(MAX_AMBIENT_OCCLUDERS_3D) {
        let closest = Vec3::new(
            world_position.x.clamp(occluder.min.x, occluder.max.x),
            world_position.y.clamp(occluder.min.y, occluder.max.y),
            world_position.z.clamp(occluder.min.z, occluder.max.z),
        );
        let closest_delta = closest.sub(world_position);
        let distance = closest_delta.length_squared().sqrt();
        if distance > settings.radius {
            continue;
        }
        let direction = if distance > settings.bias.max(0.000001) {
            closest_delta.scale(distance.recip())
        } else {
            occluder.center.sub(world_position).normalized()
        };
        let alignment = normal.dot(direction).max(0.0);
        if alignment <= 0.0 {
            continue;
        }
        let half_extent = occluder.max.sub(occluder.min).scale(0.5);
        let extent = half_extent.length_squared().sqrt().max(0.0001);
        let angular_size = (extent / (extent + distance.max(settings.bias))).clamp(0.0, 1.0);
        let proximity = (1.0
            - ((distance - settings.bias).max(0.0)
                / (settings.radius - settings.bias).max(0.0001)))
        .clamp(0.0, 1.0);
        let occlusion = alignment * proximity * proximity * angular_size * settings.intensity;
        visibility *= 1.0 - occlusion.clamp(0.0, settings.intensity);
    }
    visibility.max(1.0 - settings.intensity).clamp(0.0, 1.0)
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
    pub world_position: [f32; 3],
    pub world_normal: [f32; 3],
    pub world_tangent: [f32; 3],
    pub tangent_sign: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProjectedTriangle {
    pub vertices: [ProjectedVertex; 3],
    pub depth: f32,
    /// Index into the source mesh's material array.
    pub material: Option<usize>,
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

const SOFTWARE_GAMMA_LUT_SIZE: usize = 4096;

fn software_gamma_decode(value: f32) -> f32 {
    static TABLE: std::sync::OnceLock<[f32; SOFTWARE_GAMMA_LUT_SIZE]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        std::array::from_fn(|index| (index as f32 / (SOFTWARE_GAMMA_LUT_SIZE - 1) as f32).powf(2.2))
    });
    let index = (value.clamp(0.0, 1.0) * (SOFTWARE_GAMMA_LUT_SIZE - 1) as f32).round() as usize;
    table[index]
}

fn software_gamma_encode(value: f32) -> f32 {
    static TABLE: std::sync::OnceLock<[f32; SOFTWARE_GAMMA_LUT_SIZE]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        std::array::from_fn(|index| {
            (index as f32 / (SOFTWARE_GAMMA_LUT_SIZE - 1) as f32).powf(1.0 / 2.2)
        })
    });
    let index = (value.clamp(0.0, 1.0) * (SOFTWARE_GAMMA_LUT_SIZE - 1) as f32).round() as usize;
    table[index]
}

/// Blend an sRGB surface color toward the authored fog color in linear light.
/// Alpha is intentionally preserved so fog never turns masked/transparent
/// geometry opaque.
pub(crate) fn apply_fog_srgb(
    mut color: [f32; 4],
    world_position: [f32; 3],
    camera_position: Vec3,
    fog: crate::environment3d::Fog3D,
) -> [f32; 4] {
    let amount = fog.amount(camera_position, Vec3::from_array(world_position));
    if amount <= 0.0 {
        return color;
    }
    let fog_color = fog.color_channels();
    for channel in 0..3 {
        let surface_linear = software_gamma_decode(color[channel]);
        let fog_linear = software_gamma_decode(fog_color[channel]);
        color[channel] =
            software_gamma_encode(surface_linear + (fog_linear - surface_linear) * amount);
    }
    color
}

pub(crate) fn apply_ambient_occlusion_srgb(mut color: [f32; 4], visibility: f32) -> [f32; 4] {
    let visibility = visibility.clamp(0.0, 1.0);
    for channel in 0..3 {
        color[channel] = software_gamma_encode(software_gamma_decode(color[channel]) * visibility);
    }
    color
}

pub(crate) fn apply_ambient_occlusion_to_projected_triangles(
    triangles: &mut [ProjectedTriangle],
    settings: crate::environment3d::AmbientOcclusion3D,
    occluders: &[AmbientOccluder3D],
) {
    if !settings.enabled || occluders.is_empty() {
        return;
    }
    for triangle in triangles {
        for vertex in &mut triangle.vertices {
            let visibility = ambient_occlusion_visibility_3d(
                settings,
                Vec3::from_array(vertex.world_position),
                Vec3::from_array(vertex.world_normal),
                occluders,
            );
            vertex.color = apply_ambient_occlusion_srgb(vertex.color, visibility);
        }
    }
}

pub(crate) fn apply_fog_to_projected_triangles(
    triangles: &mut [ProjectedTriangle],
    camera_position: Vec3,
    fog: crate::environment3d::Fog3D,
) {
    if !fog.enabled {
        return;
    }
    for triangle in triangles {
        for vertex in &mut triangle.vertices {
            vertex.color =
                apply_fog_srgb(vertex.color, vertex.world_position, camera_position, fog);
        }
    }
}

/// Immutable environment snapshot shared by every software-rendered PBR draw
/// in a frame. Keeping images behind `Arc`s avoids copying pixels while also
/// making live image revisions deterministic for the duration of the frame.
#[derive(Clone)]
pub(crate) struct PbrEnvironment {
    source: PbrEnvironmentSource,
    intensity: f32,
    rotation_radians: f32,
    fallback: Option<Box<PbrEnvironment>>,
    blend_weight: f32,
}

#[derive(Clone)]
enum PbrEnvironmentSource {
    Equirectangular(Arc<RgbaImage>),
    Cubemap([Arc<RgbaImage>; 6]),
}

impl PbrEnvironment {
    pub(crate) fn new(image: Arc<RgbaImage>, intensity: f32, rotation_degrees: f32) -> Self {
        Self {
            source: PbrEnvironmentSource::Equirectangular(image),
            intensity: if intensity.is_finite() {
                intensity.max(0.0)
            } else {
                1.0
            },
            rotation_radians: if rotation_degrees.is_finite() {
                rotation_degrees.to_radians()
            } else {
                0.0
            },
            fallback: None,
            blend_weight: 1.0,
        }
    }

    pub(crate) fn new_cubemap(
        faces: [Arc<RgbaImage>; 6],
        intensity: f32,
        rotation_degrees: f32,
    ) -> Self {
        let mut environment = Self::new(faces[0].clone(), intensity, rotation_degrees);
        environment.source = PbrEnvironmentSource::Cubemap(faces);
        environment
    }

    /// Blend a local probe over the global environment. The local source stays
    /// primary so a missing global image naturally fades toward black rather
    /// than inventing a synthetic headlight at the probe boundary.
    pub(crate) fn blended(
        mut local: PbrEnvironment,
        fallback: Option<PbrEnvironment>,
        weight: f32,
    ) -> Self {
        local.fallback = fallback.map(Box::new);
        local.blend_weight = if weight.is_finite() {
            weight.clamp(0.0, 1.0)
        } else {
            1.0
        };
        local
    }

    fn sample_image(&self, image: &RgbaImage, uv: [f32; 2]) -> [f32; 3] {
        if image.width() == 0 || image.height() == 0 {
            return [0.0; 3];
        }
        let x = uv[0].clamp(0.0, 1.0) * image.width().saturating_sub(1) as f32;
        let y = uv[1].clamp(0.0, 1.0) * image.height().saturating_sub(1) as f32;
        let x0 = x.floor() as u32;
        let y0 = y.floor() as u32;
        let x1 = (x0 + 1).min(image.width() - 1);
        let y1 = (y0 + 1).min(image.height() - 1);
        let tx = x - x0 as f32;
        let ty = y - y0 as f32;
        let pixels = [
            image.get_pixel(x0, y0).0,
            image.get_pixel(x1, y0).0,
            image.get_pixel(x0, y1).0,
            image.get_pixel(x1, y1).0,
        ];
        std::array::from_fn(|channel| {
            let top = pixels[0][channel] as f32
                + (pixels[1][channel] as f32 - pixels[0][channel] as f32) * tx;
            let bottom = pixels[2][channel] as f32
                + (pixels[3][channel] as f32 - pixels[2][channel] as f32) * tx;
            software_gamma_decode((top + (bottom - top) * ty) / 255.0) * self.intensity
        })
    }

    fn sample_source(&self, input_direction: Vec3) -> [f32; 3] {
        match &self.source {
            PbrEnvironmentSource::Equirectangular(image) => {
                let direction = input_direction.normalized();
                let (yaw_sin, yaw_cos) = self.rotation_radians.sin_cos();
                let rotated = Vec3::new(
                    direction.x * yaw_cos - direction.z * yaw_sin,
                    direction.y,
                    direction.x * yaw_sin + direction.z * yaw_cos,
                );
                let u = (rotated.z.atan2(rotated.x) / std::f32::consts::TAU + 0.5).rem_euclid(1.0);
                let v = (0.5 - rotated.y.clamp(-1.0, 1.0).asin() / std::f32::consts::PI)
                    .clamp(0.0, 1.0);
                self.sample_image(image, [u, v])
            }
            PbrEnvironmentSource::Cubemap(faces) => {
                let (face, uv) = crate::environment3d::cubemap_face_uv(
                    input_direction,
                    self.rotation_radians.to_degrees(),
                );
                self.sample_image(&faces[face], uv)
            }
        }
    }

    fn sample(&self, input_direction: Vec3) -> [f32; 3] {
        let local = self.sample_source(input_direction);
        let Some(fallback) = self.fallback.as_deref() else {
            return local;
        };
        let global = fallback.sample(input_direction);
        std::array::from_fn(|channel| {
            global[channel] + (local[channel] - global[channel]) * self.blend_weight
        })
    }

    fn sample_lobe(&self, direction: Vec3, spread: f32) -> [f32; 3] {
        let direction = direction.normalized();
        let helper = if direction.y.abs() < 0.95 {
            Vec3::new(0.0, 1.0, 0.0)
        } else {
            Vec3::new(1.0, 0.0, 0.0)
        };
        let tangent = helper.cross(direction).normalized();
        let bitangent = direction.cross(tangent);
        let spread = spread.clamp(0.0, 1.0);
        let samples = [
            (self.sample(direction), 4.0),
            (
                self.sample(direction.add(tangent.scale(spread)).normalized()),
                1.0,
            ),
            (
                self.sample(direction.sub(tangent.scale(spread)).normalized()),
                1.0,
            ),
            (
                self.sample(direction.add(bitangent.scale(spread)).normalized()),
                1.0,
            ),
            (
                self.sample(direction.sub(bitangent.scale(spread)).normalized()),
                1.0,
            ),
        ];
        std::array::from_fn(|channel| {
            samples
                .iter()
                .map(|(sample, weight)| sample[channel] * weight)
                .sum::<f32>()
                * 0.125
        })
    }
}

/// Software/Web built-in PBR evaluation. Texture samples are supplied by the
/// rasterizer so all attributes and maps remain perspective-correct per pixel.
#[allow(clippy::too_many_arguments)]
pub(crate) fn shade_pbr_pixel(
    material: &MeshMaterial,
    tint: [f32; 4],
    base_sample: [f32; 4],
    normal_sample: Option<[f32; 3]>,
    metallic_roughness_sample: Option<[f32; 2]>,
    emissive_sample: Option<[f32; 3]>,
    world_position: [f32; 3],
    world_normal: [f32; 3],
    world_tangent: [f32; 3],
    tangent_sign: f32,
    camera_position: Vec3,
    lights: &[Light3D],
    environment: Option<&PbrEnvironment>,
    ambient_visibility: f32,
) -> Option<[f32; 4]> {
    let base = [
        software_gamma_decode(base_sample[0]) * material.base_color[0],
        software_gamma_decode(base_sample[1]) * material.base_color[1],
        software_gamma_decode(base_sample[2]) * material.base_color[2],
        base_sample[3] * material.base_color[3] * tint[3],
    ];
    if material.alpha_mode == crate::mesh::AlphaMode::Mask && base[3] < material.alpha_cutoff {
        return None;
    }

    let mut normal = Vec3::from_array(world_normal).normalized();
    let mut tangent = Vec3::from_array(world_tangent)
        .sub(normal.scale(normal.dot(Vec3::from_array(world_tangent))))
        .normalized();
    if tangent.length_squared() <= f32::EPSILON {
        tangent = if normal.x.abs() < 0.9 {
            Vec3::new(1.0, 0.0, 0.0)
        } else {
            Vec3::new(0.0, 1.0, 0.0)
        }
        .sub(normal.scale(normal.dot(if normal.x.abs() < 0.9 {
            Vec3::new(1.0, 0.0, 0.0)
        } else {
            Vec3::new(0.0, 1.0, 0.0)
        })))
        .normalized();
    }
    if let Some(sample) = normal_sample {
        let mapped = Vec3::new(
            sample[0] * 2.0 - 1.0,
            sample[1] * 2.0 - 1.0,
            sample[2] * 2.0 - 1.0,
        )
        .normalized();
        let bitangent = normal.cross(tangent).scale(tangent_sign.signum());
        normal = tangent
            .scale(mapped.x)
            .add(bitangent.scale(mapped.y))
            .add(normal.scale(mapped.z))
            .normalized();
    }

    let [roughness_map, metallic_map] = metallic_roughness_sample.unwrap_or([1.0, 1.0]);
    let roughness = (material.roughness * roughness_map).clamp(0.045, 1.0);
    let metallic = (material.metallic * metallic_map).clamp(0.0, 1.0);
    let position = Vec3::from_array(world_position);
    let view_direction = camera_position.sub(position).normalized();
    let n_dot_v = normal.dot(view_direction).max(0.0001);
    let f0 = std::array::from_fn::<_, 3, _>(|channel| 0.04 + (base[channel] - 0.04) * metallic);
    let mut outgoing = if environment.is_some() {
        [0.0; 3]
    } else {
        std::array::from_fn::<_, 3, _>(|channel| 0.03 * base[channel] * (1.0 - metallic))
    };
    if lights.is_empty() && environment.is_none() {
        let headlight = normal.dot(Vec3::new(0.25, 0.45, 1.0).normalized()).max(0.0);
        for channel in 0..3 {
            outgoing[channel] += base[channel] * (0.35 + headlight * 0.53);
        }
    }
    for light in lights.iter().take(64) {
        let light_color = color_channels(light.color);
        let (light_direction, mut attenuation) = match light.kind {
            LightKind3D::Directional => (light.direction.scale(-1.0).normalized(), 1.0),
            LightKind3D::Point | LightKind3D::Spot => {
                let delta = light.position.sub(position);
                let distance_squared = delta.length_squared();
                let range = light.range.max(0.0001);
                let normalized_distance = (distance_squared.sqrt() / range).clamp(0.0, 1.0);
                (
                    delta.normalized(),
                    (1.0 - normalized_distance * normalized_distance).powi(2),
                )
            }
        };
        if light.kind == LightKind3D::Spot {
            let alignment = light
                .direction
                .normalized()
                .dot(light_direction.scale(-1.0));
            let outer = (light.spot_angle_radians.max(0.001) * 0.5).cos();
            let softness = light.spot_softness.clamp(0.0, 0.999);
            let inner = (outer + (1.0 - outer) * (1.0 - softness)).min(1.0);
            attenuation *= if inner <= outer + f32::EPSILON {
                (alignment >= outer) as u8 as f32
            } else {
                ((alignment - outer) / (inner - outer)).clamp(0.0, 1.0)
            };
        }
        let n_dot_l = normal.dot(light_direction).max(0.0);
        if n_dot_l <= 0.0 || attenuation <= 0.0 {
            continue;
        }
        let half_direction = view_direction.add(light_direction).normalized();
        let n_dot_h = normal.dot(half_direction).max(0.0);
        let h_dot_v = half_direction.dot(view_direction).max(0.0);
        let alpha = roughness * roughness;
        let alpha_squared = alpha * alpha;
        let denominator = n_dot_h * n_dot_h * (alpha_squared - 1.0) + 1.0;
        let distribution =
            alpha_squared / (std::f32::consts::PI * denominator * denominator).max(0.000001);
        let geometry_k = (roughness + 1.0).powi(2) * 0.125;
        let geometry_view = n_dot_v / (n_dot_v * (1.0 - geometry_k) + geometry_k);
        let geometry_light = n_dot_l / (n_dot_l * (1.0 - geometry_k) + geometry_k);
        let fresnel_power = (1.0 - h_dot_v).powi(5);
        for channel in 0..3 {
            let fresnel = f0[channel] + (1.0 - f0[channel]) * fresnel_power;
            let specular = distribution * geometry_view * geometry_light * fresnel
                / (4.0 * n_dot_v * n_dot_l).max(0.0001);
            let diffuse_weight = (1.0 - fresnel) * (1.0 - metallic);
            let radiance = light_color[channel] * light.intensity.max(0.0) * attenuation;
            outgoing[channel] += (diffuse_weight * base[channel] / std::f32::consts::PI + specular)
                * radiance
                * n_dot_l;
        }
    }
    if let Some(environment) = environment {
        let diffuse_environment = environment.sample_lobe(normal, 0.8);
        let reflection_direction = normal
            .scale(2.0 * view_direction.dot(normal))
            .sub(view_direction)
            .normalized();
        let specular_environment =
            environment.sample_lobe(reflection_direction, roughness * roughness);
        let fresnel_power = (1.0 - n_dot_v).powi(5);
        for channel in 0..3 {
            let fresnel = f0[channel] + (1.0 - f0[channel]) * fresnel_power;
            outgoing[channel] +=
                diffuse_environment[channel] * base[channel] * (1.0 - metallic) * 0.35;
            outgoing[channel] += specular_environment[channel] * fresnel;
        }
    }
    let ambient_visibility = ambient_visibility.clamp(0.0, 1.0);
    for channel in &mut outgoing {
        *channel *= ambient_visibility;
    }
    let emissive_sample = emissive_sample.unwrap_or([1.0; 3]);
    for channel in 0..3 {
        outgoing[channel] +=
            material.emissive[channel] * software_gamma_decode(emissive_sample[channel]);
    }
    Some([
        software_gamma_encode(outgoing[0] * tint[0]),
        software_gamma_encode(outgoing[1] * tint[1]),
        software_gamma_encode(outgoing[2] * tint[2]),
        base[3].clamp(0.0, 1.0),
    ])
}

const MAX_CLIPPED_VERTICES: usize = 12;

#[derive(Clone, Copy, Debug)]
struct ClipVertex {
    clip_position: [f32; 4],
    uv: [f32; 2],
    color: [f32; 4],
    world_position: [f32; 3],
    world_normal: [f32; 3],
    world_tangent: [f32; 3],
    tangent_sign: f32,
}

impl ClipVertex {
    const ZERO: Self = Self {
        clip_position: [0.0; 4],
        uv: [0.0; 2],
        color: [0.0; 4],
        world_position: [0.0; 3],
        world_normal: [0.0; 3],
        world_tangent: [0.0; 3],
        tangent_sign: 1.0,
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
        for (channel, value) in result.world_position.iter_mut().enumerate() {
            *value = self.world_position[channel]
                + (other.world_position[channel] - self.world_position[channel]) * amount;
            result.world_normal[channel] = self.world_normal[channel]
                + (other.world_normal[channel] - self.world_normal[channel]) * amount;
            result.world_tangent[channel] = self.world_tangent[channel]
                + (other.world_tangent[channel] - self.world_tangent[channel]) * amount;
        }
        result.tangent_sign = self.tangent_sign + (other.tangent_sign - self.tangent_sign) * amount;
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
    material: Option<usize>,
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
        material,
    });
}

fn push_projected_triangle(
    clipped: [ClipVertex; 3],
    double_sided: bool,
    material: Option<usize>,
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
            world_position: vertex.world_position,
            world_normal: vertex.world_normal,
            world_tangent: vertex.world_tangent,
            tangent_sign: vertex.tangent_sign,
        }
    });
    push_ready_triangle(projected, double_sided, material, output);
}

fn push_clipped_triangle(
    original: [ClipVertex; 3],
    double_sided: bool,
    material: Option<usize>,
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
        push_projected_triangle(original, double_sided, material, output);
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
            material,
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
        let corner_position = |right_amount: f32, up_amount: f32| {
            Vec3::new(
                particle.position.x + right.x * right_amount + up.x * up_amount,
                particle.position.y + right.y * right_amount + up.y * up_amount,
                particle.position.z + right.z * right_amount + up.z * up_amount,
            )
        };
        let corner = |position: Vec3| {
            command
                .view_projection
                .transform_vec4([position.x, position.y, position.z, 1.0])
        };
        let positions = [
            corner_position(-1.0, 1.0),
            corner_position(-1.0, -1.0),
            corner_position(1.0, -1.0),
            corner_position(1.0, 1.0),
        ];
        let color = color_channels(particle.color);
        let vertices = [
            ClipVertex {
                clip_position: corner(positions[0]),
                uv: [0.0, 0.0],
                color,
                world_position: [positions[0].x, positions[0].y, positions[0].z],
                ..ClipVertex::ZERO
            },
            ClipVertex {
                clip_position: corner(positions[1]),
                uv: [0.0, 1.0],
                color,
                world_position: [positions[1].x, positions[1].y, positions[1].z],
                ..ClipVertex::ZERO
            },
            ClipVertex {
                clip_position: corner(positions[2]),
                uv: [1.0, 1.0],
                color,
                world_position: [positions[2].x, positions[2].y, positions[2].z],
                ..ClipVertex::ZERO
            },
            ClipVertex {
                clip_position: corner(positions[3]),
                uv: [1.0, 0.0],
                color,
                world_position: [positions[3].x, positions[3].y, positions[3].z],
                ..ClipVertex::ZERO
            },
        ];
        push_clipped_triangle(
            [vertices[0], vertices[1], vertices[2]],
            true,
            None,
            &mut output,
        );
        push_clipped_triangle(
            [vertices[0], vertices[2], vertices[3]],
            true,
            None,
            &mut output,
        );
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

/// Conservative visibility query for GPU-native paths. It only rejects a mesh
/// when every bounds corner lies outside the same clip plane.
pub(crate) fn bounds_visible(bounds: MeshBounds, model_view_projection: Mat4) -> bool {
    !bounds_outside_clip(bounds, model_view_projection)
}

#[derive(Clone, Copy, Debug)]
struct PreparedVertex {
    clip_position: [f32; 4],
    ndc: [f32; 3],
    uv: [f32; 2],
    illumination: [f32; 3],
    world_position: [f32; 3],
    world_normal: [f32; 3],
    world_tangent: [f32; 3],
    tangent_sign: f32,
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
    let material_overrides = command.material_override_snapshots()?;
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
        let world_tangent = normal_matrix.transform(Vec3::new(
            vertex.tangent[0],
            vertex.tangent[1],
            vertex.tangent[2],
        ));
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
            world_position: [world_position.x, world_position.y, world_position.z],
            world_normal: [world_normal.x, world_normal.y, world_normal.z],
            world_tangent: [world_tangent.x, world_tangent.y, world_tangent.z],
            tangent_sign: vertex.tangent[3],
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
        let material_slot = submesh.material.or_else(|| {
            material_overrides
                .first()
                .and_then(Option::as_ref)
                .map(|_| 0)
        });
        let material = material_slot.and_then(|material_index| {
            material_overrides
                .get(material_index)
                .and_then(Option::as_ref)
                .map(|snapshot| snapshot.material.as_ref())
                .or_else(|| mesh.materials.get(material_index))
        });
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
                    world_position: vertex.world_position,
                    world_normal: vertex.world_normal,
                    world_tangent: vertex.world_tangent,
                    tangent_sign: vertex.tangent_sign,
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
                    world_position: original[corner].world_position,
                    world_normal: original[corner].world_normal,
                    world_tangent: original[corner].world_tangent,
                    tangent_sign: original[corner].tangent_sign,
                });
                push_ready_triangle(projected, double_sided, material_slot, &mut output);
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
                    material_slot,
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
    use crate::mesh::{MaterialHandle, MeshData, MeshMaterial, Submesh, Vertex};
    use image::{Rgba, RgbaImage};

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
    fn shared_fog_blends_in_linear_light_and_preserves_alpha() {
        let fog = crate::environment3d::Fog3D {
            enabled: true,
            color: Color::rgba(255, 0, 0, 255),
            start_distance: 0.0,
            end_distance: 10.0,
            ..crate::environment3d::Fog3D::default()
        };
        let color = apply_fog_srgb([0.0, 0.0, 1.0, 0.35], [0.0, 0.0, -10.0], Vec3::ZERO, fog);
        assert!(color[0] > 0.999);
        assert!(color[1] < 0.001);
        assert!(color[2] < 0.001);
        assert!((color[3] - 0.35).abs() < 0.0001);
    }

    #[test]
    fn ambient_occlusion_is_world_space_directional_bounded_and_self_excluding() {
        let settings = crate::environment3d::AmbientOcclusion3D {
            enabled: true,
            radius: 3.0,
            intensity: 0.75,
            bias: 0.01,
        };
        let receiver = AmbientOccluder3D {
            source_index: 7,
            min: Vec3::new(-1.0, -0.1, -1.0),
            max: Vec3::new(1.0, 0.1, 1.0),
            center: Vec3::ZERO,
        };
        let nearby = AmbientOccluder3D {
            source_index: 8,
            min: Vec3::new(-0.5, 0.1, -0.5),
            max: Vec3::new(0.5, 1.1, 0.5),
            center: Vec3::new(0.0, 0.6, 0.0),
        };
        let far = AmbientOccluder3D {
            source_index: 9,
            min: Vec3::new(20.0, 20.0, 20.0),
            max: Vec3::new(21.0, 21.0, 21.0),
            center: Vec3::new(20.5, 20.5, 20.5),
        };
        let selected = select_ambient_occluders_3d(7, receiver, &[receiver, far, nearby]);
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].source_index, 8);
        let upward = ambient_occlusion_visibility_3d(
            settings,
            Vec3::new(0.0, 0.1, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            &selected,
        );
        let downward = ambient_occlusion_visibility_3d(
            settings,
            Vec3::new(0.0, 0.1, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
            &selected,
        );
        assert!(
            upward < 0.5,
            "nearby upper occluder should darken: {upward}"
        );
        assert_eq!(downward, 1.0);
        assert!(upward >= 1.0 - settings.intensity);
    }

    #[test]
    fn reflection_probes_select_priority_and_blend_at_volume_edges() {
        let face = crate::assets::ImageHandle::from_rgba_image(RgbaImage::from_pixel(
            1,
            1,
            Rgba([255, 255, 255, 255]),
        ));
        let cubemap = crate::assets::CubemapHandle::new(std::array::from_fn(|_| face.clone()))
            .expect("valid probe cubemap");
        let probe = |source_id, priority, min, max, blend_distance| ReflectionProbe3D {
            source_id,
            cubemap: cubemap.clone(),
            bounds_min: min,
            bounds_max: max,
            priority,
            intensity: 1.0,
            rotation_degrees: 0.0,
            blend_distance,
        };
        let broad = probe(
            20,
            0,
            Vec3::new(-5.0, -5.0, -5.0),
            Vec3::new(5.0, 5.0, 5.0),
            2.0,
        );
        let priority = probe(
            10,
            4,
            Vec3::new(-2.0, -2.0, -2.0),
            Vec3::new(2.0, 2.0, 2.0),
            1.0,
        );
        let probes = [broad, priority];
        let center = select_reflection_probe_3d(Vec3::ZERO, &probes).expect("center probe");
        assert_eq!(center.index, 1, "higher priority must win overlap");
        assert_eq!(center.weight, 1.0);

        let edge = select_reflection_probe_3d(Vec3::new(1.5, 0.0, 0.0), &probes)
            .expect("blended edge probe");
        assert_eq!(edge.index, 1);
        assert!((edge.weight - 0.5).abs() < 0.0001);

        let outside = select_reflection_probe_3d(Vec3::new(8.0, 0.0, 0.0), &probes);
        assert!(outside.is_none());
    }

    #[test]
    fn lod_selection_sanitizes_thresholds_and_resolves_populated_fallbacks() {
        for (distance, expected) in [
            (0.0, Some(0)),
            (19.999, Some(0)),
            (20.0, Some(1)),
            (50.0, Some(2)),
            (99.999, Some(2)),
            (100.0, None),
        ] {
            assert_eq!(
                select_lod_level_3d(distance, 20.0, 50.0, 100.0, LodForce3D::Automatic),
                expected,
                "distance {distance}"
            );
        }
        assert_eq!(
            select_lod_level_3d(10_000.0, 20.0, 50.0, 100.0, LodForce3D::Level(1)),
            Some(1)
        );
        assert_eq!(
            select_lod_level_3d(0.0, 20.0, 50.0, 100.0, LodForce3D::Culled),
            None
        );
        assert_eq!(
            lod_distances_3d(50.0, 10.0, 5.0),
            LodDistances3D {
                lod1: 50.0,
                lod2: 50.0,
                cull: 50.0,
            }
        );
        assert_eq!(
            select_lod_level_3d(50.0, 50.0, 10.0, 5.0, LodForce3D::Automatic),
            None
        );

        let inherited = resolve_lod_mesh_path_3d("base.mesh", ["", "", ""], 2);
        assert_eq!(inherited.active_level, 0);
        assert_eq!(inherited.mesh_path, "base.mesh");
        let fallback = resolve_lod_mesh_path_3d("base.mesh", ["high.mesh", "mid.mesh", ""], 2);
        assert_eq!(fallback.requested_level, 2);
        assert_eq!(fallback.active_level, 1);
        assert_eq!(fallback.mesh_path, "mid.mesh");
        let exact = resolve_lod_mesh_path_3d("base.mesh", ["", "", "low.mesh"], 2);
        assert_eq!(exact.active_level, 2);
        assert_eq!(exact.mesh_path, "low.mesh");
    }

    #[test]
    fn render_layer_masks_are_bounded_and_intersect_by_bit() {
        assert_eq!(sanitize_render_mask_3d(-1), ALL_RENDER_LAYERS_3D);
        assert_eq!(sanitize_render_mask_3d(1i64 << 31), 0);
        assert!(render_layers_intersect_3d(0b0110, 0b0010));
        assert!(render_layers_intersect_3d(0b0110, 0b0100));
        assert!(!render_layers_intersect_3d(0b0110, 0b0001));
        assert!(!render_layers_intersect_3d(ALL_RENDER_LAYERS_3D, 0));
        assert_eq!(Camera3D::default().render_mask, ALL_RENDER_LAYERS_3D);
    }

    #[test]
    fn shared_shadow_projection_corners_match_runtime_clip_volume() {
        let camera = Camera3D::default();
        for kind in [LightKind3D::Spot, LightKind3D::Directional] {
            let light = Light3D {
                kind,
                position: Vec3::new(0.0, 1.0, 2.0),
                direction: Vec3::new(0.15, -0.25, -1.0),
                color: Color::WHITE,
                intensity: 1.0,
                range: 12.0,
                spot_angle_radians: 50.0f32.to_radians(),
                spot_softness: 0.15,
                casts_shadows: true,
                shadow_bias: 0.005,
            };
            let shadow =
                shadow_projection_3d(light, camera, 16.0 / 9.0).expect("shadow projection");
            for (index, corner) in shadow.corners.into_iter().enumerate() {
                let clip = shadow
                    .view_projection
                    .transform_vec4([corner.x, corner.y, corner.z, 1.0]);
                assert!(clip.into_iter().all(f32::is_finite));
                let x = clip[0] / clip[3];
                let y = clip[1] / clip[3];
                let z = clip[2] / clip[3];
                assert!((x.abs() - 1.0).abs() < 0.001, "{kind:?} x={x}");
                assert!((y.abs() - 1.0).abs() < 0.001, "{kind:?} y={y}");
                let expected_z = if index < 4 { 0.0 } else { 1.0 };
                assert!((z - expected_z).abs() < 0.001, "{kind:?} z={z}");
            }
        }
        assert!(
            shadow_projection_3d(
                Light3D {
                    kind: LightKind3D::Point,
                    position: Vec3::ZERO,
                    direction: Vec3::new(0.0, 0.0, -1.0),
                    color: Color::WHITE,
                    intensity: 1.0,
                    range: 10.0,
                    spot_angle_radians: 1.0,
                    spot_softness: 0.0,
                    casts_shadows: true,
                    shadow_bias: 0.005,
                },
                camera,
                1.0,
            )
            .is_none()
        );
    }

    #[test]
    fn software_pbr_honors_alpha_mask_and_all_texture_channels() {
        let mut material = MeshMaterial::named("mapped");
        material.base_color = [0.8, 0.6, 0.4, 1.0];
        material.emissive = [0.6, 0.3, 0.1];
        material.metallic = 1.0;
        material.roughness = 1.0;
        material.alpha_mode = crate::mesh::AlphaMode::Mask;
        material.alpha_cutoff = 0.5;
        let common = (
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            Vec3::new(0.0, 0.0, 5.0),
        );

        assert!(
            shade_pbr_pixel(
                &material,
                [1.0; 4],
                [1.0, 1.0, 1.0, 0.49],
                None,
                None,
                None,
                common.0,
                common.1,
                common.2,
                1.0,
                common.3,
                &[],
                None,
                1.0,
            )
            .is_none()
        );

        let baseline = shade_pbr_pixel(
            &material,
            [1.0; 4],
            [0.75, 0.5, 0.25, 1.0],
            Some([0.5, 0.5, 1.0]),
            Some([1.0, 1.0]),
            Some([1.0, 1.0, 1.0]),
            common.0,
            common.1,
            common.2,
            1.0,
            common.3,
            &[],
            None,
            1.0,
        )
        .expect("opaque mapped pixel");
        let changed = shade_pbr_pixel(
            &material,
            [1.0; 4],
            [0.25, 0.75, 0.5, 1.0],
            Some([1.0, 0.5, 0.5]),
            Some([0.15, 0.0]),
            Some([0.0, 1.0, 0.0]),
            common.0,
            common.1,
            common.2,
            1.0,
            common.3,
            &[],
            None,
            1.0,
        )
        .expect("alternate mapped pixel");
        assert_ne!(baseline, changed);
        assert!(changed[1] > changed[0], "emissive/base maps were ignored");
    }

    #[test]
    fn software_pbr_renderer_tint_constrains_bright_specular_channels() {
        let material = MeshMaterial::named("bright dielectric");
        let light = Light3D {
            kind: LightKind3D::Directional,
            position: Vec3::ZERO,
            direction: Vec3::new(0.0, 0.0, -1.0),
            color: Color::WHITE,
            intensity: 200.0,
            range: 10.0,
            spot_angle_radians: 1.0,
            spot_softness: 0.0,
            casts_shadows: false,
            shadow_bias: 0.005,
        };
        let shaded = shade_pbr_pixel(
            &material,
            [1.0, 0.0, 0.0, 1.0],
            [1.0; 4],
            None,
            None,
            None,
            [0.0; 3],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            1.0,
            Vec3::new(0.0, 0.0, 5.0),
            &[light],
            None,
            1.0,
        )
        .expect("bright tinted pixel");

        assert!(
            shaded[0] > 0.99,
            "red channel should remain saturated: {shaded:?}"
        );
        assert_eq!(shaded[1], 0.0, "green specular bypassed renderer tint");
        assert_eq!(shaded[2], 0.0, "blue specular bypassed renderer tint");
    }

    #[test]
    fn software_pbr_samples_rotated_environment_for_metallic_reflections() {
        let panorama = Arc::new(RgbaImage::from_fn(8, 4, |x, _| {
            if x < 4 {
                Rgba([16, 32, 255, 255])
            } else {
                Rgba([255, 24, 12, 255])
            }
        }));
        let unrotated = PbrEnvironment::new(panorama.clone(), 1.0, 0.0);
        let rotated = PbrEnvironment::new(panorama, 1.0, 180.0);
        let mut material = MeshMaterial::named("environment mirror");
        material.base_color = [1.0; 4];
        material.metallic = 1.0;
        material.roughness = 0.045;
        let shade = |environment| {
            shade_pbr_pixel(
                &material,
                [1.0; 4],
                [1.0; 4],
                None,
                None,
                None,
                [0.0; 3],
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 0.0],
                1.0,
                Vec3::new(0.0, 0.0, 5.0),
                &[],
                Some(environment),
                1.0,
            )
            .expect("environment-lit pixel")
        };

        let red_facing = shade(&unrotated);
        let blue_facing = shade(&rotated);
        assert!(red_facing[0] > red_facing[2] * 2.0, "{red_facing:?}");
        assert!(blue_facing[2] > blue_facing[0] * 2.0, "{blue_facing:?}");
    }

    #[test]
    fn software_pbr_samples_rotated_cubemap_for_metallic_reflections() {
        let faces = std::array::from_fn(|index| {
            Arc::new(RgbaImage::from_pixel(
                4,
                4,
                match index {
                    4 => Rgba([255, 16, 8, 255]),
                    5 => Rgba([8, 24, 255, 255]),
                    _ => Rgba([12, 12, 12, 255]),
                },
            ))
        });
        let unrotated = PbrEnvironment::new_cubemap(faces.clone(), 1.0, 0.0);
        let rotated = PbrEnvironment::new_cubemap(faces, 1.0, 180.0);
        let mut material = MeshMaterial::named("cubemap mirror");
        material.base_color = [1.0; 4];
        material.metallic = 1.0;
        material.roughness = 0.045;
        let shade = |environment| {
            shade_pbr_pixel(
                &material,
                [1.0; 4],
                [1.0; 4],
                None,
                None,
                None,
                [0.0; 3],
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 0.0],
                1.0,
                Vec3::new(0.0, 0.0, 5.0),
                &[],
                Some(environment),
                1.0,
            )
            .expect("cubemap-lit pixel")
        };

        let positive_z = shade(&unrotated);
        let negative_z = shade(&rotated);
        assert!(positive_z[0] > positive_z[2] * 2.0, "{positive_z:?}");
        assert!(negative_z[2] > negative_z[0] * 2.0, "{negative_z:?}");
    }

    #[test]
    fn mesh_projection_produces_finite_visible_triangle() {
        let camera = Camera3D::default();
        let command = Mesh3DCommand {
            mesh: triangle_mesh(),
            model: Mat4::identity(),
            view_projection: camera.view_projection(16.0 / 9.0),
            camera_position: camera.position,
            tint: Color::WHITE,
            texture: None,
            materials: Vec::new(),
            shader: None,
            double_sided: true,
            casts_shadows: true,
            receives_shadows: true,
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
            camera_position: camera.position,
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
            camera_position: Camera3D::default().position,
            tint: Color::WHITE,
            texture: None,
            materials: Vec::new(),
            shader: None,
            double_sided: true,
            casts_shadows: true,
            receives_shadows: true,
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
            camera_position: camera.position,
            tint: Color::WHITE,
            texture: None,
            materials: Vec::new(),
            shader: None,
            double_sided: true,
            casts_shadows: true,
            receives_shadows: true,
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
            shadow_bias: 0.005,
        };
        let command = Mesh3DCommand {
            mesh,
            model: Mat4::scale(Vec3::new(2.0, 1.0, 1.0)),
            view_projection: Camera3D::default().view_projection(1.0),
            camera_position: Camera3D::default().position,
            tint: Color::WHITE,
            texture: None,
            materials: Vec::new(),
            shader: None,
            double_sided: true,
            casts_shadows: true,
            receives_shadows: true,
        };
        let triangles = project_mesh(&command, &[light]).expect("project lit mesh");
        let red = triangles[0].vertices[0].color[0];
        // inverse-transpose(scale(2, 1, 1)) maps (1, 1, 0) to
        // normalize(0.5, 1, 0), whose Y contribution is ~0.894.
        assert!((red - 0.5672).abs() < 0.002, "unexpected light value {red}");
    }

    #[test]
    fn reusable_material_override_updates_unassigned_submesh_live() {
        let mut red = MeshMaterial::named("Live override");
        red.metallic = 0.0;
        red.base_color = [1.0, 0.1, 0.1, 1.0];
        let material = MaterialHandle::new(red).expect("material handle");
        let command = Mesh3DCommand {
            mesh: triangle_mesh(),
            model: Mat4::identity(),
            view_projection: Camera3D::default().view_projection(1.0),
            camera_position: Camera3D::default().position,
            tint: Color::WHITE,
            texture: None,
            materials: vec![Some(material.clone())],
            shader: None,
            double_sided: true,
            casts_shadows: true,
            receives_shadows: true,
        };
        let first = project_mesh(&command, &[]).expect("red material projection");
        assert_eq!(first[0].material, Some(0));
        assert!(first[0].vertices[0].color[0] > first[0].vertices[0].color[2] * 5.0);

        material
            .mutate(|material| {
                material.base_color = [0.1, 0.1, 1.0, 1.0];
                Ok(())
            })
            .expect("live blue mutation");
        let second = project_mesh(&command, &[]).expect("blue material projection");
        assert!(second[0].vertices[0].color[2] > second[0].vertices[0].color[0] * 5.0);
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
            camera_position: Camera3D::default().position,
            tint: Color::WHITE,
            texture: None,
            materials: Vec::new(),
            shader: None,
            double_sided: true,
            casts_shadows: true,
            receives_shadows: true,
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
                shadow_bias: 0.005,
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
                    camera_position: Camera3D::default().position,
                    tint: Color::WHITE,
                    texture: None,
                    materials: Vec::new(),
                    shader: None,
                    double_sided: true,
                    casts_shadows: true,
                    receives_shadows: true,
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
