#![allow(dead_code)]

use crate::assets::ImageHandle;
use crate::platform::{Antialiasing, Color, SharedPlatformState, lock_platform_state};
use fontdue::Font;
use image::{ImageBuffer, Rgba, RgbaImage};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
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
    Mesh3D(crate::render3d::Mesh3DCommand),
    Particles3D(crate::particles3d::ParticleSystem3DCommand),
    Text(TextRenderRequest),
}

struct InteractionSurfaceId(usize);

impl Default for InteractionSurfaceId {
    fn default() -> Self {
        static NEXT_SURFACE_ID: AtomicUsize = AtomicUsize::new(1);
        Self(NEXT_SURFACE_ID.fetch_add(1, Ordering::Relaxed))
    }
}

impl Drop for InteractionSurfaceId {
    fn drop(&mut self) {
        crate::widget_interaction::remove_surface(self.0);
    }
}

#[derive(Default)]
pub(crate) struct RenderState {
    interaction_surface: InteractionSurfaceId,
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
    lights_3d: Vec<crate::render3d::Light3D>,
    last_frame_lights_3d: Vec<crate::render3d::Light3D>,
    reflection_probes_3d: Vec<crate::render3d::ReflectionProbe3D>,
    last_frame_reflection_probes_3d: Vec<crate::render3d::ReflectionProbe3D>,
    // Camera selection persists across frames, while the offset is resolved
    // afresh by the camera pre-pass. Scene commands stay in world space until
    // they are drained; editor/debug overlays are intentionally screen-space.
    active_camera: Option<usize>,
    camera_seen_this_frame: bool,
    fallback_camera: Option<(usize, Vec2)>,
    camera_offset: Vec2,
    active_camera_3d: Option<usize>,
    camera_3d_seen_this_frame: bool,
    fallback_camera_3d: Option<(usize, crate::render3d::Camera3D)>,
    camera_3d: crate::render3d::Camera3D,
    environment_3d: crate::environment3d::Environment3D,
    environment_3d_owner: Option<usize>,
    post_process: crate::post_process::PostProcessStack,
}

pub(crate) type SharedRenderState = Arc<Mutex<RenderState>>;

pub(crate) fn new_shared_render_state() -> SharedRenderState {
    Arc::new(Mutex::new(RenderState::default()))
}

