//! Backend-neutral mesh data, live-edit handles, and lightweight model importers.
//!
//! This module deliberately owns no renderer resources. Render backends can read
//! a [`MeshHandle`] under a shared lock and use its revision to decide when GPU
//! buffers need to be uploaded again.
//!
//! Importers return mesh-local geometry. glTF 2.0 skins and transform animation
//! channels are retained and can be CPU-skinned through [`MeshHandle`], keeping
//! the renderer contract backend-neutral. Morph targets, compressed accessors,
//! and model-level scene instancing remain outside this module. ASCII FBX
//! supports the common Model/Skin/Cluster/AnimationCurve subset; binary FBX
//! deformation is explicitly rejected rather than silently imported as static.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use base64::Engine as _;
use flate2::read::ZlibDecoder;
use serde_json::{Map as JsonMap, Value as JsonValue};

pub(crate) type MeshResult<T> = Result<T, MeshError>;

/// A model format accepted by [`import_from_bytes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MeshFormat {
    Obj,
    Gltf,
    Glb,
    Fbx,
}

impl MeshFormat {
    fn from_path(path: &Path) -> MeshResult<Self> {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .ok_or_else(|| MeshError::UnsupportedFormat("model path has no extension".into()))?;
        match extension.as_str() {
            "obj" => Ok(Self::Obj),
            "gltf" => Ok(Self::Gltf),
            "glb" => Ok(Self::Glb),
            "fbx" => Ok(Self::Fbx),
            _ => Err(MeshError::UnsupportedFormat(format!(
                "unsupported model extension '.{extension}'"
            ))),
        }
    }
}

/// A compact, renderer-ready vertex layout shared by all importers.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// XYZ is the tangent direction and W is its handedness.
    pub tangent: [f32; 4],
}

impl Vertex {
    pub(crate) fn from_position(position: [f32; 3]) -> Self {
        Self {
            position,
            normal: [0.0; 3],
            uv: [0.0; 2],
            tangent: [1.0, 0.0, 0.0, 1.0],
        }
    }
}

/// A texture reference retained as editable material metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TextureBinding {
    /// A relative URI, a data label, or an importer-generated embedded label.
    pub source: String,
    pub tex_coord: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum AlphaMode {
    #[default]
    Opaque,
    Mask,
    Blend,
}

/// Backend-neutral PBR material metadata.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MeshMaterial {
    pub name: String,
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: [f32; 3],
    pub base_color_texture: Option<TextureBinding>,
    pub normal_texture: Option<TextureBinding>,
    pub metallic_roughness_texture: Option<TextureBinding>,
    pub emissive_texture: Option<TextureBinding>,
    pub alpha_mode: AlphaMode,
    pub alpha_cutoff: f32,
    pub double_sided: bool,
}

impl MeshMaterial {
    pub(crate) fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            base_color: [1.0; 4],
            metallic: 1.0,
            roughness: 1.0,
            emissive: [0.0; 3],
            base_color_texture: None,
            normal_texture: None,
            metallic_roughness_texture: None,
            emissive_texture: None,
            alpha_mode: AlphaMode::Opaque,
            alpha_cutoff: 0.5,
            double_sided: false,
        }
    }
}

impl Default for MeshMaterial {
    fn default() -> Self {
        Self::named("Material")
    }
}

/// A contiguous triangle range rendered with one material.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Submesh {
    pub name: String,
    pub first_index: u32,
    pub index_count: u32,
    pub material: Option<usize>,
}

/// Bounds computed from vertex positions after every successful mutation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct MeshBounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub center: [f32; 3],
    pub radius: f32,
}

/// Local transform for one node in an imported armature hierarchy.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ArmatureNode {
    pub name: String,
    pub parent: Option<usize>,
    pub translation: [f32; 3],
    /// Unit quaternion in glTF's XYZW order.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

/// Four palette influences for a vertex. Zero total weight means unskinned.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkinWeights {
    pub joints: [u16; 4],
    pub weights: [f32; 4],
}

/// Skin palette plus immutable bind-pose geometry used by CPU deformation.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Armature {
    /// All hierarchy nodes, including non-joint ancestors needed for correct
    /// global transforms. Palette entries below index into this array.
    pub nodes: Vec<ArmatureNode>,
    pub joints: Vec<usize>,
    pub inverse_bind_matrices: Vec<[f32; 16]>,
    pub vertex_weights: Vec<SkinWeights>,
    pub bind_vertices: Vec<Vertex>,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) enum AnimationProperty {
    Translation,
    Rotation,
    Scale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AnimationInterpolation {
    Linear,
    Step,
}

/// Keyframes for one node/property pair. Translation and scale use XYZ;
/// rotation uses XYZW.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AnimationChannel {
    pub node: usize,
    pub property: AnimationProperty,
    pub interpolation: AnimationInterpolation,
    pub times: Vec<f32>,
    pub values: Vec<[f32; 4]>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AnimationClip {
    pub name: String,
    pub duration: f32,
    pub channels: Vec<AnimationChannel>,
}

/// Complete CPU-side geometry and editable metadata.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MeshData {
    pub name: String,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub submeshes: Vec<Submesh>,
    pub materials: Vec<MeshMaterial>,
    pub bounds: MeshBounds,
    pub armature: Option<Armature>,
    pub animations: Vec<AnimationClip>,
}

impl MeshData {
    pub(crate) fn new(
        name: impl Into<String>,
        vertices: Vec<Vertex>,
        indices: Vec<u32>,
        submeshes: Vec<Submesh>,
        materials: Vec<MeshMaterial>,
        recompute_normals: bool,
    ) -> MeshResult<Self> {
        let mut mesh = Self {
            name: name.into(),
            vertices,
            indices,
            submeshes,
            materials,
            bounds: MeshBounds::default(),
            armature: None,
            animations: Vec::new(),
        };
        mesh.finish_mutation(recompute_normals)?;
        Ok(mesh)
    }

    /// Validate topology, attributes, metadata, and the cached bounds.
    pub(crate) fn validate(&self) -> MeshResult<()> {
        self.validate_core()?;
        let expected = calculate_bounds(&self.vertices)?;
        if self.bounds != expected {
            return Err(MeshError::InvalidData(
                "mesh bounds are stale; mutate through MeshHandle".into(),
            ));
        }
        Ok(())
    }

    /// Recompute area-weighted smooth normals and refresh bounds.
    pub(crate) fn recompute_normals(&mut self) -> MeshResult<()> {
        self.finish_mutation(true)
    }

    fn finish_mutation(&mut self, recompute_normals: bool) -> MeshResult<()> {
        if self.indices.is_empty() {
            self.submeshes.clear();
        } else if self.submeshes.is_empty() {
            self.submeshes.push(Submesh {
                name: self.name.clone(),
                first_index: 0,
                index_count: usize_to_u32(self.indices.len(), "index count")?,
                material: None,
            });
        }
        self.validate_core()?;
        if recompute_normals {
            recompute_normals_unchecked(&mut self.vertices, &self.indices);
        }
        self.bounds = calculate_bounds(&self.vertices)?;
        self.validate_core()
    }

    fn validate_core(&self) -> MeshResult<()> {
        if self.vertices.is_empty() && !self.indices.is_empty() {
            return Err(MeshError::InvalidData(
                "mesh has indices but no vertices".into(),
            ));
        }
        if self.indices.len() % 3 != 0 {
            return Err(MeshError::InvalidData(format!(
                "index count {} is not divisible by three",
                self.indices.len()
            )));
        }

        for (index, vertex) in self.vertices.iter().enumerate() {
            if !all_finite(&vertex.position)
                || !all_finite(&vertex.normal)
                || !all_finite(&vertex.uv)
                || !all_finite(&vertex.tangent)
            {
                return Err(MeshError::InvalidData(format!(
                    "vertex {index} contains a non-finite attribute"
                )));
            }
        }
        for (position, index) in self.indices.iter().copied().enumerate() {
            let index = usize::try_from(index).map_err(|_| {
                MeshError::InvalidData(format!("index {position} cannot fit in memory"))
            })?;
            if index >= self.vertices.len() {
                return Err(MeshError::InvalidData(format!(
                    "index {position} references vertex {index}, but only {} vertices exist",
                    self.vertices.len()
                )));
            }
        }

        let mut covered = 0usize;
        for (index, submesh) in self.submeshes.iter().enumerate() {
            let first = usize::try_from(submesh.first_index).map_err(|_| {
                MeshError::InvalidData(format!("submesh {index} offset cannot fit in memory"))
            })?;
            let count = usize::try_from(submesh.index_count).map_err(|_| {
                MeshError::InvalidData(format!("submesh {index} count cannot fit in memory"))
            })?;
            if first != covered {
                return Err(MeshError::InvalidData(format!(
                    "submesh {index} starts at {first}, expected contiguous offset {covered}"
                )));
            }
            if count == 0 || first % 3 != 0 || count % 3 != 0 {
                return Err(MeshError::InvalidData(format!(
                    "submesh {index} is not a non-empty triangle range"
                )));
            }
            covered = first.checked_add(count).ok_or_else(|| {
                MeshError::InvalidData(format!("submesh {index} range overflows"))
            })?;
            if covered > self.indices.len() {
                return Err(MeshError::InvalidData(format!(
                    "submesh {index} exceeds the mesh index buffer"
                )));
            }
            if let Some(material) = submesh.material
                && material >= self.materials.len()
            {
                return Err(MeshError::InvalidData(format!(
                    "submesh {index} references missing material {material}"
                )));
            }
        }
        if covered != self.indices.len() {
            return Err(MeshError::InvalidData(format!(
                "submeshes cover {covered} of {} indices",
                self.indices.len()
            )));
        }

        for (index, material) in self.materials.iter().enumerate() {
            if !all_finite(&material.base_color)
                || !material.metallic.is_finite()
                || !material.roughness.is_finite()
                || !all_finite(&material.emissive)
                || !material.alpha_cutoff.is_finite()
            {
                return Err(MeshError::InvalidData(format!(
                    "material {index} contains a non-finite value"
                )));
            }
        }
        self.validate_animation_data()?;
        Ok(())
    }

