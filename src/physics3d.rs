//! Dependency-free 3D collider queries and broadphase acceleration.
//!
//! It provides robust spatial queries, collision filtering, trigger metadata,
//! mesh BVHs, a sweep-and-prune broadphase, and allocation-light primitive
//! contact generation. Euler angles remain the authoring representation while
//! all query work uses affine matrices internally.
//!
//! Primitive contacts deliberately report whether their geometry is exact.
//! Mesh-vs-shape contacts and primitives distorted into non-uniform ellipsoids
//! are bounds-only candidates; callers may use those for trigger notification,
//! but must not mistake them for an exact contact manifold.

use std::fmt;
use std::sync::Arc;

use crate::mesh::{MeshError, MeshHandle};
use crate::render3d::{Mat4, Vec3};

const TRANSFORM_EPSILON: f32 = 1.0e-8;
const DIRECTION_EPSILON: f32 = 1.0e-8;
const TRIANGLE_LEAF_SIZE: usize = 4;
const MESH_BVH_STACK_SIZE: usize = usize::BITS as usize;

pub(crate) type Physics3dResult<T> = Result<T, Physics3dError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Physics3dError {
    InvalidRay(String),
    InvalidTransform(String),
    InvalidShape(String),
    InvalidMesh(String),
    Mesh(MeshError),
}

impl fmt::Display for Physics3dError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRay(message) => write!(formatter, "invalid 3D ray: {message}"),
            Self::InvalidTransform(message) => {
                write!(formatter, "invalid 3D transform: {message}")
            }
            Self::InvalidShape(message) => write!(formatter, "invalid 3D collider: {message}"),
            Self::InvalidMesh(message) => write!(formatter, "invalid 3D mesh collider: {message}"),
            Self::Mesh(error) => write!(formatter, "failed to read 3D mesh collider: {error}"),
        }
    }
}

impl std::error::Error for Physics3dError {}

impl From<MeshError> for Physics3dError {
    fn from(error: MeshError) -> Self {
        Self::Mesh(error)
    }
}

/// Euler-authored transform. Angles are XYZ degrees and are converted to a
/// matrix once before a query batch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Transform3D {
    pub position: Vec3,
    pub euler: Vec3,
    pub scale: Vec3,
}

impl Default for Transform3D {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            euler: Vec3::ZERO,
            scale: Vec3::new(1.0, 1.0, 1.0),
        }
    }
}

impl Transform3D {
    pub(crate) fn matrix(self) -> Physics3dResult<Mat4> {
        if !vec3_is_finite(self.position)
            || !vec3_is_finite(self.euler)
            || !vec3_is_finite(self.scale)
        {
            return Err(Physics3dError::InvalidTransform(
                "position, Euler rotation, and scale must be finite".into(),
            ));
        }
        if self.scale.x.abs() <= TRANSFORM_EPSILON
            || self.scale.y.abs() <= TRANSFORM_EPSILON
            || self.scale.z.abs() <= TRANSFORM_EPSILON
        {
            return Err(Physics3dError::InvalidTransform(
                "scale components must be non-zero".into(),
            ));
        }
        let model = Mat4::trs(self.position, self.euler, self.scale);
        if model
            .values
            .iter()
            .flatten()
            .any(|value| !value.is_finite())
        {
            Err(Physics3dError::InvalidTransform(
                "matrix contains a non-finite value".into(),
            ))
        } else {
            Ok(model)
        }
    }

    fn prepare(self) -> Physics3dResult<PreparedTransform> {
        PreparedTransform::from_model(self.matrix()?)
    }
}

#[derive(Clone, Copy, Debug)]
struct PreparedTransform {
    inverse: Mat4,
    determinant: f32,
}