/// Stable widget-interaction identity owned for the render state's lifetime.
pub(crate) fn interaction_surface_id(render_state: &SharedRenderState) -> usize {
    match render_state.lock() {
        Ok(state) => state.interaction_surface.0,
        // The immutable id remains valid even if unrelated render work poisoned
        // the mutex, so cleanup and widget routing can still agree on the key.
        Err(poisoned) => poisoned.into_inner().interaction_surface.0,
    }
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

    pub(crate) fn queue_all(&mut self, commands: impl IntoIterator<Item = DrawCommand>) {
        self.commands.extend(commands);
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
        let commands = std::mem::take(&mut self.commands);
        if self.camera_offset.x == 0.0 && self.camera_offset.y == 0.0 {
            out.extend(commands);
        } else {
            out.extend(translate_commands(
                commands,
                self.camera_offset.x,
                self.camera_offset.y,
            ));
        }
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

    pub(crate) fn queue_light_3d(&mut self, light: crate::render3d::Light3D) {
        self.lights_3d.push(light);
    }

    pub(crate) fn queue_reflection_probe_3d(&mut self, probe: crate::render3d::ReflectionProbe3D) {
        self.reflection_probes_3d.push(probe.sanitized());
    }

    /// Start resolving the camera for a new rendered frame. The last resolved
    /// offset remains available until this point so input dispatched earlier in
    /// the frame still matches the image the player can currently see.
    pub(crate) fn begin_camera_frame(&mut self) {
        self.camera_seen_this_frame = false;
        self.fallback_camera = None;
        self.camera_offset = Vec2::default();
        self.camera_3d_seen_this_frame = false;
        self.fallback_camera_3d = None;
    }

    /// Offer an enabled Camera component to the camera pre-pass. The explicitly
    /// active camera wins; the first available camera is retained as a safe
    /// fallback when a scene unload removed the previously active component.
    pub(crate) fn submit_camera(&mut self, id: usize, offset: Vec2) {
        if self.fallback_camera.is_none() {
            self.fallback_camera = Some((id, offset));
        }
        if self.active_camera == Some(id) {
            self.camera_offset = offset;
            self.camera_seen_this_frame = true;
        }
    }

    /// Make a camera active immediately. This also resolves its transform for
    /// the current frame, which makes SetActive reliable from any update order.
    pub(crate) fn activate_camera(&mut self, id: usize, offset: Vec2) {
        self.active_camera = Some(id);
        self.camera_offset = offset;
        self.camera_seen_this_frame = true;
    }

    pub(crate) fn submit_camera_3d(&mut self, id: usize, camera: crate::render3d::Camera3D) {
        if self.fallback_camera_3d.is_none() {
            self.fallback_camera_3d = Some((id, camera));
        }
        if self.active_camera_3d == Some(id) {
            self.camera_3d = camera;
            self.camera_3d_seen_this_frame = true;
        }
    }

    pub(crate) fn activate_camera_3d(&mut self, id: usize, camera: crate::render3d::Camera3D) {
        self.active_camera_3d = Some(id);
        self.camera_3d = camera;
        self.camera_3d_seen_this_frame = true;
    }

    pub(crate) fn clear_camera(&mut self, id: usize) {
        if self.active_camera == Some(id) {
            self.active_camera = None;
            self.camera_offset = Vec2::default();
            self.camera_seen_this_frame = false;
        }
        if self
            .fallback_camera
            .is_some_and(|(candidate, _)| candidate == id)
        {
            self.fallback_camera = None;
        }
        if self.active_camera_3d == Some(id) {
            self.active_camera_3d = None;
            self.camera_3d = crate::render3d::Camera3D::default();
            self.camera_3d_seen_this_frame = false;
        }
        if self
            .fallback_camera_3d
            .is_some_and(|(candidate, _)| candidate == id)
        {
            self.fallback_camera_3d = None;
        }
    }

    pub(crate) fn is_camera_active(&self, id: usize) -> bool {
        self.active_camera == Some(id) || self.active_camera_3d == Some(id)
    }

    /// Complete camera selection. With no enabled Camera component, the offset
    /// is exactly (0, 0), preserving NeoLOVE's original screen-space rendering.
    pub(crate) fn finish_camera_frame(&mut self) {
        if !self.camera_seen_this_frame {
            if let Some((id, offset)) = self.fallback_camera {
                self.active_camera = Some(id);
                self.camera_offset = offset;
                self.camera_seen_this_frame = true;
            } else {
                self.active_camera = None;
                self.camera_offset = Vec2::default();
            }
        }
        if !self.camera_3d_seen_this_frame {
            if let Some((id, camera)) = self.fallback_camera_3d {
                self.active_camera_3d = Some(id);
                self.camera_3d = camera;
                self.camera_3d_seen_this_frame = true;
            } else {
                self.active_camera_3d = None;
                self.camera_3d = crate::render3d::Camera3D::default();
            }
        }
    }

    pub(crate) fn camera_offset(&self) -> Vec2 {
        self.camera_offset
    }

    pub(crate) fn camera_3d(&self) -> crate::render3d::Camera3D {
        self.camera_3d
    }

    pub(crate) fn set_environment_3d(
        &mut self,
        owner: Option<usize>,
        environment: crate::environment3d::Environment3D,
    ) {
        self.environment_3d = environment;
        self.environment_3d_owner = owner;
    }

    pub(crate) fn clear_environment_3d(&mut self, owner: Option<usize>) {
        if owner.is_none() || self.environment_3d_owner == owner {
            self.environment_3d = crate::environment3d::Environment3D::default();
            self.environment_3d_owner = None;
        }
    }

    pub(crate) fn environment_3d(&self) -> crate::environment3d::Environment3D {
        self.environment_3d.clone()
    }

    pub(crate) fn edit_post_process(
        &mut self,
        edit: impl FnOnce(&mut crate::post_process::PostProcessStack),
    ) {
        edit(&mut self.post_process);
    }

    pub(crate) fn post_process(&self) -> &crate::post_process::PostProcessStack {
        &self.post_process
    }

    pub(crate) fn apply_post_process(
        &mut self,
        width: usize,
        height: usize,
        pixels: &mut [u8],
    ) -> Result<(), crate::post_process::PostProcessError> {
        self.post_process.apply_in_place(width, height, pixels)
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
        let mut lights = std::mem::take(&mut self.lights);
        let mut occluders = std::mem::take(&mut self.occluders);
        if self.camera_offset.x != 0.0 || self.camera_offset.y != 0.0 {
            for light in &mut lights {
                light.x += self.camera_offset.x;
                light.y += self.camera_offset.y;
            }
            for occluder in &mut occluders {
                occluder.cx += self.camera_offset.x;
                occluder.cy += self.camera_offset.y;
            }
        }
        self.last_frame_lights = lights.clone();
        self.last_frame_occluders = occluders.clone();
        (self.lighting, lights, occluders)
    }

    pub(crate) fn take_lights_3d(&mut self) -> Vec<crate::render3d::Light3D> {
        let lights = std::mem::take(&mut self.lights_3d);
        self.last_frame_lights_3d = lights.clone();
        lights
    }

    pub(crate) fn take_reflection_probes_3d(&mut self) -> Vec<crate::render3d::ReflectionProbe3D> {
        let probes = std::mem::take(&mut self.reflection_probes_3d);
        self.last_frame_reflection_probes_3d = probes.clone();
        probes
    }

    pub(crate) fn last_frame_reflection_probes_3d(
        &self,
    ) -> Vec<crate::render3d::ReflectionProbe3D> {
        self.last_frame_reflection_probes_3d.clone()
    }

    pub(crate) fn last_frame_lights_3d(&self) -> Vec<crate::render3d::Light3D> {
        self.last_frame_lights_3d.clone()
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
        DrawCommand::Mesh3D(command) => command.shader.is_some(),
        DrawCommand::Particles3D(_) => false,
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
        DrawCommand::Mesh3D(_) | DrawCommand::Particles3D(_) => None,
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
        DrawCommand::Mesh3D(command) => DrawCommand::Mesh3D(command),
        DrawCommand::Particles3D(command) => DrawCommand::Particles3D(command),
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
    if matches!(
        command,
        DrawCommand::Mesh3D(_) | DrawCommand::Particles3D(_)
    ) {
        // Mesh bounds are projected in the backend preparation pass. Frustum
        // rejection there is both tighter and cheaper than fabricating a 2D
        // world-space rectangle here.
        return true;
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

fn perspective_correct_weights(
    vertices: &[crate::render3d::ProjectedVertex; 3],
    barycentric: [f32; 3],
) -> Option<[f32; 3]> {
    let mut weights = [0.0; 3];
    let mut denominator = 0.0;
    for corner in 0..3 {
        let clip_w = vertices[corner].clip_position[3];
        if clip_w.abs() <= f32::EPSILON || !clip_w.is_finite() {
            return None;
        }
        weights[corner] = barycentric[corner] / clip_w;
        denominator += weights[corner];
    }
    if denominator.abs() <= f32::EPSILON || !denominator.is_finite() {
        return None;
    }
    for weight in &mut weights {
        *weight /= denominator;
    }
    Some(weights)
}

pub(crate) struct SoftwareRenderer {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    depth: Vec<f32>,
    three_d_aa_scratch: Vec<u8>,
    three_d_aa_bounds: Option<(u32, u32, u32, u32)>,
    antialiasing: Antialiasing,
    light_cache: crate::lighting::LightMapCache,
    environment_cache_key: Option<crate::environment3d::SoftwareEnvironmentCacheKey>,
    environment_cache_pixels: Vec<u8>,
}

struct SoftwarePbrMaterial {
    material: Arc<crate::mesh::MeshMaterial>,
    base_color: Option<Arc<RgbaImage>>,
    normal: Option<SoftwarePbrTexture>,
    metallic_roughness: Option<SoftwarePbrTexture>,
    emissive: Option<SoftwarePbrTexture>,
}

struct SoftwarePbrTexture {
    image: Arc<RgbaImage>,
    solid: Option<[f32; 4]>,
}

impl SoftwarePbrTexture {
    fn new(image: Arc<RgbaImage>) -> Self {
        let solid = if image.width().saturating_mul(image.height()) <= 16 {
            let first = image.pixels().next().map(|pixel| pixel.0);
            first
                .filter(|first| image.pixels().all(|pixel| pixel.0 == *first))
                .map(|pixel| pixel.map(|value| value as f32 / 255.0))
        } else {
            None
        };
        Self { image, solid }
    }

    fn sample(&self, uv: [f32; 2]) -> [f32; 4] {
        self.solid
            .unwrap_or_else(|| sample_software_texture(&self.image, uv))
    }

    fn is_flat_normal(&self) -> bool {
        self.solid.is_some_and(|sample| {
            (sample[0] - 128.0 / 255.0).abs() <= f32::EPSILON
                && (sample[1] - 128.0 / 255.0).abs() <= f32::EPSILON
                && (sample[2] - 1.0).abs() <= f32::EPSILON
        })
    }

    fn is_neutral_orm(&self) -> bool {
        self.solid
            .is_some_and(|sample| sample[1] == 1.0 && sample[2] == 1.0)
    }

    fn is_white_emissive(&self) -> bool {
        self.solid.is_some_and(|sample| sample[..3] == [1.0; 3])
    }
}

fn software_image_pixels(image: Option<&ImageHandle>) -> Result<Option<Arc<RgbaImage>>, String> {
    image
        .map(|image| {
            image
                .snapshot()
                .map(|snapshot| snapshot.into_parts().2)
                .map_err(|error| error.to_string())
        })
        .transpose()
}

fn sample_software_texture(texture: &RgbaImage, uv: [f32; 2]) -> [f32; 4] {
    if texture.width() == 0 || texture.height() == 0 {
        return [1.0; 4];
    }
    let x = (uv[0].rem_euclid(1.0) * texture.width().saturating_sub(1) as f32).round() as u32;
    let y =
        ((1.0 - uv[1]).clamp(0.0, 1.0) * texture.height().saturating_sub(1) as f32).round() as u32;
    texture.get_pixel(x, y).0.map(|value| value as f32 / 255.0)
}

impl SoftwareRenderer {
    pub(crate) fn new(width: u32, height: u32) -> Self {
        Self {
            width: width.max(1),
            height: height.max(1),
            pixels: vec![0; width.max(1) as usize * height.max(1) as usize * 4],
            // Most NeoLOVE projects and editor panels only submit 2D commands.
            // Allocate the four-byte-per-pixel depth surface on the first 3D
            // frame instead of making every high-resolution renderer pay for it.
            depth: Vec::new(),
            // The software 3D edge filter needs an immutable copy of the
            // framebuffer. Keep it lazy so established 2D-only projects do
            // not pay another four bytes per logical pixel.
            three_d_aa_scratch: Vec::new(),
            three_d_aa_bounds: None,
            antialiasing: Antialiasing::High,
            light_cache: crate::lighting::LightMapCache::default(),
            environment_cache_key: None,
            environment_cache_pixels: Vec::new(),
        }
    }

    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if self.width == width && self.height == height {
            return;
        }
        self.width = width;
        self.height = height;
        self.pixels
            .resize(self.width as usize * self.height as usize * 4, 0);
        // A future 3D frame recreates this at the new dimensions. Dropping it
        // here also releases an old high-resolution allocation when a project
        // switches back to a resized 2D viewport.
        self.depth = Vec::new();
        self.three_d_aa_scratch = Vec::new();
        self.three_d_aa_bounds = None;
        self.light_cache = crate::lighting::LightMapCache::default();
        self.environment_cache_key = None;
        self.environment_cache_pixels.clear();
    }

    pub(crate) fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    #[cfg(target_os = "emscripten")]
    pub(crate) fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.pixels
    }

    pub(crate) fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub(crate) fn set_antialiasing(&mut self, antialiasing: Antialiasing) {
        self.antialiasing = antialiasing;
        if matches!(antialiasing, Antialiasing::Off) {
            self.three_d_aa_scratch = Vec::new();
        }
    }

    pub(crate) fn render(
        &mut self,
        platform: &SharedPlatformState,
        render_state: &SharedRenderState,
    ) -> Result<(), String> {
        let (
            commands,
            lighting,
            lights,
            occluders,
            lights_3d,
            reflection_probes_3d,
            environment,
            camera_3d,
        ) = {
            let mut state = render_state
                .lock()
                .map_err(|_| "render state lock poisoned".to_string())?;
            let commands = state.drain_without_remembering();
            let (lighting, lights, occluders) = state.take_lighting();
            let lights_3d = state.take_lights_3d();
            let reflection_probes_3d = state.take_reflection_probes_3d();
            let environment = state.environment_3d();
            let camera_3d = state.camera_3d();
            (
                commands,
                lighting,
                lights,
                occluders,
                lights_3d,
                reflection_probes_3d,
                environment,
                camera_3d,
            )
        };
        self.render_command_slice(
            platform,
            &commands,
            &lights_3d,
            &reflection_probes_3d,
            Some((&environment, camera_3d)),
        )?;
        self.apply_lighting_pass(&lighting, &lights, &occluders);
        render_state
            .lock()
            .map_err(|_| "render state lock poisoned".to_string())?
            .apply_post_process(self.width as usize, self.height as usize, &mut self.pixels)
            .map_err(|error| error.to_string())?;
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
        crate::lighting::apply_lighting_cached(
            &mut self.pixels,
            self.width,
            self.height,
            config,
            lights,
            occluders,
            &mut self.light_cache,
        );
    }

    pub(crate) fn apply_post_process_pass(
        &mut self,
        render_state: &SharedRenderState,
    ) -> Result<(), String> {
        render_state
            .lock()
            .map_err(|_| "render state lock poisoned".to_string())?
            .apply_post_process(self.width as usize, self.height as usize, &mut self.pixels)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn render_commands(
        &mut self,
        platform: &SharedPlatformState,
        commands: &[DrawCommand],
    ) -> Result<(), String> {
        self.render_command_slice(platform, commands, &[], &[], None)
    }

    fn render_command_slice(
        &mut self,
        platform: &SharedPlatformState,
        commands: &[DrawCommand],
        lights_3d: &[crate::render3d::Light3D],
        reflection_probes_3d: &[crate::render3d::ReflectionProbe3D],
        environment: Option<(
            &crate::environment3d::Environment3D,
            crate::render3d::Camera3D,
        )>,
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
        self.set_antialiasing(state.antialiasing());
        drop(state);
        if let Some((environment, camera)) = environment {
            self.draw_environment_background(environment, camera, clear);
        } else {
            self.clear_to_color(clear);
        }
        self.draw_unshaded_commands_with_lights(
            commands,
            lights_3d,
            reflection_probes_3d,
            environment.map(|(environment, _)| environment),
        )
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

    pub(crate) fn draw_environment_background(
        &mut self,
        environment: &crate::environment3d::Environment3D,
        camera: crate::render3d::Camera3D,
        fallback: Color,
    ) {
        let cacheable_image_environment = environment.enabled
            && match environment.mode {
                crate::environment3d::EnvironmentMode3D::Equirectangular => environment
                    .equirectangular
                    .as_ref()
                    .is_some_and(|image| image.revision().is_ok()),
                crate::environment3d::EnvironmentMode3D::Cubemap => environment
                    .cubemap
                    .as_ref()
                    .is_some_and(|cubemap| cubemap.snapshot().is_ok()),
                _ => false,
            };
        if !cacheable_image_environment {
            crate::environment3d::render_software_background(
                environment,
                camera,
                self.width as usize,
                self.height as usize,
                &mut self.pixels,
                fallback,
            );
            // Do not make ordinary 2D/solid/gradient games retain a second
            // full-resolution framebuffer. Leaving panorama mode releases its
            // cache allocation immediately.
            self.environment_cache_key = None;
            self.environment_cache_pixels = Vec::new();
            return;
        }
        let key = crate::environment3d::software_cache_key(
            environment,
            camera,
            self.width as usize,
            self.height as usize,
            fallback,
        );
        if self.environment_cache_key.as_ref() == Some(&key)
            && self.environment_cache_pixels.len() == self.pixels.len()
        {
            self.pixels.copy_from_slice(&self.environment_cache_pixels);
        } else {
            crate::environment3d::render_software_background(
                environment,
                camera,
                self.width as usize,
                self.height as usize,
                &mut self.pixels,
                fallback,
            );
            self.environment_cache_pixels.resize(self.pixels.len(), 0);
            self.environment_cache_pixels.copy_from_slice(&self.pixels);
            self.environment_cache_key = Some(key);
        }
    }

    pub(crate) fn draw_unshaded_commands(
        &mut self,
        commands: &[DrawCommand],
    ) -> Result<(), String> {
        self.draw_unshaded_commands_with_lights(commands, &[], &[], None)
    }

    pub(crate) fn draw_commands_with_3d_lights(
        &mut self,
        commands: &[DrawCommand],
        lights_3d: &[crate::render3d::Light3D],
        environment: Option<&crate::environment3d::Environment3D>,
    ) -> Result<(), String> {
        self.draw_unshaded_commands_with_lights(commands, lights_3d, &[], environment)
    }

    pub(crate) fn draw_commands_with_3d_scene(
        &mut self,
        commands: &[DrawCommand],
        lights_3d: &[crate::render3d::Light3D],
        reflection_probes_3d: &[crate::render3d::ReflectionProbe3D],
        environment: Option<&crate::environment3d::Environment3D>,
    ) -> Result<(), String> {
        self.draw_unshaded_commands_with_lights(
            commands,
            lights_3d,
            reflection_probes_3d,
            environment,
        )
    }

    fn draw_unshaded_commands_with_lights(
        &mut self,
        commands: &[DrawCommand],
        lights_3d: &[crate::render3d::Light3D],
        reflection_probes_3d: &[crate::render3d::ReflectionProbe3D],
        environment: Option<&crate::environment3d::Environment3D>,
    ) -> Result<(), String> {
        if commands.iter().any(command_uses_custom_shader) {
            return Err("draw_unshaded_commands received a shader command".to_string());
        }
        let has_3d = commands.iter().any(|command| {
            matches!(
                command,
                DrawCommand::Mesh3D(_) | DrawCommand::Particles3D(_)
            )
        });
        if has_3d {
            self.prepare_depth_buffer();
        } else if !self.three_d_aa_scratch.is_empty() {
            // Switching back to a 2D-only scene releases the optional
            // full-resolution copy instead of permanently raising its memory
            // floor after one 3D frame.
            self.three_d_aa_scratch = Vec::new();
        }
        let pbr_environment = if let Some(environment) = environment.filter(|value| value.enabled) {
            match environment.mode {
                crate::environment3d::EnvironmentMode3D::Equirectangular => environment
                    .equirectangular
                    .as_ref()
                    .map(|image| {
                        image
                            .snapshot()
                            .map(|snapshot| {
                                crate::render3d::PbrEnvironment::new(
                                    snapshot.into_parts().2,
                                    environment.intensity,
                                    environment.rotation_degrees,
                                )
                            })
                            .map_err(|error| error.to_string())
                    })
                    .transpose()?,
                crate::environment3d::EnvironmentMode3D::Cubemap => environment
                    .cubemap
                    .as_ref()
                    .map(|cubemap| {
                        cubemap
                            .snapshot()
                            .map(|snapshot| {
                                crate::render3d::PbrEnvironment::new_cubemap(
                                    snapshot.faces,
                                    environment.intensity,
                                    environment.rotation_degrees,
                                )
                            })
                            .map_err(|error| error.to_string())
                    })
                    .transpose()?,
                _ => None,
            }
        } else {
            None
        };
        let prepared_probe_environments = reflection_probes_3d
            .iter()
            .map(|probe| {
                probe
                    .cubemap
                    .snapshot()
                    .map(|snapshot| {
                        crate::render3d::PbrEnvironment::new_cubemap(
                            snapshot.faces,
                            probe.intensity,
                            probe.rotation_degrees,
                        )
                    })
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, String>>()?;
        let fog = environment
            .filter(|value| value.enabled && value.fog.enabled)
            .map(|value| value.fog.sanitized());
        let ambient_occlusion = environment
            .filter(|value| value.enabled && value.ambient_occlusion.enabled)
            .map(|value| value.ambient_occlusion.sanitized());
        let ambient_occluders = ambient_occlusion
            .map(|_| crate::render3d::gather_ambient_occluders_3d(commands.iter()))
            .unwrap_or_default();
        // Opaque 3D geometry owns the depth buffer and is rendered before the
        // existing 2D command stream. This makes canvas/UI components natural
        // overlays without changing their long-standing z/order behavior.
        for (source_index, command) in commands.iter().enumerate() {
            let DrawCommand::Mesh3D(mesh) = command else {
                continue;
            };
            let selected_probe =
                crate::render3d::mesh_world_bounds_3d(mesh)
                    .ok()
                    .and_then(|receiver| {
                        crate::render3d::select_reflection_probe_3d(
                            receiver.center,
                            reflection_probes_3d,
                        )
                    });
            let blended_environment = selected_probe.map(|selection| {
                crate::render3d::PbrEnvironment::blended(
                    prepared_probe_environments[selection.index].clone(),
                    pbr_environment.clone(),
                    selection.weight,
                )
            });
            self.draw_mesh_3d(
                mesh,
                lights_3d,
                blended_environment.as_ref().or(pbr_environment.as_ref()),
                fog,
                ambient_occlusion,
                &ambient_occluders,
                source_index,
            )?;
        }
        // Transparent billboards are submitted after all opaque meshes. Each
        // emitter's projected triangles are sorted back-to-front while the
        // existing depth buffer still rejects particles hidden by geometry.
        for command in commands {
            let DrawCommand::Particles3D(particles) = command else {
                continue;
            };
            self.draw_particles_3d(particles, fog)?;
        }
        if has_3d {
            self.apply_3d_antialiasing();
        }
        for command in commands {
            if matches!(
                command,
                DrawCommand::Mesh3D(_) | DrawCommand::Particles3D(_)
            ) {
                continue;
            }
            if !command_intersects_viewport(&command, self.width, self.height) {
                continue;
            }
            self.draw_command(command)?;
        }
        Ok(())
    }

    fn prepare_depth_buffer(&mut self) {
        let pixel_count = self.width as usize * self.height as usize;
        if self.depth.len() == pixel_count {
            self.depth.fill(f32::INFINITY);
        } else {
            self.depth = vec![f32::INFINITY; pixel_count];
        }
        self.three_d_aa_bounds = None;
    }

    fn include_3d_aa_bounds(&mut self, min_x: u32, min_y: u32, max_x: u32, max_y: u32) {
        self.three_d_aa_bounds = Some(match self.three_d_aa_bounds {
            Some((old_min_x, old_min_y, old_max_x, old_max_y)) => (
                old_min_x.min(min_x),
                old_min_y.min(min_y),
                old_max_x.max(max_x),
                old_max_y.max(max_y),
            ),
            None => (min_x, min_y, max_x, max_y),
        });
    }

    /// Smooth software-rendered 3D silhouette and high-contrast material
    /// edges. The pass is deliberately placed before the 2D command stream so
    /// canvas/UI overlays retain their existing pixel and ordering behavior.
    /// Native Vulkan uses multisampling instead of this framebuffer filter.
    fn apply_3d_antialiasing(&mut self) {
        let base_strength = match self.antialiasing {
            Antialiasing::Off => {
                self.three_d_aa_scratch = Vec::new();
                return;
            }
            Antialiasing::Standard => 0.38,
            Antialiasing::High => 0.62,
        };
        let high_quality = matches!(self.antialiasing, Antialiasing::High);
        let width = self.width as usize;
        let height = self.height as usize;
        if width < 3 || height < 3 || self.depth.len() != width * height {
            return;
        }

        let Some((min_x, min_y, max_x, max_y)) = self.three_d_aa_bounds else {
            self.three_d_aa_scratch = Vec::new();
            return;
        };
        let start_x = min_x.saturating_sub(1).max(1) as usize;
        let start_y = min_y.saturating_sub(1).max(1) as usize;
        let end_x = max_x.saturating_add(1).min(self.width.saturating_sub(2)) as usize;
        let end_y = max_y.saturating_add(1).min(self.height.saturating_sub(2)) as usize;
        if start_x > end_x || start_y > end_y {
            return;
        }

        self.three_d_aa_scratch.resize(self.pixels.len(), 0);
        // Only copy the geometry union plus the one-pixel neighborhood read by
        // the filter. Large mostly-empty 3D viewports avoid a full-frame memory
        // copy while the scratch allocation remains reusable.
        let copy_start_x = start_x.saturating_sub(1);
        let copy_end_x = (end_x + 1).min(width - 1);
        for y in start_y.saturating_sub(1)..=(end_y + 1).min(height - 1) {
            let start = (y * width + copy_start_x) * 4;
            let end = (y * width + copy_end_x + 1) * 4;
            self.three_d_aa_scratch[start..end].copy_from_slice(&self.pixels[start..end]);
        }

        let source = &self.three_d_aa_scratch;
        let depth = &self.depth;
        let pixels = &mut self.pixels;
        let luma = |pixel_index: usize| -> f32 {
            let offset = pixel_index * 4;
            // Integer-friendly Rec. 709 weights, normalized only when the
            // edge strength is calculated below.
            (source[offset] as f32 * 54.0
                + source[offset + 1] as f32 * 183.0
                + source[offset + 2] as f32 * 19.0)
                / 256.0
        };

        for y in start_y..=end_y {
            for x in start_x..=end_x {
                let center = y * width + x;
                let west = center - 1;
                let east = center + 1;
                let north = center - width;
                let south = center + width;
                let neighbors = [west, east, north, south];

                // Ignore sky/environment-only pixels. At least one sample in
                // the cross must belong to rasterized 3D geometry.
                if !depth[center].is_finite()
                    && neighbors.iter().all(|index| !depth[*index].is_finite())
                {
                    continue;
                }

                let center_luma = luma(center);
                let west_luma = luma(west);
                let east_luma = luma(east);
                let north_luma = luma(north);
                let south_luma = luma(south);
                let minimum_luma = center_luma
                    .min(west_luma)
                    .min(east_luma)
                    .min(north_luma)
                    .min(south_luma);
                let maximum_luma = center_luma
                    .max(west_luma)
                    .max(east_luma)
                    .max(north_luma)
                    .max(south_luma);
                let luma_range = maximum_luma - minimum_luma;

                let center_is_geometry = depth[center].is_finite();
                let depth_edge = neighbors.iter().any(|index| {
                    let neighbor_is_geometry = depth[*index].is_finite();
                    center_is_geometry != neighbor_is_geometry
                        || (center_is_geometry
                            && neighbor_is_geometry
                            && (depth[center] - depth[*index]).abs()
                                > 0.002 * (1.0 + depth[center].abs()))
                });
                let threshold = (maximum_luma * 0.08).max(10.0);
                if !depth_edge && luma_range <= threshold {
                    continue;
                }

                // Blend across the strongest luminance gradient. Standard is
                // intentionally lighter; High also includes the perpendicular
                // pair for a more stable diagonal silhouette.
                let horizontal_gradient = (west_luma - east_luma).abs();
                let vertical_gradient = (north_luma - south_luma).abs();
                let primary = if horizontal_gradient >= vertical_gradient {
                    [west, east]
                } else {
                    [north, south]
                };
                let secondary = if horizontal_gradient >= vertical_gradient {
                    [north, south]
                } else {
                    [west, east]
                };
                let edge_factor = if depth_edge {
                    1.0
                } else {
                    ((luma_range - threshold) / (255.0 - threshold).max(1.0)).clamp(0.2, 1.0)
                };
                let blend_strength = base_strength * edge_factor;
                let destination_offset = center * 4;
                for channel in 0..4 {
                    let primary_average = (source[primary[0] * 4 + channel] as f32
                        + source[primary[1] * 4 + channel] as f32)
                        * 0.5;
                    let target = if high_quality {
                        let secondary_average = (source[secondary[0] * 4 + channel] as f32
                            + source[secondary[1] * 4 + channel] as f32)
                            * 0.5;
                        primary_average * 0.75 + secondary_average * 0.25
                    } else {
                        primary_average
                    };
                    let current = source[destination_offset + channel] as f32;
                    pixels[destination_offset + channel] =
                        (current + (target - current) * blend_strength)
                            .round()
                            .clamp(0.0, 255.0) as u8;
                }
            }
        }
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
            DrawCommand::Mesh3D(_) | DrawCommand::Particles3D(_) => {}
            DrawCommand::Text(request) => self.draw_text(&request)?,
        }
        Ok(())
    }

    fn draw_mesh_3d(
        &mut self,
        command: &crate::render3d::Mesh3DCommand,
        lights: &[crate::render3d::Light3D],
        environment: Option<&crate::render3d::PbrEnvironment>,
        fog: Option<crate::environment3d::Fog3D>,
        ambient_occlusion: Option<crate::environment3d::AmbientOcclusion3D>,
        ambient_occluders: &[crate::render3d::AmbientOccluder3D],
        source_index: usize,
    ) -> Result<(), String> {
        let triangles = crate::render3d::project_mesh(command, lights)?;
        let selected_occluders = if command.receives_shadows {
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
        let texture = command
            .texture
            .as_ref()
            .map(|image| {
                image
                    .snapshot()
                    .map(|snapshot| snapshot.into_parts().2)
                    .map_err(|error| error.to_string())
            })
            .transpose()?;
        let materials = command
            .resolved_materials()?
            .into_iter()
            .map(|material| {
                material
                    .map(|material| {
                        Ok(SoftwarePbrMaterial {
                            base_color: software_image_pixels(
                                material
                                    .base_color_texture
                                    .as_ref()
                                    .and_then(|binding| binding.image.as_ref()),
                            )?,
                            normal: software_image_pixels(
                                material
                                    .normal_texture
                                    .as_ref()
                                    .and_then(|binding| binding.image.as_ref()),
                            )?
                            .map(SoftwarePbrTexture::new)
                            .filter(|texture| !texture.is_flat_normal()),
                            metallic_roughness: software_image_pixels(
                                material
                                    .metallic_roughness_texture
                                    .as_ref()
                                    .and_then(|binding| binding.image.as_ref()),
                            )?
                            .map(SoftwarePbrTexture::new)
                            .filter(|texture| !texture.is_neutral_orm()),
                            emissive: software_image_pixels(
                                material
                                    .emissive_texture
                                    .as_ref()
                                    .and_then(|binding| binding.image.as_ref()),
                            )?
                            .map(SoftwarePbrTexture::new)
                            .filter(|texture| !texture.is_white_emissive()),
                            material,
                        })
                    })
                    .transpose()
            })
            .collect::<Result<Vec<_>, String>>()?;
        let tint = [
            command.tint.r as f32 / 255.0,
            command.tint.g as f32 / 255.0,
            command.tint.b as f32 / 255.0,
            command.tint.a as f32 / 255.0,
        ];
        for triangle in triangles {
            let material = triangle
                .material
                .and_then(|index| materials.get(index))
                .and_then(Option::as_ref);
            let base_texture = texture.as_deref().or_else(|| {
                triangle
                    .material
                    .and_then(|index| materials.get(index))
                    .and_then(Option::as_ref)
                    .and_then(|material| material.base_color.as_deref())
            });
            self.rasterize_projected_triangle(
                &triangle,
                base_texture,
                material,
                tint,
                command.camera_position,
                lights,
                environment,
                fog,
                ambient_occlusion,
                &selected_occluders,
            );
        }
        Ok(())
    }

    fn draw_particles_3d(
        &mut self,
        command: &crate::particles3d::ParticleSystem3DCommand,
        fog: Option<crate::environment3d::Fog3D>,
    ) -> Result<(), String> {
        let triangles = crate::render3d::project_particles(command)?;
        let texture = command
            .texture
            .as_ref()
            .map(|image| {
                image
                    .snapshot()
                    .map(|snapshot| snapshot.into_parts().2)
                    .map_err(|error| error.to_string())
            })
            .transpose()?;
        for triangle in triangles {
            self.rasterize_projected_triangle(
                &triangle,
                texture.as_deref(),
                None,
                [1.0; 4],
                command.camera_position,
                &[],
                None,
                fog,
                None,
                &[],
            );
        }
        Ok(())
    }

    fn rasterize_projected_triangle(
        &mut self,
        triangle: &crate::render3d::ProjectedTriangle,
        texture: Option<&RgbaImage>,
        pbr: Option<&SoftwarePbrMaterial>,
        tint: [f32; 4],
        camera_position: crate::render3d::Vec3,
        lights: &[crate::render3d::Light3D],
        environment: Option<&crate::render3d::PbrEnvironment>,
        fog: Option<crate::environment3d::Fog3D>,
        ambient_occlusion: Option<crate::environment3d::AmbientOcclusion3D>,
        ambient_occluders: &[crate::render3d::AmbientOccluder3D],
    ) {
        let to_screen = |ndc: [f32; 3]| -> [f32; 3] {
            [
                (ndc[0] * 0.5 + 0.5) * self.width as f32,
                (0.5 - ndc[1] * 0.5) * self.height as f32,
                ndc[2],
            ]
        };
        let points = [
            to_screen(triangle.vertices[0].ndc),
            to_screen(triangle.vertices[1].ndc),
            to_screen(triangle.vertices[2].ndc),
        ];
        let edge = |a: [f32; 3], b: [f32; 3], x: f32, y: f32| {
            (x - a[0]) * (b[1] - a[1]) - (y - a[1]) * (b[0] - a[0])
        };
        let area = edge(points[0], points[1], points[2][0], points[2][1]);
        if area.abs() <= f32::EPSILON || !area.is_finite() {
            return;
        }
        let min_x = points
            .iter()
            .map(|point| point[0])
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as u32;
        let max_x = points
            .iter()
            .map(|point| point[0])
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min(self.width.saturating_sub(1) as f32) as u32;
        let min_y = points
            .iter()
            .map(|point| point[1])
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as u32;
        let max_y = points
            .iter()
            .map(|point| point[1])
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min(self.height.saturating_sub(1) as f32) as u32;
        if min_x > max_x || min_y > max_y {
            return;
        }
        self.include_3d_aa_bounds(min_x, min_y, max_x, max_y);

        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let sample_x = x as f32 + 0.5;
                let sample_y = y as f32 + 0.5;
                let w0 = edge(points[1], points[2], sample_x, sample_y) / area;
                let w1 = edge(points[2], points[0], sample_x, sample_y) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < -0.0001 || w1 < -0.0001 || w2 < -0.0001 {
                    continue;
                }
                let depth = points[0][2] * w0 + points[1][2] * w1 + points[2][2] * w2;
                if !(0.0..=1.0).contains(&depth) {
                    continue;
                }
                let pixel_index = y as usize * self.width as usize + x as usize;
                if depth >= self.depth[pixel_index] {
                    continue;
                }

                let Some(attribute_weights) =
                    perspective_correct_weights(&triangle.vertices, [w0, w1, w2])
                else {
                    continue;
                };
                let mut color = [0.0; 4];
                let mut uv = [0.0; 2];
                let mut world_position = [0.0; 3];
                let mut world_normal = [0.0; 3];
                let mut world_tangent = [0.0; 3];
                let mut tangent_sign = 0.0;
                for corner in 0..3 {
                    let weight = attribute_weights[corner];
                    for channel in 0..4 {
                        color[channel] += triangle.vertices[corner].color[channel] * weight;
                    }
                    for axis in 0..2 {
                        uv[axis] += triangle.vertices[corner].uv[axis] * weight;
                    }
                    for axis in 0..3 {
                        world_position[axis] +=
                            triangle.vertices[corner].world_position[axis] * weight;
                        world_normal[axis] += triangle.vertices[corner].world_normal[axis] * weight;
                        world_tangent[axis] +=
                            triangle.vertices[corner].world_tangent[axis] * weight;
                    }
                    tangent_sign += triangle.vertices[corner].tangent_sign * weight;
                }
                // Match Vulkan's gl_FrontFacing behavior for two-sided PBR
                // materials so normal maps illuminate the back face from the
                // physically opposite side.
                if area < 0.0 {
                    for axis in 0..3 {
                        world_normal[axis] = -world_normal[axis];
                        world_tangent[axis] = -world_tangent[axis];
                    }
                }
                let ambient_visibility = ambient_occlusion.map_or(1.0, |settings| {
                    crate::render3d::ambient_occlusion_visibility_3d(
                        settings,
                        crate::render3d::Vec3::new(
                            world_position[0],
                            world_position[1],
                            world_position[2],
                        ),
                        crate::render3d::Vec3::new(
                            world_normal[0],
                            world_normal[1],
                            world_normal[2],
                        ),
                        ambient_occluders,
                    )
                });
                if let Some(pbr) = pbr {
                    let base = texture
                        .map(|texture| sample_software_texture(texture, uv))
                        .unwrap_or([1.0; 4]);
                    let normal = pbr.normal.as_ref().map(|texture| {
                        let sample = texture.sample(uv);
                        [sample[0], sample[1], sample[2]]
                    });
                    let orm = pbr.metallic_roughness.as_ref().map(|texture| {
                        let sample = texture.sample(uv);
                        [sample[1], sample[2]]
                    });
                    let emissive = pbr.emissive.as_ref().map(|texture| {
                        let sample = texture.sample(uv);
                        [sample[0], sample[1], sample[2]]
                    });
                    let Some(shaded) = crate::render3d::shade_pbr_pixel(
                        &pbr.material,
                        tint,
                        base,
                        normal,
                        orm,
                        emissive,
                        world_position,
                        world_normal,
                        world_tangent,
                        tangent_sign,
                        camera_position,
                        lights,
                        environment,
                        ambient_visibility,
                    ) else {
                        continue;
                    };
                    color = shaded;
                } else if let Some(texture) = texture {
                    let texel = sample_software_texture(texture, uv);
                    for channel in 0..4 {
                        color[channel] *= texel[channel];
                    }
                }
                if pbr.is_none() && ambient_visibility < 1.0 {
                    color =
                        crate::render3d::apply_ambient_occlusion_srgb(color, ambient_visibility);
                }
                if let Some(fog) = fog {
                    color = crate::render3d::apply_fog_srgb(
                        color,
                        world_position,
                        camera_position,
                        fog,
                    );
                }
                let source = Color::rgba(
                    (color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
                    (color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
                    (color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
                    (color[3].clamp(0.0, 1.0) * 255.0).round() as u8,
                );
                let offset = pixel_index * 4;
                blend(&mut self.pixels[offset..offset + 4], source);
                if source.a > 0 {
                    self.depth[pixel_index] = depth;
                }
            }
        }
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

    fn test_rect(x: f32, y: f32) -> DrawCommand {
        DrawCommand::Rect {
            x,
            y,
            w: 10.0,
            h: 10.0,
            rotation: 0.0,
            offset: Vec2::default(),
            color: Color::WHITE,
            shader: None,
        }
    }

    #[test]
    fn software_environment_cache_reuses_static_panorama_and_invalidates_for_camera() {
        let panorama = RgbaImage::from_fn(8, 4, |x, y| Rgba([x as u8 * 20, y as u8 * 30, 90, 255]));
        let environment = crate::environment3d::Environment3D {
            enabled: true,
            mode: crate::environment3d::EnvironmentMode3D::Equirectangular,
            equirectangular: Some(ImageHandle::from_rgba_image(panorama)),
            ..crate::environment3d::Environment3D::default()
        };
        let mut renderer = SoftwareRenderer::new(16, 8);
        let camera = crate::render3d::Camera3D::default();
        renderer.draw_environment_background(&environment, camera, Color::rgba(0, 0, 0, 255));
        let first = renderer.pixels.clone();
        let first_key = renderer.environment_cache_key.clone();
        renderer.pixels.fill(0);
        renderer.draw_environment_background(&environment, camera, Color::rgba(0, 0, 0, 255));
        assert_eq!(renderer.pixels, first);
        assert_eq!(renderer.environment_cache_key, first_key);

        let mut rotated = camera;
        rotated.euler.y = 45.0;
        renderer.draw_environment_background(&environment, rotated, Color::rgba(0, 0, 0, 255));
        assert_ne!(renderer.environment_cache_key, first_key);

        renderer.draw_environment_background(
            &crate::environment3d::Environment3D::default(),
            rotated,
            Color::rgba(1, 2, 3, 255),
        );
        assert!(renderer.environment_cache_key.is_none());
        assert!(
            renderer.environment_cache_pixels.is_empty(),
            "ordinary frames must not retain a second full-size framebuffer"
        );
    }

    #[test]
    fn camera_translates_scene_commands_but_not_overlays_and_falls_back_to_origin() {
        let mut state = RenderState::default();
        state.begin_camera_frame();
        state.submit_camera(41, Vec2 { x: 100.0, y: -25.0 });
        state.finish_camera_frame();
        assert!(state.is_camera_active(41));
        state.queue(test_rect(5.0, 9.0));
        state.extend_overlay(vec![test_rect(7.0, 11.0)]);

        let commands = state.drain_without_remembering();
        let DrawCommand::Rect { x, y, .. } = commands[0] else {
            panic!("scene rect missing");
        };
        assert_eq!((x, y), (105.0, -16.0));
        let DrawCommand::Rect { x, y, .. } = commands[1] else {
            panic!("overlay rect missing");
        };
        assert_eq!((x, y), (7.0, 11.0));

        state.queue_light(crate::lighting::Light {
            x: 5.0,
            y: 9.0,
            ..crate::lighting::Light::default()
        });
        state.queue_occluder(crate::lighting::Occluder {
            cx: 7.0,
            cy: 11.0,
            half_w: 2.0,
            half_h: 3.0,
            rotation: 0.0,
            shape: crate::lighting::OccluderShape::Box,
        });
        let (_, lights, occluders) = state.take_lighting();
        assert_eq!((lights[0].x, lights[0].y), (105.0, -16.0));
        assert_eq!((occluders[0].cx, occluders[0].cy), (107.0, -14.0));

        state.begin_camera_frame();
        state.finish_camera_frame();
        state.queue(test_rect(5.0, 9.0));
        let commands = state.drain_without_remembering();
        let DrawCommand::Rect { x, y, .. } = commands[0] else {
            panic!("fallback rect missing");
        };
        assert_eq!((x, y), (5.0, 9.0));
    }

    #[test]
    fn explicitly_activated_camera_wins_over_automatic_fallback() {
        let mut state = RenderState::default();
        state.activate_camera(2, Vec2 { x: 1.0, y: 2.0 });
        state.begin_camera_frame();
        state.submit_camera(1, Vec2 { x: 10.0, y: 20.0 });
        state.submit_camera(2, Vec2 { x: 30.0, y: 40.0 });
        state.finish_camera_frame();
        assert!(state.is_camera_active(2));
        assert_eq!(state.camera_offset().x, 30.0);
        assert_eq!(state.camera_offset().y, 40.0);
    }

    #[test]
    fn same_size_resize_preserves_cached_light_map() {
        let mut renderer = SoftwareRenderer::new(64, 48);
        let config = crate::lighting::LightConfig {
            enabled: true,
            ambient_intensity: 0.25,
            ..crate::lighting::LightConfig::default()
        };
        let (_, first) = crate::lighting::render_light_map_cached(
            64,
            48,
            &config,
            &[],
            &[],
            &mut renderer.light_cache,
        )
        .expect("enabled non-neutral lighting should produce a light map");

        renderer.resize(64, 48);

        let (_, second) = crate::lighting::render_light_map_cached(
            64,
            48,
            &config,
            &[],
            &[],
            &mut renderer.light_cache,
        )
        .expect("same-size resize should retain the cached light map");
        assert!(
            std::sync::Arc::ptr_eq(&first, &second),
            "same-size resize rebuilt an unchanged light map"
        );
    }

    #[test]
    fn changed_size_resize_updates_pixels_and_light_map() {
        let mut renderer = SoftwareRenderer::new(64, 48);
        let config = crate::lighting::LightConfig {
            enabled: true,
            ambient_intensity: 0.25,
            ..crate::lighting::LightConfig::default()
        };
        let (_, first) = crate::lighting::render_light_map_cached(
            64,
            48,
            &config,
            &[],
            &[],
            &mut renderer.light_cache,
        )
        .expect("enabled non-neutral lighting should produce a light map");

        renderer.resize(32, 24);

        assert_eq!(renderer.dimensions(), (32, 24));
        assert_eq!(renderer.pixels().len(), 32 * 24 * 4);
        let (_, resized) = crate::lighting::render_light_map_cached(
            32,
            24,
            &config,
            &[],
            &[],
            &mut renderer.light_cache,
        )
        .expect("resized renderer should build a light map for its new dimensions");
        assert!(!std::sync::Arc::ptr_eq(&first, &resized));
        assert_eq!((resized.width, resized.height), (16, 12));
    }

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

    #[test]
    fn software_mesh_attributes_use_perspective_correct_weights() {
        let vertex = |clip_w| crate::render3d::ProjectedVertex {
            clip_position: [0.0, 0.0, 0.0, clip_w],
            ndc: [0.0; 3],
            uv: [0.0; 2],
            color: [1.0; 4],
            world_position: [0.0; 3],
            world_normal: [0.0, 0.0, 1.0],
            world_tangent: [1.0, 0.0, 0.0],
            tangent_sign: 1.0,
        };
        let vertices = [vertex(1.0), vertex(2.0), vertex(4.0)];
        let weights = perspective_correct_weights(&vertices, [0.25, 0.25, 0.5])
            .expect("valid perspective weights");
        assert!((weights[0] - 0.5).abs() < 1e-6);
        assert!((weights[1] - 0.25).abs() < 1e-6);
        assert!((weights[2] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn software_renderer_draws_projected_mesh_with_depth() {
        use crate::mesh::{MeshData, MeshHandle, Submesh, Vertex};

        let mesh = MeshHandle::new(
            MeshData::new(
                "triangle",
                vec![
                    Vertex::from_position([-0.8, -0.8, 0.0]),
                    Vertex::from_position([0.8, -0.8, 0.0]),
                    Vertex::from_position([0.0, 0.8, 0.0]),
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
            .expect("mesh data"),
        )
        .expect("mesh handle");
        let camera = crate::render3d::Camera3D::default();
        let command = |z, tint| {
            DrawCommand::Mesh3D(crate::render3d::Mesh3DCommand {
                mesh: mesh.clone(),
                model: crate::render3d::Mat4::translation(crate::render3d::Vec3::new(0.0, 0.0, z)),
                view_projection: camera.view_projection(1.0),
                camera_position: camera.position,
                tint,
                texture: None,
                materials: Vec::new(),
                shader: None,
                double_sided: true,
                casts_shadows: true,
                receives_shadows: true,
            })
        };
        let near = command(2.0, Color::rgba(255, 0, 0, 255));
        let far = command(-2.0, Color::rgba(0, 255, 0, 255));
        let platform = crate::platform::new_shared_platform_state();
        lock_platform_state(&platform).set_clear_color(Color::rgba(0, 0, 0, 255));
        let mut renderer = SoftwareRenderer::new(64, 64);
        renderer
            .render_commands(&platform, &[near, far])
            .expect("render overlapping meshes");
        let center = (32usize * 64 + 32) * 4;
        assert!(
            renderer.pixels()[center] > renderer.pixels()[center + 1],
            "far mesh overwrote a nearer mesh: {:?}",
            &renderer.pixels()[center..center + 4]
        );
        assert!(renderer.depth[32 * 64 + 32].is_finite());
    }

    #[test]
    fn software_3d_antialiasing_respects_global_quality_without_touching_2d_overlays() {
        use crate::mesh::{MeshData, MeshHandle, Submesh, Vertex};

        let mesh = MeshHandle::new(
            MeshData::new(
                "antialiased triangle",
                vec![
                    Vertex::from_position([-0.73, -0.67, 0.0]),
                    Vertex::from_position([0.78, -0.61, 0.0]),
                    Vertex::from_position([-0.08, 0.81, 0.0]),
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
            .expect("mesh data"),
        )
        .expect("mesh handle");
        let camera = crate::render3d::Camera3D::default();
        let mesh_command = DrawCommand::Mesh3D(crate::render3d::Mesh3DCommand {
            mesh,
            model: crate::render3d::Mat4::translation(crate::render3d::Vec3::new(0.0, 0.0, 2.0)),
            view_projection: camera.view_projection(1.0),
            camera_position: camera.position,
            tint: Color::WHITE,
            texture: None,
            materials: Vec::new(),
            shader: None,
            double_sided: true,
            casts_shadows: true,
            receives_shadows: true,
        });
        let platform = crate::platform::new_shared_platform_state();
        {
            let mut state = lock_platform_state(&platform);
            state.set_clear_color(Color::rgba(0, 0, 0, 0));
            state.set_antialiasing(Antialiasing::Off);
        }
        let mut renderer = SoftwareRenderer::new(64, 64);
        renderer
            .render_commands(&platform, std::slice::from_ref(&mesh_command))
            .expect("hard 3D triangle");
        assert!(
            renderer
                .pixels()
                .chunks_exact(4)
                .all(|pixel| matches!(pixel[3], 0 | 255)),
            "off mode must retain center-sampled 3D coverage"
        );
        assert!(renderer.three_d_aa_scratch.is_empty());

        lock_platform_state(&platform).set_antialiasing(Antialiasing::High);
        let overlay = DrawCommand::Rect {
            x: 0.0,
            y: 0.0,
            w: 8.0,
            h: 64.0,
            rotation: 0.0,
            offset: Vec2::default(),
            color: Color::rgba(20, 40, 60, 255),
            shader: None,
        };
        renderer
            .render_commands(&platform, &[mesh_command, overlay])
            .expect("smoothed 3D triangle with 2D overlay");
        assert!(
            renderer
                .pixels()
                .chunks_exact(4)
                .any(|pixel| pixel[3] > 0 && pixel[3] < 255),
            "high mode must generate partial 3D edge coverage"
        );
        for y in 0..64usize {
            let offset = (y * 64 + 4) * 4;
            assert_eq!(
                &renderer.pixels()[offset..offset + 4],
                &[20, 40, 60, 255],
                "the 2D overlay must be drawn after the 3D AA pass"
            );
        }
    }

    #[test]
    fn software_renderer_does_not_allocate_depth_for_2d_frames() {
        let platform = crate::platform::new_shared_platform_state();
        lock_platform_state(&platform).set_clear_color(Color::rgba(1, 2, 3, 255));
        let mut renderer = SoftwareRenderer::new(640, 360);
        assert!(renderer.depth.is_empty());
        assert_eq!(renderer.depth.capacity(), 0);
        assert!(renderer.three_d_aa_scratch.is_empty());
        assert_eq!(renderer.three_d_aa_scratch.capacity(), 0);

        renderer
            .render_commands(
                &platform,
                &[DrawCommand::Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 32.0,
                    h: 32.0,
                    rotation: 0.0,
                    offset: Vec2::default(),
                    color: Color::WHITE,
                    shader: None,
                }],
            )
            .expect("render 2D frame");
        assert!(renderer.depth.is_empty());
        assert_eq!(renderer.depth.capacity(), 0);
        assert!(renderer.three_d_aa_scratch.is_empty());
        assert_eq!(renderer.three_d_aa_scratch.capacity(), 0);

        renderer.resize(800, 450);
        assert!(renderer.depth.is_empty());
        assert_eq!(renderer.depth.capacity(), 0);
    }

    #[test]
    fn software_textured_mesh_observes_image_edits_on_the_next_draw() {
        use crate::mesh::{MeshData, MeshHandle, Submesh, Vertex};

        let mesh = MeshHandle::new(
            MeshData::new(
                "textured triangle",
                vec![
                    Vertex::from_position([-0.8, -0.8, 0.0]),
                    Vertex::from_position([0.8, -0.8, 0.0]),
                    Vertex::from_position([0.0, 0.8, 0.0]),
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
            .expect("mesh data"),
        )
        .expect("mesh handle");
        let texture =
            ImageHandle::from_rgba_image(RgbaImage::from_pixel(1, 1, Rgba([255, 0, 0, 255])));
        let camera = crate::render3d::Camera3D::default();
        let command = DrawCommand::Mesh3D(crate::render3d::Mesh3DCommand {
            mesh,
            model: crate::render3d::Mat4::translation(crate::render3d::Vec3::new(0.0, 0.0, 2.0)),
            view_projection: camera.view_projection(1.0),
            camera_position: camera.position,
            tint: Color::WHITE,
            texture: Some(texture.clone()),
            materials: Vec::new(),
            shader: None,
            double_sided: true,
            casts_shadows: true,
            receives_shadows: true,
        });
        let platform = crate::platform::new_shared_platform_state();
        lock_platform_state(&platform).set_clear_color(Color::rgba(0, 0, 0, 255));
        let mut renderer = SoftwareRenderer::new(64, 64);
        renderer
            .render_commands(&platform, std::slice::from_ref(&command))
            .expect("render red texture");
        let center = (32usize * 64 + 32) * 4;
        assert!(renderer.pixels()[center] > renderer.pixels()[center + 1]);

        texture
            .replace_rgba_image(RgbaImage::from_pixel(1, 1, Rgba([0, 255, 0, 255])))
            .expect("edit live texture");
        renderer
            .render_commands(&platform, &[command])
            .expect("render edited green texture");
        assert!(renderer.pixels()[center + 1] > renderer.pixels()[center]);
    }

    #[test]
    fn software_mesh_uses_imported_base_color_texture_per_submesh() {
        use crate::mesh::{MeshData, MeshHandle, MeshMaterial, Submesh, TextureBinding, Vertex};

        let red = ImageHandle::from_rgba_image(RgbaImage::from_pixel(1, 1, Rgba([255, 0, 0, 255])));
        let green =
            ImageHandle::from_rgba_image(RgbaImage::from_pixel(1, 1, Rgba([0, 255, 0, 255])));
        let material = |name: &str, image: ImageHandle| {
            let mut material = MeshMaterial::named(name);
            material.base_color_texture = Some(TextureBinding {
                source: format!("{name}.png"),
                tex_coord: 0,
                image: Some(image),
            });
            material
        };
        let mesh = MeshHandle::new(
            MeshData::new(
                "two materials",
                vec![
                    Vertex::from_position([-0.9, -0.7, 0.0]),
                    Vertex::from_position([-0.1, -0.7, 0.0]),
                    Vertex::from_position([-0.5, 0.7, 0.0]),
                    Vertex::from_position([0.1, -0.7, 0.0]),
                    Vertex::from_position([0.9, -0.7, 0.0]),
                    Vertex::from_position([0.5, 0.7, 0.0]),
                ],
                vec![0, 1, 2, 3, 4, 5],
                vec![
                    Submesh {
                        name: "left".into(),
                        first_index: 0,
                        index_count: 3,
                        material: Some(0),
                    },
                    Submesh {
                        name: "right".into(),
                        first_index: 3,
                        index_count: 3,
                        material: Some(1),
                    },
                ],
                vec![material("red", red), material("green", green)],
                true,
            )
            .expect("mesh data"),
        )
        .expect("mesh handle");
        let camera = crate::render3d::Camera3D::default();
        let command = DrawCommand::Mesh3D(crate::render3d::Mesh3DCommand {
            mesh,
            model: crate::render3d::Mat4::translation(crate::render3d::Vec3::new(0.0, 0.0, 2.0)),
            view_projection: camera.view_projection(1.0),
            camera_position: camera.position,
            tint: Color::WHITE,
            texture: None,
            materials: Vec::new(),
            shader: None,
            double_sided: true,
            casts_shadows: true,
            receives_shadows: true,
        });
        let platform = crate::platform::new_shared_platform_state();
        lock_platform_state(&platform).set_clear_color(Color::rgba(0, 0, 0, 255));
        let mut renderer = SoftwareRenderer::new(64, 64);
        renderer
            .render_commands(&platform, &[command])
            .expect("render imported material textures");

        let mut left_red = 0usize;
        let mut right_green = 0usize;
        for (index, pixel) in renderer.pixels().chunks_exact(4).enumerate() {
            let x = index % 64;
            if x < 32 && pixel[0] > pixel[1] {
                left_red += 1;
            }
            if x >= 32 && pixel[1] > pixel[0] {
                right_green += 1;
            }
        }
        assert!(left_red > 20, "left material did not render its red image");
        assert!(
            right_green > 20,
            "right material did not render its green image"
        );
    }

    #[test]
    fn software_mesh_observes_reusable_material_texture_edits() {
        use crate::mesh::{
            MaterialHandle, MeshData, MeshHandle, MeshMaterial, Submesh, TextureBinding, Vertex,
        };

        let red = ImageHandle::from_rgba_image(RgbaImage::from_pixel(1, 1, Rgba([255, 0, 0, 255])));
        let green =
            ImageHandle::from_rgba_image(RgbaImage::from_pixel(1, 1, Rgba([0, 255, 0, 255])));
        let mut initial = MeshMaterial::named("live override");
        initial.base_color_texture = Some(TextureBinding {
            source: "red.png".into(),
            tex_coord: 0,
            image: Some(red),
        });
        let material = MaterialHandle::new(initial).expect("material handle");
        let mesh = MeshHandle::new(
            MeshData::new(
                "material override triangle",
                vec![
                    Vertex::from_position([-0.8, -0.8, 0.0]),
                    Vertex::from_position([0.8, -0.8, 0.0]),
                    Vertex::from_position([0.0, 0.8, 0.0]),
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
            .expect("mesh data"),
        )
        .expect("mesh handle");
        let camera = crate::render3d::Camera3D::default();
        let command = DrawCommand::Mesh3D(crate::render3d::Mesh3DCommand {
            mesh,
            model: crate::render3d::Mat4::translation(crate::render3d::Vec3::new(0.0, 0.0, 2.0)),
            view_projection: camera.view_projection(1.0),
            camera_position: camera.position,
            tint: Color::WHITE,
            texture: None,
            materials: vec![Some(material.clone())],
            shader: None,
            double_sided: true,
            casts_shadows: true,
            receives_shadows: true,
        });
        let platform = crate::platform::new_shared_platform_state();
        lock_platform_state(&platform).set_clear_color(Color::rgba(0, 0, 0, 255));
        let mut renderer = SoftwareRenderer::new(64, 64);
        let center = (32usize * 64 + 32) * 4;

        renderer
            .render_commands(&platform, std::slice::from_ref(&command))
            .expect("render red material");
        assert!(renderer.pixels()[center] > renderer.pixels()[center + 1]);

        material
            .mutate(move |material| {
                material.base_color_texture = Some(TextureBinding {
                    source: "green.png".into(),
                    tex_coord: 0,
                    image: Some(green),
                });
                Ok(())
            })
            .expect("edit reusable material");
        renderer
            .render_commands(&platform, &[command])
            .expect("render edited green material");
        assert!(renderer.pixels()[center + 1] > renderer.pixels()[center]);
    }
}