    fn validate_animation_data(&self) -> MeshResult<()> {
        let Some(armature) = &self.armature else {
            if !self.animations.is_empty() {
                return Err(MeshError::InvalidData(
                    "mesh has animation clips but no armature".into(),
                ));
            }
            return Ok(());
        };
        if armature.vertex_weights.len() != self.vertices.len()
            || armature.bind_vertices.len() != self.vertices.len()
        {
            return Err(MeshError::InvalidData(
                "armature weights and bind vertices must match the mesh vertex count".into(),
            ));
        }
        if armature.joints.len() != armature.inverse_bind_matrices.len() {
            return Err(MeshError::InvalidData(
                "armature joint and inverse-bind counts differ".into(),
            ));
        }
        if armature.joints.len() > usize::from(u16::MAX) + 1 {
            return Err(MeshError::InvalidData(
                "armature exceeds the u16 skin palette limit".into(),
            ));
        }
        for (index, node) in armature.nodes.iter().enumerate() {
            if node
                .parent
                .is_some_and(|parent| parent >= armature.nodes.len() || parent == index)
                || !all_finite(&node.translation)
                || !all_finite(&node.rotation)
                || !all_finite(&node.scale)
            {
                return Err(MeshError::InvalidData(format!(
                    "armature node {index} has an invalid transform or parent"
                )));
            }
            let rotation_length = node.rotation.iter().map(|value| value * value).sum::<f32>();
            if rotation_length <= f32::EPSILON {
                return Err(MeshError::InvalidData(format!(
                    "armature node {index} has a zero quaternion"
                )));
            }
        }
        validate_armature_hierarchy(&armature.nodes)?;
        for (palette_index, node) in armature.joints.iter().copied().enumerate() {
            if node >= armature.nodes.len()
                || !all_finite(&armature.inverse_bind_matrices[palette_index])
            {
                return Err(MeshError::InvalidData(format!(
                    "armature palette entry {palette_index} is invalid"
                )));
            }
        }
        for (vertex_index, influences) in armature.vertex_weights.iter().enumerate() {
            let mut sum = 0.0;
            for influence in 0..4 {
                let weight = influences.weights[influence];
                if !weight.is_finite() || weight < 0.0 {
                    return Err(MeshError::InvalidData(format!(
                        "vertex {vertex_index} has an invalid skin weight"
                    )));
                }
                if weight > 0.0
                    && usize::from(influences.joints[influence]) >= armature.joints.len()
                {
                    return Err(MeshError::InvalidData(format!(
                        "vertex {vertex_index} references a missing skin joint"
                    )));
                }
                sum += weight;
            }
            if sum > 0.0 && (sum - 1.0).abs() > 0.002 {
                return Err(MeshError::InvalidData(format!(
                    "vertex {vertex_index} skin weights are not normalized"
                )));
            }
        }
        for (clip_index, clip) in self.animations.iter().enumerate() {
            if clip.name.is_empty() || !clip.duration.is_finite() || clip.duration < 0.0 {
                return Err(MeshError::InvalidData(format!(
                    "animation clip {clip_index} has invalid metadata"
                )));
            }
            let mut targets = std::collections::HashSet::new();
            for (channel_index, channel) in clip.channels.iter().enumerate() {
                if channel.node >= armature.nodes.len()
                    || channel.times.is_empty()
                    || channel.times.len() != channel.values.len()
                    || !targets.insert((channel.node, channel.property))
                {
                    return Err(MeshError::InvalidData(format!(
                        "animation clip {clip_index} channel {channel_index} is invalid"
                    )));
                }
                let mut previous = None;
                for (key, time) in channel.times.iter().copied().enumerate() {
                    if !time.is_finite()
                        || time < 0.0
                        || previous.is_some_and(|value| time <= value)
                    {
                        return Err(MeshError::InvalidData(format!(
                            "animation clip {clip_index} channel {channel_index} key {key} has invalid time"
                        )));
                    }
                    if !all_finite(&channel.values[key]) {
                        return Err(MeshError::InvalidData(format!(
                            "animation clip {clip_index} channel {channel_index} key {key} is non-finite"
                        )));
                    }
                    previous = Some(time);
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct MeshSnapshot {
    pub revision: u64,
    /// Cheap immutable snapshot; cloning it does not clone vertex/index buffers.
    pub mesh: Arc<MeshData>,
}

#[derive(Debug)]
struct VersionedMesh {
    revision: u64,
    mesh: Arc<MeshData>,
    playback: Option<AnimationPlayback>,
}

#[derive(Clone, Debug)]
struct AnimationPlayback {
    clip: usize,
    time: f32,
    speed: f32,
    looped: bool,
    playing: bool,
}

/// Shared mesh identity with atomic, validated live-edit transactions.
#[derive(Clone, Debug)]
pub(crate) struct MeshHandle {
    inner: Arc<RwLock<VersionedMesh>>,
}

impl MeshHandle {
    pub(crate) fn new(mut mesh: MeshData) -> MeshResult<Self> {
        mesh.finish_mutation(false)?;
        Ok(Self {
            inner: Arc::new(RwLock::new(VersionedMesh {
                revision: 0,
                mesh: Arc::new(mesh),
                playback: None,
            })),
        })
    }

    /// Stable process-local identity for renderer caches.
    pub(crate) fn identity(&self) -> usize {
        Arc::as_ptr(&self.inner) as usize
    }

    pub(crate) fn revision(&self) -> MeshResult<u64> {
        let asset = self.inner.read().map_err(|_| MeshError::LockPoisoned)?;
        Ok(asset.revision)
    }

    pub(crate) fn snapshot(&self) -> MeshResult<MeshSnapshot> {
        let asset = self.inner.read().map_err(|_| MeshError::LockPoisoned)?;
        Ok(MeshSnapshot {
            revision: asset.revision,
            mesh: Arc::clone(&asset.mesh),
        })
    }

    /// Read without cloning, suitable for renderer-side upload/cache checks.
    pub(crate) fn with_read<T>(&self, read: impl FnOnce(&MeshData, u64) -> T) -> MeshResult<T> {
        let asset = self.inner.read().map_err(|_| MeshError::LockPoisoned)?;
        Ok(read(asset.mesh.as_ref(), asset.revision))
    }

    /// Apply a transaction to a cloned candidate and commit only if valid.
    pub(crate) fn mutate(
        &self,
        edit: impl FnOnce(&mut MeshData) -> MeshResult<()>,
    ) -> MeshResult<u64> {
        self.mutate_internal(false, edit)
    }

    /// Apply a transaction and rebuild normals from the edited triangle data.
    pub(crate) fn mutate_recomputing_normals(
        &self,
        edit: impl FnOnce(&mut MeshData) -> MeshResult<()>,
    ) -> MeshResult<u64> {
        self.mutate_internal(true, edit)
    }

    pub(crate) fn set_vertex(
        &self,
        index: usize,
        vertex: Vertex,
        recompute_normals: bool,
    ) -> MeshResult<u64> {
        self.mutate_internal(recompute_normals, |mesh| {
            let destination = mesh
                .vertices
                .get_mut(index)
                .ok_or_else(|| MeshError::InvalidData(format!("vertex {index} does not exist")))?;
            *destination = vertex;
            Ok(())
        })
    }

    pub(crate) fn replace_geometry(
        &self,
        vertices: Vec<Vertex>,
        indices: Vec<u32>,
    ) -> MeshResult<u64> {
        self.mutate_internal(true, move |mesh| {
            mesh.vertices = vertices;
            mesh.indices = indices;
            mesh.submeshes.clear();
            mesh.armature = None;
            mesh.animations.clear();
            Ok(())
        })
    }

    pub(crate) fn recompute_normals(&self) -> MeshResult<u64> {
        self.mutate_internal(true, |_| Ok(()))
    }

    /// Create an independent mesh identity, useful when several entities play
    /// different poses from one cached source asset.
    pub(crate) fn detached_clone(&self) -> MeshResult<Self> {
        let snapshot = self.snapshot()?;
        Self::new(snapshot.mesh.as_ref().clone())
    }

    pub(crate) fn animation_names(&self) -> MeshResult<Vec<String>> {
        self.with_read(|mesh, _| {
            mesh.animations
                .iter()
                .map(|clip| clip.name.clone())
                .collect()
        })
    }

    pub(crate) fn animation_duration(&self, name: &str) -> MeshResult<Option<f32>> {
        self.with_read(|mesh, _| {
            mesh.animations
                .iter()
                .find(|clip| clip.name == name)
                .map(|clip| clip.duration)
        })
    }

    /// Immediately deform bind-pose vertices at a clip time.
    pub(crate) fn sample_animation(&self, name: &str, time: f32, looped: bool) -> MeshResult<u64> {
        if !time.is_finite() {
            return Err(MeshError::InvalidData(
                "animation sample time must be finite".into(),
            ));
        }
        self.mutate_internal(false, |mesh| {
            let clip = mesh
                .animations
                .iter()
                .find(|clip| clip.name == name)
                .cloned()
                .ok_or_else(|| {
                    MeshError::InvalidData(format!("animation '{name}' does not exist"))
                })?;
            apply_animation_pose(
                mesh,
                &clip,
                normalized_clip_time(time, clip.duration, looped),
            )
        })
    }

    /// Set playback state. Call [`advance_animation`] once per game update;
    /// deformation is committed atomically and observed by all renderers.
    pub(crate) fn play_animation(&self, name: &str, looped: bool, speed: f32) -> MeshResult<()> {
        if !speed.is_finite() {
            return Err(MeshError::InvalidData(
                "animation playback speed must be finite".into(),
            ));
        }
        let mut asset = self.inner.write().map_err(|_| MeshError::LockPoisoned)?;
        let clip = asset
            .mesh
            .animations
            .iter()
            .position(|clip| clip.name == name)
            .ok_or_else(|| MeshError::InvalidData(format!("animation '{name}' does not exist")))?;
        asset.playback = Some(AnimationPlayback {
            clip,
            time: 0.0,
            speed,
            looped,
            playing: true,
        });
        Ok(())
    }

    pub(crate) fn set_animation_paused(&self, paused: bool) -> MeshResult<()> {
        let mut asset = self.inner.write().map_err(|_| MeshError::LockPoisoned)?;
        let playback = asset.playback.as_mut().ok_or_else(|| {
            MeshError::InvalidData("mesh has no active animation playback".into())
        })?;
        playback.playing = !paused;
        Ok(())
    }

    pub(crate) fn stop_animation(&self) -> MeshResult<()> {
        let mut asset = self.inner.write().map_err(|_| MeshError::LockPoisoned)?;
        asset.playback = None;
        let bind_vertices = asset
            .mesh
            .armature
            .as_ref()
            .map(|armature| armature.bind_vertices.clone());
        if let Some(bind_vertices) = bind_vertices {
            let mesh = Arc::make_mut(&mut asset.mesh);
            mesh.vertices = bind_vertices;
            mesh.bounds = calculate_bounds(&mesh.vertices)?;
            asset.revision = asset.revision.wrapping_add(1);
        }
        Ok(())
    }

    pub(crate) fn advance_animation(&self, delta_seconds: f32) -> MeshResult<bool> {
        if !delta_seconds.is_finite() || delta_seconds < 0.0 {
            return Err(MeshError::InvalidData(
                "animation delta must be a finite non-negative number".into(),
            ));
        }
        let mut asset = self.inner.write().map_err(|_| MeshError::LockPoisoned)?;
        let Some(mut playback) = asset.playback.clone() else {
            return Ok(false);
        };
        if !playback.playing {
            return Ok(false);
        }
        let clip = asset
            .mesh
            .animations
            .get(playback.clip)
            .cloned()
            .ok_or_else(|| MeshError::InvalidData("active animation clip disappeared".into()))?;
        playback.time += delta_seconds * playback.speed;
        let sample_time = normalized_clip_time(playback.time, clip.duration, playback.looped);
        if !playback.looped && clip.duration > 0.0 {
            if playback.speed >= 0.0 && playback.time >= clip.duration {
                playback.time = clip.duration;
                playback.playing = false;
            } else if playback.speed < 0.0 && playback.time <= 0.0 {
                playback.time = 0.0;
                playback.playing = false;
            }
        }
        let mesh = Arc::make_mut(&mut asset.mesh);
        apply_animation_pose(mesh, &clip, sample_time)?;
        asset.playback = Some(playback);
        asset.revision = asset.revision.wrapping_add(1);
        Ok(true)
    }

    fn mutate_internal(
        &self,
        recompute_normals: bool,
        edit: impl FnOnce(&mut MeshData) -> MeshResult<()>,
    ) -> MeshResult<u64> {
        let mut asset = self.inner.write().map_err(|_| MeshError::LockPoisoned)?;
        let mut candidate = asset.mesh.as_ref().clone();
        edit(&mut candidate)?;
        candidate.finish_mutation(recompute_normals)?;
        asset.mesh = Arc::new(candidate);
        asset.revision = asset.revision.wrapping_add(1);
        Ok(asset.revision)
    }
}

/// Errors are contextual and safe to forward to script/editor diagnostics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MeshError {
    Io(String),
    UnsupportedFormat(String),
    InvalidData(String),
    LockPoisoned,
}

impl fmt::Display for MeshError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(message) => write!(formatter, "mesh I/O error: {message}"),
            Self::UnsupportedFormat(message) => write!(formatter, "unsupported mesh: {message}"),
            Self::InvalidData(message) => write!(formatter, "invalid mesh: {message}"),
            Self::LockPoisoned => formatter.write_str("mesh lock is poisoned"),
        }
    }
}

impl std::error::Error for MeshError {}

impl From<std::io::Error> for MeshError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<serde_json::Error> for MeshError {
    fn from(error: serde_json::Error) -> Self {
        Self::InvalidData(format!("invalid glTF JSON: {error}"))
    }
}

/// Import a model and resolve external glTF buffers beside the model file.
pub(crate) fn import_from_path(path: impl AsRef<Path>) -> MeshResult<MeshHandle> {
    let path = path.as_ref();
    let format = MeshFormat::from_path(path)?;
    let bytes = fs::read(path)
        .map_err(|error| MeshError::Io(format!("failed to read '{}': {error}", path.display())))?;
    let base_dir = path.parent();
    let mut mesh = parse_mesh_bytes(&bytes, format, base_dir)?;
    if let Some(name) = path.file_stem().and_then(|value| value.to_str()) {
        mesh.name = name.to_string();
    }
    MeshHandle::new(mesh)
}

/// Import an in-memory model. External glTF buffer URIs require
/// [`import_from_path`] and are rejected here.
pub(crate) fn import_from_bytes(bytes: &[u8], format: MeshFormat) -> MeshResult<MeshHandle> {
    MeshHandle::new(parse_mesh_bytes(bytes, format, None)?)
}

fn parse_mesh_bytes(
    bytes: &[u8],
    format: MeshFormat,
    base_dir: Option<&Path>,
) -> MeshResult<MeshData> {
    match format {
        MeshFormat::Obj => parse_obj(bytes),
        MeshFormat::Gltf => parse_gltf_json(bytes, base_dir, None),
        MeshFormat::Glb => parse_glb(bytes, base_dir),
        MeshFormat::Fbx => parse_fbx(bytes),
    }
}

fn calculate_bounds(vertices: &[Vertex]) -> MeshResult<MeshBounds> {
    let Some(first) = vertices.first() else {
        return Ok(MeshBounds::default());
    };
    if !all_finite(&first.position) {
        return Err(MeshError::InvalidData(
            "first vertex position is non-finite".into(),
        ));
    }
    let mut min = first.position;
    let mut max = first.position;
    for vertex in &vertices[1..] {
        if !all_finite(&vertex.position) {
            return Err(MeshError::InvalidData(
                "vertex position is non-finite".into(),
            ));
        }
        for axis in 0..3 {
            min[axis] = min[axis].min(vertex.position[axis]);
            max[axis] = max[axis].max(vertex.position[axis]);
        }
    }
    let center = [
        min[0] * 0.5 + max[0] * 0.5,
        min[1] * 0.5 + max[1] * 0.5,
        min[2] * 0.5 + max[2] * 0.5,
    ];
    let mut radius_squared = 0.0f64;
    for vertex in vertices {
        let dx = f64::from(vertex.position[0]) - f64::from(center[0]);
        let dy = f64::from(vertex.position[1]) - f64::from(center[1]);
        let dz = f64::from(vertex.position[2]) - f64::from(center[2]);
        radius_squared = radius_squared.max(dx * dx + dy * dy + dz * dz);
    }
    let radius = radius_squared.sqrt() as f32;
    if !all_finite(&center) || !radius.is_finite() {
        return Err(MeshError::InvalidData(
            "mesh positions produce non-finite bounds".into(),
        ));
    }
    Ok(MeshBounds {
        min,
        max,
        center,
        radius,
    })
}

fn recompute_normals_unchecked(vertices: &mut [Vertex], indices: &[u32]) {
    for vertex in vertices.iter_mut() {
        vertex.normal = [0.0; 3];
    }
    for triangle in indices.chunks_exact(3) {
        let Some(a) = usize::try_from(triangle[0]).ok() else {
            continue;
        };
        let Some(b) = usize::try_from(triangle[1]).ok() else {
            continue;
        };
        let Some(c) = usize::try_from(triangle[2]).ok() else {
            continue;
        };
        let (Some(pa), Some(pb), Some(pc)) = (
            vertices.get(a).map(|vertex| vertex.position),
            vertices.get(b).map(|vertex| vertex.position),
            vertices.get(c).map(|vertex| vertex.position),
        ) else {
            continue;
        };
        let ab = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let ac = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
        let normal = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        for index in [a, b, c] {
            if let Some(vertex) = vertices.get_mut(index) {
                for axis in 0..3 {
                    vertex.normal[axis] += normal[axis];
                }
            }
        }
    }
    for vertex in vertices {
        let length = (vertex.normal[0] * vertex.normal[0]
            + vertex.normal[1] * vertex.normal[1]
            + vertex.normal[2] * vertex.normal[2])
            .sqrt();
        if length > f32::EPSILON {
            for axis in 0..3 {
                vertex.normal[axis] /= length;
            }
        } else {
            vertex.normal = [0.0, 1.0, 0.0];
        }
    }
}

fn all_finite<const N: usize>(values: &[f32; N]) -> bool {
    values.iter().all(|value| value.is_finite())
}

fn usize_to_u32(value: usize, label: &str) -> MeshResult<u32> {
    u32::try_from(value)
        .map_err(|_| MeshError::InvalidData(format!("{label} exceeds the u32 GPU index limit")))
}

const MAX_ARMATURE_NODES: usize = 4096;
const MAX_ARMATURE_DEPTH: usize = 256;

fn validate_armature_hierarchy(nodes: &[ArmatureNode]) -> MeshResult<()> {
    if nodes.len() > MAX_ARMATURE_NODES {
        return Err(MeshError::InvalidData(format!(
            "armature has {} nodes; limit is {MAX_ARMATURE_NODES}",
            nodes.len()
        )));
    }
    for start in 0..nodes.len() {
        let mut current = Some(start);
        let mut depth = 0usize;
        while let Some(index) = current {
            depth += 1;
            if depth > nodes.len().min(MAX_ARMATURE_DEPTH) {
                return Err(MeshError::InvalidData(format!(
                    "armature hierarchy at node {start} is cyclic or deeper than {MAX_ARMATURE_DEPTH}"
                )));
            }
            current = nodes[index].parent;
        }
    }
    Ok(())
}

fn normalized_clip_time(time: f32, duration: f32, looped: bool) -> f32 {
    if duration <= f32::EPSILON {
        0.0
    } else if looped {
        time.rem_euclid(duration)
    } else {
        time.clamp(0.0, duration)
    }
}

fn apply_animation_pose(mesh: &mut MeshData, clip: &AnimationClip, time: f32) -> MeshResult<()> {
    let armature = mesh
        .armature
        .as_ref()
        .ok_or_else(|| MeshError::InvalidData("animation requires an armature".into()))?;
    let mut nodes = armature.nodes.clone();
    for channel in &clip.channels {
        let value = sample_animation_channel(channel, time);
        let node = nodes.get_mut(channel.node).ok_or_else(|| {
            MeshError::InvalidData("animation channel references a missing node".into())
        })?;
        match channel.property {
            AnimationProperty::Translation => node.translation = [value[0], value[1], value[2]],
            AnimationProperty::Rotation => node.rotation = normalize_quaternion(value),
            AnimationProperty::Scale => node.scale = [value[0], value[1], value[2]],
        }
    }

    let local = nodes
        .iter()
        .map(|node| transform_matrix(node.translation, node.rotation, node.scale))
        .collect::<Vec<_>>();
    let mut globals = vec![None; nodes.len()];
    let mut visiting = vec![false; nodes.len()];
    for node in 0..nodes.len() {
        resolve_global_matrix(node, &nodes, &local, &mut globals, &mut visiting, 0)?;
    }
    let palette = armature
        .joints
        .iter()
        .copied()
        .zip(armature.inverse_bind_matrices.iter().copied())
        .map(|(node, inverse_bind)| {
            globals
                .get(node)
                .and_then(|matrix| *matrix)
                .map(|global| matrix_multiply(global, inverse_bind))
                .ok_or_else(|| MeshError::InvalidData("armature palette node is missing".into()))
        })
        .collect::<MeshResult<Vec<_>>>()?;

    let mut vertices = armature.bind_vertices.clone();
    for (vertex_index, destination) in vertices.iter_mut().enumerate() {
        let bind = armature.bind_vertices[vertex_index];
        let influences = armature.vertex_weights[vertex_index];
        if influences.weights.iter().sum::<f32>() <= f32::EPSILON {
            continue;
        }
        let mut position = [0.0; 3];
        let mut normal = [0.0; 3];
        let mut tangent = [0.0; 3];
        for influence in 0..4 {
            let weight = influences.weights[influence];
            if weight <= 0.0 {
                continue;
            }
            let matrix = palette
                .get(usize::from(influences.joints[influence]))
                .ok_or_else(|| MeshError::InvalidData("skin joint is out of range".into()))?;
            add_scaled3(
                &mut position,
                transform_matrix_point(*matrix, bind.position),
                weight,
            );
            add_scaled3(
                &mut normal,
                transform_matrix_vector(*matrix, bind.normal),
                weight,
            );
            add_scaled3(
                &mut tangent,
                transform_matrix_vector(
                    *matrix,
                    [bind.tangent[0], bind.tangent[1], bind.tangent[2]],
                ),
                weight,
            );
        }
        destination.position = position;
        destination.normal = normalize_vector(normal, [0.0, 1.0, 0.0]);
        let tangent = normalize_vector(tangent, [1.0, 0.0, 0.0]);
        destination.tangent = [tangent[0], tangent[1], tangent[2], bind.tangent[3]];
    }
    let bounds = calculate_bounds(&vertices)?;
    mesh.vertices = vertices;
    mesh.bounds = bounds;
    Ok(())
}

fn sample_animation_channel(channel: &AnimationChannel, time: f32) -> [f32; 4] {
    if time <= channel.times[0] || channel.times.len() == 1 {
        return channel.values[0];
    }
    let last = channel.times.len() - 1;
    if time >= channel.times[last] {
        return channel.values[last];
    }
    let right = channel
        .times
        .partition_point(|candidate| *candidate <= time);
    let left = right - 1;
    if channel.interpolation == AnimationInterpolation::Step {
        return channel.values[left];
    }
    let amount = ((time - channel.times[left]) / (channel.times[right] - channel.times[left]))
        .clamp(0.0, 1.0);
    if channel.property == AnimationProperty::Rotation {
        quaternion_nlerp(channel.values[left], channel.values[right], amount)
    } else {
        std::array::from_fn(|component| {
            channel.values[left][component]
                + (channel.values[right][component] - channel.values[left][component]) * amount
        })
    }
}

fn quaternion_nlerp(mut from: [f32; 4], mut to: [f32; 4], amount: f32) -> [f32; 4] {
    from = normalize_quaternion(from);
    to = normalize_quaternion(to);
    if from.iter().zip(to).map(|(a, b)| a * b).sum::<f32>() < 0.0 {
        to = to.map(|value| -value);
    }
    normalize_quaternion(std::array::from_fn(|index| {
        from[index] + (to[index] - from[index]) * amount
    }))
}

fn normalize_quaternion(value: [f32; 4]) -> [f32; 4] {
    let length = value
        .iter()
        .map(|component| component * component)
        .sum::<f32>()
        .sqrt();
    if length <= f32::EPSILON {
        [0.0, 0.0, 0.0, 1.0]
    } else {
        value.map(|component| component / length)
    }
}

fn normalize_vector(value: [f32; 3], fallback: [f32; 3]) -> [f32; 3] {
    let length = value
        .iter()
        .map(|component| component * component)
        .sum::<f32>()
        .sqrt();
    if length <= f32::EPSILON {
        fallback
    } else {
        value.map(|component| component / length)
    }
}

fn add_scaled3(destination: &mut [f32; 3], value: [f32; 3], scale: f32) {
    for axis in 0..3 {
        destination[axis] += value[axis] * scale;
    }
}

fn resolve_global_matrix(
    node: usize,
    nodes: &[ArmatureNode],
    local: &[[f32; 16]],
    globals: &mut [Option<[f32; 16]>],
    visiting: &mut [bool],
    depth: usize,
) -> MeshResult<[f32; 16]> {
    if let Some(matrix) = globals[node] {
        return Ok(matrix);
    }
    if depth >= MAX_ARMATURE_DEPTH || visiting[node] {
        return Err(MeshError::InvalidData(
            "armature hierarchy is cyclic or too deep".into(),
        ));
    }
    visiting[node] = true;
    let matrix = if let Some(parent) = nodes[node].parent {
        matrix_multiply(
            resolve_global_matrix(parent, nodes, local, globals, visiting, depth + 1)?,
            local[node],
        )
    } else {
        local[node]
    };
    visiting[node] = false;
    globals[node] = Some(matrix);
    Ok(matrix)
}

fn identity_matrix() -> [f32; 16] {
    [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
}

fn transform_matrix(translation: [f32; 3], rotation: [f32; 4], scale: [f32; 3]) -> [f32; 16] {
    let [x, y, z, w] = normalize_quaternion(rotation);
    let x2 = x + x;
    let y2 = y + y;
    let z2 = z + z;
    let xx = x * x2;
    let xy = x * y2;
    let xz = x * z2;
    let yy = y * y2;
    let yz = y * z2;
    let zz = z * z2;
    let wx = w * x2;
    let wy = w * y2;
    let wz = w * z2;
    [
        (1.0 - (yy + zz)) * scale[0],
        (xy + wz) * scale[0],
        (xz - wy) * scale[0],
        0.0,
        (xy - wz) * scale[1],
        (1.0 - (xx + zz)) * scale[1],
        (yz + wx) * scale[1],
        0.0,
        (xz + wy) * scale[2],
        (yz - wx) * scale[2],
        (1.0 - (xx + yy)) * scale[2],
        0.0,
        translation[0],
        translation[1],
        translation[2],
        1.0,
    ]
}

fn matrix_multiply(left: [f32; 16], right: [f32; 16]) -> [f32; 16] {
    let mut output = [0.0; 16];
    for column in 0..4 {
        for row in 0..4 {
            output[column * 4 + row] = (0..4)
                .map(|inner| left[inner * 4 + row] * right[column * 4 + inner])
                .sum();
        }
    }
    output
}

fn transform_matrix_point(matrix: [f32; 16], point: [f32; 3]) -> [f32; 3] {
    [
        matrix[0] * point[0] + matrix[4] * point[1] + matrix[8] * point[2] + matrix[12],
        matrix[1] * point[0] + matrix[5] * point[1] + matrix[9] * point[2] + matrix[13],
        matrix[2] * point[0] + matrix[6] * point[1] + matrix[10] * point[2] + matrix[14],
    ]
}

fn transform_matrix_vector(matrix: [f32; 16], vector: [f32; 3]) -> [f32; 3] {
    [
        matrix[0] * vector[0] + matrix[4] * vector[1] + matrix[8] * vector[2],
        matrix[1] * vector[0] + matrix[5] * vector[1] + matrix[9] * vector[2],
        matrix[2] * vector[0] + matrix[6] * vector[1] + matrix[10] * vector[2],
    ]
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PrimitiveOptions {
    pub size: [f32; 3],
    pub radius: f32,
    pub height: f32,
    pub segments: u32,
    pub rings: u32,
}

impl Default for PrimitiveOptions {
    fn default() -> Self {
        Self {
            size: [1.0; 3],
            radius: 0.5,
            height: 1.0,
            segments: 24,
            rings: 12,
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum PrimitiveKind {
    Cube,
    Plane,
    Sphere,
    Cylinder,
    Capsule,
    Cone,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct PrimitiveKey {
    kind: PrimitiveKind,
    size: [u32; 3],
    radius: u32,
    height: u32,
    segments: u32,
    rings: u32,
}

static PRIMITIVE_CACHE: OnceLock<RwLock<HashMap<PrimitiveKey, MeshHandle>>> = OnceLock::new();

pub(crate) fn primitive_mesh(name: &str, options: PrimitiveOptions) -> MeshResult<MeshHandle> {
    let normalized = name
        .trim()
        .to_ascii_lowercase()
        .replace(['-', '_', ' '], "");
    let kind = match normalized.as_str() {
        "cube" | "box" => PrimitiveKind::Cube,
        "plane" | "quad" => PrimitiveKind::Plane,
        "sphere" | "uvsphere" => PrimitiveKind::Sphere,
        "cylinder" => PrimitiveKind::Cylinder,
        "capsule" => PrimitiveKind::Capsule,
        "cone" => PrimitiveKind::Cone,
        _ => {
            return Err(MeshError::InvalidData(format!(
                "unknown primitive mesh '{name}'; expected cube, plane, sphere, cylinder, capsule, or cone"
            )));
        }
    };
    validate_primitive_options(kind, options)?;
    let key = PrimitiveKey {
        kind,
        size: options.size.map(f32::to_bits),
        radius: options.radius.to_bits(),
        height: options.height.to_bits(),
        segments: options.segments,
        rings: options.rings,
    };
    let cache = PRIMITIVE_CACHE.get_or_init(|| RwLock::new(HashMap::new()));
    if let Some(mesh) = cache.read().map_err(|_| MeshError::LockPoisoned)?.get(&key) {
        return Ok(mesh.clone());
    }
    let data = match kind {
        PrimitiveKind::Cube => make_cube(options.size)?,
        PrimitiveKind::Plane => make_plane(
            options.size[0],
            options.size[2],
            options.segments,
            options.rings,
        )?,
        PrimitiveKind::Sphere => make_uv_sphere(options.radius, options.segments, options.rings)?,
        PrimitiveKind::Cylinder => make_cylinder(options.radius, options.height, options.segments)?,
        PrimitiveKind::Capsule => make_capsule(
            options.radius,
            options.height,
            options.segments,
            options.rings,
        )?,
        PrimitiveKind::Cone => make_cone(options.radius, options.height, options.segments)?,
    };
    let handle = MeshHandle::new(data)?;
    let mut cache = cache.write().map_err(|_| MeshError::LockPoisoned)?;
    Ok(cache.entry(key).or_insert_with(|| handle.clone()).clone())
}

fn validate_primitive_options(kind: PrimitiveKind, options: PrimitiveOptions) -> MeshResult<()> {
    if !all_finite(&options.size)
        || !options.radius.is_finite()
        || !options.height.is_finite()
        || options.size.iter().any(|value| *value <= 0.0)
        || options.radius <= 0.0
        || options.height <= 0.0
    {
        return Err(MeshError::InvalidData(
            "primitive dimensions must be finite positive numbers".into(),
        ));
    }
    if options.segments < 3 || options.segments > 1024 || options.rings < 1 || options.rings > 512 {
        return Err(MeshError::InvalidData(
            "primitive segments must be 3..1024 and rings must be 1..512".into(),
        ));
    }
    if kind == PrimitiveKind::Sphere && options.rings < 2 {
        return Err(MeshError::InvalidData(
            "sphere requires at least two latitude rings".into(),
        ));
    }
    if kind == PrimitiveKind::Capsule && options.height < options.radius * 2.0 {
        return Err(MeshError::InvalidData(
            "capsule height must be at least twice its radius".into(),
        ));
    }
    Ok(())
}

fn make_vertex(position: [f32; 3], normal: [f32; 3], uv: [f32; 2]) -> Vertex {
    Vertex {
        position,
        normal,
        uv,
        tangent: [1.0, 0.0, 0.0, 1.0],
    }
}

fn make_cube(size: [f32; 3]) -> MeshResult<MeshData> {
    let [x, y, z] = size.map(|value| value * 0.5);
    let faces = [
        (
            [0.0, 0.0, 1.0],
            [[-x, -y, z], [x, -y, z], [x, y, z], [-x, y, z]],
        ),
        (
            [0.0, 0.0, -1.0],
            [[x, -y, -z], [-x, -y, -z], [-x, y, -z], [x, y, -z]],
        ),
        (
            [1.0, 0.0, 0.0],
            [[x, -y, z], [x, -y, -z], [x, y, -z], [x, y, z]],
        ),
        (
            [-1.0, 0.0, 0.0],
            [[-x, -y, -z], [-x, -y, z], [-x, y, z], [-x, y, -z]],
        ),
        (
            [0.0, 1.0, 0.0],
            [[-x, y, z], [x, y, z], [x, y, -z], [-x, y, -z]],
        ),
        (
            [0.0, -1.0, 0.0],
            [[-x, -y, -z], [x, -y, -z], [x, -y, z], [-x, -y, z]],
        ),
    ];
    let uvs = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
    let mut vertices = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    for (normal, positions) in faces {
        let base = usize_to_u32(vertices.len(), "cube vertex offset")?;
        vertices.extend((0..4).map(|index| make_vertex(positions[index], normal, uvs[index])));
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    MeshData::new("Cube", vertices, indices, Vec::new(), Vec::new(), false)
}

fn make_plane(width: f32, depth: f32, x_segments: u32, z_segments: u32) -> MeshResult<MeshData> {
    let mut vertices = Vec::with_capacity((x_segments as usize + 1) * (z_segments as usize + 1));
    for z in 0..=z_segments {
        let v = z as f32 / z_segments as f32;
        for x in 0..=x_segments {
            let u = x as f32 / x_segments as f32;
            vertices.push(make_vertex(
                [(u - 0.5) * width, 0.0, (v - 0.5) * depth],
                [0.0, 1.0, 0.0],
                [u, 1.0 - v],
            ));
        }
    }
    let row = x_segments + 1;
    let mut indices = Vec::with_capacity(x_segments as usize * z_segments as usize * 6);
    for z in 0..z_segments {
        for x in 0..x_segments {
            let a = z * row + x;
            let b = a + 1;
            let d = a + row;
            let c = d + 1;
            indices.extend([a, d, c, a, c, b]);
        }
    }
    MeshData::new("Plane", vertices, indices, Vec::new(), Vec::new(), false)
}

fn make_uv_sphere(radius: f32, segments: u32, rings: u32) -> MeshResult<MeshData> {
    let row = segments + 1;
    let mut vertices = Vec::with_capacity(row as usize * (rings as usize + 1));
    for latitude in 0..=rings {
        let v = latitude as f32 / rings as f32;
        let theta = v * std::f32::consts::PI;
        let sin_theta = theta.sin();
        let cos_theta = theta.cos();
        for longitude in 0..=segments {
            let u = longitude as f32 / segments as f32;
            let phi = u * std::f32::consts::TAU;
            let normal = [sin_theta * phi.cos(), cos_theta, sin_theta * phi.sin()];
            vertices.push(make_vertex(
                normal.map(|value| value * radius),
                normal,
                [u, v],
            ));
        }
    }
    let mut indices = Vec::with_capacity((segments * (rings - 1) * 6) as usize);
    append_profile_indices(&mut indices, segments, rings, row);
    MeshData::new("Sphere", vertices, indices, Vec::new(), Vec::new(), false)
}

fn append_profile_indices(indices: &mut Vec<u32>, segments: u32, bands: u32, row: u32) {
    for band in 0..bands {
        for segment in 0..segments {
            let a = band * row + segment;
            let b = a + 1;
            let d = a + row;
            let c = d + 1;
            if band == 0 {
                indices.extend([a, c, d]);
            } else if band + 1 == bands {
                indices.extend([a, b, d]);
            } else {
                indices.extend([a, b, d, b, c, d]);
            }
        }
    }
}

fn make_cylinder(radius: f32, height: f32, segments: u32) -> MeshResult<MeshData> {
    let half = height * 0.5;
    let mut vertices = Vec::with_capacity((segments as usize + 1) * 4 + 2);
    let mut indices = Vec::with_capacity(segments as usize * 12);
    for segment in 0..=segments {
        let u = segment as f32 / segments as f32;
        let angle = u * std::f32::consts::TAU;
        let normal = [angle.cos(), 0.0, angle.sin()];
        vertices.push(make_vertex(
            [normal[0] * radius, -half, normal[2] * radius],
            normal,
            [u, 1.0],
        ));
        vertices.push(make_vertex(
            [normal[0] * radius, half, normal[2] * radius],
            normal,
            [u, 0.0],
        ));
    }
    for segment in 0..segments {
        let bottom = segment * 2;
        let top = bottom + 1;
        let next_bottom = bottom + 2;
        let next_top = bottom + 3;
        indices.extend([bottom, top, next_bottom, top, next_top, next_bottom]);
    }
    append_disc(&mut vertices, &mut indices, radius, half, segments, true)?;
    append_disc(&mut vertices, &mut indices, radius, -half, segments, false)?;
    MeshData::new("Cylinder", vertices, indices, Vec::new(), Vec::new(), false)
}

fn append_disc(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    radius: f32,
    y: f32,
    segments: u32,
    top: bool,
) -> MeshResult<()> {
    let normal = [0.0, if top { 1.0 } else { -1.0 }, 0.0];
    let center = usize_to_u32(vertices.len(), "primitive cap vertex offset")?;
    vertices.push(make_vertex([0.0, y, 0.0], normal, [0.5, 0.5]));
    for segment in 0..=segments {
        let angle = segment as f32 / segments as f32 * std::f32::consts::TAU;
        let x = angle.cos();
        let z = angle.sin();
        vertices.push(make_vertex(
            [x * radius, y, z * radius],
            normal,
            [x * 0.5 + 0.5, z * 0.5 + 0.5],
        ));
    }
    for segment in 0..segments {
        let current = center + 1 + segment;
        let next = current + 1;
        if top {
            indices.extend([center, next, current]);
        } else {
            indices.extend([center, current, next]);
        }
    }
    Ok(())
}

fn make_cone(radius: f32, height: f32, segments: u32) -> MeshResult<MeshData> {
    let half = height * 0.5;
    let slope_length = (height * height + radius * radius).sqrt();
    let mut vertices = Vec::with_capacity((segments as usize + 1) * 2 + segments as usize + 2);
    let mut indices = Vec::with_capacity(segments as usize * 6);
    for segment in 0..=segments {
        let u = segment as f32 / segments as f32;
        let angle = u * std::f32::consts::TAU;
        let normal = [
            height / slope_length * angle.cos(),
            radius / slope_length,
            height / slope_length * angle.sin(),
        ];
        vertices.push(make_vertex(
            [radius * angle.cos(), -half, radius * angle.sin()],
            normal,
            [u, 1.0],
        ));
        vertices.push(make_vertex([0.0, half, 0.0], normal, [u, 0.0]));
    }
    for segment in 0..segments {
        let base = segment * 2;
        indices.extend([base, base + 1, base + 2]);
    }
    append_disc(&mut vertices, &mut indices, radius, -half, segments, false)?;
    MeshData::new("Cone", vertices, indices, Vec::new(), Vec::new(), false)
}

fn make_capsule(radius: f32, height: f32, segments: u32, rings: u32) -> MeshResult<MeshData> {
    let hemisphere_rings = rings.max(2);
    let body_half = (height - radius * 2.0) * 0.5;
    // radius-at-height, y, horizontal normal, vertical normal
    let mut profile = Vec::<(f32, f32, f32, f32)>::new();
    profile.push((0.0, body_half + radius, 0.0, 1.0));
    for ring in 1..=hemisphere_rings {
        let angle = ring as f32 / hemisphere_rings as f32 * std::f32::consts::FRAC_PI_2;
        profile.push((
            radius * angle.sin(),
            body_half + radius * angle.cos(),
            angle.sin(),
            angle.cos(),
        ));
    }
    if body_half > f32::EPSILON {
        profile.push((radius, -body_half, 1.0, 0.0));
    }
    for ring in 1..=hemisphere_rings {
        let angle = ring as f32 / hemisphere_rings as f32 * std::f32::consts::FRAC_PI_2;
        profile.push((
            radius * angle.cos(),
            -body_half - radius * angle.sin(),
            angle.cos(),
            -angle.sin(),
        ));
    }
    let row = segments + 1;
    let bands = usize_to_u32(profile.len() - 1, "capsule band count")?;
    let mut vertices = Vec::with_capacity(profile.len() * row as usize);
    for (profile_index, (ring_radius, y, horizontal, vertical)) in
        profile.iter().copied().enumerate()
    {
        let v = profile_index as f32 / (profile.len() - 1) as f32;
        for segment in 0..=segments {
            let u = segment as f32 / segments as f32;
            let angle = u * std::f32::consts::TAU;
            let normal = [horizontal * angle.cos(), vertical, horizontal * angle.sin()];
            vertices.push(make_vertex(
                [ring_radius * angle.cos(), y, ring_radius * angle.sin()],
                normal,
                [u, v],
            ));
        }
    }
    let mut indices = Vec::with_capacity((segments * bands * 6) as usize);
    append_profile_indices(&mut indices, segments, bands, row);
    MeshData::new("Capsule", vertices, indices, Vec::new(), Vec::new(), false)
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct ObjCorner {
    position: usize,
    uv: Option<usize>,
    normal: Option<usize>,
}

fn parse_obj(bytes: &[u8]) -> MeshResult<MeshData> {
    let source = std::str::from_utf8(bytes)
        .map_err(|error| MeshError::InvalidData(format!("OBJ is not UTF-8 text: {error}")))?;
    let mut positions = Vec::<[f32; 3]>::new();
    let mut uvs = Vec::<[f32; 2]>::new();
    let mut normals = Vec::<[f32; 3]>::new();
    let mut vertices = Vec::<Vertex>::new();
    let mut indices = Vec::<u32>::new();
    let mut corner_cache = HashMap::<ObjCorner, u32>::new();
    let mut materials = Vec::<MeshMaterial>::new();
    let mut material_indices = HashMap::<String, usize>::new();
    let mut submeshes = Vec::<Submesh>::new();
    let mut current_group = "default".to_string();
    let mut current_material = None;
    let mut active_range: Option<(String, Option<usize>, usize)> = None;
    let mut missing_normals = false;

    for (line_index, raw_line) in source.lines().enumerate() {
        let line_number = line_index + 1;
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split_whitespace();
        let Some(directive) = fields.next() else {
            continue;
        };
        match directive {
            "v" => {
                positions.push([
                    parse_obj_float(fields.next(), line_number, "position x")?,
                    parse_obj_float(fields.next(), line_number, "position y")?,
                    parse_obj_float(fields.next(), line_number, "position z")?,
                ]);
            }
            "vt" => {
                uvs.push([
                    parse_obj_float(fields.next(), line_number, "texture u")?,
                    parse_obj_float(fields.next(), line_number, "texture v")?,
                ]);
            }
            "vn" => {
                normals.push([
                    parse_obj_float(fields.next(), line_number, "normal x")?,
                    parse_obj_float(fields.next(), line_number, "normal y")?,
                    parse_obj_float(fields.next(), line_number, "normal z")?,
                ]);
            }
            "o" | "g" => {
                let name = fields.collect::<Vec<_>>().join(" ");
                current_group = if name.is_empty() {
                    "default".to_string()
                } else {
                    name
                };
            }
            "usemtl" => {
                let name = fields.collect::<Vec<_>>().join(" ");
                if name.is_empty() {
                    current_material = None;
                } else if let Some(index) = material_indices.get(&name).copied() {
                    current_material = Some(index);
                } else {
                    let index = materials.len();
                    materials.push(MeshMaterial::named(name.clone()));
                    material_indices.insert(name, index);
                    current_material = Some(index);
                }
            }
            "f" => {
                let mut face = Vec::<u32>::new();
                for token in fields {
                    let corner = parse_obj_corner(
                        token,
                        positions.len(),
                        uvs.len(),
                        normals.len(),
                        line_number,
                    )?;
                    let vertex_index = if let Some(existing) = corner_cache.get(&corner).copied() {
                        existing
                    } else {
                        let position = *positions.get(corner.position).ok_or_else(|| {
                            MeshError::InvalidData(format!(
                                "OBJ line {line_number} references a missing position"
                            ))
                        })?;
                        let uv = corner
                            .uv
                            .and_then(|index| uvs.get(index).copied())
                            .unwrap_or([0.0; 2]);
                        let normal = corner
                            .normal
                            .and_then(|index| normals.get(index).copied())
                            .unwrap_or([0.0; 3]);
                        missing_normals |= corner.normal.is_none();
                        let index = usize_to_u32(vertices.len(), "OBJ vertex count")?;
                        vertices.push(Vertex {
                            position,
                            normal,
                            uv,
                            tangent: [1.0, 0.0, 0.0, 1.0],
                        });
                        corner_cache.insert(corner, index);
                        index
                    };
                    face.push(vertex_index);
                }
                if face.len() < 3 {
                    return Err(MeshError::InvalidData(format!(
                        "OBJ line {line_number} face has fewer than three corners"
                    )));
                }

                let active_matches = active_range.as_ref().is_some_and(|(name, material, _)| {
                    name == &current_group && *material == current_material
                });
                if !active_matches {
                    let range_index = submeshes.len();
                    submeshes.push(Submesh {
                        name: current_group.clone(),
                        first_index: usize_to_u32(indices.len(), "OBJ submesh offset")?,
                        index_count: 0,
                        material: current_material,
                    });
                    active_range = Some((current_group.clone(), current_material, range_index));
                }

                let first = face[0];
                for corner in 1..face.len() - 1 {
                    indices.extend_from_slice(&[first, face[corner], face[corner + 1]]);
                }
                let added = (face.len() - 2).checked_mul(3).ok_or_else(|| {
                    MeshError::InvalidData(format!("OBJ line {line_number} face is too large"))
                })?;
                let Some((_, _, range_index)) = active_range.as_ref() else {
                    return Err(MeshError::InvalidData(
                        "OBJ internal submesh state is missing".into(),
                    ));
                };
                let range = submeshes.get_mut(*range_index).ok_or_else(|| {
                    MeshError::InvalidData("OBJ internal submesh index is invalid".into())
                })?;
                range.index_count = range
                    .index_count
                    .checked_add(usize_to_u32(added, "OBJ face index count")?)
                    .ok_or_else(|| {
                        MeshError::InvalidData("OBJ submesh index count overflows".into())
                    })?;
            }
            _ => {}
        }
    }

    if positions.is_empty() || indices.is_empty() {
        return Err(MeshError::InvalidData(
            "OBJ contains no triangle geometry".into(),
        ));
    }
    MeshData::new(
        "OBJ Mesh",
        vertices,
        indices,
        submeshes,
        materials,
        missing_normals || normals.is_empty(),
    )
}

fn parse_obj_float(value: Option<&str>, line: usize, label: &str) -> MeshResult<f32> {
    let value = value
        .ok_or_else(|| MeshError::InvalidData(format!("OBJ line {line} is missing {label}")))?;
    let parsed = value.parse::<f32>().map_err(|error| {
        MeshError::InvalidData(format!("OBJ line {line} has invalid {label}: {error}"))
    })?;
    if !parsed.is_finite() {
        return Err(MeshError::InvalidData(format!(
            "OBJ line {line} has non-finite {label}"
        )));
    }
    Ok(parsed)
}

fn parse_obj_corner(
    token: &str,
    position_count: usize,
    uv_count: usize,
    normal_count: usize,
    line: usize,
) -> MeshResult<ObjCorner> {
    let parts = token.split('/').collect::<Vec<_>>();
    if parts.is_empty() || parts.len() > 3 || parts[0].is_empty() {
        return Err(MeshError::InvalidData(format!(
            "OBJ line {line} has invalid face corner '{token}'"
        )));
    }
    let position = resolve_obj_index(parts[0], position_count, line, "position")?;
    let uv = parts
        .get(1)
        .filter(|value| !value.is_empty())
        .map(|value| resolve_obj_index(value, uv_count, line, "texture coordinate"))
        .transpose()?;
    let normal = parts
        .get(2)
        .filter(|value| !value.is_empty())
        .map(|value| resolve_obj_index(value, normal_count, line, "normal"))
        .transpose()?;
    Ok(ObjCorner {
        position,
        uv,
        normal,
    })
}

fn resolve_obj_index(token: &str, count: usize, line: usize, label: &str) -> MeshResult<usize> {
    let raw = token.parse::<i64>().map_err(|error| {
        MeshError::InvalidData(format!(
            "OBJ line {line} has invalid {label} index '{token}': {error}"
        ))
    })?;
    if raw == 0 {
        return Err(MeshError::InvalidData(format!(
            "OBJ line {line} uses forbidden zero {label} index"
        )));
    }
    let count = i64::try_from(count)
        .map_err(|_| MeshError::InvalidData(format!("OBJ {label} table is too large to index")))?;
    let resolved = if raw > 0 {
        raw - 1
    } else {
        count.checked_add(raw).ok_or_else(|| {
            MeshError::InvalidData(format!("OBJ line {line} {label} index overflows"))
        })?
    };
    if resolved < 0 || resolved >= count {
        return Err(MeshError::InvalidData(format!(
            "OBJ line {line} {label} index {raw} is out of range for {count} entries"
        )));
    }
    usize::try_from(resolved).map_err(|_| {
        MeshError::InvalidData(format!(
            "OBJ line {line} {label} index cannot fit in memory"
        ))
    })
}

const GLB_MAGIC: &[u8; 4] = b"glTF";
const GLB_JSON_CHUNK: u32 = 0x4e4f_534a;
const GLB_BIN_CHUNK: u32 = 0x004e_4942;

fn parse_glb(bytes: &[u8], base_dir: Option<&Path>) -> MeshResult<MeshData> {
    if bytes.get(..4) != Some(GLB_MAGIC.as_slice()) {
        return Err(MeshError::InvalidData("GLB magic is missing".into()));
    }
    let version = read_u32_le(bytes, 4, "GLB version")?;
    if version != 2 {
        return Err(MeshError::UnsupportedFormat(format!(
            "GLB version {version}; only version 2 is supported"
        )));
    }
    let declared_length = usize::try_from(read_u32_le(bytes, 8, "GLB length")?)
        .map_err(|_| MeshError::InvalidData("GLB length cannot fit in memory".into()))?;
    if declared_length != bytes.len() {
        return Err(MeshError::InvalidData(format!(
            "GLB declares {declared_length} bytes but contains {}",
            bytes.len()
        )));
    }

    let mut offset = 12usize;
    let mut json_chunk = None;
    let mut bin_chunk = None;
    while offset < declared_length {
        let header_end = offset
            .checked_add(8)
            .ok_or_else(|| MeshError::InvalidData("GLB chunk header offset overflows".into()))?;
        if header_end > declared_length {
            return Err(MeshError::InvalidData(
                "GLB ends inside a chunk header".into(),
            ));
        }
        let chunk_length = usize::try_from(read_u32_le(bytes, offset, "GLB chunk length")?)
            .map_err(|_| MeshError::InvalidData("GLB chunk length cannot fit in memory".into()))?;
        let chunk_type = read_u32_le(bytes, offset + 4, "GLB chunk type")?;
        let chunk_end = header_end
            .checked_add(chunk_length)
            .ok_or_else(|| MeshError::InvalidData("GLB chunk range overflows".into()))?;
        if chunk_end > declared_length {
            return Err(MeshError::InvalidData(
                "GLB chunk extends past the declared file length".into(),
            ));
        }
        if chunk_length % 4 != 0 {
            return Err(MeshError::InvalidData(
                "GLB chunk length is not four-byte aligned".into(),
            ));
        }
        let chunk = &bytes[header_end..chunk_end];
        match chunk_type {
            GLB_JSON_CHUNK => {
                if json_chunk.is_some() || offset != 12 {
                    return Err(MeshError::InvalidData(
                        "GLB must contain exactly one JSON chunk first".into(),
                    ));
                }
                json_chunk = Some(chunk);
            }
            GLB_BIN_CHUNK => {
                if bin_chunk.is_some() {
                    return Err(MeshError::InvalidData(
                        "GLB contains more than one BIN chunk".into(),
                    ));
                }
                bin_chunk = Some(chunk);
            }
            _ => {}
        }
        offset = chunk_end;
    }

    let mut json =
        json_chunk.ok_or_else(|| MeshError::InvalidData("GLB has no JSON chunk".into()))?;
    while json
        .last()
        .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | 0))
    {
        json = &json[..json.len() - 1];
    }
    parse_gltf_json(json, base_dir, bin_chunk)
}

struct GltfArmatureImport {
    skin_index: usize,
    mesh_skins: Vec<Option<usize>>,
    nodes: Vec<ArmatureNode>,
    joints: Vec<usize>,
    inverse_bind_matrices: Vec<[f32; 16]>,
    animations: Vec<AnimationClip>,
}

fn parse_gltf_armature(
    root: &JsonMap<String, JsonValue>,
    buffers: &[Vec<u8>],
    mesh_count: usize,
) -> MeshResult<Option<GltfArmatureImport>> {
    let Some(skins) = root.get("skins") else {
        return Ok(None);
    };
    let skins = skins
        .as_array()
        .ok_or_else(|| MeshError::InvalidData("glTF skins must be an array".into()))?;
    let node_values = root
        .get("nodes")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| MeshError::InvalidData("skinned glTF has no nodes array".into()))?;
    if node_values.len() > MAX_ARMATURE_NODES {
        return Err(MeshError::InvalidData(format!(
            "glTF has {} armature nodes; limit is {MAX_ARMATURE_NODES}",
            node_values.len()
        )));
    }

    let mut parents = vec![None; node_values.len()];
    for (parent_index, value) in node_values.iter().enumerate() {
        let node = value.as_object().ok_or_else(|| {
            MeshError::InvalidData(format!("glTF node {parent_index} is not an object"))
        })?;
        if let Some(children) = node.get("children") {
            let children = children.as_array().ok_or_else(|| {
                MeshError::InvalidData(format!("glTF node {parent_index} children is not an array"))
            })?;
            for child in children {
                let child = child.as_u64().ok_or_else(|| {
                    MeshError::InvalidData(format!(
                        "glTF node {parent_index} has a non-integer child"
                    ))
                })?;
                let child = usize::try_from(child).map_err(|_| {
                    MeshError::InvalidData("glTF child node index cannot fit in memory".into())
                })?;
                let destination = parents.get_mut(child).ok_or_else(|| {
                    MeshError::InvalidData(format!(
                        "glTF node {parent_index} references missing child {child}"
                    ))
                })?;
                if destination.replace(parent_index).is_some() {
                    return Err(MeshError::InvalidData(format!(
                        "glTF node {child} has more than one parent"
                    )));
                }
            }
        }
    }

    let mut nodes = Vec::with_capacity(node_values.len());
    let mut mesh_skins = vec![None; mesh_count];
    let mut referenced_skins = std::collections::BTreeSet::new();
    for (index, value) in node_values.iter().enumerate() {
        let node = value
            .as_object()
            .ok_or_else(|| MeshError::InvalidData(format!("glTF node {index} is not an object")))?;
        let (translation, rotation, scale) = parse_gltf_node_transform(node, index)?;
        nodes.push(ArmatureNode {
            name: node
                .get("name")
                .and_then(JsonValue::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("Node {index}")),
            parent: parents[index],
            translation,
            rotation,
            scale,
        });
        if let Some(mesh) = optional_usize(node, "mesh", &format!("glTF node {index}"))? {
            if mesh >= mesh_count {
                return Err(MeshError::InvalidData(format!(
                    "glTF node {index} references missing mesh {mesh}"
                )));
            }
            if let Some(skin) = optional_usize(node, "skin", &format!("glTF node {index}"))? {
                if skin >= skins.len() {
                    return Err(MeshError::InvalidData(format!(
                        "glTF node {index} references missing skin {skin}"
                    )));
                }
                if mesh_skins[mesh].is_some_and(|existing| existing != skin) {
                    return Err(MeshError::UnsupportedFormat(format!(
                        "glTF mesh {mesh} is instantiated with multiple skins"
                    )));
                }
                mesh_skins[mesh] = Some(skin);
                referenced_skins.insert(skin);
            }
        }
    }
    validate_armature_hierarchy(&nodes)?;
    if referenced_skins.is_empty() {
        return Ok(None);
    }
    if referenced_skins.len() != 1 {
        return Err(MeshError::UnsupportedFormat(
            "one flattened mesh asset cannot currently contain multiple glTF skins".into(),
        ));
    }
    let skin_index = *referenced_skins.first().expect("set is not empty");
    let skin = skins[skin_index].as_object().ok_or_else(|| {
        MeshError::InvalidData(format!("glTF skin {skin_index} is not an object"))
    })?;
    let joint_values = skin
        .get("joints")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| MeshError::InvalidData(format!("glTF skin {skin_index} has no joints")))?;
    if joint_values.is_empty() || joint_values.len() > usize::from(u16::MAX) + 1 {
        return Err(MeshError::InvalidData(format!(
            "glTF skin {skin_index} has an invalid joint count"
        )));
    }
    let mut joints = Vec::with_capacity(joint_values.len());
    let mut unique = std::collections::HashSet::new();
    for value in joint_values {
        let node = value.as_u64().ok_or_else(|| {
            MeshError::InvalidData(format!("glTF skin {skin_index} has a non-integer joint"))
        })?;
        let node = usize::try_from(node)
            .map_err(|_| MeshError::InvalidData("glTF joint index is too large".into()))?;
        if node >= nodes.len() || !unique.insert(node) {
            return Err(MeshError::InvalidData(format!(
                "glTF skin {skin_index} references a missing or duplicate joint {node}"
            )));
        }
        joints.push(node);
    }
    let inverse_bind_matrices = if let Some(accessor) = optional_usize(
        skin,
        "inverseBindMatrices",
        &format!("glTF skin {skin_index}"),
    )? {
        let view = AccessorView::new(
            root,
            buffers,
            accessor,
            "MAT4",
            &format!("glTF skin {skin_index} inverse bind matrices"),
        )?;
        if view.component_type != 5126 || view.count != joints.len() || view.normalized {
            return Err(MeshError::InvalidData(format!(
                "glTF skin {skin_index} inverse bind matrices must be FLOAT MAT4 with one entry per joint"
            )));
        }
        (0..view.count)
            .map(|index| view.read_vec::<16>(index))
            .collect::<MeshResult<Vec<_>>>()?
    } else {
        vec![identity_matrix(); joints.len()]
    };
    let animations = parse_gltf_animations(root, buffers, nodes.len())?;
    Ok(Some(GltfArmatureImport {
        skin_index,
        mesh_skins,
        nodes,
        joints,
        inverse_bind_matrices,
        animations,
    }))
}

fn parse_gltf_node_transform(
    node: &JsonMap<String, JsonValue>,
    index: usize,
) -> MeshResult<([f32; 3], [f32; 4], [f32; 3])> {
    let context = format!("glTF node {index}");
    if let Some(matrix) = node.get("matrix") {
        if node.contains_key("translation")
            || node.contains_key("rotation")
            || node.contains_key("scale")
        {
            return Err(MeshError::InvalidData(format!(
                "{context} combines matrix and TRS transforms"
            )));
        }
        return decompose_gltf_matrix(
            json_float_array(
                Some(matrix),
                identity_matrix(),
                &format!("{context} matrix"),
            )?,
            &context,
        );
    }
    let translation = json_float_array(
        node.get("translation"),
        [0.0; 3],
        &format!("{context} translation"),
    )?;
    let mut rotation = json_float_array(
        node.get("rotation"),
        [0.0, 0.0, 0.0, 1.0],
        &format!("{context} rotation"),
    )?;
    let rotation_length = rotation
        .iter()
        .map(|value| value * value)
        .sum::<f32>()
        .sqrt();
    if rotation_length <= f32::EPSILON || (rotation_length - 1.0).abs() > 0.01 {
        return Err(MeshError::InvalidData(format!(
            "{context} rotation is not a unit quaternion"
        )));
    }
    rotation = normalize_quaternion(rotation);
    let scale = json_float_array(node.get("scale"), [1.0; 3], &format!("{context} scale"))?;
    Ok((translation, rotation, scale))
}

fn decompose_gltf_matrix(
    matrix: [f32; 16],
    context: &str,
) -> MeshResult<([f32; 3], [f32; 4], [f32; 3])> {
    if matrix[3].abs() > 0.0001
        || matrix[7].abs() > 0.0001
        || matrix[11].abs() > 0.0001
        || (matrix[15] - 1.0).abs() > 0.0001
    {
        return Err(MeshError::UnsupportedFormat(format!(
            "{context} is not an affine TRS matrix"
        )));
    }
    let mut columns = [
        [matrix[0], matrix[1], matrix[2]],
        [matrix[4], matrix[5], matrix[6]],
        [matrix[8], matrix[9], matrix[10]],
    ];
    let mut scale =
        columns.map(|column| column.iter().map(|value| value * value).sum::<f32>().sqrt());
    if scale
        .iter()
        .any(|value| *value <= f32::EPSILON || !value.is_finite())
    {
        return Err(MeshError::InvalidData(format!(
            "{context} has a singular scale"
        )));
    }
    let determinant = columns[0][0]
        * (columns[1][1] * columns[2][2] - columns[1][2] * columns[2][1])
        - columns[1][0] * (columns[0][1] * columns[2][2] - columns[0][2] * columns[2][1])
        + columns[2][0] * (columns[0][1] * columns[1][2] - columns[0][2] * columns[1][1]);
    if determinant < 0.0 {
        scale[0] = -scale[0];
    }
    for column in 0..3 {
        for row in 0..3 {
            columns[column][row] /= scale[column];
        }
    }
    let dot = |a: usize, b: usize| {
        (0..3)
            .map(|row| columns[a][row] * columns[b][row])
            .sum::<f32>()
    };
    if dot(0, 1).abs() > 0.001 || dot(0, 2).abs() > 0.001 || dot(1, 2).abs() > 0.001 {
        return Err(MeshError::UnsupportedFormat(format!(
            "{context} contains shear, which cannot be animated as TRS"
        )));
    }
    let m00 = columns[0][0];
    let m01 = columns[1][0];
    let m02 = columns[2][0];
    let m10 = columns[0][1];
    let m11 = columns[1][1];
    let m12 = columns[2][1];
    let m20 = columns[0][2];
    let m21 = columns[1][2];
    let m22 = columns[2][2];
    let rotation = if m00 + m11 + m22 > 0.0 {
        let s = (m00 + m11 + m22 + 1.0).sqrt() * 2.0;
        [(m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, s * 0.25]
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        [s * 0.25, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s]
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        [(m01 + m10) / s, s * 0.25, (m12 + m21) / s, (m02 - m20) / s]
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        [(m02 + m20) / s, (m12 + m21) / s, s * 0.25, (m10 - m01) / s]
    };
    Ok((
        [matrix[12], matrix[13], matrix[14]],
        normalize_quaternion(rotation),
        scale,
    ))
}

fn parse_gltf_animations(
    root: &JsonMap<String, JsonValue>,
    buffers: &[Vec<u8>],
    node_count: usize,
) -> MeshResult<Vec<AnimationClip>> {
    let Some(values) = root.get("animations") else {
        return Ok(Vec::new());
    };
    let values = values
        .as_array()
        .ok_or_else(|| MeshError::InvalidData("glTF animations must be an array".into()))?;
    if values.len() > 4096 {
        return Err(MeshError::InvalidData(
            "glTF contains more than 4096 animation clips".into(),
        ));
    }
    let mut clips = Vec::with_capacity(values.len());
    for (animation_index, value) in values.iter().enumerate() {
        let context = format!("glTF animation {animation_index}");
        let animation = value
            .as_object()
            .ok_or_else(|| MeshError::InvalidData(format!("{context} is not an object")))?;
        let samplers = animation
            .get("samplers")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| MeshError::InvalidData(format!("{context} has no samplers")))?;
        let channels = animation
            .get("channels")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| MeshError::InvalidData(format!("{context} has no channels")))?;
        if channels.len() > 65_536 {
            return Err(MeshError::InvalidData(format!(
                "{context} contains too many channels"
            )));
        }
        let mut parsed_channels = Vec::with_capacity(channels.len());
        let mut duration = 0.0f32;
        for (channel_index, value) in channels.iter().enumerate() {
            let channel_context = format!("{context} channel {channel_index}");
            let channel = value.as_object().ok_or_else(|| {
                MeshError::InvalidData(format!("{channel_context} is not an object"))
            })?;
            let sampler_index = required_usize(channel, "sampler", &channel_context)?;
            let sampler = samplers
                .get(sampler_index)
                .and_then(JsonValue::as_object)
                .ok_or_else(|| {
                    MeshError::InvalidData(format!(
                        "{channel_context} references missing sampler {sampler_index}"
                    ))
                })?;
            let target = channel
                .get("target")
                .and_then(JsonValue::as_object)
                .ok_or_else(|| {
                    MeshError::InvalidData(format!("{channel_context} has no target"))
                })?;
            let node = required_usize(target, "node", &channel_context)?;
            if node >= node_count {
                return Err(MeshError::InvalidData(format!(
                    "{channel_context} references missing node {node}"
                )));
            }
            let property = match target.get("path").and_then(JsonValue::as_str) {
                Some("translation") => AnimationProperty::Translation,
                Some("rotation") => AnimationProperty::Rotation,
                Some("scale") => AnimationProperty::Scale,
                Some("weights") => continue,
                Some(other) => {
                    return Err(MeshError::InvalidData(format!(
                        "{channel_context} has unknown target path '{other}'"
                    )));
                }
                None => {
                    return Err(MeshError::InvalidData(format!(
                        "{channel_context} target path is missing"
                    )));
                }
            };
            let interpolation = match sampler
                .get("interpolation")
                .and_then(JsonValue::as_str)
                .unwrap_or("LINEAR")
            {
                "LINEAR" => AnimationInterpolation::Linear,
                "STEP" => AnimationInterpolation::Step,
                "CUBICSPLINE" => {
                    return Err(MeshError::UnsupportedFormat(format!(
                        "{channel_context} uses CUBICSPLINE interpolation"
                    )));
                }
                other => {
                    return Err(MeshError::InvalidData(format!(
                        "{channel_context} has invalid interpolation '{other}'"
                    )));
                }
            };
            let input = required_usize(sampler, "input", &channel_context)?;
            let output = required_usize(sampler, "output", &channel_context)?;
            let times_view = AccessorView::new(
                root,
                buffers,
                input,
                "SCALAR",
                &format!("{channel_context} input"),
            )?;
            if times_view.component_type != 5126 || times_view.normalized || times_view.count == 0 {
                return Err(MeshError::InvalidData(format!(
                    "{channel_context} input must be a non-empty FLOAT SCALAR accessor"
                )));
            }
            if times_view.count > 1_000_000 {
                return Err(MeshError::InvalidData(format!(
                    "{channel_context} exceeds one million keyframes"
                )));
            }
            let times = (0..times_view.count)
                .map(|key| times_view.read_vec::<1>(key).map(|value| value[0]))
                .collect::<MeshResult<Vec<_>>>()?;
            let output_type = if property == AnimationProperty::Rotation {
                "VEC4"
            } else {
                "VEC3"
            };
            let output_view = AccessorView::new(
                root,
                buffers,
                output,
                output_type,
                &format!("{channel_context} output"),
            )?;
            if output_view.component_type != 5126
                || output_view.normalized
                || output_view.count != times.len()
            {
                return Err(MeshError::InvalidData(format!(
                    "{channel_context} output must be FLOAT {output_type} with the input key count"
                )));
            }
            let mut parsed_values = Vec::with_capacity(times.len());
            for key in 0..times.len() {
                let value = if property == AnimationProperty::Rotation {
                    let rotation = output_view.read_vec::<4>(key)?;
                    if rotation.iter().map(|value| value * value).sum::<f32>() <= f32::EPSILON {
                        return Err(MeshError::InvalidData(format!(
                            "{channel_context} key {key} has a zero rotation"
                        )));
                    }
                    normalize_quaternion(rotation)
                } else {
                    let vector = output_view.read_vec::<3>(key)?;
                    [vector[0], vector[1], vector[2], 0.0]
                };
                parsed_values.push(value);
            }
            duration = duration.max(*times.last().unwrap_or(&0.0));
            parsed_channels.push(AnimationChannel {
                node,
                property,
                interpolation,
                times,
                values: parsed_values,
            });
        }
        if !parsed_channels.is_empty() {
            clips.push(AnimationClip {
                name: animation
                    .get("name")
                    .and_then(JsonValue::as_str)
                    .map(str::to_string)
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| format!("Animation {animation_index}")),
                duration,
                channels: parsed_channels,
            });
        }
    }
    Ok(clips)
}

fn parse_gltf_json(
    bytes: &[u8],
    base_dir: Option<&Path>,
    embedded_bin: Option<&[u8]>,
) -> MeshResult<MeshData> {
    let root_value: JsonValue = serde_json::from_slice(bytes)?;
    let root = root_value
        .as_object()
        .ok_or_else(|| MeshError::InvalidData("glTF root must be a JSON object".into()))?;
    let version = root
        .get("asset")
        .and_then(JsonValue::as_object)
        .and_then(|asset| asset.get("version"))
        .and_then(JsonValue::as_str)
        .ok_or_else(|| MeshError::InvalidData("glTF asset.version is missing".into()))?;
    if version.split('.').next() != Some("2") {
        return Err(MeshError::UnsupportedFormat(format!(
            "glTF version {version}; only version 2 is supported"
        )));
    }

    if root
        .get("extensionsRequired")
        .and_then(JsonValue::as_array)
        .is_some_and(|extensions| {
            extensions.iter().any(|extension| {
                matches!(
                    extension.as_str(),
                    Some("KHR_draco_mesh_compression" | "EXT_meshopt_compression")
                )
            })
        })
    {
        return Err(MeshError::UnsupportedFormat(
            "compressed glTF meshes (Draco/meshopt) are not supported".into(),
        ));
    }

    let buffers = load_gltf_buffers(root, base_dir, embedded_bin)?;
    let materials = parse_gltf_materials(root)?;
    let meshes = root
        .get("meshes")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| MeshError::InvalidData("glTF meshes array is missing".into()))?;
    if meshes.is_empty() {
        return Err(MeshError::InvalidData("glTF has no meshes".into()));
    }
    let armature_import = parse_gltf_armature(root, &buffers, meshes.len())?;

    let mut vertices = Vec::<Vertex>::new();
    let mut vertex_weights = Vec::<SkinWeights>::new();
    let mut indices = Vec::<u32>::new();
    let mut submeshes = Vec::<Submesh>::new();
    let mut needs_normals = false;
    let mut mesh_name = "glTF Mesh".to_string();

    for (mesh_index, mesh_value) in meshes.iter().enumerate() {
        let mesh = mesh_value.as_object().ok_or_else(|| {
            MeshError::InvalidData(format!("glTF mesh {mesh_index} is not an object"))
        })?;
        let current_name = mesh
            .get("name")
            .and_then(JsonValue::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("Mesh {mesh_index}"));
        if mesh_index == 0 {
            mesh_name = current_name.clone();
        }
        let primitives = mesh
            .get("primitives")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| {
                MeshError::InvalidData(format!("glTF mesh {mesh_index} has no primitives"))
            })?;
        for (primitive_index, primitive_value) in primitives.iter().enumerate() {
            let context = format!("glTF mesh {mesh_index} primitive {primitive_index}");
            let primitive = primitive_value
                .as_object()
                .ok_or_else(|| MeshError::InvalidData(format!("{context} is not an object")))?;
            let mode = optional_usize(primitive, "mode", &context)?.unwrap_or(4);
            if mode != 4 {
                return Err(MeshError::UnsupportedFormat(format!(
                    "{context} uses primitive mode {mode}; only triangles (4) are supported"
                )));
            }
            let attributes = primitive
                .get("attributes")
                .and_then(JsonValue::as_object)
                .ok_or_else(|| MeshError::InvalidData(format!("{context} has no attributes")))?;
            let position_accessor = attributes
                .get("POSITION")
                .and_then(JsonValue::as_u64)
                .ok_or_else(|| MeshError::InvalidData(format!("{context} has no POSITION")))?;
            let position_accessor = usize::try_from(position_accessor).map_err(|_| {
                MeshError::InvalidData(format!("{context} POSITION index cannot fit in memory"))
            })?;
            let positions = AccessorView::new(
                root,
                &buffers,
                position_accessor,
                "VEC3",
                &format!("{context} POSITION"),
            )?;
            if positions.component_type != 5126 {
                return Err(MeshError::UnsupportedFormat(format!(
                    "{context} POSITION must use FLOAT components"
                )));
            }
            if positions.count == 0 {
                return Err(MeshError::InvalidData(format!(
                    "{context} POSITION accessor is empty"
                )));
            }

            let normal_accessor = optional_attribute_accessor(attributes, "NORMAL", &context)?;
            let normal_view = normal_accessor
                .map(|index| {
                    AccessorView::new(root, &buffers, index, "VEC3", &format!("{context} NORMAL"))
                })
                .transpose()?;
            if let Some(normals) = normal_view.as_ref() {
                if normals.component_type != 5126 || normals.count != positions.count {
                    return Err(MeshError::InvalidData(format!(
                        "{context} NORMAL must be FLOAT VEC3 with the POSITION count"
                    )));
                }
            } else {
                needs_normals = true;
            }

            let uv_accessor = optional_attribute_accessor(attributes, "TEXCOORD_0", &context)?;
            let uv_view = uv_accessor
                .map(|index| {
                    AccessorView::new(
                        root,
                        &buffers,
                        index,
                        "VEC2",
                        &format!("{context} TEXCOORD_0"),
                    )
                })
                .transpose()?;
            if let Some(uvs) = uv_view.as_ref()
                && uvs.count != positions.count
            {
                return Err(MeshError::InvalidData(format!(
                    "{context} TEXCOORD_0 count differs from POSITION"
                )));
            }
            if let Some(uvs) = uv_view.as_ref()
                && !matches!(
                    (uvs.component_type, uvs.normalized),
                    (5121 | 5123, true) | (5126, false)
                )
            {
                return Err(MeshError::InvalidData(format!(
                    "{context} TEXCOORD_0 must be FLOAT or normalized U8/U16"
                )));
            }

            let tangent_accessor = optional_attribute_accessor(attributes, "TANGENT", &context)?;
            let tangent_view = tangent_accessor
                .map(|index| {
                    AccessorView::new(root, &buffers, index, "VEC4", &format!("{context} TANGENT"))
                })
                .transpose()?;
            if let Some(tangents) = tangent_view.as_ref()
                && (tangents.component_type != 5126 || tangents.count != positions.count)
            {
                return Err(MeshError::InvalidData(format!(
                    "{context} TANGENT must be FLOAT VEC4 with the POSITION count"
                )));
            }

            let joints_accessor = optional_attribute_accessor(attributes, "JOINTS_0", &context)?;
            let weights_accessor = optional_attribute_accessor(attributes, "WEIGHTS_0", &context)?;
            let (joint_view, weight_view) = match (joints_accessor, weights_accessor) {
                (None, None) => (None, None),
                (Some(joints), Some(weights)) => {
                    let selected_skin = armature_import.as_ref().and_then(|armature| {
                        armature.mesh_skins.get(mesh_index).copied().flatten()
                    });
                    if selected_skin != armature_import.as_ref().map(|armature| armature.skin_index)
                    {
                        return Err(MeshError::InvalidData(format!(
                            "{context} has skin attributes but its mesh node has no selected skin"
                        )));
                    }
                    let joints = AccessorView::new(
                        root,
                        &buffers,
                        joints,
                        "VEC4",
                        &format!("{context} JOINTS_0"),
                    )?;
                    let weights = AccessorView::new(
                        root,
                        &buffers,
                        weights,
                        "VEC4",
                        &format!("{context} WEIGHTS_0"),
                    )?;
                    if !matches!(joints.component_type, 5121 | 5123)
                        || joints.normalized
                        || joints.count != positions.count
                    {
                        return Err(MeshError::InvalidData(format!(
                            "{context} JOINTS_0 must be unnormalized U8/U16 VEC4 with the POSITION count"
                        )));
                    }
                    if !matches!(
                        (weights.component_type, weights.normalized),
                        (5121 | 5123, true) | (5126, false)
                    ) || weights.count != positions.count
                    {
                        return Err(MeshError::InvalidData(format!(
                            "{context} WEIGHTS_0 must be FLOAT or normalized U8/U16 VEC4 with the POSITION count"
                        )));
                    }
                    (Some(joints), Some(weights))
                }
                _ => {
                    return Err(MeshError::InvalidData(format!(
                        "{context} must provide JOINTS_0 and WEIGHTS_0 together"
                    )));
                }
            };

            let base_vertex = usize_to_u32(vertices.len(), "glTF vertex offset")?;
            vertices.reserve(positions.count);
            vertex_weights.reserve(positions.count);
            for vertex_index in 0..positions.count {
                let position = positions.read_vec::<3>(vertex_index)?;
                let normal = if let Some(view) = normal_view.as_ref() {
                    view.read_vec::<3>(vertex_index)?
                } else {
                    [0.0; 3]
                };
                let uv = if let Some(view) = uv_view.as_ref() {
                    view.read_vec::<2>(vertex_index)?
                } else {
                    [0.0; 2]
                };
                let tangent = if let Some(view) = tangent_view.as_ref() {
                    view.read_vec::<4>(vertex_index)?
                } else {
                    [1.0, 0.0, 0.0, 1.0]
                };
                vertices.push(Vertex {
                    position,
                    normal,
                    uv,
                    tangent,
                });
                let influences = if let (Some(joints), Some(weights)) =
                    (joint_view.as_ref(), weight_view.as_ref())
                {
                    let joints = joints.read_vec::<4>(vertex_index)?;
                    let weights = weights.read_vec::<4>(vertex_index)?;
                    let mut influences = SkinWeights {
                        joints: [0; 4],
                        weights,
                    };
                    let palette_len = armature_import
                        .as_ref()
                        .map(|armature| armature.joints.len())
                        .unwrap_or(0);
                    for influence in 0..4 {
                        let joint = joints[influence];
                        if joint.fract() != 0.0 || joint < 0.0 || joint > f32::from(u16::MAX) {
                            return Err(MeshError::InvalidData(format!(
                                "{context} vertex {vertex_index} has an invalid joint index"
                            )));
                        }
                        influences.joints[influence] = joint as u16;
                        if influences.weights[influence] > 0.0
                            && usize::from(influences.joints[influence]) >= palette_len
                        {
                            return Err(MeshError::InvalidData(format!(
                                "{context} vertex {vertex_index} references missing joint {joint}"
                            )));
                        }
                    }
                    let sum = influences.weights.iter().sum::<f32>();
                    if sum > f32::EPSILON {
                        for weight in &mut influences.weights {
                            *weight /= sum;
                        }
                    }
                    influences
                } else {
                    SkinWeights::default()
                };
                vertex_weights.push(influences);
            }

            let first_index = usize_to_u32(indices.len(), "glTF submesh offset")?;
            if let Some(accessor_value) = primitive.get("indices") {
                let accessor_index = accessor_value.as_u64().ok_or_else(|| {
                    MeshError::InvalidData(format!("{context} indices is not an integer"))
                })?;
                let accessor_index = usize::try_from(accessor_index).map_err(|_| {
                    MeshError::InvalidData(format!("{context} index accessor is too large"))
                })?;
                let index_view = AccessorView::new(
                    root,
                    &buffers,
                    accessor_index,
                    "SCALAR",
                    &format!("{context} indices"),
                )?;
                if !matches!(index_view.component_type, 5121 | 5123 | 5125) || index_view.normalized
                {
                    return Err(MeshError::InvalidData(format!(
                        "{context} indices must be unnormalized U8, U16, or U32"
                    )));
                }
                if index_view.count % 3 != 0 {
                    return Err(MeshError::InvalidData(format!(
                        "{context} index count is not divisible by three"
                    )));
                }
                indices.reserve(index_view.count);
                for index_position in 0..index_view.count {
                    let local = index_view.read_index(index_position)?;
                    let local_usize = usize::try_from(local).map_err(|_| {
                        MeshError::InvalidData(format!("{context} vertex index is too large"))
                    })?;
                    if local_usize >= positions.count {
                        return Err(MeshError::InvalidData(format!(
                            "{context} index {index_position} references local vertex {local}"
                        )));
                    }
                    indices.push(base_vertex.checked_add(local).ok_or_else(|| {
                        MeshError::InvalidData(format!("{context} rebased index overflows u32"))
                    })?);
                }
            } else {
                if positions.count % 3 != 0 {
                    return Err(MeshError::InvalidData(format!(
                        "{context} unindexed POSITION count is not divisible by three"
                    )));
                }
                for local in 0..positions.count {
                    let local = usize_to_u32(local, "glTF generated index")?;
                    indices.push(base_vertex.checked_add(local).ok_or_else(|| {
                        MeshError::InvalidData(format!("{context} generated index overflows"))
                    })?);
                }
            }
            let index_count = usize_to_u32(
                indices.len()
                    - usize::try_from(first_index).map_err(|_| {
                        MeshError::InvalidData(format!("{context} offset cannot fit in memory"))
                    })?,
                "glTF submesh index count",
            )?;
            let material = optional_usize(primitive, "material", &context)?;
            if material.is_some_and(|index| index >= materials.len()) {
                return Err(MeshError::InvalidData(format!(
                    "{context} references a missing material"
                )));
            }
            submeshes.push(Submesh {
                name: format!("{current_name} {primitive_index}"),
                first_index,
                index_count,
                material,
            });
        }
    }

    if indices.is_empty() {
        return Err(MeshError::InvalidData(
            "glTF contains no triangle geometry".into(),
        ));
    }
    let mut mesh = MeshData::new(
        mesh_name,
        vertices,
        indices,
        submeshes,
        materials,
        needs_normals,
    )?;
    if let Some(import) = armature_import {
        mesh.armature = Some(Armature {
            nodes: import.nodes,
            joints: import.joints,
            inverse_bind_matrices: import.inverse_bind_matrices,
            vertex_weights,
            bind_vertices: mesh.vertices.clone(),
        });
        mesh.animations = import.animations;
        mesh.finish_mutation(false)?;
    }
    Ok(mesh)
}

fn optional_attribute_accessor(
    attributes: &JsonMap<String, JsonValue>,
    name: &str,
    context: &str,
) -> MeshResult<Option<usize>> {
    attributes
        .get(name)
        .map(|value| {
            let value = value.as_u64().ok_or_else(|| {
                MeshError::InvalidData(format!("{context} {name} accessor is not an integer"))
            })?;
            usize::try_from(value).map_err(|_| {
                MeshError::InvalidData(format!("{context} {name} accessor is too large"))
            })
        })
        .transpose()
}

fn load_gltf_buffers(
    root: &JsonMap<String, JsonValue>,
    base_dir: Option<&Path>,
    embedded_bin: Option<&[u8]>,
) -> MeshResult<Vec<Vec<u8>>> {
    let definitions = root
        .get("buffers")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| MeshError::InvalidData("glTF buffers array is missing".into()))?;
    let mut buffers = Vec::with_capacity(definitions.len());
    let mut used_embedded_bin = false;
    for (index, definition) in definitions.iter().enumerate() {
        let context = format!("glTF buffer {index}");
        let definition = definition
            .as_object()
            .ok_or_else(|| MeshError::InvalidData(format!("{context} is not an object")))?;
        let declared_length = required_usize(definition, "byteLength", &context)?;
        let uri = definition
            .get("uri")
            .map(|value| {
                value
                    .as_str()
                    .ok_or_else(|| MeshError::InvalidData(format!("{context} URI is not a string")))
            })
            .transpose()?;
        let mut data = if let Some(uri) = uri {
            if uri.starts_with("data:") {
                decode_data_uri(uri, &context)?
            } else {
                let base_dir = base_dir.ok_or_else(|| {
                    MeshError::InvalidData(format!(
                        "{context} uses external URI '{uri}'; import from a path to resolve it"
                    ))
                })?;
                let decoded = percent_decode(uri.as_bytes(), &context)?;
                let decoded = String::from_utf8(decoded).map_err(|error| {
                    MeshError::InvalidData(format!("{context} URI is not UTF-8: {error}"))
                })?;
                if decoded.contains("://") {
                    return Err(MeshError::UnsupportedFormat(format!(
                        "{context} uses a network URI"
                    )));
                }
                let relative = PathBuf::from(decoded);
                if relative.is_absolute() {
                    return Err(MeshError::InvalidData(format!(
                        "{context} URI must be relative to the model"
                    )));
                }
                let resolved = base_dir.join(relative);
                fs::read(&resolved).map_err(|error| {
                    MeshError::Io(format!(
                        "failed to read external glTF buffer '{}': {error}",
                        resolved.display()
                    ))
                })?
            }
        } else {
            if used_embedded_bin {
                return Err(MeshError::InvalidData(
                    "more than one glTF buffer attempts to use the GLB BIN chunk".into(),
                ));
            }
            let data = embedded_bin.ok_or_else(|| {
                MeshError::InvalidData(format!("{context} has no URI or GLB BIN chunk"))
            })?;
            used_embedded_bin = true;
            data.to_vec()
        };
        if data.len() < declared_length {
            return Err(MeshError::InvalidData(format!(
                "{context} declares {declared_length} bytes but provides {}",
                data.len()
            )));
        }
        data.truncate(declared_length);
        buffers.push(data);
    }
    Ok(buffers)
}