impl PreparedTransform {
    fn from_model(model: Mat4) -> Physics3dResult<Self> {
        let m = model.values;
        if m.iter().flatten().any(|value| !value.is_finite()) {
            return Err(Physics3dError::InvalidTransform(
                "matrix contains a non-finite value".into(),
            ));
        }
        let determinant = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        if !determinant.is_finite() || determinant.abs() <= TRANSFORM_EPSILON {
            return Err(Physics3dError::InvalidTransform(
                "matrix is singular or non-finite".into(),
            ));
        }

        let inverse_determinant = determinant.recip();
        let mut inverse = Mat4::identity();
        inverse.values[0][0] = (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inverse_determinant;
        inverse.values[0][1] = (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inverse_determinant;
        inverse.values[0][2] = (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inverse_determinant;
        inverse.values[1][0] = (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inverse_determinant;
        inverse.values[1][1] = (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inverse_determinant;
        inverse.values[1][2] = (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inverse_determinant;
        inverse.values[2][0] = (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inverse_determinant;
        inverse.values[2][1] = (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inverse_determinant;
        inverse.values[2][2] = (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inverse_determinant;

        let translation = Vec3::new(m[0][3], m[1][3], m[2][3]);
        let inverse_translation = inverse.transform_direction(scale(translation, -1.0));
        inverse.values[0][3] = inverse_translation.x;
        inverse.values[1][3] = inverse_translation.y;
        inverse.values[2][3] = inverse_translation.z;
        if inverse
            .values
            .iter()
            .flatten()
            .any(|value| !value.is_finite())
        {
            return Err(Physics3dError::InvalidTransform(
                "inverse matrix overflowed".into(),
            ));
        }

        Ok(Self {
            inverse,
            determinant,
        })
    }

    fn point_to_local(self, point: Vec3) -> Vec3 {
        self.inverse.transform_point(point)
    }

    fn direction_to_local(self, direction: Vec3) -> Vec3 {
        self.inverse.transform_direction(direction)
    }

    fn normal_to_world(self, normal: Vec3) -> Vec3 {
        // Normals transform with the inverse transpose, not the model matrix.
        let inverse = self.inverse.values;
        normalize(Vec3::new(
            inverse[0][0] * normal.x + inverse[1][0] * normal.y + inverse[2][0] * normal.z,
            inverse[0][1] * normal.x + inverse[1][1] * normal.y + inverse[2][1] * normal.z,
            inverse[0][2] * normal.x + inverse[1][2] * normal.y + inverse[2][2] * normal.z,
        ))
    }

    fn mesh_normal_to_world(self, normal: Vec3) -> Vec3 {
        // A reflection reverses triangle winding. Preserve the geometric
        // normal implied by the transformed vertex order.
        scale(
            self.normal_to_world(normal),
            if self.determinant < 0.0 { -1.0 } else { 1.0 },
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Ray3D {
    pub origin: Vec3,
    pub direction: Vec3,
}

impl Ray3D {
    pub(crate) fn new(origin: Vec3, direction: Vec3) -> Physics3dResult<Self> {
        if !vec3_is_finite(origin) || !vec3_is_finite(direction) {
            return Err(Physics3dError::InvalidRay(
                "origin and direction must be finite".into(),
            ));
        }
        let length_squared = dot(direction, direction);
        if length_squared <= DIRECTION_EPSILON * DIRECTION_EPSILON {
            return Err(Physics3dError::InvalidRay(
                "direction must have non-zero length".into(),
            ));
        }
        Ok(Self {
            origin,
            direction: scale(direction, length_squared.sqrt().recip()),
        })
    }

    fn normalized(self) -> Physics3dResult<Self> {
        Self::new(self.origin, self.direction)
    }

    pub(crate) fn point_at(self, distance: f32) -> Vec3 {
        add(self.origin, scale(self.direction, distance))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Aabb3D {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb3D {
    pub(crate) fn new(min: Vec3, max: Vec3) -> Physics3dResult<Self> {
        if !vec3_is_finite(min) || !vec3_is_finite(max) {
            return Err(Physics3dError::InvalidShape(
                "AABB bounds must be finite".into(),
            ));
        }
        if min.x > max.x || min.y > max.y || min.z > max.z {
            return Err(Physics3dError::InvalidShape(
                "AABB minimum must not exceed its maximum".into(),
            ));
        }
        Ok(Self { min, max })
    }

    fn from_point(point: Vec3) -> Self {
        Self {
            min: point,
            max: point,
        }
    }

    fn include_point(&mut self, point: Vec3) {
        self.min.x = self.min.x.min(point.x);
        self.min.y = self.min.y.min(point.y);
        self.min.z = self.min.z.min(point.z);
        self.max.x = self.max.x.max(point.x);
        self.max.y = self.max.y.max(point.y);
        self.max.z = self.max.z.max(point.z);
    }

    fn union(self, other: Self) -> Self {
        Self {
            min: Vec3::new(
                self.min.x.min(other.min.x),
                self.min.y.min(other.min.y),
                self.min.z.min(other.min.z),
            ),
            max: Vec3::new(
                self.max.x.max(other.max.x),
                self.max.y.max(other.max.y),
                self.max.z.max(other.max.z),
            ),
        }
    }

    pub(crate) fn overlaps(self, other: Self) -> bool {
        self.min.x <= other.max.x
            && self.max.x >= other.min.x
            && self.min.y <= other.max.y
            && self.max.y >= other.min.y
            && self.min.z <= other.max.z
            && self.max.z >= other.min.z
    }

    fn center(self) -> Vec3 {
        Vec3::new(
            self.min.x * 0.5 + self.max.x * 0.5,
            self.min.y * 0.5 + self.max.y * 0.5,
            self.min.z * 0.5 + self.max.z * 0.5,
        )
    }

    fn half_extents(self) -> Vec3 {
        Vec3::new(
            self.max.x * 0.5 - self.min.x * 0.5,
            self.max.y * 0.5 - self.min.y * 0.5,
            self.max.z * 0.5 - self.min.z * 0.5,
        )
    }

    fn transformed(self, model: Mat4) -> Self {
        let center = model.transform_point(self.center());
        let half = self.half_extents();
        let matrix = model.values;
        let world_half = Vec3::new(
            matrix[0][0].abs() * half.x + matrix[0][1].abs() * half.y + matrix[0][2].abs() * half.z,
            matrix[1][0].abs() * half.x + matrix[1][1].abs() * half.y + matrix[1][2].abs() * half.z,
            matrix[2][0].abs() * half.x + matrix[2][1].abs() * half.y + matrix[2][2].abs() * half.z,
        );
        Self {
            min: sub(center, world_half),
            max: add(center, world_half),
        }
    }

    fn ray_interval(self, ray: LocalRay, max_distance: f32) -> Option<(f32, f32)> {
        let origins = [ray.origin.x, ray.origin.y, ray.origin.z];
        let directions = [ray.direction.x, ray.direction.y, ray.direction.z];
        let minima = [self.min.x, self.min.y, self.min.z];
        let maxima = [self.max.x, self.max.y, self.max.z];
        let mut enter = 0.0f32;
        let mut exit = max_distance;
        for axis in 0..3 {
            if directions[axis].abs() <= DIRECTION_EPSILON {
                if origins[axis] < minima[axis] || origins[axis] > maxima[axis] {
                    return None;
                }
                continue;
            }
            let inverse = directions[axis].recip();
            let mut near = (minima[axis] - origins[axis]) * inverse;
            let mut far = (maxima[axis] - origins[axis]) * inverse;
            if near > far {
                std::mem::swap(&mut near, &mut far);
            }
            enter = enter.max(near);
            exit = exit.min(far);
            if enter > exit {
                return None;
            }
        }
        Some((enter, exit))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CollisionFilter3D {
    /// Bit field describing which layers this collider belongs to.
    pub layer: u32,
    /// Bit field describing which layers this collider accepts.
    pub mask: u32,
}

impl Default for CollisionFilter3D {
    fn default() -> Self {
        Self {
            layer: 1,
            mask: u32::MAX,
        }
    }
}

impl CollisionFilter3D {
    pub(crate) const fn new(layer: u32, mask: u32) -> Self {
        Self { layer, mask }
    }

    pub(crate) fn allows(self, other: Self) -> bool {
        self.mask & other.layer != 0 && other.mask & self.layer != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QueryFilter3D {
    pub collision: CollisionFilter3D,
    pub include_triggers: bool,
}

impl Default for QueryFilter3D {
    fn default() -> Self {
        Self {
            collision: CollisionFilter3D::default(),
            include_triggers: true,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct MeshTriangle {
    vertices: [Vec3; 3],
    normal: Vec3,
    bounds: Aabb3D,
    centroid: Vec3,
    source_index: u32,
}

#[derive(Clone, Copy, Debug)]
struct MeshBvhNode {
    bounds: Aabb3D,
    left: usize,
    right: usize,
    first: usize,
    count: usize,
}

impl MeshBvhNode {
    fn leaf(bounds: Aabb3D, first: usize, count: usize) -> Self {
        Self {
            bounds,
            left: usize::MAX,
            right: usize::MAX,
            first,
            count,
        }
    }

    fn branch(bounds: Aabb3D, left: usize, right: usize) -> Self {
        Self {
            bounds,
            left,
            right,
            first: 0,
            count: 0,
        }
    }

    fn is_leaf(self) -> bool {
        self.count != 0
    }
}

#[derive(Clone, Debug)]
pub(crate) struct MeshCollider3D {
    source: MeshHandle,
    revision: u64,
    geometry: Arc<BuiltMeshGeometry>,
}

impl MeshCollider3D {
    pub(crate) fn from_mesh(source: MeshHandle) -> Physics3dResult<Self> {
        let snapshot = source.snapshot()?;
        let geometry = build_mesh_geometry(&snapshot.mesh.vertices, &snapshot.mesh.indices)?;
        Ok(Self {
            source,
            revision: snapshot.revision,
            geometry: Arc::new(geometry),
        })
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn triangle_count(&self) -> usize {
        self.geometry.triangles.len()
    }

    pub(crate) fn bvh_node_count(&self) -> usize {
        self.geometry.nodes.len()
    }

    /// Atomically rebuild the collider after a live mesh edit. A failed build
    /// leaves the last valid collider snapshot untouched.
    pub(crate) fn refresh_if_changed(&mut self) -> Physics3dResult<bool> {
        if self.source.revision()? == self.revision {
            return Ok(false);
        }
        let snapshot = self.source.snapshot()?;
        if snapshot.revision == self.revision {
            return Ok(false);
        }
        let geometry = build_mesh_geometry(&snapshot.mesh.vertices, &snapshot.mesh.indices)?;
        self.revision = snapshot.revision;
        self.geometry = Arc::new(geometry);
        Ok(true)
    }

    fn raycast_local(&self, ray: LocalRay, max_distance: f32) -> Option<MeshRayHit> {
        let root = *self.geometry.nodes.first()?;
        let (root_entry, _) = root.bounds.ray_interval(ray, max_distance)?;
        // Median splitting bounds tree depth by the address-space bit count,
        // so traversal needs no per-ray heap allocation.
        let mut stack = [(0usize, 0.0f32); MESH_BVH_STACK_SIZE];
        stack[0] = (0, root_entry);
        let mut stack_len = 1usize;
        let mut best_distance = max_distance;
        let mut best = None;

        while stack_len > 0 {
            stack_len -= 1;
            let (node_index, entry) = stack[stack_len];
            if entry > best_distance {
                continue;
            }
            let node = self.geometry.nodes[node_index];
            if node.is_leaf() {
                for &triangle_index in
                    &self.geometry.triangle_order[node.first..node.first + node.count]
                {
                    let triangle = self.geometry.triangles[triangle_index];
                    if triangle.bounds.ray_interval(ray, best_distance).is_none() {
                        continue;
                    }
                    if let Some((distance, barycentric)) =
                        ray_triangle(ray, triangle.vertices, best_distance)
                    {
                        best_distance = distance;
                        best = Some(MeshRayHit {
                            distance,
                            normal: triangle.normal,
                            triangle_index: triangle.source_index,
                            barycentric,
                        });
                    }
                }
            } else {
                let left = self.geometry.nodes[node.left]
                    .bounds
                    .ray_interval(ray, best_distance)
                    .map(|interval| (node.left, interval.0));
                let right = self.geometry.nodes[node.right]
                    .bounds
                    .ray_interval(ray, best_distance)
                    .map(|interval| (node.right, interval.0));
                match (left, right) {
                    (Some(left), Some(right)) if left.1 <= right.1 => {
                        push_mesh_bvh_stack(&mut stack, &mut stack_len, right)?;
                        push_mesh_bvh_stack(&mut stack, &mut stack_len, left)?;
                    }
                    (Some(left), Some(right)) => {
                        push_mesh_bvh_stack(&mut stack, &mut stack_len, left)?;
                        push_mesh_bvh_stack(&mut stack, &mut stack_len, right)?;
                    }
                    (Some(left), None) => {
                        push_mesh_bvh_stack(&mut stack, &mut stack_len, left)?;
                    }
                    (None, Some(right)) => {
                        push_mesh_bvh_stack(&mut stack, &mut stack_len, right)?;
                    }
                    (None, None) => {}
                }
            }
        }
        best
    }
}

fn push_mesh_bvh_stack(
    stack: &mut [(usize, f32); MESH_BVH_STACK_SIZE],
    length: &mut usize,
    value: (usize, f32),
) -> Option<()> {
    let destination = stack.get_mut(*length)?;
    *destination = value;
    *length += 1;
    Some(())
}

#[derive(Debug)]
struct BuiltMeshGeometry {
    triangles: Vec<MeshTriangle>,
    triangle_order: Vec<usize>,
    nodes: Vec<MeshBvhNode>,
    local_bounds: Aabb3D,
}

fn build_mesh_geometry(
    vertices: &[crate::mesh::Vertex],
    indices: &[u32],
) -> Physics3dResult<BuiltMeshGeometry> {
    if indices.is_empty() || indices.len() % 3 != 0 {
        return Err(Physics3dError::InvalidMesh(
            "mesh collider requires a non-empty triangle index buffer".into(),
        ));
    }

    let mut triangles = Vec::with_capacity(indices.len() / 3);
    let mut degenerate_count = 0usize;
    for (source_index, triangle) in indices.chunks_exact(3).enumerate() {
        let mut points = [Vec3::ZERO; 3];
        for corner in 0..3 {
            let vertex_index = usize::try_from(triangle[corner]).map_err(|_| {
                Physics3dError::InvalidMesh(format!(
                    "triangle {source_index} has an index that cannot fit in memory"
                ))
            })?;
            let vertex = vertices.get(vertex_index).ok_or_else(|| {
                Physics3dError::InvalidMesh(format!(
                    "triangle {source_index} references missing vertex {vertex_index}"
                ))
            })?;
            points[corner] = Vec3::new(vertex.position[0], vertex.position[1], vertex.position[2]);
        }
        if points.iter().any(|point| !vec3_is_finite(*point)) {
            return Err(Physics3dError::InvalidMesh(format!(
                "triangle {source_index} contains a non-finite position"
            )));
        }

        let edge_ab = sub(points[1], points[0]);
        let edge_ac = sub(points[2], points[0]);
        let raw_normal = cross(edge_ab, edge_ac);
        let normal_length_squared = dot(raw_normal, raw_normal);
        let edge_scale = dot(edge_ab, edge_ab) * dot(edge_ac, edge_ac);
        if !normal_length_squared.is_finite() || !edge_scale.is_finite() {
            return Err(Physics3dError::InvalidMesh(format!(
                "triangle {source_index} geometry overflows f32"
            )));
        }
        let angular_epsilon = f32::EPSILON * 8.0;
        if edge_scale <= f32::MIN_POSITIVE
            || normal_length_squared <= edge_scale * angular_epsilon * angular_epsilon
        {
            degenerate_count += 1;
            continue;
        }
        let mut bounds = Aabb3D::from_point(points[0]);
        bounds.include_point(points[1]);
        bounds.include_point(points[2]);
        triangles.push(MeshTriangle {
            vertices: points,
            normal: scale(raw_normal, normal_length_squared.sqrt().recip()),
            bounds,
            centroid: add(
                points[0],
                add(
                    scale(sub(points[1], points[0]), 1.0 / 3.0),
                    scale(sub(points[2], points[0]), 1.0 / 3.0),
                ),
            ),
            source_index: u32::try_from(source_index).map_err(|_| {
                Physics3dError::InvalidMesh("triangle count exceeds the u32 limit".into())
            })?,
        });
    }
    if triangles.is_empty() {
        return Err(Physics3dError::InvalidMesh(if degenerate_count == 0 {
            "mesh collider contains no triangles".into()
        } else {
            format!("all {degenerate_count} mesh triangles are degenerate")
        }));
    }

    let local_bounds = triangles
        .iter()
        .skip(1)
        .fold(triangles[0].bounds, |bounds, triangle| {
            bounds.union(triangle.bounds)
        });
    let mut triangle_order = (0..triangles.len()).collect::<Vec<_>>();
    let mut nodes = Vec::with_capacity(triangles.len().saturating_mul(2));
    build_mesh_bvh_node(
        &triangles,
        &mut triangle_order,
        &mut nodes,
        0,
        triangles.len(),
    );

    Ok(BuiltMeshGeometry {
        triangles,
        triangle_order,
        nodes,
        local_bounds,
    })
}

fn build_mesh_bvh_node(
    triangles: &[MeshTriangle],
    order: &mut [usize],
    nodes: &mut Vec<MeshBvhNode>,
    first: usize,
    end: usize,
) -> usize {
    let bounds = order[first + 1..end]
        .iter()
        .fold(triangles[order[first]].bounds, |bounds, &index| {
            bounds.union(triangles[index].bounds)
        });
    let node_index = nodes.len();
    nodes.push(MeshBvhNode::leaf(bounds, first, end - first));
    if end - first <= TRIANGLE_LEAF_SIZE {
        return node_index;
    }

    let centroid_bounds = order[first + 1..end].iter().fold(
        Aabb3D::from_point(triangles[order[first]].centroid),
        |mut bounds, &index| {
            bounds.include_point(triangles[index].centroid);
            bounds
        },
    );
    let extent = sub(centroid_bounds.max, centroid_bounds.min);
    let axis = if extent.x >= extent.y && extent.x >= extent.z {
        0
    } else if extent.y >= extent.z {
        1
    } else {
        2
    };
    let middle = first + (end - first) / 2;
    order[first..end].select_nth_unstable_by(middle - first, |left, right| {
        component(triangles[*left].centroid, axis)
            .total_cmp(&component(triangles[*right].centroid, axis))
            .then_with(|| {
                triangles[*left]
                    .source_index
                    .cmp(&triangles[*right].source_index)
            })
    });
    let left = build_mesh_bvh_node(triangles, order, nodes, first, middle);
    let right = build_mesh_bvh_node(triangles, order, nodes, middle, end);
    nodes[node_index] = MeshBvhNode::branch(bounds, left, right);
    node_index
}

#[derive(Clone, Debug)]
pub(crate) enum ColliderShape3D {
    Box {
        half_extents: Vec3,
    },
    Sphere {
        radius: f32,
    },
    /// Y-aligned capsule in local space. `half_height` is half of the straight
    /// segment between the two hemisphere centers.
    Capsule {
        radius: f32,
        half_height: f32,
    },
    TriangleMesh(MeshCollider3D),
}

impl ColliderShape3D {
    pub(crate) fn triangle_mesh(mesh: MeshHandle) -> Physics3dResult<Self> {
        MeshCollider3D::from_mesh(mesh).map(Self::TriangleMesh)
    }

    fn validate(&self) -> Physics3dResult<()> {
        match self {
            Self::Box { half_extents } => {
                if !vec3_is_finite(*half_extents)
                    || half_extents.x <= 0.0
                    || half_extents.y <= 0.0
                    || half_extents.z <= 0.0
                {
                    return Err(Physics3dError::InvalidShape(
                        "box half extents must be finite and greater than zero".into(),
                    ));
                }
            }
            Self::Sphere { radius } => {
                if !radius.is_finite() || *radius <= 0.0 {
                    return Err(Physics3dError::InvalidShape(
                        "sphere radius must be finite and greater than zero".into(),
                    ));
                }
            }
            Self::Capsule {
                radius,
                half_height,
            } => {
                if !radius.is_finite()
                    || *radius <= 0.0
                    || !half_height.is_finite()
                    || *half_height < 0.0
                {
                    return Err(Physics3dError::InvalidShape(
                        "capsule radius must be positive and half height must be non-negative"
                            .into(),
                    ));
                }
            }
            Self::TriangleMesh(mesh) => {
                if mesh.geometry.triangles.is_empty() || mesh.geometry.nodes.is_empty() {
                    return Err(Physics3dError::InvalidMesh(
                        "mesh collider has no acceleration data".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Collider3D {
    pub id: u64,
    pub transform: Transform3D,
    pub shape: ColliderShape3D,
    pub filter: CollisionFilter3D,
    pub is_trigger: bool,
    pub enabled: bool,
    /// Combined by the lightweight response layer for exact contacts.
    pub restitution: f32,
    /// Combined by the lightweight response layer for exact contacts.
    pub friction: f32,
    /// False for query-only/non-physics colliders.
    pub response_enabled: bool,
    /// Used to split positional correction for two dynamic bodies.
    pub body_is_static: bool,
}

impl Collider3D {
    pub(crate) fn new(id: u64, shape: ColliderShape3D) -> Self {
        Self {
            id,
            transform: Transform3D::default(),
            shape,
            filter: CollisionFilter3D::default(),
            is_trigger: false,
            enabled: true,
            restitution: 0.0,
            friction: 0.5,
            response_enabled: true,
            body_is_static: true,
        }
    }

    pub(crate) fn refresh_mesh_if_changed(&mut self) -> Physics3dResult<bool> {
        match &mut self.shape {
            ColliderShape3D::TriangleMesh(mesh) => mesh.refresh_if_changed(),
            _ => Ok(false),
        }
    }

    pub(crate) fn world_aabb(&self) -> Physics3dResult<Aabb3D> {
        self.shape.validate()?;
        let model = self.transform.matrix()?;
        let bounds = match &self.shape {
            ColliderShape3D::Box { half_extents } => Aabb3D {
                min: scale(*half_extents, -1.0),
                max: *half_extents,
            }
            .transformed(model),
            ColliderShape3D::Sphere { radius } => {
                let center = model.transform_point(Vec3::ZERO);
                let matrix = model.values;
                let extent = Vec3::new(
                    radius
                        * (matrix[0][0] * matrix[0][0]
                            + matrix[0][1] * matrix[0][1]
                            + matrix[0][2] * matrix[0][2])
                            .sqrt(),
                    radius
                        * (matrix[1][0] * matrix[1][0]
                            + matrix[1][1] * matrix[1][1]
                            + matrix[1][2] * matrix[1][2])
                            .sqrt(),
                    radius
                        * (matrix[2][0] * matrix[2][0]
                            + matrix[2][1] * matrix[2][1]
                            + matrix[2][2] * matrix[2][2])
                            .sqrt(),
                );
                Aabb3D {
                    min: sub(center, extent),
                    max: add(center, extent),
                }
            }
            ColliderShape3D::Capsule {
                radius,
                half_height,
            } => {
                let center = model.transform_point(Vec3::ZERO);
                let matrix = model.values;
                let row_extent = |row: usize| {
                    half_height * matrix[row][1].abs()
                        + radius
                            * (matrix[row][0] * matrix[row][0]
                                + matrix[row][1] * matrix[row][1]
                                + matrix[row][2] * matrix[row][2])
                                .sqrt()
                };
                let extent = Vec3::new(row_extent(0), row_extent(1), row_extent(2));
                Aabb3D {
                    min: sub(center, extent),
                    max: add(center, extent),
                }
            }
            ColliderShape3D::TriangleMesh(mesh) => mesh.geometry.local_bounds.transformed(model),
        };
        if !vec3_is_finite(bounds.min) || !vec3_is_finite(bounds.max) {
            Err(Physics3dError::InvalidTransform(
                "world-space collider bounds overflowed".into(),
            ))
        } else {
            Ok(bounds)
        }
    }

    pub(crate) fn raycast(
        &self,
        ray: Ray3D,
        max_distance: f32,
        query: QueryFilter3D,
    ) -> Physics3dResult<Option<RaycastHit3D>> {
        let ray = ray.normalized()?;
        validate_max_distance(max_distance)?;
        if !self.enabled
            || (!query.include_triggers && self.is_trigger)
            || !query.collision.allows(self.filter)
        {
            return Ok(None);
        }
        self.raycast_prepared(ray, max_distance)
    }

    fn raycast_prepared(
        &self,
        ray: Ray3D,
        max_distance: f32,
    ) -> Physics3dResult<Option<RaycastHit3D>> {
        self.shape.validate()?;
        let transform = self.transform.prepare()?;
        let local_ray = LocalRay {
            origin: transform.point_to_local(ray.origin),
            direction: transform.direction_to_local(ray.direction),
        };
        if !vec3_is_finite(local_ray.origin)
            || !vec3_is_finite(local_ray.direction)
            || dot(local_ray.direction, local_ray.direction) <= f32::MIN_POSITIVE
        {
            return Err(Physics3dError::InvalidRay(
                "ray overflowed while transforming into collider space".into(),
            ));
        }

        let local_hit = match &self.shape {
            ColliderShape3D::Box { half_extents } => {
                ray_box(local_ray, *half_extents, max_distance)
            }
            ColliderShape3D::Sphere { radius } => {
                ray_sphere(local_ray, Vec3::ZERO, *radius, max_distance).map(|hit| LocalShapeHit {
                    distance: hit.distance,
                    normal: hit.normal,
                    triangle_index: None,
                    barycentric: None,
                    mesh_normal: false,
                })
            }
            ColliderShape3D::Capsule {
                radius,
                half_height,
            } => ray_capsule(local_ray, *radius, *half_height, max_distance),
            ColliderShape3D::TriangleMesh(mesh) => {
                mesh.raycast_local(local_ray, max_distance)
                    .map(|hit| LocalShapeHit {
                        distance: hit.distance,
                        normal: hit.normal,
                        triangle_index: Some(hit.triangle_index),
                        barycentric: Some(hit.barycentric),
                        mesh_normal: true,
                    })
            }
        };

        Ok(local_hit.map(|hit| {
            let normal = if hit.mesh_normal {
                transform.mesh_normal_to_world(hit.normal)
            } else {
                transform.normal_to_world(hit.normal)
            };
            RaycastHit3D {
                collider_id: self.id,
                distance: hit.distance,
                position: ray.point_at(hit.distance),
                normal,
                front_face: dot(normal, ray.direction) < 0.0,
                triangle_index: hit.triangle_index,
                barycentric: hit.barycentric,
                is_trigger: self.is_trigger,
            }
        }))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RaycastHit3D {
    pub collider_id: u64,
    /// World-space distance because input rays are normalized before querying.
    pub distance: f32,
    pub position: Vec3,
    /// Outward geometric normal, including the authored triangle winding.
    pub normal: Vec3,
    pub front_face: bool,
    pub triangle_index: Option<u32>,
    pub barycentric: Option<[f32; 3]>,
    pub is_trigger: bool,
}

pub(crate) fn raycast_nearest(
    colliders: &[Collider3D],
    ray: Ray3D,
    max_distance: f32,
    query: QueryFilter3D,
) -> Physics3dResult<Option<RaycastHit3D>> {
    let ray = ray.normalized()?;
    validate_max_distance(max_distance)?;
    let broadphase_ray = LocalRay {
        origin: ray.origin,
        direction: ray.direction,
    };
    let mut best_distance = max_distance;
    let mut best = None;
    for collider in colliders {
        if !collider.enabled
            || (!query.include_triggers && collider.is_trigger)
            || !query.collision.allows(collider.filter)
        {
            continue;
        }
        if collider
            .world_aabb()?
            .ray_interval(broadphase_ray, best_distance)
            .is_none()
        {
            continue;
        }
        if let Some(hit) = collider.raycast_prepared(ray, best_distance)? {
            best_distance = hit.distance;
            best = Some(hit);
        }
    }
    Ok(best)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BroadphasePair3D {
    pub first_index: usize,
    pub second_index: usize,
    pub first_id: u64,
    pub second_id: u64,
    pub has_trigger: bool,
}

/// Generate potentially colliding AABB pairs using a sweep along the axis with
/// the widest set-wide spread. Output is deterministic and already respects
/// enabled state and symmetric layer/mask filtering.
pub(crate) fn broadphase_pairs(colliders: &[Collider3D]) -> Physics3dResult<Vec<BroadphasePair3D>> {
    #[derive(Clone, Copy)]
    struct Entry {
        collider_index: usize,
        bounds: Aabb3D,
    }

    let mut entries = Vec::with_capacity(colliders.len());
    for (collider_index, collider) in colliders.iter().enumerate() {
        if collider.enabled {
            entries.push(Entry {
                collider_index,
                bounds: collider.world_aabb()?,
            });
        }
    }
    if entries.len() < 2 {
        return Ok(Vec::new());
    }

    let overall = entries[1..]
        .iter()
        .fold(entries[0].bounds, |bounds, entry| {
            bounds.union(entry.bounds)
        });
    let spread = sub(overall.max, overall.min);
    let axis = if spread.x >= spread.y && spread.x >= spread.z {
        0
    } else if spread.y >= spread.z {
        1
    } else {
        2
    };
    entries.sort_unstable_by(|left, right| {
        component(left.bounds.min, axis)
            .total_cmp(&component(right.bounds.min, axis))
            .then_with(|| left.collider_index.cmp(&right.collider_index))
    });

    let mut active = Vec::<Entry>::new();
    let mut pairs = Vec::new();
    for entry in entries {
        let entry_min = component(entry.bounds.min, axis);
        active.retain(|candidate| component(candidate.bounds.max, axis) >= entry_min);
        for candidate in &active {
            if !candidate.bounds.overlaps(entry.bounds) {
                continue;
            }
            let first_index = candidate.collider_index.min(entry.collider_index);
            let second_index = candidate.collider_index.max(entry.collider_index);
            let first = &colliders[first_index];
            let second = &colliders[second_index];
            if !first.filter.allows(second.filter) {
                continue;
            }
            pairs.push(BroadphasePair3D {
                first_index,
                second_index,
                first_id: first.id,
                second_id: second.id,
                has_trigger: first.is_trigger || second.is_trigger,
            });
        }
        active.push(entry);
    }
    pairs.sort_unstable_by_key(|pair| (pair.first_index, pair.second_index));
    Ok(pairs)
}

/// Describes how closely a generated contact represents the authored shape.
/// Bounds contacts are intentionally useful for triggers and diagnostics only;
/// the runtime response helper ignores them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContactQuality3D {
    Exact,
    Bounds,
}

impl ContactQuality3D {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Bounds => "bounds",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Contact3D {
    pub first_index: usize,
    pub second_index: usize,
    pub first_id: u64,
    pub second_id: u64,
    /// Unit vector pointing from the first collider toward the second.
    pub normal: Vec3,
    /// Minimum separation distance along `normal` for exact primitive pairs.
    /// For bounds contacts this is only the overlapping-AABB depth.
    pub penetration: f32,
    /// Representative world-space contact point. Exact tests return a stable
    /// midpoint between opposing support points, not a multi-point manifold.
    pub point: Vec3,
    pub has_trigger: bool,
    pub quality: ContactQuality3D,
}

#[derive(Clone, Copy, Debug)]
struct Obb3D {
    center: Vec3,
    axes: [Vec3; 3],
    half_extents: [f32; 3],
}

#[derive(Clone, Copy, Debug)]
struct WorldSphere3D {
    center: Vec3,
    radius: f32,
}

#[derive(Clone, Copy, Debug)]
struct WorldCapsule3D {
    start: Vec3,
    end: Vec3,
    radius: f32,
}

#[derive(Clone, Copy, Debug)]
enum ExactContactShape3D {
    Box(Obb3D),
    Sphere(WorldSphere3D),
    Capsule(WorldCapsule3D),
}

fn contact_shape(collider: &Collider3D) -> Physics3dResult<Option<ExactContactShape3D>> {
    collider.shape.validate()?;
    let model = collider.transform.matrix()?;
    let matrix = model.values;
    let columns = [
        Vec3::new(matrix[0][0], matrix[1][0], matrix[2][0]),
        Vec3::new(matrix[0][1], matrix[1][1], matrix[2][1]),
        Vec3::new(matrix[0][2], matrix[1][2], matrix[2][2]),
    ];
    let lengths = columns.map(|column| dot(column, column).sqrt());
    if lengths
        .iter()
        .any(|length| !length.is_finite() || *length <= TRANSFORM_EPSILON)
    {
        return Err(Physics3dError::InvalidTransform(
            "contact transform has a singular basis".into(),
        ));
    }
    let center = model.transform_point(Vec3::ZERO);

    Ok(match &collider.shape {
        ColliderShape3D::Box { half_extents } => Some(ExactContactShape3D::Box(Obb3D {
            center,
            axes: [
                scale(columns[0], lengths[0].recip()),
                scale(columns[1], lengths[1].recip()),
                scale(columns[2], lengths[2].recip()),
            ],
            half_extents: [
                half_extents.x * lengths[0],
                half_extents.y * lengths[1],
                half_extents.z * lengths[2],
            ],
        })),
        ColliderShape3D::Sphere { radius } if scale_is_uniform(lengths) => {
            Some(ExactContactShape3D::Sphere(WorldSphere3D {
                center,
                radius: radius * uniform_scale(lengths),
            }))
        }
        ColliderShape3D::Capsule {
            radius,
            half_height,
        } if scale_is_uniform(lengths) => {
            let axis = scale(columns[1], lengths[1].recip());
            let segment_half = half_height * lengths[1];
            Some(ExactContactShape3D::Capsule(WorldCapsule3D {
                start: sub(center, scale(axis, segment_half)),
                end: add(center, scale(axis, segment_half)),
                radius: radius * uniform_scale(lengths),
            }))
        }
        ColliderShape3D::Sphere { .. }
        | ColliderShape3D::Capsule { .. }
        | ColliderShape3D::TriangleMesh(_) => None,
    })
}

fn scale_is_uniform(lengths: [f32; 3]) -> bool {
    let smallest = lengths[0].min(lengths[1]).min(lengths[2]);
    let largest = lengths[0].max(lengths[1]).max(lengths[2]);
    largest - smallest <= largest.max(1.0) * 1.0e-5
}

fn uniform_scale(lengths: [f32; 3]) -> f32 {
    (lengths[0] + lengths[1] + lengths[2]) / 3.0
}

/// Generate a narrow-phase contact for a specific collider pair. Layer/mask,
/// enabled state, and AABB rejection are applied before the primitive tests.
pub(crate) fn contact_pair(
    first: &Collider3D,
    second: &Collider3D,
) -> Physics3dResult<Option<Contact3D>> {
    if !first.enabled || !second.enabled || !first.filter.allows(second.filter) {
        return Ok(None);
    }
    let first_bounds = first.world_aabb()?;
    let second_bounds = second.world_aabb()?;
    if !first_bounds.overlaps(second_bounds) {
        return Ok(None);
    }

    let exact = match (contact_shape(first)?, contact_shape(second)?) {
        (Some(ExactContactShape3D::Box(first)), Some(ExactContactShape3D::Box(second))) => {
            Some(contact_obb_obb(first, second))
        }
        (Some(ExactContactShape3D::Sphere(first)), Some(ExactContactShape3D::Sphere(second))) => {
            Some(contact_sphere_sphere(first, second))
        }
        (Some(ExactContactShape3D::Sphere(first)), Some(ExactContactShape3D::Box(second))) => {
            Some(contact_sphere_obb(first, second))
        }
        (Some(ExactContactShape3D::Box(first)), Some(ExactContactShape3D::Sphere(second))) => {
            Some(contact_sphere_obb(second, first).map(ContactGeometry3D::flipped))
        }
        (Some(ExactContactShape3D::Capsule(first)), Some(ExactContactShape3D::Sphere(second))) => {
            Some(contact_capsule_sphere(first, second))
        }
        (Some(ExactContactShape3D::Sphere(first)), Some(ExactContactShape3D::Capsule(second))) => {
            Some(contact_capsule_sphere(second, first).map(ContactGeometry3D::flipped))
        }
        (Some(ExactContactShape3D::Capsule(first)), Some(ExactContactShape3D::Capsule(second))) => {
            Some(contact_capsule_capsule(first, second))
        }
        // Capsule-box contact manifolds and all mesh contacts are intentionally
        // bounds-only until a shape-accurate solver is available.
        _ => None,
    };

    let (geometry, quality) = match exact {
        Some(Some(contact)) => (contact, ContactQuality3D::Exact),
        Some(None) => return Ok(None),
        None => (
            contact_aabb_aabb(first_bounds, second_bounds)
                .expect("overlapping bounds must produce bounds contact"),
            ContactQuality3D::Bounds,
        ),
    };
    Ok(Some(Contact3D {
        first_index: 0,
        second_index: 1,
        first_id: first.id,
        second_id: second.id,
        normal: geometry.normal,
        penetration: geometry.penetration,
        point: geometry.point,
        has_trigger: first.is_trigger || second.is_trigger,
        quality,
    }))
}

/// Sweep-and-prune followed by narrow-phase contact generation. Output order is
/// deterministic for a deterministic collider slice.
pub(crate) fn contacts(colliders: &[Collider3D]) -> Physics3dResult<Vec<Contact3D>> {
    let pairs = broadphase_pairs(colliders)?;
    let mut contacts = Vec::with_capacity(pairs.len());
    for pair in pairs {
        if let Some(mut contact) =
            contact_pair(&colliders[pair.first_index], &colliders[pair.second_index])?
        {
            contact.first_index = pair.first_index;
            contact.second_index = pair.second_index;
            contacts.push(contact);
        }
    }
    Ok(contacts)
}

#[derive(Clone, Copy, Debug)]
struct ContactGeometry3D {
    normal: Vec3,
    penetration: f32,
    point: Vec3,
}

impl ContactGeometry3D {
    fn flipped(mut self) -> Self {
        self.normal = scale(self.normal, -1.0);
        self
    }
}

fn contact_aabb_aabb(first: Aabb3D, second: Aabb3D) -> Option<ContactGeometry3D> {
    let overlaps = [
        first.max.x.min(second.max.x) - first.min.x.max(second.min.x),
        first.max.y.min(second.max.y) - first.min.y.max(second.min.y),
        first.max.z.min(second.max.z) - first.min.z.max(second.min.z),
    ];
    if overlaps.iter().any(|overlap| *overlap < 0.0) {
        return None;
    }
    let axis = if overlaps[0] <= overlaps[1] && overlaps[0] <= overlaps[2] {
        0
    } else if overlaps[1] <= overlaps[2] {
        1
    } else {
        2
    };
    let delta = sub(second.center(), first.center());
    let normal = axis_vector(
        axis,
        if component(delta, axis) < 0.0 {
            -1.0
        } else {
            1.0
        },
    );
    Some(ContactGeometry3D {
        normal,
        penetration: overlaps[axis].max(0.0),
        point: Vec3::new(
            (first.min.x.max(second.min.x) + first.max.x.min(second.max.x)) * 0.5,
            (first.min.y.max(second.min.y) + first.max.y.min(second.max.y)) * 0.5,
            (first.min.z.max(second.min.z) + first.max.z.min(second.max.z)) * 0.5,
        ),
    })
}

fn contact_obb_obb(first: Obb3D, second: Obb3D) -> Option<ContactGeometry3D> {
    let center_delta = sub(second.center, first.center);
    let mut best_normal = Vec3::ZERO;
    let mut best_penetration = f32::INFINITY;

    for raw_axis in first.axes.into_iter().chain(second.axes).chain(
        first
            .axes
            .into_iter()
            .flat_map(|left| second.axes.into_iter().map(move |right| cross(left, right))),
    ) {
        let length_squared = dot(raw_axis, raw_axis);
        if length_squared <= 1.0e-10 {
            continue;
        }
        let axis = scale(raw_axis, length_squared.sqrt().recip());
        let first_radius = obb_projection_radius(first, axis);
        let second_radius = obb_projection_radius(second, axis);
        let signed_distance = dot(center_delta, axis);
        let penetration = first_radius + second_radius - signed_distance.abs();
        if penetration < -1.0e-5 {
            return None;
        }
        if penetration < best_penetration {
            best_penetration = penetration.max(0.0);
            best_normal = if signed_distance < 0.0 {
                scale(axis, -1.0)
            } else {
                axis
            };
        }
    }
    if best_normal == Vec3::ZERO {
        return None;
    }
    let first_support = obb_support(first, best_normal);
    let second_support = obb_support(second, scale(best_normal, -1.0));
    Some(ContactGeometry3D {
        normal: best_normal,
        penetration: best_penetration,
        point: scale(add(first_support, second_support), 0.5),
    })
}

fn obb_projection_radius(obb: Obb3D, axis: Vec3) -> f32 {
    obb.half_extents[0] * dot(obb.axes[0], axis).abs()
        + obb.half_extents[1] * dot(obb.axes[1], axis).abs()
        + obb.half_extents[2] * dot(obb.axes[2], axis).abs()
}

fn obb_support(obb: Obb3D, direction: Vec3) -> Vec3 {
    let mut point = obb.center;
    for axis in 0..3 {
        point = add(
            point,
            scale(
                obb.axes[axis],
                if dot(obb.axes[axis], direction) < 0.0 {
                    -obb.half_extents[axis]
                } else {
                    obb.half_extents[axis]
                },
            ),
        );
    }
    point
}

fn contact_sphere_sphere(first: WorldSphere3D, second: WorldSphere3D) -> Option<ContactGeometry3D> {
    contact_ball_ball(first.center, first.radius, second.center, second.radius)
}

fn contact_ball_ball(
    first_center: Vec3,
    first_radius: f32,
    second_center: Vec3,
    second_radius: f32,
) -> Option<ContactGeometry3D> {
    let delta = sub(second_center, first_center);
    let distance_squared = dot(delta, delta);
    let radius_sum = first_radius + second_radius;
    if distance_squared > radius_sum * radius_sum {
        return None;
    }
    let distance = distance_squared.sqrt();
    let normal = if distance > DIRECTION_EPSILON {
        scale(delta, distance.recip())
    } else {
        Vec3::new(1.0, 0.0, 0.0)
    };
    let first_surface = add(first_center, scale(normal, first_radius));
    let second_surface = sub(second_center, scale(normal, second_radius));
    Some(ContactGeometry3D {
        normal,
        penetration: (radius_sum - distance).max(0.0),
        point: scale(add(first_surface, second_surface), 0.5),
    })
}

fn contact_sphere_obb(sphere: WorldSphere3D, obb: Obb3D) -> Option<ContactGeometry3D> {
    let from_box = sub(sphere.center, obb.center);
    let local = obb.axes.map(|axis| dot(from_box, axis));
    let clamped = [
        local[0].clamp(-obb.half_extents[0], obb.half_extents[0]),
        local[1].clamp(-obb.half_extents[1], obb.half_extents[1]),
        local[2].clamp(-obb.half_extents[2], obb.half_extents[2]),
    ];
    let mut closest = obb.center;
    for axis in 0..3 {
        closest = add(closest, scale(obb.axes[axis], clamped[axis]));
    }
    let toward_box = sub(closest, sphere.center);
    let distance_squared = dot(toward_box, toward_box);
    if distance_squared > sphere.radius * sphere.radius {
        return None;
    }
    if distance_squared > DIRECTION_EPSILON * DIRECTION_EPSILON {
        let distance = distance_squared.sqrt();
        let normal = scale(toward_box, distance.recip());
        let sphere_surface = add(sphere.center, scale(normal, sphere.radius));
        return Some(ContactGeometry3D {
            normal,
            penetration: (sphere.radius - distance).max(0.0),
            point: scale(add(sphere_surface, closest), 0.5),
        });
    }

    // The sphere center is inside the box. Select the nearest face and orient
    // the normal inward so `first -= normal * penetration` ejects the sphere.
    let gaps = [
        obb.half_extents[0] - local[0].abs(),
        obb.half_extents[1] - local[1].abs(),
        obb.half_extents[2] - local[2].abs(),
    ];
    let axis = if gaps[0] <= gaps[1] && gaps[0] <= gaps[2] {
        0
    } else if gaps[1] <= gaps[2] {
        1
    } else {
        2
    };
    let outward = scale(obb.axes[axis], if local[axis] < 0.0 { -1.0 } else { 1.0 });
    Some(ContactGeometry3D {
        normal: scale(outward, -1.0),
        penetration: sphere.radius + gaps[axis],
        point: add(sphere.center, scale(outward, gaps[axis])),
    })
}

fn contact_capsule_sphere(
    capsule: WorldCapsule3D,
    sphere: WorldSphere3D,
) -> Option<ContactGeometry3D> {
    let closest = closest_point_on_segment(capsule.start, capsule.end, sphere.center);
    contact_ball_ball(closest, capsule.radius, sphere.center, sphere.radius)
}

fn contact_capsule_capsule(
    first: WorldCapsule3D,
    second: WorldCapsule3D,
) -> Option<ContactGeometry3D> {
    let (first_point, second_point) =
        closest_points_on_segments(first.start, first.end, second.start, second.end);
    contact_ball_ball(first_point, first.radius, second_point, second.radius)
}

fn closest_point_on_segment(start: Vec3, end: Vec3, point: Vec3) -> Vec3 {
    let delta = sub(end, start);
    let denominator = dot(delta, delta);
    if denominator <= DIRECTION_EPSILON * DIRECTION_EPSILON {
        start
    } else {
        add(
            start,
            scale(
                delta,
                (dot(sub(point, start), delta) / denominator).clamp(0.0, 1.0),
            ),
        )
    }
}

fn closest_points_on_segments(
    first_start: Vec3,
    first_end: Vec3,
    second_start: Vec3,
    second_end: Vec3,
) -> (Vec3, Vec3) {
    let first_delta = sub(first_end, first_start);
    let second_delta = sub(second_end, second_start);
    let between_starts = sub(first_start, second_start);
    let first_length_squared = dot(first_delta, first_delta);
    let second_length_squared = dot(second_delta, second_delta);
    let second_projection = dot(second_delta, between_starts);
    let epsilon = DIRECTION_EPSILON * DIRECTION_EPSILON;

    let (mut first_t, mut second_t) = if first_length_squared <= epsilon {
        (
            0.0,
            if second_length_squared <= epsilon {
                0.0
            } else {
                (second_projection / second_length_squared).clamp(0.0, 1.0)
            },
        )
    } else {
        let first_projection = dot(first_delta, between_starts);
        if second_length_squared <= epsilon {
            (
                (-first_projection / first_length_squared).clamp(0.0, 1.0),
                0.0,
            )
        } else {
            let cross_projection = dot(first_delta, second_delta);
            let denominator =
                first_length_squared * second_length_squared - cross_projection * cross_projection;
            let first_t = if denominator.abs() > epsilon {
                ((cross_projection * second_projection - first_projection * second_length_squared)
                    / denominator)
                    .clamp(0.0, 1.0)
            } else {
                0.0
            };
            let second_t = (cross_projection * first_t + second_projection) / second_length_squared;
            (first_t, second_t)
        }
    };

    if second_t < 0.0 {
        second_t = 0.0;
        first_t =
            (-dot(first_delta, between_starts) / first_length_squared.max(epsilon)).clamp(0.0, 1.0);
    } else if second_t > 1.0 {
        second_t = 1.0;
        first_t = ((dot(first_delta, second_delta) - dot(first_delta, between_starts))
            / first_length_squared.max(epsilon))
        .clamp(0.0, 1.0);
    }
    (
        add(first_start, scale(first_delta, first_t)),
        add(second_start, scale(second_delta, second_t)),
    )
}

/// Resolve the first body's linear velocity against a stationary surface.
/// `normal` must point from the first body toward the surface.
pub(crate) fn resolve_contact_velocity(
    velocity: Vec3,
    normal: Vec3,
    restitution: f32,
    friction: f32,
) -> Vec3 {
    let speed_into_surface = dot(velocity, normal);
    if speed_into_surface <= 0.0 {
        return velocity;
    }
    let normal_velocity = scale(normal, speed_into_surface);
    let tangent_velocity = sub(velocity, normal_velocity);
    add(
        scale(tangent_velocity, (1.0 - friction.clamp(0.0, 1.0)).max(0.0)),
        scale(normal, -speed_into_surface * restitution.clamp(0.0, 1.0)),
    )
}

#[derive(Clone, Copy)]
struct LocalRay {
    origin: Vec3,
    direction: Vec3,
}

#[derive(Clone, Copy)]
struct LocalShapeHit {
    distance: f32,
    normal: Vec3,
    triangle_index: Option<u32>,
    barycentric: Option<[f32; 3]>,
    mesh_normal: bool,
}

#[derive(Clone, Copy)]
struct SphereHit {
    distance: f32,
    normal: Vec3,
}

#[derive(Clone, Copy)]
struct MeshRayHit {
    distance: f32,
    normal: Vec3,
    triangle_index: u32,
    barycentric: [f32; 3],
}

fn ray_box(ray: LocalRay, half: Vec3, max_distance: f32) -> Option<LocalShapeHit> {
    let origins = [ray.origin.x, ray.origin.y, ray.origin.z];
    let directions = [ray.direction.x, ray.direction.y, ray.direction.z];
    let extents = [half.x, half.y, half.z];
    let mut enter = f32::NEG_INFINITY;
    let mut exit = f32::INFINITY;
    let mut enter_normal = Vec3::ZERO;
    let mut exit_normal = Vec3::ZERO;

    for axis in 0..3 {
        if directions[axis].abs() <= DIRECTION_EPSILON {
            if origins[axis] < -extents[axis] || origins[axis] > extents[axis] {
                return None;
            }
            continue;
        }
        let mut near = (-extents[axis] - origins[axis]) / directions[axis];
        let mut far = (extents[axis] - origins[axis]) / directions[axis];
        let mut near_normal = axis_vector(axis, -1.0);
        let mut far_normal = axis_vector(axis, 1.0);
        if near > far {
            std::mem::swap(&mut near, &mut far);
            std::mem::swap(&mut near_normal, &mut far_normal);
        }
        if near > enter {
            enter = near;
            enter_normal = near_normal;
        }
        if far < exit {
            exit = far;
            exit_normal = far_normal;
        }
        if enter > exit {
            return None;
        }
    }

    let (distance, normal) = if enter >= 0.0 {
        (enter, enter_normal)
    } else if exit >= 0.0 {
        (exit, exit_normal)
    } else {
        return None;
    };
    (distance <= max_distance).then_some(LocalShapeHit {
        distance,
        normal,
        triangle_index: None,
        barycentric: None,
        mesh_normal: false,
    })
}

fn ray_sphere(ray: LocalRay, center: Vec3, radius: f32, max_distance: f32) -> Option<SphereHit> {
    let [near, far] = ray_sphere_roots(ray, center, radius)?;
    let distance = if near >= 0.0 {
        near
    } else if far >= 0.0 {
        far
    } else {
        return None;
    };
    if distance > max_distance {
        return None;
    }
    let point = add(ray.origin, scale(ray.direction, distance));
    Some(SphereHit {
        distance,
        normal: normalize(sub(point, center)),
    })
}

fn ray_sphere_roots(ray: LocalRay, center: Vec3, radius: f32) -> Option<[f32; 2]> {
    let offset = sub(ray.origin, center);
    let a = dot(ray.direction, ray.direction);
    if a <= f32::EPSILON {
        return None;
    }
    let half_b = dot(offset, ray.direction);
    let c = dot(offset, offset) - radius * radius;
    let discriminant = half_b * half_b - a * c;
    if discriminant < 0.0 {
        return None;
    }
    let root = discriminant.max(0.0).sqrt();
    let near = (-half_b - root) / a;
    let far = (-half_b + root) / a;
    Some([near, far])
}

fn ray_capsule(
    ray: LocalRay,
    radius: f32,
    half_height: f32,
    max_distance: f32,
) -> Option<LocalShapeHit> {
    let mut best: Option<SphereHit> = None;
    let mut consider = |candidate: SphereHit| {
        if candidate.distance <= max_distance
            && best.is_none_or(|current| candidate.distance < current.distance)
        {
            best = Some(candidate);
        }
    };

    // Infinite Y cylinder, clipped to the straight section.
    let a = ray.direction.x * ray.direction.x + ray.direction.z * ray.direction.z;
    if a > f32::EPSILON {
        let half_b = ray.origin.x * ray.direction.x + ray.origin.z * ray.direction.z;
        let c = ray.origin.x * ray.origin.x + ray.origin.z * ray.origin.z - radius * radius;
        let discriminant = half_b * half_b - a * c;
        if discriminant >= 0.0 {
            let root = discriminant.max(0.0).sqrt();
            for distance in [(-half_b - root) / a, (-half_b + root) / a] {
                if distance < 0.0 {
                    continue;
                }
                let point = add(ray.origin, scale(ray.direction, distance));
                if point.y >= -half_height && point.y <= half_height {
                    consider(SphereHit {
                        distance,
                        normal: normalize(Vec3::new(point.x, 0.0, point.z)),
                    });
                }
            }
        }
    }

    let top_center = Vec3::new(0.0, half_height, 0.0);
    if let Some(roots) = ray_sphere_roots(ray, top_center, radius) {
        for distance in roots {
            if distance < 0.0 || distance > max_distance {
                continue;
            }
            let point = add(ray.origin, scale(ray.direction, distance));
            if point.y >= half_height - DIRECTION_EPSILON {
                consider(SphereHit {
                    distance,
                    normal: normalize(sub(point, top_center)),
                });
            }
        }
    }
    let bottom_center = Vec3::new(0.0, -half_height, 0.0);
    if let Some(roots) = ray_sphere_roots(ray, bottom_center, radius) {
        for distance in roots {
            if distance < 0.0 || distance > max_distance {
                continue;
            }
            let point = add(ray.origin, scale(ray.direction, distance));
            if point.y <= -half_height + DIRECTION_EPSILON {
                consider(SphereHit {
                    distance,
                    normal: normalize(sub(point, bottom_center)),
                });
            }
        }
    }

    best.map(|hit| LocalShapeHit {
        distance: hit.distance,
        normal: hit.normal,
        triangle_index: None,
        barycentric: None,
        mesh_normal: false,
    })
}

/// Double-sided Moller-Trumbore test performed in f64 to keep edge and large
/// world-coordinate queries stable while retaining compact f32 engine data.
fn ray_triangle(ray: LocalRay, vertices: [Vec3; 3], max_distance: f32) -> Option<(f32, [f32; 3])> {
    let to_f64 = |value: Vec3| [value.x as f64, value.y as f64, value.z as f64];
    let subtract = |left: [f64; 3], right: [f64; 3]| {
        [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
    };
    let cross64 = |left: [f64; 3], right: [f64; 3]| {
        [
            left[1] * right[2] - left[2] * right[1],
            left[2] * right[0] - left[0] * right[2],
            left[0] * right[1] - left[1] * right[0],
        ]
    };
    let dot64 = |left: [f64; 3], right: [f64; 3]| {
        left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
    };

    let origin = to_f64(ray.origin);
    let direction = to_f64(ray.direction);
    let a = to_f64(vertices[0]);
    let edge_ab = subtract(to_f64(vertices[1]), a);
    let edge_ac = subtract(to_f64(vertices[2]), a);
    let p = cross64(direction, edge_ac);
    let determinant = dot64(edge_ab, p);
    let determinant_scale = dot64(edge_ab, edge_ab)
        .sqrt()
        .mul_add(dot64(edge_ac, edge_ac).sqrt(), 0.0)
        * dot64(direction, direction).sqrt();
    let determinant_epsilon = f64::EPSILON * 64.0 * determinant_scale.max(f64::MIN_POSITIVE);
    if determinant.abs() <= determinant_epsilon {
        return None;
    }
    let inverse_determinant = determinant.recip();
    let origin_to_a = subtract(origin, a);
    let u = dot64(origin_to_a, p) * inverse_determinant;
    const BARYCENTRIC_EPSILON: f64 = 1.0e-9;
    if u < -BARYCENTRIC_EPSILON || u > 1.0 + BARYCENTRIC_EPSILON {
        return None;
    }
    let q = cross64(origin_to_a, edge_ab);
    let v = dot64(direction, q) * inverse_determinant;
    if v < -BARYCENTRIC_EPSILON || u + v > 1.0 + BARYCENTRIC_EPSILON {
        return None;
    }
    let distance = dot64(edge_ac, q) * inverse_determinant;
    if distance < -BARYCENTRIC_EPSILON || distance > max_distance as f64 {
        return None;
    }
    let distance = distance.max(0.0) as f32;
    Some((distance, [(1.0 - u - v) as f32, u as f32, v as f32]))
}

fn validate_max_distance(max_distance: f32) -> Physics3dResult<()> {
    if max_distance.is_nan() || max_distance < 0.0 {
        Err(Physics3dError::InvalidRay(
            "maximum distance must be non-negative and not NaN".into(),
        ))
    } else {
        Ok(())
    }
}

fn vec3_is_finite(value: Vec3) -> bool {
    value.x.is_finite() && value.y.is_finite() && value.z.is_finite()
}

fn add(left: Vec3, right: Vec3) -> Vec3 {
    Vec3::new(left.x + right.x, left.y + right.y, left.z + right.z)
}

fn sub(left: Vec3, right: Vec3) -> Vec3 {
    Vec3::new(left.x - right.x, left.y - right.y, left.z - right.z)
}

fn scale(value: Vec3, amount: f32) -> Vec3 {
    Vec3::new(value.x * amount, value.y * amount, value.z * amount)
}

fn dot(left: Vec3, right: Vec3) -> f32 {
    left.x * right.x + left.y * right.y + left.z * right.z
}

fn cross(left: Vec3, right: Vec3) -> Vec3 {
    Vec3::new(
        left.y * right.z - left.z * right.y,
        left.z * right.x - left.x * right.z,
        left.x * right.y - left.y * right.x,
    )
}

fn normalize(value: Vec3) -> Vec3 {
    let length_squared = dot(value, value);
    if length_squared <= f32::EPSILON || !length_squared.is_finite() {
        Vec3::ZERO
    } else {
        scale(value, length_squared.sqrt().recip())
    }
}

fn component(value: Vec3, axis: usize) -> f32 {
    match axis {
        0 => value.x,
        1 => value.y,
        _ => value.z,
    }
}

fn axis_vector(axis: usize, value: f32) -> Vec3 {
    match axis {
        0 => Vec3::new(value, 0.0, 0.0),
        1 => Vec3::new(0.0, value, 0.0),
        _ => Vec3::new(0.0, 0.0, value),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::Instant;

    use crate::mesh::{MeshData, Vertex};

    use super::*;

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() <= 1.0e-4,
            "expected {expected}, got {actual}"
        );
    }

    fn triangle_handle(points: [[f32; 3]; 3]) -> MeshHandle {
        let vertices = points
            .into_iter()
            .map(Vertex::from_position)
            .collect::<Vec<_>>();
        let mesh = MeshData::new(
            "collision triangle",
            vertices,
            vec![0, 1, 2],
            Vec::new(),
            Vec::new(),
            true,
        )
        .expect("triangle mesh should be valid");
        MeshHandle::new(mesh).expect("triangle handle should be valid")
    }

    #[test]
    fn mesh_collider_conforms_to_triangle_instead_of_its_bounds() {
        let mesh = triangle_handle([[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]]);
        let shape = ColliderShape3D::triangle_mesh(mesh).expect("mesh collider");
        let ColliderShape3D::TriangleMesh(mesh_collider) = &shape else {
            panic!("expected triangle mesh shape");
        };
        assert_eq!(mesh_collider.triangle_count(), 1);
        assert_eq!(mesh_collider.bvh_node_count(), 1);
        let collider = Collider3D::new(41, shape);

        let hit = collider
            .raycast(
                Ray3D::new(Vec3::new(0.25, 0.25, 1.0), Vec3::new(0.0, 0.0, -1.0)).expect("ray"),
                10.0,
                QueryFilter3D::default(),
            )
            .expect("query")
            .expect("triangle hit");
        assert_close(hit.distance, 1.0);
        assert_close(hit.position.z, 0.0);
        assert_close(hit.normal.z, 1.0);
        assert_eq!(hit.triangle_index, Some(0));
        let barycentric = hit.barycentric.expect("mesh barycentrics");
        assert_close(barycentric.iter().sum(), 1.0);

        // This ray passes through the triangle's AABB but lies above the
        // diagonal edge, proving that the mesh is not approximated by a box.
        let miss = collider
            .raycast(
                Ray3D::new(Vec3::new(1.75, 1.75, 1.0), Vec3::new(0.0, 0.0, -1.0)).expect("ray"),
                10.0,
                QueryFilter3D::default(),
            )
            .expect("query");
        assert!(miss.is_none());
    }

    #[test]
    fn primitive_raycast_distances_and_normals_are_exact_after_scaling() {
        let mut sphere = Collider3D::new(1, ColliderShape3D::Sphere { radius: 1.0 });
        sphere.transform.scale = Vec3::new(2.0, 1.0, 1.0);
        let hit = sphere
            .raycast(
                Ray3D::new(Vec3::new(3.0, 0.0, 0.0), Vec3::new(-5.0, 0.0, 0.0))
                    .expect("ray is normalized by constructor"),
                100.0,
                QueryFilter3D::default(),
            )
            .expect("sphere query")
            .expect("sphere hit");
        assert_close(hit.distance, 1.0);
        assert_close(hit.normal.x, 1.0);
        assert!(hit.front_face);

        let box_collider = Collider3D::new(
            2,
            ColliderShape3D::Box {
                half_extents: Vec3::new(1.0, 1.0, 1.0),
            },
        );
        let hit = box_collider
            .raycast(
                Ray3D::new(Vec3::new(0.0, 0.0, 3.0), Vec3::new(0.0, 0.0, -1.0)).expect("ray"),
                10.0,
                QueryFilter3D::default(),
            )
            .expect("box query")
            .expect("box hit");
        assert_close(hit.distance, 2.0);
        assert_close(hit.normal.z, 1.0);

        let capsule = Collider3D::new(
            3,
            ColliderShape3D::Capsule {
                radius: 1.0,
                half_height: 2.0,
            },
        );
        let side = capsule
            .raycast(
                Ray3D::new(Vec3::new(3.0, 1.5, 0.0), Vec3::new(-1.0, 0.0, 0.0)).expect("ray"),
                10.0,
                QueryFilter3D::default(),
            )
            .expect("capsule query")
            .expect("capsule side hit");
        assert_close(side.distance, 2.0);
        assert_close(side.normal.x, 1.0);
        let cap = capsule
            .raycast(
                Ray3D::new(Vec3::new(0.0, 5.0, 0.0), Vec3::new(0.0, -1.0, 0.0)).expect("ray"),
                10.0,
                QueryFilter3D::default(),
            )
            .expect("capsule query")
            .expect("capsule cap hit");
        assert_close(cap.distance, 2.0);
        assert_close(cap.normal.y, 1.0);
        let from_inside = capsule
            .raycast(
                Ray3D::new(Vec3::ZERO, Vec3::new(0.0, 1.0, 0.0)).expect("ray"),
                10.0,
                QueryFilter3D::default(),
            )
            .expect("capsule query")
            .expect("inside ray exits top cap");
        assert_close(from_inside.distance, 3.0);
        assert_close(from_inside.normal.y, 1.0);
    }

    #[test]
    fn rotated_mesh_reports_transformed_winding_normal_and_distance() {
        let mesh = triangle_handle([[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [0.0, 1.0, 0.0]]);
        let mut collider = Collider3D::new(
            7,
            ColliderShape3D::triangle_mesh(mesh).expect("mesh collider"),
        );
        collider.transform.position = Vec3::new(4.0, 0.0, 0.0);
        collider.transform.euler = Vec3::new(0.0, 90.0, 0.0);
        let hit = collider
            .raycast(
                Ray3D::new(Vec3::new(7.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0)).expect("ray"),
                10.0,
                QueryFilter3D::default(),
            )
            .expect("query")
            .expect("rotated mesh hit");
        assert_close(hit.distance, 3.0);
        assert_close(hit.normal.x, 1.0);
    }

    #[test]
    fn filtering_and_triggers_are_respected_by_queries_and_pairs() {
        let shape = || ColliderShape3D::Sphere { radius: 1.0 };
        let mut first = Collider3D::new(10, shape());
        first.filter = CollisionFilter3D::new(0b0001, 0b0010);
        let mut second = Collider3D::new(11, shape());
        second.filter = CollisionFilter3D::new(0b0010, 0b0001);
        second.is_trigger = true;
        second.transform.position.x = 1.0;
        assert_eq!(
            broadphase_pairs(&[first.clone(), second.clone()])
                .expect("pairs")
                .len(),
            1
        );

        let ray = Ray3D::new(Vec3::new(-3.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)).expect("ray");
        let query = QueryFilter3D {
            collision: CollisionFilter3D::new(0b0001, 0b0010),
            include_triggers: false,
        };
        assert!(second.raycast(ray, 10.0, query).expect("query").is_none());
        assert!(
            second
                .raycast(
                    ray,
                    10.0,
                    QueryFilter3D {
                        include_triggers: true,
                        ..query
                    }
                )
                .expect("query")
                .is_some()
        );

        second.filter.mask = 0;
        assert!(
            broadphase_pairs(&[first, second])
                .expect("pairs")
                .is_empty()
        );
    }

    #[test]
    fn nearest_raycast_uses_world_bounds_and_returns_closest_shape() {
        let mut far = Collider3D::new(20, ColliderShape3D::Sphere { radius: 0.5 });
        far.transform.position.z = -5.0;
        let mut near = Collider3D::new(21, ColliderShape3D::Sphere { radius: 0.5 });
        near.transform.position.z = -2.0;
        let ray = Ray3D::new(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0)).expect("ray");
        let hit = raycast_nearest(&[far, near], ray, f32::INFINITY, QueryFilter3D::default())
            .expect("world query")
            .expect("nearest hit");
        assert_eq!(hit.collider_id, 21);
        assert_close(hit.distance, 1.5);

        assert!(Aabb3D::new(Vec3::ZERO, Vec3::new(-1.0, 0.0, 0.0)).is_err());
    }

    #[test]
    fn primitive_narrowphase_reports_exact_contacts_and_rejects_aabb_false_positives() {
        let mut first = Collider3D::new(1, ColliderShape3D::Sphere { radius: 1.0 });
        let mut second = Collider3D::new(2, ColliderShape3D::Sphere { radius: 0.75 });
        second.transform.position.x = 1.5;
        let contact = contact_pair(&first, &second)
            .expect("sphere contact query")
            .expect("overlapping spheres");
        assert_eq!(contact.quality, ContactQuality3D::Exact);
        assert_close(contact.normal.x, 1.0);
        assert_close(contact.penetration, 0.25);

        first.shape = ColliderShape3D::Box {
            half_extents: Vec3::new(1.0, 1.0, 1.0),
        };
        second.shape = ColliderShape3D::Sphere { radius: 0.25 };
        second.transform.position = Vec3::new(1.2, 1.2, 0.0);
        // The sphere overlaps the box's AABB on x/y, but misses its corner.
        assert!(
            first
                .world_aabb()
                .expect("box bounds")
                .overlaps(second.world_aabb().expect("sphere bounds"))
        );
        assert!(
            contact_pair(&first, &second)
                .expect("box-sphere contact query")
                .is_none()
        );
    }

    #[test]
    fn rotated_boxes_and_capsules_have_exact_contact_normals() {
        let mut first = Collider3D::new(
            10,
            ColliderShape3D::Box {
                half_extents: Vec3::new(1.0, 0.5, 0.5),
            },
        );
        first.transform.euler.z = 35.0;
        let mut second = Collider3D::new(
            11,
            ColliderShape3D::Box {
                half_extents: Vec3::new(0.75, 0.75, 0.75),
            },
        );
        second.transform.position.x = 1.25;
        let box_contact = contact_pair(&first, &second)
            .expect("box SAT")
            .expect("rotated boxes overlap");
        assert_eq!(box_contact.quality, ContactQuality3D::Exact);
        assert!(box_contact.penetration > 0.0);
        assert_close(dot(box_contact.normal, box_contact.normal), 1.0);

        let capsule_shape = || ColliderShape3D::Capsule {
            radius: 0.5,
            half_height: 1.0,
        };
        let capsule = Collider3D::new(12, capsule_shape());
        let mut other = Collider3D::new(13, capsule_shape());
        other.transform.position.x = 0.75;
        let capsule_contact = contact_pair(&capsule, &other)
            .expect("capsule query")
            .expect("capsules overlap");
        assert_eq!(capsule_contact.quality, ContactQuality3D::Exact);
        assert_close(capsule_contact.normal.x, 1.0);
        assert_close(capsule_contact.penetration, 0.25);
    }

    #[test]
    fn mesh_and_nonuniform_round_shapes_are_explicitly_bounds_only() {
        let mesh = triangle_handle([[-2.0, -2.0, 0.0], [2.0, -2.0, 0.0], [0.0, 2.0, 0.0]]);
        let mesh = Collider3D::new(
            20,
            ColliderShape3D::triangle_mesh(mesh).expect("mesh collider"),
        );
        let sphere = Collider3D::new(21, ColliderShape3D::Sphere { radius: 0.5 });
        let mesh_contact = contact_pair(&mesh, &sphere)
            .expect("mesh contact query")
            .expect("bounds overlap");
        assert_eq!(mesh_contact.quality, ContactQuality3D::Bounds);

        let mut stretched = Collider3D::new(22, ColliderShape3D::Sphere { radius: 1.0 });
        stretched.transform.scale = Vec3::new(2.0, 1.0, 1.0);
        let round_contact = contact_pair(&stretched, &sphere)
            .expect("ellipsoid bounds query")
            .expect("bounds overlap");
        assert_eq!(round_contact.quality, ContactQuality3D::Bounds);
    }

    #[test]
    fn contact_velocity_response_separates_normal_and_tangent_motion() {
        let resolved = resolve_contact_velocity(
            Vec3::new(4.0, -2.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            0.5,
            0.25,
        );
        assert_close(resolved.x, -2.0);
        assert_close(resolved.y, -1.5);
        let unchanged = resolve_contact_velocity(
            Vec3::new(-1.0, 2.0, 3.0),
            Vec3::new(1.0, 0.0, 0.0),
            1.0,
            1.0,
        );
        assert_eq!(unchanged, Vec3::new(-1.0, 2.0, 3.0));
    }

    #[test]
    fn sweep_and_prune_matches_brute_force_aabb_pairs() {
        let mut state = 0xD1CE_BA5Eu64;
        let mut random = || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((state >> 40) as u32) as f32 / ((1u32 << 24) - 1) as f32
        };
        let mut colliders = Vec::new();
        for index in 0..300usize {
            let mut collider = Collider3D::new(
                index as u64,
                ColliderShape3D::Box {
                    half_extents: Vec3::new(0.2 + random(), 0.2 + random(), 0.2 + random()),
                },
            );
            collider.transform.position = Vec3::new(
                random() * 50.0 - 25.0,
                random() * 50.0 - 25.0,
                random() * 50.0 - 25.0,
            );
            collider.transform.euler =
                Vec3::new(random() * 180.0, random() * 180.0, random() * 180.0);
            collider.filter = if index % 5 == 0 {
                CollisionFilter3D::new(0b0010, 0b0010)
            } else {
                CollisionFilter3D::new(0b0001, 0b0001)
            };
            collider.enabled = index % 29 != 0;
            colliders.push(collider);
        }

        let actual = broadphase_pairs(&colliders)
            .expect("broadphase")
            .into_iter()
            .map(|pair| (pair.first_index, pair.second_index))
            .collect::<BTreeSet<_>>();
        let bounds = colliders
            .iter()
            .map(Collider3D::world_aabb)
            .collect::<Physics3dResult<Vec<_>>>()
            .expect("bounds");
        let mut expected = BTreeSet::new();
        for first in 0..colliders.len() {
            for second in first + 1..colliders.len() {
                if colliders[first].enabled
                    && colliders[second].enabled
                    && colliders[first].filter.allows(colliders[second].filter)
                    && bounds[first].overlaps(bounds[second])
                {
                    expected.insert((first, second));
                }
            }
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn malformed_and_degenerate_meshes_are_rejected_cleanly() {
        let empty = MeshData::new(
            "empty",
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            false,
        )
        .expect("empty render mesh is valid");
        let error = MeshCollider3D::from_mesh(MeshHandle::new(empty).expect("mesh handle"))
            .expect_err("empty collision mesh must be rejected");
        assert!(error.to_string().contains("non-empty triangle"));

        let degenerate = triangle_handle([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]]);
        let error = MeshCollider3D::from_mesh(degenerate)
            .expect_err("degenerate collision mesh must be rejected");
        assert!(error.to_string().contains("degenerate"));
    }

    #[test]
    fn live_mesh_refresh_is_revisioned_and_atomic() {
        let handle = triangle_handle([[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]]);
        let mut collider = MeshCollider3D::from_mesh(handle.clone()).expect("collider");
        let original_revision = collider.revision();
        assert!(!collider.refresh_if_changed().expect("no-op refresh"));
        let old_snapshot = collider.clone();
        assert!(Arc::ptr_eq(&collider.geometry, &old_snapshot.geometry));

        handle
            .replace_geometry(
                vec![
                    Vertex::from_position([10.0, 0.0, 0.0]),
                    Vertex::from_position([12.0, 0.0, 0.0]),
                    Vertex::from_position([10.0, 2.0, 0.0]),
                ],
                vec![0, 1, 2],
            )
            .expect("live mesh edit");
        assert!(collider.refresh_if_changed().expect("refresh"));
        assert_ne!(collider.revision(), original_revision);
        assert!(!Arc::ptr_eq(&collider.geometry, &old_snapshot.geometry));
        assert_close(collider.geometry.local_bounds.min.x, 10.0);
    }

    #[test]
    #[ignore = "diagnostic performance benchmark"]
    fn benchmark_sparse_broadphase_with_many_colliders() {
        let mut colliders = Vec::with_capacity(20_000);
        for index in 0..20_000usize {
            let mut collider = Collider3D::new(
                index as u64,
                ColliderShape3D::Box {
                    half_extents: Vec3::new(0.45, 0.45, 0.45),
                },
            );
            collider.transform.position = Vec3::new(
                (index % 200) as f32 * 2.0,
                ((index / 200) % 100) as f32 * 2.0,
                (index / 20_000) as f32 * 2.0,
            );
            colliders.push(collider);
        }
        let started = Instant::now();
        let pairs = broadphase_pairs(&colliders).expect("broadphase");
        eprintln!(
            "3D sweep-and-prune: {} colliders, {} pairs in {:?}",
            colliders.len(),
            pairs.len(),
            started.elapsed()
        );
        assert!(pairs.len() < colliders.len());
    }
}
