#![allow(dead_code)]

use crate::assets::ImageHandle;
use crate::platform::{Color, SharedPlatformState};
use fontdue::Font;
use image::{ImageBuffer, Rgba, RgbaImage};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Vec2 {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextureFilter {
    Nearest,
    Linear,
}

const DEFAULT_FONT_CACHE_KEY: &str = "__neolove_default_font__";
const DEFAULT_FONT_BYTES: &[u8] =
    include_bytes!("../samples/new_features_test/assets/fonts/ProggyClean.ttf");

#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub(crate) enum FontHandle {
    #[default]
    Default,
    Path(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum TextScaleMode {
    None,
    Fit,
    FitWidth,
    FitHeight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum TextAlignX {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum TextAlignY {
    Top,
    Center,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum TextWrapMode {
    None,
    Word,
    Char,
}

#[derive(Clone, Debug)]
pub(crate) struct TextRenderRequest {
    pub text: String,
    pub bounds: Rect,
    pub rotation: f32,
    pub pivot: Vec2,
    pub color: Color,
    pub font: FontHandle,
    pub scale: f32,
    pub min_scale: f32,
    pub text_scale: TextScaleMode,
    pub align_x: TextAlignX,
    pub align_y: TextAlignY,
    pub wrap: TextWrapMode,
    pub padding_x: f32,
    pub padding_y: f32,
    pub line_spacing: f32,
    pub letter_spacing: f32,
    pub stretch_width: f32,
    pub stretch_height: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TextMetrics {
    pub width: f32,
    pub height: f32,
    pub used_scale: f32,
    pub line_count: usize,
}

#[derive(Clone, Debug)]
pub(crate) enum DrawCommand {
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        rotation: f32,
        offset: Vec2,
        color: Color,
    },
    Triangle {
        a: Vec2,
        b: Vec2,
        c: Vec2,
        color: Color,
    },
    Circle {
        center: Vec2,
        radius: f32,
        color: Color,
    },
    Image {
        image: ImageHandle,
        dest: Rect,
        source: Option<Rect>,
        rotation: f32,
        pivot: Vec2,
        tint: Color,
        filter: TextureFilter,
    },
    Text(TextRenderRequest),
}

#[derive(Default)]
pub(crate) struct RenderState {
    commands: Vec<DrawCommand>,
    overlay_commands: Vec<DrawCommand>,
}

pub(crate) type SharedRenderState = Arc<Mutex<RenderState>>;

pub(crate) fn new_shared_render_state() -> SharedRenderState {
    Arc::new(Mutex::new(RenderState::default()))
}

#[derive(Clone)]
pub(crate) struct RasterizedTextSprite {
    pub image: RgbaImage,
    pub dest: Rect,
    pub pivot: Vec2,
    pub rotation: f32,
    pub filter: TextureFilter,
}

impl RenderState {
    pub(crate) fn queue(&mut self, command: DrawCommand) {
        self.commands.push(command);
    }

    pub(crate) fn extend_overlay(&mut self, commands: Vec<DrawCommand>) {
        self.overlay_commands.extend(commands);
    }

    pub(crate) fn drain(&mut self) -> Vec<DrawCommand> {
        let mut out = self.commands.drain(..).collect::<Vec<_>>();
        out.extend(self.overlay_commands.drain(..));
        out
    }
}

fn font_cache() -> &'static Mutex<HashMap<String, Arc<Font>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Font>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn font_warning_cache() -> &'static Mutex<HashSet<String>> {
    static CACHE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashSet::new()))
}

fn warn_font_once(key: &str, message: impl FnOnce() -> String) {
    if let Ok(mut warned) = font_warning_cache().lock() {
        if warned.insert(key.to_string()) {
            eprintln!("{}", message());
        }
    }
}

fn load_font(source: &FontHandle) -> Option<Arc<Font>> {
    let cache_key = match source {
        FontHandle::Default => DEFAULT_FONT_CACHE_KEY.to_string(),
        FontHandle::Path(path) if !path.trim().is_empty() => path.clone(),
        FontHandle::Path(_) => DEFAULT_FONT_CACHE_KEY.to_string(),
    };

    if let Ok(cache) = font_cache().lock() {
        if let Some(font) = cache.get(&cache_key) {
            return Some(font.clone());
        }
    }

    let font = match source {
        FontHandle::Default => {
            Arc::new(Font::from_bytes(DEFAULT_FONT_BYTES, fontdue::FontSettings::default()).ok()?)
        }
        FontHandle::Path(path) if !path.trim().is_empty() => {
            let bytes = match std::fs::read(path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    warn_font_once(&cache_key, || {
                        format!(
                            "font warning: failed to read '{}': {}. Falling back to the built-in default font.",
                            path, error
                        )
                    });
                    DEFAULT_FONT_BYTES.to_vec()
                }
            };

            match Font::from_bytes(bytes, fontdue::FontSettings::default()) {
                Ok(font) => Arc::new(font),
                Err(error) => {
                    warn_font_once(&cache_key, || {
                        format!(
                            "font warning: failed to parse '{}': {}. Falling back to the built-in default font.",
                            path, error
                        )
                    });
                    Arc::new(
                        Font::from_bytes(DEFAULT_FONT_BYTES, fontdue::FontSettings::default())
                            .ok()?,
                    )
                }
            }
        }
        FontHandle::Path(_) => {
            Arc::new(Font::from_bytes(DEFAULT_FONT_BYTES, fontdue::FontSettings::default()).ok()?)
        }
    };
    if let Ok(mut cache) = font_cache().lock() {
        cache.insert(cache_key, font.clone());
    }
    Some(font)
}

pub(crate) fn drain_commands(render_state: &SharedRenderState) -> Result<Vec<DrawCommand>, String> {
    render_state
        .lock()
        .map_err(|_| "render state lock poisoned".to_string())
        .map(|mut state| state.drain())
}

#[derive(Clone, Debug)]
struct PreparedTextLine {
    text: String,
    width: f32,
}

#[derive(Clone, Copy, Debug)]
struct PreparedGlyph {
    ch: char,
    x: f32,
    y: f32,
}

#[derive(Clone, Debug)]
struct PreparedTextLayout {
    glyphs: Vec<PreparedGlyph>,
    metrics: TextMetrics,
    pixel_bounds: Option<(f32, f32, f32, f32)>,
}

fn line_metrics_for(font: &Font, px: f32) -> fontdue::LineMetrics {
    font.horizontal_line_metrics(px)
        .unwrap_or(fontdue::LineMetrics {
            ascent: px,
            descent: 0.0,
            line_gap: 0.0,
            new_line_size: px,
        })
}

fn measure_line_width(font: &Font, text: &str, px: f32, letter_spacing: f32) -> f32 {
    let mut width = 0.0f32;
    let mut previous = None;
    let spacing = letter_spacing;

    for (index, ch) in text.chars().enumerate() {
        if index > 0 {
            width += spacing;
        }
        if let Some(prev) = previous {
            width += font.horizontal_kern(prev, ch, px).unwrap_or(0.0);
        }
        width += font.metrics(ch, px).advance_width;
        previous = Some(ch);
    }

    width.max(0.0)
}

fn wrap_paragraph_char(
    font: &Font,
    text: &str,
    px: f32,
    limit: f32,
    letter_spacing: f32,
) -> Vec<String> {
    if limit <= 0.0 || !limit.is_finite() {
        return vec![text.to_string()];
    }

    let spacing = letter_spacing;
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0.0f32;
    let mut previous = None;

    for ch in text.chars() {
        let kern = previous
            .and_then(|prev| font.horizontal_kern(prev, ch, px))
            .unwrap_or(0.0);
        let char_width = font.metrics(ch, px).advance_width;
        let next_width = if current.is_empty() {
            char_width
        } else {
            current_width + spacing + kern + char_width
        };

        if !current.is_empty() && next_width > limit {
            lines.push(current);
            current = ch.to_string();
            current_width = char_width;
            previous = Some(ch);
            continue;
        }

        if !current.is_empty() {
            current_width += spacing + kern + char_width;
        } else {
            current_width = char_width;
        }
        current.push(ch);
        previous = Some(ch);
    }

    if current.is_empty() {
        lines.push(String::new());
    } else {
        lines.push(current);
    }

    lines
}

fn wrap_paragraph_word(
    font: &Font,
    text: &str,
    px: f32,
    limit: f32,
    letter_spacing: f32,
) -> Vec<String> {
    if limit <= 0.0 || !limit.is_finite() {
        return vec![text.to_string()];
    }

    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();

    for word in words {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        let candidate_width = measure_line_width(font, &candidate, px, letter_spacing);
        if !current.is_empty() && candidate_width > limit {
            lines.push(current);
            let word_width = measure_line_width(font, word, px, letter_spacing);
            if word_width > limit {
                let mut wrapped = wrap_paragraph_char(font, word, px, limit, letter_spacing);
                current = wrapped.pop().unwrap_or_default();
                lines.extend(wrapped);
            } else {
                current = word.to_string();
            }
        } else {
            current = candidate;
        }
    }

    if current.is_empty() {
        lines.push(String::new());
    } else {
        lines.push(current);
    }

    lines
}

fn layout_lines_for(
    font: &Font,
    text: &str,
    px: f32,
    wrap: TextWrapMode,
    width_limit: Option<f32>,
    letter_spacing: f32,
) -> Vec<PreparedTextLine> {
    if text.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let wrapped = match (wrap, width_limit) {
            (TextWrapMode::None, _) | (_, None) => vec![paragraph.to_string()],
            (TextWrapMode::Word, Some(limit)) => {
                wrap_paragraph_word(font, paragraph, px, limit, letter_spacing)
            }
            (TextWrapMode::Char, Some(limit)) => {
                wrap_paragraph_char(font, paragraph, px, limit, letter_spacing)
            }
        };
        for line in wrapped {
            let width = measure_line_width(font, &line, px, letter_spacing);
            lines.push(PreparedTextLine { text: line, width });
        }
    }
    lines
}

fn prepare_text_layout(request: &TextRenderRequest) -> Option<PreparedTextLayout> {
    if request.text.is_empty() {
        return Some(PreparedTextLayout {
            glyphs: Vec::new(),
            metrics: TextMetrics::default(),
            pixel_bounds: None,
        });
    }

    let font = load_font(&request.font)?;
    let preferred_scale = request.scale.max(1.0);
    let minimum_scale = request.min_scale.max(1.0).min(preferred_scale);
    let available_width = if request.bounds.w > 0.0 {
        Some((request.bounds.w - request.padding_x * 2.0).max(0.0))
    } else {
        None
    };
    let available_height = if request.bounds.h > 0.0 {
        Some((request.bounds.h - request.padding_y * 2.0).max(0.0))
    } else {
        None
    };
    let wrap_limit = if matches!(request.wrap, TextWrapMode::None) {
        None
    } else {
        available_width
    };

    let measure_for_scale = |scale: f32| {
        let px = scale.max(1.0);
        let line_metrics = line_metrics_for(&font, px);
        let base_line_height = line_metrics
            .new_line_size
            .max((line_metrics.ascent - line_metrics.descent).abs())
            .max(px);
        let line_advance = (base_line_height * request.line_spacing.max(0.1)).max(1.0);
        let lines = layout_lines_for(
            &font,
            &request.text,
            px,
            request.wrap,
            wrap_limit,
            request.letter_spacing,
        );
        let width = lines.iter().map(|line| line.width).fold(0.0f32, f32::max);
        let height = if lines.is_empty() {
            0.0
        } else {
            base_line_height + line_advance * (lines.len().saturating_sub(1) as f32)
        };
        (
            lines,
            width.max(0.0),
            height.max(0.0),
            line_metrics,
            base_line_height,
            line_advance,
        )
    };

    let fits = |width: f32, height: f32| -> bool {
        match request.text_scale {
            TextScaleMode::None => true,
            TextScaleMode::Fit => {
                available_width.is_none_or(|limit| width <= limit + 0.5)
                    && available_height.is_none_or(|limit| height <= limit + 0.5)
            }
            TextScaleMode::FitWidth => available_width.is_none_or(|limit| width <= limit + 0.5),
            TextScaleMode::FitHeight => available_height.is_none_or(|limit| height <= limit + 0.5),
        }
    };

    let mut measured = measure_for_scale(preferred_scale);
    let mut used_scale = preferred_scale;
    if !matches!(request.text_scale, TextScaleMode::None)
        && (available_width.is_some() || available_height.is_some())
        && !fits(measured.1, measured.2)
    {
        let mut low = minimum_scale;
        let mut high = preferred_scale;
        let mut best_scale = minimum_scale;
        let mut best_measured = measure_for_scale(minimum_scale);
        if fits(best_measured.1, best_measured.2) {
            best_scale = minimum_scale;
            for _ in 0..10 {
                let mid = (low + high) * 0.5;
                let candidate = measure_for_scale(mid);
                if fits(candidate.1, candidate.2) {
                    best_scale = mid;
                    best_measured = candidate;
                    low = mid;
                } else {
                    high = mid;
                }
            }
        }
        used_scale = best_scale;
        measured = best_measured;
    }

    let (lines, block_width, block_height, line_metrics, _base_line_height, line_advance) =
        measured;
    let padded_origin_x = request.bounds.x + request.padding_x.max(0.0);
    let padded_origin_y = request.bounds.y + request.padding_y.max(0.0);
    let content_box_width = available_width.unwrap_or(block_width);
    let content_box_height = available_height.unwrap_or(block_height);
    let start_y = padded_origin_y
        + match request.align_y {
            TextAlignY::Top => 0.0,
            TextAlignY::Center => (content_box_height - block_height) * 0.5,
            TextAlignY::Bottom => content_box_height - block_height,
        }
        .max(0.0);

    let mut glyphs = Vec::new();
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let spacing = request.letter_spacing;
    let px = used_scale.max(1.0);

    for (line_index, line) in lines.iter().enumerate() {
        let line_start_x = padded_origin_x
            + match request.align_x {
                TextAlignX::Left => 0.0,
                TextAlignX::Center => (content_box_width - line.width) * 0.5,
                TextAlignX::Right => content_box_width - line.width,
            }
            .max(0.0);
        let baseline_y = start_y + line_metrics.ascent + line_advance * line_index as f32;
        let mut pen_x = 0.0f32;
        let mut previous = None;

        for (char_index, ch) in line.text.chars().enumerate() {
            if char_index > 0 {
                pen_x += spacing;
            }
            if let Some(prev) = previous {
                pen_x += font.horizontal_kern(prev, ch, px).unwrap_or(0.0);
            }
            let metrics = font.metrics(ch, px);
            let glyph_x = line_start_x + pen_x + metrics.xmin as f32;
            let glyph_y = baseline_y - metrics.height as f32 - metrics.ymin as f32;
            min_x = min_x.min(glyph_x.floor());
            min_y = min_y.min(glyph_y.floor());
            max_x = max_x.max((glyph_x + metrics.width as f32).ceil());
            max_y = max_y.max((glyph_y + metrics.height as f32).ceil());
            glyphs.push(PreparedGlyph {
                ch,
                x: glyph_x,
                y: glyph_y,
            });
            pen_x += metrics.advance_width;
            previous = Some(ch);
        }
    }

    let pixel_bounds = if glyphs.is_empty() || !min_x.is_finite() || !min_y.is_finite() {
        None
    } else {
        Some((min_x, min_y, max_x.max(min_x + 1.0), max_y.max(min_y + 1.0)))
    };

    let mut metrics = TextMetrics {
        width: pixel_bounds
            .map(|(min_x, _, max_x, _)| (max_x - min_x).max(0.0))
            .unwrap_or(block_width),
        height: pixel_bounds
            .map(|(_, min_y, _, max_y)| (max_y - min_y).max(0.0))
            .unwrap_or(block_height),
        used_scale,
        line_count: lines.len(),
    };
    if request.stretch_width > 0.0 && request.stretch_height > 0.0 {
        metrics.width = request.stretch_width;
        metrics.height = request.stretch_height;
    }

    Some(PreparedTextLayout {
        glyphs,
        metrics,
        pixel_bounds,
    })
}

pub(crate) fn measure_text(request: &TextRenderRequest) -> Option<TextMetrics> {
    Some(prepare_text_layout(request)?.metrics)
}

pub(crate) fn rasterize_text_sprite(request: &TextRenderRequest) -> Option<RasterizedTextSprite> {
    let layout = prepare_text_layout(request)?;
    let (min_x, min_y, max_x, max_y) = layout.pixel_bounds?;
    let font = load_font(&request.font)?;
    let px = layout.metrics.used_scale.max(1.0);
    let border = if request.rotation.abs() > 0.0001
        && request.stretch_width <= 0.0
        && request.stretch_height <= 0.0
    {
        1u32
    } else {
        0u32
    };
    let width = (max_x - min_x).ceil().max(1.0) as u32 + border * 2;
    let height = (max_y - min_y).ceil().max(1.0) as u32 + border * 2;
    let mut text_image: RgbaImage = ImageBuffer::from_pixel(width, height, Rgba([0, 0, 0, 0]));

    for glyph in layout.glyphs {
        let (metrics, bitmap) = font.rasterize(glyph.ch, px);
        let base_x = (glyph.x - min_x).round() as i32 + border as i32;
        let top_y = (glyph.y - min_y).round() as i32 + border as i32;
        for gy in 0..metrics.height {
            for gx in 0..metrics.width {
                let alpha = bitmap[gy * metrics.width + gx];
                if alpha == 0 {
                    continue;
                }
                let tx = base_x + gx as i32;
                let ty = top_y + gy as i32;
                if tx < 0 || ty < 0 || tx >= width as i32 || ty >= height as i32 {
                    continue;
                }
                text_image.put_pixel(
                    tx as u32,
                    ty as u32,
                    Rgba([request.color.r, request.color.g, request.color.b, alpha]),
                );
            }
        }
    }

    let filter = if request.stretch_width > 0.0 && request.stretch_height > 0.0 {
        TextureFilter::Nearest
    } else if request.rotation.abs() > 0.0001 && matches!(request.font, FontHandle::Default) {
        TextureFilter::Nearest
    } else {
        TextureFilter::Linear
    };

    Some(RasterizedTextSprite {
        image: text_image,
        dest: Rect {
            x: min_x.round() - border as f32,
            y: min_y.round() - border as f32,
            w: if request.stretch_width > 0.0 && request.stretch_height > 0.0 {
                request.stretch_width.max(1.0)
            } else {
                width as f32
            },
            h: if request.stretch_width > 0.0 && request.stretch_height > 0.0 {
                request.stretch_height.max(1.0)
            } else {
                height as f32
            },
        },
        pivot: request.pivot,
        rotation: request.rotation,
        filter,
    })
}

fn blend(dest: &mut [u8], src: Color) {
    let src_a = src.a as f32 / 255.0;
    let inv = 1.0 - src_a;
    dest[0] = (src.r as f32 * src_a + dest[0] as f32 * inv).round() as u8;
    dest[1] = (src.g as f32 * src_a + dest[1] as f32 * inv).round() as u8;
    dest[2] = (src.b as f32 * src_a + dest[2] as f32 * inv).round() as u8;
    dest[3] = ((src.a as f32) + dest[3] as f32 * inv)
        .round()
        .clamp(0.0, 255.0) as u8;
}

fn rotate_local(x: f32, y: f32, rotation: f32) -> (f32, f32) {
    let cos_r = rotation.cos();
    let sin_r = rotation.sin();
    (x * cos_r - y * sin_r, x * sin_r + y * cos_r)
}

fn inverse_rotate(x: f32, y: f32, rotation: f32) -> (f32, f32) {
    rotate_local(x, y, -rotation)
}

fn world_point(x: f32, y: f32, pivot_x: f32, pivot_y: f32, rotation: f32) -> Vec2 {
    let local_x = x - pivot_x;
    let local_y = y - pivot_y;
    let (rx, ry) = rotate_local(local_x, local_y, rotation);
    Vec2 {
        x: pivot_x + rx,
        y: pivot_y + ry,
    }
}

fn rotated_rect_corners(bounds: Rect, pivot: Vec2, rotation: f32) -> [Vec2; 4] {
    [
        world_point(bounds.x, bounds.y, pivot.x, pivot.y, rotation),
        world_point(bounds.x + bounds.w, bounds.y, pivot.x, pivot.y, rotation),
        world_point(
            bounds.x + bounds.w,
            bounds.y + bounds.h,
            pivot.x,
            pivot.y,
            rotation,
        ),
        world_point(bounds.x, bounds.y + bounds.h, pivot.x, pivot.y, rotation),
    ]
}

fn bounds_from_points(points: &[Vec2]) -> Rect {
    let min_x = points
        .iter()
        .map(|point| point.x)
        .fold(f32::INFINITY, f32::min);
    let max_x = points
        .iter()
        .map(|point| point.x)
        .fold(f32::NEG_INFINITY, f32::max);
    let min_y = points
        .iter()
        .map(|point| point.y)
        .fold(f32::INFINITY, f32::min);
    let max_y = points
        .iter()
        .map(|point| point.y)
        .fold(f32::NEG_INFINITY, f32::max);
    Rect {
        x: min_x,
        y: min_y,
        w: (max_x - min_x).max(0.0),
        h: (max_y - min_y).max(0.0),
    }
}

fn rect_intersects_viewport(bounds: Rect, width: u32, height: u32) -> bool {
    bounds.x < width as f32
        && bounds.x + bounds.w > 0.0
        && bounds.y < height as f32
        && bounds.y + bounds.h > 0.0
}

pub(crate) fn command_intersects_viewport(command: &DrawCommand, width: u32, height: u32) -> bool {
    if width == 0 || height == 0 {
        return false;
    }

    let bounds = match command {
        DrawCommand::Rect {
            x,
            y,
            w,
            h,
            rotation,
            offset,
            ..
        } => {
            if *w <= 0.0 || *h <= 0.0 {
                return false;
            }
            let pivot = Vec2 {
                x: *x + *w * offset.x,
                y: *y + *h * offset.y,
            };
            bounds_from_points(&rotated_rect_corners(
                Rect {
                    x: *x,
                    y: *y,
                    w: *w,
                    h: *h,
                },
                pivot,
                *rotation,
            ))
        }
        DrawCommand::Triangle { a, b, c, .. } => bounds_from_points(&[*a, *b, *c]),
        DrawCommand::Circle { center, radius, .. } => {
            if *radius <= 0.0 {
                return false;
            }
            Rect {
                x: center.x - *radius,
                y: center.y - *radius,
                w: radius * 2.0,
                h: radius * 2.0,
            }
        }
        DrawCommand::Image {
            dest,
            rotation,
            pivot,
            ..
        } => {
            if dest.w <= 0.0 || dest.h <= 0.0 {
                return false;
            }
            bounds_from_points(&rotated_rect_corners(*dest, *pivot, *rotation))
        }
        DrawCommand::Text(request) => {
            if request.bounds.w <= 0.0 || request.bounds.h <= 0.0 {
                // Content-sized text computes its real sprite bounds during layout/rasterization,
                // so pre-layout culling cannot safely reject it here.
                return true;
            }
            bounds_from_points(&rotated_rect_corners(
                request.bounds,
                request.pivot,
                request.rotation,
            ))
        }
    };

    rect_intersects_viewport(bounds, width, height)
}

pub(crate) struct SoftwareRenderer {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl SoftwareRenderer {
    pub(crate) fn new(width: u32, height: u32) -> Self {
        Self {
            width: width.max(1),
            height: height.max(1),
            pixels: vec![0; width.max(1) as usize * height.max(1) as usize * 4],
        }
    }

    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(1);
        self.height = height.max(1);
        self.pixels
            .resize(self.width as usize * self.height as usize * 4, 0);
    }

    pub(crate) fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub(crate) fn render(
        &mut self,
        platform: &SharedPlatformState,
        render_state: &SharedRenderState,
    ) -> Result<(), String> {
        let clear = platform
            .lock()
            .map_err(|_| "platform lock poisoned".to_string())?
            .clear_color();
        for pixel in self.pixels.chunks_exact_mut(4) {
            pixel[0] = clear.r;
            pixel[1] = clear.g;
            pixel[2] = clear.b;
            pixel[3] = clear.a;
        }

        let commands = render_state
            .lock()
            .map_err(|_| "render state lock poisoned".to_string())?
            .drain();
        for command in commands {
            if !command_intersects_viewport(&command, self.width, self.height) {
                continue;
            }
            self.draw_command(command)?;
        }
        Ok(())
    }

    fn draw_command(&mut self, command: DrawCommand) -> Result<(), String> {
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
                let p0 = self.to_world(x, y, pivot_x, pivot_y, rotation);
                let p1 = self.to_world(x + w, y, pivot_x, pivot_y, rotation);
                let p2 = self.to_world(x + w, y + h, pivot_x, pivot_y, rotation);
                let p3 = self.to_world(x, y + h, pivot_x, pivot_y, rotation);
                self.fill_triangle(p0, p1, p2, color);
                self.fill_triangle(p0, p2, p3, color);
            }
            DrawCommand::Triangle { a, b, c, color } => self.fill_triangle(a, b, c, color),
            DrawCommand::Circle {
                center,
                radius,
                color,
            } => self.fill_circle(center, radius, color),
            DrawCommand::Image {
                image,
                dest,
                source,
                rotation,
                pivot,
                tint,
                filter,
            } => self.draw_image(image, dest, source, rotation, pivot, tint, filter)?,
            DrawCommand::Text(request) => self.draw_text(&request)?,
        }
        Ok(())
    }

    fn to_world(&self, x: f32, y: f32, pivot_x: f32, pivot_y: f32, rotation: f32) -> Vec2 {
        let local_x = x - pivot_x;
        let local_y = y - pivot_y;
        let (rx, ry) = rotate_local(local_x, local_y, rotation);
        Vec2 {
            x: pivot_x + rx,
            y: pivot_y + ry,
        }
    }

    fn fill_circle(&mut self, center: Vec2, radius: f32, color: Color) {
        let min_x = (center.x - radius).floor().max(0.0) as i32;
        let max_x = (center.x + radius).ceil().min(self.width as f32 - 1.0) as i32;
        let min_y = (center.y - radius).floor().max(0.0) as i32;
        let max_y = (center.y + radius).ceil().min(self.height as f32 - 1.0) as i32;
        let rr = radius * radius;
        for py in min_y..=max_y {
            for px in min_x..=max_x {
                let dx = px as f32 + 0.5 - center.x;
                let dy = py as f32 + 0.5 - center.y;
                if dx * dx + dy * dy <= rr {
                    self.put_pixel(px as u32, py as u32, color);
                }
            }
        }
    }

    fn edge(a: Vec2, b: Vec2, p: Vec2) -> f32 {
        (p.x - a.x) * (b.y - a.y) - (p.y - a.y) * (b.x - a.x)
    }

    fn fill_triangle(&mut self, a: Vec2, b: Vec2, c: Vec2, color: Color) {
        let min_x = a.x.min(b.x).min(c.x).floor().max(0.0) as i32;
        let max_x = a.x.max(b.x).max(c.x).ceil().min(self.width as f32 - 1.0) as i32;
        let min_y = a.y.min(b.y).min(c.y).floor().max(0.0) as i32;
        let max_y = a.y.max(b.y).max(c.y).ceil().min(self.height as f32 - 1.0) as i32;
        let area = Self::edge(a, b, c);
        if area.abs() < 0.0001 {
            return;
        }
        for py in min_y..=max_y {
            for px in min_x..=max_x {
                let point = Vec2 {
                    x: px as f32 + 0.5,
                    y: py as f32 + 0.5,
                };
                let w0 = Self::edge(b, c, point);
                let w1 = Self::edge(c, a, point);
                let w2 = Self::edge(a, b, point);
                if (area > 0.0 && w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0)
                    || (area < 0.0 && w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0)
                {
                    self.put_pixel(px as u32, py as u32, color);
                }
            }
        }
    }

    fn draw_image(
        &mut self,
        image: ImageHandle,
        dest: Rect,
        source: Option<Rect>,
        rotation: f32,
        pivot: Vec2,
        tint: Color,
        filter: TextureFilter,
    ) -> Result<(), String> {
        let (img_w, img_h) = image.dimensions().map_err(|e| e.to_string())?;
        let source = source.unwrap_or(Rect {
            x: 0.0,
            y: 0.0,
            w: img_w as f32,
            h: img_h as f32,
        });
        let corners = [
            self.to_world(dest.x, dest.y, pivot.x, pivot.y, rotation),
            self.to_world(dest.x + dest.w, dest.y, pivot.x, pivot.y, rotation),
            self.to_world(dest.x + dest.w, dest.y + dest.h, pivot.x, pivot.y, rotation),
            self.to_world(dest.x, dest.y + dest.h, pivot.x, pivot.y, rotation),
        ];
        let min_x = corners
            .iter()
            .map(|v| v.x)
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as i32;
        let max_x = corners
            .iter()
            .map(|v| v.x)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min(self.width as f32 - 1.0) as i32;
        let min_y = corners
            .iter()
            .map(|v| v.y)
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as i32;
        let max_y = corners
            .iter()
            .map(|v| v.y)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min(self.height as f32 - 1.0) as i32;

        image
            .with_image(|source_image| {
                for py in min_y..=max_y {
                    for px in min_x..=max_x {
                        let local_x = px as f32 + 0.5 - pivot.x;
                        let local_y = py as f32 + 0.5 - pivot.y;
                        let (rx, ry) = inverse_rotate(local_x, local_y, rotation);
                        let image_x = rx + pivot.x;
                        let image_y = ry + pivot.y;
                        let u = (image_x - dest.x) / dest.w;
                        let v = (image_y - dest.y) / dest.h;
                        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
                            continue;
                        }
                        let src_x = source.x + source.w * u;
                        let src_y = source.y + source.h * v;
                        let sample = sample_rgba(source_image, src_x, src_y, filter);
                        let color = modulate(sample, tint);
                        self.put_pixel(px as u32, py as u32, color);
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn draw_text(&mut self, request: &TextRenderRequest) -> Result<(), String> {
        let Some(sprite) = rasterize_text_sprite(request) else {
            return Ok(());
        };
        let image = crate::assets::ImageHandle::from_rgba_image(sprite.image);
        self.draw_image(
            image,
            sprite.dest,
            None,
            sprite.rotation,
            sprite.pivot,
            Color::WHITE,
            sprite.filter,
        )
    }

    fn put_pixel(&mut self, x: u32, y: u32, color: Color) {
        if x >= self.width || y >= self.height {
            return;
        }
        let index = ((y * self.width + x) * 4) as usize;
        blend(&mut self.pixels[index..index + 4], color);
    }
}

fn sample_rgba(image: &RgbaImage, x: f32, y: f32, filter: TextureFilter) -> Color {
    match filter {
        TextureFilter::Nearest => {
            let sx = x.floor().clamp(0.0, image.width().saturating_sub(1) as f32) as u32;
            let sy = y
                .floor()
                .clamp(0.0, image.height().saturating_sub(1) as f32) as u32;
            let [r, g, b, a] = image.get_pixel(sx, sy).0;
            Color::rgba(r, g, b, a)
        }
        TextureFilter::Linear => {
            let x0 = x.floor();
            let y0 = y.floor();
            let x1 = (x0 + 1.0).min(image.width().saturating_sub(1) as f32);
            let y1 = (y0 + 1.0).min(image.height().saturating_sub(1) as f32);
            let tx = (x - x0).clamp(0.0, 1.0);
            let ty = (y - y0).clamp(0.0, 1.0);
            let c00 = image.get_pixel(x0.max(0.0) as u32, y0.max(0.0) as u32).0;
            let c10 = image.get_pixel(x1 as u32, y0.max(0.0) as u32).0;
            let c01 = image.get_pixel(x0.max(0.0) as u32, y1 as u32).0;
            let c11 = image.get_pixel(x1 as u32, y1 as u32).0;
            let lerp = |a: u8, b: u8, t: f32| a as f32 + (b as f32 - a as f32) * t;
            let bilerp = |c00: u8, c10: u8, c01: u8, c11: u8| {
                let top = lerp(c00, c10, tx);
                let bottom = lerp(c01, c11, tx);
                lerp(top as u8, bottom as u8, ty).round() as u8
            };
            Color::rgba(
                bilerp(c00[0], c10[0], c01[0], c11[0]),
                bilerp(c00[1], c10[1], c01[1], c11[1]),
                bilerp(c00[2], c10[2], c01[2], c11[2]),
                bilerp(c00[3], c10[3], c01[3], c11[3]),
            )
        }
    }
}

fn modulate(sample: Color, tint: Color) -> Color {
    Color::rgba(
        ((sample.r as u16 * tint.r as u16) / 255) as u8,
        ((sample.g as u16 * tint.g as u16) / 255) as u8,
        ((sample.b as u16 * tint.b as u16) / 255) as u8,
        ((sample.a as u16 * tint.a as u16) / 255) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_sized_text_is_not_culled_before_layout() {
        let request = TextRenderRequest {
            text: "Hello".to_string(),
            bounds: Rect {
                x: 32.0,
                y: 48.0,
                w: 0.0,
                h: 0.0,
            },
            rotation: 0.0,
            pivot: Vec2::default(),
            color: Color::WHITE,
            font: FontHandle::Default,
            scale: 16.0,
            min_scale: 16.0,
            text_scale: TextScaleMode::None,
            align_x: TextAlignX::Left,
            align_y: TextAlignY::Top,
            wrap: TextWrapMode::None,
            padding_x: 0.0,
            padding_y: 0.0,
            line_spacing: 1.0,
            letter_spacing: 0.0,
            stretch_width: 0.0,
            stretch_height: 0.0,
        };

        assert!(command_intersects_viewport(
            &DrawCommand::Text(request),
            800,
            600
        ));
    }
}