fn decode_data_uri(uri: &str, context: &str) -> MeshResult<Vec<u8>> {
    let (metadata, payload) = uri
        .split_once(',')
        .ok_or_else(|| MeshError::InvalidData(format!("{context} has a malformed data URI")))?;
    if metadata.split(';').any(|part| part == "base64") {
        base64::engine::general_purpose::STANDARD
            .decode(payload.as_bytes())
            .map_err(|error| {
                MeshError::InvalidData(format!("{context} has invalid base64 data: {error}"))
            })
    } else {
        percent_decode(payload.as_bytes(), context)
    }
}

fn percent_decode(bytes: &[u8], context: &str) -> MeshResult<Vec<u8>> {
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let end = index.checked_add(3).ok_or_else(|| {
                MeshError::InvalidData(format!("{context} percent escape overflows"))
            })?;
            let escape = bytes.get(index + 1..end).ok_or_else(|| {
                MeshError::InvalidData(format!("{context} has a truncated percent escape"))
            })?;
            let high = decode_hex(escape[0]).ok_or_else(|| {
                MeshError::InvalidData(format!("{context} has an invalid percent escape"))
            })?;
            let low = decode_hex(escape[1]).ok_or_else(|| {
                MeshError::InvalidData(format!("{context} has an invalid percent escape"))
            })?;
            decoded.push((high << 4) | low);
            index = end;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    Ok(decoded)
}

