//! Backend-neutral post-processing for RGBA8 framebuffers.
//!
//! The CPU implementation is useful as a software-renderer path and as a
//! deterministic reference for GPU implementations. Runtime buffers grow only
//! when necessary, are reused between frames, and are excluded from serialized
//! project settings.

use serde::{Deserialize, Serialize};
use std::fmt;

/// The default safety ceiling is large enough for an 8K framebuffer.
pub const DEFAULT_MAX_POST_PROCESS_PIXELS: usize = 33_554_432;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "settings", rename_all = "snake_case")]
pub enum Effect {
    Bloom(BloomConfig),
    Pixelate(PixelateConfig),
    ChromaticAberration(ChromaticAberrationConfig),
    MotionBlur(MotionBlurConfig),
    Quantization(QuantizationConfig),
    Vignette(VignetteConfig),
    Grayscale(GrayscaleConfig),
    Invert(InvertConfig),
    BrightnessContrastSaturation(BrightnessContrastSaturationConfig),
    ExposureTonemap(ExposureTonemapConfig),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BloomConfig {
    /// Pixels below this luminance do not contribute to the bloom buffer.
    pub threshold: f32,
    /// Strength of the blurred light added to the original image.
    pub intensity: f32,
    /// Box-blur radius in pixels. Runtime work is capped at 64 pixels.
    pub radius: u32,
}

impl Default for BloomConfig {
    fn default() -> Self {
        Self {
            threshold: 0.75,
            intensity: 0.8,
            radius: 6,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PixelateConfig {
    /// Width and height of each square pixel block.
    pub block_size: u32,
}

impl Default for PixelateConfig {
    fn default() -> Self {
        Self { block_size: 4 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChromaticAberrationConfig {
    /// Red and blue channel displacement in pixels.
    pub offset_pixels: f32,
    /// Direction of the displacement in degrees.
    pub angle_degrees: f32,
}

impl Default for ChromaticAberrationConfig {
    fn default() -> Self {
        Self {
            offset_pixels: 2.0,
            angle_degrees: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MotionBlurConfig {
    /// Contribution of the preceding frame, from 0 (current only) to 1.
    pub strength: f32,
}

impl Default for MotionBlurConfig {
    fn default() -> Self {
        Self { strength: 0.5 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuantizationConfig {
    /// Number of output values per RGB channel. Values below two use two.
    pub levels: u8,
    /// Ordered-dither amplitude in output-level steps. Zero disables dithering.
    pub dither_strength: f32,
}

impl Default for QuantizationConfig {
    fn default() -> Self {
        Self {
            levels: 8,
            dither_strength: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VignetteConfig {
    pub strength: f32,
    /// Normalized radius at which darkening begins.
    pub radius: f32,
    /// Width of the transition toward the corners.
    pub softness: f32,
}

impl Default for VignetteConfig {
    fn default() -> Self {
        Self {
            strength: 0.5,
            radius: 0.35,
            softness: 0.55,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GrayscaleConfig {
    pub amount: f32,
}

impl Default for GrayscaleConfig {
    fn default() -> Self {
        Self { amount: 1.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InvertConfig {
    pub amount: f32,
}

impl Default for InvertConfig {
    fn default() -> Self {
        Self { amount: 1.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrightnessContrastSaturationConfig {
    /// Additive brightness in normalized color space. Zero is unchanged.
    pub brightness: f32,
    /// Contrast adjustment where zero is unchanged and one doubles contrast.
    pub contrast: f32,
    /// Saturation adjustment where zero is unchanged and -1 is grayscale.
    pub saturation: f32,
}

impl Default for BrightnessContrastSaturationConfig {
    fn default() -> Self {
        Self {
            brightness: 0.0,
            contrast: 0.0,
            saturation: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TonemapOperator {
    #[default]
    None,
    Reinhard,
    Aces,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExposureTonemapConfig {
    /// Exposure in photographic stops.
    pub exposure: f32,
    pub operator: TonemapOperator,
    /// Gamma used to decode and encode the RGBA8 input. Must be positive.
    pub gamma: f32,
}

impl Default for ExposureTonemapConfig {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            operator: TonemapOperator::Reinhard,
            gamma: 2.2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EffectPass {
    pub enabled: bool,
    pub effect: Effect,
}

impl EffectPass {
    pub fn new(effect: Effect) -> Self {
        Self {
            enabled: true,
            effect,
        }
    }
}

impl From<Effect> for EffectPass {
    fn from(effect: Effect) -> Self {
        Self::new(effect)
    }
}

impl Default for EffectPass {
    fn default() -> Self {
        Self::new(Effect::Grayscale(GrayscaleConfig::default()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostProcessError {
    DimensionOverflow { width: usize, height: usize },
    LengthMismatch { expected: usize, actual: usize },
    PixelLimitExceeded { pixels: usize, limit: usize },
}

impl fmt::Display for PostProcessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DimensionOverflow { width, height } => {
                write!(formatter, "RGBA8 dimensions {width}x{height} overflow")
            }
            Self::LengthMismatch { expected, actual } => write!(
                formatter,
                "RGBA8 framebuffer has {actual} bytes, expected {expected}"
            ),
            Self::PixelLimitExceeded { pixels, limit } => write!(
                formatter,
                "post-process framebuffer has {pixels} pixels, exceeding limit {limit}"
            ),
        }
    }
}

impl std::error::Error for PostProcessError {}

/// An ordered, serializable post-process pipeline.
///
/// `scratch_a`, `scratch_b`, and `history` retain capacity across frames, but
/// never grow beyond `max_pixels * 4` through [`Self::apply_in_place`].
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PostProcessStack {
    pub enabled: bool,
    /// Effects are evaluated from first to last.
    pub effects: Vec<EffectPass>,
    pub max_pixels: usize,
    #[serde(skip)]
    scratch_a: Vec<u8>,
    #[serde(skip)]
    scratch_b: Vec<u8>,
    #[serde(skip)]
    history: Vec<u8>,
    #[serde(skip)]
    history_dimensions: Option<(usize, usize)>,
}

impl Default for PostProcessStack {
    fn default() -> Self {
        Self {
            enabled: true,
            effects: Vec::new(),
            max_pixels: DEFAULT_MAX_POST_PROCESS_PIXELS,
            scratch_a: Vec::new(),
            scratch_b: Vec::new(),
            history: Vec::new(),
            history_dimensions: None,
        }
    }
}

impl PostProcessStack {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_effects(effects: impl IntoIterator<Item = Effect>) -> Self {
        Self {
            effects: effects.into_iter().map(EffectPass::new).collect(),
            ..Self::default()
        }
    }

    pub fn push(&mut self, effect: Effect) -> usize {
        let index = self.effects.len();
        self.effects.push(EffectPass::new(effect));
        index
    }

    pub fn insert(&mut self, index: usize, effect: Effect) -> bool {
        if index > self.effects.len() {
            return false;
        }
        self.effects.insert(index, EffectPass::new(effect));
        true
    }

    pub fn move_effect(&mut self, from: usize, to: usize) -> bool {
        if from >= self.effects.len() || to >= self.effects.len() || from == to {
            return from == to && from < self.effects.len();
        }
        let effect = self.effects.remove(from);
        self.effects.insert(to, effect);
        true
    }

    pub fn remove(&mut self, index: usize) -> Option<EffectPass> {
        (index < self.effects.len()).then(|| self.effects.remove(index))
    }

    /// Drops temporal state without changing configured effects.
    pub fn clear_history(&mut self) {
        self.history.clear();
        self.history_dimensions = None;
    }

    /// Frees all reusable runtime allocations without changing configuration.
    pub fn clear_runtime_buffers(&mut self) {
        self.scratch_a = Vec::new();
        self.scratch_b = Vec::new();
        self.clear_history();
    }

    /// Applies all enabled passes to a tightly packed RGBA8 framebuffer.
    ///
    /// Alpha is preserved by every built-in effect. Motion blur remembers the
    /// input at its position in the pipeline and starts with an identity frame.
    pub fn apply_in_place(
        &mut self,
        width: usize,
        height: usize,
        pixels: &mut [u8],
    ) -> Result<(), PostProcessError> {
        let pixel_count = width
            .checked_mul(height)
            .ok_or(PostProcessError::DimensionOverflow { width, height })?;
        let expected = pixel_count
            .checked_mul(4)
            .ok_or(PostProcessError::DimensionOverflow { width, height })?;
        if expected != pixels.len() {
            return Err(PostProcessError::LengthMismatch {
                expected,
                actual: pixels.len(),
            });
        }
        if pixel_count > self.max_pixels {
            return Err(PostProcessError::PixelLimitExceeded {
                pixels: pixel_count,
                limit: self.max_pixels,
            });
        }
        if self
            .history_dimensions
            .is_some_and(|dimensions| dimensions != (width, height))
        {
            self.clear_history();
        }
        if pixel_count == 0 || !self.enabled {
            return Ok(());
        }

        // Effect and config types are Copy, so walking the ordered list does
        // not allocate and does not hold a borrow across runtime-buffer writes.
        for index in 0..self.effects.len() {
            let pass = self.effects[index];
            if !pass.enabled {
                continue;
            }
            match pass.effect {
                Effect::Bloom(config) => apply_bloom(
                    width,
                    height,
                    pixels,
                    &mut self.scratch_a,
                    &mut self.scratch_b,
                    config,
                ),
                Effect::Pixelate(config) => {
                    apply_pixelate(width, height, pixels, &mut self.scratch_a, config)
                }
                Effect::ChromaticAberration(config) => {
                    apply_chromatic_aberration(width, height, pixels, &mut self.scratch_a, config)
                }
                Effect::MotionBlur(config) => {
                    apply_motion_blur(pixels, &mut self.scratch_a, &mut self.history, config);
                    self.history_dimensions = Some((width, height));
                }
                Effect::Quantization(config) => {
                    apply_quantization(width, pixels, config);
                }
                Effect::Vignette(config) => apply_vignette(width, height, pixels, config),
                Effect::Grayscale(config) => apply_grayscale(pixels, config),
                Effect::Invert(config) => apply_invert(pixels, config),
                Effect::BrightnessContrastSaturation(config) => {
                    apply_brightness_contrast_saturation(pixels, config)
                }
                Effect::ExposureTonemap(config) => apply_exposure_tonemap(pixels, config),
            }
        }
        Ok(())
    }
}

fn resize_buffer(buffer: &mut Vec<u8>, len: usize) {
    if buffer.len() != len {
        buffer.resize(len, 0);
    }
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

fn clamp01(value: f32) -> f32 {
    finite_or(value, 0.0).clamp(0.0, 1.0)
}

fn byte_from_unit(value: f32) -> u8 {
    (clamp01(value) * 255.0).round() as u8
}

fn mix_byte(from: u8, to: u8, amount: f32) -> u8 {
    let amount = clamp01(amount);
    (from as f32 + (to as f32 - from as f32) * amount).round() as u8
}

fn luma(red: f32, green: f32, blue: f32) -> f32 {
    red * 0.2126 + green * 0.7152 + blue * 0.0722
}

fn apply_bloom(
    width: usize,
    height: usize,
    pixels: &mut [u8],
    thresholded: &mut Vec<u8>,
    blurred: &mut Vec<u8>,
    config: BloomConfig,
) {
    let radius = config.radius.min(64) as usize;
    let intensity = finite_or(config.intensity, 0.0).max(0.0);
    if radius == 0 || intensity == 0.0 {
        return;
    }
    let threshold = clamp01(config.threshold);
    resize_buffer(thresholded, pixels.len());
    resize_buffer(blurred, pixels.len());

    let threshold_span = (1.0 - threshold).max(1.0 / 255.0);
    for (source, destination) in pixels.chunks_exact(4).zip(thresholded.chunks_exact_mut(4)) {
        let luminance = luma(
            source[0] as f32 / 255.0,
            source[1] as f32 / 255.0,
            source[2] as f32 / 255.0,
        );
        let contribution = ((luminance - threshold) / threshold_span).clamp(0.0, 1.0);
        destination[0] = (source[0] as f32 * contribution).round() as u8;
        destination[1] = (source[1] as f32 * contribution).round() as u8;
        destination[2] = (source[2] as f32 * contribution).round() as u8;
        destination[3] = source[3];
    }

    // A separable sliding box blur keeps cost linear in framebuffer size and
    // independent of the configured radius.
    for y in 0..height {
        for channel in 0..3 {
            let mut sum = 0u32;
            let initial_end = radius.min(width - 1);
            for x in 0..=initial_end {
                sum += thresholded[(y * width + x) * 4 + channel] as u32;
            }
            for x in 0..width {
                let left = x.saturating_sub(radius);
                let right = x.saturating_add(radius).min(width - 1);
                let count = right - left + 1;
                blurred[(y * width + x) * 4 + channel] = (sum / count as u32) as u8;
                if x + 1 < width {
                    if x >= radius {
                        sum -= thresholded[(y * width + x - radius) * 4 + channel] as u32;
                    }
                    if let Some(add_x) = x.checked_add(radius + 1).filter(|&value| value < width) {
                        sum += thresholded[(y * width + add_x) * 4 + channel] as u32;
                    }
                }
            }
        }
    }

    for x in 0..width {
        for channel in 0..3 {
            let mut sum = 0u32;
            let initial_end = radius.min(height - 1);
            for y in 0..=initial_end {
                sum += blurred[(y * width + x) * 4 + channel] as u32;
            }
            for y in 0..height {
                let top = y.saturating_sub(radius);
                let bottom = y.saturating_add(radius).min(height - 1);
                let count = bottom - top + 1;
                let bloom = sum as f32 / count as f32 * intensity;
                let index = (y * width + x) * 4 + channel;
                pixels[index] = (pixels[index] as f32 + bloom).clamp(0.0, 255.0).round() as u8;
                if y + 1 < height {
                    if y >= radius {
                        sum -= blurred[((y - radius) * width + x) * 4 + channel] as u32;
                    }
                    if let Some(add_y) = y.checked_add(radius + 1).filter(|&value| value < height) {
                        sum += blurred[(add_y * width + x) * 4 + channel] as u32;
                    }
                }
            }
        }
    }
}

fn apply_pixelate(
    width: usize,
    height: usize,
    pixels: &mut [u8],
    source: &mut Vec<u8>,
    config: PixelateConfig,
) {
    let block_size = (config.block_size.max(1) as usize).min(width.max(height));
    if block_size <= 1 {
        return;
    }
    resize_buffer(source, pixels.len());
    source.copy_from_slice(pixels);
    for block_y in (0..height).step_by(block_size) {
        for block_x in (0..width).step_by(block_size) {
            let sample = (block_y * width + block_x) * 4;
            let color = [
                source[sample],
                source[sample + 1],
                source[sample + 2],
                source[sample + 3],
            ];
            let end_y = block_y.saturating_add(block_size).min(height);
            let end_x = block_x.saturating_add(block_size).min(width);
            for y in block_y..end_y {
                for x in block_x..end_x {
                    pixels[(y * width + x) * 4..(y * width + x) * 4 + 4].copy_from_slice(&color);
                }
            }
        }
    }
}

fn apply_chromatic_aberration(
    width: usize,
    height: usize,
    pixels: &mut [u8],
    source: &mut Vec<u8>,
    config: ChromaticAberrationConfig,
) {
    let distance = finite_or(config.offset_pixels, 0.0).clamp(-4096.0, 4096.0);
    if distance.abs() < 0.5 {
        return;
    }
    let angle = finite_or(config.angle_degrees, 0.0).to_radians();
    let dx = (distance * angle.cos()).round() as isize;
    let dy = (distance * angle.sin()).round() as isize;
    if dx == 0 && dy == 0 {
        return;
    }
    resize_buffer(source, pixels.len());
    source.copy_from_slice(pixels);
    let max_x = width.saturating_sub(1) as isize;
    let max_y = height.saturating_sub(1) as isize;
    for y in 0..height {
        for x in 0..width {
            let red_x = (x as isize + dx).clamp(0, max_x) as usize;
            let red_y = (y as isize + dy).clamp(0, max_y) as usize;
            let blue_x = (x as isize - dx).clamp(0, max_x) as usize;
            let blue_y = (y as isize - dy).clamp(0, max_y) as usize;
            let destination = (y * width + x) * 4;
            pixels[destination] = source[(red_y * width + red_x) * 4];
            pixels[destination + 2] = source[(blue_y * width + blue_x) * 4 + 2];
        }
    }
}

fn apply_motion_blur(
    pixels: &mut [u8],
    current: &mut Vec<u8>,
    history: &mut Vec<u8>,
    config: MotionBlurConfig,
) {
    let strength = clamp01(config.strength);
    resize_buffer(current, pixels.len());
    current.copy_from_slice(pixels);
    if history.len() == pixels.len() {
        for index in (0..pixels.len()).step_by(4) {
            pixels[index] = mix_byte(current[index], history[index], strength);
            pixels[index + 1] = mix_byte(current[index + 1], history[index + 1], strength);
            pixels[index + 2] = mix_byte(current[index + 2], history[index + 2], strength);
        }
    }
    resize_buffer(history, pixels.len());
    history.copy_from_slice(current);
}

fn apply_quantization(width: usize, pixels: &mut [u8], config: QuantizationConfig) {
    const BAYER_4X4: [f32; 16] = [
        0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0,
    ];
    let levels = config.levels.max(2) as f32;
    let steps = levels - 1.0;
    let dither_strength = finite_or(config.dither_strength, 0.0).clamp(0.0, 1.0);
    for (pixel_index, pixel) in pixels.chunks_exact_mut(4).enumerate() {
        let x = pixel_index % width;
        let y = pixel_index / width;
        let dither =
            ((BAYER_4X4[(y % 4) * 4 + x % 4] + 0.5) / 16.0 - 0.5) * dither_strength / steps;
        for channel in &mut pixel[..3] {
            let normalized = (*channel as f32 / 255.0 + dither).clamp(0.0, 1.0);
            *channel = byte_from_unit((normalized * steps).round() / steps);
        }
    }
}

fn apply_vignette(width: usize, height: usize, pixels: &mut [u8], config: VignetteConfig) {
    let strength = clamp01(config.strength);
    if strength == 0.0 {
        return;
    }
    let radius = clamp01(config.radius);
    let softness = finite_or(config.softness, 0.0).clamp(0.0001, 1.0);
    let width_denominator = width.saturating_sub(1).max(1) as f32;
    let height_denominator = height.saturating_sub(1).max(1) as f32;
    for y in 0..height {
        let normalized_y = y as f32 / height_denominator * 2.0 - 1.0;
        for x in 0..width {
            let normalized_x = x as f32 / width_denominator * 2.0 - 1.0;
            let distance = (normalized_x * normalized_x + normalized_y * normalized_y).sqrt()
                / std::f32::consts::SQRT_2;
            let transition = ((distance - radius) / softness).clamp(0.0, 1.0);
            let smooth = transition * transition * (3.0 - 2.0 * transition);
            let multiplier = 1.0 - smooth * strength;
            let index = (y * width + x) * 4;
            pixels[index] = (pixels[index] as f32 * multiplier).round() as u8;
            pixels[index + 1] = (pixels[index + 1] as f32 * multiplier).round() as u8;
            pixels[index + 2] = (pixels[index + 2] as f32 * multiplier).round() as u8;
        }
    }
}

fn apply_grayscale(pixels: &mut [u8], config: GrayscaleConfig) {
    let amount = clamp01(config.amount);
    if amount == 0.0 {
        return;
    }
    for pixel in pixels.chunks_exact_mut(4) {
        let gray = luma(pixel[0] as f32, pixel[1] as f32, pixel[2] as f32).round() as u8;
        pixel[0] = mix_byte(pixel[0], gray, amount);
        pixel[1] = mix_byte(pixel[1], gray, amount);
        pixel[2] = mix_byte(pixel[2], gray, amount);
    }
}

fn apply_invert(pixels: &mut [u8], config: InvertConfig) {
    let amount = clamp01(config.amount);
    if amount == 0.0 {
        return;
    }
    for pixel in pixels.chunks_exact_mut(4) {
        pixel[0] = mix_byte(pixel[0], 255 - pixel[0], amount);
        pixel[1] = mix_byte(pixel[1], 255 - pixel[1], amount);
        pixel[2] = mix_byte(pixel[2], 255 - pixel[2], amount);
    }
}

fn apply_brightness_contrast_saturation(
    pixels: &mut [u8],
    config: BrightnessContrastSaturationConfig,
) {
    let brightness = finite_or(config.brightness, 0.0).clamp(-1.0, 1.0);
    let contrast = finite_or(config.contrast, 0.0).clamp(-1.0, 4.0) + 1.0;
    let saturation = finite_or(config.saturation, 0.0).clamp(-1.0, 4.0) + 1.0;
    if brightness == 0.0 && contrast == 1.0 && saturation == 1.0 {
        return;
    }
    for pixel in pixels.chunks_exact_mut(4) {
        let mut red = (pixel[0] as f32 / 255.0 - 0.5) * contrast + 0.5 + brightness;
        let mut green = (pixel[1] as f32 / 255.0 - 0.5) * contrast + 0.5 + brightness;
        let mut blue = (pixel[2] as f32 / 255.0 - 0.5) * contrast + 0.5 + brightness;
        let gray = luma(red, green, blue);
        red = gray + (red - gray) * saturation;
        green = gray + (green - gray) * saturation;
        blue = gray + (blue - gray) * saturation;
        pixel[0] = byte_from_unit(red);
        pixel[1] = byte_from_unit(green);
        pixel[2] = byte_from_unit(blue);
    }
}

fn apply_exposure_tonemap(pixels: &mut [u8], config: ExposureTonemapConfig) {
    let exposure = finite_or(config.exposure, 0.0).clamp(-24.0, 24.0).exp2();
    let gamma = finite_or(config.gamma, 2.2).clamp(0.1, 8.0);
    for pixel in pixels.chunks_exact_mut(4) {
        for channel in &mut pixel[..3] {
            let linear = (*channel as f32 / 255.0).powf(gamma) * exposure;
            let mapped = match config.operator {
                TonemapOperator::None => linear,
                TonemapOperator::Reinhard => linear / (1.0 + linear),
                TonemapOperator::Aces => ((linear * (2.51 * linear + 0.03))
                    / (linear * (2.43 * linear + 0.59) + 0.14))
                    .clamp(0.0, 1.0),
            };
            *channel = byte_from_unit(mapped.max(0.0).powf(1.0 / gamma));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(effect: Effect, width: usize, height: usize, pixels: &mut [u8]) {
        PostProcessStack::with_effects([effect])
            .apply_in_place(width, height, pixels)
            .expect("post-process effect should apply");
    }

    #[test]
    fn empty_or_disabled_stack_is_identity() {
        let original = vec![10, 20, 30, 40, 100, 110, 120, 130];
        let mut pixels = original.clone();
        PostProcessStack::new()
            .apply_in_place(2, 1, &mut pixels)
            .expect("empty stack should apply");
        assert_eq!(pixels, original);

        let mut stack = PostProcessStack::with_effects([Effect::Invert(InvertConfig::default())]);
        stack.enabled = false;
        stack
            .apply_in_place(2, 1, &mut pixels)
            .expect("disabled stack should apply");
        assert_eq!(pixels, original);
    }

    #[test]
    fn dimensions_and_pixel_limit_are_validated() {
        let mut stack = PostProcessStack::new();
        assert!(matches!(
            stack.apply_in_place(2, 2, &mut [0; 8]),
            Err(PostProcessError::LengthMismatch { .. })
        ));
        assert!(matches!(
            stack.apply_in_place(usize::MAX, 2, &mut []),
            Err(PostProcessError::DimensionOverflow { .. })
        ));
        stack.max_pixels = 1;
        assert!(matches!(
            stack.apply_in_place(2, 1, &mut [0; 8]),
            Err(PostProcessError::PixelLimitExceeded { .. })
        ));
    }

    #[test]
    fn bloom_spreads_bright_pixels_without_changing_alpha() {
        let mut pixels = vec![0, 0, 0, 17, 255, 255, 255, 23, 0, 0, 0, 31];
        apply(
            Effect::Bloom(BloomConfig {
                threshold: 0.5,
                intensity: 1.0,
                radius: 1,
            }),
            3,
            1,
            &mut pixels,
        );
        assert!(pixels[0] > 0 && pixels[8] > 0);
        assert_eq!([pixels[3], pixels[7], pixels[11]], [17, 23, 31]);
    }

    #[test]
    fn pixelate_repeats_the_first_color_in_each_block() {
        let mut pixels = vec![10, 20, 30, 40, 200, 210, 220, 230];
        apply(
            Effect::Pixelate(PixelateConfig { block_size: 2 }),
            2,
            1,
            &mut pixels,
        );
        assert_eq!(pixels, vec![10, 20, 30, 40, 10, 20, 30, 40]);
    }

    #[test]
    fn chromatic_aberration_offsets_red_and_blue_in_opposite_directions() {
        let mut pixels = vec![0, 0, 200, 255, 0, 50, 0, 255, 100, 0, 0, 255];
        apply(
            Effect::ChromaticAberration(ChromaticAberrationConfig {
                offset_pixels: 1.0,
                angle_degrees: 0.0,
            }),
            3,
            1,
            &mut pixels,
        );
        assert_eq!(&pixels[4..8], &[100, 50, 200, 255]);
    }

    #[test]
    fn motion_blur_uses_previous_frame_and_resets_on_resize() {
        let mut stack = PostProcessStack::with_effects([Effect::MotionBlur(MotionBlurConfig {
            strength: 0.5,
        })]);
        let mut first = vec![0, 0, 0, 33];
        stack
            .apply_in_place(1, 1, &mut first)
            .expect("first temporal frame should apply");
        assert_eq!(first, vec![0, 0, 0, 33]);

        let mut second = vec![255, 255, 255, 44];
        stack
            .apply_in_place(1, 1, &mut second)
            .expect("second temporal frame should apply");
        assert_eq!(second, vec![128, 128, 128, 44]);

        let mut resized = vec![200, 100, 50, 10, 20, 30, 40, 50];
        let expected = resized.clone();
        stack
            .apply_in_place(2, 1, &mut resized)
            .expect("resized temporal frame should apply");
        assert_eq!(resized, expected);
    }

    #[test]
    fn quantization_reduces_channel_levels() {
        let mut pixels = vec![64, 190, 255, 99];
        apply(
            Effect::Quantization(QuantizationConfig {
                levels: 2,
                dither_strength: 0.0,
            }),
            1,
            1,
            &mut pixels,
        );
        assert_eq!(pixels, vec![0, 255, 255, 99]);
    }

    #[test]
    fn vignette_darkens_corners_more_than_the_center() {
        let mut pixels = vec![255; 5 * 5 * 4];
        apply(
            Effect::Vignette(VignetteConfig {
                strength: 1.0,
                radius: 0.0,
                softness: 1.0,
            }),
            5,
            5,
            &mut pixels,
        );
        let corner = pixels[0];
        let center = pixels[(2 * 5 + 2) * 4];
        assert!(center > corner);
        assert!(pixels.chunks_exact(4).all(|pixel| pixel[3] == 255));
    }

    #[test]
    fn grayscale_uses_luminance_and_preserves_alpha() {
        let mut pixels = vec![255, 0, 0, 71];
        apply(
            Effect::Grayscale(GrayscaleConfig::default()),
            1,
            1,
            &mut pixels,
        );
        assert_eq!(pixels[0], pixels[1]);
        assert_eq!(pixels[1], pixels[2]);
        assert_eq!(pixels[3], 71);
    }

    #[test]
    fn invert_supports_partial_blending() {
        let mut pixels = vec![0, 100, 255, 19];
        apply(
            Effect::Invert(InvertConfig { amount: 1.0 }),
            1,
            1,
            &mut pixels,
        );
        assert_eq!(pixels, vec![255, 155, 0, 19]);
    }

    #[test]
    fn brightness_contrast_saturation_adjusts_rgb_only() {
        let mut pixels = vec![0, 0, 0, 12];
        apply(
            Effect::BrightnessContrastSaturation(BrightnessContrastSaturationConfig {
                brightness: 0.25,
                ..BrightnessContrastSaturationConfig::default()
            }),
            1,
            1,
            &mut pixels,
        );
        assert_eq!(pixels, vec![64, 64, 64, 12]);
    }

    #[test]
    fn exposure_and_tonemap_compress_highlights() {
        let mut pixels = vec![255, 255, 255, 101];
        apply(
            Effect::ExposureTonemap(ExposureTonemapConfig {
                exposure: 1.0,
                operator: TonemapOperator::Reinhard,
                gamma: 2.2,
            }),
            1,
            1,
            &mut pixels,
        );
        assert!(pixels[0] > 128 && pixels[0] < 255);
        assert_eq!(pixels[3], 101);
    }

    #[test]
    fn pass_order_is_observable_and_disabled_passes_are_skipped() {
        let quantize = Effect::Quantization(QuantizationConfig {
            levels: 2,
            dither_strength: 0.0,
        });
        let brighten = Effect::BrightnessContrastSaturation(BrightnessContrastSaturationConfig {
            brightness: 0.25,
            ..BrightnessContrastSaturationConfig::default()
        });
        let mut quantize_then_brighten = PostProcessStack::with_effects([quantize, brighten]);
        let mut brighten_then_quantize = PostProcessStack::with_effects([brighten, quantize]);
        let mut first = vec![64, 64, 64, 255];
        let mut second = first.clone();
        quantize_then_brighten
            .apply_in_place(1, 1, &mut first)
            .expect("ordered stack should apply");
        brighten_then_quantize
            .apply_in_place(1, 1, &mut second)
            .expect("reversed stack should apply");
        assert_ne!(first, second);

        quantize_then_brighten.effects[0].enabled = false;
        let mut disabled = vec![64, 64, 64, 255];
        quantize_then_brighten
            .apply_in_place(1, 1, &mut disabled)
            .expect("stack with disabled pass should apply");
        assert_eq!(disabled, vec![128, 128, 128, 255]);
    }

    #[test]
    fn scratch_capacity_is_reused_across_frames() {
        let mut stack = PostProcessStack::with_effects([Effect::Bloom(BloomConfig {
            threshold: 0.1,
            intensity: 1.0,
            radius: 2,
        })]);
        let mut large = vec![128; 8 * 8 * 4];
        stack
            .apply_in_place(8, 8, &mut large)
            .expect("large frame should apply");
        let capacities = (stack.scratch_a.capacity(), stack.scratch_b.capacity());
        stack
            .apply_in_place(8, 8, &mut large)
            .expect("same-size frame should reuse storage");
        assert_eq!(
            capacities,
            (stack.scratch_a.capacity(), stack.scratch_b.capacity())
        );

        let mut small = vec![128; 2 * 2 * 4];
        stack
            .apply_in_place(2, 2, &mut small)
            .expect("smaller frame should apply");
        assert_eq!(
            capacities,
            (stack.scratch_a.capacity(), stack.scratch_b.capacity())
        );
    }
}
