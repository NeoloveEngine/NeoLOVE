#![allow(dead_code)]

use crate::assets::ImageHandle;
use crate::platform::{lock_platform_state, Antialiasing, Color, SharedPlatformState};
use fontdue::Font;
use image::{ImageBuffer, Rgba, RgbaImage};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum TextAntialiasing {
    Off,
    Standard,
    #[default]
    High,
}

const DEFAULT_FONT_CACHE_KEY: &str = "__neolove_default_font__";
const DEFAULT_FONT_BYTES: &[u8] = include_bytes!("editor/assets/OpenSans-Regular.ttf");

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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TextStyleRange {
    pub start: usize,
    pub end: usize,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub color: Option<[u8; 4]>,
    pub size: Option<u32>,
    pub font: Option<FontHandle>,
    pub offset_x: Option<u32>,
    pub offset_y: Option<u32>,
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
    pub tab_size: f32,
    pub stretch_width: f32,
    pub stretch_height: f32,
    pub rich_text: Vec<TextStyleRange>,
    pub antialiasing: TextAntialiasing,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TextMetrics {
    pub width: f32,
    pub height: f32,
    pub used_scale: f32,
    pub line_count: usize,
    pub letter_bounds: Vec<Rect>,
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
        shader: Option<crate::shader::ShaderHandle>,
    },
    Triangle {
        a: Vec2,
        b: Vec2,
        c: Vec2,
        color: Color,
        shader: Option<crate::shader::ShaderHandle>,
    },
    Circle {
        center: Vec2,
        radius: f32,
        color: Color,
        shader: Option<crate::shader::ShaderHandle>,
    },
    Image {
        image: ImageHandle,
        dest: Rect,
        source: Option<Rect>,
        rotation: f32,
        pivot: Vec2,
        tint: Color,
        filter: TextureFilter,
        shader: Option<crate::shader::ShaderHandle>,
    },
    Text(TextRenderRequest),
}

#[derive(Default)]
pub(crate) struct RenderState {
    commands: Vec<DrawCommand>,
    overlay_commands: Vec<DrawCommand>,
    last_frame_commands: Option<Arc<[DrawCommand]>>,
    // Lighting: `config` persists across frames; lights and occluders are
    // re-queued every frame by their components and drained with the commands.
    lighting: crate::lighting::LightConfig,
    lights: Vec<crate::lighting::Light>,
    occluders: Vec<crate::lighting::Occluder>,
    // The last frame's lights/occluders, kept so `snapPhoto` can reproduce the
    // lit image (it re-renders from `last_frame_commands`).
    last_frame_lights: Vec<crate::lighting::Light>,
    last_frame_occluders: Vec<crate::lighting::Occluder>,
}

pub(crate) type SharedRenderState = Arc<Mutex<RenderState>>;

pub(crate) fn new_shared_render_state() -> SharedRenderState {
    Arc::new(Mutex::new(RenderState::default()))
}

#[derive(Clone)]
pub(crate) struct RasterizedTextSprite {
    pub image: Arc<RgbaImage>,
    pub dest: Rect,
    pub pivot: Vec2,
    pub rotation: f32,
    pub filter: TextureFilter,
}

#[derive(Clone)]
struct CachedRasterizedTextSprite {
    image: Arc<RgbaImage>,
    dest: Rect,
    pivot: Vec2,
    rotation: f32,
    filter: TextureFilter,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TextSpriteCacheKey {
    text: String,
    bounds: [u32; 4],
    rotation: u32,
    pivot: [u32; 2],
    color: [u8; 4],
    font: FontHandle,
    scale: u32,
    min_scale: u32,
    text_scale: TextScaleMode,
    align_x: TextAlignX,
    align_y: TextAlignY,
    wrap: TextWrapMode,
    padding_x: u32,
    padding_y: u32,
    line_spacing: u32,
    letter_spacing: u32,
    tab_size: u32,
    stretch_width: u32,
    stretch_height: u32,
    rich_text: Vec<TextStyleRange>,
    antialiasing: TextAntialiasing,
}

const TEXT_SPRITE_CACHE_LIMIT: usize = 256;

impl RenderState {
    pub(crate) fn queue(&mut self, command: DrawCommand) {
        self.commands.push(command);
    }

    pub(crate) fn extend_overlay(&mut self, commands: Vec<DrawCommand>) {
        self.overlay_commands.extend(commands);
    }

    pub(crate) fn drain(&mut self) -> Vec<DrawCommand> {
        let out = self.drain_without_remembering();
        self.last_frame_commands = Some(Arc::from(out.clone().into_boxed_slice()));
        out
    }

    fn drain_and_remember_shared(&mut self) -> Arc<[DrawCommand]> {
        let out = self.drain_without_remembering();
        let out = Arc::from(out.into_boxed_slice());
        self.last_frame_commands = Some(Arc::clone(&out));
        out
    }

    fn drain_without_remembering(&mut self) -> Vec<DrawCommand> {
        let mut out = Vec::with_capacity(self.commands.len() + self.overlay_commands.len());
        out.append(&mut self.commands);
        out.append(&mut self.overlay_commands);
        out
    }

    fn remember_last_frame(&mut self, commands: Vec<DrawCommand>) {
        self.last_frame_commands = Some(Arc::from(commands.into_boxed_slice()));
    }

    pub(crate) fn queue_light(&mut self, light: crate::lighting::Light) {
        self.lights.push(light);
    }

    pub(crate) fn queue_occluder(&mut self, occluder: crate::lighting::Occluder) {
        self.occluders.push(occluder);
    }

    pub(crate) fn lighting_config(&self) -> crate::lighting::LightConfig {
        self.lighting
    }

    pub(crate) fn set_lighting_config(&mut self, config: crate::lighting::LightConfig) {
        self.lighting = config;
    }

    pub(crate) fn update_lighting_config(
        &mut self,
        edit: impl FnOnce(&mut crate::lighting::LightConfig),
    ) {
        edit(&mut self.lighting);
    }

    /// Take the persistent config plus this frame's queued lights/occluders,
    /// clearing the per-frame lists for the next frame.
    pub(crate) fn take_lighting(
        &mut self,
    ) -> (
        crate::lighting::LightConfig,
        Vec<crate::lighting::Light>,
        Vec<crate::lighting::Occluder>,
    ) {
        let lights = std::mem::take(&mut self.lights);
        let occluders = std::mem::take(&mut self.occluders);
        self.last_frame_lights = lights.clone();
        self.last_frame_occluders = occluders.clone();
        (self.lighting, lights, occluders)
    }