fn decode_hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn parse_gltf_materials(root: &JsonMap<String, JsonValue>) -> MeshResult<Vec<MeshMaterial>> {
    let Some(definitions) = root.get("materials") else {
        return Ok(Vec::new());
    };
    let definitions = definitions
        .as_array()
        .ok_or_else(|| MeshError::InvalidData("glTF materials must be an array".into()))?;
    let mut materials = Vec::with_capacity(definitions.len());
    for (index, definition) in definitions.iter().enumerate() {
        let context = format!("glTF material {index}");
        let definition = definition
            .as_object()
            .ok_or_else(|| MeshError::InvalidData(format!("{context} is not an object")))?;
        let pbr = definition
            .get("pbrMetallicRoughness")
            .and_then(JsonValue::as_object);
        let mut material = MeshMaterial::named(
            definition
                .get("name")
                .and_then(JsonValue::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("Material {index}")),
        );
        material.base_color = json_float_array(
            pbr.and_then(|value| value.get("baseColorFactor")),
            [1.0; 4],
            &format!("{context} baseColorFactor"),
        )?;
        material.metallic = optional_json_f32(
            pbr.and_then(|value| value.get("metallicFactor")),
            1.0,
            &format!("{context} metallicFactor"),
        )?;
        material.roughness = optional_json_f32(
            pbr.and_then(|value| value.get("roughnessFactor")),
            1.0,
            &format!("{context} roughnessFactor"),
        )?;
        material.emissive = json_float_array(
            definition.get("emissiveFactor"),
            [0.0; 3],
            &format!("{context} emissiveFactor"),
        )?;
        material.base_color_texture = parse_texture_binding(
            root,
            pbr.and_then(|value| value.get("baseColorTexture")),
            &format!("{context} baseColorTexture"),
        )?;
        material.metallic_roughness_texture = parse_texture_binding(
            root,
            pbr.and_then(|value| value.get("metallicRoughnessTexture")),
            &format!("{context} metallicRoughnessTexture"),
        )?;
        material.normal_texture = parse_texture_binding(
            root,
            definition.get("normalTexture"),
            &format!("{context} normalTexture"),
        )?;
        material.emissive_texture = parse_texture_binding(
            root,
            definition.get("emissiveTexture"),
            &format!("{context} emissiveTexture"),
        )?;
        material.alpha_mode = match definition
            .get("alphaMode")
            .and_then(JsonValue::as_str)
            .unwrap_or("OPAQUE")
        {
            "OPAQUE" => AlphaMode::Opaque,
            "MASK" => AlphaMode::Mask,
            "BLEND" => AlphaMode::Blend,
            value => {
                return Err(MeshError::InvalidData(format!(
                    "{context} has invalid alphaMode '{value}'"
                )));
            }
        };
        material.alpha_cutoff = optional_json_f32(
            definition.get("alphaCutoff"),
            0.5,
            &format!("{context} alphaCutoff"),
        )?;
        material.double_sided = definition
            .get("doubleSided")
            .map(|value| {
                value.as_bool().ok_or_else(|| {
                    MeshError::InvalidData(format!("{context} doubleSided is not boolean"))
                })
            })
            .transpose()?
            .unwrap_or(false);
        materials.push(material);
    }
    Ok(materials)
}

