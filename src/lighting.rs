#![allow(dead_code)]

//! CPU 2D lighting compositor.
//!
//! The runtime rasterizes the scene into an RGBA framebuffer through
//! [`crate::renderer::SoftwareRenderer`]. When lighting is enabled this module
//! builds a light map from the per-frame [`Light`] and [`Occluder`] lists plus
//! the persistent [`LightConfig`], then multiplies it over that framebuffer so
//! lights reveal the scene's true colors (like a 2D deferred light pass) rather
//! than painting a flat tint on top.
//!
//! Everything works in the same logical screen space the draw commands use, so
//! light and occluder positions are simply on-screen pixel coordinates.
//!
//! Performance: screen tiles index only relevant lights, occluders live in a
//! BVH, exact shadow rays and AO probes are cached into compact byte fields,
//! and Ultra adapts its map density on large displays. Work is split into
//! bounded row/light bands, so cost tracks visible light/occluder complexity
//! rather than `pixels * lights * occluders`.

use crate::platform::Color;
use std::sync::Arc;

/// Kind of emitter. `Point` radiates in every direction, `Spot` is a cone, and
/// `Directional` floods the whole scene from one direction (a 2D "sun").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LightKind {
    Point,
    Spot,
    Directional,
}

impl LightKind {
    pub(crate) fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "spot" | "cone" | "spotlight" => Self::Spot,
            "directional" | "sun" | "global" | "sky" => Self::Directional,
            _ => Self::Point,
        }
    }
}

/// A single light queued for the current frame. Positions are screen-space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Light {
    pub kind: LightKind,
    pub x: f32,
    pub y: f32,
    /// Reach in pixels for point/spot lights.
    pub radius: f32,
    pub color: Color,
    /// Brightness multiplier.
    pub intensity: f32,
    /// Distance-attenuation exponent: 1 is linear, 2 is quadratic, etc.
    pub falloff: f32,
    /// Aim direction in radians for spot/directional lights.
    pub angle: f32,
    /// Half-angle of a spot cone in radians.
    pub cone: f32,
    /// 0 is a hard cone edge, 1 fades across the whole cone.
    pub cone_softness: f32,
    pub casts_shadows: bool,
    /// Per-light penumbra size in pixels. Negative means "use the global
    /// [`LightConfig::soft_shadows`]".
    pub shadow_softness: f32,
}

impl Default for Light {
    fn default() -> Self {
        Self {
            kind: LightKind::Point,
            x: 0.0,
            y: 0.0,
            radius: 256.0,
            color: Color::WHITE,
            intensity: 1.0,
            falloff: 2.0,
            angle: 0.0,
            cone: std::f32::consts::FRAC_PI_4,
            cone_softness: 0.35,
            casts_shadows: true,
            shadow_softness: -1.0,
        }
    }
}

/// Shape of an occluder in its local frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OccluderShape {
    Box,
    Circle,
}

impl OccluderShape {
    pub(crate) fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "circle" | "ellipse" | "round" => Self::Circle,
            _ => Self::Box,
        }
    }
}

/// A rotated box or ellipse that blocks light (casts shadows) and contributes
/// to ambient occlusion. Described by its center, half extents, and rotation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Occluder {
    pub cx: f32,
    pub cy: f32,
    pub half_w: f32,
    pub half_h: f32,
    pub rotation: f32,
    pub shape: OccluderShape,
}

impl Occluder {
    /// Radius of a bounding circle around the occluder, for cheap early-outs.
    #[inline]
    fn bound_radius(&self) -> f32 {
        (self.half_w * self.half_w + self.half_h * self.half_h).sqrt()
    }
}

/// An occluder with its rotation and bounding radius resolved once, so the hot
/// shadow/AO loops do no trig or square roots per sample.
#[derive(Clone, Copy)]
struct PreparedOccluder {
    cx: f32,
    cy: f32,
    half_w: f32,
    half_h: f32,
    shape: OccluderShape,
    /// cos/sin of the occluder's rotation.
    cos_r: f32,
    sin_r: f32,
    bound_radius_sq: f32,
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
}

/// A compact bounding-volume hierarchy over the frame's occluders. Shadow rays
/// and AO point probes used to scan every occluder. Traversing this hierarchy
/// makes both queries logarithmic for the normal case while exact shape tests
/// still decide the result, so it does not alter shadow geometry.
struct OccluderIndex {
    occluders: Vec<PreparedOccluder>,
    nodes: Vec<OccluderNode>,
}

#[derive(Clone, Copy)]
struct OccluderNode {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
    kind: OccluderNodeKind,
}

#[derive(Clone, Copy)]
enum OccluderNodeKind {
    Branch { left: usize, right: usize },
    Leaf { start: usize, len: usize },
}

const OCCLUDERS_PER_LEAF: usize = 4;

impl OccluderIndex {
    fn new(occluders: &[Occluder]) -> Self {
        let mut prepared: Vec<PreparedOccluder> = occluders
            .iter()
            .map(|o| {
                let br = o.bound_radius();
                PreparedOccluder {
                    cx: o.cx,
                    cy: o.cy,
                    half_w: o.half_w,
                    half_h: o.half_h,
                    shape: o.shape,
                    cos_r: o.rotation.cos(),
                    sin_r: o.rotation.sin(),
                    bound_radius_sq: br * br,
                    min_x: o.cx - br,
                    min_y: o.cy - br,
                    max_x: o.cx + br,
                    max_y: o.cy + br,
                }
            })
            .collect();
        let mut nodes = Vec::with_capacity(prepared.len().saturating_mul(2));
        if !prepared.is_empty() {
            Self::build_node(&mut prepared, 0, &mut nodes);
        }
        Self {
            occluders: prepared,
            nodes,
        }
    }

    fn is_empty(&self) -> bool {
        self.occluders.is_empty()
    }

    fn build_node(
        occluders: &mut [PreparedOccluder],
        start: usize,
        nodes: &mut Vec<OccluderNode>,
    ) -> usize {
        let (min_x, min_y, max_x, max_y) = prepared_occluder_bounds(occluders);
        let node_index = nodes.len();
        // Reserve the parent before recursing so the root always remains node
        // zero and traversal can use a tiny fixed stack with no allocation.
        nodes.push(OccluderNode {
            min_x,
            min_y,
            max_x,
            max_y,
            kind: OccluderNodeKind::Leaf {
                start,
                len: occluders.len(),
            },
        });

        if occluders.len() > OCCLUDERS_PER_LEAF {
            let span_x = max_x - min_x;
            let span_y = max_y - min_y;
            if span_x >= span_y {
                occluders.sort_unstable_by(|a, b| a.cx.total_cmp(&b.cx));
            } else {
                occluders.sort_unstable_by(|a, b| a.cy.total_cmp(&b.cy));
            }
            let middle = occluders.len() / 2;
            let (left_occluders, right_occluders) = occluders.split_at_mut(middle);
            let left = Self::build_node(left_occluders, start, nodes);
            let right = Self::build_node(right_occluders, start + middle, nodes);
            nodes[node_index].kind = OccluderNodeKind::Branch { left, right };
        }
        node_index
    }