    /// The persistent config plus the previous frame's lights/occluders, for
    /// reproducing the lit image outside the main render loop (e.g. snapPhoto).
    pub(crate) fn last_frame_lighting(
        &self,
    ) -> (
        crate::lighting::LightConfig,
        Vec<crate::lighting::Light>,
        Vec<crate::lighting::Occluder>,
    ) {
        (
            self.lighting,
            self.last_frame_lights.clone(),
            self.last_frame_occluders.clone(),
        )
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

fn text_sprite_cache() -> &'static Mutex<HashMap<TextSpriteCacheKey, CachedRasterizedTextSprite>> {
    static CACHE: OnceLock<Mutex<HashMap<TextSpriteCacheKey, CachedRasterizedTextSprite>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn text_layout_cache() -> &'static Mutex<HashMap<TextSpriteCacheKey, PreparedTextLayout>> {
    static CACHE: OnceLock<Mutex<HashMap<TextSpriteCacheKey, PreparedTextLayout>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn f32_cache_key(value: f32) -> u32 {
    if value == 0.0 { 0 } else { value.to_bits() }
}

fn text_sprite_cache_key(request: &TextRenderRequest) -> TextSpriteCacheKey {
    TextSpriteCacheKey {
        text: request.text.clone(),
        bounds: [
            f32_cache_key(request.bounds.x),
            f32_cache_key(request.bounds.y),
            f32_cache_key(request.bounds.w),
            f32_cache_key(request.bounds.h),
        ],
        rotation: f32_cache_key(request.rotation),
        pivot: [
            f32_cache_key(request.pivot.x),
            f32_cache_key(request.pivot.y),
        ],
        color: [
            request.color.r,
            request.color.g,
            request.color.b,
            request.color.a,
        ],
        font: request.font.clone(),
        scale: f32_cache_key(request.scale),
        min_scale: f32_cache_key(request.min_scale),
        text_scale: request.text_scale,
        align_x: request.align_x,
        align_y: request.align_y,
        wrap: request.wrap,
        padding_x: f32_cache_key(request.padding_x),
        padding_y: f32_cache_key(request.padding_y),
        line_spacing: f32_cache_key(request.line_spacing),
        letter_spacing: f32_cache_key(request.letter_spacing),
        tab_size: f32_cache_key(normalize_tab_size(request.tab_size)),
        stretch_width: f32_cache_key(request.stretch_width),
        stretch_height: f32_cache_key(request.stretch_height),
        rich_text: request.rich_text.clone(),
        antialiasing: request.antialiasing,
    }
}

pub(crate) fn text_render_request_cache_id(request: &TextRenderRequest) -> u64 {
    let mut hasher = DefaultHasher::new();
    request.text.hash(&mut hasher);
    [
        f32_cache_key(request.bounds.x),
        f32_cache_key(request.bounds.y),
        f32_cache_key(request.bounds.w),
        f32_cache_key(request.bounds.h),
    ]
    .hash(&mut hasher);
    f32_cache_key(request.rotation).hash(&mut hasher);
    [
        f32_cache_key(request.pivot.x),
        f32_cache_key(request.pivot.y),
    ]
    .hash(&mut hasher);
    [
        request.color.r,
        request.color.g,
        request.color.b,
        request.color.a,
    ]
    .hash(&mut hasher);
    request.font.hash(&mut hasher);
    f32_cache_key(request.scale).hash(&mut hasher);
    f32_cache_key(request.min_scale).hash(&mut hasher);
    request.text_scale.hash(&mut hasher);
    request.align_x.hash(&mut hasher);
    request.align_y.hash(&mut hasher);
    request.wrap.hash(&mut hasher);
    f32_cache_key(request.padding_x).hash(&mut hasher);
    f32_cache_key(request.padding_y).hash(&mut hasher);
    f32_cache_key(request.line_spacing).hash(&mut hasher);
    f32_cache_key(request.letter_spacing).hash(&mut hasher);
    f32_cache_key(normalize_tab_size(request.tab_size)).hash(&mut hasher);
    f32_cache_key(request.stretch_width).hash(&mut hasher);
    f32_cache_key(request.stretch_height).hash(&mut hasher);
    request.rich_text.hash(&mut hasher);
    request.antialiasing.hash(&mut hasher);
    hasher.finish()
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

pub(crate) fn drain_commands_without_remembering(
    render_state: &SharedRenderState,
) -> Result<Vec<DrawCommand>, String> {
    render_state
        .lock()
        .map_err(|_| "render state lock poisoned".to_string())
        .map(|mut state| state.drain_without_remembering())
}

pub(crate) fn drain_commands_and_remember(
    render_state: &SharedRenderState,
) -> Result<Arc<[DrawCommand]>, String> {
    render_state
        .lock()
        .map_err(|_| "render state lock poisoned".to_string())
        .map(|mut state| state.drain_and_remember_shared())
}

pub(crate) fn remember_last_frame_commands(
    render_state: &SharedRenderState,
    commands: Vec<DrawCommand>,
) -> Result<(), String> {
    render_state
        .lock()
        .map_err(|_| "render state lock poisoned".to_string())
        .map(|mut state| state.remember_last_frame(commands))
}

pub(crate) fn last_frame_commands(
    render_state: &SharedRenderState,
) -> Result<Option<Arc<[DrawCommand]>>, String> {
    render_state
        .lock()
        .map_err(|_| "render state lock poisoned".to_string())
        .map(|state| state.last_frame_commands.clone())
}

pub(crate) fn last_frame_lighting(
    render_state: &SharedRenderState,
) -> Result<
    (
        crate::lighting::LightConfig,
        Vec<crate::lighting::Light>,
        Vec<crate::lighting::Occluder>,
    ),
    String,
> {
    render_state
        .lock()
        .map_err(|_| "render state lock poisoned".to_string())
        .map(|state| state.last_frame_lighting())
}

fn command_uses_custom_shader(command: &DrawCommand) -> bool {
    match command {
        DrawCommand::Rect { shader, .. }
        | DrawCommand::Triangle { shader, .. }
        | DrawCommand::Circle { shader, .. }
        | DrawCommand::Image { shader, .. } => shader.is_some(),
        DrawCommand::Text(_) => false,
    }
}

#[derive(Clone, Debug)]
struct PreparedTextLine {
    text: String,
    width: f32,
}

#[derive(Clone, Debug)]
struct PreparedGlyph {
    ch: char,
    x: f32,
    y: f32,
    style: ResolvedTextStyle,
    bounds: Rect,
}

#[derive(Clone, Debug)]
struct ResolvedTextStyle {
    bold: bool,
    italic: bool,
    underline: bool,
    color: Color,
    scale: f32,
    font: FontHandle,
    offset_x: f32,
    offset_y: f32,
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

fn normalize_tab_size(tab_size: f32) -> f32 {
    if tab_size.is_finite() {
        tab_size.clamp(1.0, 32.0)
    } else {
        4.0
    }
}

fn glyph_advance_width(font: &Font, ch: char, px: f32, tab_size: f32) -> f32 {
    if ch == '\t' {
        font.metrics(' ', px).advance_width * normalize_tab_size(tab_size)
    } else {
        font.metrics(ch, px).advance_width
    }
}

fn horizontal_kern(font: &Font, previous: char, ch: char, px: f32) -> f32 {
    if previous == '\t' || ch == '\t' {
        0.0
    } else {
        font.horizontal_kern(previous, ch, px).unwrap_or(0.0)
    }
}

fn measure_line_width(font: &Font, text: &str, px: f32, letter_spacing: f32, tab_size: f32) -> f32 {
    let mut width = 0.0f32;
    let mut previous = None;
    let spacing = letter_spacing;

    for (index, ch) in text.chars().enumerate() {
        if index > 0 {
            width += spacing;
        }
        if let Some(prev) = previous {
            width += horizontal_kern(font, prev, ch, px);
        }
        width += glyph_advance_width(font, ch, px, tab_size);
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
    tab_size: f32,
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
            .map(|prev| horizontal_kern(font, prev, ch, px))
            .unwrap_or(0.0);
        let char_width = glyph_advance_width(font, ch, px, tab_size);
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

fn whitespace_preserving_tokens(text: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = 0usize;
    let mut current_is_whitespace = None;

    for (byte_index, ch) in text.char_indices() {
        let is_whitespace = ch.is_whitespace();
        match current_is_whitespace {
            Some(previous) if previous != is_whitespace => {
                tokens.push(&text[start..byte_index]);
                start = byte_index;
                current_is_whitespace = Some(is_whitespace);
            }
            Some(_) => {}
            None => current_is_whitespace = Some(is_whitespace),
        }
    }

    if start < text.len() {
        tokens.push(&text[start..]);
    }

    tokens
}

fn wrap_paragraph_word(
    font: &Font,
    text: &str,
    px: f32,
    limit: f32,
    letter_spacing: f32,
    tab_size: f32,
) -> Vec<String> {
    if limit <= 0.0 || !limit.is_finite() {
        return vec![text.to_string()];
    }

    let tokens = whitespace_preserving_tokens(text);
    if tokens.is_empty() {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();

    for token in tokens {
        let mut candidate = String::with_capacity(current.len() + token.len());
        candidate.push_str(&current);
        candidate.push_str(token);
        let candidate_width = measure_line_width(font, &candidate, px, letter_spacing, tab_size);
        if !current.is_empty() && candidate_width > limit {
            lines.push(current);
            current = String::new();
        }

        let token_width = measure_line_width(font, token, px, letter_spacing, tab_size);
        let token_is_word = token.chars().any(|ch| !ch.is_whitespace());
        if current.is_empty() && token_is_word && token_width > limit {
            let mut wrapped = wrap_paragraph_char(font, token, px, limit, letter_spacing, tab_size);
            current = wrapped.pop().unwrap_or_default();
            lines.extend(wrapped);
        } else {
            current.push_str(token);
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
    tab_size: f32,
) -> Vec<PreparedTextLine> {
    if text.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let wrapped = match (wrap, width_limit) {
            (TextWrapMode::None, _) | (_, None) => vec![paragraph.to_string()],
            (TextWrapMode::Word, Some(limit)) => {
                wrap_paragraph_word(font, paragraph, px, limit, letter_spacing, tab_size)
            }
            (TextWrapMode::Char, Some(limit)) => {
                wrap_paragraph_char(font, paragraph, px, limit, letter_spacing, tab_size)
            }
        };
        for line in wrapped {
            let width = measure_line_width(font, &line, px, letter_spacing, tab_size);
            lines.push(PreparedTextLine { text: line, width });
        }
    }
    lines
}

fn style_for_index(
    request: &TextRenderRequest,
    index: usize,
    base_scale: f32,
) -> ResolvedTextStyle {
    let mut style = ResolvedTextStyle {
        bold: false,
        italic: false,
        underline: false,
        color: request.color,
        scale: base_scale.max(1.0),
        font: request.font.clone(),
        offset_x: 0.0,
        offset_y: 0.0,
    };
    for range in &request.rich_text {
        if index >= range.start && index < range.end {
            style.bold |= range.bold;
            style.italic |= range.italic;
            style.underline |= range.underline;
            if let Some([r, g, b, a]) = range.color {
                style.color = Color::rgba(r, g, b, a);
            }
            if let Some(bits) = range.size {
                style.scale = (base_scale * f32::from_bits(bits)).max(1.0);
            }
            if let Some(font) = &range.font {
                style.font = font.clone();
            }
            if let Some(bits) = range.offset_x {
                style.offset_x = f32::from_bits(bits);
            }
            if let Some(bits) = range.offset_y {
                style.offset_y = f32::from_bits(bits);
            }
        }
    }
    style
}

fn prepare_text_layout_uncached(request: &TextRenderRequest) -> Option<PreparedTextLayout> {
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
            request.tab_size,
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

    let (lines, block_width, block_height, line_metrics, base_line_height, line_advance) = measured;
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
    let mut letter_bounds = Vec::new();
    let spacing = request.letter_spacing;
    let _px = used_scale.max(1.0);

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
            let global_index = glyphs.len();
            let style = style_for_index(request, global_index, used_scale);
            let glyph_font = load_font(&style.font).unwrap_or_else(|| font.clone());
            let glyph_px = style.scale.max(1.0);
            if char_index > 0 {
                pen_x += spacing;
            }
            if let Some(prev) = previous {
                pen_x += horizontal_kern(&glyph_font, prev, ch, glyph_px);
            }
            let render_ch = if ch == '\t' { ' ' } else { ch };
            let metrics = glyph_font.metrics(render_ch, glyph_px);
            let advance_width = glyph_advance_width(&glyph_font, ch, glyph_px, request.tab_size);
            let italic_slant = if style.italic { glyph_px * 0.18 } else { 0.0 };
            let cell_x = line_start_x + pen_x + style.offset_x;
            let cell_y = start_y + line_advance * line_index as f32 + style.offset_y;
            let cell_w = advance_width.max(0.0);
            let glyph_x = line_start_x + pen_x + metrics.xmin as f32 + style.offset_x;
            let glyph_y = baseline_y - metrics.height as f32 - metrics.ymin as f32 + style.offset_y;
            let bounds = Rect {
                x: glyph_x,
                y: glyph_y,
                w: metrics.width as f32 + italic_slant + if style.bold { 1.0 } else { 0.0 },
                h: metrics.height as f32,
            };
            letter_bounds.push(Rect {
                x: cell_x,
                y: cell_y,
                w: cell_w,
                h: base_line_height.max(1.0),
            });
            min_x = min_x.min(bounds.x.floor());
            min_y = min_y.min(bounds.y.floor());
            max_x = max_x.max((bounds.x + bounds.w).ceil());
            max_y = max_y.max((bounds.y + bounds.h).ceil());
            glyphs.push(PreparedGlyph {
                ch: render_ch,
                x: glyph_x,
                y: glyph_y,
                style,
                bounds,
            });
            pen_x += advance_width;
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
        letter_bounds,
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

fn prepare_text_layout(request: &TextRenderRequest) -> Option<PreparedTextLayout> {
    let cache_key = text_sprite_cache_key(request);
    if let Ok(cache) = text_layout_cache().lock() {
        if let Some(layout) = cache.get(&cache_key) {
            return Some(layout.clone());
        }
    }

    let layout = prepare_text_layout_uncached(request)?;
    if let Ok(mut cache) = text_layout_cache().lock() {
        if cache.len() >= TEXT_SPRITE_CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(cache_key, layout.clone());
    }
    Some(layout)
}

pub(crate) fn measure_text(request: &TextRenderRequest) -> Option<TextMetrics> {
    if cfg!(target_os = "emscripten") {
        let line_count = request.text.lines().count().max(1);
        let widest_line = request
            .text
            .lines()
            .map(|line| {
                line.chars()
                    .map(|ch| {
                        if ch == '\t' {
                            normalize_tab_size(request.tab_size)
                        } else {
                            1.0
                        }
                    })
                    .sum::<f32>()
            })
            .fold(0.0f32, f32::max);
        let used_scale = request.scale.max(1.0);
        Some(TextMetrics {
            width: widest_line * used_scale * 0.6,
            height: line_count as f32 * used_scale * request.line_spacing.max(0.1),
            used_scale,
            line_count,
            letter_bounds: Vec::new(),
        })
    } else {
        Some(prepare_text_layout(request)?.metrics)
    }
}

pub(crate) fn text_letter_bounds(request: &TextRenderRequest) -> Vec<Rect> {
    if cfg!(target_os = "emscripten") {
        return Vec::new();
    }
    prepare_text_layout(request)
        .map(|layout| layout.metrics.letter_bounds)
        .unwrap_or_default()
}

fn blend_text_pixel(image: &mut RgbaImage, x: u32, y: u32, source: [u8; 4]) {
    let destination = image.get_pixel(x, y).0;
    let source_alpha = source[3] as f32 / 255.0;
    let destination_alpha = destination[3] as f32 / 255.0;
    let output_alpha = source_alpha + destination_alpha * (1.0 - source_alpha);
    if output_alpha <= f32::EPSILON {
        return;
    }
    let channel = |index: usize| {
        ((source[index] as f32 * source_alpha
            + destination[index] as f32 * destination_alpha * (1.0 - source_alpha))
            / output_alpha)
            .round() as u8
    };
    image.put_pixel(
        x,
        y,
        Rgba([
            channel(0),
            channel(1),
            channel(2),
            (output_alpha * 255.0).round() as u8,
        ]),
    );
}

fn downsample_text_image(source: &RgbaImage, factor: u32, width: u32, height: u32) -> RgbaImage {
    let mut output = ImageBuffer::from_pixel(width, height, Rgba([0, 0, 0, 0]));
    let sample_count = (factor * factor) as f32;
    for y in 0..height {
        for x in 0..width {
            let mut alpha_sum = 0.0f32;
            let mut premultiplied = [0.0f32; 3];
            for sy in 0..factor {
                for sx in 0..factor {
                    let pixel = source.get_pixel(x * factor + sx, y * factor + sy).0;
                    let alpha = pixel[3] as f32 / 255.0;
                    alpha_sum += alpha;
                    for channel in 0..3 {
                        premultiplied[channel] += pixel[channel] as f32 * alpha;
                    }
                }
            }
            let output_alpha = alpha_sum / sample_count;
            if output_alpha <= f32::EPSILON {
                continue;
            }
            let color_divisor = alpha_sum.max(f32::EPSILON);
            output.put_pixel(
                x,
                y,
                Rgba([
                    (premultiplied[0] / color_divisor).round() as u8,
                    (premultiplied[1] / color_divisor).round() as u8,
                    (premultiplied[2] / color_divisor).round() as u8,
                    (output_alpha * 255.0).round() as u8,
                ]),
            );
        }
    }
    output
}

pub(crate) fn rasterize_text_sprite(request: &TextRenderRequest) -> Option<RasterizedTextSprite> {
    if cfg!(target_os = "emscripten") {
        return None;
    }

    let cache_key = text_sprite_cache_key(request);
    if let Ok(cache) = text_sprite_cache().lock() {
        if let Some(sprite) = cache.get(&cache_key) {
            return Some(RasterizedTextSprite {
                image: sprite.image.clone(),
                dest: sprite.dest,
                pivot: sprite.pivot,
                rotation: sprite.rotation,
                filter: sprite.filter,
            });
        }
    }

    let layout = prepare_text_layout(request)?;
    let (min_x, min_y, max_x, max_y) = layout.pixel_bounds?;
    let font = load_font(&request.font)?;
    let _px = layout.metrics.used_scale.max(1.0);
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
    let supersample = match request.antialiasing {
        TextAntialiasing::High => 2u32,
        TextAntialiasing::Off | TextAntialiasing::Standard => 1u32,
    };
    let mut text_image: RgbaImage = ImageBuffer::from_pixel(
        width * supersample,
        height * supersample,
        Rgba([0, 0, 0, 0]),
    );

    for glyph in layout.glyphs {
        let glyph_font = load_font(&glyph.style.font).unwrap_or_else(|| font.clone());
        let layout_metrics = glyph_font.metrics(glyph.ch, glyph.style.scale.max(1.0));
        let (metrics, bitmap) =
            glyph_font.rasterize(glyph.ch, glyph.style.scale.max(1.0) * supersample as f32);
        let glyph_origin_x = glyph.x - layout_metrics.xmin as f32;
        let glyph_baseline_y = glyph.y + layout_metrics.height as f32 + layout_metrics.ymin as f32;
        let base_x = ((glyph_origin_x - min_x) * supersample as f32 + metrics.xmin as f32).round()
            as i32
            + (border * supersample) as i32;
        let top_y = ((glyph_baseline_y - min_y) * supersample as f32
            - metrics.height as f32
            - metrics.ymin as f32)
            .round() as i32
            + (border * supersample) as i32;
        let passes = if glyph.style.bold { 2 } else { 1 };
        for pass in 0..passes {
            for gy in 0..metrics.height {
                for gx in 0..metrics.width {
                    let coverage = bitmap[gy * metrics.width + gx];
                    let coverage = if matches!(request.antialiasing, TextAntialiasing::Off) {
                        if coverage >= 128 { 255 } else { 0 }
                    } else {
                        coverage
                    };
                    let alpha = modulate_alpha(coverage, glyph.style.color.a);
                    if alpha == 0 {
                        continue;
                    }
                    let slant = if glyph.style.italic {
                        ((metrics.height - gy) as f32 * 0.18).round() as i32
                    } else {
                        0
                    };
                    let tx = base_x + gx as i32 + pass as i32 * supersample as i32 + slant;
                    let ty = top_y + gy as i32;
                    if tx < 0
                        || ty < 0
                        || tx >= text_image.width() as i32
                        || ty >= text_image.height() as i32
                    {
                        continue;
                    }
                    blend_text_pixel(
                        &mut text_image,
                        tx as u32,
                        ty as u32,
                        [
                            glyph.style.color.r,
                            glyph.style.color.g,
                            glyph.style.color.b,
                            alpha,
                        ],
                    );
                }
            }
        }
        if glyph.style.underline {
            let y = (top_y + metrics.height as i32 + supersample as i32)
                .clamp(0, text_image.height() as i32 - 1);
            let end_x = (base_x + (glyph.bounds.w * supersample as f32).ceil() as i32)
                .min(text_image.width() as i32);
            for underline_y in y..(y + supersample as i32).min(text_image.height() as i32) {
                for x in base_x.max(0)..end_x {
                    blend_text_pixel(
                        &mut text_image,
                        x as u32,
                        underline_y as u32,
                        [
                            glyph.style.color.r,
                            glyph.style.color.g,
                            glyph.style.color.b,
                            glyph.style.color.a,
                        ],
                    );
                }
            }
        }
    }

    let text_image = if supersample > 1 {
        downsample_text_image(&text_image, supersample, width, height)
    } else {
        text_image
    };

    let filter = if (request.stretch_width > 0.0 && request.stretch_height > 0.0)
        || (request.rotation.abs() > 0.0001 && matches!(request.font, FontHandle::Default))
    {
        TextureFilter::Nearest
    } else {
        TextureFilter::Linear
    };

    let sprite = RasterizedTextSprite {
        image: Arc::new(text_image),
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
    };

    if let Ok(mut cache) = text_sprite_cache().lock() {
        if cache.len() >= TEXT_SPRITE_CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(
            cache_key,
            CachedRasterizedTextSprite {
                image: sprite.image.clone(),
                dest: sprite.dest,
                pivot: sprite.pivot,
                rotation: sprite.rotation,
                filter: sprite.filter,
            },
        );
    }

    Some(sprite)
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

fn modulate_alpha(mask: u8, alpha: u8) -> u8 {
    ((mask as u16 * alpha as u16 + 127) / 255) as u8
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DirtyBounds {
    pub(crate) x: u32,
    pub(crate) y: u32,
    pub(crate) w: u32,
    pub(crate) h: u32,
}

fn command_bounds(command: &DrawCommand) -> Option<Rect> {
    match command {
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
                return None;
            }
            let pivot = Vec2 {
                x: *x + *w * offset.x,
                y: *y + *h * offset.y,
            };
            Some(bounds_from_points(&rotated_rect_corners(
                Rect {
                    x: *x,
                    y: *y,
                    w: *w,
                    h: *h,
                },
                pivot,
                *rotation,
            )))
        }
        DrawCommand::Triangle { a, b, c, .. } => Some(bounds_from_points(&[*a, *b, *c])),
        DrawCommand::Circle { center, radius, .. } => {
            if *radius <= 0.0 {
                return None;
            }
            Some(Rect {
                x: center.x - *radius,
                y: center.y - *radius,
                w: radius * 2.0,
                h: radius * 2.0,
            })
        }
        DrawCommand::Image {
            dest,
            rotation,
            pivot,
            ..
        } => {
            if dest.w <= 0.0 || dest.h <= 0.0 {
                return None;
            }
            Some(bounds_from_points(&rotated_rect_corners(
                *dest, *pivot, *rotation,
            )))
        }
        DrawCommand::Text(request) => {
            if request.bounds.w <= 0.0 || request.bounds.h <= 0.0 {
                return Some(request.bounds);
            }
            Some(bounds_from_points(&rotated_rect_corners(
                request.bounds,
                request.pivot,
                request.rotation,
            )))
        }
    }
}

pub(crate) fn commands_dirty_bounds<'a>(
    commands: impl IntoIterator<Item = &'a DrawCommand>,
    viewport: (u32, u32),
) -> Option<DirtyBounds> {
    let (width, height) = viewport;
    if width == 0 || height == 0 {
        return None;
    }

    let mut min_x = width as i32;
    let mut min_y = height as i32;
    let mut max_x = 0i32;
    let mut max_y = 0i32;
    let mut found = false;

    for command in commands {
        let Some(bounds) = command_bounds(command) else {
            continue;
        };
        if !rect_intersects_viewport(bounds, width, height) {
            continue;
        }
        let left = bounds.x.floor().max(0.0) as i32;
        let top = bounds.y.floor().max(0.0) as i32;
        let right = (bounds.x + bounds.w).ceil().min(width as f32) as i32;
        let bottom = (bounds.y + bounds.h).ceil().min(height as f32) as i32;
        if right <= left || bottom <= top {
            continue;
        }
        min_x = min_x.min(left);
        min_y = min_y.min(top);
        max_x = max_x.max(right);
        max_y = max_y.max(bottom);
        found = true;
    }

    found.then_some(DirtyBounds {
        x: min_x as u32,
        y: min_y as u32,
        w: (max_x - min_x) as u32,
        h: (max_y - min_y) as u32,
    })
}

pub(crate) fn translate_commands(commands: Vec<DrawCommand>, dx: f32, dy: f32) -> Vec<DrawCommand> {
    commands
        .into_iter()
        .map(|command| translate_command(command, dx, dy))
        .collect()
}

pub(crate) fn translate_command(command: DrawCommand, dx: f32, dy: f32) -> DrawCommand {
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
        } => DrawCommand::Rect {
            x: x + dx,
            y: y + dy,
            w,
            h,
            rotation,
            offset,
            color,
            shader,
        },
        DrawCommand::Triangle {
            a,
            b,
            c,
            color,
            shader,
        } => DrawCommand::Triangle {
            a: Vec2 {
                x: a.x + dx,
                y: a.y + dy,
            },
            b: Vec2 {
                x: b.x + dx,
                y: b.y + dy,
            },
            c: Vec2 {
                x: c.x + dx,
                y: c.y + dy,
            },
            color,
            shader,
        },
        DrawCommand::Circle {
            center,
            radius,
            color,
            shader,
        } => DrawCommand::Circle {
            center: Vec2 {
                x: center.x + dx,
                y: center.y + dy,
            },
            radius,
            color,
            shader,
        },
        DrawCommand::Image {
            image,
            dest,
            source,
            rotation,
            pivot,
            tint,
            filter,
            shader,
        } => DrawCommand::Image {
            image,
            dest: Rect {
                x: dest.x + dx,
                y: dest.y + dy,
                ..dest
            },
            source,
            rotation,
            pivot: Vec2 {
                x: pivot.x + dx,
                y: pivot.y + dy,
            },
            tint,
            filter,
            shader,
        },
        DrawCommand::Text(mut request) => {
            request.bounds.x += dx;
            request.bounds.y += dy;
            request.pivot.x += dx;
            request.pivot.y += dy;
            DrawCommand::Text(request)
        }
    }
}

pub(crate) fn command_intersects_viewport(command: &DrawCommand, width: u32, height: u32) -> bool {
    if width == 0 || height == 0 {
        return false;
    }

    let Some(bounds) = command_bounds(command) else {
        return false;
    };
    if matches!(command, DrawCommand::Text(request) if request.bounds.w <= 0.0 || request.bounds.h <= 0.0)
    {
        // Content-sized text computes its real sprite bounds during layout/rasterization,
        // so pre-layout culling cannot safely reject it here.
        return true;
    }

    rect_intersects_viewport(bounds, width, height)
}

pub(crate) struct SoftwareRenderer {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    antialiasing: Antialiasing,
}

impl SoftwareRenderer {
    pub(crate) fn new(width: u32, height: u32) -> Self {
        Self {
            width: width.max(1),
            height: height.max(1),
            pixels: vec![0; width.max(1) as usize * height.max(1) as usize * 4],
            antialiasing: Antialiasing::High,
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

    pub(crate) fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub(crate) fn render(
        &mut self,
        platform: &SharedPlatformState,
        render_state: &SharedRenderState,
    ) -> Result<(), String> {
        let (commands, lighting, lights, occluders) = {
            let mut state = render_state
                .lock()
                .map_err(|_| "render state lock poisoned".to_string())?;
            let commands = state.drain_without_remembering();
            let (lighting, lights, occluders) = state.take_lighting();
            (commands, lighting, lights, occluders)
        };
        self.render_command_slice(platform, &commands)?;
        self.apply_lighting_pass(&lighting, &lights, &occluders);
        render_state
            .lock()
            .map_err(|_| "render state lock poisoned".to_string())?
            .remember_last_frame(commands);
        Ok(())
    }

    /// Composite the 2D light map over the current framebuffer. A no-op when
    /// lighting is disabled or has nothing to contribute.
    pub(crate) fn apply_lighting_pass(
        &mut self,
        config: &crate::lighting::LightConfig,
        lights: &[crate::lighting::Light],
        occluders: &[crate::lighting::Occluder],
    ) {
        crate::lighting::apply_lighting(
            &mut self.pixels,
            self.width,
            self.height,
            config,
            lights,
            occluders,
        );
    }

    pub(crate) fn render_commands(
        &mut self,
        platform: &SharedPlatformState,
        commands: &[DrawCommand],
    ) -> Result<(), String> {
        self.render_command_slice(platform, commands)
    }

    fn render_command_slice(
        &mut self,
        platform: &SharedPlatformState,
        commands: &[DrawCommand],
    ) -> Result<(), String> {
        if commands.iter().any(command_uses_custom_shader) {
            #[cfg(target_os = "emscripten")]
            let shader_error =
                "custom shaders require the browser WebGL path, but a shader command reached the software fallback unexpectedly.".to_string();
            #[cfg(all(not(target_os = "emscripten"), feature = "vulkan"))]
            let shader_error =
                "custom shaders require the Vulkan renderer; NeoLOVE is currently using the software fallback because Vulkan initialization failed earlier. Check the Vulkan warning above for the exact driver or surface error.".to_string();
            #[cfg(all(not(target_os = "emscripten"), not(feature = "vulkan")))]
            let shader_error =
                "custom shaders require the Vulkan renderer, but this NeoLOVE binary was built without Vulkan support. Rebuild or reinstall with `--features vulkan` so NeoLOVE can use your installed Vulkan driver.".to_string();
            return Err(shader_error);
        }

        let state = lock_platform_state(platform);
        let clear = state.clear_color();
        self.antialiasing = state.antialiasing();
        drop(state);
        self.clear_to_color(clear);
        self.draw_unshaded_commands(commands)
    }

    pub(crate) fn clear_to_color(&mut self, clear: Color) {
        for pixel in self.pixels.chunks_exact_mut(4) {
            pixel[0] = clear.r;
            pixel[1] = clear.g;
            pixel[2] = clear.b;
            pixel[3] = clear.a;
        }
    }

    pub(crate) fn clear_transparent(&mut self) {
        self.clear_to_color(Color::rgba(0, 0, 0, 0));
    }

    pub(crate) fn draw_unshaded_commands(
        &mut self,
        commands: &[DrawCommand],
    ) -> Result<(), String> {
        if commands.iter().any(command_uses_custom_shader) {
            return Err("draw_unshaded_commands received a shader command".to_string());
        }
        for command in commands {
            if !command_intersects_viewport(&command, self.width, self.height) {
                continue;
            }
            self.draw_command(command)?;
        }
        Ok(())
    }

    fn draw_command(&mut self, command: &DrawCommand) -> Result<(), String> {
        match command {
            DrawCommand::Rect {
                x,
                y,
                w,
                h,
                rotation,
                offset,
                color,
                ..
            } => {
                let (x, y, w, h, rotation, offset, color) =
                    (*x, *y, *w, *h, *rotation, *offset, *color);
                if rotation.abs() <= 0.0001 {
                    self.fill_axis_aligned_rect(x, y, w, h, color);
                    return Ok(());
                }
                let pivot_x = x + w * offset.x;
                let pivot_y = y + h * offset.y;
                let p0 = self.to_world(x, y, pivot_x, pivot_y, rotation);
                let p1 = self.to_world(x + w, y, pivot_x, pivot_y, rotation);
                let p2 = self.to_world(x + w, y + h, pivot_x, pivot_y, rotation);
                let p3 = self.to_world(x, y + h, pivot_x, pivot_y, rotation);
                self.fill_triangle(p0, p1, p2, color);
                self.fill_triangle(p0, p2, p3, color);
            }
            DrawCommand::Triangle { a, b, c, color, .. } => self.fill_triangle(*a, *b, *c, *color),
            DrawCommand::Circle {
                center,
                radius,
                color,
                ..
            } => self.fill_circle(*center, *radius, *color),
            DrawCommand::Image {
                image,
                dest,
                source,
                rotation,
                pivot,
                tint,
                filter,
                ..
            } => self.draw_image(
                image.clone(),
                *dest,
                *source,
                *rotation,
                *pivot,
                *tint,
                *filter,
            )?,
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

    fn fill_axis_aligned_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }

        if color.a == 255
            && x.fract() == 0.0
            && y.fract() == 0.0
            && w.fract() == 0.0
            && h.fract() == 0.0
        {
            let min_x = x.max(0.0) as i32;
            let max_x = (x + w).min(self.width as f32) as i32;
            let min_y = y.max(0.0) as i32;
            let max_y = (y + h).min(self.height as f32) as i32;
            if max_x <= min_x || max_y <= min_y {
                return;
            }
            for py in min_y..max_y {
                let row_start = py as usize * self.width as usize * 4;
                let row = &mut self.pixels
                    [row_start + min_x as usize * 4..row_start + max_x as usize * 4];
                for pixel in row.chunks_exact_mut(4) {
                    pixel[0] = color.r;
                    pixel[1] = color.g;
                    pixel[2] = color.b;
                    pixel[3] = 255;
                }
            }
            return;
        }

        let min_x = x.floor().max(0.0) as i32;
        let max_x = (x + w).ceil().min(self.width as f32) as i32;
        let min_y = y.floor().max(0.0) as i32;
        let max_y = (y + h).ceil().min(self.height as f32) as i32;
        for py in min_y..max_y {
            for px in min_x..max_x {
                let coverage = if matches!(self.antialiasing, Antialiasing::Off) {
                    let cx = px as f32 + 0.5;
                    let cy = py as f32 + 0.5;
                    if cx >= x && cx <= x + w && cy >= y && cy <= y + h {
                        1.0
                    } else {
                        0.0
                    }
                } else {
                    let overlap_x =
                        ((px as f32 + 1.0).min(x + w) - (px as f32).max(x)).clamp(0.0, 1.0);
                    let overlap_y =
                        ((py as f32 + 1.0).min(y + h) - (py as f32).max(y)).clamp(0.0, 1.0);
                    overlap_x * overlap_y
                };
                if coverage > 0.0 {
                    let mut sampled = color;
                    sampled.a = (color.a as f32 * coverage).round() as u8;
                    self.put_pixel(px as u32, py as u32, sampled);
                }
            }
        }
    }

    fn fill_circle(&mut self, center: Vec2, radius: f32, color: Color) {
        let min_x = (center.x - radius).floor().max(0.0) as i32;
        let max_x = (center.x + radius).ceil().min(self.width as f32 - 1.0) as i32;
        let min_y = (center.y - radius).floor().max(0.0) as i32;
        let max_y = (center.y + radius).ceil().min(self.height as f32 - 1.0) as i32;
        let samples = match self.antialiasing {
            Antialiasing::Off => 1,
            Antialiasing::Standard => 2,
            Antialiasing::High => 4,
        };
        let total_samples = (samples * samples) as u32;
        let rr = radius * radius;
        for py in min_y..=max_y {
            for px in min_x..=max_x {
                let center_dx = px as f32 + 0.5 - center.x;
                let center_dy = py as f32 + 0.5 - center.y;
                let center_distance = (center_dx * center_dx + center_dy * center_dy).sqrt();
                if center_distance <= radius - std::f32::consts::FRAC_1_SQRT_2 {
                    self.put_pixel(px as u32, py as u32, color);
                    continue;
                }
                if center_distance >= radius + std::f32::consts::FRAC_1_SQRT_2 {
                    continue;
                }
                let mut covered = 0u32;
                for sample_y in 0..samples {
                    for sample_x in 0..samples {
                        let dx = px as f32 + (sample_x as f32 + 0.5) / samples as f32 - center.x;
                        let dy = py as f32 + (sample_y as f32 + 0.5) / samples as f32 - center.y;
                        if dx * dx + dy * dy <= rr {
                            covered += 1;
                        }
                    }
                }
                if covered > 0 {
                    let mut sampled = color;
                    sampled.a = ((color.a as u32 * covered) / total_samples) as u8;
                    self.put_pixel(px as u32, py as u32, sampled);
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
        let samples = match self.antialiasing {
            Antialiasing::Off => 1,
            Antialiasing::Standard => 2,
            Antialiasing::High => 4,
        };
        let total_samples = (samples * samples) as u32;
        for py in min_y..=max_y {
            for px in min_x..=max_x {
                let corners = [
                    Vec2 {
                        x: px as f32,
                        y: py as f32,
                    },
                    Vec2 {
                        x: px as f32 + 1.0,
                        y: py as f32,
                    },
                    Vec2 {
                        x: px as f32,
                        y: py as f32 + 1.0,
                    },
                    Vec2 {
                        x: px as f32 + 1.0,
                        y: py as f32 + 1.0,
                    },
                ];
                let inside = |point: Vec2| {
                    let w0 = Self::edge(b, c, point);
                    let w1 = Self::edge(c, a, point);
                    let w2 = Self::edge(a, b, point);
                    (area > 0.0 && w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0)
                        || (area < 0.0 && w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0)
                };
                if corners.iter().copied().all(inside) {
                    self.put_pixel(px as u32, py as u32, color);
                    continue;
                }
                let fully_outside_edge = |edge_a: Vec2, edge_b: Vec2| {
                    if area > 0.0 {
                        corners
                            .iter()
                            .all(|point| Self::edge(edge_a, edge_b, *point) < 0.0)
                    } else {
                        corners
                            .iter()
                            .all(|point| Self::edge(edge_a, edge_b, *point) > 0.0)
                    }
                };
                if fully_outside_edge(b, c) || fully_outside_edge(c, a) || fully_outside_edge(a, b)
                {
                    continue;
                }
                let mut covered = 0u32;
                for sample_y in 0..samples {
                    for sample_x in 0..samples {
                        let point = Vec2 {
                            x: px as f32 + (sample_x as f32 + 0.5) / samples as f32,
                            y: py as f32 + (sample_y as f32 + 0.5) / samples as f32,
                        };
                        let w0 = Self::edge(b, c, point);
                        let w1 = Self::edge(c, a, point);
                        let w2 = Self::edge(a, b, point);
                        if (area > 0.0 && w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0)
                            || (area < 0.0 && w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0)
                        {
                            covered += 1;
                        }
                    }
                }
                if covered > 0 {
                    let mut sampled = color;
                    sampled.a = ((color.a as u32 * covered) / total_samples) as u8;
                    self.put_pixel(px as u32, py as u32, sampled);
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
        image
            .with_image(|source_image| {
                self.draw_image_pixels(source_image, dest, source, rotation, pivot, tint, filter)
            })
            .map_err(|e| e.to_string())?
    }

    fn draw_image_pixels(
        &mut self,
        source_image: &RgbaImage,
        dest: Rect,
        source: Rect,
        rotation: f32,
        pivot: Vec2,
        tint: Color,
        filter: TextureFilter,
    ) -> Result<(), String> {
        if dest.w <= 0.0 || dest.h <= 0.0 || source.w <= 0.0 || source.h <= 0.0 || tint.a == 0 {
            return Ok(());
        }

        if rotation.abs() <= 0.0001 {
            self.draw_axis_aligned_image_pixels(source_image, dest, source, tint, filter);
            return Ok(());
        }

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
        Ok(())
    }

    fn draw_axis_aligned_image_pixels(
        &mut self,
        source_image: &RgbaImage,
        dest: Rect,
        source: Rect,
        tint: Color,
        filter: TextureFilter,
    ) {
        let start_x = dest.x.floor().max(0.0) as i32;
        let end_x = (dest.x + dest.w).ceil().min(self.width as f32) as i32;
        let start_y = dest.y.floor().max(0.0) as i32;
        let end_y = (dest.y + dest.h).ceil().min(self.height as f32) as i32;
        if end_x <= start_x || end_y <= start_y {
            return;
        }

        let src_max_x = source_image.width().saturating_sub(1) as f32;
        let src_max_y = source_image.height().saturating_sub(1) as f32;
        let tint_is_white = tint == Color::WHITE;

        for py in start_y..end_y {
            let v = (py as f32 + 0.5 - dest.y) / dest.h;
            if !(0.0..=1.0).contains(&v) {
                continue;
            }
            let src_y = source.y + source.h * v;
            let row_start = py as usize * self.width as usize * 4;

            for px in start_x..end_x {
                let u = (px as f32 + 0.5 - dest.x) / dest.w;
                if !(0.0..=1.0).contains(&u) {
                    continue;
                }
                let src_x = source.x + source.w * u;
                let sample = match filter {
                    TextureFilter::Nearest => {
                        let sx = src_x.floor().clamp(0.0, src_max_x) as u32;
                        let sy = src_y.floor().clamp(0.0, src_max_y) as u32;
                        let [r, g, b, a] = source_image.get_pixel(sx, sy).0;
                        Color::rgba(r, g, b, a)
                    }
                    TextureFilter::Linear => sample_rgba(source_image, src_x, src_y, filter),
                };
                let color = if tint_is_white {
                    sample
                } else {
                    modulate(sample, tint)
                };
                if color.a == 0 {
                    continue;
                }

                let index = row_start + px as usize * 4;
                let dest_pixel = &mut self.pixels[index..index + 4];
                if color.a == 255 {
                    dest_pixel[0] = color.r;
                    dest_pixel[1] = color.g;
                    dest_pixel[2] = color.b;
                    dest_pixel[3] = color.a;
                } else {
                    blend(dest_pixel, color);
                }
            }
        }
    }

    fn draw_text(&mut self, request: &TextRenderRequest) -> Result<(), String> {
        let Some(sprite) = rasterize_text_sprite(request) else {
            return Ok(());
        };
        self.draw_image_pixels(
            sprite.image.as_ref(),
            sprite.dest,
            Rect {
                x: 0.0,
                y: 0.0,
                w: sprite.image.width() as f32,
                h: sprite.image.height() as f32,
            },
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
        let dest = &mut self.pixels[index..index + 4];
        if color.a == 255 {
            dest[0] = color.r;
            dest[1] = color.g;
            dest[2] = color.b;
            dest[3] = 255;
        } else {
            blend(dest, color);
        }
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
            tab_size: 4.0,
            stretch_width: 0.0,
            stretch_height: 0.0,
            rich_text: Vec::new(),
            antialiasing: TextAntialiasing::High,
        };

        assert!(command_intersects_viewport(
            &DrawCommand::Text(request),
            800,
            600
        ));
    }

    #[test]
    fn rasterizes_text_sprite_for_default_font() {
        let request = TextRenderRequest {
            text: "NeoLOVE".to_string(),
            bounds: Rect {
                x: 24.0,
                y: 32.0,
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
            tab_size: 4.0,
            stretch_width: 0.0,
            stretch_height: 0.0,
            rich_text: Vec::new(),
            antialiasing: TextAntialiasing::High,
        };

        let sprite = rasterize_text_sprite(&request).expect("expected rasterized text sprite");
        assert!(sprite.dest.w > 0.0);
        assert!(sprite.dest.h > 0.0);
        assert!(sprite.image.width() > 0);
        assert!(sprite.image.height() > 0);
    }

    #[test]
    fn fitted_text_rasterizes_at_used_scale() {
        let request = TextRenderRequest {
            text: "Text".to_string(),
            bounds: Rect {
                x: 0.0,
                y: 0.0,
                w: 120.0,
                h: 60.0,
            },
            rotation: 0.0,
            pivot: Vec2::default(),
            color: Color::WHITE,
            font: FontHandle::Default,
            scale: 3500.0,
            min_scale: 1.0,
            text_scale: TextScaleMode::Fit,
            align_x: TextAlignX::Center,
            align_y: TextAlignY::Center,
            wrap: TextWrapMode::None,
            padding_x: 0.0,
            padding_y: 0.0,
            line_spacing: 1.0,
            letter_spacing: 0.0,
            tab_size: 4.0,
            stretch_width: 0.0,
            stretch_height: 0.0,
            rich_text: Vec::new(),
            antialiasing: TextAntialiasing::High,
        };

        let metrics = measure_text(&request).expect("metrics");
        assert!(metrics.used_scale < 3500.0);
        assert!(metrics.used_scale > 1.0);
        let sprite = rasterize_text_sprite(&request).expect("sprite");
        assert!(
            sprite.dest.w <= request.bounds.w + 2.0,
            "sprite width {} exceeded bounds {}",
            sprite.dest.w,
            request.bounds.w
        );
        assert!(
            sprite.dest.h <= request.bounds.h + 2.0,
            "sprite height {} exceeded bounds {}",
            sprite.dest.h,
            request.bounds.h
        );
    }

    #[test]
    fn letter_bounds_use_stable_line_box_for_punctuation() {
        let request = TextRenderRequest {
            text: "a,a".to_string(),
            bounds: Rect {
                x: 24.0,
                y: 32.0,
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
            tab_size: 4.0,
            stretch_width: 0.0,
            stretch_height: 0.0,
            rich_text: Vec::new(),
            antialiasing: TextAntialiasing::High,
        };

        let metrics = measure_text(&request).expect("expected text metrics");
        assert_eq!(metrics.letter_bounds.len(), 3);
        assert_eq!(metrics.letter_bounds[0].y, metrics.letter_bounds[1].y);
        assert_eq!(metrics.letter_bounds[0].h, metrics.letter_bounds[1].h);
        assert_eq!(metrics.letter_bounds[1].y, metrics.letter_bounds[2].y);
        assert_eq!(metrics.letter_bounds[1].h, metrics.letter_bounds[2].h);
    }

    #[test]
    fn word_wrap_preserves_trailing_space_letter_bounds() {
        let request = TextRenderRequest {
            text: "abc ".to_string(),
            bounds: Rect {
                x: 24.0,
                y: 32.0,
                w: 180.0,
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
            wrap: TextWrapMode::Word,
            padding_x: 0.0,
            padding_y: 0.0,
            line_spacing: 1.0,
            letter_spacing: 0.0,
            tab_size: 4.0,
            stretch_width: 0.0,
            stretch_height: 0.0,
            rich_text: Vec::new(),
            antialiasing: TextAntialiasing::High,
        };

        let metrics = measure_text(&request).expect("expected text metrics");
        assert_eq!(metrics.letter_bounds.len(), 4);
        assert!(metrics.letter_bounds[3].w > 0.0);
    }

    #[test]
    fn tab_size_controls_tab_advance() {
        let mut narrow = TextRenderRequest {
            text: "\t".to_string(),
            bounds: Rect {
                x: 24.0,
                y: 32.0,
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
            tab_size: 2.0,
            stretch_width: 0.0,
            stretch_height: 0.0,
            rich_text: Vec::new(),
            antialiasing: TextAntialiasing::High,
        };
        let narrow_metrics = measure_text(&narrow).expect("expected text metrics");

        narrow.tab_size = 4.0;
        let wide_metrics = measure_text(&narrow).expect("expected text metrics");

        assert_eq!(narrow_metrics.letter_bounds.len(), 1);
        assert_eq!(wide_metrics.letter_bounds.len(), 1);
        assert!(wide_metrics.letter_bounds[0].w > narrow_metrics.letter_bounds[0].w * 1.9);
    }

    #[test]
    fn rasterized_text_sprite_applies_color_alpha_to_glyph_mask() {
        let request = TextRenderRequest {
            text: "NeoLOVE".to_string(),
            bounds: Rect {
                x: 24.0,
                y: 32.0,
                w: 0.0,
                h: 0.0,
            },
            rotation: 0.0,
            pivot: Vec2::default(),
            color: Color::rgba(255, 255, 255, 96),
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
            tab_size: 4.0,
            stretch_width: 0.0,
            stretch_height: 0.0,
            rich_text: Vec::new(),
            antialiasing: TextAntialiasing::High,
        };

        let sprite = rasterize_text_sprite(&request).expect("expected rasterized text sprite");
        let max_alpha = sprite
            .image
            .pixels()
            .map(|pixel| pixel.0[3])
            .max()
            .unwrap_or(0);

        assert!(max_alpha > 0);
        assert!(max_alpha <= request.color.a);
    }

    #[test]
    fn high_quality_text_has_smooth_coverage_while_off_is_pixel_hard() {
        let mut request = TextRenderRequest {
            text: "Quality".to_string(),
            bounds: Rect::default(),
            rotation: 0.0,
            pivot: Vec2::default(),
            color: Color::WHITE,
            font: FontHandle::Default,
            scale: 19.0,
            min_scale: 19.0,
            text_scale: TextScaleMode::None,
            align_x: TextAlignX::Left,
            align_y: TextAlignY::Top,
            wrap: TextWrapMode::None,
            padding_x: 0.0,
            padding_y: 0.0,
            line_spacing: 1.0,
            letter_spacing: 0.0,
            tab_size: 4.0,
            stretch_width: 0.0,
            stretch_height: 0.0,
            rich_text: Vec::new(),
            antialiasing: TextAntialiasing::Off,
        };
        let hard = rasterize_text_sprite(&request).expect("hard text");
        assert!(
            hard.image
                .pixels()
                .all(|pixel| matches!(pixel.0[3], 0 | 255))
        );

        request.antialiasing = TextAntialiasing::High;
        let smooth = rasterize_text_sprite(&request).expect("smooth text");
        assert!(
            smooth
                .image
                .pixels()
                .any(|pixel| pixel.0[3] > 0 && pixel.0[3] < 255)
        );
    }

    #[test]
    fn rich_text_character_offsets_move_visual_and_letter_bounds() {
        let base = TextRenderRequest {
            text: "AB".to_string(),
            bounds: Rect::default(),
            rotation: 0.0,
            pivot: Vec2::default(),
            color: Color::WHITE,
            font: FontHandle::Default,
            scale: 20.0,
            min_scale: 20.0,
            text_scale: TextScaleMode::None,
            align_x: TextAlignX::Left,
            align_y: TextAlignY::Top,
            wrap: TextWrapMode::None,
            padding_x: 0.0,
            padding_y: 0.0,
            line_spacing: 1.0,
            letter_spacing: 0.0,
            tab_size: 4.0,
            stretch_width: 0.0,
            stretch_height: 0.0,
            rich_text: Vec::new(),
            antialiasing: TextAntialiasing::High,
        };
        let original = measure_text(&base).expect("base metrics");
        let mut offset = base;
        offset.rich_text.push(TextStyleRange {
            start: 1,
            end: 2,
            bold: false,
            italic: false,
            underline: false,
            color: None,
            size: None,
            font: None,
            offset_x: Some(6.0f32.to_bits()),
            offset_y: Some((-4.0f32).to_bits()),
        });
        let moved = measure_text(&offset).expect("offset metrics");
        assert!((moved.letter_bounds[0].x - original.letter_bounds[0].x).abs() < 0.01);
        assert!((moved.letter_bounds[0].y - original.letter_bounds[0].y).abs() < 0.01);
        assert!((moved.letter_bounds[1].x - original.letter_bounds[1].x - 6.0).abs() < 0.01);
        assert!((moved.letter_bounds[1].y - original.letter_bounds[1].y + 4.0).abs() < 0.01);
    }

    #[test]
    fn software_geometry_antialiasing_is_configurable() {
        let platform = crate::platform::new_shared_platform_state();
        {
            let mut state = lock_platform_state(&platform);
            state.set_clear_color(Color::rgba(0, 0, 0, 0));
            state.set_antialiasing(Antialiasing::Off);
        }
        let command = DrawCommand::Circle {
            center: Vec2 { x: 8.2, y: 8.4 },
            radius: 4.3,
            color: Color::WHITE,
            shader: None,
        };
        let mut renderer = SoftwareRenderer::new(18, 18);
        renderer
            .render_commands(&platform, std::slice::from_ref(&command))
            .expect("hard circle");
        assert!(
            renderer
                .pixels()
                .chunks_exact(4)
                .all(|pixel| matches!(pixel[3], 0 | 255))
        );

        lock_platform_state(&platform).set_antialiasing(Antialiasing::High);
        renderer
            .render_commands(&platform, &[command])
            .expect("smooth circle");
        assert!(
            renderer
                .pixels()
                .chunks_exact(4)
                .any(|pixel| pixel[3] > 0 && pixel[3] < 255)
        );
    }
}