fn parse_texture_binding(
    root: &JsonMap<String, JsonValue>,
    value: Option<&JsonValue>,
    context: &str,
) -> MeshResult<Option<TextureBinding>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let info = value
        .as_object()
        .ok_or_else(|| MeshError::InvalidData(format!("{context} is not an object")))?;
    let texture_index = required_usize(info, "index", context)?;
    let texture = root
        .get("textures")
        .and_then(JsonValue::as_array)
        .and_then(|textures| textures.get(texture_index))
        .and_then(JsonValue::as_object)
        .ok_or_else(|| {
            MeshError::InvalidData(format!(
                "{context} references missing texture {texture_index}"
            ))
        })?;
    let image_index = required_usize(texture, "source", context)?;
    let image = root
        .get("images")
        .and_then(JsonValue::as_array)
        .and_then(|images| images.get(image_index))
        .and_then(JsonValue::as_object)
        .ok_or_else(|| {
            MeshError::InvalidData(format!("{context} references missing image {image_index}"))
        })?;
    let source = if let Some(uri) = image.get("uri").and_then(JsonValue::as_str) {
        if uri.starts_with("data:") {
            format!("embedded-image:{image_index}")
        } else {
            uri.to_string()
        }
    } else if let Some(view) = image.get("bufferView").and_then(JsonValue::as_u64) {
        let view = usize::try_from(view).map_err(|_| {
            MeshError::InvalidData(format!("{context} image bufferView is too large"))
        })?;
        let view_exists = root
            .get("bufferViews")
            .and_then(JsonValue::as_array)
            .is_some_and(|views| view < views.len());
        if !view_exists {
            return Err(MeshError::InvalidData(format!(
                "{context} image references missing bufferView {view}"
            )));
        }
        format!("embedded-buffer-view:{view}")
    } else {
        return Err(MeshError::InvalidData(format!(
            "{context} image has neither URI nor bufferView"
        )));
    };
    let tex_coord = optional_usize(info, "texCoord", context)?.unwrap_or(0);
    Ok(Some(TextureBinding {
        source,
        tex_coord: usize_to_u32(tex_coord, "glTF texture coordinate set")?,
    }))
}

fn json_float_array<const N: usize>(
    value: Option<&JsonValue>,
    default: [f32; N],
    context: &str,
) -> MeshResult<[f32; N]> {
    let Some(value) = value else {
        return Ok(default);
    };
    let values = value
        .as_array()
        .ok_or_else(|| MeshError::InvalidData(format!("{context} is not an array")))?;
    if values.len() != N {
        return Err(MeshError::InvalidData(format!(
            "{context} must contain {N} numbers"
        )));
    }
    let mut result = [0.0; N];
    for (index, destination) in result.iter_mut().enumerate() {
        *destination = json_f32(&values[index], &format!("{context}[{index}]"))?;
    }
    Ok(result)
}

fn optional_json_f32(value: Option<&JsonValue>, default: f32, context: &str) -> MeshResult<f32> {
    value
        .map(|value| json_f32(value, context))
        .unwrap_or(Ok(default))
}

fn json_f32(value: &JsonValue, context: &str) -> MeshResult<f32> {
    let value = value
        .as_f64()
        .ok_or_else(|| MeshError::InvalidData(format!("{context} is not a number")))?;
    let converted = value as f32;
    if !converted.is_finite() {
        return Err(MeshError::InvalidData(format!(
            "{context} is outside the finite f32 range"
        )));
    }
    Ok(converted)
}

fn required_usize(
    object: &JsonMap<String, JsonValue>,
    field: &str,
    context: &str,
) -> MeshResult<usize> {
    optional_usize(object, field, context)?
        .ok_or_else(|| MeshError::InvalidData(format!("{context} is missing {field}")))
}

fn optional_usize(
    object: &JsonMap<String, JsonValue>,
    field: &str,
    context: &str,
) -> MeshResult<Option<usize>> {
    object
        .get(field)
        .map(|value| {
            let value = value.as_u64().ok_or_else(|| {
                MeshError::InvalidData(format!("{context} {field} is not an unsigned integer"))
            })?;
            usize::try_from(value).map_err(|_| {
                MeshError::InvalidData(format!("{context} {field} cannot fit in memory"))
            })
        })
        .transpose()
}

struct AccessorView<'a> {
    bytes: &'a [u8],
    start: usize,
    stride: usize,
    count: usize,
    components: usize,
    component_type: u32,
    component_size: usize,
    normalized: bool,
    context: String,
}

impl<'a> AccessorView<'a> {
    fn new(
        root: &JsonMap<String, JsonValue>,
        buffers: &'a [Vec<u8>],
        accessor_index: usize,
        expected_type: &str,
        context: &str,
    ) -> MeshResult<Self> {
        let accessor = root
            .get("accessors")
            .and_then(JsonValue::as_array)
            .and_then(|accessors| accessors.get(accessor_index))
            .and_then(JsonValue::as_object)
            .ok_or_else(|| {
                MeshError::InvalidData(format!(
                    "{context} references missing accessor {accessor_index}"
                ))
            })?;
        if accessor.contains_key("sparse") {
            return Err(MeshError::UnsupportedFormat(format!(
                "{context} uses a sparse accessor"
            )));
        }
        let accessor_type = accessor
            .get("type")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| MeshError::InvalidData(format!("{context} has no accessor type")))?;
        if accessor_type != expected_type {
            return Err(MeshError::InvalidData(format!(
                "{context} accessor type is {accessor_type}, expected {expected_type}"
            )));
        }
        let components = accessor_components(accessor_type).ok_or_else(|| {
            MeshError::InvalidData(format!("{context} has unsupported accessor type"))
        })?;
        let component_type = u32::try_from(required_usize(accessor, "componentType", context)?)
            .map_err(|_| {
                MeshError::InvalidData(format!("{context} component type is too large"))
            })?;
        let component_size = component_size(component_type).ok_or_else(|| {
            MeshError::InvalidData(format!(
                "{context} uses unsupported component type {component_type}"
            ))
        })?;
        let normalized = accessor
            .get("normalized")
            .map(|value| {
                value.as_bool().ok_or_else(|| {
                    MeshError::InvalidData(format!("{context} normalized is not boolean"))
                })
            })
            .transpose()?
            .unwrap_or(false);
        if normalized && matches!(component_type, 5125 | 5126) {
            return Err(MeshError::InvalidData(format!(
                "{context} cannot normalize this component type"
            )));
        }
        let count = required_usize(accessor, "count", context)?;
        let view_index = required_usize(accessor, "bufferView", context)?;
        let view = root
            .get("bufferViews")
            .and_then(JsonValue::as_array)
            .and_then(|views| views.get(view_index))
            .and_then(JsonValue::as_object)
            .ok_or_else(|| {
                MeshError::InvalidData(format!(
                    "{context} references missing bufferView {view_index}"
                ))
            })?;
        let buffer_index = required_usize(view, "buffer", context)?;
        let buffer = buffers.get(buffer_index).ok_or_else(|| {
            MeshError::InvalidData(format!(
                "{context} references missing buffer {buffer_index}"
            ))
        })?;
        let view_offset = optional_usize(view, "byteOffset", context)?.unwrap_or(0);
        let view_length = required_usize(view, "byteLength", context)?;
        let view_end = view_offset.checked_add(view_length).ok_or_else(|| {
            MeshError::InvalidData(format!("{context} bufferView range overflows"))
        })?;
        if view_end > buffer.len() {
            return Err(MeshError::InvalidData(format!(
                "{context} bufferView exceeds buffer {buffer_index}"
            )));
        }
        let element_size = component_size
            .checked_mul(components)
            .ok_or_else(|| MeshError::InvalidData(format!("{context} element size overflows")))?;
        let stride = optional_usize(view, "byteStride", context)?.unwrap_or(element_size);
        if stride < element_size || stride > 252 || stride % component_size != 0 {
            return Err(MeshError::InvalidData(format!(
                "{context} byteStride {stride} is invalid for {element_size}-byte elements"
            )));
        }
        let accessor_offset = optional_usize(accessor, "byteOffset", context)?.unwrap_or(0);
        let occupied = if count == 0 {
            0
        } else {
            stride
                .checked_mul(count - 1)
                .and_then(|value| value.checked_add(element_size))
                .ok_or_else(|| {
                    MeshError::InvalidData(format!("{context} accessor range overflows"))
                })?
        };
        let accessor_end = accessor_offset.checked_add(occupied).ok_or_else(|| {
            MeshError::InvalidData(format!("{context} accessor offset overflows"))
        })?;
        if accessor_end > view_length {
            return Err(MeshError::InvalidData(format!(
                "{context} accessor exceeds its bufferView"
            )));
        }
        let start = view_offset.checked_add(accessor_offset).ok_or_else(|| {
            MeshError::InvalidData(format!("{context} absolute offset overflows"))
        })?;
        if start % component_size != 0 {
            return Err(MeshError::InvalidData(format!(
                "{context} offset is not aligned to its component size"
            )));
        }
        Ok(Self {
            bytes: buffer,
            start,
            stride,
            count,
            components,
            component_type,
            component_size,
            normalized,
            context: context.to_string(),
        })
    }

    fn read_vec<const N: usize>(&self, index: usize) -> MeshResult<[f32; N]> {
        if self.components != N {
            return Err(MeshError::InvalidData(format!(
                "{} has {} components, expected {N}",
                self.context, self.components
            )));
        }
        if index >= self.count {
            return Err(MeshError::InvalidData(format!(
                "{} element {index} is out of range",
                self.context
            )));
        }
        let mut output = [0.0; N];
        for (component, destination) in output.iter_mut().enumerate() {
            *destination = self.read_component(index, component)?;
        }
        Ok(output)
    }

    fn read_index(&self, index: usize) -> MeshResult<u32> {
        if self.components != 1 || index >= self.count {
            return Err(MeshError::InvalidData(format!(
                "{} index element {index} is out of range",
                self.context
            )));
        }
        let offset = self.element_component_offset(index, 0)?;
        match self.component_type {
            5121 => Ok(u32::from(self.bytes[offset])),
            5123 => Ok(u32::from(read_u16_le(self.bytes, offset, &self.context)?)),
            5125 => read_u32_le(self.bytes, offset, &self.context),
            _ => Err(MeshError::InvalidData(format!(
                "{} is not an unsigned index accessor",
                self.context
            ))),
        }
    }

    fn read_component(&self, index: usize, component: usize) -> MeshResult<f32> {
        let offset = self.element_component_offset(index, component)?;
        let value = match self.component_type {
            5120 => {
                let value = self.bytes[offset] as i8;
                if self.normalized {
                    (f32::from(value) / 127.0).max(-1.0)
                } else {
                    f32::from(value)
                }
            }
            5121 => {
                let value = self.bytes[offset];
                if self.normalized {
                    f32::from(value) / 255.0
                } else {
                    f32::from(value)
                }
            }
            5122 => {
                let value = read_i16_le(self.bytes, offset, &self.context)?;
                if self.normalized {
                    (f32::from(value) / 32767.0).max(-1.0)
                } else {
                    f32::from(value)
                }
            }
            5123 => {
                let value = read_u16_le(self.bytes, offset, &self.context)?;
                if self.normalized {
                    f32::from(value) / 65535.0
                } else {
                    f32::from(value)
                }
            }
            5125 => read_u32_le(self.bytes, offset, &self.context)? as f32,
            5126 => read_f32_le(self.bytes, offset, &self.context)?,
            _ => {
                return Err(MeshError::InvalidData(format!(
                    "{} has an unsupported component type",
                    self.context
                )));
            }
        };
        if !value.is_finite() {
            return Err(MeshError::InvalidData(format!(
                "{} contains a non-finite component",
                self.context
            )));
        }
        Ok(value)
    }

    fn element_component_offset(&self, index: usize, component: usize) -> MeshResult<usize> {
        if index >= self.count || component >= self.components {
            return Err(MeshError::InvalidData(format!(
                "{} component access is out of range",
                self.context
            )));
        }
        self.start
            .checked_add(self.stride.checked_mul(index).ok_or_else(|| {
                MeshError::InvalidData(format!("{} element offset overflows", self.context))
            })?)
            .and_then(|offset| {
                self.component_size
                    .checked_mul(component)
                    .and_then(|component_offset| offset.checked_add(component_offset))
            })
            .ok_or_else(|| {
                MeshError::InvalidData(format!("{} component offset overflows", self.context))
            })
    }
}

fn accessor_components(value: &str) -> Option<usize> {
    match value {
        "SCALAR" => Some(1),
        "VEC2" => Some(2),
        "VEC3" => Some(3),
        "VEC4" => Some(4),
        "MAT2" => Some(4),
        "MAT3" => Some(9),
        "MAT4" => Some(16),
        _ => None,
    }
}

fn component_size(component_type: u32) -> Option<usize> {
    match component_type {
        5120 | 5121 => Some(1),
        5122 | 5123 => Some(2),
        5125 | 5126 => Some(4),
        _ => None,
    }
}

fn read_u16_le(bytes: &[u8], offset: usize, context: &str) -> MeshResult<u16> {
    let data = bytes
        .get(offset..offset.saturating_add(2))
        .ok_or_else(|| MeshError::InvalidData(format!("{context} ends inside a u16")))?;
    let mut array = [0u8; 2];
    array.copy_from_slice(data);
    Ok(u16::from_le_bytes(array))
}

fn read_i16_le(bytes: &[u8], offset: usize, context: &str) -> MeshResult<i16> {
    let data = bytes
        .get(offset..offset.saturating_add(2))
        .ok_or_else(|| MeshError::InvalidData(format!("{context} ends inside an i16")))?;
    let mut array = [0u8; 2];
    array.copy_from_slice(data);
    Ok(i16::from_le_bytes(array))
}

fn read_u32_le(bytes: &[u8], offset: usize, context: &str) -> MeshResult<u32> {
    let data = bytes
        .get(offset..offset.saturating_add(4))
        .ok_or_else(|| MeshError::InvalidData(format!("{context} ends inside a u32")))?;
    let mut array = [0u8; 4];
    array.copy_from_slice(data);
    Ok(u32::from_le_bytes(array))
}

fn read_f32_le(bytes: &[u8], offset: usize, context: &str) -> MeshResult<f32> {
    let data = bytes
        .get(offset..offset.saturating_add(4))
        .ok_or_else(|| MeshError::InvalidData(format!("{context} ends inside an f32")))?;
    let mut array = [0u8; 4];
    array.copy_from_slice(data);
    Ok(f32::from_le_bytes(array))
}

fn read_i32_le(bytes: &[u8], offset: usize, context: &str) -> MeshResult<i32> {
    let data = bytes
        .get(offset..offset.saturating_add(4))
        .ok_or_else(|| MeshError::InvalidData(format!("{context} ends inside an i32")))?;
    let mut array = [0u8; 4];
    array.copy_from_slice(data);
    Ok(i32::from_le_bytes(array))
}

fn read_u64_le(bytes: &[u8], offset: usize, context: &str) -> MeshResult<u64> {
    let data = bytes
        .get(offset..offset.saturating_add(8))
        .ok_or_else(|| MeshError::InvalidData(format!("{context} ends inside a u64")))?;
    let mut array = [0u8; 8];
    array.copy_from_slice(data);
    Ok(u64::from_le_bytes(array))
}

fn read_i64_le(bytes: &[u8], offset: usize, context: &str) -> MeshResult<i64> {
    let data = bytes
        .get(offset..offset.saturating_add(8))
        .ok_or_else(|| MeshError::InvalidData(format!("{context} ends inside an i64")))?;
    let mut array = [0u8; 8];
    array.copy_from_slice(data);
    Ok(i64::from_le_bytes(array))
}

fn read_f64_le(bytes: &[u8], offset: usize, context: &str) -> MeshResult<f64> {
    let data = bytes
        .get(offset..offset.saturating_add(8))
        .ok_or_else(|| MeshError::InvalidData(format!("{context} ends inside an f64")))?;
    let mut array = [0u8; 8];
    array.copy_from_slice(data);
    Ok(f64::from_le_bytes(array))
}

const BINARY_FBX_MAGIC: &[u8; 23] = b"Kaydara FBX Binary  \0\x1a\0";
const MAX_BINARY_FBX_DEPTH: usize = 128;
const MAX_BINARY_FBX_ARRAY_BYTES: usize = 256 * 1024 * 1024;

fn parse_fbx(bytes: &[u8]) -> MeshResult<MeshData> {
    if bytes.starts_with(BINARY_FBX_MAGIC) || bytes.starts_with(b"Kaydara FBX Binary") {
        parse_binary_fbx(bytes)
    } else {
        parse_ascii_fbx(bytes)
    }
}

#[derive(Debug)]
struct BinaryFbxGeometry {
    name: String,
    positions: Option<Vec<f64>>,
    polygon_indices: Option<Vec<i64>>,
}

#[derive(Debug)]
enum BinaryFbxNumericArray {
    Floating(Vec<f64>),
    Integer(Vec<i64>),
}

impl BinaryFbxNumericArray {
    fn into_positions(self) -> Vec<f64> {
        match self {
            Self::Floating(values) => values,
            Self::Integer(values) => values.into_iter().map(|value| value as f64).collect(),
        }
    }

    fn into_polygon_indices(self, context: &str) -> MeshResult<Vec<i64>> {
        match self {
            Self::Integer(values) => Ok(values),
            Self::Floating(values) => values
                .into_iter()
                .enumerate()
                .map(|(index, value)| {
                    if !value.is_finite()
                        || value.fract() != 0.0
                        || value < i64::MIN as f64
                        // `i64::MAX as f64` rounds up to 2^63, so that upper
                        // endpoint must be excluded rather than compared with
                        // `>` before the saturating float-to-int cast.
                        || value >= -(i64::MIN as f64)
                    {
                        return Err(MeshError::InvalidData(format!(
                            "{context} entry {index} is not an integer"
                        )));
                    }
                    Ok(value as i64)
                })
                .collect(),
        }
    }
}

#[derive(Debug)]
enum BinaryFbxProperty {
    Text(String),
    NumericArray(BinaryFbxNumericArray),
}

#[derive(Debug)]
struct BinaryFbxNodeHeader {
    end: usize,
    property_count: usize,
    properties_start: usize,
    properties_end: usize,
    name: String,
}

struct BinaryFbxParser<'a> {
    bytes: &'a [u8],
    version: u32,
    wide_records: bool,
    geometries: Vec<BinaryFbxGeometry>,
    has_deformation: bool,
}

impl<'a> BinaryFbxParser<'a> {
    fn new(bytes: &'a [u8]) -> MeshResult<Self> {
        if !bytes.starts_with(BINARY_FBX_MAGIC) {
            return Err(MeshError::InvalidData(
                "binary FBX header is truncated or has an invalid magic sequence".into(),
            ));
        }
        let version = read_u32_le(bytes, BINARY_FBX_MAGIC.len(), "binary FBX header")?;
        if version < 6000 {
            return Err(MeshError::UnsupportedFormat(format!(
                "binary FBX version {version} predates the supported node format"
            )));
        }
        Ok(Self {
            bytes,
            version,
            wide_records: version >= 7500,
            geometries: Vec::new(),
            has_deformation: false,
        })
    }

    fn header_len(&self) -> usize {
        if self.wide_records { 25 } else { 13 }
    }

    fn is_null_record(&self, offset: usize, limit: usize) -> MeshResult<bool> {
        let end = offset.checked_add(self.header_len()).ok_or_else(|| {
            MeshError::InvalidData("binary FBX null-record range overflows".into())
        })?;
        let record = self.bytes.get(offset..end).ok_or_else(|| {
            MeshError::InvalidData("binary FBX ends inside a node record header".into())
        })?;
        if end > limit {
            return Err(MeshError::InvalidData(
                "binary FBX node record crosses its parent boundary".into(),
            ));
        }
        Ok(record.iter().all(|byte| *byte == 0))
    }

    fn node_header(&self, offset: usize, parent_end: usize) -> MeshResult<BinaryFbxNodeHeader> {
        let context = format!("binary FBX {} node at byte {offset}", self.version);
        let (end_offset, property_count, property_list_len) = if self.wide_records {
            (
                read_u64_le(self.bytes, offset, &context)?,
                read_u64_le(self.bytes, offset + 8, &context)?,
                read_u64_le(self.bytes, offset + 16, &context)?,
            )
        } else {
            (
                u64::from(read_u32_le(self.bytes, offset, &context)?),
                u64::from(read_u32_le(self.bytes, offset + 4, &context)?),
                u64::from(read_u32_le(self.bytes, offset + 8, &context)?),
            )
        };
        let name_len_offset = offset
            .checked_add(self.header_len() - 1)
            .ok_or_else(|| MeshError::InvalidData(format!("{context} header overflows")))?;
        let name_len = usize::from(*self.bytes.get(name_len_offset).ok_or_else(|| {
            MeshError::InvalidData(format!("{context} ends before its name length"))
        })?);
        if end_offset == 0 || name_len == 0 {
            return Err(MeshError::InvalidData(format!(
                "{context} is neither a node nor a complete null record"
            )));
        }
        let end = usize::try_from(end_offset).map_err(|_| {
            MeshError::InvalidData(format!("{context} end offset does not fit in memory"))
        })?;
        if end <= offset || end > parent_end || end > self.bytes.len() {
            return Err(MeshError::InvalidData(format!(
                "{context} end offset {end} is outside its parent boundary {parent_end}"
            )));
        }
        let property_count = usize::try_from(property_count).map_err(|_| {
            MeshError::InvalidData(format!("{context} property count does not fit in memory"))
        })?;
        let property_list_len = usize::try_from(property_list_len).map_err(|_| {
            MeshError::InvalidData(format!("{context} property length does not fit in memory"))
        })?;
        if property_count > property_list_len {
            return Err(MeshError::InvalidData(format!(
                "{context} declares more properties than property bytes"
            )));
        }
        let name_start = offset
            .checked_add(self.header_len())
            .ok_or_else(|| MeshError::InvalidData(format!("{context} name offset overflows")))?;
        let name_end = name_start
            .checked_add(name_len)
            .ok_or_else(|| MeshError::InvalidData(format!("{context} name range overflows")))?;
        let name_bytes = self.bytes.get(name_start..name_end).ok_or_else(|| {
            MeshError::InvalidData(format!("{context} ends inside its node name"))
        })?;
        let name = std::str::from_utf8(name_bytes)
            .map_err(|error| {
                MeshError::InvalidData(format!("{context} name is not UTF-8: {error}"))
            })?
            .to_string();
        let properties_end = name_end
            .checked_add(property_list_len)
            .ok_or_else(|| MeshError::InvalidData(format!("{context} property range overflows")))?;
        if properties_end > end {
            return Err(MeshError::InvalidData(format!(
                "{context} properties extend beyond node end offset"
            )));
        }
        Ok(BinaryFbxNodeHeader {
            end,
            property_count,
            properties_start: name_end,
            properties_end,
            name,
        })
    }

    fn parse_document(&mut self) -> MeshResult<()> {
        let mut cursor = BINARY_FBX_MAGIC.len() + 4;
        loop {
            if self.is_null_record(cursor, self.bytes.len())? {
                return Ok(());
            }
            cursor = self.parse_node(cursor, self.bytes.len(), 0, None)?;
        }
    }