    #[inline]
    fn segment_occluded(&self, sample: (f32, f32), pixel: (f32, f32)) -> bool {
        if self.nodes.is_empty() {
            return false;
        }

        // A balanced binary hierarchy needs at most log2(N) pending siblings.
        // Sixty-four entries therefore covers every representable Vec length.
        let mut stack = [0usize; 64];
        let mut stack_len = 1usize;
        while stack_len > 0 {
            stack_len -= 1;
            let node = self.nodes[stack[stack_len]];
            if !segment_hits_aabb(
                sample, pixel, node.min_x, node.min_y, node.max_x, node.max_y,
            ) {
                continue;
            }
            match node.kind {
                OccluderNodeKind::Branch { left, right } => {
                    stack[stack_len] = left;
                    stack[stack_len + 1] = right;
                    stack_len += 2;
                }
                OccluderNodeKind::Leaf { start, len } => {
                    for occ in &self.occluders[start..start + len] {
                        if point_segment_dist_sq(occ.cx, occ.cy, sample, pixel)
                            > occ.bound_radius_sq
                        {
                            continue;
                        }
                        let p0 = to_local(occ, sample.0, sample.1);
                        let p1 = to_local(occ, pixel.0, pixel.1);
                        if local_segment_hits(occ, p0, p1) {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    #[inline]
    fn contains_point(&self, x: f32, y: f32) -> bool {
        if self.nodes.is_empty() {
            return false;
        }
        let mut stack = [0usize; 64];
        let mut stack_len = 1usize;
        while stack_len > 0 {
            stack_len -= 1;
            let node = self.nodes[stack[stack_len]];
            if x < node.min_x || x > node.max_x || y < node.min_y || y > node.max_y {
                continue;
            }
            match node.kind {
                OccluderNodeKind::Branch { left, right } => {
                    stack[stack_len] = left;
                    stack[stack_len + 1] = right;
                    stack_len += 2;
                }
                OccluderNodeKind::Leaf { start, len } => {
                    if self.occluders[start..start + len]
                        .iter()
                        .any(|occ| point_in_occluder(occ, x, y))
                    {
                        return true;
                    }
                }
            }
        }
        false
    }
}

fn prepared_occluder_bounds(occluders: &[PreparedOccluder]) -> (f32, f32, f32, f32) {
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for occ in occluders {
        min_x = min_x.min(occ.min_x);
        min_y = min_y.min(occ.min_y);
        max_x = max_x.max(occ.max_x);
        max_y = max_y.max(occ.max_y);
    }
    (min_x, min_y, max_x, max_y)
}

/// Resolution of the intermediate light map relative to the framebuffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LightQuality {
    /// Quarter resolution: fastest, softest.
    Low,
    /// Half resolution: the default balance.
    Medium,
    /// Full resolution.
    High,
    /// Adaptive high-detail map with extra shadow/AO samples. It stays full
    /// resolution below the texel budget and scales smoothly on dense displays.
    Ultra,
}

impl Default for LightQuality {
    fn default() -> Self {
        Self::Medium
    }
}

impl LightQuality {
    pub(crate) fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "low" | "fast" | "quarter" => Self::Low,
            "high" | "full" => Self::High,
            "ultra" | "max" | "best" => Self::Ultra,
            _ => Self::Medium,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Ultra => "ultra",
        }
    }

    /// Framebuffer pixels per light-map texel.
    fn downsample(self) -> usize {
        match self {
            Self::Low => 4,
            Self::Medium => 2,
            Self::High | Self::Ultra => 1,
        }
    }

    /// Ultra retains one light texel per framebuffer pixel while that is
    /// affordable, then adapts to display density so 1080p, 1440p, and 4K do
    /// not multiply CPU work without bound. Both presenters linearly upscale
    /// the result, and shadow/AO fields retain their independent detail.
    fn downsample_for_frame(self, width: usize, height: usize) -> usize {
        let base = self.downsample();
        if self != Self::Ultra {
            return base;
        }
        const ULTRA_TEXEL_BUDGET: usize = 1_000_000;
        let mut scale = base;
        while width.div_ceil(scale).saturating_mul(height.div_ceil(scale))
            > ULTRA_TEXEL_BUDGET
        {
            scale += 1;
        }
        scale
    }

    /// Multiplier applied to soft-shadow and AO sample counts.
    fn sample_scale(self) -> u32 {
        match self {
            Self::Ultra => 2,
            _ => 1,
        }
    }
}

/// Persistent global lighting settings. Lives on the render state and is
/// mutated through the `lighting` Luau global; it is not cleared each frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LightConfig {
    pub enabled: bool,
    pub ambient: Color,
    pub ambient_intensity: f32,
    pub ao_enabled: bool,
    pub ao_radius: f32,
    pub ao_intensity: f32,
    pub ao_samples: u32,
    pub shadows_enabled: bool,
    /// Default penumbra size in pixels; 0 is a hard shadow. Per-light
    /// `shadow_softness` overrides this when non-negative.
    pub soft_shadows: f32,
    /// Extra glow added where light exceeds full brightness.
    pub bloom: f32,
    /// Overall output multiplier applied after lighting.
    pub exposure: f32,
    pub quality: LightQuality,
}

impl Default for LightConfig {
    fn default() -> Self {
        Self {
            // Disabled by default so existing projects render unchanged until a
            // scene opts in.
            enabled: false,
            // Full white ambient means "enabled with no lights" looks like the
            // unlit scene; authors lower this to create darkness.
            ambient: Color::WHITE,
            ambient_intensity: 1.0,
            ao_enabled: false,
            ao_radius: 32.0,
            ao_intensity: 0.6,
            ao_samples: 12,
            shadows_enabled: true,
            soft_shadows: 0.0,
            bloom: 0.0,
            exposure: 1.0,
            quality: LightQuality::Medium,
        }
    }
}

impl LightConfig {
    /// Whether the compositor would change any pixels. Skipping the pass when
    /// there is nothing to do keeps the disabled/common case free.
    pub(crate) fn is_noop(&self, lights: &[Light], occluders: &[Occluder]) -> bool {
        if !self.enabled {
            return true;
        }
        let ambient_neutral =
            self.ambient_intensity == 1.0 && self.ambient == Color::WHITE && self.exposure == 1.0;
        let ao_active = self.ao_enabled && self.ao_intensity > 0.0 && !occluders.is_empty();
        ambient_neutral && lights.is_empty() && !ao_active
    }
}

#[inline]
fn clamp01(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

/// Transform a world point into an occluder's local (unrotated, centered) frame,
/// using its precomputed rotation (rotate by `-rotation`).
#[inline]
fn to_local(occ: &PreparedOccluder, x: f32, y: f32) -> (f32, f32) {
    let dx = x - occ.cx;
    let dy = y - occ.cy;
    (
        dx * occ.cos_r + dy * occ.sin_r,
        -dx * occ.sin_r + dy * occ.cos_r,
    )
}

#[inline]
fn point_in_occluder(occ: &PreparedOccluder, x: f32, y: f32) -> bool {
    let (lx, ly) = to_local(occ, x, y);
    match occ.shape {
        OccluderShape::Box => lx.abs() <= occ.half_w && ly.abs() <= occ.half_h,
        OccluderShape::Circle => {
            let hw = occ.half_w.max(1e-3);
            let hh = occ.half_h.max(1e-3);
            let nx = lx / hw;
            let ny = ly / hh;
            nx * nx + ny * ny <= 1.0
        }
    }
}

/// Segment vs. axis-aligned box (centered at origin, given half extents) using
/// the slab method. Returns true when `[p0, p1]` crosses the box interior.
fn segment_hits_box(p0: (f32, f32), p1: (f32, f32), half_w: f32, half_h: f32) -> bool {
    let dx = p1.0 - p0.0;
    let dy = p1.1 - p0.1;
    let mut t_min = 0.0f32;
    let mut t_max = 1.0f32;

    for (origin, delta, half) in [(p0.0, dx, half_w), (p0.1, dy, half_h)] {
        if delta.abs() < 1e-6 {
            if origin < -half || origin > half {
                return false;
            }
        } else {
            let inv = 1.0 / delta;
            let mut t1 = (-half - origin) * inv;
            let mut t2 = (half - origin) * inv;
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }
            t_min = t_min.max(t1);
            t_max = t_max.min(t2);
            if t_min > t_max {
                return false;
            }
        }
    }
    true
}

/// Conservative segment/AABB test used while traversing the occluder BVH.
/// Unlike the exact shape tests this deliberately treats non-finite values as
/// intersecting so malformed user data cannot make a real occluder disappear.
#[inline]
fn segment_hits_aabb(
    p0: (f32, f32),
    p1: (f32, f32),
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
) -> bool {
    if !p0.0.is_finite()
        || !p0.1.is_finite()
        || !p1.0.is_finite()
        || !p1.1.is_finite()
        || !min_x.is_finite()
        || !min_y.is_finite()
        || !max_x.is_finite()
        || !max_y.is_finite()
    {
        return true;
    }

    let mut t_min = 0.0f32;
    let mut t_max = 1.0f32;
    for (origin, delta, min, max) in [
        (p0.0, p1.0 - p0.0, min_x, max_x),
        (p0.1, p1.1 - p0.1, min_y, max_y),
    ] {
        if delta == 0.0 {
            if origin < min || origin > max {
                return false;
            }
            continue;
        }
        let inv = 1.0 / delta;
        let mut near = (min - origin) * inv;
        let mut far = (max - origin) * inv;
        if near > far {
            std::mem::swap(&mut near, &mut far);
        }
        t_min = t_min.max(near);
        t_max = t_max.min(far);
        if t_min > t_max {
            return false;
        }
    }
    true
}

/// Segment vs. unit circle (centered at origin). Used for ellipse occluders
/// after scaling the segment by the inverse half extents.
fn segment_hits_unit_circle(p0: (f32, f32), p1: (f32, f32)) -> bool {
    // Closest point on the segment to the origin.
    let dx = p1.0 - p0.0;
    let dy = p1.1 - p0.1;
    let len_sq = dx * dx + dy * dy;
    let t = if len_sq < 1e-9 {
        0.0
    } else {
        (-(p0.0 * dx + p0.1 * dy) / len_sq).clamp(0.0, 1.0)
    };
    let cx = p0.0 + dx * t;
    let cy = p0.1 + dy * t;
    cx * cx + cy * cy <= 1.0
}

/// Whether the local segment crosses a single occluder's shape.
#[inline]
fn local_segment_hits(occ: &PreparedOccluder, p0: (f32, f32), p1: (f32, f32)) -> bool {
    match occ.shape {
        OccluderShape::Box => segment_hits_box(p0, p1, occ.half_w, occ.half_h),
        OccluderShape::Circle => {
            let hw = occ.half_w.max(1e-3);
            let hh = occ.half_h.max(1e-3);
            segment_hits_unit_circle((p0.0 / hw, p0.1 / hh), (p1.0 / hw, p1.1 / hh))
        }
    }
}

/// Minimum squared distance from a point to a segment, for the bounding-circle
/// early-out on shadow rays.
fn point_segment_dist_sq(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let len_sq = dx * dx + dy * dy;
    let t = if len_sq < 1e-9 {
        0.0
    } else {
        (((px - a.0) * dx + (py - a.1) * dy) / len_sq).clamp(0.0, 1.0)
    };
    let cx = a.0 + dx * t;
    let cy = a.1 + dy * t;
    let ex = px - cx;
    let ey = py - cy;
    ex * ex + ey * ey
}

/// Distance/cone attenuation for a light at a pixel, in `0..=1`.
fn attenuation(light: &Light, px: f32, py: f32) -> f32 {
    match light.kind {
        LightKind::Directional => 1.0,
        LightKind::Point | LightKind::Spot => {
            let radius = light.radius.max(1.0);
            let dx = px - light.x;
            let dy = py - light.y;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist >= radius {
                return 0.0;
            }
            let mut atten = clamp01(1.0 - dist / radius);
            atten = atten.powf(light.falloff.max(0.1));

            if light.kind == LightKind::Spot {
                if dist < 1e-4 {
                    return atten;
                }
                let aim_x = light.angle.cos();
                let aim_y = light.angle.sin();
                let cos_to_pixel = (dx * aim_x + dy * aim_y) / dist;
                let angle_to_pixel = cos_to_pixel.clamp(-1.0, 1.0).acos();
                let cone = light.cone.max(1e-3);
                if angle_to_pixel >= cone {
                    return 0.0;
                }
                let soft = clamp01(light.cone_softness);
                let inner = cone * (1.0 - soft);
                let cone_factor = if angle_to_pixel <= inner {
                    1.0
                } else {
                    clamp01((cone - angle_to_pixel) / (cone - inner).max(1e-3))
                };
                atten *= cone_factor;
            }
            atten
        }
    }
}

/// Fraction of a light that reaches a pixel given occluders. `0` is fully in
/// shadow, `1` fully lit. Soft shadows average several jittered sample origins.
fn visibility(
    light: &Light,
    px: f32,
    py: f32,
    occluders: &OccluderIndex,
    soft_radius: f32,
    samples: u32,
) -> f32 {
    let origin = match light.kind {
        LightKind::Directional => {
            let far = 100_000.0;
            (px - light.angle.cos() * far, py - light.angle.sin() * far)
        }
        _ => (light.x, light.y),
    };

    if soft_radius <= 0.5 || samples <= 1 || light.kind == LightKind::Directional {
        return if occluders.segment_occluded(origin, (px, py)) {
            0.0
        } else {
            1.0
        };
    }

    let mut visible = 0u32;
    for i in 0..samples {
        let theta = (i as f32 / samples as f32) * std::f32::consts::TAU;
        let jx = origin.0 + theta.cos() * soft_radius;
        let jy = origin.1 + theta.sin() * soft_radius;
        if !occluders.segment_occluded((jx, jy), (px, py)) {
            visible += 1;
        }
    }
    visible as f32 / samples as f32
}

/// Ambient-occlusion darkening in `0..=1` (1 = fully occluded) by sampling a
/// ring around the pixel and measuring how much lands inside occluders.
fn ambient_occlusion(
    px: f32,
    py: f32,
    occluders: &OccluderIndex,
    sample_offsets: &[(f32, f32)],
) -> f32 {
    if sample_offsets.is_empty() {
        return 0.0;
    }
    if occluders.contains_point(px, py) {
        return 1.0;
    }
    // A single ring of samples; the light-map blur smooths the result, so a
    // second ring is not worth the extra point-in-occluder tests.
    let mut occluded = 0u32;
    for &(dx, dy) in sample_offsets {
        if occluders.contains_point(px + dx, py + dy) {
            occluded += 1;
        }
    }
    clamp01(occluded as f32 / sample_offsets.len() as f32)
}

fn build_shadow_field(
    light: &PreparedLight,
    occluders: &OccluderIndex,
    fb_width: usize,
    fb_height: usize,
    step: f32,
) -> ShadowField {
    let min_x = light.min_x.max(0.0).min(fb_width as f32);
    let min_y = light.min_y.max(0.0).min(fb_height as f32);
    let max_x = light.max_x.max(0.0).min(fb_width as f32);
    let max_y = light.max_y.max(0.0).min(fb_height as f32);
    let origin_x = (min_x / step).floor() * step;
    let origin_y = (min_y / step).floor() * step;
    let width = (((max_x - origin_x).max(0.0) / step).ceil() as usize + 1).max(1);
    let height = (((max_y - origin_y).max(0.0) / step).ceil() as usize + 1).max(1);
    let mut visibility_field = vec![255u8; width * height];

    for y in 0..height {
        let py = origin_y + y as f32 * step;
        for x in 0..width {
            let px = origin_x + x as f32 * step;
            let contribution = prepared_attenuation(light, px, py) * light.light.intensity;
            if contribution < MIN_CONTRIBUTION {
                continue;
            }
            if visibility(&light.light, px, py, occluders, 0.0, 1) <= 0.0 {
                visibility_field[y * width + x] = 0;
            }
        }
    }

    let has_soft_shadow = light.soft_radius > 0.5;
    if has_soft_shadow {
        let radius = ((light.soft_radius / step).ceil() as usize)
            .clamp(1, width.max(height).saturating_sub(1).max(1));
        blur_u8_field(&mut visibility_field, width, height, radius);
        blur_u8_field(&mut visibility_field, width, height, radius);
    }
    ShadowField {
        origin_x,
        origin_y,
        inv_step: 1.0 / step,
        width,
        height,
        // Interpolate coarser adaptive fields even for nominally hard shadows
        // so their edge remains anti-aliased rather than visibly blocky.
        smooth: has_soft_shadow || step > 2.0,
        visibility: visibility_field,
    }
}

fn build_shadow_fields(
    prepared: &[PreparedLight],
    occluders: &OccluderIndex,
    fb_width: usize,
    fb_height: usize,
    lightmap_scale: usize,
) -> Vec<Option<ShadowField>> {
    let mut fields: Vec<Option<ShadowField>> =
        std::iter::repeat_with(|| None).take(prepared.len()).collect();
    if fb_width.saturating_mul(fb_height) < 4096
        || occluders.is_empty()
        || !prepared.iter().any(|light| light.cast_shadows)
    {
        return fields;
    }
    let step = (lightmap_scale.saturating_mul(2) as f32).max(2.0);
    let workers = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
        .clamp(1, 16)
        .min(prepared.len().max(1));
    let chunk_len = prepared.len().div_ceil(workers);
    std::thread::scope(|scope| {
        for (chunk_index, chunk) in fields.chunks_mut(chunk_len).enumerate() {
            let start = chunk_index * chunk_len;
            scope.spawn(move || {
                for (offset, slot) in chunk.iter_mut().enumerate() {
                    let light = &prepared[start + offset];
                    if light.cast_shadows {
                        *slot = Some(build_shadow_field(
                            light,
                            occluders,
                            fb_width,
                            fb_height,
                            step,
                        ));
                    }
                }
            });
        }
    });
    fields
}

fn build_ao_field(
    region: AoRegion,
    sample_offsets: &[(f32, f32)],
    occluders: &OccluderIndex,
    fb_width: usize,
    fb_height: usize,
    radius: f32,
) -> Option<AoField> {
    if sample_offsets.is_empty() {
        return None;
    }
    let min_x = region.min_x.max(0.0);
    let min_y = region.min_y.max(0.0);
    let max_x = region.max_x.min(fb_width as f32);
    let max_y = region.max_y.min(fb_height as f32);
    if max_x < min_x || max_y < min_y {
        return None;
    }
    let step = (radius * 0.25).clamp(2.0, 8.0);
    let origin_x = (min_x / step).floor() * step;
    let origin_y = (min_y / step).floor() * step;
    let width = (((max_x - origin_x) / step).ceil() as usize + 1).max(1);
    let height = (((max_y - origin_y) / step).ceil() as usize + 1).max(1);
    let mut data = vec![0u8; width * height];
    let compute_rows = |chunk: &mut [u8], row0: usize| {
        let rows = chunk.len() / width;
        for local_y in 0..rows {
            let py = origin_y + (row0 + local_y) as f32 * step;
            for x in 0..width {
                let px = origin_x + x as f32 * step;
                chunk[local_y * width + x] =
                    (ambient_occlusion(px, py, occluders, sample_offsets) * 255.0).round() as u8;
            }
        }
    };
    let workers = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
        .clamp(1, 16)
        .min(height.max(1));
    if workers <= 1 || width * height < 4096 {
        compute_rows(&mut data, 0);
    } else {
        let rows_per = height.div_ceil(workers);
        let band = rows_per * width;
        let compute_rows = &compute_rows;
        std::thread::scope(|scope| {
            for (index, chunk) in data.chunks_mut(band).enumerate() {
                scope.spawn(move || compute_rows(chunk, index * rows_per));
            }
        });
    }
    Some(AoField {
        origin_x,
        origin_y,
        inv_step: 1.0 / step,
        width,
        height,
        occlusion: data,
        intensity: region.intensity,
    })
}

/// Bilinearly sample a 3-channel light map at fractional texel coordinates.
fn sample_lightmap(map: &[f32], lw: usize, lh: usize, fx: f32, fy: f32) -> (f32, f32, f32) {
    let x0 = fx.floor().max(0.0) as usize;
    let y0 = fy.floor().max(0.0) as usize;
    let x1 = (x0 + 1).min(lw - 1);
    let y1 = (y0 + 1).min(lh - 1);
    let tx = fx - x0 as f32;
    let ty = fy - y0 as f32;

    let at = |x: usize, y: usize, c: usize| map[(y * lw + x) * 3 + c];
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;

    let mut out = [0.0f32; 3];
    for c in 0..3 {
        let top = lerp(at(x0, y0, c), at(x1, y0, c), tx);
        let bottom = lerp(at(x0, y1, c), at(x1, y1, c), tx);
        out[c] = lerp(top, bottom, ty);
    }
    (out[0], out[1], out[2])
}

/// The light map produced by [`build_light_map`], plus its dimensions and the
/// framebuffer-to-texel scale.
struct LightMap {
    data: Vec<f32>,
    width: usize,
    height: usize,
    scale: usize,
}

/// Per-renderer cache for the expensive CPU light map. Entity components queue
/// fresh vectors every frame, but most scenes repeat identical values. Exact
/// slice/config comparisons let those frames reuse the map without relying on
/// collision-prone hashes. The cached allocation is also reused by Vulkan's
/// uploaded composite texture.
#[derive(Default)]
pub(crate) struct LightMapCache {
    width: u32,
    height: u32,
    config: Option<LightConfig>,
    lights: Vec<Light>,
    occluders: Vec<Occluder>,
    map: Option<Arc<LightMap>>,
    image: Option<Arc<LightMapImage>>,
    generation: u64,
}

impl LightMapCache {
    fn matches(
        &self,
        width: u32,
        height: u32,
        config: &LightConfig,
        lights: &[Light],
        occluders: &[Occluder],
    ) -> bool {
        self.width == width
            && self.height == height
            && self.config.as_ref() == Some(config)
            && self.lights == lights
            && self.occluders == occluders
    }

    fn prepare(
        &mut self,
        width: u32,
        height: u32,
        config: &LightConfig,
        lights: &[Light],
        occluders: &[Occluder],
    ) -> (u64, Arc<LightMap>) {
        if !self.matches(width, height, config, lights, occluders) {
            self.width = width;
            self.height = height;
            self.config = Some(*config);
            self.lights.clear();
            self.lights.extend_from_slice(lights);
            self.occluders.clear();
            self.occluders.extend_from_slice(occluders);
            self.map = Some(Arc::new(build_light_map(
                width as usize,
                height as usize,
                config,
                lights,
                occluders,
            )));
            self.image = None;
            self.generation = self.generation.wrapping_add(1);
        }
        (
            self.generation,
            Arc::clone(self.map.as_ref().expect("prepared light map missing")),
        )
    }
}

/// A light contribution below this (color × attenuation × intensity) changes an
/// 8-bit channel by less than one level, so its shadow ray is not worth casting.
const MIN_CONTRIBUTION: f32 = 0.004;

/// A light with its per-frame constants resolved once, so the hot per-texel
/// loop does no redundant work and can run on worker threads.
#[derive(Clone, Copy)]
struct PreparedLight {
    light: Light,
    cast_shadows: bool,
    soft_radius: f32,
    shadow_samples: u32,
    color: [f32; 3],
    // World-space influence box, for a cheap per-texel skip.
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
    radius_sq: f32,
    inv_radius: f32,
    falloff: f32,
    aim_x: f32,
    aim_y: f32,
    cone: f32,
    cone_inner: f32,
    cone_fade_inv: f32,
}

/// Ambient-occlusion region: the occluder union box (world space) plus the
/// sample count, evaluated only for texels inside it.
#[derive(Clone, Copy)]
struct AoRegion {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
    intensity: f32,
}

/// A compact visibility field for one shadow-casting light. Exact BVH rays are
/// evaluated once on a small grid, then reused by every full-resolution light
/// texel. Softness is blurred on this field rather than across the complete
/// RGB light map, preserving crisp light gradients while avoiding millions of
/// duplicate ray traversals.
struct ShadowField {
    origin_x: f32,
    origin_y: f32,
    inv_step: f32,
    width: usize,
    height: usize,
    smooth: bool,
    visibility: Vec<u8>,
}

impl ShadowField {
    #[inline]
    fn sample(&self, px: f32, py: f32) -> f32 {
        let fx = ((px - self.origin_x) * self.inv_step)
            .clamp(0.0, self.width.saturating_sub(1) as f32);
        let fy = ((py - self.origin_y) * self.inv_step)
            .clamp(0.0, self.height.saturating_sub(1) as f32);
        if !self.smooth {
            let x = fx.round() as usize;
            let y = fy.round() as usize;
            return self.visibility[y * self.width + x] as f32 / 255.0;
        }
        sample_u8_field(&self.visibility, self.width, self.height, fx, fy)
    }
}

/// Low-frequency ambient occlusion is evaluated on a coarser field and
/// bilinearly sampled by the light-map pass. With the default 32px radius an
/// 8px grid is visually smooth but reduces Ultra's AO probes by roughly 64x.
struct AoField {
    origin_x: f32,
    origin_y: f32,
    inv_step: f32,
    width: usize,
    height: usize,
    occlusion: Vec<u8>,
    intensity: f32,
}

impl AoField {
    #[inline]
    fn ambient_at(&self, px: f32, py: f32, ambient: [f32; 3]) -> [f32; 3] {
        let fx = (px - self.origin_x) * self.inv_step;
        let fy = (py - self.origin_y) * self.inv_step;
        if fx < 0.0
            || fy < 0.0
            || fx > self.width.saturating_sub(1) as f32
            || fy > self.height.saturating_sub(1) as f32
        {
            return ambient;
        }
        let occlusion = sample_u8_field(&self.occlusion, self.width, self.height, fx, fy);
        let factor = 1.0 - self.intensity * occlusion;
        [ambient[0] * factor, ambient[1] * factor, ambient[2] * factor]
    }
}

#[inline]
fn sample_u8_field(data: &[u8], width: usize, height: usize, fx: f32, fy: f32) -> f32 {
    let x0 = fx.floor().max(0.0) as usize;
    let y0 = fy.floor().max(0.0) as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let tx = fx - x0 as f32;
    let ty = fy - y0 as f32;
    let top = data[y0 * width + x0] as f32
        + (data[y0 * width + x1] as f32 - data[y0 * width + x0] as f32) * tx;
    let bottom = data[y1 * width + x0] as f32
        + (data[y1 * width + x1] as f32 - data[y1 * width + x0] as f32) * tx;
    (top + (bottom - top) * ty) / 255.0
}

fn blur_u8_field(data: &mut [u8], width: usize, height: usize, radius: usize) {
    if radius == 0 || width == 0 || height == 0 {
        return;
    }
    let mut temp = vec![0u8; data.len()];
    for y in 0..height {
        let row = y * width;
        let mut sum: u32 = data[row..=row + radius.min(width - 1)]
            .iter()
            .map(|value| u32::from(*value))
            .sum();
        for x in 0..width {
            let x0 = x.saturating_sub(radius);
            let x1 = (x + radius).min(width - 1);
            temp[row + x] = (sum / (x1 - x0 + 1) as u32) as u8;
            if x >= radius {
                sum -= u32::from(data[row + x - radius]);
            }
            if let Some(next) = x.checked_add(radius + 1).filter(|next| *next < width) {
                sum += u32::from(data[row + next]);
            }
        }
    }
    for x in 0..width {
        let mut sum: u32 = (0..=radius.min(height - 1))
            .map(|y| u32::from(temp[y * width + x]))
            .sum();
        for y in 0..height {
            let y0 = y.saturating_sub(radius);
            let y1 = (y + radius).min(height - 1);
            data[y * width + x] = (sum / (y1 - y0 + 1) as u32) as u8;
            if y >= radius {
                sum -= u32::from(temp[(y - radius) * width + x]);
            }
            if let Some(next) = y.checked_add(radius + 1).filter(|next| *next < height) {
                sum += u32::from(temp[next * width + x]);
            }
        }
    }
}

/// Resolve a light's per-frame constants once. `bounds` is its world-space
/// influence box (use infinities to disable the per-texel skip).
fn prepare_light(
    light: &Light,
    config: &LightConfig,
    occluders_present: bool,
    sample_scale: u32,
    bounds: (f32, f32, f32, f32),
) -> PreparedLight {
    let cast_shadows = config.shadows_enabled && light.casts_shadows && occluders_present;
    let soft_radius = if light.shadow_softness >= 0.0 {
        light.shadow_softness
    } else {
        config.soft_shadows
    };
    let _ = sample_scale;
    let radius = light.radius.max(1.0);
    let falloff = light.falloff.max(0.1);
    let aim_x = light.angle.cos();
    let aim_y = light.angle.sin();
    let cone = light.cone.max(1e-3);
    let cone_inner = cone * (1.0 - clamp01(light.cone_softness));
    PreparedLight {
        light: *light,
        cast_shadows,
        soft_radius,
        // Shadows are always cast hard (one ray); softness is produced by
        // blurring the finished light map, which is far cheaper than casting
        // many jittered rays per texel.
        shadow_samples: 1,
        color: [
            light.color.r as f32 / 255.0,
            light.color.g as f32 / 255.0,
            light.color.b as f32 / 255.0,
        ],
        min_x: bounds.0,
        min_y: bounds.1,
        max_x: bounds.2,
        max_y: bounds.3,
        radius_sq: radius * radius,
        inv_radius: 1.0 / radius,
        falloff,
        aim_x,
        aim_y,
        cone,
        cone_inner,
        cone_fade_inv: 1.0 / (cone - cone_inner).max(1e-3),
    }
}

/// Prepared distance/cone attenuation. The overwhelmingly common exponents 1
/// and 2 avoid libm's general `powf`, and all light-invariant trig/divisions
/// are resolved before entering the texel loop.
#[inline]
fn prepared_attenuation(light: &PreparedLight, px: f32, py: f32) -> f32 {
    match light.light.kind {
        LightKind::Directional => 1.0,
        LightKind::Point | LightKind::Spot => {
            let dx = px - light.light.x;
            let dy = py - light.light.y;
            let dist_sq = dx * dx + dy * dy;
            if dist_sq >= light.radius_sq {
                return 0.0;
            }
            let dist = dist_sq.sqrt();
            let linear = clamp01(1.0 - dist * light.inv_radius);
            let mut atten = if light.falloff == 2.0 {
                linear * linear
            } else if light.falloff == 1.0 {
                linear
            } else if light.falloff == 3.0 {
                linear * linear * linear
            } else if light.falloff == 4.0 {
                let squared = linear * linear;
                squared * squared
            } else {
                linear.powf(light.falloff)
            };

            if light.light.kind == LightKind::Spot {
                if dist < 1e-4 {
                    return atten;
                }
                let cos_to_pixel = (dx * light.aim_x + dy * light.aim_y) / dist;
                let angle_to_pixel = cos_to_pixel.clamp(-1.0, 1.0).acos();
                if angle_to_pixel >= light.cone {
                    return 0.0;
                }
                if angle_to_pixel > light.cone_inner {
                    atten *= clamp01((light.cone - angle_to_pixel) * light.cone_fade_inv);
                }
            }
            atten
        }
    }
}

/// The light reaching one texel: ambient, minus ambient occlusion, plus every
/// light that contributes meaningfully. Shared by the parallel map build and
/// [`sample_light_at`] so the two never diverge.
#[inline]
fn texel_light(
    px: f32,
    py: f32,
    ambient: [f32; 3],
    prepared: &[PreparedLight],
    occluders: &OccluderIndex,
    ao_region: Option<AoRegion>,
    ao_offsets: &[(f32, f32)],
    shadow_fields: Option<&[Option<ShadowField>]>,
    light_indices: Option<&[usize]>,
) -> [f32; 3] {
    let mut acc = ambient;

    if let Some(ao) = ao_region {
        if px >= ao.min_x && px <= ao.max_x && py >= ao.min_y && py <= ao.max_y {
            let occ = ambient_occlusion(px, py, occluders, ao_offsets);
            if occ > 0.0 {
                let factor = 1.0 - ao.intensity * occ;
                acc[0] *= factor;
                acc[1] *= factor;
                acc[2] *= factor;
            }
        }
    }

    let mut apply_light = |index: usize| {
        let pl = &prepared[index];
        if px < pl.min_x || px > pl.max_x || py < pl.min_y || py > pl.max_y {
            return;
        }
        let atten = prepared_attenuation(pl, px, py);
        let contribution = atten * pl.light.intensity;
        // Skip the (expensive) shadow ray where the light is imperceptible.
        if contribution < MIN_CONTRIBUTION {
            return;
        }
        let vis = if pl.cast_shadows {
            shadow_fields
                .and_then(|fields| fields.get(index))
                .and_then(Option::as_ref)
                .map(|field| field.sample(px, py))
                .unwrap_or_else(|| {
                    visibility(
                        &pl.light,
                        px,
                        py,
                        occluders,
                        pl.soft_radius,
                        pl.shadow_samples,
                    )
                })
        } else {
            1.0
        };
        if vis <= 0.0 {
            return;
        }
        let s = contribution * vis;
        acc[0] += pl.color[0] * s;
        acc[1] += pl.color[1] * s;
        acc[2] += pl.color[2] * s;
    };

    if let Some(indices) = light_indices {
        for &index in indices {
            apply_light(index);
        }
    } else {
        for index in 0..prepared.len() {
            apply_light(index);
        }
    }

    acc
}

/// CSR-style tile lists for lights. Local lights are inserted only into tiles
/// touched by their radius; directional lights naturally occupy every tile.
/// A flat allocation is substantially cheaper to build and traverse than one
/// `Vec` per tile, and preserves the original light accumulation order.
struct LightBins {
    tile_size: usize,
    tiles_x: usize,
    offsets: Vec<usize>,
    indices: Vec<usize>,
}

impl LightBins {
    const TILE_SIZE: usize = 16;

    fn build(prepared: &[PreparedLight], lw: usize, lh: usize, scale: usize) -> Self {
        let tile_size = Self::TILE_SIZE;
        let tiles_x = lw.div_ceil(tile_size).max(1);
        let tiles_y = lh.div_ceil(tile_size).max(1);
        let tile_count = tiles_x * tiles_y;
        let mut counts = vec![0usize; tile_count];
        let mut ranges = Vec::with_capacity(prepared.len());

        for light in prepared {
            let tx0 = world_to_texel(light.min_x, scale, lw) / tile_size;
            let tx1 = world_to_texel(light.max_x, scale, lw) / tile_size;
            let ty0 = world_to_texel(light.min_y, scale, lh) / tile_size;
            let ty1 = world_to_texel(light.max_y, scale, lh) / tile_size;
            ranges.push((tx0, tx1, ty0, ty1));
            for tile_y in ty0..=ty1 {
                let row = tile_y * tiles_x;
                for tile_x in tx0..=tx1 {
                    counts[row + tile_x] += 1;
                }
            }
        }

        let mut offsets = vec![0usize; tile_count + 1];
        for (index, count) in counts.into_iter().enumerate() {
            offsets[index + 1] = offsets[index] + count;
        }
        let mut cursors = offsets[..tile_count].to_vec();
        let mut indices = vec![0usize; offsets[tile_count]];
        for (light_index, &(tx0, tx1, ty0, ty1)) in ranges.iter().enumerate() {
            for tile_y in ty0..=ty1 {
                let row = tile_y * tiles_x;
                for tile_x in tx0..=tx1 {
                    let tile = row + tile_x;
                    indices[cursors[tile]] = light_index;
                    cursors[tile] += 1;
                }
            }
        }

        Self {
            tile_size,
            tiles_x,
            offsets,
            indices,
        }
    }

    #[inline]
    fn at(&self, texel_x: usize, texel_y: usize) -> &[usize] {
        let tile = (texel_y / self.tile_size) * self.tiles_x + texel_x / self.tile_size;
        &self.indices[self.offsets[tile]..self.offsets[tile + 1]]
    }
}

#[inline]
fn world_to_texel(world: f32, scale: usize, texel_len: usize) -> usize {
    if world.is_nan() || world <= 0.0 {
        0
    } else if world == f32::INFINITY {
        texel_len.saturating_sub(1)
    } else {
        ((world / scale as f32) as usize).min(texel_len.saturating_sub(1))
    }
}

/// Build the (possibly downsampled) light map: ambient + AO + all lights. The
/// rows are split across worker threads because each texel is independent.
fn build_light_map(
    fb_width: usize,
    fb_height: usize,
    config: &LightConfig,
    lights: &[Light],
    occluders: &[Occluder],
) -> LightMap {
    let scale = config
        .quality
        .downsample_for_frame(fb_width, fb_height)
        .max(1);
    let sample_scale = config.quality.sample_scale();
    let lw = fb_width.div_ceil(scale).max(1);
    let lh = fb_height.div_ceil(scale).max(1);

    let ambient = [
        config.ambient.r as f32 / 255.0 * config.ambient_intensity,
        config.ambient.g as f32 / 255.0 * config.ambient_intensity,
        config.ambient.b as f32 / 255.0 * config.ambient_intensity,
    ];

    // Resolve occluder rotation/bounds once for the whole frame.
    let occluders = OccluderIndex::new(occluders);

    // Resolve each light's constants once, dropping off-screen ones.
    let occluders_present = !occluders.is_empty();
    let mut prepared: Vec<PreparedLight> = Vec::with_capacity(lights.len());
    for light in lights {
        if light.intensity <= 0.0 {
            continue;
        }
        let bounds = light_bounds(light, fb_width, fb_height);
        if bounds.2 < 0.0
            || bounds.3 < 0.0
            || bounds.0 > fb_width as f32
            || bounds.1 > fb_height as f32
        {
            continue; // fully off-screen
        }
        prepared.push(prepare_light(
            light,
            config,
            occluders_present,
            sample_scale,
            bounds,
        ));
    }

    let ao_region = if config.ao_enabled && config.ao_intensity > 0.0 && occluders_present {
        let pad = config.ao_radius.max(0.0) + 1.0;
        let (min_x, min_y, max_x, max_y) = occluder_bounds(&occluders.occluders, pad);
        Some(AoRegion {
            min_x,
            min_y,
            max_x,
            max_y,
            intensity: config.ao_intensity,
        })
    } else {
        None
    };

    // Ring trig is frame-invariant. Resolve it once instead of once per AO
    // texel (which was especially costly at high/ultra quality).
    let ao_offsets = ao_region
        .filter(|_| config.ao_radius > 0.5)
        .map(|_| {
            let samples = (config.ao_samples * sample_scale).max(1);
            (0..samples)
                .map(|i| {
                    let theta = (i as f32 / samples as f32) * std::f32::consts::TAU;
                    (
                        theta.cos() * config.ao_radius,
                        theta.sin() * config.ao_radius,
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let ao_field = ao_region.and_then(|region| {
        build_ao_field(
            region,
            &ao_offsets,
            &occluders,
            fb_width,
            fb_height,
            config.ao_radius,
        )
    });
    let shadow_fields = build_shadow_fields(
        &prepared,
        &occluders,
        fb_width,
        fb_height,
        scale,
    );

    let mut data = vec![0.0f32; lw * lh * 3];
    let half = scale as f32 * 0.5;
    let step = scale as f32;
    let light_bins = LightBins::build(&prepared, lw, lh, scale);

    // One texel's light value, shared by the sequential and threaded paths.
    let compute_row = |chunk: &mut [f32], row0: usize| {
        let rows = chunk.len() / (lw * 3);
        for local in 0..rows {
            let py = (row0 + local) as f32 * step + half;
            for lx in 0..lw {
                let px = lx as f32 * step + half;
                let texel_ambient = ao_field
                    .as_ref()
                    .map(|field| field.ambient_at(px, py, ambient))
                    .unwrap_or(ambient);
                let acc = texel_light(
                    px,
                    py,
                    texel_ambient,
                    &prepared,
                    &occluders,
                    None,
                    &[],
                    Some(&shadow_fields),
                    Some(light_bins.at(lx, row0 + local)),
                );
                let o = (local * lw + lx) * 3;
                chunk[o] = acc[0];
                chunk[o + 1] = acc[1];
                chunk[o + 2] = acc[2];
            }
        }
    };

    // Parallelize across rows. Each thread owns a disjoint band of the buffer.
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 16)
        .min(lh.max(1));

    if workers <= 1 || lw * lh < 4096 {
        compute_row(&mut data, 0);
    } else {
        let rows_per = lh.div_ceil(workers);
        let band = rows_per * lw * 3;
        let compute_row = &compute_row;
        std::thread::scope(|scope| {
            for (index, chunk) in data.chunks_mut(band).enumerate() {
                scope.spawn(move || compute_row(chunk, index * rows_per));
            }
        });
    }

    LightMap {
        data,
        width: lw,
        height: lh,
        scale,
    }
}

/// Separable reference blur for three-channel maps. The production shadow path
/// now blurs each compact byte visibility field with [`blur_u8_field`]; this
/// helper remains useful for validating the rolling-window implementation.
fn blur_light_map(data: &mut [f32], lw: usize, lh: usize, radius: usize, iterations: usize) {
    if radius == 0 || lw == 0 || lh == 0 {
        return;
    }
    let mut buf = vec![0.0f32; data.len()];
    for _ in 0..iterations {
        // Horizontal pass: data -> buf. Maintain a rolling sum so cost is
        // independent of blur radius (the previous nested window loop became
        // one of the largest costs for wide penumbras).
        for y in 0..lh {
            let row = y * lw;
            let initial_end = radius.min(lw - 1);
            let mut sum = [0.0f32; 3];
            for x in 0..=initial_end {
                let i = (row + x) * 3;
                sum[0] += data[i];
                sum[1] += data[i + 1];
                sum[2] += data[i + 2];
            }
            for x in 0..lw {
                let x0 = x.saturating_sub(radius);
                let x1 = (x + radius).min(lw - 1);
                let n = (x1 - x0 + 1) as f32;
                let o = (row + x) * 3;
                buf[o] = sum[0] / n;
                buf[o + 1] = sum[1] / n;
                buf[o + 2] = sum[2] / n;

                if x >= radius {
                    let leaving = (row + x - radius) * 3;
                    sum[0] -= data[leaving];
                    sum[1] -= data[leaving + 1];
                    sum[2] -= data[leaving + 2];
                }
                if let Some(entering_x) = x.checked_add(radius + 1).filter(|&next| next < lw) {
                    let entering = (row + entering_x) * 3;
                    sum[0] += data[entering];
                    sum[1] += data[entering + 1];
                    sum[2] += data[entering + 2];
                }
            }
        }
        // Vertical pass: buf -> data, also with rolling sums.
        for x in 0..lw {
            let initial_end = radius.min(lh - 1);
            let mut sum = [0.0f32; 3];
            for y in 0..=initial_end {
                let i = (y * lw + x) * 3;
                sum[0] += buf[i];
                sum[1] += buf[i + 1];
                sum[2] += buf[i + 2];
            }
            for y in 0..lh {
                let y0 = y.saturating_sub(radius);
                let y1 = (y + radius).min(lh - 1);
                let n = (y1 - y0 + 1) as f32;
                let o = (y * lw + x) * 3;
                data[o] = sum[0] / n;
                data[o + 1] = sum[1] / n;
                data[o + 2] = sum[2] / n;

                if y >= radius {
                    let leaving = ((y - radius) * lw + x) * 3;
                    sum[0] -= buf[leaving];
                    sum[1] -= buf[leaving + 1];
                    sum[2] -= buf[leaving + 2];
                }
                if let Some(entering_y) = y.checked_add(radius + 1).filter(|&next| next < lh) {
                    let entering = (entering_y * lw + x) * 3;
                    sum[0] += buf[entering];
                    sum[1] += buf[entering + 1];
                    sum[2] += buf[entering + 2];
                }
            }
        }
    }
}

/// Screen-space bounding box a light can influence.
fn light_bounds(light: &Light, fb_width: usize, fb_height: usize) -> (f32, f32, f32, f32) {
    match light.kind {
        LightKind::Directional => (0.0, 0.0, fb_width as f32, fb_height as f32),
        LightKind::Point | LightKind::Spot => {
            let r = light.radius.max(1.0);
            (light.x - r, light.y - r, light.x + r, light.y + r)
        }
    }
}

/// Union bounding box of all occluders, padded on every side.
fn occluder_bounds(occluders: &[PreparedOccluder], pad: f32) -> (f32, f32, f32, f32) {
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for occ in occluders {
        let r = occ.bound_radius_sq.sqrt() + pad;
        min_x = min_x.min(occ.cx - r);
        min_y = min_y.min(occ.cy - r);
        max_x = max_x.max(occ.cx + r);
        max_y = max_y.max(occ.cy + r);
    }
    (min_x, min_y, max_x, max_y)
}

/// Build the light map and multiply it over the RGBA `pixels` in place. Alpha
/// is preserved. This is the whole lighting pass.
pub(crate) fn apply_lighting(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    config: &LightConfig,
    lights: &[Light],
    occluders: &[Occluder],
) {
    let mut cache = LightMapCache::default();
    apply_lighting_cached(pixels, width, height, config, lights, occluders, &mut cache);
}

/// Cached form used by long-lived renderers. Only the final framebuffer
/// multiply remains on frames whose lights, occluders, config, and dimensions
/// are unchanged.
pub(crate) fn apply_lighting_cached(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    config: &LightConfig,
    lights: &[Light],
    occluders: &[Occluder],
    cache: &mut LightMapCache,
) {
    if width == 0 || height == 0 || config.is_noop(lights, occluders) {
        return;
    }
    let w = width as usize;
    let h = height as usize;
    if pixels.len() < w * h * 4 {
        return;
    }

    let (_, map) = cache.prepare(width, height, config, lights, occluders);
    let lightmap = map.data.as_slice();
    let lw = map.width;
    let lh = map.height;
    let scale = map.scale;

    let exposure = if config.exposure > 0.0 {
        config.exposure
    } else {
        1.0
    };
    let inv_scale = 1.0 / scale as f32;
    let bloom = config.bloom;
    // Composite one band of framebuffer rows (multiply scene by the light map).
    let composite_rows = move |chunk: &mut [u8], row0: usize| {
        let rows = chunk.len() / (w * 4);
        for local in 0..rows {
            let y = row0 + local;
            let fy = (y as f32 * inv_scale).min(lh as f32 - 1.0);
            for x in 0..w {
                let (mut lr, mut lg, mut lb) = if scale == 1 {
                    let idx = (y * lw + x) * 3;
                    (lightmap[idx], lightmap[idx + 1], lightmap[idx + 2])
                } else {
                    let fx = (x as f32 * inv_scale).min(lw as f32 - 1.0);
                    sample_lightmap(lightmap, lw, lh, fx, fy)
                };
                lr *= exposure;
                lg *= exposure;
                lb *= exposure;

                let idx = (local * w + x) * 4;
                let sr = chunk[idx] as f32 / 255.0;
                let sg = chunk[idx + 1] as f32 / 255.0;
                let sb = chunk[idx + 2] as f32 / 255.0;

                let mut or = sr * lr;
                let mut og = sg * lg;
                let mut ob = sb * lb;

                if bloom > 0.0 {
                    or += (lr - 1.0).max(0.0) * bloom * sr;
                    og += (lg - 1.0).max(0.0) * bloom * sg;
                    ob += (lb - 1.0).max(0.0) * bloom * sb;
                }

                chunk[idx] = (clamp01(or) * 255.0).round() as u8;
                chunk[idx + 1] = (clamp01(og) * 255.0).round() as u8;
                chunk[idx + 2] = (clamp01(ob) * 255.0).round() as u8;
                // Alpha (chunk[idx + 3]) is left untouched.
            }
        }
    };

    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 16)
        .min(h.max(1));
    if workers <= 1 || w * h < 4096 {
        composite_rows(&mut pixels[..h * w * 4], 0);
    } else {
        let rows_per = h.div_ceil(workers);
        let band = rows_per * w * 4;
        let composite_rows = &composite_rows;
        std::thread::scope(|scope| {
            for (index, chunk) in pixels[..h * w * 4].chunks_mut(band).enumerate() {
                scope.spawn(move || composite_rows(chunk, index * rows_per));
            }
        });
    }
}

/// A downsampled light map encoded as RGBA8, for uploading to the GPU. Each
/// texel is the clamped light color (× exposure); the GPU multiplies it over
/// the scene with a linear sampler, matching the software composite. Bloom and
/// over-bright (> 1) light are not represented — the GPU path is a plain
/// multiply.
#[derive(Clone)]
pub(crate) struct LightMapImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Build the light map for a GPU composite, or `None` when lighting would not
/// change the frame.
pub(crate) fn render_light_map(
    fb_width: u32,
    fb_height: u32,
    config: &LightConfig,
    lights: &[Light],
    occluders: &[Occluder],
) -> Option<LightMapImage> {
    let mut cache = LightMapCache::default();
    render_light_map_cached(fb_width, fb_height, config, lights, occluders, &mut cache)
        .map(|(_, image)| (*image).clone())
}

/// Cached GPU light-map encoding. The generation changes only when exact
/// lighting inputs change, allowing the presenter to retain the descriptor and
/// device image instead of synchronously reallocating/uploading every frame.
pub(crate) fn render_light_map_cached(
    fb_width: u32,
    fb_height: u32,
    config: &LightConfig,
    lights: &[Light],
    occluders: &[Occluder],
    cache: &mut LightMapCache,
) -> Option<(u64, Arc<LightMapImage>)> {
    if fb_width == 0 || fb_height == 0 || config.is_noop(lights, occluders) {
        return None;
    }
    let (generation, map) = cache.prepare(fb_width, fb_height, config, lights, occluders);
    if cache.image.is_none() {
        let exposure = if config.exposure > 0.0 {
            config.exposure
        } else {
            1.0
        };
        let map_width = map.width;
        let map_height = map.height;
        let mut rgba = vec![0u8; map_width * map_height * 4];
        let encode = |output: &mut [u8], texel0: usize| {
            let input = &map.data[texel0 * 3..texel0 * 3 + output.len() / 4 * 3];
            for (texel, pixel) in input.chunks_exact(3).zip(output.chunks_exact_mut(4)) {
                pixel[0] = (clamp01(texel[0] * exposure) * 255.0).round() as u8;
                pixel[1] = (clamp01(texel[1] * exposure) * 255.0).round() as u8;
                pixel[2] = (clamp01(texel[2] * exposure) * 255.0).round() as u8;
                pixel[3] = 255;
            }
        };
        let workers = std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1)
            .clamp(1, 16)
            .min(map_height.max(1));
        if workers <= 1 || map_width * map_height < 4096 {
            encode(&mut rgba, 0);
        } else {
            let rows_per = map_height.div_ceil(workers);
            let band = rows_per * map_width * 4;
            let encode = &encode;
            std::thread::scope(|scope| {
                for (index, output) in rgba.chunks_mut(band).enumerate() {
                    scope.spawn(move || encode(output, index * rows_per * map_width));
                }
            });
        }
        cache.image = Some(Arc::new(LightMapImage {
            width: map_width as u32,
            height: map_height as u32,
            rgba,
        }));
    }
    Some((
        generation,
        Arc::clone(cache.image.as_ref().expect("encoded light map missing")),
    ))
}

/// Sample the light multiplier (rgb, each `>= 0`) at a single world point. Used
/// by the editor's per-object preview so lights actually brighten/darken
/// objects instead of painting a flat overlay.
pub(crate) fn sample_light_at(
    x: f32,
    y: f32,
    config: &LightConfig,
    lights: &[Light],
    occluders: &[Occluder],
) -> (f32, f32, f32) {
    LightSampler::new(config, lights, occluders).sample(x, y)
}

/// A prepared point-sampler for the light at arbitrary positions. Building it
/// resolves lights and occluders once; `sample` is then cheap, so callers that
/// probe many points (such as the editor preview) build one per frame and
/// reuse it.
pub(crate) struct LightSampler {
    ambient: [f32; 3],
    exposure: f32,
    prepared: Vec<PreparedLight>,
    occluders: OccluderIndex,
    ao_region: Option<AoRegion>,
    ao_offsets: Vec<(f32, f32)>,
}

impl LightSampler {
    pub(crate) fn new(config: &LightConfig, lights: &[Light], occluders: &[Occluder]) -> Self {
        let ambient = [
            config.ambient.r as f32 / 255.0 * config.ambient_intensity,
            config.ambient.g as f32 / 255.0 * config.ambient_intensity,
            config.ambient.b as f32 / 255.0 * config.ambient_intensity,
        ];
        let occluders = OccluderIndex::new(occluders);
        let occluders_present = !occluders.is_empty();
        let sample_scale = config.quality.sample_scale();
        let infinite = (
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
            f32::INFINITY,
            f32::INFINITY,
        );
        let prepared: Vec<PreparedLight> = lights
            .iter()
            .filter(|l| l.intensity > 0.0)
            .map(|l| prepare_light(l, config, occluders_present, sample_scale, infinite))
            .collect();

        let ao_region = if config.ao_enabled && config.ao_intensity > 0.0 && occluders_present {
            let pad = config.ao_radius.max(0.0) + 1.0;
            let (min_x, min_y, max_x, max_y) = occluder_bounds(&occluders.occluders, pad);
            Some(AoRegion {
                min_x,
                min_y,
                max_x,
                max_y,
                intensity: config.ao_intensity,
            })
        } else {
            None
        };
        let ao_offsets = ao_region
            .filter(|_| config.ao_radius > 0.5)
            .map(|_| {
                let samples = (config.ao_samples * sample_scale).max(1);
                (0..samples)
                    .map(|i| {
                        let theta = (i as f32 / samples as f32) * std::f32::consts::TAU;
                        (
                            theta.cos() * config.ao_radius,
                            theta.sin() * config.ao_radius,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        Self {
            ambient,
            exposure: if config.exposure > 0.0 {
                config.exposure
            } else {
                1.0
            },
            prepared,
            occluders,
            ao_region,
            ao_offsets,
        }
    }

    pub(crate) fn sample(&self, x: f32, y: f32) -> (f32, f32, f32) {
        let acc = texel_light(
            x,
            y,
            self.ambient,
            &self.prepared,
            &self.occluders,
            self.ao_region,
            &self.ao_offsets,
            None,
            None,
        );
        (
            acc[0] * self.exposure,
            acc[1] * self.exposure,
            acc[2] * self.exposure,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_gray(w: u32, h: u32, value: u8) -> Vec<u8> {
        vec![value; (w * h * 4) as usize]
    }

    fn box_occluder(cx: f32, cy: f32, half_w: f32, half_h: f32) -> Occluder {
        Occluder {
            cx,
            cy,
            half_w,
            half_h,
            rotation: 0.0,
            shape: OccluderShape::Box,
        }
    }

    #[test]
    fn disabled_config_leaves_pixels_untouched() {
        let mut pixels = solid_gray(4, 4, 200);
        let original = pixels.clone();
        let config = LightConfig::default();
        apply_lighting(&mut pixels, 4, 4, &config, &[], &[]);
        assert_eq!(pixels, original);
    }

    #[test]
    fn ultra_adapts_only_when_the_full_resolution_map_exceeds_its_budget() {
        assert_eq!(LightQuality::Ultra.downsample_for_frame(1280, 720), 1);
        assert_eq!(LightQuality::Ultra.downsample_for_frame(1920, 1080), 2);
        assert_eq!(LightQuality::Ultra.downsample_for_frame(2560, 1440), 2);
        assert_eq!(LightQuality::Ultra.downsample_for_frame(3840, 2160), 3);

        // Other quality levels keep their explicit, predictable scales.
        assert_eq!(LightQuality::High.downsample_for_frame(3840, 2160), 1);
        assert_eq!(LightQuality::Medium.downsample_for_frame(3840, 2160), 2);
        assert_eq!(LightQuality::Low.downsample_for_frame(3840, 2160), 4);
    }

    #[test]
    fn zero_ambient_with_no_lights_goes_black() {
        let mut pixels = solid_gray(8, 8, 255);
        let mut config = LightConfig::default();
        config.enabled = true;
        config.ambient_intensity = 0.0;
        apply_lighting(&mut pixels, 8, 8, &config, &[], &[]);
        assert!(
            pixels
                .chunks_exact(4)
                .all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0)
        );
    }

    #[test]
    fn point_light_brightens_its_center_more_than_the_edge() {
        let (w, h) = (64u32, 64u32);
        let mut pixels = solid_gray(w, h, 128);
        let mut config = LightConfig::default();
        config.enabled = true;
        config.ambient_intensity = 0.0;
        config.quality = LightQuality::High;
        let light = Light {
            kind: LightKind::Point,
            x: 32.0,
            y: 32.0,
            radius: 30.0,
            intensity: 1.0,
            ..Light::default()
        };
        apply_lighting(&mut pixels, w, h, &config, &[light], &[]);

        let sample = |x: usize, y: usize| pixels[(y * w as usize + x) * 4] as u32;
        let center = sample(32, 32);
        let edge = sample(50, 32);
        let outside = sample(2, 2);
        assert!(center > edge, "center {center} should exceed edge {edge}");
        assert_eq!(outside, 0, "pixels beyond the radius stay dark");
    }

    #[test]
    fn occluder_casts_a_shadow_away_from_a_point_light() {
        let (w, h) = (96u32, 32u32);
        let mut pixels = solid_gray(w, h, 200);
        let mut config = LightConfig::default();
        config.enabled = true;
        config.ambient_intensity = 0.0;
        config.shadows_enabled = true;
        config.quality = LightQuality::High;
        let light = Light {
            kind: LightKind::Point,
            x: 8.0,
            y: 16.0,
            radius: 200.0,
            falloff: 1.0,
            intensity: 1.0,
            ..Light::default()
        };
        let occ = box_occluder(40.0, 16.0, 4.0, 4.0);
        apply_lighting(&mut pixels, w, h, &config, &[light], &[occ]);
        let sample = |x: usize, y: usize| pixels[(y * w as usize + x) * 4] as u32;
        let shadowed = sample(70, 16);
        let lit = sample(70, 2);
        assert!(
            shadowed < lit,
            "shadow {shadowed} should be darker than lit {lit}"
        );
    }

    #[test]
    fn circle_occluder_blocks_light() {
        let (w, h) = (96u32, 32u32);
        let mut pixels = solid_gray(w, h, 200);
        let mut config = LightConfig::default();
        config.enabled = true;
        config.ambient_intensity = 0.0;
        config.quality = LightQuality::High;
        let light = Light {
            x: 8.0,
            y: 16.0,
            radius: 200.0,
            falloff: 1.0,
            ..Light::default()
        };
        let occ = Occluder {
            cx: 40.0,
            cy: 16.0,
            half_w: 6.0,
            half_h: 6.0,
            rotation: 0.0,
            shape: OccluderShape::Circle,
        };
        apply_lighting(&mut pixels, w, h, &config, &[light], &[occ]);
        let sample = |x: usize, y: usize| pixels[(y * w as usize + x) * 4] as u32;
        assert!(
            sample(70, 16) < sample(70, 2),
            "circle occluder should shadow"
        );
    }

    #[test]
    fn scatter_matches_a_naive_reference() {
        // The bounding-box scatter must produce the same result as evaluating
        // every light at every texel.
        let (w, h) = (48usize, 48usize);
        let mut config = LightConfig::default();
        config.enabled = true;
        config.ambient_intensity = 0.3;
        config.quality = LightQuality::High;
        let lights = [
            Light {
                x: 12.0,
                y: 12.0,
                radius: 20.0,
                ..Light::default()
            },
            Light {
                kind: LightKind::Point,
                x: 36.0,
                y: 30.0,
                radius: 25.0,
                color: Color::rgba(120, 200, 255, 255),
                ..Light::default()
            },
        ];
        let occ = [box_occluder(24.0, 24.0, 3.0, 8.0)];

        let map = build_light_map(w, h, &config, &lights, &occ);
        for ty in 0..map.height {
            for tx in 0..map.width {
                let px = tx as f32 + 0.5;
                let py = ty as f32 + 0.5;
                let (rr, gg, bb) = sample_light_at(px, py, &config, &lights, &occ);
                let idx = (ty * map.width + tx) * 3;
                assert!((map.data[idx] - rr).abs() < 1e-3, "r mismatch at {tx},{ty}");
                assert!(
                    (map.data[idx + 1] - gg).abs() < 1e-3,
                    "g mismatch at {tx},{ty}"
                );
                assert!(
                    (map.data[idx + 2] - bb).abs() < 1e-3,
                    "b mismatch at {tx},{ty}"
                );
            }
        }
    }

    #[test]
    fn segment_box_intersection_basic_cases() {
        assert!(segment_hits_box((-5.0, 0.0), (5.0, 0.0), 1.0, 1.0));
        assert!(!segment_hits_box((-5.0, 5.0), (5.0, 5.0), 1.0, 1.0));
        assert!(segment_hits_box((0.0, 0.0), (10.0, 0.0), 1.0, 1.0));
    }

    #[test]
    fn prepared_attenuation_matches_reference_for_supported_light_kinds() {
        let config = LightConfig::default();
        for kind in [LightKind::Point, LightKind::Spot, LightKind::Directional] {
            for falloff in [0.25, 1.0, 2.0, 3.0, 4.0, 5.5] {
                let light = Light {
                    kind,
                    x: 31.0,
                    y: 27.0,
                    radius: 41.0,
                    falloff,
                    angle: 0.73,
                    cone: 0.91,
                    cone_softness: 0.42,
                    ..Light::default()
                };
                let prepared = prepare_light(
                    &light,
                    &config,
                    false,
                    1,
                    (
                        f32::NEG_INFINITY,
                        f32::NEG_INFINITY,
                        f32::INFINITY,
                        f32::INFINITY,
                    ),
                );
                for y in (0..80).step_by(7) {
                    for x in (0..80).step_by(5) {
                        let expected = attenuation(&light, x as f32 + 0.25, y as f32 + 0.75);
                        let actual =
                            prepared_attenuation(&prepared, x as f32 + 0.25, y as f32 + 0.75);
                        assert!(
                            (expected - actual).abs() < 2e-6,
                            "{kind:?} falloff {falloff} mismatch at {x},{y}: {expected} vs {actual}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn occluder_bvh_matches_linear_shape_tests() {
        let occluders: Vec<_> = (0..37)
            .map(|index| Occluder {
                cx: 10.0 + (index % 8) as f32 * 19.0,
                cy: 8.0 + (index / 8) as f32 * 23.0,
                half_w: 2.0 + (index % 5) as f32,
                half_h: 3.0 + (index % 7) as f32,
                rotation: index as f32 * 0.13,
                shape: if index % 2 == 0 {
                    OccluderShape::Box
                } else {
                    OccluderShape::Circle
                },
            })
            .collect();
        let index = OccluderIndex::new(&occluders);
        let linear = |start: (f32, f32), end: (f32, f32)| {
            index.occluders.iter().any(|occ| {
                if point_segment_dist_sq(occ.cx, occ.cy, start, end) > occ.bound_radius_sq {
                    return false;
                }
                local_segment_hits(
                    occ,
                    to_local(occ, start.0, start.1),
                    to_local(occ, end.0, end.1),
                )
            })
        };

        for ray in 0..200 {
            let start = (-30.0, (ray * 17 % 140) as f32 - 10.0);
            let end = (190.0, (ray * 43 % 150) as f32 - 20.0);
            assert_eq!(index.segment_occluded(start, end), linear(start, end));
        }
    }

    #[test]
    fn rolling_blur_matches_naive_window_blur() {
        let (w, h, radius, iterations) = (19usize, 13usize, 4usize, 2usize);
        let mut optimized: Vec<f32> = (0..w * h * 3)
            .map(|index| ((index * 37 % 101) as f32) / 100.0)
            .collect();
        let mut expected = optimized.clone();
        let mut scratch = vec![0.0f32; expected.len()];
        for _ in 0..iterations {
            for y in 0..h {
                for x in 0..w {
                    let x0 = x.saturating_sub(radius);
                    let x1 = (x + radius).min(w - 1);
                    for channel in 0..3 {
                        let sum: f32 = (x0..=x1)
                            .map(|xx| expected[(y * w + xx) * 3 + channel])
                            .sum();
                        scratch[(y * w + x) * 3 + channel] = sum / (x1 - x0 + 1) as f32;
                    }
                }
            }
            for y in 0..h {
                let y0 = y.saturating_sub(radius);
                let y1 = (y + radius).min(h - 1);
                for x in 0..w {
                    for channel in 0..3 {
                        let sum: f32 = (y0..=y1)
                            .map(|yy| scratch[(yy * w + x) * 3 + channel])
                            .sum();
                        expected[(y * w + x) * 3 + channel] = sum / (y1 - y0 + 1) as f32;
                    }
                }
            }
        }

        blur_light_map(&mut optimized, w, h, radius, iterations);
        for (index, (actual, expected)) in optimized.iter().zip(&expected).enumerate() {
            assert!(
                (actual - expected).abs() < 2e-6,
                "blur mismatch at {index}: {actual} vs {expected}"
            );
        }
    }

    #[test]
    fn light_map_cache_reuses_and_invalidates_exact_inputs() {
        let mut config = LightConfig::default();
        config.enabled = true;
        config.ambient_intensity = 0.1;
        let mut lights = vec![Light {
            x: 30.0,
            y: 30.0,
            radius: 25.0,
            ..Light::default()
        }];
        let mut cache = LightMapCache::default();

        let (first_generation, first) =
            render_light_map_cached(64, 64, &config, &lights, &[], &mut cache).unwrap();
        let (same_generation, same) =
            render_light_map_cached(64, 64, &config, &lights, &[], &mut cache).unwrap();
        assert_eq!(first_generation, same_generation);
        assert!(Arc::ptr_eq(&first, &same));

        lights[0].x += 1.0;
        let (moved_generation, moved) =
            render_light_map_cached(64, 64, &config, &lights, &[], &mut cache).unwrap();
        assert_ne!(first_generation, moved_generation);
        assert!(!Arc::ptr_eq(&first, &moved));
    }

    #[test]
    #[ignore = "performance benchmark; run in release mode with --ignored --nocapture"]
    fn benchmark_dynamic_and_cached_light_maps() {
        use std::time::Instant;

        let mut config = LightConfig::default();
        config.enabled = true;
        config.ambient_intensity = 0.08;
        config.quality = LightQuality::Medium;
        let mut lights: Vec<_> = (0..24)
            .map(|index| Light {
                x: 80.0 + (index % 8) as f32 * 150.0,
                y: 70.0 + (index / 8) as f32 * 250.0,
                radius: 190.0,
                ..Light::default()
            })
            .collect();
        let occluders: Vec<_> = (0..40)
            .map(|index| {
                box_occluder(
                    30.0 + (index % 10) as f32 * 125.0,
                    40.0 + (index / 10) as f32 * 170.0,
                    10.0,
                    18.0,
                )
            })
            .collect();
        let mut cache = LightMapCache::default();

        let dynamic_start = Instant::now();
        for frame in 0..6 {
            lights[0].x += 0.25 + frame as f32 * 0.01;
            let _ = render_light_map_cached(1280, 720, &config, &lights, &occluders, &mut cache);
        }
        let dynamic_per_frame = dynamic_start.elapsed() / 6;

        let cached_start = Instant::now();
        for _ in 0..200 {
            let _ = render_light_map_cached(1280, 720, &config, &lights, &occluders, &mut cache);
        }
        let cached_per_frame = cached_start.elapsed() / 200;
        eprintln!(
            "dynamic light-map: {dynamic_per_frame:?}/frame; cached light-map: {cached_per_frame:?}/frame"
        );
    }

    #[test]
    #[ignore = "performance benchmark; run in release mode with --ignored --nocapture"]
    fn benchmark_ultra_dynamic_light_maps() {
        use std::time::Instant;

        let full_config = LightConfig {
            enabled: true,
            ambient_intensity: 0.08,
            ao_enabled: true,
            ao_radius: 32.0,
            ao_intensity: 0.6,
            ao_samples: 12,
            shadows_enabled: true,
            soft_shadows: 4.0,
            quality: LightQuality::Ultra,
            ..LightConfig::default()
        };
        let mut lights: Vec<_> = (0..16)
            .map(|index| Light {
                x: 120.0 + (index % 8) as f32 * 230.0,
                y: 150.0 + (index / 8) as f32 * 650.0,
                radius: 260.0,
                ..Light::default()
            })
            .collect();
        let occluders: Vec<_> = (0..40)
            .map(|index| {
                box_occluder(
                    55.0 + (index % 10) as f32 * 195.0,
                    80.0 + (index / 10) as f32 * 270.0,
                    14.0,
                    28.0,
                )
            })
            .collect();
        let mut measure = |label: &str, config: LightConfig| {
            let mut cache = LightMapCache::default();
            let started = Instant::now();
            for frame in 0..3 {
                lights[0].x += 0.25 + frame as f32 * 0.01;
                let _ = render_light_map_cached(
                    1920,
                    1080,
                    &config,
                    &lights,
                    &occluders,
                    &mut cache,
                );
            }
            eprintln!("ultra {label}: {:?}/frame", started.elapsed() / 3);
        };
        measure(
            "lights only",
            LightConfig {
                ao_enabled: false,
                shadows_enabled: false,
                ..full_config
            },
        );
        measure(
            "shadows",
            LightConfig {
                ao_enabled: false,
                ..full_config
            },
        );
        measure(
            "ambient occlusion",
            LightConfig {
                shadows_enabled: false,
                ..full_config
            },
        );
        measure("shadows + ambient occlusion", full_config);
    }
}
