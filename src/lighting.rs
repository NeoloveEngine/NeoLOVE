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
//! Performance: the light map is built by *scattering* each light only across
//! its own bounding box, ambient occlusion is only evaluated near occluders,
//! and shadow ray tests reject with a per-occluder bounding circle first, so the
//! cost tracks the lit area rather than `pixels * lights * occluders`.

use crate::platform::Color;

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
#[derive(Clone, Copy, Debug)]
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
#[derive(Clone, Copy, Debug)]
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

/// Resolution of the intermediate light map relative to the framebuffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LightQuality {
    /// Quarter resolution: fastest, softest.
    Low,
    /// Half resolution: the default balance.
    Medium,
    /// Full resolution.
    High,
    /// Full resolution with extra shadow/AO samples.
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
#[derive(Clone, Copy, Debug)]
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

/// Transform a world point into an occluder's local (unrotated, centered) frame.
#[inline]
fn to_local(occ: &Occluder, x: f32, y: f32) -> (f32, f32) {
    let dx = x - occ.cx;
    let dy = y - occ.cy;
    let cos_r = (-occ.rotation).cos();
    let sin_r = (-occ.rotation).sin();
    (dx * cos_r - dy * sin_r, dx * sin_r + dy * cos_r)
}

#[inline]
fn point_in_occluder(occ: &Occluder, x: f32, y: f32) -> bool {
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
fn local_segment_hits(occ: &Occluder, p0: (f32, f32), p1: (f32, f32)) -> bool {
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

/// Whether the segment from a light sample to a pixel is blocked by any occluder.
fn segment_occluded(sample: (f32, f32), pixel: (f32, f32), occluders: &[Occluder]) -> bool {
    occluders.iter().any(|occ| {
        // Cheap reject: if the whole occluder is farther from the ray than its
        // bounding circle, it cannot block.
        let radius = occ.bound_radius();
        if point_segment_dist_sq(occ.cx, occ.cy, sample, pixel) > radius * radius {
            return false;
        }
        let p0 = to_local(occ, sample.0, sample.1);
        let p1 = to_local(occ, pixel.0, pixel.1);
        local_segment_hits(occ, p0, p1)
    })
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
    occluders: &[Occluder],
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
        return if segment_occluded(origin, (px, py), occluders) {
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
        if !segment_occluded((jx, jy), (px, py), occluders) {
            visible += 1;
        }
    }
    visible as f32 / samples as f32
}

/// Ambient-occlusion darkening in `0..=1` (1 = fully occluded) by sampling a
/// ring around the pixel and measuring how much lands inside occluders.
fn ambient_occlusion(px: f32, py: f32, occluders: &[Occluder], radius: f32, samples: u32) -> f32 {
    if radius <= 0.5 || samples == 0 {
        return 0.0;
    }
    if occluders.iter().any(|occ| point_in_occluder(occ, px, py)) {
        return 1.0;
    }
    let mut occluded = 0u32;
    for ring in 1..=2 {
        let r = radius * (ring as f32 / 2.0);
        for i in 0..samples {
            let theta = (i as f32 / samples as f32) * std::f32::consts::TAU;
            let sx = px + theta.cos() * r;
            let sy = py + theta.sin() * r;
            if occluders.iter().any(|occ| point_in_occluder(occ, sx, sy)) {
                occluded += 3 - ring as u32;
            }
        }
    }
    let max_weight = samples * (2 + 1);
    clamp01(occluded as f32 / max_weight as f32)
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

/// Build the (possibly downsampled) light map: ambient + AO, then each light
/// scattered across its own bounding box.
fn build_light_map(
    fb_width: usize,
    fb_height: usize,
    config: &LightConfig,
    lights: &[Light],
    occluders: &[Occluder],
) -> LightMap {
    let scale = config.quality.downsample().max(1);
    let sample_scale = config.quality.sample_scale();
    let lw = fb_width.div_ceil(scale).max(1);
    let lh = fb_height.div_ceil(scale).max(1);

    let ambient = [
        config.ambient.r as f32 / 255.0 * config.ambient_intensity,
        config.ambient.g as f32 / 255.0 * config.ambient_intensity,
        config.ambient.b as f32 / 255.0 * config.ambient_intensity,
    ];

    let mut data = vec![0.0f32; lw * lh * 3];

    // Texel center in framebuffer space.
    let to_world = |i: usize| i as f32 * scale as f32 + scale as f32 * 0.5;
    // Framebuffer coordinate to texel index (clamped).
    let to_texel = |c: f32, len: usize| ((c / scale as f32) as isize).clamp(0, len as isize - 1) as usize;

    // Ambient fill.
    for texel in data.chunks_exact_mut(3) {
        texel[0] = ambient[0];
        texel[1] = ambient[1];
        texel[2] = ambient[2];
    }

    // Ambient occlusion, only near occluders (their union bbox padded by radius).
    if config.ao_enabled && config.ao_intensity > 0.0 && !occluders.is_empty() {
        let ao_samples = (config.ao_samples * sample_scale).max(1);
        let pad = config.ao_radius.max(0.0) + 1.0;
        let (min_x, min_y, max_x, max_y) = occluder_bounds(occluders, pad);
        let tx0 = to_texel(min_x, lw);
        let tx1 = to_texel(max_x, lw);
        let ty0 = to_texel(min_y, lh);
        let ty1 = to_texel(max_y, lh);
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                let px = to_world(tx);
                let py = to_world(ty);
                let ao = ambient_occlusion(px, py, occluders, config.ao_radius, ao_samples);
                if ao > 0.0 {
                    let factor = 1.0 - config.ao_intensity * ao;
                    let idx = (ty * lw + tx) * 3;
                    data[idx] *= factor;
                    data[idx + 1] *= factor;
                    data[idx + 2] *= factor;
                }
            }
        }
    }

    let shadow_samples_base = (8 * sample_scale).max(4);

    // Scatter each light across only the texels it can reach.
    for light in lights {
        if light.intensity <= 0.0 {
            continue;
        }
        let (min_x, min_y, max_x, max_y) = light_bounds(light, fb_width, fb_height);
        if max_x < 0.0 || max_y < 0.0 || min_x > fb_width as f32 || min_y > fb_height as f32 {
            continue; // fully off-screen
        }
        let tx0 = to_texel(min_x.max(0.0), lw);
        let tx1 = to_texel(max_x.min(fb_width as f32), lw);
        let ty0 = to_texel(min_y.max(0.0), lh);
        let ty1 = to_texel(max_y.min(fb_height as f32), lh);

        let cast_shadows = config.shadows_enabled && light.casts_shadows && !occluders.is_empty();
        let soft_radius = if light.shadow_softness >= 0.0 {
            light.shadow_softness
        } else {
            config.soft_shadows
        };
        let shadow_samples = if soft_radius > 0.5 { shadow_samples_base } else { 1 };

        let lr = light.color.r as f32 / 255.0;
        let lg = light.color.g as f32 / 255.0;
        let lb = light.color.b as f32 / 255.0;

        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                let px = to_world(tx);
                let py = to_world(ty);
                let atten = attenuation(light, px, py);
                if atten <= 0.0 {
                    continue;
                }
                let vis = if cast_shadows {
                    visibility(light, px, py, occluders, soft_radius, shadow_samples)
                } else {
                    1.0
                };
                if vis <= 0.0 {
                    continue;
                }
                let s = atten * vis * light.intensity;
                let idx = (ty * lw + tx) * 3;
                data[idx] += lr * s;
                data[idx + 1] += lg * s;
                data[idx + 2] += lb * s;
            }
        }
    }

    LightMap {
        data,
        width: lw,
        height: lh,
        scale,
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
fn occluder_bounds(occluders: &[Occluder], pad: f32) -> (f32, f32, f32, f32) {
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for occ in occluders {
        let r = occ.bound_radius() + pad;
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
    if width == 0 || height == 0 || config.is_noop(lights, occluders) {
        return;
    }
    let w = width as usize;
    let h = height as usize;
    if pixels.len() < w * h * 4 {
        return;
    }

    let map = build_light_map(w, h, config, lights, occluders);
    let LightMap {
        data: lightmap,
        width: lw,
        height: lh,
        scale,
    } = map;

    let exposure = if config.exposure > 0.0 {
        config.exposure
    } else {
        1.0
    };
    let inv_scale = 1.0 / scale as f32;
    for y in 0..h {
        let fy = (y as f32 * inv_scale).min(lh as f32 - 1.0);
        for x in 0..w {
            let fx = (x as f32 * inv_scale).min(lw as f32 - 1.0);
            let (mut lr, mut lg, mut lb) = if scale == 1 {
                let idx = (y * lw + x) * 3;
                (lightmap[idx], lightmap[idx + 1], lightmap[idx + 2])
            } else {
                sample_lightmap(&lightmap, lw, lh, fx, fy)
            };
            lr *= exposure;
            lg *= exposure;
            lb *= exposure;

            let idx = (y * w + x) * 4;
            let sr = pixels[idx] as f32 / 255.0;
            let sg = pixels[idx + 1] as f32 / 255.0;
            let sb = pixels[idx + 2] as f32 / 255.0;

            let mut or = sr * lr;
            let mut og = sg * lg;
            let mut ob = sb * lb;

            if config.bloom > 0.0 {
                or += (lr - 1.0).max(0.0) * config.bloom * sr;
                og += (lg - 1.0).max(0.0) * config.bloom * sg;
                ob += (lb - 1.0).max(0.0) * config.bloom * sb;
            }

            pixels[idx] = (clamp01(or) * 255.0).round() as u8;
            pixels[idx + 1] = (clamp01(og) * 255.0).round() as u8;
            pixels[idx + 2] = (clamp01(ob) * 255.0).round() as u8;
            // Alpha (pixels[idx + 3]) is left untouched.
        }
    }
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
    let mut r = config.ambient.r as f32 / 255.0 * config.ambient_intensity;
    let mut g = config.ambient.g as f32 / 255.0 * config.ambient_intensity;
    let mut b = config.ambient.b as f32 / 255.0 * config.ambient_intensity;

    if config.ao_enabled && config.ao_intensity > 0.0 && !occluders.is_empty() {
        let ao = ambient_occlusion(x, y, occluders, config.ao_radius, config.ao_samples.max(1));
        let factor = 1.0 - config.ao_intensity * ao;
        r *= factor;
        g *= factor;
        b *= factor;
    }

    for light in lights {
        if light.intensity <= 0.0 {
            continue;
        }
        let atten = attenuation(light, x, y);
        if atten <= 0.0 {
            continue;
        }
        let cast = config.shadows_enabled && light.casts_shadows && !occluders.is_empty();
        let soft = if light.shadow_softness >= 0.0 {
            light.shadow_softness
        } else {
            config.soft_shadows
        };
        let vis = if cast {
            visibility(light, x, y, occluders, soft, if soft > 0.5 { 8 } else { 1 })
        } else {
            1.0
        };
        if vis <= 0.0 {
            continue;
        }
        let s = atten * vis * light.intensity;
        r += light.color.r as f32 / 255.0 * s;
        g += light.color.g as f32 / 255.0 * s;
        b += light.color.b as f32 / 255.0 * s;
    }

    let exposure = if config.exposure > 0.0 { config.exposure } else { 1.0 };
    (r * exposure, g * exposure, b * exposure)
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
    fn zero_ambient_with_no_lights_goes_black() {
        let mut pixels = solid_gray(8, 8, 255);
        let mut config = LightConfig::default();
        config.enabled = true;
        config.ambient_intensity = 0.0;
        apply_lighting(&mut pixels, 8, 8, &config, &[], &[]);
        assert!(pixels.chunks_exact(4).all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0));
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
        assert!(shadowed < lit, "shadow {shadowed} should be darker than lit {lit}");
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
        assert!(sample(70, 16) < sample(70, 2), "circle occluder should shadow");
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
            Light { x: 12.0, y: 12.0, radius: 20.0, ..Light::default() },
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
                assert!((map.data[idx + 1] - gg).abs() < 1e-3, "g mismatch at {tx},{ty}");
                assert!((map.data[idx + 2] - bb).abs() < 1e-3, "b mismatch at {tx},{ty}");
            }
        }
    }

    #[test]
    fn segment_box_intersection_basic_cases() {
        assert!(segment_hits_box((-5.0, 0.0), (5.0, 0.0), 1.0, 1.0));
        assert!(!segment_hits_box((-5.0, 5.0), (5.0, 5.0), 1.0, 1.0));
        assert!(segment_hits_box((0.0, 0.0), (10.0, 0.0), 1.0, 1.0));
    }
}