    fn parse_node(
        &mut self,
        offset: usize,
        parent_end: usize,
        depth: usize,
        inherited_geometry: Option<usize>,
    ) -> MeshResult<usize> {
        if depth >= MAX_BINARY_FBX_DEPTH {
            return Err(MeshError::InvalidData(format!(
                "binary FBX nesting exceeds {MAX_BINARY_FBX_DEPTH} levels"
            )));
        }
        let header = self.node_header(offset, parent_end)?;
        if matches!(
            header.name.as_str(),
            "Deformer"
                | "AnimationStack"
                | "AnimationLayer"
                | "AnimationCurveNode"
                | "AnimationCurve"
        ) {
            self.has_deformation = true;
        }
        let capture_text = header.name == "Geometry";
        let capture_numeric = matches!(header.name.as_str(), "Vertices" | "PolygonVertexIndex");
        let properties = self.parse_properties(&header, capture_text, capture_numeric)?;

        let mut geometry_index = inherited_geometry;
        if header.name == "Geometry" {
            geometry_index = None;
            let strings = properties
                .iter()
                .filter_map(|property| match property {
                    BinaryFbxProperty::Text(value) => Some(value.as_str()),
                    BinaryFbxProperty::NumericArray(_) => None,
                })
                .collect::<Vec<_>>();
            if strings.get(1).is_some_and(|kind| *kind == "Mesh") {
                let name = strings
                    .first()
                    .map(|name| name.strip_prefix("Geometry::").unwrap_or(name).to_string())
                    .unwrap_or_else(|| format!("Geometry {}", self.geometries.len()));
                geometry_index = Some(self.geometries.len());
                self.geometries.push(BinaryFbxGeometry {
                    name,
                    positions: None,
                    polygon_indices: None,
                });
            }
        } else if let Some(index) = geometry_index
            && (header.name == "Vertices" || header.name == "PolygonVertexIndex")
        {
            let array = properties
                .into_iter()
                .find_map(|property| match property {
                    BinaryFbxProperty::NumericArray(array) => Some(array),
                    BinaryFbxProperty::Text(_) => None,
                })
                .ok_or_else(|| {
                    MeshError::InvalidData(format!(
                        "binary FBX {} node has no numeric array property",
                        header.name
                    ))
                })?;
            let geometry = self.geometries.get_mut(index).ok_or_else(|| {
                MeshError::InvalidData("binary FBX geometry context is invalid".into())
            })?;
            if header.name == "Vertices" {
                if geometry.positions.is_some() {
                    return Err(MeshError::InvalidData(format!(
                        "binary FBX geometry '{}' has duplicate Vertices nodes",
                        geometry.name
                    )));
                }
                geometry.positions = Some(array.into_positions());
            } else {
                if geometry.polygon_indices.is_some() {
                    return Err(MeshError::InvalidData(format!(
                        "binary FBX geometry '{}' has duplicate PolygonVertexIndex nodes",
                        geometry.name
                    )));
                }
                geometry.polygon_indices =
                    Some(array.into_polygon_indices("FBX PolygonVertexIndex array")?);
            }
        }

        let mut child_cursor = header.properties_end;
        while child_cursor < header.end {
            if self.is_null_record(child_cursor, header.end)? {
                child_cursor = child_cursor.checked_add(self.header_len()).ok_or_else(|| {
                    MeshError::InvalidData("binary FBX null-record end overflows".into())
                })?;
                if child_cursor != header.end {
                    return Err(MeshError::InvalidData(format!(
                        "binary FBX node '{}' has data after its child terminator",
                        header.name
                    )));
                }
                break;
            }
            child_cursor = self.parse_node(child_cursor, header.end, depth + 1, geometry_index)?;
        }
        if child_cursor != header.end {
            return Err(MeshError::InvalidData(format!(
                "binary FBX node '{}' does not end at its declared offset",
                header.name
            )));
        }
        Ok(header.end)
    }

    fn parse_properties(
        &self,
        header: &BinaryFbxNodeHeader,
        capture_text: bool,
        capture_numeric: bool,
    ) -> MeshResult<Vec<BinaryFbxProperty>> {
        let mut cursor = header.properties_start;
        let mut captured = Vec::new();
        for property_index in 0..header.property_count {
            let type_code = *self.bytes.get(cursor).ok_or_else(|| {
                MeshError::InvalidData(format!(
                    "binary FBX node '{}' ends before property {property_index}",
                    header.name
                ))
            })?;
            cursor += 1;
            let fixed_size = match type_code {
                b'Y' => Some(2),
                b'C' => Some(1),
                b'I' | b'F' => Some(4),
                b'D' | b'L' => Some(8),
                _ => None,
            };
            if let Some(size) = fixed_size {
                cursor = cursor.checked_add(size).ok_or_else(|| {
                    MeshError::InvalidData("binary FBX scalar property range overflows".into())
                })?;
            } else if matches!(type_code, b'S' | b'R') {
                let length = usize::try_from(read_u32_le(
                    self.bytes,
                    cursor,
                    "binary FBX string/raw property",
                )?)
                .map_err(|_| {
                    MeshError::InvalidData(
                        "binary FBX property length does not fit in memory".into(),
                    )
                })?;
                cursor += 4;
                let end = cursor.checked_add(length).ok_or_else(|| {
                    MeshError::InvalidData("binary FBX string/raw property range overflows".into())
                })?;
                if end > header.properties_end {
                    return Err(MeshError::InvalidData(format!(
                        "binary FBX node '{}' string/raw property exceeds its declared property list",
                        header.name
                    )));
                }
                let value = self.bytes.get(cursor..end).ok_or_else(|| {
                    MeshError::InvalidData("binary FBX ends inside a string/raw property".into())
                })?;
                if capture_text && type_code == b'S' {
                    captured.push(BinaryFbxProperty::Text(
                        std::str::from_utf8(value)
                            .map_err(|error| {
                                MeshError::InvalidData(format!(
                                    "binary FBX string property is not UTF-8: {error}"
                                ))
                            })?
                            .to_string(),
                    ));
                }
                cursor = end;
            } else if matches!(type_code, b'f' | b'd' | b'i' | b'l' | b'b') {
                let length = usize::try_from(read_u32_le(
                    self.bytes,
                    cursor,
                    "binary FBX numeric array length",
                )?)
                .map_err(|_| {
                    MeshError::InvalidData("binary FBX array length does not fit in memory".into())
                })?;
                let encoding =
                    read_u32_le(self.bytes, cursor + 4, "binary FBX numeric array encoding")?;
                let payload_len = usize::try_from(read_u32_le(
                    self.bytes,
                    cursor + 8,
                    "binary FBX numeric array payload length",
                )?)
                .map_err(|_| {
                    MeshError::InvalidData("binary FBX array payload does not fit in memory".into())
                })?;
                cursor += 12;
                let payload_end = cursor.checked_add(payload_len).ok_or_else(|| {
                    MeshError::InvalidData("binary FBX array payload range overflows".into())
                })?;
                if payload_end > header.properties_end {
                    return Err(MeshError::InvalidData(format!(
                        "binary FBX node '{}' numeric array exceeds its declared property list",
                        header.name
                    )));
                }
                let payload = self.bytes.get(cursor..payload_end).ok_or_else(|| {
                    MeshError::InvalidData("binary FBX ends inside a numeric array payload".into())
                })?;
                let element_size = match type_code {
                    b'f' | b'i' => 4usize,
                    b'd' | b'l' => 8,
                    b'b' => 1,
                    _ => unreachable!("numeric array type was matched above"),
                };
                let raw_len = length.checked_mul(element_size).ok_or_else(|| {
                    MeshError::InvalidData("binary FBX raw array size overflows".into())
                })?;
                match encoding {
                    0 if payload.len() != raw_len => {
                        return Err(MeshError::InvalidData(format!(
                            "raw binary FBX array has {} bytes, expected {raw_len}",
                            payload.len()
                        )));
                    }
                    0 | 1 => {}
                    other => {
                        return Err(MeshError::UnsupportedFormat(format!(
                            "binary FBX array encoding {other} is unsupported"
                        )));
                    }
                }
                if capture_numeric
                    && !captured
                        .iter()
                        .any(|value| matches!(value, BinaryFbxProperty::NumericArray(_)))
                {
                    captured.push(BinaryFbxProperty::NumericArray(decode_binary_fbx_array(
                        type_code, length, encoding, payload,
                    )?));
                }
                cursor = payload_end;
            } else {
                let printable = if type_code.is_ascii_graphic() {
                    char::from(type_code).to_string()
                } else {
                    format!("0x{type_code:02x}")
                };
                return Err(MeshError::UnsupportedFormat(format!(
                    "binary FBX node '{}' uses unknown property type {printable}",
                    header.name
                )));
            }
            if cursor > header.properties_end {
                return Err(MeshError::InvalidData(format!(
                    "binary FBX node '{}' property {property_index} exceeds its declared property list",
                    header.name
                )));
            }
        }
        if cursor != header.properties_end {
            return Err(MeshError::InvalidData(format!(
                "binary FBX node '{}' property count/length disagree",
                header.name
            )));
        }
        Ok(captured)
    }
}

fn decode_binary_fbx_array(
    type_code: u8,
    length: usize,
    encoding: u32,
    payload: &[u8],
) -> MeshResult<BinaryFbxNumericArray> {
    let element_size = match type_code {
        b'f' | b'i' => 4usize,
        b'd' | b'l' => 8,
        b'b' => 1,
        _ => {
            return Err(MeshError::UnsupportedFormat(format!(
                "unsupported binary FBX array type 0x{type_code:02x}"
            )));
        }
    };
    let expected_len = length
        .checked_mul(element_size)
        .ok_or_else(|| MeshError::InvalidData("binary FBX expanded array size overflows".into()))?;
    if expected_len > MAX_BINARY_FBX_ARRAY_BYTES {
        return Err(MeshError::InvalidData(format!(
            "binary FBX expanded array is {expected_len} bytes; limit is {MAX_BINARY_FBX_ARRAY_BYTES}"
        )));
    }
    let expanded;
    let bytes = match encoding {
        0 => {
            if payload.len() != expected_len {
                return Err(MeshError::InvalidData(format!(
                    "raw binary FBX array has {} bytes, expected {expected_len}",
                    payload.len()
                )));
            }
            payload
        }
        1 => {
            let limit = expected_len.checked_add(1).ok_or_else(|| {
                MeshError::InvalidData("binary FBX decompression limit overflows".into())
            })?;
            let mut decoder = ZlibDecoder::new(payload).take(limit as u64);
            let mut output = Vec::with_capacity(expected_len.min(1024 * 1024));
            decoder.read_to_end(&mut output).map_err(|error| {
                MeshError::InvalidData(format!("binary FBX zlib array is invalid: {error}"))
            })?;
            if output.len() != expected_len {
                return Err(MeshError::InvalidData(format!(
                    "binary FBX zlib array expands to {} bytes, expected {expected_len}",
                    output.len()
                )));
            }
            expanded = output;
            expanded.as_slice()
        }
        other => {
            return Err(MeshError::UnsupportedFormat(format!(
                "binary FBX array encoding {other} is unsupported"
            )));
        }
    };

    match type_code {
        b'f' => (0..length)
            .map(|index| {
                read_f32_le(bytes, index * 4, "binary FBX float array").map(|value| value as f64)
            })
            .collect::<MeshResult<Vec<_>>>()
            .map(BinaryFbxNumericArray::Floating),
        b'd' => (0..length)
            .map(|index| read_f64_le(bytes, index * 8, "binary FBX double array"))
            .collect::<MeshResult<Vec<_>>>()
            .map(BinaryFbxNumericArray::Floating),
        b'i' => (0..length)
            .map(|index| read_i32_le(bytes, index * 4, "binary FBX integer array").map(i64::from))
            .collect::<MeshResult<Vec<_>>>()
            .map(BinaryFbxNumericArray::Integer),
        b'l' => (0..length)
            .map(|index| read_i64_le(bytes, index * 8, "binary FBX long array"))
            .collect::<MeshResult<Vec<_>>>()
            .map(BinaryFbxNumericArray::Integer),
        b'b' => Ok(BinaryFbxNumericArray::Integer(
            bytes.iter().map(|value| i64::from(*value)).collect(),
        )),
        _ => unreachable!("array type was validated above"),
    }
}

fn parse_binary_fbx(bytes: &[u8]) -> MeshResult<MeshData> {
    let mut parser = BinaryFbxParser::new(bytes)?;
    parser.parse_document()?;
    if parser.has_deformation {
        return Err(MeshError::UnsupportedFormat(
            "binary FBX skin/animation data is not supported yet; use ASCII FBX 7.x or glTF 2.0 for armatures"
                .into(),
        ));
    }
    if parser.geometries.is_empty() {
        return Err(MeshError::InvalidData(
            "binary FBX contains no mesh Geometry nodes".into(),
        ));
    }

    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut submeshes = Vec::new();
    let first_name = parser.geometries[0].name.clone();
    for geometry in parser.geometries {
        let positions = geometry.positions.ok_or_else(|| {
            MeshError::InvalidData(format!(
                "binary FBX geometry '{}' has no Vertices array",
                geometry.name
            ))
        })?;
        let polygon_indices = geometry.polygon_indices.ok_or_else(|| {
            MeshError::InvalidData(format!(
                "binary FBX geometry '{}' has no PolygonVertexIndex array",
                geometry.name
            ))
        })?;
        append_fbx_geometry(
            &mut vertices,
            &mut indices,
            &mut submeshes,
            geometry.name,
            &positions,
            &polygon_indices,
        )?;
    }
    MeshData::new(first_name, vertices, indices, submeshes, Vec::new(), true)
}

fn parse_ascii_fbx(bytes: &[u8]) -> MeshResult<MeshData> {
    let source = std::str::from_utf8(bytes)
        .map_err(|error| MeshError::InvalidData(format!("FBX is not UTF-8 text: {error}")))?;
    if !source.is_ascii() {
        return Err(MeshError::UnsupportedFormat(
            "FBX has neither the binary magic header nor valid ASCII contents".into(),
        ));
    }

    let mut vertices = Vec::<Vertex>::new();
    let mut indices = Vec::<u32>::new();
    let mut submeshes = Vec::<Submesh>::new();
    let mut search_offset = 0usize;
    let mut first_name = None;
    let mut geometry_ranges = Vec::<AsciiFbxGeometryRange>::new();

    while let Some(relative_marker) = source[search_offset..].find("Geometry:") {
        let marker = search_offset + relative_marker;
        let Some(relative_open) = source[marker..].find('{') else {
            return Err(MeshError::InvalidData(
                "FBX Geometry block has no opening brace".into(),
            ));
        };
        let open = marker + relative_open;
        let close = find_matching_brace(source.as_bytes(), open, "FBX Geometry")?;
        let header = &source[marker..open];
        search_offset = close + 1;
        if !header.contains("\"Mesh\"") {
            continue;
        }
        let name =
            fbx_geometry_name(header).unwrap_or_else(|| format!("Geometry {}", submeshes.len()));
        let geometry_id = fbx_object_id(header, "Geometry")?;
        if first_name.is_none() {
            first_name = Some(name.clone());
        }
        let block = &source[open + 1..close];
        let position_tokens = fbx_array_tokens(block, "Vertices")?.ok_or_else(|| {
            MeshError::InvalidData(format!("FBX geometry '{name}' has no Vertices array"))
        })?;
        let polygon_tokens = fbx_array_tokens(block, "PolygonVertexIndex")?.ok_or_else(|| {
            MeshError::InvalidData(format!(
                "FBX geometry '{name}' has no PolygonVertexIndex array"
            ))
        })?;
        let positions = position_tokens
            .iter()
            .enumerate()
            .map(|(position_index, token)| {
                token.parse::<f64>().map_err(|error| {
                    MeshError::InvalidData(format!(
                        "FBX geometry '{name}' position {position_index} is invalid: {error}"
                    ))
                })
            })
            .collect::<MeshResult<Vec<_>>>()?;
        let polygon_indices = polygon_tokens
            .iter()
            .enumerate()
            .map(|(polygon_index, token)| {
                token.parse::<i64>().map_err(|error| {
                    MeshError::InvalidData(format!(
                        "FBX geometry '{name}' polygon index {polygon_index} is invalid: {error}"
                    ))
                })
            })
            .collect::<MeshResult<Vec<_>>>()?;
        let base_vertex = vertices.len();
        append_fbx_geometry(
            &mut vertices,
            &mut indices,
            &mut submeshes,
            name,
            &positions,
            &polygon_indices,
        )?;
        geometry_ranges.push(AsciiFbxGeometryRange {
            id: geometry_id,
            base_vertex,
            vertex_count: positions.len() / 3,
        });
    }

    if submeshes.is_empty() {
        return Err(MeshError::InvalidData(
            "ASCII FBX contains no mesh Geometry blocks".into(),
        ));
    }
    let mesh = MeshData::new(
        first_name.unwrap_or_else(|| "FBX Mesh".to_string()),
        vertices,
        indices,
        submeshes,
        Vec::new(),
        true,
    )?;
    attach_ascii_fbx_armature(source, mesh, &geometry_ranges)
}

#[derive(Clone, Copy, Debug)]
struct AsciiFbxGeometryRange {
    id: i64,
    base_vertex: usize,
    vertex_count: usize,
}

#[derive(Clone, Debug)]
struct AsciiFbxObject<'a> {
    id: i64,
    name: String,
    kind: String,
    block: &'a str,
}

#[derive(Clone, Debug)]
struct AsciiFbxConnection {
    kind: String,
    child: i64,
    parent: i64,
    property: Option<String>,
}

fn fbx_object_id(header: &str, label: &str) -> MeshResult<i64> {
    let marker = format!("{label}:");
    let value = header
        .split_once(&marker)
        .map(|(_, value)| value)
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .ok_or_else(|| MeshError::InvalidData(format!("FBX {label} header has no object ID")))?;
    value.parse::<i64>().map_err(|error| {
        MeshError::InvalidData(format!(
            "FBX {label} object ID '{value}' is invalid: {error}"
        ))
    })
}

fn fbx_quoted_strings(source: &str) -> Vec<String> {
    let mut output = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let mut escaped = false;
    for character in source.chars() {
        if in_string {
            if escaped {
                current.push(character);
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                output.push(std::mem::take(&mut current));
                in_string = false;
            } else {
                current.push(character);
            }
        } else if character == '"' {
            in_string = true;
        }
    }
    output
}

fn fbx_ascii_objects<'a>(source: &'a str, label: &str) -> MeshResult<Vec<AsciiFbxObject<'a>>> {
    let marker_text = format!("{label}:");
    let mut objects = Vec::new();
    let mut search = 0usize;
    while let Some(relative) = source[search..].find(&marker_text) {
        let marker = search + relative;
        let Some(relative_open) = source[marker..].find('{') else {
            return Err(MeshError::InvalidData(format!(
                "FBX {label} block has no opening brace"
            )));
        };
        let open = marker + relative_open;
        let close = find_matching_brace(source.as_bytes(), open, &format!("FBX {label}"))?;
        let header = &source[marker..open];
        let strings = fbx_quoted_strings(header);
        let name = strings
            .first()
            .map(|name| {
                name.split_once("::")
                    .map(|(_, value)| value)
                    .unwrap_or(name)
                    .to_string()
            })
            .unwrap_or_else(|| format!("{label} {}", objects.len()));
        objects.push(AsciiFbxObject {
            id: fbx_object_id(header, label)?,
            name,
            kind: strings.get(1).cloned().unwrap_or_default(),
            block: &source[open + 1..close],
        });
        search = close + 1;
    }
    Ok(objects)
}

fn fbx_ascii_connections(source: &str) -> MeshResult<Vec<AsciiFbxConnection>> {
    let mut output = Vec::new();
    for line in source.lines() {
        let line = line.trim();
        if !line.starts_with("C:") {
            continue;
        }
        let strings = fbx_quoted_strings(line);
        let numbers = scan_fbx_numbers(line);
        if strings.is_empty() || numbers.len() < 2 {
            return Err(MeshError::InvalidData(format!(
                "malformed FBX connection '{line}'"
            )));
        }
        let child = numbers[0].parse::<i64>().map_err(|error| {
            MeshError::InvalidData(format!("invalid FBX connection child: {error}"))
        })?;
        let parent = numbers[1].parse::<i64>().map_err(|error| {
            MeshError::InvalidData(format!("invalid FBX connection parent: {error}"))
        })?;
        output.push(AsciiFbxConnection {
            kind: strings[0].clone(),
            child,
            parent,
            property: strings.get(1).cloned(),
        });
    }
    Ok(output)
}

fn fbx_property_vec3(block: &str, label: &str, default: [f32; 3]) -> MeshResult<[f32; 3]> {
    let marker = format!("P: \"{label}\"");
    let Some(line) = block.lines().find(|line| line.contains(&marker)) else {
        return Ok(default);
    };
    let numbers = scan_fbx_numbers(line);
    if numbers.len() < 3 {
        return Err(MeshError::InvalidData(format!(
            "FBX property {label} has fewer than three values"
        )));
    }
    let start = numbers.len() - 3;
    let mut output = [0.0; 3];
    for axis in 0..3 {
        output[axis] = numbers[start + axis].parse::<f32>().map_err(|error| {
            MeshError::InvalidData(format!("FBX property {label} is invalid: {error}"))
        })?;
        if !output[axis].is_finite() {
            return Err(MeshError::InvalidData(format!(
                "FBX property {label} is non-finite"
            )));
        }
    }
    Ok(output)
}

fn quaternion_from_euler_degrees(euler: [f32; 3]) -> [f32; 4] {
    let [x, y, z] = euler.map(|value| value.to_radians() * 0.5);
    let (sx, cx) = x.sin_cos();
    let (sy, cy) = y.sin_cos();
    let (sz, cz) = z.sin_cos();
    normalize_quaternion([
        sx * cy * cz - cx * sy * sz,
        cx * sy * cz + sx * cy * sz,
        cx * cy * sz - sx * sy * cz,
        cx * cy * cz + sx * sy * sz,
    ])
}

fn fbx_matrix(block: &str, label: &str) -> MeshResult<[f32; 16]> {
    let tokens = fbx_array_tokens(block, label)?
        .ok_or_else(|| MeshError::InvalidData(format!("FBX cluster has no {label} matrix")))?;
    if tokens.len() != 16 {
        return Err(MeshError::InvalidData(format!(
            "FBX {label} matrix has {} values, expected 16",
            tokens.len()
        )));
    }
    let mut row_major = [0.0; 16];
    for (index, token) in tokens.iter().enumerate() {
        row_major[index] = token.parse::<f32>().map_err(|error| {
            MeshError::InvalidData(format!("FBX {label} matrix is invalid: {error}"))
        })?;
    }
    Ok(std::array::from_fn(|index| {
        row_major[(index % 4) * 4 + index / 4]
    }))
}

fn invert_matrix(matrix: [f32; 16], context: &str) -> MeshResult<[f32; 16]> {
    let mut augmented = [[0.0f32; 8]; 4];
    for row in 0..4 {
        for column in 0..4 {
            augmented[row][column] = matrix[column * 4 + row];
        }
        augmented[row][row + 4] = 1.0;
    }
    for column in 0..4 {
        let pivot = (column..4)
            .max_by(|left, right| {
                augmented[*left][column]
                    .abs()
                    .total_cmp(&augmented[*right][column].abs())
            })
            .unwrap_or(column);
        if augmented[pivot][column].abs() <= 1.0e-8 {
            return Err(MeshError::InvalidData(format!(
                "{context} matrix is singular"
            )));
        }
        augmented.swap(column, pivot);
        let divisor = augmented[column][column];
        for value in &mut augmented[column] {
            *value /= divisor;
        }
        for row in 0..4 {
            if row == column {
                continue;
            }
            let factor = augmented[row][column];
            for entry in 0..8 {
                augmented[row][entry] -= factor * augmented[column][entry];
            }
        }
    }
    Ok(std::array::from_fn(|index| {
        augmented[index % 4][index / 4 + 4]
    }))
}

fn attach_ascii_fbx_armature(
    source: &str,
    mut mesh: MeshData,
    geometry_ranges: &[AsciiFbxGeometryRange],
) -> MeshResult<MeshData> {
    let deformers = fbx_ascii_objects(source, "Deformer")?;
    let has_animation =
        source.contains("AnimationCurve:") || source.contains("AnimationCurveNode:");
    if deformers.is_empty() {
        if has_animation {
            return Err(MeshError::UnsupportedFormat(
                "ASCII FBX contains animation curves but no supported Skin/Cluster armature".into(),
            ));
        }
        return Ok(mesh);
    }
    let connections = fbx_ascii_connections(source)?;
    let skins = deformers
        .iter()
        .filter(|object| object.kind.eq_ignore_ascii_case("Skin"))
        .collect::<Vec<_>>();
    let mut linked = Vec::<(&AsciiFbxObject<'_>, AsciiFbxGeometryRange)>::new();
    for skin in skins {
        for range in geometry_ranges {
            if connections.iter().any(|connection| {
                connection.kind == "OO"
                    && connection.child == skin.id
                    && connection.parent == range.id
            }) {
                linked.push((skin, *range));
            }
        }
    }
    if linked.is_empty() {
        return Ok(mesh);
    }
    if linked.len() != 1 {
        return Err(MeshError::UnsupportedFormat(
            "ASCII FBX armature import currently supports one skinned Geometry per asset".into(),
        ));
    }
    let (skin, geometry) = linked[0];
    let models = fbx_ascii_objects(source, "Model")?;
    if models.is_empty() || models.len() > MAX_ARMATURE_NODES {
        return Err(MeshError::InvalidData(
            "ASCII FBX skin has no models or exceeds the armature node limit".into(),
        ));
    }
    let model_indices = models
        .iter()
        .enumerate()
        .map(|(index, model)| (model.id, index))
        .collect::<HashMap<_, _>>();
    let mut nodes = Vec::with_capacity(models.len());
    let mut rest_euler = Vec::with_capacity(models.len());
    for model in &models {
        if let Some(line) = model
            .block
            .lines()
            .find(|line| line.contains("P: \"RotationOrder\""))
        {
            let values = scan_fbx_numbers(line);
            if values
                .last()
                .and_then(|value| value.parse::<i32>().ok())
                .is_some_and(|order| order != 0)
            {
                return Err(MeshError::UnsupportedFormat(format!(
                    "ASCII FBX model '{}' uses a non-XYZ RotationOrder",
                    model.name
                )));
            }
        }
        let translation = fbx_property_vec3(model.block, "Lcl Translation", [0.0; 3])?;
        let euler = fbx_property_vec3(model.block, "Lcl Rotation", [0.0; 3])?;
        let scale = fbx_property_vec3(model.block, "Lcl Scaling", [1.0; 3])?;
        rest_euler.push(euler);
        nodes.push(ArmatureNode {
            name: model.name.clone(),
            parent: None,
            translation,
            rotation: quaternion_from_euler_degrees(euler),
            scale,
        });
    }
    for connection in &connections {
        if connection.kind != "OO" {
            continue;
        }
        if let (Some(&child), Some(&parent)) = (
            model_indices.get(&connection.child),
            model_indices.get(&connection.parent),
        ) {
            if nodes[child].parent.replace(parent).is_some() {
                return Err(MeshError::InvalidData(format!(
                    "ASCII FBX model '{}' has multiple parents",
                    nodes[child].name
                )));
            }
        }
    }
    validate_armature_hierarchy(&nodes)?;

    let clusters = deformers
        .iter()
        .filter(|object| {
            object.kind.eq_ignore_ascii_case("Cluster")
                && connections.iter().any(|connection| {
                    connection.kind == "OO"
                        && connection.child == object.id
                        && connection.parent == skin.id
                })
        })
        .collect::<Vec<_>>();
    if clusters.is_empty() || clusters.len() > usize::from(u16::MAX) + 1 {
        return Err(MeshError::InvalidData(
            "ASCII FBX skin has no clusters or too many joints".into(),
        ));
    }
    let mut joints = Vec::with_capacity(clusters.len());
    let mut inverse_bind_matrices = Vec::with_capacity(clusters.len());
    let mut raw_influences = vec![Vec::<(u16, f32)>::new(); mesh.vertices.len()];
    let mut used_bones = std::collections::HashSet::new();
    for (palette_index, cluster) in clusters.iter().enumerate() {
        let bone_id = connections
            .iter()
            .find(|connection| {
                connection.kind == "OO"
                    && connection.parent == cluster.id
                    && model_indices.contains_key(&connection.child)
            })
            .map(|connection| connection.child)
            .ok_or_else(|| {
                MeshError::InvalidData(format!(
                    "ASCII FBX cluster '{}' is not connected to a bone Model",
                    cluster.name
                ))
            })?;
        let node = model_indices[&bone_id];
        if !used_bones.insert(node) {
            return Err(MeshError::UnsupportedFormat(format!(
                "ASCII FBX bone '{}' has multiple clusters for one geometry",
                nodes[node].name
            )));
        }
        joints.push(node);
        inverse_bind_matrices.push(invert_matrix(
            fbx_matrix(cluster.block, "TransformLink")?,
            &format!("ASCII FBX cluster '{}' TransformLink", cluster.name),
        )?);
        let index_tokens = fbx_array_tokens(cluster.block, "Indexes")?.ok_or_else(|| {
            MeshError::InvalidData(format!(
                "ASCII FBX cluster '{}' has no Indexes",
                cluster.name
            ))
        })?;
        let weight_tokens = fbx_array_tokens(cluster.block, "Weights")?.ok_or_else(|| {
            MeshError::InvalidData(format!(
                "ASCII FBX cluster '{}' has no Weights",
                cluster.name
            ))
        })?;
        if index_tokens.len() != weight_tokens.len() {
            return Err(MeshError::InvalidData(format!(
                "ASCII FBX cluster '{}' index/weight counts differ",
                cluster.name
            )));
        }
        for (influence, (index, weight)) in
            index_tokens.iter().zip(weight_tokens.iter()).enumerate()
        {
            let control_point = index.parse::<usize>().map_err(|error| {
                MeshError::InvalidData(format!(
                    "ASCII FBX cluster '{}' index {influence} is invalid: {error}",
                    cluster.name
                ))
            })?;
            let weight = weight.parse::<f32>().map_err(|error| {
                MeshError::InvalidData(format!(
                    "ASCII FBX cluster '{}' weight {influence} is invalid: {error}",
                    cluster.name
                ))
            })?;
            if control_point >= geometry.vertex_count || !weight.is_finite() || weight < 0.0 {
                return Err(MeshError::InvalidData(format!(
                    "ASCII FBX cluster '{}' has an out-of-range influence",
                    cluster.name
                )));
            }
            if weight > 0.0 {
                raw_influences[geometry.base_vertex + control_point]
                    .push((palette_index as u16, weight));
            }
        }
    }
    let mut vertex_weights = vec![SkinWeights::default(); mesh.vertices.len()];
    for (vertex, influences) in raw_influences.iter_mut().enumerate() {
        influences.sort_by(|left, right| right.1.total_cmp(&left.1));
        influences.truncate(4);
        let sum = influences.iter().map(|(_, weight)| *weight).sum::<f32>();
        if sum > f32::EPSILON {
            for (slot, (joint, weight)) in influences.iter().copied().enumerate() {
                vertex_weights[vertex].joints[slot] = joint;
                vertex_weights[vertex].weights[slot] = weight / sum;
            }
        }
    }
    let animations =
        parse_ascii_fbx_animations(source, &models, &model_indices, &connections, &rest_euler)?;
    mesh.armature = Some(Armature {
        nodes,
        joints,
        inverse_bind_matrices,
        vertex_weights,
        bind_vertices: mesh.vertices.clone(),
    });
    mesh.animations = animations;
    mesh.finish_mutation(false)?;
    Ok(mesh)
}

#[derive(Clone, Debug)]
struct AsciiFbxCurve {
    times: Vec<f32>,
    values: Vec<f32>,
}

fn parse_ascii_fbx_animations(
    source: &str,
    models: &[AsciiFbxObject<'_>],
    model_indices: &HashMap<i64, usize>,
    connections: &[AsciiFbxConnection],
    rest_euler: &[[f32; 3]],
) -> MeshResult<Vec<AnimationClip>> {
    let curve_objects = fbx_ascii_objects(source, "AnimationCurve")?;
    let curve_nodes = fbx_ascii_objects(source, "AnimationCurveNode")?;
    if curve_objects.is_empty() && curve_nodes.is_empty() {
        return Ok(Vec::new());
    }
    if curve_objects.len() > 65_536 || curve_nodes.len() > 65_536 {
        return Err(MeshError::InvalidData(
            "ASCII FBX animation exceeds the curve limit".into(),
        ));
    }
    let mut curves = HashMap::<i64, AsciiFbxCurve>::new();
    for object in curve_objects {
        let time_tokens = fbx_array_tokens(object.block, "KeyTime")?.ok_or_else(|| {
            MeshError::InvalidData(format!("ASCII FBX curve '{}' has no KeyTime", object.name))
        })?;
        let value_tokens = fbx_array_tokens(object.block, "KeyValueFloat")?
            .or(fbx_array_tokens(object.block, "KeyValueDouble")?)
            .ok_or_else(|| {
                MeshError::InvalidData(format!(
                    "ASCII FBX curve '{}' has no KeyValueFloat/Double",
                    object.name
                ))
            })?;
        if time_tokens.is_empty()
            || time_tokens.len() != value_tokens.len()
            || time_tokens.len() > 1_000_000
        {
            return Err(MeshError::InvalidData(format!(
                "ASCII FBX curve '{}' has invalid key counts",
                object.name
            )));
        }
        let mut times = Vec::with_capacity(time_tokens.len());
        let mut values = Vec::with_capacity(value_tokens.len());
        for (key, (time, value)) in time_tokens.iter().zip(value_tokens.iter()).enumerate() {
            let ticks = time.parse::<i64>().map_err(|error| {
                MeshError::InvalidData(format!(
                    "ASCII FBX curve '{}' key {key} time is invalid: {error}",
                    object.name
                ))
            })?;
            let seconds = ticks as f64 / 46_186_158_000.0;
            let seconds = seconds as f32;
            let value = value.parse::<f32>().map_err(|error| {
                MeshError::InvalidData(format!(
                    "ASCII FBX curve '{}' key {key} value is invalid: {error}",
                    object.name
                ))
            })?;
            if !seconds.is_finite()
                || !value.is_finite()
                || times.last().is_some_and(|previous| seconds <= *previous)
            {
                return Err(MeshError::InvalidData(format!(
                    "ASCII FBX curve '{}' has non-finite or unsorted keys",
                    object.name
                )));
            }
            times.push(seconds);
            values.push(value);
        }
        curves.insert(object.id, AsciiFbxCurve { times, values });
    }

    let mut channels = Vec::new();
    for curve_node in curve_nodes {
        let Some(target) = connections.iter().find(|connection| {
            connection.child == curve_node.id
                && model_indices.contains_key(&connection.parent)
                && connection.property.is_some()
        }) else {
            continue;
        };
        let property = match target.property.as_deref() {
            Some("Lcl Translation") => AnimationProperty::Translation,
            Some("Lcl Rotation") => AnimationProperty::Rotation,
            Some("Lcl Scaling") => AnimationProperty::Scale,
            _ => continue,
        };
        let node = model_indices[&target.parent];
        let mut axes: [Option<&AsciiFbxCurve>; 3] = [None, None, None];
        for connection in connections.iter().filter(|connection| {
            connection.parent == curve_node.id && curves.contains_key(&connection.child)
        }) {
            let axis = match connection.property.as_deref() {
                Some("d|X") | Some("X") => 0,
                Some("d|Y") | Some("Y") => 1,
                Some("d|Z") | Some("Z") => 2,
                _ => continue,
            };
            if axes[axis].replace(&curves[&connection.child]).is_some() {
                return Err(MeshError::InvalidData(format!(
                    "ASCII FBX animation curve node '{}' has duplicate axis curves",
                    curve_node.name
                )));
            }
        }
        if axes.iter().all(Option::is_none) {
            continue;
        }
        let mut times = axes
            .iter()
            .flatten()
            .flat_map(|curve| curve.times.iter().copied())
            .collect::<Vec<_>>();
        times.sort_by(f32::total_cmp);
        times.dedup_by(|left, right| left.to_bits() == right.to_bits());
        let default = match property {
            AnimationProperty::Translation => {
                fbx_property_vec3(models[node].block, "Lcl Translation", [0.0; 3])?
            }
            AnimationProperty::Rotation => rest_euler[node],
            AnimationProperty::Scale => {
                fbx_property_vec3(models[node].block, "Lcl Scaling", [1.0; 3])?
            }
        };
        let mut values = Vec::with_capacity(times.len());
        for time in &times {
            let vector = std::array::from_fn(|axis| {
                axes[axis]
                    .map(|curve| sample_fbx_curve(curve, *time))
                    .unwrap_or(default[axis])
            });
            values.push(if property == AnimationProperty::Rotation {
                quaternion_from_euler_degrees(vector)
            } else {
                [vector[0], vector[1], vector[2], 0.0]
            });
        }
        channels.push(AnimationChannel {
            node,
            property,
            interpolation: AnimationInterpolation::Linear,
            times,
            values,
        });
    }
    if channels.is_empty() {
        return Ok(Vec::new());
    }
    let start = channels
        .iter()
        .filter_map(|channel| channel.times.first().copied())
        .min_by(f32::total_cmp)
        .unwrap_or(0.0);
    for channel in &mut channels {
        for time in &mut channel.times {
            *time -= start;
            if time.abs() < 1.0e-7 {
                *time = 0.0;
            }
        }
    }
    let duration = channels
        .iter()
        .filter_map(|channel| channel.times.last().copied())
        .max_by(f32::total_cmp)
        .unwrap_or(0.0);
    let clip_name = fbx_ascii_objects(source, "AnimationStack")?
        .first()
        .map(|stack| stack.name.clone())
        .unwrap_or_else(|| "FBX Take".to_string());
    Ok(vec![AnimationClip {
        name: clip_name,
        duration,
        channels,
    }])
}

fn sample_fbx_curve(curve: &AsciiFbxCurve, time: f32) -> f32 {
    if time <= curve.times[0] || curve.times.len() == 1 {
        return curve.values[0];
    }
    let last = curve.times.len() - 1;
    if time >= curve.times[last] {
        return curve.values[last];
    }
    let right = curve.times.partition_point(|candidate| *candidate <= time);
    let left = right - 1;
    let amount = (time - curve.times[left]) / (curve.times[right] - curve.times[left]);
    curve.values[left] + (curve.values[right] - curve.values[left]) * amount
}

fn append_fbx_geometry(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    submeshes: &mut Vec<Submesh>,
    name: String,
    positions: &[f64],
    polygon_indices: &[i64],
) -> MeshResult<()> {
    if positions.is_empty() || !positions.len().is_multiple_of(3) {
        return Err(MeshError::InvalidData(format!(
            "FBX geometry '{name}' Vertices count is not a non-zero multiple of three"
        )));
    }
    let base_vertex = usize_to_u32(vertices.len(), "FBX vertex offset")?;
    for (position_index, coordinates) in positions.chunks_exact(3).enumerate() {
        let mut position = [0.0f32; 3];
        for axis in 0..3 {
            let value = coordinates[axis] as f32;
            if !value.is_finite() {
                return Err(MeshError::InvalidData(format!(
                    "FBX geometry '{name}' vertex {position_index} is non-finite or outside f32 range"
                )));
            }
            position[axis] = value;
        }
        vertices.push(Vertex::from_position(position));
    }

    let local_vertex_count = positions.len() / 3;
    let first_index = usize_to_u32(indices.len(), "FBX submesh offset")?;
    let mut polygon = Vec::<u32>::new();
    for (polygon_index, raw) in polygon_indices.iter().copied().enumerate() {
        let ends_polygon = raw < 0;
        let decoded = if ends_polygon {
            raw.checked_neg()
                .and_then(|value| value.checked_sub(1))
                .ok_or_else(|| {
                    MeshError::InvalidData(format!(
                        "FBX geometry '{name}' polygon index {polygon_index} overflows"
                    ))
                })?
        } else {
            raw
        };
        let local = usize::try_from(decoded).map_err(|_| {
            MeshError::InvalidData(format!(
                "FBX geometry '{name}' polygon index {polygon_index} is negative or too large"
            ))
        })?;
        if local >= local_vertex_count {
            return Err(MeshError::InvalidData(format!(
                "FBX geometry '{name}' references control point {local}, but has {local_vertex_count}"
            )));
        }
        let local = usize_to_u32(local, "FBX control point index")?;
        polygon.push(base_vertex.checked_add(local).ok_or_else(|| {
            MeshError::InvalidData(format!("FBX geometry '{name}' rebased index overflows"))
        })?);
        if ends_polygon {
            if polygon.len() < 3 {
                return Err(MeshError::InvalidData(format!(
                    "FBX geometry '{name}' has a polygon with fewer than three vertices"
                )));
            }
            let first = polygon[0];
            for corner in 1..polygon.len() - 1 {
                indices.extend_from_slice(&[first, polygon[corner], polygon[corner + 1]]);
            }
            polygon.clear();
        }
    }
    if !polygon.is_empty() {
        return Err(MeshError::InvalidData(format!(
            "FBX geometry '{name}' final polygon has no negative terminator"
        )));
    }
    let first_index_usize = usize::try_from(first_index).map_err(|_| {
        MeshError::InvalidData(format!("FBX geometry '{name}' offset is too large"))
    })?;
    let index_count = indices.len() - first_index_usize;
    if index_count == 0 {
        return Err(MeshError::InvalidData(format!(
            "FBX geometry '{name}' contains no polygons"
        )));
    }
    submeshes.push(Submesh {
        name,
        first_index,
        index_count: usize_to_u32(index_count, "FBX submesh index count")?,
        material: None,
    });
    Ok(())
}

fn find_matching_brace(bytes: &[u8], open: usize, context: &str) -> MeshResult<usize> {
    if bytes.get(open) != Some(&b'{') {
        return Err(MeshError::InvalidData(format!(
            "{context} does not start at an opening brace"
        )));
    }
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut in_comment = false;
    for (index, byte) in bytes.iter().copied().enumerate().skip(open) {
        if in_comment {
            if matches!(byte, b'\n' | b'\r') {
                in_comment = false;
            }
            continue;
        }
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b';' => in_comment = true,
            b'"' => in_string = true,
            b'{' => {
                depth = depth.checked_add(1).ok_or_else(|| {
                    MeshError::InvalidData(format!("{context} nesting overflows"))
                })?;
            }
            b'}' => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    MeshError::InvalidData(format!("{context} has an unmatched closing brace"))
                })?;
                if depth == 0 {
                    return Ok(index);
                }
            }
            _ => {}
        }
    }
    Err(MeshError::InvalidData(format!(
        "{context} has no matching closing brace"
    )))
}

fn fbx_geometry_name(header: &str) -> Option<String> {
    let start = header.find('"')? + 1;
    let end = start + header[start..].find('"')?;
    let name = &header[start..end];
    Some(name.strip_prefix("Geometry::").unwrap_or(name).to_string())
}

fn fbx_array_tokens(block: &str, label: &str) -> MeshResult<Option<Vec<String>>> {
    let marker = format!("{label}:");
    let Some(marker_offset) = block.find(&marker) else {
        return Ok(None);
    };
    let Some(relative_open) = block[marker_offset + marker.len()..].find('{') else {
        return Err(MeshError::InvalidData(format!(
            "FBX {label} array has no opening brace"
        )));
    };
    let open = marker_offset + marker.len() + relative_open;
    let close = find_matching_brace(block.as_bytes(), open, &format!("FBX {label} array"))?;
    let contents = &block[open + 1..close];
    let data_offset = contents
        .find("a:")
        .ok_or_else(|| MeshError::InvalidData(format!("FBX {label} array has no data payload")))?;
    Ok(Some(scan_fbx_numbers(&contents[data_offset + 2..])))
}

fn scan_fbx_numbers(source: &str) -> Vec<String> {
    let mut numbers = Vec::new();
    let mut current = String::new();
    let mut in_comment = false;
    for character in source.chars() {
        if in_comment {
            if matches!(character, '\n' | '\r') {
                in_comment = false;
            }
            continue;
        }
        if character == ';' {
            if !current.is_empty() {
                numbers.push(std::mem::take(&mut current));
            }
            in_comment = true;
        } else if character.is_ascii_digit() || matches!(character, '+' | '-' | '.' | 'e' | 'E') {
            current.push(character);
        } else if !current.is_empty() {
            numbers.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        numbers.push(current);
    }
    numbers
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::Write as _;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn built_in_primitives_are_cached_valid_triangle_meshes() -> MeshResult<()> {
        for kind in ["cube", "plane", "sphere", "cylinder", "capsule", "cone"] {
            let mut options = PrimitiveOptions::default();
            options.segments = 12;
            options.rings = 6;
            options.height = 2.0;
            let first = primitive_mesh(kind, options)?;
            let second = primitive_mesh(kind, options)?;
            assert_eq!(first.identity(), second.identity(), "{kind} was not cached");
            let snapshot = first.snapshot()?;
            snapshot.mesh.validate()?;
            assert!(!snapshot.mesh.vertices.is_empty(), "{kind} has no vertices");
            assert!(!snapshot.mesh.indices.is_empty(), "{kind} has no triangles");
            for triangle in snapshot.mesh.indices.chunks_exact(3) {
                let a = snapshot.mesh.vertices[triangle[0] as usize].position;
                let b = snapshot.mesh.vertices[triangle[1] as usize].position;
                let c = snapshot.mesh.vertices[triangle[2] as usize].position;
                let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let cross = [
                    ab[1] * ac[2] - ab[2] * ac[1],
                    ab[2] * ac[0] - ab[0] * ac[2],
                    ab[0] * ac[1] - ab[1] * ac[0],
                ];
                assert!(
                    cross.iter().map(|value| value * value).sum::<f32>() > 1.0e-12,
                    "{kind} contains a degenerate triangle"
                );
                let normal = [
                    snapshot.mesh.vertices[triangle[0] as usize].normal[0]
                        + snapshot.mesh.vertices[triangle[1] as usize].normal[0]
                        + snapshot.mesh.vertices[triangle[2] as usize].normal[0],
                    snapshot.mesh.vertices[triangle[0] as usize].normal[1]
                        + snapshot.mesh.vertices[triangle[1] as usize].normal[1]
                        + snapshot.mesh.vertices[triangle[2] as usize].normal[1],
                    snapshot.mesh.vertices[triangle[0] as usize].normal[2]
                        + snapshot.mesh.vertices[triangle[1] as usize].normal[2]
                        + snapshot.mesh.vertices[triangle[2] as usize].normal[2],
                ];
                assert!(
                    cross.iter().zip(normal).map(|(a, b)| a * b).sum::<f32>() > 0.0,
                    "{kind} has an inward-facing triangle"
                );
            }
        }
        let cube = primitive_mesh("cube", PrimitiveOptions::default())?.snapshot()?;
        assert_eq!(cube.mesh.vertices.len(), 24);
        assert_eq!(cube.mesh.indices.len(), 36);
        assert_eq!(cube.mesh.bounds.min, [-0.5; 3]);
        assert_eq!(cube.mesh.bounds.max, [0.5; 3]);
        Ok(())
    }

    #[test]
    fn gltf_skin_and_animation_deform_bind_vertices() -> MeshResult<()> {
        let mut buffer = Vec::new();
        for position in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            for value in position {
                buffer.extend_from_slice(&value.to_le_bytes());
            }
        }
        for index in [0u16, 1, 2] {
            buffer.extend_from_slice(&index.to_le_bytes());
        }
        buffer.extend_from_slice(&[0, 0]);
        for _ in 0..3 {
            buffer.extend_from_slice(&[0, 0, 0, 0]);
        }
        for _ in 0..3 {
            for weight in [1.0f32, 0.0, 0.0, 0.0] {
                buffer.extend_from_slice(&weight.to_le_bytes());
            }
        }
        for value in identity_matrix() {
            buffer.extend_from_slice(&value.to_le_bytes());
        }
        for time in [0.0f32, 1.0] {
            buffer.extend_from_slice(&time.to_le_bytes());
        }
        for translation in [[0.0f32, 0.0, 0.0], [2.0, 0.0, 0.0]] {
            for value in translation {
                buffer.extend_from_slice(&value.to_le_bytes());
            }
        }
        assert_eq!(buffer.len(), 200);
        let uri = format!(
            "data:application/octet-stream;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&buffer)
        );
        let document = serde_json::json!({
            "asset": { "version": "2.0" },
            "buffers": [{ "byteLength": buffer.len(), "uri": uri }],
            "bufferViews": [
                { "buffer": 0, "byteOffset": 0, "byteLength": 36 },
                { "buffer": 0, "byteOffset": 36, "byteLength": 6 },
                { "buffer": 0, "byteOffset": 44, "byteLength": 12 },
                { "buffer": 0, "byteOffset": 56, "byteLength": 48 },
                { "buffer": 0, "byteOffset": 104, "byteLength": 64 },
                { "buffer": 0, "byteOffset": 168, "byteLength": 8 },
                { "buffer": 0, "byteOffset": 176, "byteLength": 24 }
            ],
            "accessors": [
                { "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" },
                { "bufferView": 1, "componentType": 5123, "count": 3, "type": "SCALAR" },
                { "bufferView": 2, "componentType": 5121, "count": 3, "type": "VEC4" },
                { "bufferView": 3, "componentType": 5126, "count": 3, "type": "VEC4" },
                { "bufferView": 4, "componentType": 5126, "count": 1, "type": "MAT4" },
                { "bufferView": 5, "componentType": 5126, "count": 2, "type": "SCALAR" },
                { "bufferView": 6, "componentType": 5126, "count": 2, "type": "VEC3" }
            ],
            "meshes": [{ "name": "Skinned Triangle", "primitives": [{
                "attributes": { "POSITION": 0, "JOINTS_0": 2, "WEIGHTS_0": 3 },
                "indices": 1
            }] }],
            "nodes": [
                { "name": "Mesh", "mesh": 0, "skin": 0 },
                { "name": "Bone" }
            ],
            "skins": [{ "name": "Armature", "joints": [1], "inverseBindMatrices": 4 }],
            "animations": [{
                "name": "Slide",
                "samplers": [{ "input": 5, "output": 6, "interpolation": "LINEAR" }],
                "channels": [{ "sampler": 0, "target": { "node": 1, "path": "translation" } }]
            }]
        });
        let handle = import_from_bytes(&serde_json::to_vec(&document)?, MeshFormat::Gltf)?;
        let imported = handle.snapshot()?;
        assert_eq!(
            imported
                .mesh
                .armature
                .as_ref()
                .map(|value| value.joints.len()),
            Some(1)
        );
        assert_eq!(handle.animation_names()?, vec!["Slide"]);
        assert_eq!(handle.animation_duration("Slide")?, Some(1.0));
        handle.sample_animation("Slide", 0.5, false)?;
        let posed = handle.snapshot()?;
        assert!((posed.mesh.vertices[0].position[0] - 1.0).abs() < 0.0001);
        assert!((posed.mesh.vertices[1].position[0] - 2.0).abs() < 0.0001);
        let independent = handle.detached_clone()?;
        independent.sample_animation("Slide", 0.0, false)?;
        assert_ne!(handle.identity(), independent.identity());
        assert!((handle.snapshot()?.mesh.vertices[0].position[0] - 1.0).abs() < 0.0001);
        assert!(independent.snapshot()?.mesh.vertices[0].position[0].abs() < 0.0001);
        independent.play_animation("Slide", false, 1.0)?;
        assert!(independent.advance_animation(0.25)?);
        assert!((independent.snapshot()?.mesh.vertices[0].position[0] - 0.5).abs() < 0.0001);
        independent.set_animation_paused(true)?;
        assert!(!independent.advance_animation(0.25)?);
        independent.stop_animation()?;
        assert!(independent.snapshot()?.mesh.vertices[0].position[0].abs() < 0.0001);
        Ok(())
    }

    #[test]
    fn ascii_fbx_skin_cluster_and_curve_deform_vertices() -> MeshResult<()> {
        let fixture = br#"
            ; FBX 7.4.0 project file
            Objects: {
                Geometry: 1, "Geometry::Triangle", "Mesh" {
                    Vertices: *9 { a: 0,0,0, 1,0,0, 0,1,0 }
                    PolygonVertexIndex: *3 { a: 0,1,-3 }
                }
                Model: 30, "Model::Bone", "LimbNode" {
                    Properties70: {
                        P: "Lcl Translation", "Lcl Translation", "", "A",0,0,0
                        P: "Lcl Rotation", "Lcl Rotation", "", "A",0,0,0
                        P: "Lcl Scaling", "Lcl Scaling", "", "A",1,1,1
                    }
                }
                Deformer: 20, "Deformer::Skin", "Skin" { }
                Deformer: 21, "SubDeformer::Cluster", "Cluster" {
                    Indexes: *3 { a: 0,1,2 }
                    Weights: *3 { a: 1,1,1 }
                    TransformLink: *16 { a:
                        1,0,0,0,
                        0,1,0,0,
                        0,0,1,0,
                        0,0,0,1
                    }
                }
                AnimationStack: 40, "AnimStack::Move", "" { }
                AnimationCurveNode: 50, "AnimCurveNode::Translate", "" { }
                AnimationCurve: 51, "AnimCurve::X", "" {
                    KeyTime: *2 { a: 0,46186158000 }
                    KeyValueFloat: *2 { a: 0,2 }
                }
            }
            Connections: {
                C: "OO",20,1
                C: "OO",21,20
                C: "OO",30,21
                C: "OP",51,50,"d|X"
                C: "OP",50,30,"Lcl Translation"
            }
        "#;
        let handle = import_from_bytes(fixture, MeshFormat::Fbx)?;
        assert_eq!(handle.animation_names()?, vec!["Move"]);
        assert_eq!(
            handle
                .snapshot()?
                .mesh
                .armature
                .as_ref()
                .map(|armature| armature.joints.len()),
            Some(1)
        );
        handle.sample_animation("Move", 0.5, false)?;
        let snapshot = handle.snapshot()?;
        assert!((snapshot.mesh.vertices[0].position[0] - 1.0).abs() < 0.0001);
        assert!((snapshot.mesh.vertices[1].position[0] - 2.0).abs() < 0.0001);
        Ok(())
    }

    #[test]
    fn live_mutations_are_revisioned_validated_and_atomic() -> MeshResult<()> {
        let mut material = MeshMaterial::named("Editable");
        material.roughness = 0.4;
        let mesh = MeshData::new(
            "Triangle",
            vec![
                Vertex::from_position([0.0, 0.0, 0.0]),
                Vertex::from_position([1.0, 0.0, 0.0]),
                Vertex::from_position([0.0, 1.0, 0.0]),
            ],
            vec![0, 1, 2],
            vec![Submesh {
                name: "Triangle".into(),
                first_index: 0,
                index_count: 3,
                material: Some(0),
            }],
            vec![material],
            true,
        )?;
        mesh.validate()?;
        assert_eq!(mesh.bounds.max, [1.0, 1.0, 0.0]);
        assert_eq!(mesh.vertices[0].normal, [0.0, 0.0, 1.0]);

        let handle = MeshHandle::new(mesh)?;
        let clone = handle.clone();
        assert_eq!(handle.identity(), clone.identity());
        assert_eq!(handle.revision()?, 0);
        let mut moved = handle.snapshot()?.mesh.vertices[2];
        moved.position = [0.0, 2.0, 0.0];
        assert_eq!(handle.set_vertex(2, moved, true)?, 1);
        assert_eq!(clone.revision()?, 1);
        assert_eq!(clone.snapshot()?.mesh.bounds.max, [1.0, 2.0, 0.0]);

        let texture_revision = handle.mutate(|mesh| {
            let material = mesh
                .materials
                .get_mut(0)
                .ok_or_else(|| MeshError::InvalidData("test material is missing".into()))?;
            material.base_color_texture = Some(TextureBinding {
                source: "textures/live.png".into(),
                tex_coord: 0,
            });
            Ok(())
        })?;
        assert_eq!(texture_revision, 2);
        let texture_source = handle.with_read(|mesh, _| {
            mesh.materials
                .first()
                .and_then(|material| material.base_color_texture.as_ref())
                .map(|binding| binding.source.clone())
        })?;
        assert_eq!(texture_source.as_deref(), Some("textures/live.png"));

        let failed = handle.mutate(|mesh| {
            mesh.name = "must not commit".into();
            mesh.indices.push(99);
            Ok(())
        });
        assert!(matches!(failed, Err(MeshError::InvalidData(_))));
        assert_eq!(handle.revision()?, 2);
        let after_failure = handle.snapshot()?;
        assert_eq!(after_failure.revision, 2);
        assert_eq!(after_failure.mesh.name, "Triangle");

        assert_eq!(handle.recompute_normals()?, 3);
        let replacement = vec![
            Vertex::from_position([0.0, 0.0, 0.0]),
            Vertex::from_position([0.0, 1.0, 0.0]),
            Vertex::from_position([0.0, 0.0, 1.0]),
        ];
        assert_eq!(handle.replace_geometry(replacement, vec![0, 1, 2])?, 4);
        assert_eq!(handle.snapshot()?.mesh.vertices[0].normal, [1.0, 0.0, 0.0]);
        Ok(())
    }

    #[test]
    fn obj_imports_negative_indices_triangulates_and_tracks_materials() -> MeshResult<()> {
        let fixture = br#"
            o Quad
            v 0 0 0
            v 1 0 0
            v 1 1 0
            v 0 1 0
            vt 0 0
            vt 1 0
            vt 1 1
            vt 0 1
            usemtl Painted
            f -4/-4 -3/-3 -2/-2 -1/-1
        "#;
        let snapshot = import_from_bytes(fixture, MeshFormat::Obj)?.snapshot()?;
        assert_eq!(snapshot.mesh.vertices.len(), 4);
        assert_eq!(snapshot.mesh.indices, vec![0, 1, 2, 0, 2, 3]);
        assert_eq!(snapshot.mesh.submeshes.len(), 1);
        assert_eq!(snapshot.mesh.materials[0].name, "Painted");
        assert_eq!(snapshot.mesh.vertices[0].normal, [0.0, 0.0, 1.0]);

        let invalid = import_from_bytes(b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 0 2 3\n", MeshFormat::Obj);
        assert!(matches!(invalid, Err(MeshError::InvalidData(_))));
        Ok(())
    }

    #[test]
    fn gltf_json_imports_embedded_buffer_and_material() -> MeshResult<()> {
        let buffer = triangle_buffer();
        let uri = format!(
            "data:application/octet-stream;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&buffer)
        );
        let document = triangle_gltf(
            serde_json::json!({
                "byteLength": buffer.len(),
                "uri": uri
            }),
            3,
            36,
        );
        let bytes = serde_json::to_vec(&document)?;
        let snapshot = import_from_bytes(&bytes, MeshFormat::Gltf)?.snapshot()?;
        assert_eq!(snapshot.mesh.vertices.len(), 3);
        assert_eq!(snapshot.mesh.indices, vec![0, 1, 2]);
        assert_eq!(snapshot.mesh.materials[0].name, "Red");
        assert_eq!(snapshot.mesh.materials[0].base_color, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(snapshot.mesh.vertices[1].position, [1.0, 0.0, 0.0]);
        snapshot.mesh.validate()?;
        Ok(())
    }

    #[test]
    fn gltf_interleaved_accessors_honor_validated_stride() -> MeshResult<()> {
        let mut buffer = Vec::new();
        for position in [[0.0f32, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 3.0, 0.0]] {
            for value in position {
                buffer.extend_from_slice(&value.to_le_bytes());
            }
            buffer.extend_from_slice(&99.0f32.to_le_bytes());
        }
        for index in [0u16, 1, 2] {
            buffer.extend_from_slice(&index.to_le_bytes());
        }
        let uri = format!(
            "data:application/octet-stream;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&buffer)
        );
        let document = serde_json::json!({
            "asset": { "version": "2.0" },
            "buffers": [{ "byteLength": buffer.len(), "uri": uri }],
            "bufferViews": [
                { "buffer": 0, "byteLength": 48, "byteStride": 16 },
                { "buffer": 0, "byteOffset": 48, "byteLength": 6 }
            ],
            "accessors": [
                { "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" },
                { "bufferView": 1, "componentType": 5123, "count": 3, "type": "SCALAR" }
            ],
            "meshes": [{ "primitives": [{
                "attributes": { "POSITION": 0 }, "indices": 1
            }] }]
        });
        let bytes = serde_json::to_vec(&document)?;
        let snapshot = import_from_bytes(&bytes, MeshFormat::Gltf)?.snapshot()?;
        assert_eq!(snapshot.mesh.vertices[1].position, [2.0, 0.0, 0.0]);
        assert_eq!(snapshot.mesh.vertices[2].position, [0.0, 3.0, 0.0]);
        assert_eq!(snapshot.mesh.bounds.max, [2.0, 3.0, 0.0]);
        Ok(())
    }

    #[test]
    fn glb_imports_embedded_bin_chunk() -> MeshResult<()> {
        let buffer = triangle_buffer();
        let document = triangle_gltf(serde_json::json!({ "byteLength": buffer.len() }), 3, 36);
        let glb = build_glb(&document, &buffer)?;
        let snapshot = import_from_bytes(&glb, MeshFormat::Glb)?.snapshot()?;
        assert_eq!(snapshot.mesh.vertices.len(), 3);
        assert_eq!(snapshot.mesh.indices.len(), 3);
        assert_eq!(snapshot.mesh.bounds.max, [1.0, 1.0, 0.0]);
        Ok(())
    }

    #[test]
    fn gltf_resolves_external_buffers_relative_to_model_path() -> MeshResult<()> {
        let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("neolove-mesh-gltf-{}-{unique}", std::process::id()));
        let buffer_dir = root.join("buffers");
        fs::create_dir_all(&buffer_dir)?;
        let buffer = triangle_buffer();
        fs::write(buffer_dir.join("triangle.bin"), &buffer)?;
        let document = triangle_gltf(
            serde_json::json!({
                "byteLength": buffer.len(),
                "uri": "buffers/triangle.bin"
            }),
            3,
            36,
        );
        let model_path = root.join("triangle.gltf");
        fs::write(&model_path, serde_json::to_vec(&document)?)?;
        let imported = import_from_path(&model_path);
        let _ = fs::remove_dir_all(&root);
        let snapshot = imported?.snapshot()?;
        assert_eq!(snapshot.mesh.name, "triangle");
        assert_eq!(snapshot.mesh.indices, vec![0, 1, 2]);
        Ok(())
    }

    #[test]
    fn gltf_rejects_accessor_ranges_outside_their_buffer_view() -> MeshResult<()> {
        let buffer = triangle_buffer();
        let uri = format!(
            "data:application/octet-stream;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&buffer)
        );
        let document = triangle_gltf(
            serde_json::json!({ "byteLength": buffer.len(), "uri": uri }),
            4,
            36,
        );
        let bytes = serde_json::to_vec(&document)?;
        let imported = import_from_bytes(&bytes, MeshFormat::Gltf);
        assert!(matches!(imported, Err(MeshError::InvalidData(_))));
        Ok(())
    }

    #[test]
    fn ascii_fbx_imports_and_triangulates_a_quad() -> MeshResult<()> {
        let fixture = br#"
            ; FBX 7.4.0 project file
            Objects: {
                Geometry: 1, "Geometry::Quad", "Mesh" {
                    Vertices: *12 {
                        a: 0,0,0, 1,0,0, 1,1,0, 0,1,0
                    }
                    PolygonVertexIndex: *4 {
                        a: 0,1,2,-4
                    }
                }
            }
        "#;
        let snapshot = import_from_bytes(fixture, MeshFormat::Fbx)?.snapshot()?;
        assert_eq!(snapshot.mesh.name, "Quad");
        assert_eq!(snapshot.mesh.vertices.len(), 4);
        assert_eq!(snapshot.mesh.indices, vec![0, 1, 2, 0, 2, 3]);
        assert_eq!(snapshot.mesh.vertices[0].normal, [0.0, 0.0, 1.0]);

        let truncated_binary = import_from_bytes(b"Kaydara FBX Binary  \0", MeshFormat::Fbx);
        assert!(matches!(truncated_binary, Err(MeshError::InvalidData(_))));
        Ok(())
    }

    #[test]
    fn binary_fbx_7400_imports_raw_arrays_and_triangulates() -> MeshResult<()> {
        let bytes = binary_fbx_quad(7400, false)?;
        let snapshot = import_from_bytes(&bytes, MeshFormat::Fbx)?.snapshot()?;
        assert_eq!(snapshot.mesh.name, "BinaryQuad");
        assert_eq!(snapshot.mesh.vertices.len(), 4);
        assert_eq!(snapshot.mesh.indices, vec![0, 1, 2, 0, 2, 3]);
        assert_eq!(snapshot.mesh.vertices[0].normal, [0.0, 0.0, 1.0]);
        assert_eq!(snapshot.mesh.bounds.max, [1.0, 1.0, 0.0]);
        snapshot.mesh.validate()?;
        Ok(())
    }

    #[test]
    fn binary_fbx_7500_imports_zlib_arrays() -> MeshResult<()> {
        let bytes = binary_fbx_quad(7500, true)?;
        let snapshot = import_from_bytes(&bytes, MeshFormat::Fbx)?.snapshot()?;
        assert_eq!(snapshot.mesh.name, "BinaryQuad");
        assert_eq!(snapshot.mesh.indices, vec![0, 1, 2, 0, 2, 3]);
        assert_eq!(snapshot.mesh.vertices[2].position, [1.0, 1.0, 0.0]);
        Ok(())
    }

    #[test]
    fn binary_fbx_rejects_truncated_and_out_of_bounds_records() -> MeshResult<()> {
        let valid = binary_fbx_quad(7400, false)?;

        let mut truncated = valid.clone();
        truncated.truncate(truncated.len() - 7);
        let result = import_from_bytes(&truncated, MeshFormat::Fbx);
        assert!(matches!(result, Err(MeshError::InvalidData(_))));

        let mut outside_parent = valid;
        let root_offset = BINARY_FBX_MAGIC.len() + 4;
        outside_parent[root_offset..root_offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        let result = import_from_bytes(&outside_parent, MeshFormat::Fbx);
        assert!(matches!(result, Err(MeshError::InvalidData(_))));
        Ok(())
    }

    fn binary_fbx_quad(version: u32, compressed: bool) -> MeshResult<Vec<u8>> {
        let positions = [
            0.0f64, 0.0, 0.0, // vertex 0
            1.0, 0.0, 0.0, // vertex 1
            1.0, 1.0, 0.0, // vertex 2
            0.0, 1.0, 0.0, // vertex 3
        ];
        let mut position_bytes = Vec::new();
        for value in positions {
            position_bytes.extend_from_slice(&value.to_le_bytes());
        }
        let mut polygon_bytes = Vec::new();
        for value in [0i32, 1, 2, -4] {
            polygon_bytes.extend_from_slice(&value.to_le_bytes());
        }

        let vertices = TestFbxNode {
            name: "Vertices",
            properties: vec![test_fbx_array(
                b'd',
                positions.len(),
                position_bytes,
                compressed,
            )?],
            children: Vec::new(),
        };
        let polygons = TestFbxNode {
            name: "PolygonVertexIndex",
            properties: vec![test_fbx_array(b'i', 4, polygon_bytes, compressed)?],
            children: Vec::new(),
        };
        let geometry = TestFbxNode {
            name: "Geometry",
            properties: vec![
                test_fbx_i64(1),
                test_fbx_string("Geometry::BinaryQuad")?,
                test_fbx_string("Mesh")?,
            ],
            children: vec![vertices, polygons],
        };
        let objects = TestFbxNode {
            name: "Objects",
            properties: Vec::new(),
            children: vec![geometry],
        };

        let mut output = Vec::new();
        output.extend_from_slice(BINARY_FBX_MAGIC);
        output.extend_from_slice(&version.to_le_bytes());
        write_test_fbx_node(&mut output, version, &objects)?;
        output.resize(output.len() + if version >= 7500 { 25 } else { 13 }, 0);
        Ok(output)
    }

    struct TestFbxNode<'a> {
        name: &'a str,
        properties: Vec<Vec<u8>>,
        children: Vec<TestFbxNode<'a>>,
    }

    fn test_fbx_i64(value: i64) -> Vec<u8> {
        let mut property = vec![b'L'];
        property.extend_from_slice(&value.to_le_bytes());
        property
    }

    fn test_fbx_string(value: &str) -> MeshResult<Vec<u8>> {
        let mut property = vec![b'S'];
        property
            .extend_from_slice(&usize_to_u32(value.len(), "test FBX string length")?.to_le_bytes());
        property.extend_from_slice(value.as_bytes());
        Ok(property)
    }

    fn test_fbx_array(
        type_code: u8,
        length: usize,
        raw: Vec<u8>,
        compressed: bool,
    ) -> MeshResult<Vec<u8>> {
        let (encoding, payload) = if compressed {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&raw)?;
            (1u32, encoder.finish()?)
        } else {
            (0u32, raw)
        };
        let mut property = vec![type_code];
        property.extend_from_slice(&usize_to_u32(length, "test FBX array length")?.to_le_bytes());
        property.extend_from_slice(&encoding.to_le_bytes());
        property.extend_from_slice(
            &usize_to_u32(payload.len(), "test FBX array payload length")?.to_le_bytes(),
        );
        property.extend_from_slice(&payload);
        Ok(property)
    }

    fn write_test_fbx_node(
        output: &mut Vec<u8>,
        version: u32,
        node: &TestFbxNode<'_>,
    ) -> MeshResult<()> {
        let wide = version >= 7500;
        let header_len = if wide { 25 } else { 13 };
        let start = output.len();
        output.resize(start + header_len, 0);
        let name_len = u8::try_from(node.name.len())
            .map_err(|_| MeshError::InvalidData("test FBX node name is too long".into()))?;
        output[start + header_len - 1] = name_len;
        output.extend_from_slice(node.name.as_bytes());
        let properties_start = output.len();
        for property in &node.properties {
            output.extend_from_slice(property);
        }
        let property_len = output.len() - properties_start;
        for child in &node.children {
            write_test_fbx_node(output, version, child)?;
        }
        output.resize(output.len() + header_len, 0);
        let end = output.len();

        if wide {
            output[start..start + 8].copy_from_slice(&(end as u64).to_le_bytes());
            output[start + 8..start + 16]
                .copy_from_slice(&(node.properties.len() as u64).to_le_bytes());
            output[start + 16..start + 24].copy_from_slice(&(property_len as u64).to_le_bytes());
        } else {
            output[start..start + 4]
                .copy_from_slice(&usize_to_u32(end, "test FBX node end")?.to_le_bytes());
            output[start + 4..start + 8].copy_from_slice(
                &usize_to_u32(node.properties.len(), "test FBX property count")?.to_le_bytes(),
            );
            output[start + 8..start + 12].copy_from_slice(
                &usize_to_u32(property_len, "test FBX property length")?.to_le_bytes(),
            );
        }
        Ok(())
    }

    fn triangle_buffer() -> Vec<u8> {
        let mut bytes = Vec::new();
        for value in [
            0.0f32, 0.0, 0.0, // vertex 0
            1.0, 0.0, 0.0, // vertex 1
            0.0, 1.0, 0.0, // vertex 2
        ] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for index in [0u16, 1, 2] {
            bytes.extend_from_slice(&index.to_le_bytes());
        }
        bytes
    }

    fn triangle_gltf(
        buffer: JsonValue,
        position_count: usize,
        position_view_length: usize,
    ) -> JsonValue {
        serde_json::json!({
            "asset": { "version": "2.0" },
            "buffers": [buffer],
            "bufferViews": [
                { "buffer": 0, "byteOffset": 0, "byteLength": position_view_length },
                { "buffer": 0, "byteOffset": 36, "byteLength": 6 }
            ],
            "accessors": [
                {
                    "bufferView": 0,
                    "componentType": 5126,
                    "count": position_count,
                    "type": "VEC3"
                },
                {
                    "bufferView": 1,
                    "componentType": 5123,
                    "count": 3,
                    "type": "SCALAR"
                }
            ],
            "materials": [{
                "name": "Red",
                "pbrMetallicRoughness": {
                    "baseColorFactor": [1.0, 0.0, 0.0, 1.0],
                    "metallicFactor": 0.25,
                    "roughnessFactor": 0.75
                }
            }],
            "meshes": [{
                "name": "Triangle",
                "primitives": [{
                    "attributes": { "POSITION": 0 },
                    "indices": 1,
                    "material": 0
                }]
            }]
        })
    }

    fn build_glb(document: &JsonValue, binary: &[u8]) -> MeshResult<Vec<u8>> {
        let mut json = serde_json::to_vec(document)?;
        while json.len() % 4 != 0 {
            json.push(b' ');
        }
        let mut binary = binary.to_vec();
        while binary.len() % 4 != 0 {
            binary.push(0);
        }
        let total_length = 12usize
            .checked_add(8)
            .and_then(|value| value.checked_add(json.len()))
            .and_then(|value| value.checked_add(8))
            .and_then(|value| value.checked_add(binary.len()))
            .ok_or_else(|| MeshError::InvalidData("test GLB length overflows".into()))?;
        let mut output = Vec::with_capacity(total_length);
        output.extend_from_slice(GLB_MAGIC);
        output.extend_from_slice(&2u32.to_le_bytes());
        output.extend_from_slice(&usize_to_u32(total_length, "test GLB length")?.to_le_bytes());
        output.extend_from_slice(&usize_to_u32(json.len(), "test JSON length")?.to_le_bytes());
        output.extend_from_slice(&GLB_JSON_CHUNK.to_le_bytes());
        output.extend_from_slice(&json);
        output.extend_from_slice(&usize_to_u32(binary.len(), "test BIN length")?.to_le_bytes());
        output.extend_from_slice(&GLB_BIN_CHUNK.to_le_bytes());
        output.extend_from_slice(&binary);
        Ok(output)
    }
}
