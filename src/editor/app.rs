//! Editor application state and per-frame UI layout.
//!
//! [`EditorApp`] owns the scene and the editor configuration (theme + dock
//! layout). It renders a dockable Hierarchy / Inspector, a pannable 2D
//! viewport, a bottom Project browser, and a toolbar, plus an overlay layer for
//! context menus, dropdowns, the color picker and modal dialogs.

use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Instant, SystemTime};

use rodio::Source;
use serde::{Deserialize, Serialize};

use crate::platform::Color;
use crate::post_process::{
    BloomConfig, BrightnessContrastSaturationConfig, ChromaticAberrationConfig,
    Effect as PostProcessEffect, EffectPass as PostProcessEffectPass, ExposureTonemapConfig,
    GrayscaleConfig, InvertConfig, MotionBlurConfig, PixelateConfig, QuantizationConfig,
    TonemapOperator, VignetteConfig,
};
use crate::render3d::{
    Camera3D as RenderCamera3D, Light3D as RenderLight3D, LightKind3D, Mat4, Mesh3DCommand,
    Projection3D, Vec3, project_mesh,
};
use crate::renderer::{
    self, FontHandle, Rect as RenderRect, TextAlignX, TextAlignY, TextAntialiasing,
    TextRenderRequest, TextScaleMode, TextWrapMode, Vec2 as RenderVec2,
};
use crate::scene::{
    ADVANCED_COMPONENTS, AttachedValue, CORE_COMPONENTS, ColorKeypoint, Component,
    ComponentReference, DictionaryEntry, Entity, NumberKeypoint, Prop, PropValue, Scene, SceneKind,
    ScriptVar, VarControl, VarKey, VarValue, load_prefab as load_prefab_file,
    save_prefab as save_prefab_file,
};
use crate::update::AvailableUpdate;

use super::inspector::{parse_inspector_variables, script_registers_component_picker};
use super::ui::{Painter, Rect, Rgba, Theme, Ui, icon};

const TOOLBAR_H: f32 = 40.0;
const STATUS_H: f32 = 24.0;
const HEADER_H: f32 = 26.0;
const ROW_H: f32 = 24.0;
const FIELD_H: f32 = 22.0;
const PAD: f32 = 10.0;
const LABEL_W: f32 = 84.0;
const MIN_PANEL_W: f32 = 150.0;
const MIN_VIEWPORT_W: f32 = 160.0;
const SPLIT_HALF: f32 = 4.0;
const PREVIEW_ROOT_WIDTH: f32 = 1280.0;
const PREVIEW_ROOT_HEIGHT: f32 = 720.0;
const WAVEFORM_PREVIEW_BUCKETS: usize = 192;
const VIEWPORT_MESH_CACHE_LIMIT: usize = 64;
/// Enough samples for smooth projected rotation rings without making editor
/// chrome expensive in scenes that redraw continuously.
const ROTATION_RING_SAMPLES: usize = 32;
/// Screen-space length of the rotation gizmo's stalk above the entity.
const ROT_HANDLE_DIST: f32 = 28.0;

/// Move gizmo axis colors: X (horizontal) red, Y (vertical) green, matching the
/// convention used by Unity/Godot so the axes read at a glance.
const MOVE_X_COLOR: Rgba = [231, 76, 76, 255];
const MOVE_Y_COLOR: Rgba = [122, 204, 106, 255];
/// Scale gizmo corner-handle color (blue), distinct from the move axes.
const SCALE_HANDLE_COLOR: Rgba = [86, 156, 214, 255];

/// Human-facing names in the same stable order used by the Inspector's kind
/// cycle button. Replacing a kind intentionally resets it to runtime defaults.
fn post_process_effect_label(effect: &PostProcessEffect) -> &'static str {
    match effect {
        PostProcessEffect::Bloom(_) => "Bloom",
        PostProcessEffect::Pixelate(_) => "Pixelate",
        PostProcessEffect::ChromaticAberration(_) => "Chromatic Aberration",
        PostProcessEffect::MotionBlur(_) => "Motion Blur",
        PostProcessEffect::Quantization(_) => "Quantization",
        PostProcessEffect::Vignette(_) => "Vignette",
        PostProcessEffect::Grayscale(_) => "Grayscale",
        PostProcessEffect::Invert(_) => "Invert",
        PostProcessEffect::BrightnessContrastSaturation(_) => "Color Adjustment",
        PostProcessEffect::ExposureTonemap(_) => "Exposure / Tonemap",
    }
}

fn default_post_process_effect(index: usize) -> PostProcessEffect {
    match index % 10 {
        0 => PostProcessEffect::Bloom(BloomConfig::default()),
        1 => PostProcessEffect::Pixelate(PixelateConfig::default()),
        2 => PostProcessEffect::ChromaticAberration(ChromaticAberrationConfig::default()),
        3 => PostProcessEffect::MotionBlur(MotionBlurConfig::default()),
        4 => PostProcessEffect::Quantization(QuantizationConfig::default()),
        5 => PostProcessEffect::Vignette(VignetteConfig::default()),
        6 => PostProcessEffect::Grayscale(GrayscaleConfig::default()),
        7 => PostProcessEffect::Invert(InvertConfig::default()),
        8 => PostProcessEffect::BrightnessContrastSaturation(
            BrightnessContrastSaturationConfig::default(),
        ),
        _ => PostProcessEffect::ExposureTonemap(ExposureTonemapConfig::default()),
    }
}

fn next_post_process_effect(effect: &PostProcessEffect) -> PostProcessEffect {
    let index = match effect {
        PostProcessEffect::Bloom(_) => 1,
        PostProcessEffect::Pixelate(_) => 2,
        PostProcessEffect::ChromaticAberration(_) => 3,
        PostProcessEffect::MotionBlur(_) => 4,
        PostProcessEffect::Quantization(_) => 5,
        PostProcessEffect::Vignette(_) => 6,
        PostProcessEffect::Grayscale(_) => 7,
        PostProcessEffect::Invert(_) => 8,
        PostProcessEffect::BrightnessContrastSaturation(_) => 9,
        PostProcessEffect::ExposureTonemap(_) => 0,
    };
    default_post_process_effect(index)
}

/// Rotate the screen point `(px, py)` by `angle` radians about `(cx, cy)`.
fn rotate_point_about(px: f32, py: f32, cx: f32, cy: f32, angle: f32) -> (f32, f32) {
    let (sin, cos) = (angle.sin(), angle.cos());
    let dx = px - cx;
    let dy = py - cy;
    (cx + dx * cos - dy * sin, cy + dx * sin + dy * cos)
}

fn rotate_vector(x: f32, y: f32, angle: f32) -> (f32, f32) {
    let (sin, cos) = (angle.sin(), angle.cos());
    (x * cos - y * sin, x * sin + y * cos)
}

fn normalized_position_pivot_name(value: &str) -> &'static str {
    let name = value
        .trim()
        .to_ascii_lowercase()
        .replace(' ', "_")
        .replace('-', "_");
    match name.as_str() {
        "center" | "middle" => "center",
        "top_right" | "topright" => "top_right",
        _ => "top_left",
    }
}

fn normalized_rotation_pivot_name(value: &str) -> &'static str {
    let name = value
        .trim()
        .to_ascii_lowercase()
        .replace(' ', "_")
        .replace('-', "_");
    match name.as_str() {
        "center" | "middle" => "center",
        _ => "top_left",
    }
}

fn pivot_storage_value(key: &str) -> String {
    if key == "top_left" {
        String::new()
    } else {
        key.to_string()
    }
}

fn position_pivot_fraction_from_name(value: &str) -> (f32, f32) {
    match normalized_position_pivot_name(value) {
        "center" => (0.5, 0.5),
        "top_right" => (1.0, 0.0),
        _ => (0.0, 0.0),
    }
}

fn rotation_pivot_fraction_from_name(value: &str) -> (f32, f32) {
    match normalized_rotation_pivot_name(value) {
        "center" => (0.5, 0.5),
        _ => (0.0, 0.0),
    }
}

fn reset_entity_rotation(entity: &mut Entity, kind: SceneKind) {
    match kind {
        SceneKind::TwoD => entity.rotation = 0.0,
        SceneKind::ThreeD => {
            entity.rotation_x = 0.0;
            entity.rotation_y = 0.0;
            entity.rotation_z = 0.0;
        }
    }
}

fn reset_entity_scale(entity: &mut Entity, kind: SceneKind) {
    match kind {
        SceneKind::TwoD => entity.scale = 1.0,
        SceneKind::ThreeD => {
            entity.scale_x = 1.0;
            entity.scale_y = 1.0;
            entity.scale_z = 1.0;
        }
    }
}

fn reset_entity_transform(entity: &mut Entity, kind: SceneKind) {
    entity.x = 0.0;
    entity.y = 0.0;
    match kind {
        SceneKind::TwoD => {
            entity.z = 0.0;
            entity.anchor_x = 0.0;
            entity.anchor_y = 0.0;
        }
        SceneKind::ThreeD => entity.position_z = 0.0,
    }
    reset_entity_rotation(entity, kind);
    reset_entity_scale(entity, kind);
}

fn short_revision(revision: &str) -> &str {
    revision.get(..revision.len().min(8)).unwrap_or(revision)
}

fn rect_from_points(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    Rect::new(x0.min(x1), y0.min(y1), (x1 - x0).abs(), (y1 - y0).abs())
}

fn rects_intersect(a: Rect, b: Rect) -> bool {
    a.x < b.right() && a.right() > b.x && a.y < b.bottom() && a.bottom() > b.y
}

fn rotated_rect_bounds(rect: Rect, pivot_x: f32, pivot_y: f32, angle: f32) -> Rect {
    if angle.abs() < 1e-4 {
        return rect;
    }
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    for (x, y) in [
        (rect.x, rect.y),
        (rect.right(), rect.y),
        (rect.right(), rect.bottom()),
        (rect.x, rect.bottom()),
    ] {
        let (rx, ry) = rotate_point_about(x, y, pivot_x, pivot_y, angle);
        min_x = min_x.min(rx);
        min_y = min_y.min(ry);
        max_x = max_x.max(rx);
        max_y = max_y.max(ry);
    }
    Rect::new(min_x, min_y, max_x - min_x, max_y - min_y)
}

/// Which side of the window a dockable panel lives on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    fn toggled(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Panel {
    Hierarchy,
    Inspector,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditorWidget {
    Hierarchy,
    Inspector,
    Project,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Splitter {
    LeftWidth,
    RightWidth,
    LeftSplit,
    RightSplit,
    BinHeight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewTool {
    Move,
    Scale,
    Rotate,
    /// Combined gizmo: scale corners + rotate knob at once. The body stays
    /// draggable to move, but the dedicated move handle is not shown.
    Transform,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DocumentKind {
    Scene,
    Prefab,
}

#[derive(Clone, Debug)]
struct OpenDocument {
    path: PathBuf,
    scene: Scene,
    kind: DocumentKind,
    dirty: bool,
}

/// Persisted dock layout, grid and snapping preferences.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub left_w: f32,
    pub right_w: f32,
    pub hierarchy_side: Side,
    pub inspector_side: Side,
    pub left_split: f32,
    pub right_split: f32,
    pub snap: bool,
    pub grid: f32,
    pub show_grid: bool,
    pub bin_h: f32,
    /// Use the HSV square/hue-strip color picker instead of plain RGBA sliders.
    pub hsv_picker: bool,
    /// Whether the bottom Project browser is visible.
    pub show_project: bool,
    /// Whether the Hierarchy panel is visible.
    pub show_hierarchy: bool,
    /// Whether the Inspector panel is visible.
    pub show_inspector: bool,
    /// Whether panels are rendered in separate editor windows.
    pub undock_hierarchy: bool,
    pub undock_inspector: bool,
    pub undock_project: bool,
    /// Active Scene view transform tool.
    pub view_tool: ViewTool,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            left_w: 240.0,
            right_w: 330.0,
            hierarchy_side: Side::Left,
            inspector_side: Side::Right,
            left_split: 0.5,
            right_split: 0.5,
            snap: true,
            grid: 32.0,
            show_grid: true,
            bin_h: 170.0,
            hsv_picker: true,
            show_project: true,
            show_hierarchy: true,
            show_inspector: true,
            undock_hierarchy: false,
            undock_inspector: false,
            undock_project: false,
            view_tool: ViewTool::Move,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum AlignKind {
    Left,
    CenterX,
    Right,
    Top,
    CenterY,
    Bottom,
}

#[derive(Clone, Copy, Debug)]
enum ZMove {
    Front,
    Back,
    Forward,
    Backward,
}

type ScriptSchemaCache = HashMap<String, (Option<SystemTime>, Result<Vec<ScriptVar>, String>)>;

struct EnumPropMenuTarget {
    entity: u64,
    component: usize,
    prop: usize,
    options: Vec<String>,
    current: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorConfig {
    pub theme: Theme,
    /// The user's editable palette is retained even while a named preset is
    /// active, so switching themes never destroys custom work.
    pub custom_theme: Theme,
    pub layout: Layout,
    pub settings: EditorSettings,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorSettings {
    pub theme_name: String,
    pub font_path: String,
    pub show_tooltips: bool,
    pub show_window_bounds: bool,
    pub show_transform_hud: bool,
    pub preview_lighting: bool,
    pub autosave_before_run: bool,
    pub autosave_before_build: bool,
    /// Mouse/trackpad look and pan multiplier for the scene viewport.
    pub viewport_camera_sensitivity: f32,
    /// Free-fly camera movement speed in world units per second. The 2D
    /// viewport persists it now so switching to 3D keeps one preference.
    pub viewport_camera_speed: f32,
    /// Perspective field of view, in vertical degrees, for 3D scene views.
    pub viewport_camera_fov: f32,
    /// Reverse vertical RMB mouse-look without changing horizontal yaw.
    pub viewport_invert_mouse_look: bool,
    pub mobile_emulator: bool,
    pub mobile_orientation: String,
    pub mobile_wifi: bool,
    pub mobile_cellular: bool,
    pub mobile_low_power: bool,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            theme_name: "dark_plus".to_string(),
            font_path: String::new(),
            show_tooltips: true,
            show_window_bounds: true,
            show_transform_hud: true,
            preview_lighting: true,
            autosave_before_run: true,
            autosave_before_build: true,
            viewport_camera_sensitivity: 1.0,
            viewport_camera_speed: 10.0,
            viewport_camera_fov: 60.0,
            viewport_invert_mouse_look: false,
            mobile_emulator: false,
            mobile_orientation: "portrait".to_string(),
            mobile_wifi: true,
            mobile_cellular: false,
            mobile_low_power: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BuildTarget {
    Desktop,
    WindowsDesktop,
    LinuxDesktop,
    Webasm,
    Android,
    Ios,
}

impl BuildTarget {
    fn label(self) -> &'static str {
        match self {
            Self::Desktop => "Desktop",
            Self::WindowsDesktop => "Windows Desktop",
            Self::LinuxDesktop => "Linux Desktop",
            Self::Webasm => "WebAssembly",
            Self::Android => "Android APK",
            Self::Ios => "iOS Simulator",
        }
    }

    fn cli_arg(self) -> Option<&'static str> {
        match self {
            Self::Desktop => None,
            Self::WindowsDesktop => Some("--windows"),
            Self::LinuxDesktop => Some("--linux"),
            Self::Webasm => Some("--webasm"),
            Self::Android => Some("--android"),
            Self::Ios => Some("--ios"),
        }
    }
}

/// A target a color picker writes back to.
#[derive(Clone, Debug)]
enum ColorTarget {
    Background,
    LightingAmbient,
    Prop {
        entity: u64,
        comp: usize,
        prop: usize,
    },
    Var {
        entity: u64,
        comp: usize,
        var: usize,
        path: Vec<VarPathPart>,
    },
    AttachedValue {
        entity: u64,
        value: usize,
        path: Vec<VarPathPart>,
    },
}

#[derive(Clone, Debug)]
enum VarPathPart {
    List(usize),
    Dictionary(usize),
}

/// An action a menu item or dialog performs.
#[derive(Clone, Debug)]
enum Action {
    NewScene,
    SaveScene,
    LoadScene,
    ExportScene,
    RunScene,
    AddComponent(u64, String),
    /// Add a user-authored behaviour script component by project-relative path.
    AddScriptComponent(u64, String),
    PasteComponent(u64),
    AddEntity(Option<u64>),
    /// Add an entity at a specific world position (viewport context menu).
    AddEntityAt(f32, f32),
    Rename(u64),
    Duplicate(u64),
    Copy(u64),
    Delete(u64),
    Paste,
    Unparent(u64),
    ResetTransform(u64),
    FrameSelected(u64),
    ToggleActive(u64),
    NewFolder,
    NewScript,
    NewShader,
    NewAnimation,
    RevealInExplorer,
    OpenProjectInVscode,
    OpenPath(PathBuf),
    OpenAnimation(PathBuf),
    OpenScene(PathBuf),
    EnterFolder(PathBuf),
    OpenSelectionTools(f32, f32),
    OpenHierarchyTools(f32, f32),
    OpenArrangeTools(f32, f32),
    OpenViewTools(f32, f32),
    OpenEditorSettings,
    OpenProjectWindowSettings,
    OpenMobileEmulator,
    BuildProject,
    ToggleHierarchy,
    ToggleInspector,
    ToggleHierarchyUndocked,
    ToggleInspectorUndocked,
    ToggleProjectUndocked,
    SetSceneAntialiasing(String),
    SetPropEnum {
        entity: u64,
        component: usize,
        prop: usize,
        value: String,
    },
    SetAttachedValueType {
        entity: u64,
        value: usize,
        path: Vec<VarPathPart>,
        kind: AttachedValueType,
    },
    SelectAll,
    InvertSelection,
    SelectChildren,
    SelectParent,
    SelectRoots,
    SelectLeaves,
    SelectVisible,
    SelectHidden,
    SelectLocked,
    SelectActive,
    SelectInactive,
    SelectSiblings,
    SelectNext,
    SelectPrevious,
    DuplicateSelection,
    GroupSelected,
    UnparentSelected,
    HideSelected,
    HideUnselected,
    IsolateSelection,
    ShowAllHidden,
    ShowSelected,
    LockSelected,
    LockUnselected,
    UnlockSelection,
    UnlockAll,
    ToggleActiveSelection,
    CollapseSelected,
    ExpandSelected,
    CollapseAll,
    ExpandAll,
    SnapSelected,
    SnapSelectedSize,
    ResetSelected,
    ResetSelectedRotation,
    ResetSelectedScale,
    ResetSelectedAnchors,
    FitSelectionToWindow,
    CenterSelectionInWindow,
    NormalizeSelectedSizes,
    Align(AlignKind),
    BringToFront,
    SendToBack,
    BringForward,
    SendBackward,
    NudgeZ(f32),
    RefreshProject,
    RevealSceneFile,
    OpenProjectRoot,
    FrameAll,
    Zoom100,
    ToggleMaximize,
    ToggleProject,
}

#[derive(Clone, Debug)]
struct ProjectWindowSettings {
    start_scene: String,
    width: f32,
    height: f32,
    fullscreen: bool,
    resizable: bool,
}

impl Default for ProjectWindowSettings {
    fn default() -> Self {
        Self {
            start_scene: super::DEFAULT_SCENE_FILE.to_string(),
            width: PREVIEW_ROOT_WIDTH,
            height: PREVIEW_ROOT_HEIGHT,
            fullscreen: false,
            resizable: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct AnimationClipAsset {
    duration: f32,
    looping: bool,
    tracks: Vec<AnimationTrackAsset>,
}

impl Default for AnimationClipAsset {
    fn default() -> Self {
        Self {
            duration: 1.0,
            looping: false,
            tracks: vec![AnimationTrackAsset::default()],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct AnimationTrackAsset {
    property: String,
    interpolation: String,
    keys: Vec<AnimationKeyAsset>,
}

impl Default for AnimationTrackAsset {
    fn default() -> Self {
        Self {
            property: "x".to_string(),
            interpolation: "linear".to_string(),
            keys: vec![
                AnimationKeyAsset::new(0.0, 0.0),
                AnimationKeyAsset::new(1.0, 100.0),
            ],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct AnimationKeyAsset {
    time: f32,
    value: f32,
    out_x: f32,
    out_y: f32,
    in_x: f32,
    in_y: f32,
}

impl AnimationKeyAsset {
    fn new(time: f32, value: f32) -> Self {
        Self {
            time,
            value,
            out_x: 0.333,
            out_y: 0.0,
            in_x: 0.667,
            in_y: 1.0,
        }
    }
}

impl Default for AnimationKeyAsset {
    fn default() -> Self {
        Self::new(0.0, 0.0)
    }
}

#[derive(Clone, Debug)]
struct MenuItem {
    action: Action,
    glyph: char,
    label: String,
    danger: bool,
}

#[derive(Clone, Debug)]
struct ViewportDrag {
    primary: u64,
    grab_x: f32,
    grab_y: f32,
    start_world: Vec<(u64, f32, f32)>,
    descendant_start_world: Vec<(u64, f32, f32)>,
    /// When set, the drag is constrained to this world-space unit axis (the
    /// move gizmo's X or Y arrow); otherwise the entity moves freely in 2D.
    axis: Option<(f32, f32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Viewport3DAxis {
    X,
    Y,
    Z,
}

impl Viewport3DAxis {
    const ALL: [Self; 3] = [Self::X, Self::Y, Self::Z];

    fn vector(self) -> Vec3 {
        match self {
            Self::X => Vec3::new(1.0, 0.0, 0.0),
            Self::Y => Vec3::new(0.0, 1.0, 0.0),
            Self::Z => Vec3::new(0.0, 0.0, 1.0),
        }
    }

    fn color(self) -> Rgba {
        match self {
            Self::X => MOVE_X_COLOR,
            Self::Y => MOVE_Y_COLOR,
            Self::Z => [72, 132, 240, 255],
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Viewport3DTransformStart {
    id: u64,
    position: Vec3,
    rotation: Vec3,
    scale: Vec3,
}

#[derive(Clone, Copy, Debug)]
enum Viewport3DDragMode {
    /// Free movement on the entity parent's local XY plane. The two projected
    /// derivatives map screen deltas back to authored local coordinates.
    MovePlane {
        screen_x_axis: (f32, f32),
        screen_y_axis: (f32, f32),
    },
    /// Movement along one authored position axis. `screen_axis` is the
    /// projected displacement caused by adding exactly one local unit.
    MoveAxis {
        axis: Viewport3DAxis,
        screen_axis: (f32, f32),
    },
    /// Positive, non-inverting per-axis scaling. The handle begins
    /// `screen_length` logical pixels from the origin and follows that ray.
    ScaleAxis {
        axis: Viewport3DAxis,
        screen_direction: (f32, f32),
        screen_length: f32,
    },
    /// Uniform scaling uses a fixed screen-space diagonal, so the gesture is
    /// independent of object size, camera distance, and degenerate parents.
    ScaleUniform {
        screen_direction: (f32, f32),
        screen_length: f32,
    },
    /// Directly edits one authored Euler field from the tangent of its
    /// projected rotation ring. The conversion is captured at press time so
    /// the ring never chases the pointer while its entity rotates.
    RotateAxis {
        axis: Viewport3DAxis,
        screen_tangent: (f32, f32),
        degrees_per_pixel: f32,
    },
}

/// Retained state for a 3D move/scale gesture. Capturing all selected
/// transforms at press time prevents frame-to-frame accumulation and makes
/// snapping and high-DPI pointer coalescing deterministic.
#[derive(Clone, Debug)]
struct Viewport3DDrag {
    start_mouse: (f32, f32),
    start: Vec<Viewport3DTransformStart>,
    mode: Viewport3DDragMode,
}

#[derive(Clone, Copy, Debug)]
struct Viewport3DGizmoAxis {
    axis: Viewport3DAxis,
    end: (f32, f32),
}

#[derive(Clone, Copy, Debug)]
struct Viewport3DRotationRing {
    axis: Viewport3DAxis,
    points: [Option<(f32, f32)>; ROTATION_RING_SAMPLES],
}

#[derive(Clone, Copy, Debug)]
struct Viewport3DGizmo {
    origin: (f32, f32),
    axes: [Option<Viewport3DGizmoAxis>; 3],
    rotation_rings: [Viewport3DRotationRing; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Viewport3DGizmoHit {
    MoveFree,
    MoveAxis(Viewport3DAxis),
    ScaleAxis(Viewport3DAxis),
    ScaleUniform,
    RotateAxis(Viewport3DAxis),
}

#[derive(Clone, Copy, Debug)]
struct Viewport3DRotationDragHit {
    axis: Viewport3DAxis,
    screen_tangent: (f32, f32),
    degrees_per_pixel: f32,
}

#[derive(Clone, Copy, Debug)]
struct Viewport3DLook {
    mouse_x: f32,
    mouse_y: f32,
    pitch: f32,
    yaw: f32,
    navigated: bool,
}

#[derive(Clone, Copy, Debug)]
struct Viewport3DHit {
    id: u64,
    points: [(f32, f32); 3],
    bounds: Rect,
    depth: f32,
}

#[derive(Clone, Copy, Debug)]
struct Viewport3DProxyHit {
    id: u64,
    x: f32,
    y: f32,
    radius: f32,
    depth: f32,
}

type Viewport3DDrawTriangle = (f32, u64, [(f32, f32); 3], Rgba);

/// Active rotation drag via the gizmo knob. The pivot is the entity's world
/// center captured at drag start, so rotating spins the entity in place.
#[derive(Clone, Copy, Debug)]
struct RotateDrag {
    id: u64,
    center_x: f32,
    center_y: f32,
}

#[derive(Clone, Copy, Debug)]
struct BoxSelect {
    start_x: f32,
    start_y: f32,
    additive: bool,
}

#[derive(Clone, Debug)]
enum InspectorReferenceDrag {
    Entity {
        id: u64,
        /// Keep the original Inspector visible while another hierarchy entity
        /// is picked up, so its reference field remains a valid drop target.
        inspector_owner: Option<u64>,
    },
    Component(ComponentReference),
}

/// Deferred work that a confirm/prompt dialog triggers on accept.
#[derive(Clone, Debug)]
enum Pending {
    LoadScene,
    Quit,
    RenameScene,
    CreateFolder,
    CreateScript,
    CreateShader,
    CreateAnimation,
    RenameEntity(u64),
    CloseDocument(usize),
    UpdateEngine,
}

#[derive(Clone, Copy, Debug)]
enum AssetKind {
    Image,
    Font,
    Sound,
    Mesh,
    Shader,
    Animation,
}

impl AssetKind {
    fn title(self) -> &'static str {
        match self {
            Self::Image => "Choose Image",
            Self::Font => "Choose Font",
            Self::Sound => "Choose Sound",
            Self::Mesh => "Choose 3D Mesh",
            Self::Shader => "Choose Fragment Shader",
            Self::Animation => "Choose Animation",
        }
    }

    fn glyph(self) -> char {
        match self {
            Self::Image => icon::IMAGE,
            Self::Font => icon::FONT_DOWNLOAD,
            Self::Sound => icon::AUDIOTRACK,
            Self::Mesh => icon::VIEW_IN_AR,
            Self::Shader => icon::DATA_OBJECT,
            Self::Animation => icon::PLAY,
        }
    }

    fn accepts(self, path: &Path) -> bool {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        match self {
            Self::Image => matches_ignore_ascii_case(
                extension,
                &[
                    "png", "jpg", "jpeg", "bmp", "gif", "webp", "tga", "tif", "tiff", "pnm", "ppm",
                    "pgm", "hdr", "dds",
                ],
            ),
            Self::Font => matches_ignore_ascii_case(extension, &["ttf", "otf"]),
            Self::Sound => {
                matches_ignore_ascii_case(extension, &["wav", "mp3", "ogg", "oga", "flac"])
            }
            Self::Mesh => matches_ignore_ascii_case(extension, &["obj", "fbx", "gltf", "glb"]),
            Self::Shader => matches_ignore_ascii_case(extension, &["glsl", "frag", "fs", "shader"]),
            Self::Animation => {
                matches_ignore_ascii_case(extension, &["neoanim", "animation", "anim"])
            }
        }
    }
}

/// Types available for values authored directly on an entity. The editor uses
/// explicit, readable names rather than exposing the serialized enum spelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AttachedValueType {
    Number,
    String,
    Boolean,
    Color,
    Entity,
    Component,
    Image,
    Sound,
    Shader,
    Animation,
    List,
    Table,
}

impl AttachedValueType {
    const ALL: [Self; 12] = [
        Self::Number,
        Self::String,
        Self::Boolean,
        Self::Color,
        Self::Entity,
        Self::Component,
        Self::Image,
        Self::Sound,
        Self::Shader,
        Self::Animation,
        Self::List,
        Self::Table,
    ];

    fn from_value(value: &VarValue) -> Self {
        match value {
            VarValue::Number(_) => Self::Number,
            VarValue::Text(_) => Self::String,
            VarValue::Bool(_) => Self::Boolean,
            VarValue::Color(_) => Self::Color,
            VarValue::Entity(_) => Self::Entity,
            VarValue::Component(_) => Self::Component,
            VarValue::Image(_) => Self::Image,
            VarValue::Audio(_) => Self::Sound,
            VarValue::Shader(_) => Self::Shader,
            VarValue::Animation(_) => Self::Animation,
            VarValue::List(_) => Self::List,
            VarValue::Dictionary(_) => Self::Table,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Number => "Number",
            Self::String => "String",
            Self::Boolean => "Boolean",
            Self::Color => "Color",
            Self::Entity => "Entity",
            Self::Component => "Component",
            Self::Image => "Image",
            Self::Sound => "Sound",
            Self::Shader => "Shader",
            Self::Animation => "Animation",
            Self::List => "List",
            Self::Table => "Table",
        }
    }

    fn glyph(self) -> char {
        match self {
            Self::Number => icon::NUMBERS,
            Self::String => icon::TEXT_FIELDS,
            Self::Boolean => icon::CHECK_BOX,
            Self::Color => icon::PALETTE,
            Self::Entity => icon::VIEW_IN_AR,
            Self::Component => icon::EXTENSION,
            Self::Image => icon::IMAGE,
            Self::Sound => icon::AUDIOTRACK,
            Self::Shader => icon::DATA_OBJECT,
            Self::Animation => icon::PLAY,
            Self::List => icon::FORMAT_LIST_BULLETED,
            Self::Table => icon::TABLE_ROWS,
        }
    }

    fn default_value(self) -> VarValue {
        match self {
            Self::Number => VarValue::Number(0.0),
            Self::String => VarValue::Text(String::new()),
            Self::Boolean => VarValue::Bool(false),
            Self::Color => VarValue::Color([255, 255, 255, 255]),
            Self::Entity => VarValue::Entity(None),
            Self::Component => VarValue::Component(None),
            Self::Image => VarValue::Image(String::new()),
            Self::Sound => VarValue::Audio(String::new()),
            Self::Shader => VarValue::Shader(String::new()),
            Self::Animation => VarValue::Animation(String::new()),
            Self::List => VarValue::List(Vec::new()),
            Self::Table => VarValue::Dictionary(Vec::new()),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum SequenceKind {
    Color,
    Transparency,
}

#[derive(Clone, Debug)]
enum SequenceValue {
    Colors(Vec<ColorKeypoint>),
    Numbers(Vec<NumberKeypoint>),
}

#[derive(Clone, Copy, Debug)]
struct SequenceColorPicker {
    rgba: [u8; 4],
    hue: f32,
}

struct ColorPickerPanelResponse {
    rgba: [u8; 4],
    hue: f32,
    changed: bool,
    open: bool,
}

#[derive(Clone, Debug)]
enum AssetTarget {
    Prop {
        entity: u64,
        component: usize,
        prop: usize,
    },
    ScriptVar {
        entity: u64,
        component: usize,
        var: usize,
        path: Vec<VarPathPart>,
    },
    AttachedValue {
        entity: u64,
        value: usize,
        path: Vec<VarPathPart>,
    },
}

#[derive(Clone, Copy, Debug)]
enum ValueOwner {
    ScriptVar {
        entity: u64,
        component: usize,
        var: usize,
    },
    AttachedValue {
        entity: u64,
        value: usize,
    },
}

impl ValueOwner {
    fn asset_target(self, path: &[VarPathPart]) -> AssetTarget {
        match self {
            Self::ScriptVar {
                entity,
                component,
                var,
            } => AssetTarget::ScriptVar {
                entity,
                component,
                var,
                path: path.to_vec(),
            },
            Self::AttachedValue { entity, value } => AssetTarget::AttachedValue {
                entity,
                value,
                path: path.to_vec(),
            },
        }
    }

    fn color_target(self, path: &[VarPathPart]) -> ColorTarget {
        match self {
            Self::ScriptVar {
                entity,
                component,
                var,
            } => ColorTarget::Var {
                entity,
                comp: component,
                var,
                path: path.to_vec(),
            },
            Self::AttachedValue { entity, value } => ColorTarget::AttachedValue {
                entity,
                value,
                path: path.to_vec(),
            },
        }
    }
}

/// An overlay drawn above everything, with input precedence.
/// One selectable row in the searchable "Add Component" picker.
#[derive(Clone)]
struct ComponentPickerEntry {
    label: String,
    glyph: char,
    action: Action,
}

enum Popup {
    Menu {
        x: f32,
        y: f32,
        items: Vec<MenuItem>,
    },
    /// Searchable "Add Component" list: filtered live by `query`, auto-focused,
    /// and Enter adds the top match.
    ComponentPicker {
        x: f32,
        y: f32,
        query: String,
        scroll: f32,
        entries: Vec<ComponentPickerEntry>,
    },
    Color {
        target: ColorTarget,
        x: f32,
        y: f32,
        rgba: [u8; 4],
        /// Cached hue (0..360) so dragging stays stable at greys where hue is
        /// otherwise undefined.
        hue: f32,
    },
    Confirm {
        message: String,
        action: Pending,
    },
    Prompt {
        title: String,
        action: Pending,
    },
    Asset {
        target: AssetTarget,
        kind: AssetKind,
        files: Vec<String>,
        query: String,
        scroll: f32,
    },
    Sequence {
        target: AssetTarget,
        kind: SequenceKind,
        value: SequenceValue,
        selected: usize,
        dragging: Option<usize>,
        color_picker: Option<SequenceColorPicker>,
    },
    /// A runtime error captured from a failed `Run`, with a copy button.
    Error {
        message: String,
        copied: bool,
    },
    BuildTarget,
    ProjectWindow {
        start_scene: String,
        width: String,
        height: String,
        fullscreen: bool,
        resizable: bool,
    },
    MobileEmulator {
        enabled: bool,
        orientation: String,
        wifi: bool,
        cellular: bool,
        low_power: bool,
    },
    EditorSettings {
        theme_name: String,
        custom_theme: Theme,
        original_theme: Theme,
        font_path: String,
        show_tooltips: bool,
        show_window_bounds: bool,
        show_transform_hud: bool,
        preview_lighting: bool,
        autosave_before_run: bool,
        autosave_before_build: bool,
        viewport_camera_sensitivity: f32,
        viewport_camera_speed: f32,
        viewport_camera_fov: f32,
        viewport_invert_mouse_look: bool,
    },
    AnimationEditor {
        path: PathBuf,
        clip: AnimationClipAsset,
        selected_track: usize,
        selected_key: usize,
    },
}

pub struct EditorApp {
    project_root: PathBuf,
    scene_path: PathBuf,
    config_path: PathBuf,
    scene: Scene,
    documents: Vec<OpenDocument>,
    active_document: usize,
    document_kind: DocumentKind,
    config: EditorConfig,
    project_window: ProjectWindowSettings,
    selected: Option<u64>,
    selected_ids: HashSet<u64>,
    dragging: Option<ViewportDrag>,
    box_select: Option<BoxSelect>,
    /// Active resize: (entity id, fixed anchor corner world x/y, grabbed-corner
    /// local fractions). The fractions (0 or 1 on each axis) identify which
    /// corner is being dragged so resizes stay correct when the entity is
    /// rotated.
    resizing: Option<(u64, f32, f32, f32, f32)>,
    /// Active rotation drag via the gizmo knob.
    rotating: Option<RotateDrag>,
    active_splitter: Option<Splitter>,
    hierarchy_scroll: f32,
    inspector_scroll: f32,
    hierarchy_content_h: f32,
    inspector_content_h: f32,
    bin_dir: PathBuf,
    bin_back: Vec<PathBuf>,
    bin_forward: Vec<PathBuf>,
    bin_scroll: f32,
    bin_content_h: f32,
    /// Viewport pan offset (middle-mouse drag) and zoom (scroll wheel).
    cam_x: f32,
    cam_y: f32,
    cam_zoom: f32,
    /// Independent perspective Scene-view camera used only by 3D projects.
    /// Runtime Camera3D components remain scene data and never steal editor
    /// navigation state.
    viewport_camera_3d: RenderCamera3D,
    viewport_3d_look: Option<Viewport3DLook>,
    viewport_3d_pan_anchor: Option<(f32, f32, Vec3)>,
    viewport_3d_drag: Option<Viewport3DDrag>,
    viewport_3d_last_frame: Instant,
    /// Reused CPU preview/picking storage. Complex scenes otherwise allocate
    /// and free several large vectors on every continuously-redrawn frame.
    viewport_3d_triangles: Vec<Viewport3DDrawTriangle>,
    viewport_3d_triangle_hits: Vec<Viewport3DHit>,
    viewport_3d_proxy_hits: Vec<Viewport3DProxyHit>,
    /// Anchor captured when a middle-mouse pan begins: (mouse x, mouse y, cam x,
    /// cam y). Panning relative to a fixed anchor avoids the camera jumping by
    /// accumulated hover movement.
    pan_anchor: Option<(f32, f32, f32, f32)>,
    /// The viewport rect from the last frame (for framing the selection).
    last_viewport: Rect,
    /// Hierarchy name filter (search box).
    hierarchy_filter: String,
    /// Per-branch fold state in the hierarchy.
    hierarchy_collapsed: HashSet<u64>,
    /// Scene-view-only visibility and picking state (not exported).
    hidden_ids: HashSet<u64>,
    locked_ids: HashSet<u64>,
    /// Unity-style focused Scene view; restores the dock layout when toggled off.
    maximize_view: bool,
    /// Undo/redo stacks of serialized scene snapshots, plus the baseline used
    /// to coalesce a continuous edit (a drag, a typing session) into one entry.
    undo_stack: Vec<String>,
    redo_stack: Vec<String>,
    undo_baseline: String,
    /// Collapsed section keys (component bodies / advanced groups).
    collapsed: HashSet<String>,
    clipboard: Option<Entity>,
    /// A copied component, pasteable onto any entity.
    component_clipboard: Option<Component>,
    /// Hierarchy drag-to-reparent: the entity being dragged.
    reparent_drag: Option<u64>,
    /// An entity or component currently being carried toward an Inspector
    /// reference field.
    inspector_reference_drag: Option<InspectorReferenceDrag>,
    /// A `.neoprefab` file being dragged from the project bin into the scene.
    prefab_drag: Option<PathBuf>,
    /// A Luau component script being dragged onto a hierarchy or viewport entity.
    script_drag: Option<PathBuf>,
    /// A model asset being dragged from the Project browser into a 3D scene or
    /// onto a hierarchy entity.
    mesh_drag: Option<PathBuf>,
    /// Active tilemap paint target (entity id, component index) and selected tile id.
    tile_paint: Option<(u64, usize)>,
    tile_paint_tile: i32,
    popup: Option<Popup>,
    /// Lazily-loaded image assets for accurate viewport previews. `None` marks
    /// a path that failed to load so we don't retry it every frame.
    image_cache: RefCell<HashMap<String, EditorImageCacheEntry>>,
    /// Imported editor-preview meshes. Failures are cached too; a changed file
    /// stamp retries automatically and least-recently-used entries are evicted.
    mesh_cache: RefCell<HashMap<String, EditorMeshCacheEntry>>,
    mesh_cache_clock: Cell<u64>,
    /// Downsampled audio peaks for asset-picker waveform previews.
    waveform_cache: RefCell<HashMap<String, EditorWaveformCacheEntry>>,
    project_directory_cache: RefCell<HashMap<PathBuf, ProjectDirectoryCacheEntry>>,
    /// World transforms are requested repeatedly by drawing, selection,
    /// gizmos, and collider previews. Cache each hierarchy walk once per frame.
    world_transform_cache: RefCell<HashMap<u64, EditorWorldTransform>>,
    world_model_3d_cache: RefCell<HashMap<u64, Mat4>>,
    /// Cached viewport light grid. The editor redraws continuously, so this
    /// avoids re-tracing shadow rays every frame while the camera and lighting
    /// inputs are unchanged (see [`EditorApp::composite_preview_lighting`]).
    preview_light_cache: RefCell<Option<PreviewLightGrid>>,
    /// Parsed Inspector schemas cached by source path and modification time.
    script_schema_cache: ScriptSchemaCache,
    /// Receiver for the outcome of a launched `Run` (None when finished).
    run_rx: Option<std::sync::mpsc::Receiver<Option<String>>>,
    /// Receiver for the outcome of a launched `Build` (None when finished).
    build_rx: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    /// A freshly created logger IPC session waiting to be picked up by the
    /// windowing layer to open/show the logger window.
    pending_logger_session: Option<crate::editor_ipc::LoggerSession>,
    /// Background Git upstream check; network work never blocks editor frames.
    update_rx: Option<std::sync::mpsc::Receiver<Result<Option<AvailableUpdate>, String>>>,
    /// An update result waiting for another modal popup to close.
    pending_update: Option<AvailableUpdate>,
    status: String,
    scene_dirty: bool,
    should_quit: bool,
    dirty: bool,
    focus: Option<String>,
    edit_buffer: String,
    edit_cursor: usize,
    edit_selection_anchor: Option<usize>,
    pointer_capture: Option<String>,
    /// A validated font selection waiting for the window layer to install it.
    /// An empty string switches back to the bundled font.
    font_reload_request: Option<String>,
}

impl EditorApp {
    #[allow(dead_code)]
    pub fn new(
        project_root: PathBuf,
        scene_path: PathBuf,
        scene: Scene,
        config: EditorConfig,
    ) -> Self {
        let config_path = project_root.join("editor.json");
        Self::new_with_config_path(project_root, scene_path, scene, config, config_path)
    }

    pub fn new_with_config_path(
        project_root: PathBuf,
        scene_path: PathBuf,
        scene: Scene,
        config: EditorConfig,
        config_path: PathBuf,
    ) -> Self {
        let scene_json = scene.to_json().unwrap_or_default();
        let project_window = load_project_window_settings(&project_root);
        let documents = vec![OpenDocument {
            path: scene_path.clone(),
            scene: scene.clone(),
            kind: DocumentKind::Scene,
            dirty: false,
        }];
        Self {
            bin_dir: project_root.clone(),
            project_root,
            scene_path,
            config_path,
            scene,
            documents,
            active_document: 0,
            document_kind: DocumentKind::Scene,
            config,
            project_window,
            selected: None,
            selected_ids: HashSet::new(),
            dragging: None,
            box_select: None,
            resizing: None,
            rotating: None,
            active_splitter: None,
            hierarchy_scroll: 0.0,
            inspector_scroll: 0.0,
            hierarchy_content_h: 0.0,
            inspector_content_h: 0.0,
            bin_back: Vec::new(),
            bin_forward: Vec::new(),
            bin_scroll: 0.0,
            bin_content_h: 0.0,
            cam_x: 0.0,
            cam_y: 0.0,
            cam_zoom: 1.0,
            viewport_camera_3d: default_editor_camera_3d(60.0),
            viewport_3d_look: None,
            viewport_3d_pan_anchor: None,
            viewport_3d_drag: None,
            viewport_3d_last_frame: Instant::now(),
            viewport_3d_triangles: Vec::new(),
            viewport_3d_triangle_hits: Vec::new(),
            viewport_3d_proxy_hits: Vec::new(),
            pan_anchor: None,
            last_viewport: Rect::new(0.0, 0.0, 0.0, 0.0),
            hierarchy_filter: String::new(),
            hierarchy_collapsed: HashSet::new(),
            hidden_ids: HashSet::new(),
            locked_ids: HashSet::new(),
            maximize_view: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            undo_baseline: scene_json,
            collapsed: HashSet::new(),
            clipboard: None,
            component_clipboard: None,
            reparent_drag: None,
            inspector_reference_drag: None,
            prefab_drag: None,
            script_drag: None,
            mesh_drag: None,
            tile_paint: None,
            tile_paint_tile: 0,
            popup: None,
            image_cache: RefCell::new(HashMap::new()),
            mesh_cache: RefCell::new(HashMap::new()),
            mesh_cache_clock: Cell::new(0),
            waveform_cache: RefCell::new(HashMap::new()),
            project_directory_cache: RefCell::new(HashMap::new()),
            world_transform_cache: RefCell::new(HashMap::new()),
            world_model_3d_cache: RefCell::new(HashMap::new()),
            preview_light_cache: RefCell::new(None),
            script_schema_cache: HashMap::new(),
            run_rx: None,
            build_rx: None,
            pending_logger_session: None,
            update_rx: None,
            pending_update: None,
            status: "Ready".to_string(),
            scene_dirty: false,
            should_quit: false,
            dirty: false,
            focus: None,
            edit_buffer: String::new(),
            edit_cursor: 0,
            edit_selection_anchor: None,
            pointer_capture: None,
            font_reload_request: None,
        }
    }

    pub fn title(&self) -> String {
        let star = if self.scene_dirty { "*" } else { "" };
        format!("NeoLOVE Editor — {}{}", self.scene.name, star)
    }

    pub fn theme(&self) -> Theme {
        self.config.theme.clone()
    }

    pub(crate) fn widget_title(widget: EditorWidget) -> &'static str {
        match widget {
            EditorWidget::Hierarchy => "NeoLOVE - Hierarchy",
            EditorWidget::Inspector => "NeoLOVE - Inspector",
            EditorWidget::Project => "NeoLOVE - Project",
        }
    }

    pub(crate) fn widget_undocked(&self, widget: EditorWidget) -> bool {
        match widget {
            EditorWidget::Hierarchy => {
                self.config.layout.show_hierarchy && self.config.layout.undock_hierarchy
            }
            EditorWidget::Inspector => {
                self.config.layout.show_inspector && self.config.layout.undock_inspector
            }
            EditorWidget::Project => {
                self.config.layout.show_project && self.config.layout.undock_project
            }
        }
    }

    pub(crate) fn close_detached_widget(&mut self, widget: EditorWidget) {
        match widget {
            EditorWidget::Hierarchy => {
                self.config.layout.show_hierarchy = false;
                self.config.layout.undock_hierarchy = false;
            }
            EditorWidget::Inspector => {
                self.config.layout.show_inspector = false;
                self.config.layout.undock_inspector = false;
            }
            EditorWidget::Project => {
                self.config.layout.show_project = false;
                self.config.layout.undock_project = false;
            }
        }
        self.dirty = true;
    }

    pub(crate) fn dock_widget(&mut self, widget: EditorWidget) {
        match widget {
            EditorWidget::Hierarchy => {
                self.config.layout.show_hierarchy = true;
                self.config.layout.undock_hierarchy = false;
            }
            EditorWidget::Inspector => {
                self.config.layout.show_inspector = true;
                self.config.layout.undock_inspector = false;
            }
            EditorWidget::Project => {
                self.config.layout.show_project = true;
                self.config.layout.undock_project = false;
            }
        }
        self.dirty = true;
    }

    pub(crate) fn frame_detached_widget(&mut self, ui: &mut Ui, widget: EditorWidget) {
        let w = ui.painter.width();
        let h = ui.painter.height();
        let area = Rect::new(0.0, 0.0, w, h);
        let raw_left = ui.input.mouse_pressed;
        let raw_right = ui.input.right_pressed;
        let popup_interactive = self.popup.is_some();
        if self.popup.is_some() {
            ui.input.mouse_pressed = false;
            ui.input.right_pressed = false;
        }
        ui.painter.clear(self.config.theme.panel);
        match widget {
            EditorWidget::Hierarchy => self.render_panel(ui, area, Panel::Hierarchy),
            EditorWidget::Inspector => self.render_panel(ui, area, Panel::Inspector),
            EditorWidget::Project => self.project_bin(ui, area),
        }
        ui.input.mouse_pressed = raw_left;
        ui.input.right_pressed = raw_right;
        self.handle_popup(ui, w, h, popup_interactive);
        self.commit_undo_if_settled(ui);
        if self.config.settings.show_tooltips {
            ui.draw_tooltip();
        }
    }

    pub fn take_focus(&mut self) -> Option<String> {
        self.focus.take()
    }

    pub fn take_edit_buffer(&mut self) -> String {
        std::mem::take(&mut self.edit_buffer)
    }

    pub fn take_edit_cursor(&mut self) -> usize {
        std::mem::take(&mut self.edit_cursor)
    }

    pub fn take_edit_selection_anchor(&mut self) -> Option<usize> {
        self.edit_selection_anchor.take()
    }

    pub fn take_pointer_capture(&mut self) -> Option<String> {
        self.pointer_capture.take()
    }

    pub fn set_focus(
        &mut self,
        focus: Option<String>,
        edit_buffer: String,
        edit_cursor: usize,
        edit_selection_anchor: Option<usize>,
        pointer_capture: Option<String>,
    ) {
        // A dialog may request focus while the immediate-mode frame is being
        // drawn. Preserve that newer request instead of overwriting it with
        // the focus state captured at the beginning of the frame.
        if self.focus.is_none() {
            self.focus = focus;
            self.edit_buffer = edit_buffer;
            self.edit_cursor = edit_cursor;
            self.edit_selection_anchor = edit_selection_anchor;
        }
        self.pointer_capture = pointer_capture;
    }

    pub fn take_font_reload_request(&mut self) -> Option<String> {
        self.font_reload_request.take()
    }

    pub fn flush_config(&mut self) {
        if self.dirty {
            if let Err(error) = save_config(&self.config_path, &self.config) {
                eprintln!("warning: failed to save editor config: {error}");
            }
            self.dirty = false;
        }
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn start_update_check(&mut self) {
        if self.update_rx.is_some() {
            return;
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(crate::update::check_for_update());
        });
        self.update_rx = Some(receiver);
    }

    /// Poll the non-blocking update check. Returns true when visible state changed.
    pub fn poll_update_check(&mut self) -> bool {
        let result = self
            .update_rx
            .as_ref()
            .and_then(|receiver| match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some(Err("update check stopped unexpectedly".to_string()))
                }
            });

        if let Some(result) = result {
            self.update_rx = None;
            match result {
                Ok(Some(update)) => self.offer_update(update),
                Ok(None) => {
                    self.status = "NeoLOVE is up to date".to_string();
                }
                Err(error) => {
                    eprintln!("warning: NeoLOVE update check failed: {error}");
                }
            }
            return true;
        }

        if self.popup.is_none()
            && let Some(update) = self.pending_update.take()
        {
            self.offer_update(update);
            return true;
        }
        false
    }

    fn offer_update(&mut self, update: AvailableUpdate) {
        self.status = format!(
            "Update available on {}: {} -> {}",
            update.branch,
            short_revision(&update.current_revision),
            short_revision(&update.latest_revision)
        );
        if self.popup.is_some() {
            self.pending_update = Some(update);
        } else {
            self.open_confirm(
                "A NeoLOVE update is available. Update and restart?",
                Pending::UpdateEngine,
            );
        }
    }

    fn launch_update(&mut self) {
        self.sync_active_document();
        if self.documents.iter().any(|document| document.dirty) {
            self.popup = Some(Popup::Error {
                message: "Save or discard all unsaved scene changes before updating NeoLOVE."
                    .to_string(),
                copied: false,
            });
            return;
        }

        let executable = match std::env::current_exe() {
            Ok(executable) => executable,
            Err(error) => {
                self.popup = Some(Popup::Error {
                    message: format!("Could not start the NeoLOVE update: {error}"),
                    copied: false,
                });
                return;
            }
        };
        match std::process::Command::new(executable).arg("update").spawn() {
            Ok(_) => {
                self.status = "Updating NeoLOVE; the editor will restart when reopened".to_string();
                self.should_quit = true;
            }
            Err(error) => {
                self.popup = Some(Popup::Error {
                    message: format!("Could not start the NeoLOVE update: {error}"),
                    copied: false,
                });
            }
        }
    }

    /// Called from the event loop when the window is asked to close. Returns
    /// true if it's safe to exit; otherwise opens a save-confirmation dialog.
    pub fn request_close(&mut self) -> bool {
        self.sync_active_document();
        if self.documents.iter().any(|document| document.dirty) {
            self.open_confirm("Discard unsaved changes and quit?", Pending::Quit);
            false
        } else {
            true
        }
    }

    fn mark_dirty(&mut self) {
        self.world_transform_cache.borrow_mut().clear();
        self.world_model_3d_cache.borrow_mut().clear();
        self.scene_dirty = true;
        if let Some(document) = self.documents.get_mut(self.active_document) {
            document.dirty = true;
        }
    }

    fn add_post_process_pass(&mut self) {
        self.scene
            .post_process
            .effects
            .push(PostProcessEffectPass::new(default_post_process_effect(0)));
        self.mark_dirty();
    }

    fn cycle_post_process_pass_kind(&mut self, index: usize) -> bool {
        let Some(pass) = self.scene.post_process.effects.get_mut(index) else {
            return false;
        };
        pass.effect = next_post_process_effect(&pass.effect);
        self.mark_dirty();
        true
    }

    fn move_post_process_pass(&mut self, from: usize, to: usize) -> bool {
        let len = self.scene.post_process.effects.len();
        if from >= len || to >= len || from == to {
            return false;
        }
        let pass = self.scene.post_process.effects.remove(from);
        self.scene.post_process.effects.insert(to, pass);
        self.mark_dirty();
        true
    }

    fn remove_post_process_pass(&mut self, index: usize) -> bool {
        if index >= self.scene.post_process.effects.len() {
            return false;
        }
        self.scene.post_process.effects.remove(index);
        self.mark_dirty();
        true
    }

    fn sync_active_document(&mut self) {
        if let Some(document) = self.documents.get_mut(self.active_document) {
            document.path = self.scene_path.clone();
            document.scene = self.scene.clone();
            document.kind = self.document_kind;
            document.dirty = self.scene_dirty;
        }
    }

    fn switch_document(&mut self, index: usize) {
        if index == self.active_document || index >= self.documents.len() {
            return;
        }
        self.sync_active_document();
        self.active_document = index;
        let document = self.documents[index].clone();
        self.scene_path = document.path;
        self.scene = document.scene;
        self.document_kind = document.kind;
        self.scene_dirty = document.dirty;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.undo_baseline = self.scene.to_json().unwrap_or_default();
        self.clear_scene_view_state();
        self.status = format!("Switched to {}", self.scene.name);
    }

    fn request_close_document(&mut self, index: usize) {
        if self.documents.len() <= 1 || index >= self.documents.len() {
            return;
        }
        self.sync_active_document();
        if self.documents[index].dirty {
            self.open_confirm(
                &format!(
                    "Discard unsaved changes in '{}' and close it?",
                    self.documents[index].scene.name
                ),
                Pending::CloseDocument(index),
            );
        } else {
            self.close_document(index);
        }
    }

    fn close_document(&mut self, index: usize) {
        if self.documents.len() <= 1 || index >= self.documents.len() {
            return;
        }
        self.sync_active_document();
        let closed_name = self.documents[index].scene.name.clone();
        let closing_active = index == self.active_document;
        self.documents.remove(index);

        if index < self.active_document {
            self.active_document -= 1;
        } else if closing_active {
            self.active_document = index.min(self.documents.len() - 1);
            let document = self.documents[self.active_document].clone();
            self.scene_path = document.path;
            self.scene = document.scene;
            self.document_kind = document.kind;
            self.scene_dirty = document.dirty;
            self.undo_stack.clear();
            self.redo_stack.clear();
            self.undo_baseline = self.scene.to_json().unwrap_or_default();
            self.clear_scene_view_state();
        }
        self.status = format!("Closed {closed_name}");
    }

    fn add_document(&mut self, path: PathBuf, scene: Scene, kind: DocumentKind) {
        if let Some(index) = self
            .documents
            .iter()
            .position(|document| document.path == path)
        {
            self.switch_document(index);
            return;
        }
        self.sync_active_document();
        self.documents.push(OpenDocument {
            path: path.clone(),
            scene: scene.clone(),
            kind,
            dirty: false,
        });
        self.active_document = self.documents.len() - 1;
        self.scene_path = path;
        self.scene = scene;
        self.document_kind = kind;
        self.scene_dirty = false;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.undo_baseline = self.scene.to_json().unwrap_or_default();
        self.clear_scene_view_state();
    }

    fn prune_selection(&mut self) {
        let scene = &self.scene;
        self.selected_ids.retain(|id| scene.entity(*id).is_some());
        self.hidden_ids.retain(|id| scene.entity(*id).is_some());
        self.locked_ids.retain(|id| scene.entity(*id).is_some());
        self.hierarchy_collapsed
            .retain(|id| scene.entity(*id).is_some());
        if self.selected.and_then(|id| self.scene.entity(id)).is_none() {
            self.selected = self.selection_ids_ordered().into_iter().next();
        }
    }

    fn is_selected(&self, id: u64) -> bool {
        self.selected == Some(id) || self.selected_ids.contains(&id)
    }

    fn selection_ids_ordered(&self) -> Vec<u64> {
        self.scene
            .entities
            .iter()
            .filter(|entity| {
                self.selected == Some(entity.id) || self.selected_ids.contains(&entity.id)
            })
            .map(|entity| entity.id)
            .collect()
    }

    fn selection_count(&self) -> usize {
        self.selection_ids_ordered().len()
    }

    fn clear_selection(&mut self) {
        self.selected = None;
        self.selected_ids.clear();
    }

    fn clear_scene_view_state(&mut self) {
        self.clear_selection();
        self.hidden_ids.clear();
        self.locked_ids.clear();
        self.hierarchy_collapsed.clear();
    }

    fn select_only(&mut self, id: u64) {
        self.selected = Some(id);
        self.selected_ids.clear();
        self.selected_ids.insert(id);
    }

    fn add_to_selection(&mut self, id: u64) {
        self.selected = Some(id);
        self.selected_ids.insert(id);
    }

    fn toggle_selection(&mut self, id: u64) {
        if self.is_selected(id) {
            self.selected_ids.remove(&id);
            if self.selected == Some(id) {
                self.selected = None;
                self.selected = self.selection_ids_ordered().into_iter().next();
            }
        } else {
            self.add_to_selection(id);
        }
    }

    fn select_many(&mut self, ids: Vec<u64>, additive: bool) {
        if !additive {
            self.clear_selection();
        }
        for id in ids {
            if self.scene.entity(id).is_some() {
                self.selected_ids.insert(id);
                self.selected = Some(id);
            }
        }
    }

    fn select_with_modifiers(&mut self, id: u64, ui: &Ui) {
        if ui.input.ctrl {
            self.toggle_selection(id);
        } else if ui.input.shift {
            self.add_to_selection(id);
        } else if self.is_selected(id) && self.selection_count() > 1 {
            self.selected = Some(id);
            self.selected_ids.insert(id);
        } else {
            self.select_only(id);
        }
    }

    /// Once an interaction settles (no mouse button held, no field focused),
    /// coalesce any change since the last baseline into a single undo entry.
    fn commit_undo_if_settled(&mut self, ui: &Ui) {
        let settled = !ui.input.mouse_down
            && !ui.has_focus()
            && self.dragging.is_none()
            && self.viewport_3d_drag.is_none()
            && self.resizing.is_none()
            && self.rotating.is_none()
            && self.box_select.is_none();
        if !settled {
            return;
        }
        if let Ok(cur) = self.scene.to_json() {
            if cur != self.undo_baseline {
                self.undo_stack
                    .push(std::mem::replace(&mut self.undo_baseline, cur));
                if self.undo_stack.len() > 100 {
                    self.undo_stack.remove(0);
                }
                self.redo_stack.clear();
            }
        }
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo_stack.pop() {
            if let Ok(scene) = Scene::from_json(&prev) {
                self.redo_stack
                    .push(std::mem::replace(&mut self.undo_baseline, prev));
                self.scene = scene;
                self.clear_selection();
                self.scene_dirty = true;
                self.status = "Undo".to_string();
            }
        } else {
            self.status = "Nothing to undo".to_string();
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo_stack.pop() {
            if let Ok(scene) = Scene::from_json(&next) {
                self.undo_stack
                    .push(std::mem::replace(&mut self.undo_baseline, next));
                self.scene = scene;
                self.clear_selection();
                self.scene_dirty = true;
                self.status = "Redo".to_string();
            }
        } else {
            self.status = "Nothing to redo".to_string();
        }
    }

    /// Center the viewport on the selected entity at a comfortable zoom.
    fn frame_selected(&mut self) {
        let area = self.last_viewport;
        if area.w <= 0.0 {
            return;
        }
        let ids = self.selection_ids_ordered();
        if ids.is_empty() {
            return;
        }
        if self.scene.kind == SceneKind::ThreeD {
            let mut target = Vec3::ZERO;
            let mut count = 0.0;
            for id in ids {
                if let Some(model) = self.entity_world_model_3d(id) {
                    let position = model.transform_point(Vec3::ZERO);
                    target.x += position.x;
                    target.y += position.y;
                    target.z += position.z;
                    count += 1.0;
                }
            }
            if count > 0.0 {
                target.x /= count;
                target.y /= count;
                target.z /= count;
                let forward = camera_forward(self.viewport_camera_3d.euler);
                let distance = 6.0_f32.max(count.sqrt() * 2.0);
                self.viewport_camera_3d.position = Vec3::new(
                    target.x - forward.x * distance,
                    target.y - forward.y * distance,
                    target.z - forward.z * distance,
                );
                self.status = "Framed 3D selection".to_string();
            }
            return;
        }
        let mut bounds: Option<(f32, f32, f32, f32)> = None;
        for id in ids {
            if let Some(e) = self.scene.entity(id) {
                let world = self
                    .entity_world_transform(id)
                    .unwrap_or(EditorWorldTransform {
                        x: e.x,
                        y: e.y,
                        scale: editor_entity_scale(e),
                        rotation: e.rotation,
                    });
                let (size_x, size_y) = editor_entity_size(&self.scene, e, self.preview_root_size());
                let w = (size_x * world.scale).max(1.0);
                let h = (size_y * world.scale).max(1.0);
                bounds = Some(match bounds {
                    Some((min_x, min_y, max_x, max_y)) => (
                        min_x.min(world.x),
                        min_y.min(world.y),
                        max_x.max(world.x + w),
                        max_y.max(world.y + h),
                    ),
                    None => (world.x, world.y, world.x + w, world.y + h),
                });
            }
        }
        if let Some((min_x, min_y, max_x, max_y)) = bounds {
            let w = (max_x - min_x).max(1.0);
            let h = (max_y - min_y).max(1.0);
            let zoom = ((area.w * 0.5 / w).min(area.h * 0.5 / h)).clamp(0.2, 4.0);
            let cx = min_x + w * 0.5;
            let cy = min_y + h * 0.5;
            self.cam_zoom = zoom;
            self.cam_x = area.w * 0.5 - cx * zoom;
            self.cam_y = area.h * 0.5 - cy * zoom;
            self.status = "Framed selection".to_string();
        }
    }

    fn reset_view(&mut self) {
        if self.scene.kind == SceneKind::ThreeD {
            self.viewport_camera_3d =
                default_editor_camera_3d(self.config.settings.viewport_camera_fov);
            self.viewport_3d_look = None;
            self.viewport_3d_pan_anchor = None;
            return;
        }
        self.cam_x = 0.0;
        self.cam_y = 0.0;
        self.cam_zoom = 1.0;
    }

    fn select_all(&mut self) {
        let ids = self.scene.entities.iter().map(|entity| entity.id).collect();
        self.select_many(ids, false);
        self.status = format!("Selected {} entities", self.selection_count());
    }

    fn invert_selection(&mut self) {
        let previous: HashSet<u64> = self.selection_ids_ordered().into_iter().collect();
        let ids = self
            .scene
            .entities
            .iter()
            .filter(|entity| !previous.contains(&entity.id))
            .map(|entity| entity.id)
            .collect();
        self.select_many(ids, false);
        self.status = "Inverted selection".to_string();
    }

    fn select_children(&mut self) {
        let roots = self.selection_ids_ordered();
        let mut stack = roots.clone();
        let mut ids = roots;
        while let Some(id) = stack.pop() {
            for child in self.scene.children_of(Some(id)) {
                if !ids.contains(&child) {
                    ids.push(child);
                    stack.push(child);
                }
            }
        }
        self.select_many(ids, false);
        self.status = "Selected descendants".to_string();
    }

    fn select_parent(&mut self) {
        let parents: Vec<u64> = self
            .selection_ids_ordered()
            .into_iter()
            .filter_map(|id| self.scene.entity(id).and_then(|entity| entity.parent))
            .collect();
        if !parents.is_empty() {
            self.select_many(parents, false);
            self.status = "Selected parent".to_string();
        }
    }

    fn duplicate_selection(&mut self) {
        let selected = self.selection_ids_ordered();
        if selected.is_empty() {
            return;
        }
        let selected_set: HashSet<u64> = selected.iter().copied().collect();
        let roots: Vec<u64> = selected
            .into_iter()
            .filter(|id| {
                let mut parent = self.scene.entity(*id).and_then(|entity| entity.parent);
                while let Some(parent_id) = parent {
                    if selected_set.contains(&parent_id) {
                        return false;
                    }
                    parent = self
                        .scene
                        .entity(parent_id)
                        .and_then(|entity| entity.parent);
                }
                true
            })
            .collect();
        let mut new_roots = Vec::new();
        for root in roots {
            let original_parent = self.scene.entity(root).and_then(|entity| entity.parent);
            let mut proto = self.scene.subtree(root);
            if let Some(first) = proto.first_mut() {
                first.x += 16.0;
                first.y += 16.0;
                first.name = format!("{} Copy", first.name);
            }
            if let Some(new_root) = self.scene.instantiate(proto) {
                if let Some(entity) = self.scene.entity_mut(new_root) {
                    entity.parent = original_parent;
                }
                new_roots.push(new_root);
            }
        }
        self.select_many(new_roots, false);
        self.mark_dirty();
        self.status = "Duplicated selection".to_string();
    }

    fn group_selected(&mut self) {
        let selected = self.selection_ids_ordered();
        if selected.is_empty() {
            return;
        }
        let selected_set: HashSet<u64> = selected.iter().copied().collect();
        let roots: Vec<u64> = selected
            .into_iter()
            .filter(|id| {
                !self
                    .scene
                    .entity(*id)
                    .and_then(|entity| entity.parent)
                    .is_some_and(|parent| selected_set.contains(&parent))
            })
            .collect();
        let positions: Vec<(u64, f32, f32)> = roots
            .iter()
            .filter_map(|id| {
                self.entity_world_transform(*id)
                    .map(|world| (*id, world.x, world.y))
            })
            .collect();
        if positions.is_empty() {
            return;
        }
        let gx = positions.iter().map(|(_, x, _)| *x).sum::<f32>() / positions.len() as f32;
        let gy = positions.iter().map(|(_, _, y)| *y).sum::<f32>() / positions.len() as f32;
        let group = self.scene.add_entity("Group", gx, gy).id;
        for (id, world_x, world_y) in positions {
            if let Some(entity) = self.scene.entity_mut(id) {
                entity.parent = Some(group);
                entity.x = world_x - gx;
                entity.y = world_y - gy;
            }
        }
        self.select_only(group);
        self.mark_dirty();
        self.status = "Grouped selection".to_string();
    }

    fn unparent_selected(&mut self) {
        let updates: Vec<(u64, f32, f32)> = self
            .selection_ids_ordered()
            .into_iter()
            .filter_map(|id| {
                self.entity_world_transform(id)
                    .map(|world| (id, world.x, world.y))
            })
            .collect();
        for (id, world_x, world_y) in updates {
            if let Some(entity) = self.scene.entity_mut(id) {
                entity.parent = None;
                entity.x = world_x;
                entity.y = world_y;
            }
        }
        self.mark_dirty();
        self.status = "Unparented selection".to_string();
    }

    fn hide_selected(&mut self) {
        let ids = self.selection_ids_ordered();
        self.hidden_ids.extend(ids.iter().copied());
        self.clear_selection();
        self.status = format!("Hidden {} entities in Scene view", ids.len());
    }

    fn lock_selected(&mut self) {
        let ids = self.selection_ids_ordered();
        self.locked_ids.extend(ids.iter().copied());
        self.status = format!("Locked {} entities for scene picking", ids.len());
    }

    fn reset_selected(&mut self) {
        let kind = self.scene.kind;
        for id in self.selection_ids_ordered() {
            if let Some(entity) = self.scene.entity_mut(id) {
                reset_entity_transform(entity, kind);
            }
        }
        self.mark_dirty();
        self.status = "Reset selected transforms".to_string();
    }

    fn snap_selected(&mut self) {
        let grid = self.config.layout.grid.max(1.0);
        for id in self.selection_ids_ordered() {
            if let Some(entity) = self.scene.entity_mut(id) {
                entity.x = (entity.x / grid).round() * grid;
                entity.y = (entity.y / grid).round() * grid;
            }
        }
        self.mark_dirty();
        self.status = "Snapped selection to grid".to_string();
    }

    fn align_selected(&mut self, kind: AlignKind) {
        let ids = self.selection_ids_ordered();
        if ids.len() < 2 {
            self.status = "Select at least two entities to align".to_string();
            return;
        }
        let items: Vec<(u64, f32, f32, f32, f32)> = ids
            .iter()
            .filter_map(|id| {
                let entity = self.scene.entity(*id)?;
                let world = self.entity_world_transform(*id)?;
                let (w, h) = editor_entity_size(&self.scene, entity, self.preview_root_size());
                Some((*id, world.x, world.y, w * world.scale, h * world.scale))
            })
            .collect();
        if items.len() < 2 {
            return;
        }
        let target = match kind {
            AlignKind::Left => items
                .iter()
                .map(|item| item.1)
                .fold(f32::INFINITY, f32::min),
            AlignKind::CenterX => {
                let min = items
                    .iter()
                    .map(|item| item.1)
                    .fold(f32::INFINITY, f32::min);
                let max = items
                    .iter()
                    .map(|item| item.1 + item.3)
                    .fold(f32::NEG_INFINITY, f32::max);
                (min + max) * 0.5
            }
            AlignKind::Right => items
                .iter()
                .map(|item| item.1 + item.3)
                .fold(f32::NEG_INFINITY, f32::max),
            AlignKind::Top => items
                .iter()
                .map(|item| item.2)
                .fold(f32::INFINITY, f32::min),
            AlignKind::CenterY => {
                let min = items
                    .iter()
                    .map(|item| item.2)
                    .fold(f32::INFINITY, f32::min);
                let max = items
                    .iter()
                    .map(|item| item.2 + item.4)
                    .fold(f32::NEG_INFINITY, f32::max);
                (min + max) * 0.5
            }
            AlignKind::Bottom => items
                .iter()
                .map(|item| item.2 + item.4)
                .fold(f32::NEG_INFINITY, f32::max),
        };
        let updates: Vec<(u64, f32, f32)> = items
            .iter()
            .filter_map(|(id, x, y, w, h)| {
                let (next_x, next_y) = match kind {
                    AlignKind::Left => (target, *y),
                    AlignKind::CenterX => (target - *w * 0.5, *y),
                    AlignKind::Right => (target - *w, *y),
                    AlignKind::Top => (*x, target),
                    AlignKind::CenterY => (*x, target - *h * 0.5),
                    AlignKind::Bottom => (*x, target - *h),
                };
                self.world_origin_to_local_position(*id, next_x, next_y)
                    .map(|(lx, ly)| (*id, lx, ly))
            })
            .collect();
        for (id, x, y) in updates {
            if let Some(entity) = self.scene.entity_mut(id) {
                entity.x = x;
                entity.y = y;
            }
        }
        self.mark_dirty();
        self.status = "Aligned selection".to_string();
    }

    fn select_by_filter<F>(&mut self, message: &str, predicate: F)
    where
        F: Fn(&Self, &Entity) -> bool,
    {
        let ids = self
            .scene
            .entities
            .iter()
            .filter(|entity| predicate(self, entity))
            .map(|entity| entity.id)
            .collect::<Vec<_>>();
        self.select_many(ids, false);
        self.status = format!("{message} ({})", self.selection_count());
    }

    fn select_siblings(&mut self) {
        let Some(id) = self.selected else {
            return;
        };
        let parent = self.scene.entity(id).and_then(|entity| entity.parent);
        let ids = self
            .scene
            .entities
            .iter()
            .filter(|entity| entity.parent == parent)
            .map(|entity| entity.id)
            .collect::<Vec<_>>();
        self.select_many(ids, false);
        self.status = "Selected siblings".to_string();
    }

    fn select_relative(&mut self, offset: isize) {
        if self.scene.entities.is_empty() {
            return;
        }
        let current = self
            .selected
            .and_then(|id| {
                self.scene
                    .entities
                    .iter()
                    .position(|entity| entity.id == id)
            })
            .unwrap_or(0);
        let len = self.scene.entities.len() as isize;
        let next = (current as isize + offset).rem_euclid(len) as usize;
        let id = self.scene.entities[next].id;
        self.select_only(id);
        self.status = format!("Selected {}", self.scene.entities[next].name);
    }

    fn hide_unselected(&mut self) {
        let selected: HashSet<u64> = self.selection_ids_ordered().into_iter().collect();
        if selected.is_empty() {
            return;
        }
        self.hidden_ids = self
            .scene
            .entities
            .iter()
            .filter(|entity| !selected.contains(&entity.id))
            .map(|entity| entity.id)
            .collect();
        self.status = "Hidden unselected entities in Scene view".to_string();
    }

    fn lock_unselected(&mut self) {
        let selected: HashSet<u64> = self.selection_ids_ordered().into_iter().collect();
        if selected.is_empty() {
            return;
        }
        self.locked_ids = self
            .scene
            .entities
            .iter()
            .filter(|entity| !selected.contains(&entity.id))
            .map(|entity| entity.id)
            .collect();
        self.status = "Locked unselected entities".to_string();
    }

    fn toggle_active_selection(&mut self) {
        let ids = self.selection_ids_ordered();
        if ids.is_empty() {
            return;
        }
        let all_active = ids
            .iter()
            .all(|id| self.scene.entity(*id).is_some_and(|entity| entity.enabled));
        for id in ids {
            if let Some(entity) = self.scene.entity_mut(id) {
                entity.enabled = !all_active;
            }
        }
        self.mark_dirty();
        self.status = if all_active {
            "Deactivated selection".to_string()
        } else {
            "Activated selection".to_string()
        };
    }

    fn snap_selected_size(&mut self) {
        let grid = self.config.layout.grid.max(1.0);
        for id in self.selection_ids_ordered() {
            if let Some(entity) = self.scene.entity_mut(id) {
                entity.size_x = ((entity.size_x / grid).round() * grid).max(1.0);
                entity.size_y = ((entity.size_y / grid).round() * grid).max(1.0);
            }
        }
        self.mark_dirty();
        self.status = "Snapped selection size to grid".to_string();
    }

    fn reset_selected_rotation(&mut self) {
        let kind = self.scene.kind;
        for id in self.selection_ids_ordered() {
            if let Some(entity) = self.scene.entity_mut(id) {
                reset_entity_rotation(entity, kind);
            }
        }
        self.mark_dirty();
        self.status = "Reset selected rotation".to_string();
    }

    fn reset_selected_scale(&mut self) {
        let kind = self.scene.kind;
        for id in self.selection_ids_ordered() {
            if let Some(entity) = self.scene.entity_mut(id) {
                reset_entity_scale(entity, kind);
            }
        }
        self.mark_dirty();
        self.status = "Reset selected scale".to_string();
    }

    fn reset_selected_anchors(&mut self) {
        for id in self.selection_ids_ordered() {
            if let Some(entity) = self.scene.entity_mut(id) {
                entity.anchor_x = 0.0;
                entity.anchor_y = 0.0;
            }
        }
        self.mark_dirty();
        self.status = "Reset selected anchors".to_string();
    }

    fn normalize_selected_sizes(&mut self) {
        for id in self.selection_ids_ordered() {
            if let Some(entity) = self.scene.entity_mut(id) {
                if entity.size_x < 0.0 {
                    entity.x += entity.size_x;
                    entity.size_x = entity.size_x.abs();
                }
                if entity.size_y < 0.0 {
                    entity.y += entity.size_y;
                    entity.size_y = entity.size_y.abs();
                }
            }
        }
        self.mark_dirty();
        self.status = "Normalized selected sizes".to_string();
    }

    fn fit_selection_to_window(&mut self) {
        let (root_w, root_h) = self.preview_root_size();
        for id in self.selection_ids_ordered() {
            if let Some(entity) = self.scene.entity_mut(id) {
                entity.x = 0.0;
                entity.y = 0.0;
                entity.size_x = root_w;
                entity.size_y = root_h;
            }
        }
        self.mark_dirty();
        self.status = "Fit selection to default window".to_string();
    }

    fn center_selection_in_window(&mut self) {
        let root_size = self.preview_root_size();
        let updates = self
            .selection_ids_ordered()
            .into_iter()
            .filter_map(|id| {
                let entity = self.scene.entity(id)?;
                let (parent_w, parent_h) = editor_parent_size(&self.scene, entity, root_size);
                let (size_x, size_y) = editor_entity_size(&self.scene, entity, root_size);
                Some((id, (parent_w - size_x) * 0.5, (parent_h - size_y) * 0.5))
            })
            .collect::<Vec<_>>();
        for (id, x, y) in updates {
            if let Some(entity) = self.scene.entity_mut(id) {
                entity.x = x;
                entity.y = y;
            }
        }
        self.mark_dirty();
        self.status = "Centered selection in parent/window".to_string();
    }

    fn move_selection_z(&mut self, mode: ZMove) {
        let ids = self.selection_ids_ordered();
        if ids.is_empty() {
            return;
        }
        match mode {
            ZMove::Front => {
                let max_z = self
                    .scene
                    .entities
                    .iter()
                    .map(|entity| entity.z)
                    .fold(f32::NEG_INFINITY, f32::max);
                for (index, id) in ids.iter().enumerate() {
                    if let Some(entity) = self.scene.entity_mut(*id) {
                        entity.z = max_z + 1.0 + index as f32;
                    }
                }
                self.status = "Brought selection to front".to_string();
            }
            ZMove::Back => {
                let min_z = self
                    .scene
                    .entities
                    .iter()
                    .map(|entity| entity.z)
                    .fold(f32::INFINITY, f32::min);
                for (index, id) in ids.iter().enumerate() {
                    if let Some(entity) = self.scene.entity_mut(*id) {
                        entity.z = min_z - 1.0 - index as f32;
                    }
                }
                self.status = "Sent selection to back".to_string();
            }
            ZMove::Forward => {
                for id in ids {
                    if let Some(entity) = self.scene.entity_mut(id) {
                        entity.z += 1.0;
                    }
                }
                self.status = "Brought selection forward".to_string();
            }
            ZMove::Backward => {
                for id in ids {
                    if let Some(entity) = self.scene.entity_mut(id) {
                        entity.z -= 1.0;
                    }
                }
                self.status = "Sent selection backward".to_string();
            }
        }
        self.mark_dirty();
    }

    fn nudge_selection_z(&mut self, delta: f32) {
        for id in self.selection_ids_ordered() {
            if let Some(entity) = self.scene.entity_mut(id) {
                entity.z += delta;
            }
        }
        self.mark_dirty();
        self.status = format!("Nudged selection Z by {}", format_num(delta));
    }

    fn refresh_project_browser(&mut self) {
        self.image_cache.borrow_mut().clear();
        self.waveform_cache.borrow_mut().clear();
        self.project_directory_cache.borrow_mut().clear();
        self.script_schema_cache.clear();
        self.bin_scroll = 0.0;
        self.status = "Refreshed project browser and editor caches".to_string();
    }

    fn frame_all(&mut self) {
        let previous = self.selection_ids_ordered();
        let visible = self
            .scene
            .entities
            .iter()
            .filter(|entity| !self.hidden_ids.contains(&entity.id))
            .map(|entity| entity.id)
            .collect();
        self.select_many(visible, false);
        self.frame_selected();
        self.select_many(previous, false);
        self.status = "Framed all visible entities".to_string();
    }

    fn zoom_100(&mut self) {
        let area = self.last_viewport;
        if area.w <= 0.0 {
            return;
        }
        let cx = (area.w * 0.5 - self.cam_x) / self.cam_zoom;
        let cy = (area.h * 0.5 - self.cam_y) / self.cam_zoom;
        self.cam_zoom = 1.0;
        self.cam_x = area.w * 0.5 - cx;
        self.cam_y = area.h * 0.5 - cy;
        self.status = "Scene view zoom 100%".to_string();
    }

    // ---- Frame -------------------------------------------------------------

    pub fn frame(&mut self, ui: &mut Ui) {
        self.world_transform_cache.borrow_mut().clear();
        self.world_model_3d_cache.borrow_mut().clear();
        self.prune_selection();
        let w = ui.painter.width();
        let h = ui.painter.height();
        ui.painter.clear(self.config.theme.panel_alt);

        // Popups take input precedence: while one is open the background UI sees
        // no left/right press this frame.
        let raw_left = ui.input.mouse_pressed;
        let raw_right = ui.input.right_pressed;
        // A popup only reacts to clicks if it already existed at the start of
        // this frame; otherwise the very click that opened it would register as
        // an "outside click" and close it instantly.
        let popup_interactive = self.popup.is_some();
        if self.popup.is_some() {
            ui.input.mouse_pressed = false;
            ui.input.right_pressed = false;
        }

        // Global shortcuts (only when no text field is focused).
        if !ui.has_focus() && self.popup.is_none() {
            if ui.input.undo {
                self.undo();
            }
            if ui.input.redo {
                self.redo();
            }
            if ui.input.copy {
                if let Some(id) = self.selected {
                    self.copy_entity(id);
                }
            }
            if ui.input.paste {
                self.paste_entity();
            }
            if ui.input.duplicate {
                self.duplicate_selection();
            }
            if ui.input.save {
                self.save();
            }
            if ui.input.focus_selection {
                self.frame_selected();
            }
            if ui.input.reset_view {
                self.reset_view();
            }
            if ui.input.select_all {
                self.select_all();
            }
            if ui.input.invert_selection {
                self.invert_selection();
            }
            if ui.input.group_selection {
                self.group_selected();
            }
            if ui.input.unparent_selection {
                self.unparent_selected();
            }
            if ui.input.hide_selection {
                self.hide_selected();
            }
            if ui.input.show_all {
                self.hidden_ids.clear();
                self.status = "Revealed all Scene-view objects".to_string();
            }
            if ui.input.lock_selection {
                self.lock_selected();
            }
            if ui.input.unlock_all {
                self.locked_ids.clear();
                self.status = "Unlocked all Scene-view objects".to_string();
            }
            if ui.input.frame_all {
                self.frame_all();
            }
            if ui.input.maximize_view {
                self.maximize_view = !self.maximize_view;
            }
            if ui.input.toggle_grid {
                self.config.layout.show_grid = !self.config.layout.show_grid;
                self.dirty = true;
            }
            if ui.input.toggle_snap {
                self.config.layout.snap = !self.config.layout.snap;
                self.dirty = true;
            }
            if ui.input.rename {
                if let Some(id) = self.selected {
                    let cur = self
                        .scene
                        .entity(id)
                        .map(|e| e.name.clone())
                        .unwrap_or_default();
                    self.open_prompt("Rename entity", Pending::RenameEntity(id), &cur);
                }
            }
            // Arrow-key nudge.
            if (ui.input.nudge_x != 0.0 || ui.input.nudge_y != 0.0) && self.selected.is_some() {
                let step = if ui.input.nudge_big {
                    self.config.layout.grid.max(1.0)
                } else {
                    1.0
                };
                for id in self.selection_ids_ordered() {
                    if self.locked_ids.contains(&id) {
                        continue;
                    }
                    if let Some(e) = self.scene.entity_mut(id) {
                        e.x += ui.input.nudge_x * step;
                        e.y += ui.input.nudge_y * step;
                    }
                }
                self.mark_dirty();
            }
            if ui.input.delete {
                let ids = self.selection_ids_ordered();
                if !ids.is_empty() {
                    for id in ids {
                        self.scene.remove_entity(id);
                        self.hidden_ids.remove(&id);
                        self.locked_ids.remove(&id);
                        self.hierarchy_collapsed.remove(&id);
                    }
                    self.clear_selection();
                    self.mark_dirty();
                    self.status = "Deleted selection".to_string();
                }
            }
            if ui.input.escape {
                self.clear_selection();
            }
        }

        // Mouse back/forward navigate the project browser history.
        if ui.input.back_pressed {
            self.bin_back();
        }
        if ui.input.forward_pressed {
            self.bin_forward();
        }

        self.toolbar(ui, w);

        let body_top = TOOLBAR_H;
        let body_total = (h - TOOLBAR_H - STATUS_H).max(0.0);
        let bin_h = if self.maximize_view
            || !self.config.layout.show_project
            || self.config.layout.undock_project
        {
            0.0
        } else {
            self.config
                .layout
                .bin_h
                .clamp(0.0, (body_total - 120.0).max(0.0))
        };
        let bin_gap = if bin_h > 0.0 { SPLIT_HALF * 2.0 } else { 0.0 };
        let body_h = (body_total - bin_h - bin_gap).max(0.0);
        let bin_split_y = body_top + body_h;
        let bin_rect = Rect::new(0.0, bin_split_y + bin_gap, w, bin_h);

        let left_panels = if self.maximize_view {
            Vec::new()
        } else {
            self.panels_on(Side::Left)
        };
        let right_panels = if self.maximize_view {
            Vec::new()
        } else {
            self.panels_on(Side::Right)
        };
        let max_col = (w - MIN_VIEWPORT_W).max(0.0);
        let mut left_w = if left_panels.is_empty() {
            0.0
        } else {
            self.config.layout.left_w
        };
        let mut right_w = if right_panels.is_empty() {
            0.0
        } else {
            self.config.layout.right_w
        };
        if left_w > 0.0 {
            left_w = clamp_range(left_w, MIN_PANEL_W.min(max_col), (max_col * 0.6).max(0.0));
        }
        if right_w > 0.0 {
            right_w = clamp_range(right_w, MIN_PANEL_W.min(max_col), (max_col * 0.6).max(0.0));
        }
        if left_w + right_w > max_col {
            let total = left_w + right_w;
            if total > 0.0 {
                let scale = max_col / total;
                left_w *= scale;
                right_w *= scale;
            }
        }

        let viewport = Rect::new(left_w, body_top, (w - left_w - right_w).max(0.0), body_h);
        let left_col = Rect::new(0.0, body_top, left_w, body_h);
        let right_col = Rect::new(w - right_w, body_top, right_w, body_h);

        // Splitters first so a press on a handle is consumed before panels.
        let on_splitter = self.handle_splitters(
            ui,
            left_col,
            right_col,
            &left_panels,
            &right_panels,
            w,
            bin_split_y,
            body_total,
        );
        if on_splitter {
            ui.input.mouse_pressed = false;
        }

        self.viewport(ui, viewport);
        if !left_panels.is_empty() {
            self.render_column(ui, left_col, &left_panels, Side::Left);
        }
        if !right_panels.is_empty() {
            self.render_column(ui, right_col, &right_panels, Side::Right);
        }
        if bin_h > 0.0 {
            self.project_bin(ui, bin_rect);
        }
        if self.script_drag.is_some() && !ui.input.mouse_down {
            self.script_drag = None;
        }
        if self.mesh_drag.is_some() && !ui.input.mouse_down {
            self.mesh_drag = None;
        }
        // A successful Inspector drop consumes these states while drawing the
        // target field. Any remaining drag released elsewhere is cancelled.
        if !ui.input.mouse_down {
            self.inspector_reference_drag = None;
            self.reparent_drag = None;
        }

        self.status_bar(ui, w, h);
        self.document_tabs(ui, w, h - STATUS_H);

        // Restore the real press for popup handling, then render the overlay.
        ui.input.mouse_pressed = raw_left;
        ui.input.right_pressed = raw_right;
        self.handle_popup(ui, w, h, popup_interactive);

        self.commit_undo_if_settled(ui);
        if self.config.settings.show_tooltips {
            ui.draw_tooltip();
        }
    }

    fn panels_on(&self, side: Side) -> Vec<Panel> {
        let mut panels = Vec::new();
        if self.config.layout.show_hierarchy
            && !self.config.layout.undock_hierarchy
            && self.config.layout.hierarchy_side == side
        {
            panels.push(Panel::Hierarchy);
        }
        if self.config.layout.show_inspector
            && !self.config.layout.undock_inspector
            && self.config.layout.inspector_side == side
        {
            panels.push(Panel::Inspector);
        }
        panels
    }

    // ---- Toolbar -----------------------------------------------------------

    fn toolbar(&mut self, ui: &mut Ui, w: f32) {
        ui.painter
            .fill_rect(Rect::new(0.0, 0.0, w, TOOLBAR_H), self.config.theme.toolbar);
        ui.painter
            .stroke_rect(Rect::new(0.0, 0.0, w, TOOLBAR_H), self.config.theme.border);

        let y = 6.0;
        let bh = TOOLBAR_H - 12.0;
        let mut x = 8.0;

        // Keep the top bar focused on high-frequency work. Scene lifecycle,
        // export, build, mobile and settings actions live in one compact menu.
        let scene_menu = Rect::new(x, y, 30.0, bh);
        if ui.icon_toggle(scene_menu, icon::FOLDER_OPEN, false, self.config.theme.text) {
            self.open_scene_menu(scene_menu.x, scene_menu.bottom() + 2.0);
        }
        ui.tooltip(scene_menu, "Scene, project and build actions");
        x += 35.0;

        let dirty_mark = if self.scene_dirty { " •" } else { "" };
        let scene_label = format!("{}{dirty_mark}", self.scene.name);
        let scene_width = (ui.painter.text_width(&scene_label, 13.0) + 34.0)
            .clamp(96.0, 200.0)
            .min((w - 520.0).max(96.0));
        let scene_rect = Rect::new(x, y, scene_width, bh);
        if ui.icon_button(scene_rect, icon::EDIT, &scene_label) {
            self.open_prompt(
                "Rename scene",
                Pending::RenameScene,
                &self.scene.name.clone(),
            );
        }
        ui.tooltip(scene_rect, "Rename the active scene");
        x += scene_width + 5.0;

        let save = Rect::new(x, y, 30.0, bh);
        if ui.icon_toggle(save, icon::SAVE, false, self.config.theme.text) {
            self.save();
        }
        ui.tooltip(save, "Save scene (Ctrl+S)");
        x += 35.0;

        let run_width = ui.painter.text_width("Run", 14.0) + 31.0;
        let run = Rect::new(x, y, run_width, bh);
        if ui.icon_button(run, icon::PLAY, "Run") {
            self.run_scene();
        }
        ui.tooltip(run, "Run the current project");
        x += run_width + 5.0;

        let add_entity = Rect::new(x, y, 30.0, bh);
        if ui.icon_toggle(add_entity, icon::ADD_CIRCLE, false, self.config.theme.text) {
            self.add_entity(None);
        }
        ui.tooltip(add_entity, "Add entity");
        x += 40.0;

        ui.painter.fill_rect(
            Rect::new(x - 5.0, y + 3.0, 1.0, bh - 6.0),
            self.config.theme.border,
        );
        for (tool, glyph, tip) in [
            (ViewTool::Move, icon::OPEN_WITH, "Move tool"),
            (ViewTool::Scale, icon::ASPECT_RATIO, "Scale tool"),
            (ViewTool::Rotate, icon::ROTATE_RIGHT, "Rotate tool"),
            (ViewTool::Transform, icon::TRANSFORM, "Transform tool"),
        ] {
            let rect = Rect::new(x, y, 30.0, bh);
            if ui.icon_toggle(
                rect,
                glyph,
                self.config.layout.view_tool == tool,
                self.config.theme.text,
            ) {
                self.config.layout.view_tool = tool;
                self.dirty = true;
            }
            ui.tooltip(rect, tip);
            x += 32.0;
        }
        x += 8.0;
        // Snap + grid toggles.
        let snap = self.config.layout.snap;
        let snap_glyph = if snap { icon::GRID_ON } else { icon::GRID_OFF };
        let sr = Rect::new(x, y, 30.0, bh);
        if ui.icon_toggle(sr, snap_glyph, snap, self.config.theme.text) {
            self.config.layout.snap = !snap;
            self.dirty = true;
        }
        ui.tooltip(sr, "Snap to grid");
        x += 35.0;
        let show_grid = self.config.layout.show_grid;
        let gr = Rect::new(x, y, 30.0, bh);
        if ui.icon_toggle(gr, icon::BORDER_ALL, show_grid, self.config.theme.text) {
            self.config.layout.show_grid = !show_grid;
            self.dirty = true;
        }
        ui.tooltip(gr, "Show grid");
        x += 35.0;

        if w >= 900.0 {
            let grid_field = Rect::new(x, y, 46.0, bh);
            let grid_str = format_num(self.config.layout.grid);
            let r = ui.text_field("grid_size", grid_field, &grid_str);
            if r.changed {
                if let Ok(v) = r.text.trim().parse::<f32>() {
                    self.config.layout.grid = v.clamp(1.0, 512.0);
                    self.dirty = true;
                }
            }
            ui.tooltip(grid_field, "Grid size");
            x += 50.0;
        }

        if w >= 820.0 {
            let cam_rect = Rect::new(x, y, 30.0, bh);
            if ui.icon_toggle(cam_rect, icon::MY_LOCATION, false, self.config.theme.text) {
                self.reset_view();
                self.status = "Camera reset to (0, 0)".to_string();
            }
            ui.tooltip(cam_rect, "Reset camera to origin (0)");
            x += 35.0;
        }

        // Compact Unity-style utility menu for selection, hierarchy, arrange,
        // and Scene-view commands without crowding the main toolbar.
        let tools_rect = Rect::new(x, y, 30.0, bh);
        if ui.icon_toggle(tools_rect, icon::MORE_VERT, false, self.config.theme.text) {
            self.open_tools_menu(tools_rect.x, tools_rect.bottom() + 2.0);
        }
        ui.tooltip(tools_rect, "Editor tools and layout");
        x += 35.0;

        let window_rect = Rect::new(x, y, 30.0, bh);
        if ui.icon_toggle(window_rect, icon::VIEW_QUILT, false, self.config.theme.text) {
            self.open_window_menu(window_rect.x, window_rect.bottom() + 2.0);
        }
        ui.tooltip(window_rect, "Window panels and project window settings");
    }

    fn document_tabs(&mut self, ui: &mut Ui, w: f32, y: f32) {
        if self.documents.len() <= 1 {
            return;
        }
        let bar = Rect::new(0.0, y, w, STATUS_H);
        ui.painter.fill_rect(bar, self.config.theme.header);
        ui.painter.fill_rect(
            Rect::new(bar.x, bar.y, bar.w, 1.0),
            self.config.theme.border,
        );
        let mut x = 6.0;
        let mut activate = None;
        let mut close = None;
        for (index, document) in self.documents.iter().enumerate() {
            let active = index == self.active_document;
            let dirty = if active {
                self.scene_dirty
            } else {
                document.dirty
            };
            let kind = if document.kind == DocumentKind::Prefab {
                "◆"
            } else {
                ""
            };
            let label = format!(
                "{kind}{}{}",
                document.scene.name,
                if dirty { " •" } else { "" }
            );
            let width = (ui.painter.text_width(&label, 13.0) + 44.0).clamp(94.0, 220.0);
            let tab = Rect::new(x, y + 3.0, width, STATUS_H - 6.0);
            let hovered = tab.contains(ui.input.mouse_x, ui.input.mouse_y);
            if active || hovered {
                let fill = if active {
                    self.config.theme.panel
                } else {
                    self.config.theme.panel_alt
                };
                ui.painter.fill_round_rect(tab, 4.0, fill);
                ui.painter
                    .stroke_round_rect(tab, 4.0, self.config.theme.border);
            }
            if active {
                ui.painter.fill_round_rect(
                    Rect::new(tab.x + 5.0, tab.bottom() - 2.0, tab.w - 10.0, 2.0),
                    1.0,
                    self.config.theme.accent,
                );
            }
            ui.painter.text_clipped(
                tab.x + 12.0,
                tab.y + (tab.h - 13.0) * 0.5,
                &label,
                13.0,
                if active {
                    self.config.theme.text
                } else {
                    self.config.theme.text_dim
                },
                (tab.w - 42.0).max(0.0),
            );
            let close_rect = Rect::new(tab.right() - 25.0, tab.y, 24.0, tab.h);
            if active || hovered {
                ui.icon(
                    close_rect.x + close_rect.w * 0.5,
                    close_rect.y + close_rect.h * 0.5,
                    icon::CLOSE,
                    15.0,
                    if close_rect.contains(ui.input.mouse_x, ui.input.mouse_y) {
                        self.config.theme.text
                    } else {
                        self.config.theme.text_dim
                    },
                );
            }
            if ui.input.mouse_pressed && close_rect.contains(ui.input.mouse_x, ui.input.mouse_y) {
                close = Some(index);
            } else if ui.input.middle_pressed && hovered {
                close = Some(index);
            } else if ui.input.mouse_pressed && hovered {
                activate = Some(index);
            }
            x += width + 3.0;
            if x >= w - 20.0 {
                break;
            }
        }
        if let Some(index) = close {
            self.request_close_document(index);
        } else if let Some(index) = activate {
            self.switch_document(index);
        }
    }

    // ---- Hierarchy ---------------------------------------------------------

    fn render_column(&mut self, ui: &mut Ui, col: Rect, panels: &[Panel], side: Side) {
        match panels.len() {
            0 => {}
            1 => self.render_panel(ui, col, panels[0]),
            _ => {
                let ratio = match side {
                    Side::Left => self.config.layout.left_split,
                    Side::Right => self.config.layout.right_split,
                }
                .clamp(0.15, 0.85);
                let top_h = (col.h * ratio).round();
                let top = Rect::new(col.x, col.y, col.w, top_h - SPLIT_HALF);
                let bottom = Rect::new(
                    col.x,
                    col.y + top_h + SPLIT_HALF,
                    col.w,
                    (col.h - top_h - SPLIT_HALF).max(0.0),
                );
                self.render_panel(ui, top, panels[0]);
                self.render_panel(ui, bottom, panels[1]);
            }
        }
    }

    fn render_panel(&mut self, ui: &mut Ui, area: Rect, panel: Panel) {
        ui.painter.fill_rect(area, self.config.theme.panel);
        let header = Rect::new(area.x, area.y, area.w, HEADER_H);
        ui.painter.fill_rect(header, self.config.theme.header);
        let (glyph, title, side) = match panel {
            Panel::Hierarchy => (
                icon::ACCOUNT_TREE,
                "Hierarchy",
                self.config.layout.hierarchy_side,
            ),
            Panel::Inspector => (icon::TUNE, "Inspector", self.config.layout.inspector_side),
        };
        let is_undocked = match panel {
            Panel::Hierarchy => self.config.layout.undock_hierarchy,
            Panel::Inspector => self.config.layout.undock_inspector,
        };
        ui.icon(
            area.x + 16.0,
            area.y + HEADER_H / 2.0,
            glyph,
            16.0,
            self.config.theme.text,
        );
        ui.painter.text_clipped(
            area.x + 30.0,
            area.y + (HEADER_H - 14.0) / 2.0,
            title,
            14.0,
            self.config.theme.text,
            (area.w - 110.0).max(0.0),
        );
        let close = Rect::new(area.right() - 26.0, area.y + 3.0, 20.0, HEADER_H - 6.0);
        ui.tooltip(close, "Close panel");
        if ui.icon_toggle(close, icon::DELETE, false, self.config.theme.text_dim) {
            match panel {
                Panel::Hierarchy => {
                    self.config.layout.show_hierarchy = false;
                    self.config.layout.undock_hierarchy = false;
                }
                Panel::Inspector => {
                    self.config.layout.show_inspector = false;
                    self.config.layout.undock_inspector = false;
                }
            }
            self.dirty = true;
        }
        let swap = Rect::new(area.right() - 50.0, area.y + 3.0, 20.0, HEADER_H - 6.0);
        if !is_undocked {
            ui.tooltip(swap, "Dock to other side");
            if ui.icon_toggle(swap, icon::SWAP, false, self.config.theme.text_dim) {
                match panel {
                    Panel::Hierarchy => self.config.layout.hierarchy_side = side.toggled(),
                    Panel::Inspector => self.config.layout.inspector_side = side.toggled(),
                }
                self.dirty = true;
            }
        }
        let undock = Rect::new(area.right() - 74.0, area.y + 3.0, 20.0, HEADER_H - 6.0);
        ui.tooltip(
            undock,
            if is_undocked {
                "Dock back into main window"
            } else {
                "Undock to separate window"
            },
        );
        if ui.icon_toggle(undock, icon::OPEN_IN_NEW, false, self.config.theme.text_dim) {
            if is_undocked {
                match panel {
                    Panel::Hierarchy => self.dock_widget(EditorWidget::Hierarchy),
                    Panel::Inspector => self.dock_widget(EditorWidget::Inspector),
                }
            } else {
                match panel {
                    Panel::Hierarchy => self.config.layout.undock_hierarchy = true,
                    Panel::Inspector => self.config.layout.undock_inspector = true,
                }
                self.dirty = true;
            }
        }
        ui.painter.stroke_rect(area, self.config.theme.border);

        let content = Rect::new(
            area.x,
            area.y + HEADER_H,
            area.w,
            (area.h - HEADER_H).max(0.0),
        );
        let (scroll, content_h) = match panel {
            Panel::Hierarchy => (&mut self.hierarchy_scroll, self.hierarchy_content_h),
            Panel::Inspector => (&mut self.inspector_scroll, self.inspector_content_h),
        };
        if content.contains(ui.input.mouse_x, ui.input.mouse_y) && ui.input.scroll != 0.0 {
            *scroll -= ui.input.scroll * 32.0;
            ui.wants_redraw = true;
        }
        let max_scroll = (content_h - content.h).max(0.0);
        *scroll = scroll.clamp(0.0, max_scroll);
        let scroll_value = *scroll;

        let prev_clip = ui.painter.push_clip(content);
        ui.set_input_clip(content);
        let start_y = content.y + PAD - scroll_value;
        let consumed = match panel {
            Panel::Hierarchy => self.hierarchy_content(ui, content, start_y),
            Panel::Inspector => self.inspector_content(ui, content, start_y),
        };
        ui.reset_input_clip();
        ui.painter.set_clip_raw(prev_clip);

        let total = consumed - (content.y - scroll_value);
        match panel {
            Panel::Hierarchy => self.hierarchy_content_h = total,
            Panel::Inspector => self.inspector_content_h = total,
        }
        if total > content.h {
            let thumb_h = (content.h * (content.h / total)).max(20.0);
            let thumb_y = content.y + (scroll_value / (total - content.h)) * (content.h - thumb_h);
            ui.painter.fill_round_rect(
                Rect::new(content.right() - 6.0, thumb_y, 4.0, thumb_h),
                2.0,
                self.config.theme.text_dim,
            );
        }
    }

    fn hierarchy_content(&mut self, ui: &mut Ui, area: Rect, start_y: f32) -> f32 {
        let mut y = start_y;

        // Search box.
        let filter = self.hierarchy_filter.clone();
        ui.icon(
            area.x + 12.0,
            y + FIELD_H / 2.0,
            icon::SEARCH,
            14.0,
            self.config.theme.text_dim,
        );
        let resp = ui.text_field(
            "hier_filter",
            Rect::new(area.x + 22.0, y, area.w - 30.0, FIELD_H),
            &filter,
        );
        if resp.changed {
            self.hierarchy_filter = resp.text;
        }
        y += FIELD_H + 6.0;
        let query = self.hierarchy_filter.trim().to_lowercase();

        if self.scene.entities.is_empty() {
            ui.painter.text_clipped(
                area.x + PAD,
                y,
                "No entities.",
                14.0,
                self.config.theme.text_dim,
                (area.w - PAD * 2.0).max(0.0),
            );
            ui.painter.text_clipped(
                area.x + PAD,
                y + 18.0,
                "Right-click or use + Entity.",
                14.0,
                self.config.theme.text_dim,
                (area.w - PAD * 2.0).max(0.0),
            );
            if area.contains(ui.input.mouse_x, ui.input.mouse_y) && ui.input.right_pressed {
                self.open_hierarchy_empty_menu(ui.input.mouse_x, ui.input.mouse_y);
            }
            return y + 40.0;
        }

        if !query.is_empty() {
            // Filtered: flat list of matching entities, no tree.
            let ids: Vec<u64> = self
                .scene
                .entities
                .iter()
                .filter(|e| e.name.to_lowercase().contains(&query))
                .map(|e| e.id)
                .collect();
            if ids.is_empty() {
                ui.painter.text_clipped(
                    area.x + PAD,
                    y,
                    "No matches.",
                    14.0,
                    self.config.theme.text_dim,
                    (area.w - PAD * 2.0).max(0.0),
                );
                return y + 24.0;
            }
            for id in ids {
                y = self.hierarchy_node(ui, area, id, 0, y);
            }
            return y + PAD;
        }

        // Render the tree depth-first.
        let roots = self.scene.children_of(None);
        for id in roots {
            y = self.hierarchy_node(ui, area, id, 0, y);
        }

        // Drop onto empty space => unparent.
        if self.reparent_drag.is_some()
            && !ui.input.mouse_down
            && area.contains(ui.input.mouse_x, ui.input.mouse_y)
        {
            if let Some(drag) = self.reparent_drag.take() {
                if ui.input.mouse_y > y {
                    if let Some(e) = self.scene.entity_mut(drag) {
                        e.parent = None;
                    }
                    self.mark_dirty();
                }
            }
        }

        // Right-click empty area below the rows.
        let empty = Rect::new(area.x, y, area.w, (area.bottom() - y).max(0.0));
        if empty.contains(ui.input.mouse_x, ui.input.mouse_y) && ui.input.right_pressed {
            self.open_hierarchy_empty_menu(ui.input.mouse_x, ui.input.mouse_y);
        }
        y + PAD
    }

    fn hierarchy_node(&mut self, ui: &mut Ui, area: Rect, id: u64, depth: u32, y: f32) -> f32 {
        let mut y = y;
        let name = self
            .scene
            .entity(id)
            .map(|e| e.name.clone())
            .unwrap_or_default();
        let has_children = !self.scene.children_of(Some(id)).is_empty();
        let collapsed = self.hierarchy_collapsed.contains(&id);
        let indent = 6.0 + depth as f32 * 14.0;
        let row = Rect::new(area.x + 2.0, y, area.w - 4.0, ROW_H);
        let selected = self.is_selected(id);

        // Reparent drop indicator: highlight when dragging over this row.
        let hovering = row.contains(ui.input.mouse_x, ui.input.mouse_y);
        if self.reparent_drag.is_some() && self.reparent_drag != Some(id) && hovering {
            ui.painter.stroke_rect(row, self.config.theme.accent);
        }
        if self.script_drag.is_some() && hovering {
            ui.painter
                .stroke_round_rect(row.shrink(1.0), 3.0, self.config.theme.accent);
        }
        if self.mesh_drag.is_some() && hovering {
            ui.painter
                .stroke_round_rect(row.shrink(1.0), 3.0, [92, 180, 240, 255]);
        }
        if matches!(
            self.inspector_reference_drag,
            Some(InspectorReferenceDrag::Component(_))
        ) && hovering
        {
            ui.painter
                .stroke_round_rect(row.shrink(1.0), 3.0, self.config.theme.selection);
            // Keep carrying the source component, but inspect the hovered
            // entity so one continuous drag can reach its reference field.
            if ui.input.mouse_down && self.selected != Some(id) {
                self.select_only(id);
            }
        }

        let enabled = self.scene.entity(id).map(|e| e.enabled).unwrap_or(true);
        let hidden = self.hidden_ids.contains(&id);
        let locked = self.locked_ids.contains(&id);
        let eye = Rect::new(row.right() - 22.0, y + 3.0, 18.0, ROW_H - 6.0);
        let lock = Rect::new(row.right() - 42.0, y + 3.0, 18.0, ROW_H - 6.0);
        let fold = Rect::new(area.x + indent - 10.0, y + 3.0, 18.0, ROW_H - 6.0);
        let eye_hit = eye.contains(ui.input.mouse_x, ui.input.mouse_y);
        let lock_hit = lock.contains(ui.input.mouse_x, ui.input.mouse_y);
        let fold_hit = has_children && fold.contains(ui.input.mouse_x, ui.input.mouse_y);
        let clicked = ui.list_row(row, &name, selected, indent);
        if !enabled || hidden {
            // Dim disabled rows like Unity greys out inactive objects.
            let d = self.config.theme.panel;
            ui.painter.fill_rect(row, [d[0], d[1], d[2], 130]);
        }
        if has_children {
            ui.icon(
                area.x + indent - 2.0,
                y + ROW_H / 2.0,
                if collapsed {
                    icon::CHEVRON_RIGHT
                } else {
                    icon::EXPAND_MORE
                },
                14.0,
                self.config.theme.text_dim,
            );
        }
        let eye_glyph = if hidden {
            icon::VISIBILITY_OFF
        } else {
            icon::VISIBILITY
        };
        ui.icon(
            lock.x + lock.w / 2.0,
            lock.y + lock.h / 2.0,
            if locked { icon::LOCK } else { icon::LOCK_OPEN },
            13.0,
            self.config.theme.text_dim,
        );
        ui.icon(
            eye.x + eye.w / 2.0,
            eye.y + eye.h / 2.0,
            eye_glyph,
            14.0,
            self.config.theme.text_dim,
        );
        ui.tooltip(
            eye,
            if hidden {
                "Show in Scene view"
            } else {
                "Hide in Scene view"
            },
        );
        ui.tooltip(
            lock,
            if locked {
                "Enable Scene picking"
            } else {
                "Disable Scene picking"
            },
        );
        if fold_hit && ui.input.mouse_pressed {
            if collapsed {
                self.hierarchy_collapsed.remove(&id);
            } else {
                self.hierarchy_collapsed.insert(id);
            }
        } else if eye_hit && ui.input.mouse_pressed {
            if hidden {
                self.hidden_ids.remove(&id);
            } else {
                self.hidden_ids.insert(id);
                self.selected_ids.remove(&id);
                if self.selected == Some(id) {
                    self.selected = None;
                    self.selected = self.selection_ids_ordered().into_iter().next();
                }
            }
        } else if lock_hit && ui.input.mouse_pressed {
            if locked {
                self.locked_ids.remove(&id);
            } else {
                self.locked_ids.insert(id);
            }
        } else if clicked {
            let inspector_owner = self.selected;
            self.select_with_modifiers(id, ui);
            self.reparent_drag = Some(id);
            self.inspector_reference_drag = Some(InspectorReferenceDrag::Entity {
                id,
                inspector_owner,
            });
        }
        if hovering && ui.input.right_pressed {
            if !self.is_selected(id) {
                self.select_only(id);
            }
            self.open_entity_menu(id, ui.input.mouse_x, ui.input.mouse_y);
        }

        if !ui.input.mouse_down && hovering {
            if let Some(path) = self.script_drag.take() {
                self.add_script_component_from_path(id, &path);
            }
            if let Some(path) = self.mesh_drag.take() {
                self.assign_mesh_to_entity(id, &path);
            }
        }

        // Complete a reparent when the drag is released over another row.
        if let Some(drag) = self.reparent_drag {
            if !ui.input.mouse_down && drag != id && hovering {
                if !self.scene.would_cycle(drag, id) {
                    if let Some(e) = self.scene.entity_mut(drag) {
                        e.parent = Some(id);
                    }
                    self.mark_dirty();
                    self.status = format!("Reparented to {name}");
                }
                self.reparent_drag = None;
            }
        }

        y += ROW_H + 1.0;
        if !collapsed {
            for child in self.scene.children_of(Some(id)) {
                y = self.hierarchy_node(ui, area, child, depth + 1, y);
            }
        }
        y
    }

    // ---- Viewport ----------------------------------------------------------

    fn viewport(&mut self, ui: &mut Ui, area: Rect) {
        if area.w <= 0.0 {
            return;
        }
        // Keep the mature 2D viewport below byte-for-byte in behavior. 3D
        // scenes have independent navigation, projection, picking, and gizmos.
        if self.scene.kind == SceneKind::ThreeD {
            self.viewport_3d(ui, area);
            return;
        }
        self.last_viewport = area;
        let prev = ui.painter.push_clip(area);
        ui.set_input_clip(area);

        let inside = area.contains(ui.input.mouse_x, ui.input.mouse_y);

        // Middle-mouse pan, anchored so the camera tracks the cursor exactly
        // instead of jumping by accumulated hover movement.
        let camera_sensitivity = self
            .config
            .settings
            .viewport_camera_sensitivity
            .clamp(0.05, 8.0);
        if ui.input.middle_down && (inside || self.pan_anchor.is_some()) {
            let (mx0, my0, cx0, cy0) = *self.pan_anchor.get_or_insert((
                ui.input.mouse_x,
                ui.input.mouse_y,
                self.cam_x,
                self.cam_y,
            ));
            self.cam_x = cx0 + (ui.input.mouse_x - mx0) * camera_sensitivity;
            self.cam_y = cy0 + (ui.input.mouse_y - my0) * camera_sensitivity;
            ui.wants_redraw = true;
        } else {
            self.pan_anchor = None;
        }
        // Scroll-wheel zoom, anchored at the cursor.
        if inside && ui.input.scroll != 0.0 {
            let old = self.cam_zoom;
            let zoom_delta = (ui.input.scroll * 0.12 * camera_sensitivity).clamp(-0.9, 4.0);
            let new = (old * (1.0 + zoom_delta)).clamp(0.2, 5.0);
            let wx = (ui.input.mouse_x - (area.x + self.cam_x)) / old;
            let wy = (ui.input.mouse_y - (area.y + self.cam_y)) / old;
            self.cam_x = ui.input.mouse_x - area.x - wx * new;
            self.cam_y = ui.input.mouse_y - area.y - wy * new;
            self.cam_zoom = new;
            ui.wants_redraw = true;
        }

        ui.painter.fill_rect(area, self.config.theme.viewport_bg);
        let [br, bg, bb, _] = self.scene.background;
        let bg_frame = area.shrink(1.0);
        ui.painter.fill_rect(bg_frame, [br, bg, bb, 255]);
        if self.config.layout.show_grid {
            self.draw_grid(ui, bg_frame);
        }
        if self.config.settings.show_window_bounds {
            self.draw_window_bounds(ui, bg_frame);
        }

        let z = self.cam_zoom;

        // Gather the scene's lighting, then composite it over the finished scene
        // per pixel — exactly as the runtime multiplies its framebuffer by the
        // light map. This reveals falloff, colored light pools, and occluder
        // shadows across the background and every object, instead of laying a
        // flat tint on each object's primary color.
        let scene_lighting = self.gather_scene_lighting();

        // Draw entities sorted by z (lower first).
        let mut entity_order = (0..self.scene.entities.len()).collect::<Vec<_>>();
        entity_order.sort_by(|left, right| {
            compare_editor_entity_order(&self.scene.entities[*left], &self.scene.entities[*right])
        });
        for &index in &entity_order {
            let entity = &self.scene.entities[index];
            if self.hidden_ids.contains(&entity.id) {
                continue;
            }
            let Some(rect) = self.entity_screen_rect(entity, area) else {
                continue;
            };
            let active = self.scene.is_active_in_tree(entity.id);
            self.draw_entity(ui, entity, rect, z);
            if !active {
                // Dim inactive entities, like Unity greys out disabled objects.
                ui.painter.fill_rect(rect, [30, 30, 30, 150]);
            }
        }

        // Light the scene now that the background and all objects are drawn, so
        // the composite matches the runtime's deferred pass. Selection gizmos,
        // handles, and the HUD stay above it — editor chrome the game never sees.
        if let Some((config, lights, occluders)) = scene_lighting.as_ref() {
            self.composite_preview_lighting(ui, area, config, lights, occluders);
        }

        for &index in &entity_order {
            let entity = &self.scene.entities[index];
            if self.hidden_ids.contains(&entity.id) || !self.is_selected(entity.id) {
                continue;
            }
            let Some(rect) = self.entity_screen_rect(entity, area) else {
                continue;
            };
            let angle = self.entity_world_rotation(entity);
            // Outline and collider preview rotate with the entity.
            let prev_rot = ui.painter.push_rotation(rect.x, rect.y, angle);
            ui.painter
                .stroke_rect(rect.shrink(-1.0), self.config.theme.selection);
            let world_scale = self
                .entity_world_transform(entity.id)
                .map(|transform| transform.scale)
                .unwrap_or_else(|| editor_entity_scale(entity));
            self.draw_collider_preview(ui, entity, rect, z, world_scale);
            ui.painter.set_rotation_raw(prev_rot);

            if self.selected != Some(entity.id) || self.locked_ids.contains(&entity.id) {
                continue;
            }

            let (mx, my) = (ui.input.mouse_x, ui.input.mouse_y);
            match self.config.layout.view_tool {
                ViewTool::Move => {
                    self.draw_move_handle(ui, rect, angle, mx, my);
                }
                ViewTool::Scale => {
                    self.draw_scale_handles(ui, rect, angle, mx, my);
                }
                ViewTool::Rotate => {
                    let (kx, ky) = self.rotate_handle_knob(rect, angle);
                    let rot_hot = self.rotating.map(|r| r.id) == Some(entity.id)
                        || ((mx - kx).abs() <= 8.0 && (my - ky).abs() <= 8.0);
                    self.draw_rotate_handle(ui, rect, angle, rot_hot);
                }
                ViewTool::Transform => {
                    // Combined gizmo: scale corners plus the rotate knob. No
                    // move handle — the body itself stays draggable.
                    self.draw_scale_handles(ui, rect, angle, mx, my);
                    let (kx, ky) = self.rotate_handle_knob(rect, angle);
                    let rot_hot = self.rotating.map(|r| r.id) == Some(entity.id)
                        || ((mx - kx).abs() <= 8.0 && (my - ky).abs() <= 8.0);
                    self.draw_rotate_handle(ui, rect, angle, rot_hot);
                }
            }
        }

        if self.script_drag.is_some() {
            self.handle_script_drop(ui, area);
        } else if self.handle_tilemap_paint(ui, area) {
            // Tile painting owns the pointer while active over its grid.
        } else {
            self.handle_viewport_input(ui, area);
        }
        self.handle_prefab_drop(ui, area, z);

        // Transform/zoom HUD overlay (Unity-style), bottom-left of the viewport.
        if self.config.settings.show_transform_hud {
            let scene_flags = if self.hidden_ids.is_empty() && self.locked_ids.is_empty() {
                String::new()
            } else {
                format!(
                    "   hidden {} locked {}",
                    self.hidden_ids.len(),
                    self.locked_ids.len()
                )
            };
            let hud = if self.selection_count() > 1 {
                format!(
                    "{} selected   zoom {}%{}",
                    self.selection_count(),
                    (self.cam_zoom * 100.0).round() as i32,
                    scene_flags,
                )
            } else if let Some(e) = self.selected.and_then(|id| self.scene.entity(id)) {
                format!(
                    "{}   x {} y {}   w {} h {}   zoom {}%{}",
                    e.name,
                    format_num(e.x),
                    format_num(e.y),
                    format_num(e.size_x),
                    format_num(e.size_y),
                    (self.cam_zoom * 100.0).round() as i32,
                    scene_flags,
                )
            } else {
                format!(
                    "zoom {}%{}   (scroll to zoom, middle-drag to pan, F to frame)",
                    (self.cam_zoom * 100.0).round() as i32,
                    scene_flags
                )
            };
            let hud_w = ui.painter.text_width(&hud, 13.0) + 16.0;
            let hud_rect = Rect::new(
                area.x + 6.0,
                area.bottom() - 26.0,
                hud_w.min(area.w - 12.0),
                20.0,
            );
            ui.painter.fill_round_rect(hud_rect, 4.0, [0, 0, 0, 150]);
            ui.painter.text_clipped(
                hud_rect.x + 8.0,
                hud_rect.y + 3.0,
                &hud,
                13.0,
                self.config.theme.text,
                hud_rect.w - 12.0,
            );
        }

        ui.reset_input_clip();
        ui.painter.set_clip_raw(prev);
    }

    fn viewport_3d(&mut self, ui: &mut Ui, area: Rect) {
        self.last_viewport = area;
        let previous_clip = ui.painter.push_clip(area);
        ui.set_input_clip(area);
        let inside = area.contains(ui.input.mouse_x, ui.input.mouse_y);
        let display_scale = viewport_display_scale(ui.input.display_scale);

        let now = Instant::now();
        let delta_seconds = now
            .saturating_duration_since(self.viewport_3d_last_frame)
            .as_secs_f32()
            .min(0.05);
        self.viewport_3d_last_frame = now;
        self.viewport_camera_3d.fov = self.config.settings.viewport_camera_fov.clamp(20.0, 140.0);
        self.viewport_camera_3d.projection = Projection3D::Perspective;

        let sensitivity = self
            .config
            .settings
            .viewport_camera_sensitivity
            .clamp(0.05, 8.0);
        let move_speed = self
            .config
            .settings
            .viewport_camera_speed
            .clamp(0.1, 1_000.0);

        // Preserve a click candidate even when winit coalesces RMB press and
        // release before the next redraw.
        if inside && ui.input.right_pressed && self.viewport_3d_look.is_none() {
            self.viewport_3d_look = Some(Viewport3DLook {
                mouse_x: ui.input.mouse_x,
                mouse_y: ui.input.mouse_y,
                pitch: self.viewport_camera_3d.euler.x,
                yaw: self.viewport_camera_3d.euler.y,
                navigated: ui.input.right_dragged,
            });
        }

        // RMB owns fly navigation in 3D. Starting from a fixed anchor makes
        // look independent of frame rate and mouse-event coalescing.
        if ui.input.right_down && (inside || self.viewport_3d_look.is_some()) {
            let anchor = *self.viewport_3d_look.get_or_insert(Viewport3DLook {
                mouse_x: ui.input.mouse_x,
                mouse_y: ui.input.mouse_y,
                pitch: self.viewport_camera_3d.euler.x,
                yaw: self.viewport_camera_3d.euler.y,
                navigated: ui.input.right_dragged,
            });
            let look_dx = (ui.input.mouse_x - anchor.mouse_x) / display_scale;
            let look_dy = (ui.input.mouse_y - anchor.mouse_y) / display_scale;
            let pitch_direction = if self.config.settings.viewport_invert_mouse_look {
                -1.0
            } else {
                1.0
            };
            self.viewport_camera_3d.euler.x =
                (anchor.pitch + look_dy * sensitivity * 0.2 * pitch_direction).clamp(-89.0, 89.0);
            self.viewport_camera_3d.euler.y = anchor.yaw + look_dx * sensitivity * 0.2;

            let fly_key = ui.input.key_w
                || ui.input.key_a
                || ui.input.key_s
                || ui.input.key_d
                || ui.input.key_q
                || ui.input.key_e;
            if let Some(look) = self.viewport_3d_look.as_mut() {
                look.navigated |= look_dx * look_dx + look_dy * look_dy > 9.0
                    || fly_key
                    || ui.input.right_dragged;
            }

            if !ui.input.ctrl {
                let mut movement = Vec3::ZERO;
                let forward = camera_forward(self.viewport_camera_3d.euler);
                let right = camera_right(self.viewport_camera_3d.euler);
                if ui.input.key_w {
                    movement = add_vec3(movement, forward);
                }
                if ui.input.key_s {
                    movement = sub_vec3(movement, forward);
                }
                if ui.input.key_d {
                    movement = add_vec3(movement, right);
                }
                if ui.input.key_a {
                    movement = sub_vec3(movement, right);
                }
                if ui.input.key_e {
                    movement.y += 1.0;
                }
                if ui.input.key_q {
                    movement.y -= 1.0;
                }
                movement = normalized_vec3(movement);
                let boost = if ui.input.shift { 3.0 } else { 1.0 };
                self.viewport_camera_3d.position = add_vec3(
                    self.viewport_camera_3d.position,
                    scale_vec3(movement, move_speed * boost * delta_seconds),
                );
            }
            ui.wants_redraw = true;
        } else if !ui.input.right_released {
            self.viewport_3d_look = None;
        }

        // MMB pans in the camera plane. This mirrors the established 2D MMB
        // gesture while keeping the two camera states completely independent.
        if !ui.input.right_down
            && ui.input.middle_down
            && (inside || self.viewport_3d_pan_anchor.is_some())
        {
            let (start_x, start_y, start_position) = *self.viewport_3d_pan_anchor.get_or_insert((
                ui.input.mouse_x,
                ui.input.mouse_y,
                self.viewport_camera_3d.position,
            ));
            let dx = ui.input.mouse_x - start_x;
            let dy = ui.input.mouse_y - start_y;
            let right = camera_right(self.viewport_camera_3d.euler);
            let up = camera_up(self.viewport_camera_3d.euler);
            let units_per_pixel = move_speed * sensitivity * 0.0015;
            self.viewport_camera_3d.position = add_vec3(
                start_position,
                add_vec3(
                    scale_vec3(right, -dx * units_per_pixel),
                    scale_vec3(up, dy * units_per_pixel),
                ),
            );
            ui.wants_redraw = true;
        } else {
            self.viewport_3d_pan_anchor = None;
        }

        // Wheel dolly is useful without entering fly-look mode.
        if inside && ui.input.scroll != 0.0 && !ui.input.right_down {
            let forward = camera_forward(self.viewport_camera_3d.euler);
            self.viewport_camera_3d.position = add_vec3(
                self.viewport_camera_3d.position,
                scale_vec3(forward, ui.input.scroll * move_speed * 0.08),
            );
            ui.wants_redraw = true;
        }

        ui.painter.fill_rect(area, self.config.theme.viewport_bg);
        let background = self.scene.background;
        if !self.draw_environment_preview_3d(ui, area.shrink(1.0)) {
            ui.painter.fill_rect(
                area.shrink(1.0),
                [background[0], background[1], background[2], 255],
            );
        }

        let aspect = (area.w / area.h.max(1.0)).max(0.001);
        let view_projection = self.viewport_camera_3d.view_projection(aspect);
        if self.config.layout.show_grid {
            self.draw_grid_3d(ui, area, view_projection);
        }

        let lights = self.gather_viewport_lights_3d();
        let mut triangles = std::mem::take(&mut self.viewport_3d_triangles);
        let mut triangle_hits = std::mem::take(&mut self.viewport_3d_triangle_hits);
        let mut proxy_hits = std::mem::take(&mut self.viewport_3d_proxy_hits);
        triangles.clear();
        triangle_hits.clear();
        proxy_hits.clear();
        // Editor surfaces are already rendered in logical pixels, with
        // `display_scale == 1`. Budget against the viewport area that the CPU
        // painter actually touches rather than the monitor's physical scale.
        let triangle_budget = viewport_triangle_budget(area);

        for entity in &self.scene.entities {
            if self.hidden_ids.contains(&entity.id) {
                continue;
            }
            let Some(model) = self.entity_world_model_3d(entity.id) else {
                continue;
            };
            let world_origin = model.transform_point(Vec3::ZERO);
            if let Some(point) = project_world_point(view_projection, world_origin, area) {
                if !self.locked_ids.contains(&entity.id) {
                    proxy_hits.push(Viewport3DProxyHit {
                        id: entity.id,
                        x: point.0,
                        y: point.1,
                        radius: entity_proxy_radius_3d(entity) * display_scale,
                        depth: point.2,
                    });
                }
            }

            let active = self.scene.is_active_in_tree(entity.id);
            for component in &entity.components {
                let Component::Core { name, props } = component else {
                    continue;
                };
                if name != "MeshRenderer3D"
                    || prop_bool(props, &["visible"]).is_some_and(|visible| !visible)
                {
                    continue;
                }
                let mesh = prop_string_like(props, &["mesh_path", "meshPath"])
                    .filter(|path| !path.trim().is_empty())
                    .and_then(|path| self.load_viewport_mesh(&path))
                    .or_else(|| {
                        let primitive = prop_string_like(props, &["primitive", "shape"])
                            .unwrap_or_else(|| "cube".to_string());
                        if matches!(
                            primitive.trim().to_ascii_lowercase().as_str(),
                            "" | "none" | "off"
                        ) {
                            return None;
                        }
                        let minimum_rings = if matches!(
                            primitive.trim().to_ascii_lowercase().as_str(),
                            "sphere" | "uvsphere"
                        ) {
                            2.0
                        } else {
                            1.0
                        };
                        let options = crate::mesh::PrimitiveOptions {
                            size: [
                                prop_number(props, &["primitive_size_x"]).unwrap_or(1.0),
                                prop_number(props, &["primitive_size_y"]).unwrap_or(1.0),
                                prop_number(props, &["primitive_size_z"]).unwrap_or(1.0),
                            ],
                            radius: prop_number(props, &["primitive_radius"]).unwrap_or(0.5),
                            height: prop_number(props, &["primitive_height"]).unwrap_or(1.0),
                            segments: prop_number(props, &["primitive_segments"])
                                .unwrap_or(24.0)
                                .round()
                                .clamp(3.0, 1024.0) as u32,
                            rings: prop_number(props, &["primitive_rings"])
                                .unwrap_or(12.0)
                                .round()
                                .clamp(minimum_rings, 512.0)
                                as u32,
                        };
                        crate::mesh::primitive_mesh(&primitive, options).ok()
                    });
                let Some(mesh) = mesh else {
                    continue;
                };
                let rgba = prop_color(props, "color").unwrap_or([255, 255, 255, 255]);
                let command = Mesh3DCommand {
                    mesh,
                    model,
                    view_projection,
                    tint: Color::rgba(rgba[0], rgba[1], rgba[2], rgba[3]),
                    texture: None,
                    shader: None,
                    // Asset inspection should remain useful even when an
                    // imported file has inconsistent winding.
                    double_sided: true,
                };
                let Ok(projected) = project_mesh(&command, &lights) else {
                    continue;
                };
                for triangle in projected {
                    if triangles.len() >= triangle_budget {
                        break;
                    }
                    let points = triangle.vertices.map(|vertex| {
                        (
                            area.x + (vertex.ndc[0] * 0.5 + 0.5) * area.w,
                            area.y + (0.5 - vertex.ndc[1] * 0.5) * area.h,
                        )
                    });
                    let mut color = [0u8; 4];
                    for channel in 0..4 {
                        let average = triangle
                            .vertices
                            .iter()
                            .map(|vertex| vertex.color[channel])
                            .sum::<f32>()
                            / 3.0;
                        color[channel] = (average.clamp(0.0, 1.0) * 255.0).round() as u8;
                    }
                    if !active {
                        color[0] /= 3;
                        color[1] /= 3;
                        color[2] /= 3;
                    }
                    let bounds = triangle_screen_bounds(points);
                    triangles.push((triangle.depth, entity.id, points, color));
                    if !self.locked_ids.contains(&entity.id) {
                        triangle_hits.push(Viewport3DHit {
                            id: entity.id,
                            points,
                            bounds,
                            depth: triangle.depth,
                        });
                    }
                }
            }
        }

        // Painter has no depth buffer, so render opaque preview triangles from
        // far to near. Picking below still compares per-triangle depth and is
        // therefore independent of entity iteration order.
        triangles.sort_by(|left, right| right.0.partial_cmp(&left.0).unwrap_or(Ordering::Equal));
        for (_, id, points, color) in &triangles {
            ui.painter
                .fill_triangle(points[0], points[1], points[2], *color);
            if self.is_selected(*id)
                && triangles.len() <= (60_000.0 / display_scale).max(8_000.0) as usize
            {
                let outline = [
                    self.config.theme.selection[0],
                    self.config.theme.selection[1],
                    self.config.theme.selection[2],
                    105,
                ];
                ui.painter
                    .stroke_line(points[0].0, points[0].1, points[1].0, points[1].1, outline);
                ui.painter
                    .stroke_line(points[1].0, points[1].1, points[2].0, points[2].1, outline);
                ui.painter
                    .stroke_line(points[2].0, points[2].1, points[0].0, points[0].1, outline);
            }
        }

        // Component proxies remain editor chrome above scene geometry: cameras,
        // lights and colliders are useful even when they have no renderer.
        for entity in &self.scene.entities {
            if self.hidden_ids.contains(&entity.id) {
                continue;
            }
            let Some(model) = self.entity_world_model_3d(entity.id) else {
                continue;
            };
            self.draw_entity_proxies_3d(ui, area, view_projection, entity, model);
        }

        if let Some(id) = self.selected
            && !self.locked_ids.contains(&id)
            && let Some(model) = self.entity_world_model_3d(id)
        {
            self.draw_transform_gizmo_3d(ui, area, view_projection, id, model);
        }

        if !self.handle_mesh_drop_3d(ui, area) {
            self.handle_viewport_input_3d(ui, area, view_projection, &triangle_hits, &proxy_hits);
        }
        recycle_viewport_scratch(&mut self.viewport_3d_triangles, triangles);
        recycle_viewport_scratch(&mut self.viewport_3d_triangle_hits, triangle_hits);
        recycle_viewport_scratch(&mut self.viewport_3d_proxy_hits, proxy_hits);

        if self.config.settings.show_transform_hud {
            let position = self.viewport_camera_3d.position;
            let selected = self
                .selected
                .and_then(|id| self.scene.entity(id))
                .map(|entity| {
                    format!(
                        "{}  x {} y {} z {}   ",
                        entity.name,
                        format_num(entity.x),
                        format_num(entity.y),
                        format_num(entity.position_z)
                    )
                })
                .unwrap_or_default();
            let hud = format!(
                "{selected}camera {:.1}, {:.1}, {:.1}   FOV {:.0}°   RMB look + WASD/QE, Shift boosts, MMB pans",
                position.x, position.y, position.z, self.viewport_camera_3d.fov
            );
            let hud_width = (ui.painter.text_width(&hud, 13.0) + 16.0).min(area.w - 12.0);
            let hud_rect = Rect::new(
                area.x + 6.0,
                area.bottom() - 26.0,
                hud_width.max(40.0),
                20.0,
            );
            ui.painter.fill_round_rect(hud_rect, 4.0, [0, 0, 0, 165]);
            ui.painter.text_clipped(
                hud_rect.x + 8.0,
                hud_rect.y + 3.0,
                &hud,
                13.0,
                self.config.theme.text,
                hud_rect.w - 12.0,
            );
        }

        ui.reset_input_clip();
        ui.painter.set_clip_raw(previous_clip);
    }

    fn draw_grid_3d(&self, ui: &mut Ui, area: Rect, view_projection: Mat4) {
        let layout = grid_3d_layout(self.viewport_camera_3d, area);
        let grid = self.config.theme.grid;

        // Draw a sparse, long-range level first, then add a camera-centred
        // detail level. Keeping a fixed line budget makes this substantially
        // cheaper than drawing every one-unit line out to the far plane, while
        // the long clipped segments make the ground plane read as infinite.
        // Both level origins are snapped in world space, so the grid does not
        // visibly swim as the camera moves.
        let mut draw_level =
            |step: f32, half_lines: i32, minor_alpha: u8, major_alpha: u8, skip: Option<f32>| {
                let center_x = (self.viewport_camera_3d.position.x / step).round() * step;
                let center_z = (self.viewport_camera_3d.position.z / step).round() * step;
                for line in -half_lines..=half_lines {
                    let z = center_z + line as f32 * step;
                    if skip.is_none_or(|skip_step| !grid_line_aligned(z, skip_step, step)) {
                        let global_line = (z / step).round() as i64;
                        let alpha = if global_line.rem_euclid(5) == 0 {
                            major_alpha
                        } else {
                            minor_alpha
                        };
                        let color = if z.abs() <= step * 0.01 {
                            [220, 75, 75, 205]
                        } else {
                            [grid[0], grid[1], grid[2], alpha]
                        };
                        if let Some((start, end)) = project_world_segment_clipped(
                            view_projection,
                            Vec3::new(layout.min_x, 0.0, z),
                            Vec3::new(layout.max_x, 0.0, z),
                            area,
                        ) {
                            ui.painter
                                .stroke_line(start.0, start.1, end.0, end.1, color);
                        }
                    }

                    let x = center_x + line as f32 * step;
                    if skip.is_none_or(|skip_step| !grid_line_aligned(x, skip_step, step)) {
                        let global_line = (x / step).round() as i64;
                        let alpha = if global_line.rem_euclid(5) == 0 {
                            major_alpha
                        } else {
                            minor_alpha
                        };
                        let color = if x.abs() <= step * 0.01 {
                            [76, 150, 230, 205]
                        } else {
                            [grid[0], grid[1], grid[2], alpha]
                        };
                        if let Some((start, end)) = project_world_segment_clipped(
                            view_projection,
                            Vec3::new(x, 0.0, layout.min_z),
                            Vec3::new(x, 0.0, layout.max_z),
                            area,
                        ) {
                            ui.painter
                                .stroke_line(start.0, start.1, end.0, end.1, color);
                        }
                    }
                }
            };

        draw_level(layout.coarse_step, layout.coarse_half_lines, 24, 54, None);
        if layout.fine_step < layout.coarse_step * 0.999 {
            draw_level(
                layout.fine_step,
                layout.fine_half_lines,
                48,
                105,
                Some(layout.coarse_step),
            );
        }
    }

    fn gather_viewport_lights_3d(&self) -> Vec<RenderLight3D> {
        let mut lights = Vec::new();
        for entity in &self.scene.entities {
            if self.hidden_ids.contains(&entity.id) || !self.scene.is_active_in_tree(entity.id) {
                continue;
            }
            let Some(model) = self.entity_world_model_3d(entity.id) else {
                continue;
            };
            for component in &entity.components {
                let Component::Core { name, props } = component else {
                    continue;
                };
                if name != "Light3D"
                    || prop_bool(props, &["visible"]).is_some_and(|visible| !visible)
                {
                    continue;
                }
                let kind = match prop_string_like(props, &["kind"])
                    .unwrap_or_else(|| "point".to_string())
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "directional" => LightKind3D::Directional,
                    "spot" => LightKind3D::Spot,
                    _ => LightKind3D::Point,
                };
                let rgba = prop_color(props, "color").unwrap_or([255, 255, 255, 255]);
                lights.push(RenderLight3D {
                    kind,
                    position: model.transform_point(Vec3::ZERO),
                    direction: normalized_vec3(
                        model.transform_direction(Vec3::new(0.0, 0.0, -1.0)),
                    ),
                    color: Color::rgba(rgba[0], rgba[1], rgba[2], rgba[3]),
                    intensity: prop_number(props, &["intensity"]).unwrap_or(1.0).max(0.0),
                    range: prop_number(props, &["range"]).unwrap_or(10.0).max(0.001),
                    spot_angle_radians: prop_number(props, &["spot_angle", "spotAngle"])
                        .unwrap_or(45.0)
                        .to_radians(),
                    spot_softness: prop_number(props, &["spot_softness", "spotSoftness"])
                        .unwrap_or(0.15)
                        .clamp(0.0, 1.0),
                    casts_shadows: prop_bool(props, &["casts_shadows", "castsShadows"])
                        .unwrap_or(true),
                });
            }
        }
        lights
    }

    /// Draw the first enabled scene environment behind the 3D preview. The
    /// runtime performs perspective-correct equirectangular sampling; the
    /// editor uses a lightweight yaw-scrolled panorama preview so inspector
    /// edits remain immediate without adding another full-frame CPU ray pass.
    fn draw_environment_preview_3d(&self, ui: &mut Ui, area: Rect) -> bool {
        let environment = self.scene.entities.iter().find_map(|entity| {
            if self.hidden_ids.contains(&entity.id) || !self.scene.is_active_in_tree(entity.id) {
                return None;
            }
            entity
                .components
                .iter()
                .find_map(|component| match component {
                    Component::Core { name, props }
                        if matches!(name.as_str(), "Environment3D" | "Skybox3D")
                            && prop_bool(props, &["enabled"]).unwrap_or(true) =>
                    {
                        Some(props.as_slice())
                    }
                    _ => None,
                })
        });
        let Some(props) = environment else {
            return false;
        };
        let intensity = prop_number(props, &["intensity"]).unwrap_or(1.0).max(0.0);
        let scaled = |color: Rgba| {
            [
                (color[0] as f32 * intensity).clamp(0.0, 255.0) as u8,
                (color[1] as f32 * intensity).clamp(0.0, 255.0) as u8,
                (color[2] as f32 * intensity).clamp(0.0, 255.0) as u8,
                color[3],
            ]
        };
        match prop_string_like(props, &["mode"])
            .unwrap_or_else(|| "gradient".to_string())
            .to_ascii_lowercase()
            .as_str()
        {
            "solid" | "color" | "colour" => {
                ui.painter.fill_rect(
                    area,
                    scaled(prop_color(props, "color").unwrap_or([20, 24, 32, 255])),
                );
            }
            "equirectangular" | "panorama" | "skybox" | "texture" => {
                let path = prop_string_like(props, &["texture", "texture_path"])
                    .filter(|path| !path.trim().is_empty());
                let Some(image) = path.as_deref().and_then(|path| self.load_image(path)) else {
                    return self.draw_environment_gradient_preview(ui, area, props, intensity);
                };
                let yaw = (self.viewport_camera_3d.euler.y
                    + prop_number(props, &["rotation"]).unwrap_or(0.0))
                .rem_euclid(360.0)
                    / 360.0;
                let split = (1.0 - yaw).clamp(0.0, 1.0);
                let image_width = image.width() as f32;
                let image_height = image.height() as f32;
                let first_width = area.w * split;
                if first_width > 0.0 {
                    ui.painter.draw_image(
                        &image,
                        Rect::new(area.x, area.y, first_width, area.h),
                        Some(Rect::new(
                            yaw * image_width,
                            0.0,
                            split * image_width,
                            image_height,
                        )),
                        scaled([255, 255, 255, 255]),
                    );
                }
                if first_width < area.w {
                    ui.painter.draw_image(
                        &image,
                        Rect::new(area.x + first_width, area.y, area.w - first_width, area.h),
                        Some(Rect::new(0.0, 0.0, yaw * image_width, image_height)),
                        scaled([255, 255, 255, 255]),
                    );
                }
            }
            _ => return self.draw_environment_gradient_preview(ui, area, props, intensity),
        }
        true
    }

    fn draw_environment_gradient_preview(
        &self,
        ui: &mut Ui,
        area: Rect,
        props: &[Prop],
        intensity: f32,
    ) -> bool {
        let top = prop_color(props, "top_color").unwrap_or([30, 47, 78, 255]);
        let bottom = prop_color(props, "bottom_color").unwrap_or([8, 10, 16, 255]);
        let rows = area.h.ceil().max(1.0) as usize;
        for row in 0..rows {
            let amount = row as f32 / rows.saturating_sub(1).max(1) as f32;
            let channel = |index: usize| {
                ((top[index] as f32 + (bottom[index] as f32 - top[index] as f32) * amount)
                    * intensity)
                    .clamp(0.0, 255.0) as u8
            };
            ui.painter.fill_rect(
                Rect::new(area.x, area.y + row as f32, area.w, 1.0),
                [channel(0), channel(1), channel(2), 255],
            );
        }
        true
    }

    fn draw_entity_proxies_3d(
        &self,
        ui: &mut Ui,
        area: Rect,
        view_projection: Mat4,
        entity: &Entity,
        model: Mat4,
    ) {
        let origin = model.transform_point(Vec3::ZERO);
        let Some(screen) = project_world_point(view_projection, origin, area) else {
            return;
        };
        let selected = self.is_selected(entity.id);
        let base = if selected {
            self.config.theme.selection
        } else {
            [175, 180, 188, 210]
        };
        let chrome_scale = viewport_display_scale(ui.input.display_scale);
        ui.painter
            .fill_circle(screen.0, screen.1, 2.5 * chrome_scale, base);

        for component in &entity.components {
            let Component::Core { name, props } = component else {
                continue;
            };
            match name.as_str() {
                "Camera3D" => {
                    let camera_color = [90, 205, 235, 235];
                    let fov = prop_number(props, &["fov"])
                        .unwrap_or(60.0)
                        .clamp(15.0, 150.0);
                    draw_camera_proxy_3d(
                        &mut ui.painter,
                        area,
                        view_projection,
                        model,
                        fov,
                        camera_color,
                        chrome_scale,
                    );
                }
                "Light3D" => {
                    let light_color = prop_color(props, "color").unwrap_or([255, 220, 90, 255]);
                    ui.painter
                        .fill_circle(screen.0, screen.1, 5.0 * chrome_scale, light_color);
                    for index in 0..8 {
                        let angle = index as f32 * std::f32::consts::TAU / 8.0;
                        ui.painter.stroke_line(
                            screen.0 + angle.cos() * 7.0 * chrome_scale,
                            screen.1 + angle.sin() * 7.0 * chrome_scale,
                            screen.0 + angle.cos() * 11.0 * chrome_scale,
                            screen.1 + angle.sin() * 11.0 * chrome_scale,
                            light_color,
                        );
                    }
                }
                "Collider3D" => {
                    if prop_bool(props, &["enabled"]).unwrap_or(true) {
                        let offset = Vec3::new(
                            prop_number(props, &["offset_x", "offsetX"]).unwrap_or(0.0),
                            prop_number(props, &["offset_y", "offsetY"]).unwrap_or(0.0),
                            prop_number(props, &["offset_z", "offsetZ"]).unwrap_or(0.0),
                        );
                        let half = Vec3::new(
                            prop_number(props, &["size_x", "sizeX"])
                                .unwrap_or(1.0)
                                .abs()
                                * 0.5,
                            prop_number(props, &["size_y", "sizeY"])
                                .unwrap_or(1.0)
                                .abs()
                                * 0.5,
                            prop_number(props, &["size_z", "sizeZ"])
                                .unwrap_or(1.0)
                                .abs()
                                * 0.5,
                        );
                        draw_wire_box_3d(
                            &mut ui.painter,
                            area,
                            view_projection,
                            model,
                            offset,
                            half,
                            [92, 220, 130, 205],
                        );
                    }
                }
                "ParticleSystem3D" => {
                    let particle_color =
                        prop_color(props, "start_color").unwrap_or([255, 190, 80, 255]);
                    for (dx, dy, radius) in [
                        (-7.0, 4.0, 1.8),
                        (-3.0, -5.0, 2.2),
                        (2.0, 2.0, 2.6),
                        (6.0, -4.0, 1.7),
                        (9.0, 5.0, 1.4),
                    ] {
                        ui.painter.fill_circle(
                            screen.0 + dx * chrome_scale,
                            screen.1 + dy * chrome_scale,
                            radius * chrome_scale,
                            particle_color,
                        );
                    }
                }
                _ => {}
            }
        }
    }

    fn transform_gizmo_3d(
        &self,
        area: Rect,
        view_projection: Mat4,
        id: u64,
        model: Mat4,
    ) -> Option<Viewport3DGizmo> {
        let entity = self.scene.entity(id)?;
        let origin_world = model.transform_point(Vec3::ZERO);
        let origin = project_world_point(view_projection, origin_world, area)?;

        // Move handles edit the authored position axes, so they follow the
        // parent basis. Scale handles follow the entity's rotated local basis.
        // Normalization deliberately removes entity/parent scale: a scale of
        // 100 must not make the gizmo itself 100 times larger.
        let basis = if self.config.layout.view_tool == ViewTool::Move {
            entity
                .parent
                .and_then(|parent| self.entity_world_model_3d(parent))
                .unwrap_or_else(Mat4::identity)
        } else {
            model
        };
        let distance = length_vec3(sub_vec3(origin_world, self.viewport_camera_3d.position))
            .max(self.viewport_camera_3d.near_clip * 2.0);
        let world_per_pixel =
            2.0 * distance * (self.viewport_camera_3d.fov.to_radians() * 0.5).tan()
                / area.h.max(1.0);
        let handle_length = (world_per_pixel * 72.0).clamp(0.02, 10_000.0);
        let axes = Viewport3DAxis::ALL.map(|axis| {
            let transformed = normalized_vec3(basis.transform_direction(axis.vector()));
            let direction = if length_vec3(transformed) > 0.5 {
                transformed
            } else {
                axis.vector()
            };
            let end_world = add_vec3(origin_world, scale_vec3(direction, handle_length));
            project_world_point(view_projection, end_world, area).map(|end| Viewport3DGizmoAxis {
                axis,
                end: (end.0, end.1),
            })
        });

        // Rotation rings use normalized model columns so authored/parent scale
        // cannot inflate the editor chrome. Sampling in world space preserves
        // the useful ellipse/edge-on appearance of a real 3D rotation gizmo,
        // while the ring's projected perimeter supplies a stable px-to-degree
        // conversion for dragging below.
        let model_axes = Viewport3DAxis::ALL.map(|axis| {
            let transformed = normalized_vec3(model.transform_direction(axis.vector()));
            if length_vec3(transformed) > 0.5 {
                transformed
            } else {
                axis.vector()
            }
        });
        let ring_radius = handle_length * 0.78;
        let rotation_rings = Viewport3DAxis::ALL.map(|axis| {
            let (first, second) = match axis {
                Viewport3DAxis::X => (model_axes[1], model_axes[2]),
                Viewport3DAxis::Y => (model_axes[2], model_axes[0]),
                Viewport3DAxis::Z => (model_axes[0], model_axes[1]),
            };
            let points = std::array::from_fn(|index| {
                let angle = index as f32 / ROTATION_RING_SAMPLES as f32 * std::f32::consts::TAU;
                let offset = add_vec3(
                    scale_vec3(first, angle.cos() * ring_radius),
                    scale_vec3(second, angle.sin() * ring_radius),
                );
                project_world_point(view_projection, add_vec3(origin_world, offset), area)
                    .map(|point| (point.0, point.1))
            });
            Viewport3DRotationRing { axis, points }
        });
        Some(Viewport3DGizmo {
            origin: (origin.0, origin.1),
            axes,
            rotation_rings,
        })
    }

    fn draw_transform_gizmo_3d(
        &self,
        ui: &mut Ui,
        area: Rect,
        view_projection: Mat4,
        id: u64,
        model: Mat4,
    ) {
        let Some(gizmo) = self.transform_gizmo_3d(area, view_projection, id, model) else {
            return;
        };
        let chrome_scale = viewport_display_scale(ui.input.display_scale);
        let tool = self.config.layout.view_tool;

        if matches!(tool, ViewTool::Rotate | ViewTool::Transform) {
            for ring in gizmo.rotation_rings {
                let active = self.viewport_3d_drag.as_ref().is_some_and(|drag| {
                    matches!(
                        drag.mode,
                        Viewport3DDragMode::RotateAxis { axis, .. } if axis == ring.axis
                    )
                });
                let color = if active {
                    self.config.theme.selection
                } else {
                    ring.axis.color()
                };
                for index in 0..ROTATION_RING_SAMPLES {
                    let next = (index + 1) % ROTATION_RING_SAMPLES;
                    if let (Some(start), Some(end)) = (ring.points[index], ring.points[next]) {
                        stroke_line_hidpi(&mut ui.painter, start, end, color, chrome_scale);
                    }
                }
            }
        }

        if matches!(tool, ViewTool::Move | ViewTool::Scale | ViewTool::Transform) {
            for projected in gizmo.axes.into_iter().flatten() {
                let color = projected.axis.color();
                ui.painter.stroke_line(
                    gizmo.origin.0,
                    gizmo.origin.1,
                    projected.end.0,
                    projected.end.1,
                    color,
                );
                if matches!(tool, ViewTool::Scale | ViewTool::Transform) {
                    let radius = 5.0 * chrome_scale;
                    ui.painter.fill_rect(
                        Rect::new(
                            projected.end.0 - radius,
                            projected.end.1 - radius,
                            radius * 2.0,
                            radius * 2.0,
                        ),
                        color,
                    );
                } else {
                    ui.painter.fill_circle(
                        projected.end.0,
                        projected.end.1,
                        4.0 * chrome_scale,
                        color,
                    );
                }
            }
        }

        if tool == ViewTool::Scale {
            let radius = 5.5 * chrome_scale;
            let rect = Rect::new(
                gizmo.origin.0 - radius,
                gizmo.origin.1 - radius,
                radius * 2.0,
                radius * 2.0,
            );
            ui.painter.fill_rect(rect, self.config.theme.selection);
            ui.painter.stroke_rect(rect, [16, 16, 16, 230]);
        } else if tool == ViewTool::Transform {
            // The inner dot remains free-move. The surrounding square is a
            // distinct uniform-scale annulus, avoiding an ambiguous shared hit
            // target while keeping all transform modes in one compact gizmo.
            let radius = 9.0 * chrome_scale;
            ui.painter.stroke_rect(
                Rect::new(
                    gizmo.origin.0 - radius,
                    gizmo.origin.1 - radius,
                    radius * 2.0,
                    radius * 2.0,
                ),
                self.config.theme.selection,
            );
        }
        if matches!(tool, ViewTool::Move | ViewTool::Transform) {
            ui.painter.fill_circle(
                gizmo.origin.0,
                gizmo.origin.1,
                4.5 * chrome_scale,
                self.config.theme.selection,
            );
        }
    }

    fn handle_viewport_input_3d(
        &mut self,
        ui: &mut Ui,
        area: Rect,
        view_projection: Mat4,
        triangle_hits: &[Viewport3DHit],
        proxy_hits: &[Viewport3DProxyHit],
    ) {
        if ui.input.right_down {
            return;
        }

        if ui.input.right_released {
            let context_click = self
                .viewport_3d_look
                .take()
                .is_some_and(|look| !look.navigated);
            if context_click && area.contains(ui.input.mouse_x, ui.input.mouse_y) {
                if let Some(id) = viewport_hit_3d(
                    triangle_hits,
                    proxy_hits,
                    ui.input.mouse_x,
                    ui.input.mouse_y,
                ) {
                    if !self.is_selected(id) {
                        self.select_only(id);
                    }
                    self.open_entity_menu(id, ui.input.mouse_x, ui.input.mouse_y);
                } else {
                    let position = viewport_drop_position_3d(
                        self.viewport_camera_3d,
                        area,
                        ui.input.mouse_x,
                        ui.input.mouse_y,
                    );
                    self.open_viewport_menu(
                        ui.input.mouse_x,
                        ui.input.mouse_y,
                        position.x,
                        position.y,
                    );
                }
            }
            return;
        }
        // A focus loss may omit a release event on some window systems.
        self.viewport_3d_look = None;

        if let Some(drag) = self.viewport_3d_drag.clone() {
            if ui.input.mouse_down {
                let screen_delta = (
                    ui.input.mouse_x - drag.start_mouse.0,
                    ui.input.mouse_y - drag.start_mouse.1,
                );
                let snap = self.config.layout.snap;
                // The 2D grid value is measured in pixels. 3D snapping must
                // instead follow the world-space interval currently visible
                // in the adaptive ground grid; using e.g. `32` as world units
                // made ordinary drags appear completely frozen.
                let snap_step = grid_3d_layout(self.viewport_camera_3d, area).fine_step;
                let mut changed = false;
                match drag.mode {
                    Viewport3DDragMode::MovePlane {
                        screen_x_axis,
                        screen_y_axis,
                    } => {
                        let determinant =
                            screen_x_axis.0 * screen_y_axis.1 - screen_x_axis.1 * screen_y_axis.0;
                        let (mut local_dx, mut local_dy) = if determinant.abs() > 1.0e-5 {
                            (
                                (screen_delta.0 * screen_y_axis.1
                                    - screen_delta.1 * screen_y_axis.0)
                                    / determinant,
                                (screen_x_axis.0 * screen_delta.1
                                    - screen_x_axis.1 * screen_delta.0)
                                    / determinant,
                            )
                        } else {
                            // Looking exactly edge-on makes the projected XY
                            // basis singular. Fall back to a bounded camera-
                            // distance conversion instead of exploding or
                            // freezing the transform.
                            let units =
                                length_vec3(sub_vec3(self.viewport_camera_3d.position, Vec3::ZERO))
                                    .clamp(1.0, 1_000.0)
                                    * 0.002;
                            (screen_delta.0 * units, -screen_delta.1 * units)
                        };
                        if snap {
                            local_dx = (local_dx / snap_step).round() * snap_step;
                            local_dy = (local_dy / snap_step).round() * snap_step;
                        }
                        if local_dx.is_finite() && local_dy.is_finite() {
                            for start in &drag.start {
                                if let Some(entity) = self.scene.entity_mut(start.id) {
                                    let x = start.position.x + local_dx;
                                    let y = start.position.y + local_dy;
                                    changed |= (entity.x - x).abs() > 1.0e-6
                                        || (entity.y - y).abs() > 1.0e-6;
                                    entity.x = x;
                                    entity.y = y;
                                    // position_z is deliberately untouched.
                                }
                            }
                        }
                    }
                    Viewport3DDragMode::MoveAxis { axis, screen_axis } => {
                        let denominator =
                            screen_axis.0 * screen_axis.0 + screen_axis.1 * screen_axis.1;
                        if denominator > 1.0e-4 && denominator.is_finite() {
                            let mut local_delta = (screen_delta.0 * screen_axis.0
                                + screen_delta.1 * screen_axis.1)
                                / denominator;
                            local_delta = local_delta.clamp(-100_000.0, 100_000.0);
                            if snap {
                                local_delta = (local_delta / snap_step).round() * snap_step;
                            }
                            if local_delta.is_finite() {
                                for start in &drag.start {
                                    if let Some(entity) = self.scene.entity_mut(start.id) {
                                        let before = entity_position_axis_3d(entity, axis);
                                        let value =
                                            entity_position_axis_from_vec3(start.position, axis)
                                                + local_delta;
                                        set_entity_position_axis_3d(entity, axis, value);
                                        changed |= (before - value).abs() > 1.0e-6;
                                    }
                                }
                            }
                        }
                    }
                    Viewport3DDragMode::ScaleAxis {
                        axis,
                        screen_direction,
                        screen_length,
                    } => {
                        let along = screen_delta.0 * screen_direction.0
                            + screen_delta.1 * screen_direction.1;
                        // A linear handle ratio feels direct and, unlike
                        // dividing by projected world derivatives, stays
                        // bounded for tiny or parent-scaled objects.
                        let factor =
                            ((screen_length + along) / screen_length.max(24.0)).clamp(0.01, 32.0);
                        if factor.is_finite() {
                            for start in &drag.start {
                                if let Some(entity) = self.scene.entity_mut(start.id) {
                                    let before = entity_scale_axis_3d(entity, axis);
                                    let start_value = vec3_axis(start.scale, axis);
                                    let value = stable_drag_scale_3d(start_value, factor, snap);
                                    set_entity_scale_axis_3d(entity, axis, value);
                                    changed |= (before - value).abs() > 1.0e-6;
                                }
                            }
                        }
                    }
                    Viewport3DDragMode::ScaleUniform {
                        screen_direction,
                        screen_length,
                    } => {
                        let along = screen_delta.0 * screen_direction.0
                            + screen_delta.1 * screen_direction.1;
                        let factor =
                            ((screen_length + along) / screen_length.max(24.0)).clamp(0.01, 32.0);
                        if factor.is_finite() {
                            for start in &drag.start {
                                if let Some(entity) = self.scene.entity_mut(start.id) {
                                    let values = Viewport3DAxis::ALL.map(|axis| {
                                        stable_drag_scale_3d(
                                            vec3_axis(start.scale, axis),
                                            factor,
                                            snap,
                                        )
                                    });
                                    let before = (entity.scale_x, entity.scale_y, entity.scale_z);
                                    entity.scale_x = values[0];
                                    entity.scale_y = values[1];
                                    entity.scale_z = values[2];
                                    changed |= (before.0 - values[0]).abs() > 1.0e-6
                                        || (before.1 - values[1]).abs() > 1.0e-6
                                        || (before.2 - values[2]).abs() > 1.0e-6;
                                }
                            }
                        }
                    }
                    Viewport3DDragMode::RotateAxis {
                        axis,
                        screen_tangent,
                        degrees_per_pixel,
                    } => {
                        let along =
                            screen_delta.0 * screen_tangent.0 + screen_delta.1 * screen_tangent.1;
                        let delta_degrees = (along * degrees_per_pixel).clamp(-36_000.0, 36_000.0);
                        if delta_degrees.is_finite() {
                            for start in &drag.start {
                                if let Some(entity) = self.scene.entity_mut(start.id) {
                                    let before = entity_rotation_axis_3d(entity, axis);
                                    let start_value = vec3_axis(start.rotation, axis);
                                    let value =
                                        stable_drag_rotation_3d(start_value, delta_degrees, snap);
                                    set_entity_rotation_axis_3d(entity, axis, value);
                                    changed |= (before - value).abs() > 1.0e-6;
                                }
                            }
                        }
                    }
                }
                if changed {
                    self.world_model_3d_cache.borrow_mut().clear();
                    self.scene_dirty = true;
                    ui.wants_redraw = true;
                }
                return;
            }
            self.viewport_3d_drag = None;
            self.mark_dirty();
            return;
        }

        if !area.contains(ui.input.mouse_x, ui.input.mouse_y) || !ui.input.mouse_pressed {
            return;
        }

        // Gizmos own the pointer before scene geometry. Previously their
        // screen handles were decorative only, so pressing an endpoint either
        // selected the mesh behind it or cleared the selection entirely.
        if let Some(selected) = self.selected
            && !self.locked_ids.contains(&selected)
            && let Some(model) = self.entity_world_model_3d(selected)
            && let Some(gizmo) = self.transform_gizmo_3d(area, view_projection, selected, model)
            && let Some(hit) = viewport_gizmo_hit_3d(
                gizmo,
                self.config.layout.view_tool,
                ui.input.mouse_x,
                ui.input.mouse_y,
                viewport_display_scale(ui.input.display_scale),
            )
        {
            let start = self
                .selection_ids_ordered()
                .into_iter()
                .filter(|id| !self.locked_ids.contains(id))
                .filter_map(|id| {
                    self.scene
                        .entity(id)
                        .map(|entity| Viewport3DTransformStart {
                            id,
                            position: Vec3::new(entity.x, entity.y, entity.position_z),
                            rotation: Vec3::new(
                                entity.rotation_x,
                                entity.rotation_y,
                                entity.rotation_z,
                            ),
                            scale: Vec3::new(entity.scale_x, entity.scale_y, entity.scale_z),
                        })
                })
                .collect::<Vec<_>>();
            if start.is_empty() {
                return;
            }
            let Some(entity) = self.scene.entity(selected) else {
                return;
            };
            let parent_model = entity
                .parent
                .and_then(|parent| self.entity_world_model_3d(parent))
                .unwrap_or_else(Mat4::identity);
            let local = Vec3::new(entity.x, entity.y, entity.position_z);
            let origin = parent_model.transform_point(local);
            let Some(screen) = project_world_point(view_projection, origin, area) else {
                return;
            };
            let mode = match hit {
                Viewport3DGizmoHit::MoveFree => {
                    let x_point =
                        parent_model.transform_point(Vec3::new(local.x + 1.0, local.y, local.z));
                    let y_point =
                        parent_model.transform_point(Vec3::new(local.x, local.y + 1.0, local.z));
                    let (Some(screen_x), Some(screen_y)) = (
                        project_world_point(view_projection, x_point, area),
                        project_world_point(view_projection, y_point, area),
                    ) else {
                        return;
                    };
                    Viewport3DDragMode::MovePlane {
                        screen_x_axis: (screen_x.0 - screen.0, screen_x.1 - screen.1),
                        screen_y_axis: (screen_y.0 - screen.0, screen_y.1 - screen.1),
                    }
                }
                Viewport3DGizmoHit::MoveAxis(axis) => {
                    let local_axis_point = add_vec3(local, axis.vector());
                    let projected_local_axis = parent_model.transform_point(local_axis_point);
                    let projected =
                        project_world_point(view_projection, projected_local_axis, area);
                    let mut screen_axis = projected
                        .map(|point| (point.0 - screen.0, point.1 - screen.1))
                        .unwrap_or((0.0, 0.0));
                    // Extremely small parent scale would otherwise turn a
                    // one-pixel gesture into thousands of local units. Use the
                    // fixed-size gizmo direction with a conservative 32 px per
                    // local unit in that degenerate case.
                    if vector2_length(screen_axis) < 8.0
                        && let Some(projected_axis) = gizmo_axis(gizmo, axis)
                    {
                        let direction = normalized_vec2((
                            projected_axis.end.0 - gizmo.origin.0,
                            projected_axis.end.1 - gizmo.origin.1,
                        ));
                        screen_axis = (direction.0 * 32.0, direction.1 * 32.0);
                    }
                    Viewport3DDragMode::MoveAxis { axis, screen_axis }
                }
                Viewport3DGizmoHit::ScaleAxis(axis) => {
                    let Some(projected_axis) = gizmo_axis(gizmo, axis) else {
                        return;
                    };
                    let offset = (
                        projected_axis.end.0 - gizmo.origin.0,
                        projected_axis.end.1 - gizmo.origin.1,
                    );
                    let screen_length = vector2_length(offset).max(24.0);
                    let screen_direction = normalized_vec2(offset);
                    Viewport3DDragMode::ScaleAxis {
                        axis,
                        screen_direction,
                        screen_length,
                    }
                }
                Viewport3DGizmoHit::ScaleUniform => Viewport3DDragMode::ScaleUniform {
                    screen_direction: normalized_vec2((1.0, -1.0)),
                    screen_length: 72.0 * viewport_display_scale(ui.input.display_scale),
                },
                Viewport3DGizmoHit::RotateAxis(axis) => {
                    let Some(rotation_hit) = viewport_rotation_ring_hit_3d(
                        gizmo,
                        ui.input.mouse_x,
                        ui.input.mouse_y,
                        viewport_display_scale(ui.input.display_scale),
                    ) else {
                        return;
                    };
                    if rotation_hit.axis != axis {
                        return;
                    }
                    Viewport3DDragMode::RotateAxis {
                        axis,
                        screen_tangent: rotation_hit.screen_tangent,
                        degrees_per_pixel: rotation_hit.degrees_per_pixel,
                    }
                }
            };
            self.viewport_3d_drag = Some(Viewport3DDrag {
                start_mouse: (ui.input.mouse_x, ui.input.mouse_y),
                start,
                mode,
            });
            return;
        }

        let hit = viewport_hit_3d(
            triangle_hits,
            proxy_hits,
            ui.input.mouse_x,
            ui.input.mouse_y,
        );
        let Some(id) = hit else {
            if !ui.input.ctrl {
                self.clear_selection();
            }
            return;
        };
        if ui.input.ctrl {
            self.toggle_selection(id);
        } else if !self.is_selected(id) {
            self.select_only(id);
        }
        if ui.input.double_click {
            self.frame_selected();
            return;
        }
        if self.locked_ids.contains(&id)
            || !matches!(
                self.config.layout.view_tool,
                ViewTool::Move | ViewTool::Transform
            )
        {
            return;
        }

        let Some(entity) = self.scene.entity(id) else {
            return;
        };
        let parent_model = entity
            .parent
            .and_then(|parent| self.entity_world_model_3d(parent))
            .unwrap_or_else(Mat4::identity);
        let local = Vec3::new(entity.x, entity.y, entity.position_z);
        let origin = parent_model.transform_point(local);
        let x_point = parent_model.transform_point(Vec3::new(local.x + 1.0, local.y, local.z));
        let y_point = parent_model.transform_point(Vec3::new(local.x, local.y + 1.0, local.z));
        let (Some(screen), Some(screen_x), Some(screen_y)) = (
            project_world_point(view_projection, origin, area),
            project_world_point(view_projection, x_point, area),
            project_world_point(view_projection, y_point, area),
        ) else {
            return;
        };
        let start = self
            .selection_ids_ordered()
            .into_iter()
            .filter(|selected| !self.locked_ids.contains(selected))
            .filter_map(|selected| {
                self.scene
                    .entity(selected)
                    .map(|entity| Viewport3DTransformStart {
                        id: selected,
                        position: Vec3::new(entity.x, entity.y, entity.position_z),
                        rotation: Vec3::new(
                            entity.rotation_x,
                            entity.rotation_y,
                            entity.rotation_z,
                        ),
                        scale: Vec3::new(entity.scale_x, entity.scale_y, entity.scale_z),
                    })
            })
            .collect();
        self.viewport_3d_drag = Some(Viewport3DDrag {
            start_mouse: (ui.input.mouse_x, ui.input.mouse_y),
            start,
            mode: Viewport3DDragMode::MovePlane {
                screen_x_axis: (screen_x.0 - screen.0, screen_x.1 - screen.1),
                screen_y_axis: (screen_y.0 - screen.0, screen_y.1 - screen.1),
            },
        });
    }

    fn draw_grid(&self, ui: &mut Ui, area: Rect) {
        let step = (self.config.layout.grid.max(2.0) * self.cam_zoom).max(4.0);
        let line = self.config.theme.grid;
        // Offset grid by the pan so it scrolls with the scene.
        let start_x = area.x + (self.cam_x % step);
        let mut x = start_x - step;
        while x < area.right() {
            if x >= area.x {
                ui.painter
                    .fill_rect(Rect::new(x, area.y, 1.0, area.h), line);
            }
            x += step;
        }
        let start_y = area.y + (self.cam_y % step);
        let mut y = start_y - step;
        while y < area.bottom() {
            if y >= area.y {
                ui.painter
                    .fill_rect(Rect::new(area.x, y, area.w, 1.0), line);
            }
            y += step;
        }
    }

    fn draw_window_bounds(&self, ui: &mut Ui, area: Rect) {
        let (root_w, root_h) = self.preview_root_size();
        let z = self.cam_zoom;
        let rect = Rect::new(
            area.x + self.cam_x,
            area.y + self.cam_y,
            root_w * z,
            root_h * z,
        );
        ui.painter.stroke_rect(rect, self.config.theme.selection);
        ui.painter.stroke_rect(rect.shrink(-1.0), [0, 0, 0, 120]);
        let label = if self.config.settings.mobile_emulator {
            format!(
                "Mobile {} x {}",
                root_w.round() as i32,
                root_h.round() as i32
            )
        } else {
            let mut suffix = String::new();
            if self.project_window.fullscreen {
                suffix.push_str(" fullscreen");
            }
            if !self.project_window.resizable {
                suffix.push_str(" fixed");
            }
            format!(
                "{} x {}{}",
                root_w.round() as i32,
                root_h.round() as i32,
                suffix
            )
        };
        let w = ui.painter.text_width(&label, 12.0) + 10.0;
        let tag = Rect::new(
            rect.x + 6.0,
            rect.y + 6.0,
            w.min((area.w - 12.0).max(20.0)),
            18.0,
        );
        if rects_intersect(tag, area) {
            ui.painter.fill_round_rect(tag, 3.0, [0, 0, 0, 150]);
            ui.painter.text_clipped(
                tag.x + 5.0,
                tag.y + 2.0,
                &label,
                12.0,
                self.config.theme.text,
                tag.w - 8.0,
            );
        }
    }

    /// Load an image asset (relative to the project root), cached. `None` is
    /// remembered so a missing file isn't retried each frame.
    fn load_image(&self, path: &str) -> Option<Rc<image::RgbaImage>> {
        if path.is_empty() {
            return None;
        }
        let full = self.project_root.join(path);
        let modified = std::fs::metadata(&full)
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        if let Some(entry) = self.image_cache.borrow().get(path) {
            if entry.modified == modified {
                return entry.image.clone();
            }
        }
        let loaded = crate::assets::decode_base64_png(path)
            .ok()
            .flatten()
            .and_then(|(_, bytes)| {
                image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).ok()
            })
            .or_else(|| image::open(&full).ok())
            .map(|image| Rc::new(image.to_rgba8()));
        self.image_cache.borrow_mut().insert(
            path.to_string(),
            EditorImageCacheEntry {
                modified,
                image: loaded.clone(),
            },
        );
        loaded
    }

    fn load_sound_waveform(&self, path: &str) -> Option<Rc<Vec<f32>>> {
        if path.is_empty() {
            return None;
        }
        let full = self.project_root.join(path);
        let modified = std::fs::metadata(&full)
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        if let Some(entry) = self.waveform_cache.borrow().get(path) {
            if entry.modified == modified {
                return entry.peaks.clone();
            }
        }
        let peaks = decode_waveform_peaks(&full, WAVEFORM_PREVIEW_BUCKETS)
            .ok()
            .map(Rc::new);
        self.waveform_cache.borrow_mut().insert(
            path.to_string(),
            EditorWaveformCacheEntry {
                modified,
                peaks: peaks.clone(),
            },
        );
        peaks
    }

    fn entity_world_transform(&self, id: u64) -> Option<EditorWorldTransform> {
        if let Some(transform) = self.world_transform_cache.borrow().get(&id).copied() {
            return Some(transform);
        }
        let mut cache = self.world_transform_cache.borrow_mut();
        let mut visiting = HashSet::new();
        scene_world_transform_cached(
            &self.scene,
            id,
            self.preview_root_size(),
            &mut visiting,
            &mut cache,
        )
    }

    fn entity_world_model_3d(&self, id: u64) -> Option<Mat4> {
        if let Some(model) = self.world_model_3d_cache.borrow().get(&id).copied() {
            return Some(model);
        }
        let mut cache = self.world_model_3d_cache.borrow_mut();
        let mut visiting = HashSet::new();
        scene_world_model_3d_cached(&self.scene, id, &mut visiting, &mut cache)
    }

    /// Resolve a project-relative mesh for the Scene view. Missing or malformed
    /// files stay nonfatal and are cached until their modification stamp/length
    /// changes. The small LRU bound prevents browsing a large asset tree from
    /// retaining every imported model for the lifetime of the editor.
    fn load_viewport_mesh(&self, path: &str) -> Option<crate::mesh::MeshHandle> {
        let path = path.trim();
        if path.is_empty() {
            return None;
        }
        let full = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            self.project_root.join(path)
        };
        let metadata = std::fs::metadata(&full).ok();
        let stamp = EditorMeshFileStamp {
            modified: metadata.as_ref().and_then(|value| value.modified().ok()),
            len: metadata.as_ref().map(std::fs::Metadata::len),
        };
        let tick = self.mesh_cache_clock.get().wrapping_add(1);
        self.mesh_cache_clock.set(tick);
        {
            let mut cache = self.mesh_cache.borrow_mut();
            if let Some(entry) = cache.get_mut(path)
                && entry.stamp == stamp
            {
                entry.last_used = tick;
                return entry.mesh.clone();
            }
        }

        let mesh = crate::mesh::import_from_path(&full).ok();
        let mut cache = self.mesh_cache.borrow_mut();
        cache.insert(
            path.to_string(),
            EditorMeshCacheEntry {
                stamp,
                mesh: mesh.clone(),
                last_used: tick,
            },
        );
        if cache.len() > VIEWPORT_MESH_CACHE_LIMIT
            && let Some(oldest) = cache
                .iter()
                .filter(|(key, _)| key.as_str() != path)
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
        {
            cache.remove(&oldest);
        }
        mesh
    }

    /// Gather the scene's lighting (config + lights + occluders in world space)
    /// for the viewport preview, or `None` when the preview is off or the scene
    /// has lighting disabled. This is the cheap per-frame entity walk; the
    /// expensive light grid it feeds is cached in
    /// [`Self::composite_preview_lighting`].
    #[allow(clippy::type_complexity)]
    fn gather_scene_lighting(
        &self,
    ) -> Option<(
        crate::lighting::LightConfig,
        Vec<crate::lighting::Light>,
        Vec<crate::lighting::Occluder>,
    )> {
        if !self.config.settings.preview_lighting {
            return None;
        }
        let s = &self.scene.lighting;
        if !s.enabled {
            return None;
        }
        let to_color = |c: [u8; 4]| crate::platform::Color::rgba(c[0], c[1], c[2], c[3]);
        let config = crate::lighting::LightConfig {
            enabled: true,
            ambient: to_color(s.ambient),
            ambient_intensity: s.ambient_intensity,
            ao_enabled: s.ambient_occlusion,
            ao_radius: s.ao_radius,
            ao_intensity: s.ao_intensity,
            ao_samples: 10,
            shadows_enabled: s.shadows,
            soft_shadows: s.soft_shadows,
            bloom: s.bloom,
            exposure: s.exposure,
            quality: crate::lighting::LightQuality::parse(&s.quality),
        };

        let num = |props: &[Prop], name: &str, default: f32| {
            props
                .iter()
                .find(|p| p.name == name)
                .and_then(|p| match p.value {
                    PropValue::Number(v) => Some(v),
                    _ => None,
                })
                .unwrap_or(default)
        };
        let flag = |props: &[Prop], name: &str, default: bool| {
            props
                .iter()
                .find(|p| p.name == name)
                .and_then(|p| match p.value {
                    PropValue::Bool(v) => Some(v),
                    _ => None,
                })
                .unwrap_or(default)
        };
        let text = |props: &[Prop], name: &str| {
            props
                .iter()
                .find(|p| p.name == name)
                .and_then(|p| match &p.value {
                    PropValue::Enum { value, .. } => Some(value.clone()),
                    PropValue::Text(s) => Some(s.clone()),
                    _ => None,
                })
        };

        let mut lights = Vec::new();
        let mut occluders = Vec::new();
        for entity in &self.scene.entities {
            // Ordinary render/UI/physics entities dominate real scenes. Reject
            // them before hierarchy activation, transform, and size work.
            if !entity.components.iter().any(|component| {
                matches!(
                    component,
                    Component::Core { name, .. }
                        if matches!(name.as_str(), "Light2D" | "LightOccluder2D")
                )
            }) {
                continue;
            }
            if !self.scene.is_active_in_tree(entity.id) {
                continue;
            }
            let Some(transform) = self.entity_world_transform(entity.id) else {
                continue;
            };
            let mut entity_size = None;
            for component in &entity.components {
                let Component::Core { name, props } = component else {
                    continue;
                };
                match name.as_str() {
                    "Light2D" => {
                        let kind = crate::lighting::LightKind::parse(
                            &text(props, "kind").unwrap_or_else(|| "point".into()),
                        );
                        let color = prop_color(props, "color").unwrap_or([255, 255, 255, 255]);
                        lights.push(crate::lighting::Light {
                            kind,
                            x: transform.x,
                            y: transform.y,
                            radius: num(props, "radius", 256.0),
                            color: to_color(color),
                            intensity: num(props, "intensity", 1.0),
                            falloff: num(props, "falloff", 2.0),
                            angle: transform.rotation + num(props, "angleOffset", 0.0).to_radians(),
                            cone: (num(props, "coneAngle", 60.0) * 0.5).to_radians(),
                            cone_softness: num(props, "coneSoftness", 0.35),
                            casts_shadows: flag(props, "castsShadows", true),
                            shadow_softness: num(props, "shadowSoftness", -1.0),
                        });
                    }
                    "LightOccluder2D" => {
                        let (size_x, size_y) = *entity_size.get_or_insert_with(|| {
                            editor_entity_size(&self.scene, entity, self.preview_root_size())
                        });
                        let half_w = size_x * transform.scale * 0.5;
                        let half_h = size_y * transform.scale * 0.5;
                        // Center on the (possibly rotated) visual bounds.
                        let (cos_r, sin_r) = (transform.rotation.cos(), transform.rotation.sin());
                        let offset_x = half_w * cos_r - half_h * sin_r;
                        let offset_y = half_w * sin_r + half_h * cos_r;
                        occluders.push(crate::lighting::Occluder {
                            cx: transform.x + offset_x,
                            cy: transform.y + offset_y,
                            half_w,
                            half_h,
                            rotation: transform.rotation,
                            shape: crate::lighting::OccluderShape::parse(
                                &text(props, "shape").unwrap_or_else(|| "box".into()),
                            ),
                        });
                    }
                    _ => {}
                }
            }
        }

        Some((config, lights, occluders))
    }

    /// Composite the scene lighting over the viewport, mirroring the runtime's
    /// per-pixel `scene × light` multiply (plus bloom, clamped). The light is
    /// evaluated on a coarse world-space grid and bilinearly upsampled — the
    /// same downsample-and-interpolate the runtime's own light map uses.
    ///
    /// The editor redraws continuously, so the grid (which traces shadow rays
    /// per light) is cached and only rebuilt when the camera, viewport, or
    /// lighting inputs change. The per-pixel composite still runs every frame —
    /// the scene beneath it is freshly rasterized — but it is threaded and cheap.
    fn composite_preview_lighting(
        &self,
        ui: &mut Ui,
        area: Rect,
        config: &crate::lighting::LightConfig,
        lights: &[crate::lighting::Light],
        occluders: &[crate::lighting::Occluder],
    ) {
        // Sample the light on a grid a few screen-pixels apart; lighting is
        // low-frequency, so bilinear upsampling is visually indistinguishable
        // from a per-pixel evaluation while doing a fraction of the work.
        const STEP: f32 = 4.0;
        let gx0 = area.x;
        let gy0 = area.y;
        let gw = (area.w / STEP).ceil() as usize + 2;
        let gh = (area.h / STEP).ceil() as usize + 2;

        // Rebuild the cached grid only when an input changed; otherwise every
        // continuous redraw would re-trace the same shadow rays.
        {
            let mut cache = self.preview_light_cache.borrow_mut();
            let fresh = cache.as_ref().is_some_and(|c| {
                c.cam_x == self.cam_x
                    && c.cam_y == self.cam_y
                    && c.cam_zoom == self.cam_zoom
                    && c.gx0 == gx0
                    && c.gy0 == gy0
                    && c.gw == gw
                    && c.gh == gh
                    && c.config == *config
                    && c.lights.as_slice() == lights
                    && c.occluders.as_slice() == occluders
            });
            if !fresh {
                // Screen-to-world: a world point sits at `area.xy + cam + world * zoom`.
                let z = self.cam_zoom.max(1e-3);
                let ox = area.x + self.cam_x;
                let oy = area.y + self.cam_y;
                let sampler = crate::lighting::LightSampler::new(config, lights, occluders);
                let mut grid = vec![(0.0f32, 0.0f32, 0.0f32); gw * gh];
                // Sampling casts shadow rays per light, so spread the grid across
                // worker threads (each row band is independent).
                let fill_rows = |chunk: &mut [(f32, f32, f32)], row_start: usize| {
                    let rows = chunk.len() / gw;
                    for local in 0..rows {
                        let wy = (gy0 + (row_start + local) as f32 * STEP - oy) / z;
                        for gx in 0..gw {
                            let wx = (gx0 + gx as f32 * STEP - ox) / z;
                            chunk[local * gw + gx] = sampler.sample(wx, wy);
                        }
                    }
                };
                let workers = std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(1)
                    .clamp(1, 16)
                    .min(gh.max(1));
                if workers <= 1 || gw * gh < 4096 {
                    fill_rows(&mut grid, 0);
                } else {
                    let rows_per = gh.div_ceil(workers).max(1);
                    let band = rows_per * gw;
                    let fill_rows = &fill_rows;
                    std::thread::scope(|scope| {
                        for (index, chunk) in grid.chunks_mut(band).enumerate() {
                            scope.spawn(move || fill_rows(chunk, index * rows_per));
                        }
                    });
                }
                *cache = Some(PreviewLightGrid {
                    cam_x: self.cam_x,
                    cam_y: self.cam_y,
                    cam_zoom: self.cam_zoom,
                    gx0,
                    gy0,
                    gw,
                    gh,
                    config: *config,
                    lights: lights.to_vec(),
                    occluders: occluders.to_vec(),
                    grid,
                });
            }
        }

        // Composite the cached grid over the scene background region (inside the
        // 1px viewport frame); exposure is already folded into the sampled values.
        let cache = self.preview_light_cache.borrow();
        let cached = cache.as_ref().expect("grid was built above");
        let grid = &cached.grid;
        let sample = |px: f32, py: f32| {
            let fx = ((px - gx0) / STEP).clamp(0.0, (gw - 1) as f32);
            let fy = ((py - gy0) / STEP).clamp(0.0, (gh - 1) as f32);
            let x0 = fx.floor() as usize;
            let y0 = fy.floor() as usize;
            let x1 = (x0 + 1).min(gw - 1);
            let y1 = (y0 + 1).min(gh - 1);
            let tx = fx - x0 as f32;
            let ty = fy - y0 as f32;
            let at = |x: usize, y: usize| grid[y * gw + x];
            let lerp = |a: (f32, f32, f32), b: (f32, f32, f32), t: f32| {
                (
                    a.0 + (b.0 - a.0) * t,
                    a.1 + (b.1 - a.1) * t,
                    a.2 + (b.2 - a.2) * t,
                )
            };
            let top = lerp(at(x0, y0), at(x1, y0), tx);
            let bottom = lerp(at(x0, y1), at(x1, y1), tx);
            lerp(top, bottom, ty)
        };
        ui.painter
            .composite_light(area.shrink(1.0), config.bloom, sample);
    }

    /// Accumulated world rotation (radians) for an entity, matching what the
    /// runtime applies. Falls back to the entity's own rotation if the world
    /// transform can't be resolved.
    fn entity_world_rotation(&self, entity: &Entity) -> f32 {
        self.entity_world_transform(entity.id)
            .map(|t| t.rotation)
            .unwrap_or(entity.rotation)
    }

    fn entity_screen_rect(&self, entity: &Entity, area: Rect) -> Option<Rect> {
        let transform = self.entity_world_transform(entity.id)?;
        let (size_x, size_y) = editor_entity_size(&self.scene, entity, self.preview_root_size());
        let z = self.cam_zoom;
        Some(Rect::new(
            area.x + self.cam_x + transform.x * z,
            area.y + self.cam_y + transform.y * z,
            size_x * transform.scale * z,
            size_y * transform.scale * z,
        ))
    }

    fn world_origin_to_local_position(
        &self,
        entity_id: u64,
        world_x: f32,
        world_y: f32,
    ) -> Option<(f32, f32)> {
        scene_world_origin_to_local_position(
            &self.scene,
            entity_id,
            world_x,
            world_y,
            self.preview_root_size(),
        )
    }

    fn preview_root_size(&self) -> (f32, f32) {
        if self.config.settings.mobile_emulator {
            let portrait = self.config.settings.mobile_orientation != "landscape";
            return if portrait {
                (
                    crate::mobile_emulation::DEFAULT_WIDTH as f32,
                    crate::mobile_emulation::DEFAULT_HEIGHT as f32,
                )
            } else {
                (
                    crate::mobile_emulation::DEFAULT_HEIGHT as f32,
                    crate::mobile_emulation::DEFAULT_WIDTH as f32,
                )
            };
        }
        (
            self.project_window.width.max(1.0),
            self.project_window.height.max(1.0),
        )
    }

    fn mobile_emulation_profile(&self) -> crate::mobile_emulation::MobileEmulation {
        crate::mobile_emulation::MobileEmulation {
            enabled: self.config.settings.mobile_emulator,
            width: crate::mobile_emulation::DEFAULT_WIDTH,
            height: crate::mobile_emulation::DEFAULT_HEIGHT,
            orientation: if self.config.settings.mobile_orientation == "landscape" {
                crate::mobile_emulation::MobileOrientation::Landscape
            } else {
                crate::mobile_emulation::MobileOrientation::Portrait
            },
            wifi: self.config.settings.mobile_wifi,
            cellular: self.config.settings.mobile_cellular,
            low_power: self.config.settings.mobile_low_power,
        }
    }

    fn draw_entity(&self, ui: &mut Ui, entity: &Entity, rect: Rect, zoom: f32) {
        // Rotate the whole entity about its origin (its top-left, which is the
        // runtime's default rotation pivot). All the component draw paths below
        // go through the painter, so they inherit the rotation for free. Entities
        // are drawn at their true colors; scene lighting is composited over the
        // whole viewport afterward (see [`Self::composite_preview_lighting`]).
        let angle = self.entity_world_rotation(entity);
        let prev_rot = ui.painter.push_rotation(rect.x, rect.y, angle);
        let world_scale = self
            .entity_world_transform(entity.id)
            .map(|transform| transform.scale)
            .unwrap_or_else(|| editor_entity_scale(entity));
        let mut drew = false;
        for component in &entity.components {
            if let Component::Core { name, props } = component {
                let color = prop_color(props, "color").unwrap_or([200, 200, 200, 255]);
                let prop_num = |n: &str, d: f32| {
                    props
                        .iter()
                        .find(|p| p.name == n)
                        .and_then(|p| match p.value {
                            PropValue::Number(v) => Some(v),
                            _ => None,
                        })
                        .unwrap_or(d)
                };
                let prop_img = |n: &str| {
                    props
                        .iter()
                        .find(|p| p.name == n)
                        .and_then(|p| match &p.value {
                            PropValue::Image(s) => Some(s.clone()),
                            _ => None,
                        })
                };
                let prop_enum = |n: &str| {
                    props
                        .iter()
                        .find(|p| p.name == n)
                        .and_then(|p| match &p.value {
                            PropValue::Enum { value, .. } => Some(value.clone()),
                            PropValue::Text(s) => Some(s.clone()),
                            _ => None,
                        })
                };
                let prop_int = |n: &str, d: i32| {
                    props
                        .iter()
                        .find(|p| p.name == n)
                        .and_then(|p| match p.value {
                            PropValue::Int(v) => Some(v),
                            _ => None,
                        })
                        .unwrap_or(d)
                };
                match name.as_str() {
                    "Rect2D" | "ScrollList" => {
                        let radius = prop_num("corner_radius", 0.0) * zoom;
                        ui.painter.fill_round_rect(rect, radius, color);
                        drew = true;
                    }
                    "Shape2D" => {
                        // Mirror the runtime Shape2D primitives so the preview
                        // matches: offset/size, box, inscribed circle, or a
                        // corner triangle.
                        let shape = prop_enum("shape")
                            .unwrap_or_else(|| "box".into())
                            .to_ascii_lowercase();
                        let sx = prop_num("offset_x", 0.0) * zoom;
                        let sy = prop_num("offset_y", 0.0) * zoom;
                        let sw = match prop_num("size_x", 0.0) {
                            v if v > 0.0 => v * world_scale * zoom,
                            _ => rect.w,
                        };
                        let sh = match prop_num("size_y", 0.0) {
                            v if v > 0.0 => v * world_scale * zoom,
                            _ => rect.h,
                        };
                        let shape_rect = Rect::new(rect.x + sx, rect.y + sy, sw, sh);
                        match shape.as_str() {
                            "circle" => {
                                let r = (shape_rect.w.min(shape_rect.h) * 0.5).max(0.0);
                                ui.painter.fill_circle(
                                    shape_rect.x + shape_rect.w * 0.5,
                                    shape_rect.y + shape_rect.h * 0.5,
                                    r,
                                    color,
                                );
                            }
                            "triangle"
                            | "right_triangle"
                            | "righttriangle"
                            | "rightangledtriangle" => {
                                let corner = prop_enum("triangle_corner")
                                    .unwrap_or_else(|| "bl".into())
                                    .to_ascii_lowercase();
                                let (x0, y0, x1, y1) = (
                                    shape_rect.x,
                                    shape_rect.y,
                                    shape_rect.right(),
                                    shape_rect.bottom(),
                                );
                                let (a, b, c) = match corner.as_str() {
                                    "br" | "bottomright" | "rightbottom" => {
                                        ((x1, y1), (x1, y0), (x0, y1))
                                    }
                                    "tl" | "topleft" | "lefttop" => ((x0, y0), (x1, y0), (x0, y1)),
                                    "tr" | "topright" | "righttop" => {
                                        ((x1, y0), (x0, y0), (x1, y1))
                                    }
                                    _ => ((x0, y1), (x0, y0), (x1, y1)),
                                };
                                ui.painter.fill_triangle(a, b, c, color);
                            }
                            _ => ui.painter.fill_rect(shape_rect, color),
                        }
                        drew = true;
                    }
                    "ParticleSystem2D" => {
                        // Deterministic representative particles make the
                        // authored motion readable without advancing editor time.
                        let lifetime = prop_num("lifetime", 1.5).max(0.001);
                        let rate = prop_num("emission_rate", 12.0).max(0.0);
                        let speed = prop_num("speed", 80.0);
                        let direction = prop_num("direction", -90.0).to_radians();
                        let spread = prop_num("spread", 30.0).abs().to_radians();
                        let start_size = prop_num("start_size", 8.0).max(0.0);
                        let end_size = prop_num("end_size", 2.0).max(0.0);
                        let start_color =
                            prop_color(props, "start_color").unwrap_or([255, 184, 76, 255]);
                        let end_color = prop_color(props, "end_color").unwrap_or([255, 92, 40, 0]);
                        let color_sequence = props
                            .iter()
                            .find(|prop| prop.name == "color_sequence")
                            .and_then(|prop| match &prop.value {
                                PropValue::ColorSequence(keypoints) => Some(keypoints.clone()),
                                _ => None,
                            })
                            .unwrap_or_else(|| {
                                vec![
                                    ColorKeypoint {
                                        time: 0.0,
                                        color: start_color,
                                    },
                                    ColorKeypoint {
                                        time: 1.0,
                                        color: end_color,
                                    },
                                ]
                            });
                        let transparency_sequence = props
                            .iter()
                            .find(|prop| prop.name == "transparency_sequence")
                            .and_then(|prop| match &prop.value {
                                PropValue::NumberSequence(keypoints) => Some(keypoints.clone()),
                                _ => None,
                            })
                            .unwrap_or_else(|| {
                                vec![
                                    NumberKeypoint {
                                        time: 0.0,
                                        value: 1.0 - start_color[3] as f32 / 255.0,
                                    },
                                    NumberKeypoint {
                                        time: 1.0,
                                        value: 1.0 - end_color[3] as f32 / 255.0,
                                    },
                                ]
                            });
                        let emitter = prop_enum("shape").unwrap_or_else(|| "point".into());
                        let emitter_radius = prop_num("radius", 32.0).max(0.0) * world_scale * zoom;
                        let gravity_x = prop_num("gravity_x", 0.0);
                        let gravity_y = prop_num("gravity_y", 60.0);
                        let particle_image =
                            prop_img("image").and_then(|path| self.load_image(&path));
                        let max_particles = props
                            .iter()
                            .find(|prop| prop.name == "max_particles")
                            .and_then(|prop| match prop.value {
                                PropValue::Int(value) => Some(value),
                                _ => None,
                            })
                            .unwrap_or(256)
                            .clamp(1, 10_000) as usize;
                        let count = ((rate * lifetime).round() as usize)
                            .clamp(1, 32)
                            .min(max_particles);
                        let mut seed = (entity.id as u32)
                            .wrapping_mul(747_796_405)
                            .wrapping_add(2_891_336_453);
                        let mut random = || {
                            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                            seed as f32 / u32::MAX as f32
                        };
                        for index in 0..count {
                            let phase = (index as f32 + 0.5) / count as f32;
                            let age = phase * lifetime;
                            let particle_angle = direction + (random() - 0.5) * spread;
                            let (emit_x, emit_y) = match emitter.as_str() {
                                "box" => (random() * rect.w, random() * rect.h),
                                "circle" => {
                                    let a = random() * std::f32::consts::TAU;
                                    let r = random().sqrt() * emitter_radius;
                                    (a.cos() * r, a.sin() * r)
                                }
                                _ => (0.0, 0.0),
                            };
                            let px = rect.x
                                + emit_x
                                + particle_angle.cos() * speed * age * world_scale * zoom
                                + gravity_x * age * age * 0.5 * zoom;
                            let py = rect.y
                                + emit_y
                                + particle_angle.sin() * speed * age * world_scale * zoom
                                + gravity_y * age * age * 0.5 * zoom;
                            let size =
                                (start_size + (end_size - start_size) * phase) * world_scale * zoom;
                            let mut particle_color = sample_color_sequence(&color_sequence, phase);
                            particle_color[3] = ((1.0
                                - sample_number_sequence(&transparency_sequence, phase)
                                    .clamp(0.0, 1.0))
                                * 255.0)
                                .round() as u8;
                            if let Some(image) = &particle_image {
                                ui.painter.draw_image(
                                    image,
                                    Rect::new(px - size * 0.5, py - size * 0.5, size, size),
                                    None,
                                    particle_color,
                                );
                            } else {
                                ui.painter.fill_circle(px, py, size * 0.5, particle_color);
                            }
                        }
                        drew = true;
                    }
                    "Button" | "Dropdown" | "TextInput" => {
                        let radius = prop_num("corner_radius", 6.0) * zoom;
                        let fill_default = match name.as_str() {
                            "Button" => [14, 99, 156, 255],
                            _ => [60, 60, 60, 255],
                        };
                        let fill = prop_color(props, "background_color").unwrap_or(fill_default);
                        ui.painter.fill_round_rect(rect, radius, fill);
                        let defaults = match name.as_str() {
                            "Button" => TextPreviewDefaults {
                                default_scale: 18.0,
                                default_align_x: TextAlignX::Center,
                                default_align_y: TextAlignY::Center,
                                default_text_scale: TextScaleMode::Fit,
                                default_wrap: TextWrapMode::None,
                                default_size_mode_uses_entity: true,
                                color_names: &["text_color", "textColor"],
                                fallback_color: [20, 20, 20, 255],
                            },
                            "TextInput" => TextPreviewDefaults {
                                default_scale: 18.0,
                                default_align_x: TextAlignX::Left,
                                default_align_y: TextAlignY::Center,
                                default_text_scale: TextScaleMode::None,
                                default_wrap: TextWrapMode::None,
                                default_size_mode_uses_entity: true,
                                color_names: &["text_color", "textColor"],
                                fallback_color: [20, 20, 20, 255],
                            },
                            _ => TextPreviewDefaults {
                                default_scale: 18.0,
                                default_align_x: TextAlignX::Left,
                                default_align_y: TextAlignY::Center,
                                default_text_scale: TextScaleMode::FitWidth,
                                default_wrap: TextWrapMode::None,
                                default_size_mode_uses_entity: true,
                                color_names: &["text_color", "textColor"],
                                fallback_color: [20, 20, 20, 255],
                            },
                        };
                        if prop_string_like(props, &["text"]).is_some() {
                            self.draw_text_preview(ui, props, rect, zoom, defaults);
                        }
                        drew = true;
                    }
                    "TextBox" | "TextLabel" | "RudimentaryTextLabel" => {
                        let defaults = TextPreviewDefaults {
                            default_scale: 32.0,
                            default_align_x: TextAlignX::Left,
                            default_align_y: TextAlignY::Top,
                            default_text_scale: TextScaleMode::Fit,
                            default_wrap: TextWrapMode::None,
                            default_size_mode_uses_entity: true,
                            color_names: &["color"],
                            fallback_color: color,
                        };
                        drew |= self.draw_text_preview(ui, props, rect, zoom, defaults);
                    }
                    "Sprite2D" | "Image2D" => {
                        if let Some(img) = prop_img("image").and_then(|p| self.load_image(&p)) {
                            ui.painter.draw_image(&img, rect, None, color);
                        } else {
                            self.draw_missing_image(ui, rect, color);
                        }
                        drew = true;
                    }
                    "SpriteSheet2D" => {
                        if let Some(img) = prop_img("image").and_then(|p| self.load_image(&p)) {
                            let frame_w = prop_num("frame_width", 32.0)
                                .max(1.0)
                                .min(img.width() as f32);
                            let frame_h = prop_num("frame_height", 32.0)
                                .max(1.0)
                                .min(img.height() as f32);
                            let spacing = prop_num("spacing", 0.0).max(0.0);
                            let margin = prop_num("margin", 0.0)
                                .max(0.0)
                                .min(((img.width() as f32 - frame_w) * 0.5).max(0.0))
                                .min(((img.height() as f32 - frame_h) * 0.5).max(0.0));
                            let usable_w = (img.width() as f32 - margin * 2.0).max(frame_w);
                            let usable_h = (img.height() as f32 - margin * 2.0).max(frame_h);
                            let available_columns =
                                (((usable_w + spacing) / (frame_w + spacing)).floor() as i32)
                                    .max(1);
                            let configured_columns = prop_int("columns", 0).max(0);
                            let columns = if configured_columns == 0 {
                                available_columns
                            } else {
                                configured_columns.min(available_columns).max(1)
                            };
                            let rows = (((usable_h + spacing) / (frame_h + spacing)).floor()
                                as i32)
                                .max(1);
                            let available_frames = columns.saturating_mul(rows).max(1);
                            let configured_count = prop_int("frame_count", 0);
                            let frame_count = if configured_count <= 0 {
                                available_frames
                            } else {
                                configured_count.min(available_frames).max(1)
                            };
                            let frame = prop_int("frame", 0).clamp(0, frame_count - 1);
                            let source = Rect::new(
                                margin + (frame % columns) as f32 * (frame_w + spacing),
                                margin + (frame / columns) as f32 * (frame_h + spacing),
                                frame_w,
                                frame_h,
                            );
                            ui.painter.draw_image(&img, rect, Some(source), color);
                        } else {
                            self.draw_missing_image(ui, rect, color);
                        }
                        drew = true;
                    }
                    "NineSliceSprite2D" => {
                        if let Some(img) = prop_img("image").and_then(|p| self.load_image(&p)) {
                            draw_nine_slice(
                                &mut ui.painter,
                                &img,
                                rect,
                                prop_num("slice_left", 0.0),
                                prop_num("slice_right", 0.0),
                                prop_num("slice_top", 0.0),
                                prop_num("slice_bottom", 0.0),
                                color,
                                zoom,
                            );
                        } else {
                            self.draw_missing_image(ui, rect, color);
                        }
                        drew = true;
                    }
                    "TileTexture2D" => {
                        if let Some(img) = prop_img("image").and_then(|p| self.load_image(&p)) {
                            draw_tiled(
                                &mut ui.painter,
                                &img,
                                rect,
                                prop_num("tile_width", 32.0),
                                prop_num("tile_height", 32.0),
                                color,
                                zoom,
                            );
                        } else {
                            self.draw_missing_image(ui, rect, color);
                        }
                        drew = true;
                    }
                    "Tilemap2D" => {
                        if let Some(img) = prop_img("image").and_then(|p| self.load_image(&p)) {
                            let tiles = prop_enum("tiles").unwrap_or_default();
                            draw_tilemap(
                                &mut ui.painter,
                                &img,
                                rect,
                                prop_int("map_width", 1).max(1) as usize,
                                prop_int("map_height", 1).max(1) as usize,
                                prop_num("tile_width", 32.0),
                                prop_num("tile_height", 32.0),
                                prop_num("spacing", 0.0),
                                prop_num("margin", 0.0),
                                &tiles,
                                color,
                            );
                        } else {
                            self.draw_missing_image(ui, rect, color);
                        }
                        drew = true;
                    }
                    "Panel" | "Frame" => {
                        let bg = prop_color(props, "background_color").unwrap_or([37, 37, 38, 255]);
                        let border = prop_color(props, "border_color").unwrap_or([69, 69, 69, 255]);
                        let radius = prop_num("corner_radius", 4.0) * zoom;
                        ui.painter.fill_round_rect(rect, radius, bg);
                        ui.painter.stroke_round_rect(rect, radius, border);
                        drew = true;
                    }
                    "Slider" => {
                        let track =
                            prop_color(props, "background_color").unwrap_or([60, 60, 60, 255]);
                        let fill = prop_color(props, "fill_color").unwrap_or([0, 122, 204, 255]);
                        let thumb =
                            prop_color(props, "thumb_color").unwrap_or([204, 204, 204, 255]);
                        let vertical = prop_enum("orientation")
                            .map(|value| value.eq_ignore_ascii_case("vertical"))
                            .unwrap_or(false);
                        let min = prop_num("min", 0.0);
                        let max = prop_num("max", 100.0);
                        let range = max - min;
                        let fraction = if range.abs() > f32::EPSILON {
                            ((prop_num("value", 0.0) - min) / range).clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        let thumb_size = prop_num("thumb_size", 16.0).max(0.0) * zoom;
                        let half = thumb_size * 0.5;
                        let thickness = prop_num("track_thickness", 6.0).max(0.0) * zoom;
                        let radius = prop_num("corner_radius", 3.0) * zoom;
                        if vertical {
                            let tw = if thickness > 0.0 {
                                thickness.min(rect.w)
                            } else {
                                rect.w
                            };
                            let track_rect =
                                Rect::new(rect.x + (rect.w - tw) * 0.5, rect.y, tw, rect.h);
                            ui.painter.fill_round_rect(track_rect, radius, track);
                            let travel = (rect.h - thumb_size).max(0.0);
                            let cy = rect.y + rect.h - half - fraction * travel;
                            let fill_rect = Rect::new(
                                track_rect.x,
                                cy,
                                tw,
                                (track_rect.y + track_rect.h - cy).max(0.0),
                            );
                            ui.painter.fill_round_rect(fill_rect, radius, fill);
                            ui.painter.fill_round_rect(
                                Rect::new(
                                    track_rect.x + tw * 0.5 - half,
                                    cy - half,
                                    thumb_size,
                                    thumb_size,
                                ),
                                half,
                                thumb,
                            );
                        } else {
                            let th = if thickness > 0.0 {
                                thickness.min(rect.h)
                            } else {
                                rect.h
                            };
                            let track_rect =
                                Rect::new(rect.x, rect.y + (rect.h - th) * 0.5, rect.w, th);
                            ui.painter.fill_round_rect(track_rect, radius, track);
                            let travel = (rect.w - thumb_size).max(0.0);
                            let cx = rect.x + half + fraction * travel;
                            let fill_rect = Rect::new(
                                track_rect.x,
                                track_rect.y,
                                (cx - track_rect.x).max(0.0),
                                th,
                            );
                            ui.painter.fill_round_rect(fill_rect, radius, fill);
                            ui.painter.fill_round_rect(
                                Rect::new(
                                    cx - half,
                                    track_rect.y + th * 0.5 - half,
                                    thumb_size,
                                    thumb_size,
                                ),
                                half,
                                thumb,
                            );
                        }
                        drew = true;
                    }
                    _ => {}
                }
            }
        }
        if !drew {
            ui.painter.stroke_rect(rect, self.config.theme.text_dim);
            ui.painter.text_clipped(
                rect.x + 4.0,
                rect.y + 4.0,
                &entity.name,
                12.0,
                self.config.theme.text_dim,
                (rect.w - 8.0).max(0.0),
            );
        }
        ui.painter.set_rotation_raw(prev_rot);
    }

    fn draw_text_preview(
        &self,
        ui: &mut Ui,
        props: &[Prop],
        rect: Rect,
        zoom: f32,
        defaults: TextPreviewDefaults,
    ) -> bool {
        let Some(mut request) =
            text_preview_request(&self.project_root, props, rect, zoom, defaults)
        else {
            return false;
        };
        if prop_string_like(props, &["antialiasing"])
            .is_none_or(|mode| mode.eq_ignore_ascii_case("inherit"))
        {
            request.antialiasing = match self.scene.antialiasing.as_str() {
                "off" => TextAntialiasing::Off,
                "standard" => TextAntialiasing::Standard,
                _ => TextAntialiasing::High,
            };
        }
        let Some(sprite) = renderer::rasterize_text_sprite(&request) else {
            return false;
        };
        // Clip to the rotated extent so a rotated label isn't cropped to its
        // axis-aligned bounds (a no-op when the painter isn't rotating).
        let clip_rect = ui.painter.rotated_bounds(rect);
        let clip = ui.painter.push_clip(clip_rect);
        ui.painter.draw_image(
            sprite.image.as_ref(),
            Rect::new(sprite.dest.x, sprite.dest.y, sprite.dest.w, sprite.dest.h),
            None,
            [255, 255, 255, 255],
        );
        ui.painter.set_clip_raw(clip);
        true
    }

    /// Placeholder for an image component whose asset is missing/unset.
    fn draw_missing_image(&self, ui: &mut Ui, rect: Rect, tint: [u8; 4]) {
        ui.painter
            .fill_rect(rect, [tint[0] / 3, tint[1] / 3, tint[2] / 3, 255]);
        ui.painter.stroke_rect(rect, self.config.theme.accent);
        ui.painter.icon_centered(
            rect.x + rect.w / 2.0,
            rect.y + rect.h / 2.0,
            icon::IMAGE,
            (rect.w.min(rect.h) * 0.4).clamp(10.0, 40.0),
            [255, 255, 255, 150],
        );
    }

    /// Draw a green outline for a selected entity's Collider2D, since the
    /// collider's shape/size/offset often differ from the entity bounds.
    fn draw_collider_preview(
        &self,
        ui: &mut Ui,
        entity: &Entity,
        rect: Rect,
        z: f32,
        world_scale: f32,
    ) {
        for component in &entity.components {
            if let Component::Core { name, props } = component {
                if name != "Collider2D" {
                    continue;
                }
                let num = |n: &str| {
                    props
                        .iter()
                        .find(|p| p.name == n)
                        .and_then(|p| match p.value {
                            PropValue::Number(v) => Some(v),
                            _ => None,
                        })
                };
                let shape = props
                    .iter()
                    .find(|p| p.name == "shape")
                    .and_then(|p| match &p.value {
                        PropValue::Enum { value, .. } => Some(value.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| "box".to_string());
                let ox = num("offset_x").unwrap_or(0.0) * z;
                let oy = num("offset_y").unwrap_or(0.0) * z;
                // Size 0 means "use the entity bounds".
                let cw = match num("size_x") {
                    Some(v) if v > 0.0 => v * world_scale * z,
                    _ => rect.w,
                };
                let ch = match num("size_y") {
                    Some(v) if v > 0.0 => v * world_scale * z,
                    _ => rect.h,
                };
                let cr = Rect::new(rect.x + ox, rect.y + oy, cw, ch);
                let green = [80, 220, 90, 255];
                match shape.to_ascii_lowercase().as_str() {
                    "circle" => {
                        ui.painter.stroke_round_rect(cr, cw.min(ch) / 2.0, green);
                    }
                    "triangle" | "right_triangle" | "righttriangle" | "rightangledtriangle" => {
                        let corner = props
                            .iter()
                            .find(|p| p.name == "triangle_corner")
                            .and_then(|p| match &p.value {
                                PropValue::Enum { value, .. } => Some(value.clone()),
                                PropValue::Text(value) => Some(value.clone()),
                                _ => None,
                            })
                            .unwrap_or_else(|| "bl".to_string())
                            .to_ascii_lowercase();
                        let (x0, y0, x1, y1) = (cr.x, cr.y, cr.right(), cr.bottom());
                        let (a, b, c) = match corner.as_str() {
                            "br" | "bottomright" | "rightbottom" => ((x1, y1), (x1, y0), (x0, y1)),
                            "tl" | "topleft" | "lefttop" => ((x0, y0), (x1, y0), (x0, y1)),
                            "tr" | "topright" | "righttop" => ((x1, y0), (x0, y0), (x1, y1)),
                            _ => ((x0, y1), (x0, y0), (x1, y1)),
                        };
                        ui.painter.stroke_line(a.0, a.1, b.0, b.1, green);
                        ui.painter.stroke_line(b.0, b.1, c.0, c.1, green);
                        ui.painter.stroke_line(c.0, c.1, a.0, a.1, green);
                    }
                    _ => ui.painter.stroke_rect(cr, green),
                }
                ui.painter
                    .icon_centered(cr.x + 7.0, cr.y + 7.0, icon::BORDER_ALL, 11.0, green);
            }
        }
    }

    /// Screen position of the rotation gizmo's knob for a selected entity:
    /// straight up from the top edge centre, then rotated by `angle` about the
    /// entity origin.
    fn rotate_handle_knob(&self, rect: Rect, angle: f32) -> (f32, f32) {
        let cx = rect.x + rect.w / 2.0;
        let knob_y = rect.y - ROT_HANDLE_DIST;
        rotate_point_about(cx, knob_y, rect.x, rect.y, angle)
    }

    fn move_handle_rect(&self, rect: Rect) -> Rect {
        let size = 18.0;
        Rect::new(
            rect.x + rect.w * 0.5 - size * 0.5,
            rect.y + rect.h * 0.5 - size * 0.5,
            size,
            size,
        )
    }

    /// If the cursor is over the move gizmo's X (right) or Y (up) arrow, return
    /// that axis as a world-space unit vector to constrain the drag along. The
    /// arms rotate with the entity, so the axes are the entity's local X/Y.
    fn move_axis_hit(&self, rect: Rect, angle: f32, mx: f32, my: f32) -> Option<(f32, f32)> {
        let center_x = rect.x + rect.w * 0.5;
        let center_y = rect.y + rect.h * 0.5;
        // Express the cursor in the gizmo's unrotated, centre-origin frame.
        let (dx, dy) = (mx - center_x, my - center_y);
        let (sin, cos) = (angle.sin(), angle.cos());
        let lx = dx * cos + dy * sin;
        let ly = -dx * sin + dy * cos;
        // X arm points right (+local X); Y arm points up (-local Y). The centre
        // square (|.| < 9) belongs to the free-move handle.
        if ly.abs() <= 6.0 && (9.0..=36.0).contains(&lx) {
            Some((cos, sin))
        } else if lx.abs() <= 6.0 && (-36.0..=-9.0).contains(&ly) {
            Some((-sin, cos))
        } else {
            None
        }
    }

    /// Draw the four screen-aligned scale corner handles at the (possibly
    /// rotated) corners of `rect`, highlighting whichever the cursor is over.
    fn draw_scale_handles(&self, ui: &mut Ui, rect: Rect, angle: f32, mx: f32, my: f32) {
        for (cx, cy) in [
            (rect.x, rect.y),
            (rect.right(), rect.y),
            (rect.x, rect.bottom()),
            (rect.right(), rect.bottom()),
        ] {
            let (hx, hy) = rotate_point_about(cx, cy, rect.x, rect.y, angle);
            let hot = (mx - hx).abs() <= 7.0 && (my - hy).abs() <= 7.0;
            let s = if hot { 5.0 } else { 4.0 };
            ui.painter.fill_rect(
                Rect::new(hx - s, hy - s, s * 2.0, s * 2.0),
                SCALE_HANDLE_COLOR,
            );
            ui.painter.fill_rect(
                Rect::new(hx - s + 1.5, hy - s + 1.5, s * 2.0 - 3.0, s * 2.0 - 3.0),
                if hot {
                    [255, 255, 255, 255]
                } else {
                    [40, 40, 40, 255]
                },
            );
        }
    }

    fn draw_move_handle(&self, ui: &mut Ui, rect: Rect, angle: f32, mx: f32, my: f32) {
        let center_x = rect.x + rect.w * 0.5;
        let center_y = rect.y + rect.h * 0.5;
        // Which part of the gizmo the cursor is over, so it highlights.
        let axis = self.move_axis_hit(rect, angle, mx, my);
        let center_hot = self.move_handle_rect(rect).contains(mx, my);
        let (sin, cos) = (angle.sin(), angle.cos());
        let x_hot = axis == Some((cos, sin));
        let y_hot = axis == Some((-sin, cos));
        let x_color = if x_hot {
            [255, 255, 255, 255]
        } else {
            MOVE_X_COLOR
        };
        let y_color = if y_hot {
            [255, 255, 255, 255]
        } else {
            MOVE_Y_COLOR
        };
        let prev = ui.painter.push_rotation(center_x, center_y, angle);
        // Y axis (vertical) green, X axis (horizontal) red.
        ui.painter.fill_rect(
            Rect::new(center_x - 1.0, center_y - 24.0, 2.0, 48.0),
            y_color,
        );
        ui.painter.fill_rect(
            Rect::new(center_x - 24.0, center_y - 1.0, 48.0, 2.0),
            x_color,
        );
        ui.painter.fill_triangle(
            (center_x, center_y - 31.0),
            (center_x - 5.0, center_y - 22.0),
            (center_x + 5.0, center_y - 22.0),
            y_color,
        );
        ui.painter.fill_triangle(
            (center_x + 31.0, center_y),
            (center_x + 22.0, center_y - 5.0),
            (center_x + 22.0, center_y + 5.0),
            x_color,
        );
        ui.painter.set_rotation_raw(prev);
        let handle = self.move_handle_rect(rect);
        ui.painter
            .fill_round_rect(handle, 4.0, self.config.theme.selection);
        ui.painter.fill_round_rect(
            handle.shrink(3.0),
            2.0,
            if center_hot {
                [255, 255, 255, 255]
            } else {
                self.config.theme.selection
            },
        );
    }

    /// Draw the rotation gizmo (stalk + knob) for the selected entity. The
    /// stalk rotates with the entity; the knob is a rotation-invariant circle.
    fn draw_rotate_handle(&self, ui: &mut Ui, rect: Rect, angle: f32, hot: bool) {
        let prev = ui.painter.push_rotation(rect.x, rect.y, angle);
        let cx = rect.x + rect.w / 2.0;
        let knob_y = rect.y - ROT_HANDLE_DIST;
        ui.painter.fill_rect(
            Rect::new(cx - 0.75, knob_y, 1.5, ROT_HANDLE_DIST),
            self.config.theme.selection,
        );
        let r = if hot { 6.0 } else { 5.0 };
        ui.painter.fill_round_rect(
            Rect::new(cx - r, knob_y - r, r * 2.0, r * 2.0),
            r,
            self.config.theme.selection,
        );
        ui.painter.fill_round_rect(
            Rect::new(cx - r + 1.5, knob_y - r + 1.5, r * 2.0 - 3.0, r * 2.0 - 3.0),
            r - 1.5,
            if hot {
                [255, 255, 255, 255]
            } else {
                [40, 40, 40, 255]
            },
        );
        ui.painter.set_rotation_raw(prev);
    }

    fn handle_script_drop(&mut self, ui: &mut Ui, area: Rect) {
        let Some(path) = self.script_drag.clone() else {
            return;
        };
        let (mx, my) = (ui.input.mouse_x, ui.input.mouse_y);
        if !area.contains(mx, my) {
            return;
        }

        let target = self.viewport_hit(area, mx, my);
        if let Some(id) = target {
            if let Some(entity) = self.scene.entity(id) {
                if let Some(rect) = self.entity_screen_rect(entity, area) {
                    ui.painter
                        .stroke_rect(rect.shrink(-2.0), self.config.theme.accent);
                }
            }
        }
        if ui.input.mouse_down {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy())
                .unwrap_or_default();
            let label = format!(
                "{} → {}",
                name,
                target
                    .and_then(|id| self.scene.entity(id))
                    .map(|entity| entity.name.as_str())
                    .unwrap_or("entity")
            );
            let width = (ui.painter.text_width(&label, 13.0) + 34.0).min((area.w - 8.0).max(0.0));
            let bubble_x = (mx + 8.0).min(area.right() - width - 4.0).max(area.x + 4.0);
            let bubble_y = (my + 8.0).min(area.bottom() - 26.0).max(area.y + 4.0);
            let bubble = Rect::new(bubble_x, bubble_y, width, 22.0);
            ui.painter.fill_round_rect(bubble, 4.0, [0, 0, 0, 210]);
            ui.painter.icon_centered(
                bubble.x + 12.0,
                bubble.y + 11.0,
                icon::CODE,
                13.0,
                self.config.theme.accent,
            );
            ui.painter.text_clipped(
                bubble.x + 22.0,
                bubble.y + 4.0,
                &label,
                13.0,
                self.config.theme.text,
                (bubble.w - 28.0).max(0.0),
            );
            ui.wants_redraw = true;
        } else {
            if let Some(id) = target {
                self.add_script_component_from_path(id, &path);
                self.select_only(id);
            }
            self.script_drag = None;
        }
    }

    fn project_relative_path(&self, path: &Path) -> String {
        path.strip_prefix(&self.project_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn assign_mesh_to_entity(&mut self, id: u64, path: &Path) {
        let relative = self.project_relative_path(path);
        let Some(entity) = self.scene.entity_mut(id) else {
            return;
        };
        if let Some(props) = entity
            .components
            .iter_mut()
            .find_map(|component| match component {
                Component::Core { name, props } if name == "MeshRenderer3D" => Some(props),
                _ => None,
            })
        {
            if let Some(prop) = props.iter_mut().find(|prop| prop.name == "mesh_path") {
                prop.value = PropValue::Mesh(relative.clone());
            }
        } else {
            let mut component = Component::core("MeshRenderer3D");
            if let Component::Core { props, .. } = &mut component
                && let Some(prop) = props.iter_mut().find(|prop| prop.name == "mesh_path")
            {
                prop.value = PropValue::Mesh(relative.clone());
            }
            entity.components.push(component);
        }
        self.mesh_cache.borrow_mut().remove(&relative);
        self.mark_dirty();
        self.status = format!("Assigned mesh {relative}");
    }

    fn add_mesh_entity_at(&mut self, path: &Path, position: Vec3) {
        let relative = self.project_relative_path(path);
        let name = path
            .file_stem()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .unwrap_or("Mesh")
            .to_string();
        let id = self.scene.add_entity(name, position.x, position.y).id;
        if let Some(entity) = self.scene.entity_mut(id) {
            entity.position_z = position.z;
            let mut component = Component::core("MeshRenderer3D");
            if let Component::Core { props, .. } = &mut component
                && let Some(prop) = props.iter_mut().find(|prop| prop.name == "mesh_path")
            {
                prop.value = PropValue::Mesh(relative.clone());
            }
            entity.components.push(component);
        }
        self.select_only(id);
        self.mark_dirty();
        self.status = format!("Added mesh {relative}");
    }

    /// Return true while a project-bin mesh drag owns the 3D viewport pointer.
    fn handle_mesh_drop_3d(&mut self, ui: &mut Ui, area: Rect) -> bool {
        let Some(path) = self.mesh_drag.clone() else {
            return false;
        };
        let inside = area.contains(ui.input.mouse_x, ui.input.mouse_y);
        if ui.input.mouse_down {
            if inside {
                let label = path
                    .file_name()
                    .map(|value| value.to_string_lossy())
                    .unwrap_or_default();
                let width = (ui.painter.text_width(&label, 13.0) + 34.0).max(100.0);
                let bubble = Rect::new(
                    (ui.input.mouse_x + 8.0).min(area.right() - width - 4.0),
                    (ui.input.mouse_y + 8.0).min(area.bottom() - 26.0),
                    width,
                    22.0,
                );
                ui.painter.fill_round_rect(bubble, 4.0, [0, 0, 0, 210]);
                ui.painter.icon_centered(
                    bubble.x + 12.0,
                    bubble.y + 11.0,
                    icon::VIEW_IN_AR,
                    13.0,
                    self.config.theme.accent,
                );
                ui.painter.text_clipped(
                    bubble.x + 22.0,
                    bubble.y + 4.0,
                    &label,
                    13.0,
                    self.config.theme.text,
                    bubble.w - 26.0,
                );
            }
            ui.wants_redraw = true;
            return true;
        }
        if inside {
            let position = viewport_drop_position_3d(
                self.viewport_camera_3d,
                area,
                ui.input.mouse_x,
                ui.input.mouse_y,
            );
            self.add_mesh_entity_at(&path, position);
        }
        self.mesh_drag = None;
        true
    }

    fn handle_tilemap_paint(&mut self, ui: &mut Ui, area: Rect) -> bool {
        let Some((entity_id, component_index)) = self.tile_paint else {
            return false;
        };
        let Some(entity) = self.scene.entity(entity_id).cloned() else {
            self.tile_paint = None;
            return false;
        };
        let Some(Component::Core { name, props }) = entity.components.get(component_index) else {
            self.tile_paint = None;
            return false;
        };
        if name != "Tilemap2D" {
            self.tile_paint = None;
            return false;
        }
        let Some(rect) = self.entity_screen_rect(&entity, area) else {
            return false;
        };
        let columns = prop_number(props, &["map_width"])
            .unwrap_or(1.0)
            .round()
            .max(1.0) as usize;
        let rows = prop_number(props, &["map_height"])
            .unwrap_or(1.0)
            .round()
            .max(1.0) as usize;
        if columns == 0 || rows == 0 || rect.w <= 0.0 || rect.h <= 0.0 {
            return false;
        }

        let angle = self.entity_world_rotation(&entity);
        let prev = ui.painter.push_rotation(rect.x, rect.y, angle);
        let cell_w = rect.w / columns as f32;
        let cell_h = rect.h / rows as f32;
        for column in 0..=columns {
            let x = rect.x + column as f32 * cell_w;
            ui.painter
                .fill_rect(Rect::new(x, rect.y, 1.0, rect.h), [255, 255, 255, 36]);
        }
        for row in 0..=rows {
            let y = rect.y + row as f32 * cell_h;
            ui.painter
                .fill_rect(Rect::new(rect.x, y, rect.w, 1.0), [255, 255, 255, 36]);
        }
        ui.painter.set_rotation_raw(prev);

        let (mx, my) = (ui.input.mouse_x, ui.input.mouse_y);
        let (lx, ly) = rotate_point_about(mx, my, rect.x, rect.y, -angle);
        if !rect.contains(lx, ly) {
            return false;
        }
        let column =
            (((lx - rect.x) / cell_w).floor() as isize).clamp(0, columns as isize - 1) as usize;
        let row = (((ly - rect.y) / cell_h).floor() as isize).clamp(0, rows as isize - 1) as usize;
        let highlight = Rect::new(
            rect.x + column as f32 * cell_w,
            rect.y + row as f32 * cell_h,
            cell_w,
            cell_h,
        );
        let prev = ui.painter.push_rotation(rect.x, rect.y, angle);
        ui.painter
            .stroke_rect(highlight.shrink(1.0), self.config.theme.selection);
        ui.painter.set_rotation_raw(prev);

        if ui.input.mouse_down {
            let mut changed = false;
            if let Some(entity) = self.scene.entity_mut(entity_id) {
                if let Some(Component::Core { props, .. }) =
                    entity.components.get_mut(component_index)
                {
                    if let Some(prop) = props.iter_mut().find(|prop| prop.name == "tiles") {
                        let mut tiles = match &prop.value {
                            PropValue::Text(value) => parse_tile_ids(value, columns * rows),
                            _ => vec![-1; columns * rows],
                        };
                        let index = row * columns + column;
                        if let Some(tile) = tiles.get_mut(index) {
                            if *tile != self.tile_paint_tile {
                                *tile = self.tile_paint_tile;
                                prop.value = PropValue::Text(format_tile_ids(&tiles, columns));
                                changed = true;
                            }
                        }
                    }
                }
            }
            if changed {
                self.mark_dirty();
            }
            ui.wants_redraw = true;
        }
        true
    }

    /// While a `.neoprefab` is dragged from the bin, show a ghost in the
    /// viewport and instantiate it at the drop position on release.
    fn handle_prefab_drop(&mut self, ui: &mut Ui, area: Rect, z: f32) {
        let Some(path) = self.prefab_drag.clone() else {
            return;
        };
        let (mx, my) = (ui.input.mouse_x, ui.input.mouse_y);
        if ui.input.mouse_down {
            if area.contains(mx, my) {
                let name = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                ui.painter.fill_round_rect(
                    Rect::new(mx + 8.0, my + 8.0, 120.0, 20.0),
                    4.0,
                    [0, 0, 0, 200],
                );
                ui.painter.icon_centered(
                    mx + 20.0,
                    my + 18.0,
                    icon::VIEW_IN_AR,
                    13.0,
                    self.config.theme.accent,
                );
                ui.painter.text_clipped(
                    mx + 30.0,
                    my + 11.0,
                    &name,
                    13.0,
                    self.config.theme.text,
                    86.0,
                );
                ui.painter.stroke_rect(
                    Rect::new(mx - 6.0, my - 6.0, 12.0, 12.0),
                    self.config.theme.accent,
                );
            }
            ui.wants_redraw = true;
        } else {
            if area.contains(mx, my) {
                let wx = ((mx - (area.x + self.cam_x)) / z).round();
                let wy = ((my - (area.y + self.cam_y)) / z).round();
                self.instantiate_prefab(&path, wx, wy);
            }
            self.prefab_drag = None;
        }
    }

    fn start_viewport_drag(
        &mut self,
        primary: u64,
        rect: Rect,
        mx: f32,
        my: f32,
        axis: Option<(f32, f32)>,
    ) {
        let selected = self.selection_ids_ordered();
        let selected_set: HashSet<u64> = selected.iter().copied().collect();
        let mut start_world = Vec::new();
        for selected_id in selected {
            if self.locked_ids.contains(&selected_id) {
                continue;
            }
            if let Some(transform) = self.entity_world_transform(selected_id) {
                start_world.push((selected_id, transform.x, transform.y));
            }
        }
        if !start_world
            .iter()
            .any(|(selected_id, _, _)| *selected_id == primary)
        {
            return;
        }
        let mut descendant_start_world = Vec::new();
        for (id, _, _) in &start_world {
            for descendant in self.descendants_of(*id) {
                if selected_set.contains(&descendant) || self.locked_ids.contains(&descendant) {
                    continue;
                }
                if let Some(transform) = self.entity_world_transform(descendant) {
                    descendant_start_world.push((descendant, transform.x, transform.y));
                }
            }
        }
        descendant_start_world.sort_by_key(|(id, _, _)| *id);
        descendant_start_world.dedup_by_key(|(id, _, _)| *id);
        self.dragging = Some(ViewportDrag {
            primary,
            grab_x: mx - rect.x,
            grab_y: my - rect.y,
            start_world,
            descendant_start_world,
            axis,
        });
    }

    fn descendants_of(&self, id: u64) -> Vec<u64> {
        let mut out = Vec::new();
        let mut stack = self.scene.children_of(Some(id));
        while let Some(child) = stack.pop() {
            out.push(child);
            stack.extend(self.scene.children_of(Some(child)));
        }
        out
    }

    fn instantiate_prefab(&mut self, path: &Path, wx: f32, wy: f32) {
        let mut proto = match load_prefab_file(path) {
            Ok(p) => p,
            Err(e) => {
                self.status = format!("Prefab load failed: {e}");
                return;
            }
        };
        if proto.is_empty() {
            return;
        }
        // Offset only prefab roots so child positions remain parent-local.
        let (rx, ry) = proto
            .iter()
            .find(|e| e.parent.is_none())
            .map(|e| (e.x, e.y))
            .unwrap_or((proto[0].x, proto[0].y));
        let (dx, dy) = (wx - rx, wy - ry);
        for e in &mut proto {
            if e.parent.is_none() {
                e.x += dx;
                e.y += dy;
            }
        }
        let source = self.prefab_source_key(path);
        if let Some(root) = self.scene.instantiate_linked(proto, source) {
            self.select_only(root);
            self.mark_dirty();
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            self.status = format!("Placed prefab {name}");
        }
    }

    fn handle_viewport_input(&mut self, ui: &mut Ui, area: Rect) {
        let mx = ui.input.mouse_x;
        let my = ui.input.mouse_y;
        let z = self.cam_zoom;
        let inside = area.contains(mx, my);

        if inside && ui.input.right_pressed {
            // Right-click selects the entity under the cursor (if any), then
            // opens the appropriate context menu.
            let hit = self.viewport_hit(area, mx, my);
            match hit {
                Some(id) => {
                    if !self.is_selected(id) {
                        self.select_only(id);
                    }
                    self.open_entity_menu(id, mx, my);
                }
                None => {
                    let wx = ((mx - (area.x + self.cam_x)) / z).round();
                    let wy = ((my - (area.y + self.cam_y)) / z).round();
                    self.open_viewport_menu(mx, my, wx, wy);
                }
            }
            return;
        }

        if let Some(select) = self.box_select {
            let marquee = rect_from_points(select.start_x, select.start_y, mx, my);
            if ui.input.mouse_down {
                if marquee.w > 2.0 || marquee.h > 2.0 {
                    ui.painter.fill_rect(marquee, [255, 199, 89, 36]);
                    ui.painter.stroke_rect(marquee, self.config.theme.selection);
                }
                ui.wants_redraw = true;
                return;
            }

            self.box_select = None;
            if marquee.w < 3.0 && marquee.h < 3.0 {
                if !select.additive {
                    self.clear_selection();
                }
                return;
            }

            let mut ids = Vec::new();
            for entity in &self.scene.entities {
                if self.hidden_ids.contains(&entity.id) || self.locked_ids.contains(&entity.id) {
                    continue;
                }
                let Some(rect) = self.entity_screen_rect(entity, area) else {
                    continue;
                };
                let bounds =
                    rotated_rect_bounds(rect, rect.x, rect.y, self.entity_world_rotation(entity));
                if rects_intersect(marquee, bounds) {
                    ids.push(entity.id);
                }
            }
            self.select_many(ids, select.additive);
            ui.wants_redraw = true;
            return;
        }

        // Rotation gizmo drag takes priority over resize/move.
        if let Some(rot) = self.rotating {
            if ui.input.mouse_down {
                if let Some(e) = self.scene.entity(rot.id) {
                    if let Some(rect) = self.entity_screen_rect(e, area) {
                        // Pivot about the entity's fixed world centre so it spins
                        // in place. The knob sits straight up from the centre, so
                        // at world rotation 0 its direction is -90°.
                        let pivot_x = area.x + self.cam_x + rot.center_x * z;
                        let pivot_y = area.y + self.cam_y + rot.center_y * z;
                        let base_angle = -std::f32::consts::FRAC_PI_2;
                        let mouse_angle = (my - pivot_y).atan2(mx - pivot_x);
                        let mut world = mouse_angle - base_angle;
                        if self.config.layout.snap {
                            let step = std::f32::consts::FRAC_PI_2 / 6.0; // 15°
                            world = (world / step).round() * step;
                        }
                        let parent_rot = self.entity_world_rotation(e) - e.rotation;
                        let local = world - parent_rot;
                        // Keep the world centre fixed: solve the world top-left
                        // (the entity origin) for the new rotation, then convert
                        // back to a local position.
                        let (hx, hy) = (rect.w / (2.0 * z), rect.h / (2.0 * z));
                        let (sin, cos) = (world.sin(), world.cos());
                        let new_wx = rot.center_x - (hx * cos - hy * sin);
                        let new_wy = rot.center_y - (hx * sin + hy * cos);
                        let (local_x, local_y) = self
                            .world_origin_to_local_position(rot.id, new_wx, new_wy)
                            .unwrap_or((new_wx, new_wy));
                        if let Some(em) = self.scene.entity_mut(rot.id) {
                            if (em.rotation - local).abs() > 1e-5
                                || (em.x - local_x).abs() > 1e-4
                                || (em.y - local_y).abs() > 1e-4
                            {
                                em.rotation = local;
                                em.x = local_x;
                                em.y = local_y;
                                self.scene_dirty = true;
                            }
                        }
                        ui.wants_redraw = true;
                    }
                }
                return;
            }
            self.rotating = None;
        }

        // Start a move drag when the move gizmo is pressed: an axis arm
        // constrains to that axis, the centre square moves freely in 2D.
        if self.rotating.is_none()
            && self.resizing.is_none()
            && self.dragging.is_none()
            && inside
            && ui.input.mouse_pressed
            && self.config.layout.view_tool == ViewTool::Move
        {
            if let Some(id) = self.selected {
                if let Some(e) = self.scene.entity(id) {
                    if let Some(rect) = self.entity_screen_rect(e, area) {
                        let angle = self.entity_world_rotation(e);
                        if let Some(axis) = self.move_axis_hit(rect, angle, mx, my) {
                            self.start_viewport_drag(id, rect, mx, my, Some(axis));
                            return;
                        }
                        if self.move_handle_rect(rect).contains(mx, my) {
                            self.start_viewport_drag(id, rect, mx, my, None);
                            return;
                        }
                    }
                }
            }
        }

        // Start a rotation drag when the rotate tool's gizmo knob is pressed.
        if self.rotating.is_none()
            && self.resizing.is_none()
            && self.dragging.is_none()
            && inside
            && ui.input.mouse_pressed
            && matches!(
                self.config.layout.view_tool,
                ViewTool::Rotate | ViewTool::Transform
            )
        {
            if let Some(id) = self.selected {
                if let Some(e) = self.scene.entity(id) {
                    if let Some(rect) = self.entity_screen_rect(e, area) {
                        let angle = self.entity_world_rotation(e);
                        let (kx, ky) = self.rotate_handle_knob(rect, angle);
                        if (mx - kx).abs() <= 8.0 && (my - ky).abs() <= 8.0 {
                            // Capture the world centre as the fixed pivot.
                            let (csx, csy) = rotate_point_about(
                                rect.x + rect.w / 2.0,
                                rect.y + rect.h / 2.0,
                                rect.x,
                                rect.y,
                                angle,
                            );
                            self.rotating = Some(RotateDrag {
                                id,
                                center_x: (csx - (area.x + self.cam_x)) / z,
                                center_y: (csy - (area.y + self.cam_y)) / z,
                            });
                            return;
                        }
                    }
                }
            }
        }

        // Resize handles: pressing one of the selected entity's four corners
        // starts a resize anchored at the opposite corner.
        if self.resizing.is_none()
            && self.dragging.is_none()
            && inside
            && ui.input.mouse_pressed
            && matches!(
                self.config.layout.view_tool,
                ViewTool::Scale | ViewTool::Transform
            )
        {
            if let Some(id) = self.selected {
                if let Some(e) = self.scene.entity(id) {
                    let Some(rect) = self.entity_screen_rect(e, area) else {
                        return;
                    };
                    let angle = self.entity_world_rotation(e);
                    // Local fractions of (grabbed corner, opposite anchor corner).
                    let corners = [
                        ((0.0, 0.0), (1.0, 1.0)),
                        ((1.0, 0.0), (0.0, 1.0)),
                        ((0.0, 1.0), (1.0, 0.0)),
                        ((1.0, 1.0), (0.0, 0.0)),
                    ];
                    for ((gfx, gfy), (afx, afy)) in corners {
                        let (csx, csy) = rotate_point_about(
                            rect.x + gfx * rect.w,
                            rect.y + gfy * rect.h,
                            rect.x,
                            rect.y,
                            angle,
                        );
                        if (mx - csx).abs() <= 7.0 && (my - csy).abs() <= 7.0 {
                            // Anchor (opposite corner) stored in scene/world units.
                            let (asx, asy) = rotate_point_about(
                                rect.x + afx * rect.w,
                                rect.y + afy * rect.h,
                                rect.x,
                                rect.y,
                                angle,
                            );
                            let anchor_x = (asx - (area.x + self.cam_x)) / z;
                            let anchor_y = (asy - (area.y + self.cam_y)) / z;
                            self.resizing = Some((id, anchor_x, anchor_y, gfx, gfy));
                            break;
                        }
                    }
                }
            }
        }

        if let Some((id, ax, ay, gfx, gfy)) = self.resizing {
            if ui.input.mouse_down {
                let snap = self.config.layout.snap;
                let grid = self.config.layout.grid.max(1.0);
                let mut wx = (mx - (area.x + self.cam_x)) / z;
                let mut wy = (my - (area.y + self.cam_y)) / z;
                if snap {
                    wx = (wx / grid).round() * grid;
                    wy = (wy / grid).round() * grid;
                } else {
                    wx = wx.round();
                    wy = wy.round();
                }
                let fallback_scale = self
                    .scene
                    .entity(id)
                    .map(editor_entity_scale)
                    .unwrap_or(1.0);
                let world_scale = self
                    .entity_world_transform(id)
                    .map(|transform| transform.scale)
                    .unwrap_or(fallback_scale);
                let scale = if world_scale.abs() < f32::EPSILON {
                    1.0
                } else {
                    world_scale
                };
                let angle = self
                    .entity_world_transform(id)
                    .map(|transform| transform.rotation)
                    .unwrap_or(0.0);
                // Measure along the entity's local axes. This works for both
                // axis-aligned and rotated entities and lets aspect locking use
                // the same path in either case.
                let (sin, cos) = (angle.sin(), angle.cos());
                let (ux, uy) = (cos, sin); // local +x
                let (vx, vy) = (-sin, cos); // local +y
                let dx = wx - ax;
                let dy = wy - ay;
                let sgn_u = 2.0 * gfx - 1.0;
                let sgn_v = 2.0 * gfy - 1.0;
                let mut nw = (((dx * ux + dy * uy) * sgn_u) / scale).max(1.0);
                let mut nh = (((dx * vx + dy * vy) * sgn_v) / scale).max(1.0);

                if ui.input.ctrl {
                    let (current_w, current_h) = self
                        .scene
                        .entity(id)
                        .map(|entity| {
                            editor_entity_size(&self.scene, entity, self.preview_root_size())
                        })
                        .unwrap_or((nw, nh));
                    if current_w > f32::EPSILON && current_h > f32::EPSILON {
                        let aspect = current_w / current_h;
                        let width_change = (nw / current_w - 1.0).abs();
                        let height_change = (nh / current_h - 1.0).abs();
                        if width_change >= height_change {
                            nh = (nw / aspect).max(1.0);
                        } else {
                            nw = (nh * aspect).max(1.0);
                        }
                    }
                }

                // Rebuild the origin from the fixed opposite corner after any
                // aspect correction so that corner never drifts.
                let w_world = nw * scale;
                let h_world = nh * scale;
                let afx = 1.0 - gfx;
                let afy = 1.0 - gfy;
                let nx = ax - (afx * w_world * ux + afy * h_world * vx);
                let ny = ay - (afx * w_world * uy + afy * h_world * vy);
                let (local_x, local_y) = self
                    .world_origin_to_local_position(id, nx, ny)
                    .unwrap_or((nx, ny));
                let scaler_update = self.scene.entity(id).and_then(|entity| {
                    let scaler = entity_scaler_editor_state(entity)?;
                    let (current_w, current_h) =
                        editor_entity_size(&self.scene, entity, self.preview_root_size());
                    let local = editor_entity_local_transform(entity);
                    // world_origin_to_local_position uses the current size for
                    // pivot compensation. Adjust it for the size being applied
                    // so the opposite resize corner remains fixed.
                    let target_offset_x = local_x + (nw - current_w) * local.scale * local.pivot_x;
                    let target_offset_y = local_y + (nh - current_h) * local.scale * local.pivot_y;
                    let (parent_w, parent_h) =
                        editor_parent_size(&self.scene, entity, self.preview_root_size());
                    if scaler.edit_with_percent {
                        let x_percent = if parent_w.abs() < f32::EPSILON {
                            scaler.x_percent
                        } else {
                            (scaler.x_percent + (target_offset_x - scaler.offset_x) / parent_w)
                                .clamp(0.0, 1.0)
                        };
                        let y_percent = if parent_h.abs() < f32::EPSILON {
                            scaler.y_percent
                        } else {
                            (scaler.y_percent + (target_offset_y - scaler.offset_y) / parent_h)
                                .clamp(0.0, 1.0)
                        };
                        let size_x_percent = if parent_w.abs() < f32::EPSILON {
                            scaler.size_x_percent
                        } else {
                            (nw / parent_w).clamp(0.0, 1.0)
                        };
                        let size_y_percent = if parent_h.abs() < f32::EPSILON {
                            scaler.size_y_percent
                        } else {
                            (nh / parent_h).clamp(0.0, 1.0)
                        };
                        Some((true, x_percent, y_percent, size_x_percent, size_y_percent))
                    } else {
                        Some((false, target_offset_x, target_offset_y, 0.0, 0.0))
                    }
                });
                if let Some(e) = self.scene.entity_mut(id) {
                    let changed = match scaler_update {
                        Some((true, x, y, size_x, size_y)) => set_entity_scaler_numbers(
                            e,
                            &[
                                ("x_percent", "X %", x),
                                ("y_percent", "Y %", y),
                                ("size_x_percent", "Size X %", size_x),
                                ("size_y_percent", "Size Y %", size_y),
                            ],
                        ),
                        Some((false, x, y, _, _)) => {
                            let mut changed = set_entity_scaler_numbers(
                                e,
                                &[
                                    ("offset_x", "Offset X", x),
                                    ("offset_y", "Offset Y", y),
                                    ("size_x_percent", "Size X %", 0.0),
                                    ("size_y_percent", "Size Y %", 0.0),
                                ],
                            );
                            if e.size_x != nw || e.size_y != nh {
                                e.size_x = nw;
                                e.size_y = nh;
                                changed = true;
                            }
                            changed
                        }
                        None if e.x != local_x
                            || e.y != local_y
                            || e.size_x != nw
                            || e.size_y != nh =>
                        {
                            e.x = local_x;
                            e.y = local_y;
                            e.size_x = nw;
                            e.size_y = nh;
                            true
                        }
                        None => false,
                    };
                    if changed {
                        self.scene_dirty = true;
                    }
                }
                ui.wants_redraw = true;
                return;
            }
            self.resizing = None;
        }

        if inside && ui.input.mouse_pressed {
            match self.viewport_hit(area, mx, my) {
                Some(id) => {
                    self.select_with_modifiers(id, ui);
                    let e = self.scene.entity(id);
                    if self.is_selected(id) {
                        if let Some(e) = e {
                            if let Some(rect) = self.entity_screen_rect(e, area) {
                                if matches!(
                                    self.config.layout.view_tool,
                                    ViewTool::Move | ViewTool::Transform
                                ) {
                                    self.start_viewport_drag(id, rect, mx, my, None);
                                }
                            }
                        }
                    }
                }
                None => {
                    self.box_select = Some(BoxSelect {
                        start_x: mx,
                        start_y: my,
                        additive: ui.input.ctrl || ui.input.shift,
                    });
                }
            }
        }

        if let Some(drag) = self.dragging.clone() {
            if ui.input.mouse_down {
                let snap = self.config.layout.snap;
                let grid = self.config.layout.grid.max(1.0);
                let world_x = (mx - drag.grab_x - (area.x + self.cam_x)) / z;
                let world_y = (my - drag.grab_y - (area.y + self.cam_y)) / z;
                let Some((_, primary_start_x, primary_start_y)) = drag
                    .start_world
                    .iter()
                    .find(|(id, _, _)| *id == drag.primary)
                    .copied()
                else {
                    self.dragging = None;
                    return;
                };
                let (dx, dy) = if let Some((ux, uy)) = drag.axis {
                    // Constrained to one axis: project the raw movement onto the
                    // axis and snap the distance along it.
                    let mut t = (world_x - primary_start_x) * ux + (world_y - primary_start_y) * uy;
                    if snap {
                        t = (t / grid).round() * grid;
                    } else {
                        t = t.round();
                    }
                    (t * ux, t * uy)
                } else {
                    let (mut nx, mut ny) = (world_x, world_y);
                    if snap {
                        nx = (nx / grid).round() * grid;
                        ny = (ny / grid).round() * grid;
                    } else {
                        nx = nx.round();
                        ny = ny.round();
                    }
                    (nx - primary_start_x, ny - primary_start_y)
                };
                let mut updates = Vec::new();
                for (id, start_x, start_y) in &drag.start_world {
                    let target_x = start_x + dx;
                    let target_y = start_y + dy;
                    let (local_x, local_y) = self
                        .world_origin_to_local_position(*id, target_x, target_y)
                        .unwrap_or((target_x, target_y));
                    let scaler_update = self.scene.entity(*id).and_then(|entity| {
                        let scaler = entity_scaler_editor_state(entity)?;
                        if scaler.edit_with_percent {
                            let (parent_w, parent_h) =
                                editor_parent_size(&self.scene, entity, self.preview_root_size());
                            let x = if parent_w.abs() < f32::EPSILON {
                                scaler.x_percent
                            } else {
                                (scaler.x_percent + (local_x - scaler.offset_x) / parent_w)
                                    .clamp(0.0, 1.0)
                            };
                            let y = if parent_h.abs() < f32::EPSILON {
                                scaler.y_percent
                            } else {
                                (scaler.y_percent + (local_y - scaler.offset_y) / parent_h)
                                    .clamp(0.0, 1.0)
                            };
                            Some((true, x, y))
                        } else {
                            Some((false, local_x, local_y))
                        }
                    });
                    updates.push((*id, local_x, local_y, scaler_update));
                }
                for (id, local_x, local_y, scaler_update) in updates {
                    if let Some(e) = self.scene.entity_mut(id) {
                        let changed = match scaler_update {
                            Some((true, x, y)) => set_entity_scaler_numbers(
                                e,
                                &[("x_percent", "X %", x), ("y_percent", "Y %", y)],
                            ),
                            Some((false, x, y)) => set_entity_scaler_numbers(
                                e,
                                &[("offset_x", "Offset X", x), ("offset_y", "Offset Y", y)],
                            ),
                            None if e.x != local_x || e.y != local_y => {
                                e.x = local_x;
                                e.y = local_y;
                                true
                            }
                            None => false,
                        };
                        if changed {
                            self.scene_dirty = true;
                        }
                    }
                }
                if ui.input.ctrl {
                    let descendant_updates: Vec<(u64, f32, f32)> = drag
                        .descendant_start_world
                        .iter()
                        .filter_map(|(id, world_x, world_y)| {
                            self.world_origin_to_local_position(*id, *world_x, *world_y)
                                .map(|(x, y)| (*id, x, y))
                        })
                        .collect();
                    for (id, x, y) in descendant_updates {
                        if let Some(e) = self.scene.entity_mut(id) {
                            if e.x != x || e.y != y {
                                e.x = x;
                                e.y = y;
                                self.scene_dirty = true;
                            }
                        }
                    }
                }
                ui.wants_redraw = true;
            } else {
                self.dragging = None;
            }
        }
    }

    fn viewport_hit(&self, area: Rect, mx: f32, my: f32) -> Option<u64> {
        let mut order: Vec<&Entity> = self.scene.entities.iter().collect();
        order.sort_by(|a, b| compare_editor_entity_order(a, b).reverse());
        for e in order {
            if self.hidden_ids.contains(&e.id) || self.locked_ids.contains(&e.id) {
                continue;
            }
            let Some(r) = self.entity_screen_rect(e, area) else {
                continue;
            };
            // Inverse-rotate the cursor into the entity's unrotated frame so the
            // hit test matches the rotated preview.
            let angle = self.entity_world_rotation(e);
            let (lx, ly) = rotate_point_about(mx, my, r.x, r.y, -angle);
            if r.contains(lx, ly) {
                return Some(e.id);
            }
        }
        None
    }

    // ---- Splitters ---------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn handle_splitters(
        &mut self,
        ui: &mut Ui,
        left_col: Rect,
        right_col: Rect,
        left_panels: &[Panel],
        right_panels: &[Panel],
        w: f32,
        bin_split_y: f32,
        body_total: f32,
    ) -> bool {
        let mx = ui.input.mouse_x;
        let my = ui.input.mouse_y;
        let theme = self.config.theme.clone();
        let left_edge = left_col.right();
        let right_edge = right_col.x;

        let near = |a: f32, b: f32| (a - b).abs() <= SPLIT_HALF + 1.0;
        let mut hot: Option<Splitter> = None;
        if self.config.layout.show_project
            && !self.maximize_view
            && near(my, bin_split_y)
            && my >= TOOLBAR_H
        {
            hot = Some(Splitter::BinHeight);
        } else if !left_panels.is_empty()
            && near(mx, left_edge)
            && my >= left_col.y
            && my <= left_col.bottom()
        {
            hot = Some(Splitter::LeftWidth);
        } else if !right_panels.is_empty()
            && near(mx, right_edge)
            && my >= right_col.y
            && my <= right_col.bottom()
        {
            hot = Some(Splitter::RightWidth);
        } else if left_panels.len() == 2 {
            let sy = left_col.y + left_col.h * self.config.layout.left_split.clamp(0.15, 0.85);
            if near(my, sy) && mx >= left_col.x && mx <= left_col.right() {
                hot = Some(Splitter::LeftSplit);
            }
        }
        if hot.is_none() && right_panels.len() == 2 {
            let sy = right_col.y + right_col.h * self.config.layout.right_split.clamp(0.15, 0.85);
            if near(my, sy) && mx >= right_col.x && mx <= right_col.right() {
                hot = Some(Splitter::RightSplit);
            }
        }

        if ui.input.mouse_pressed {
            if let Some(s) = hot {
                self.active_splitter = Some(s);
            }
        }
        if !ui.input.mouse_down && self.active_splitter.take().is_some() {
            self.dirty = true;
        }

        if let Some(splitter) = self.active_splitter {
            ui.wants_redraw = true;
            match splitter {
                Splitter::LeftWidth => {
                    self.config.layout.left_w = clamp_range(mx, MIN_PANEL_W, w * 0.6)
                }
                Splitter::RightWidth => {
                    self.config.layout.right_w = clamp_range(w - mx, MIN_PANEL_W, w * 0.6)
                }
                Splitter::LeftSplit => {
                    self.config.layout.left_split =
                        ((my - left_col.y) / left_col.h.max(1.0)).clamp(0.15, 0.85)
                }
                Splitter::RightSplit => {
                    self.config.layout.right_split =
                        ((my - right_col.y) / right_col.h.max(1.0)).clamp(0.15, 0.85)
                }
                Splitter::BinHeight => {
                    let from_bottom = (TOOLBAR_H + body_total) - my;
                    self.config.layout.bin_h =
                        from_bottom.clamp(0.0, (body_total - 120.0).max(0.0));
                }
            }
        }

        // Visuals.
        let active = self.active_splitter;
        let col_of = |which: Splitter| active == Some(which) || hot == Some(which);
        let line = |ui: &mut Ui, r: Rect, lit: bool| {
            ui.painter.fill_rect(
                r,
                if lit {
                    theme.splitter_hover
                } else {
                    theme.splitter
                },
            );
        };
        line(
            ui,
            Rect::new(0.0, bin_split_y - 1.0, w, 2.0),
            col_of(Splitter::BinHeight),
        );
        if !left_panels.is_empty() {
            line(
                ui,
                Rect::new(left_edge - 1.0, left_col.y, 2.0, left_col.h),
                col_of(Splitter::LeftWidth),
            );
        }
        if !right_panels.is_empty() {
            line(
                ui,
                Rect::new(right_edge - 1.0, right_col.y, 2.0, right_col.h),
                col_of(Splitter::RightWidth),
            );
        }

        self.active_splitter.is_some() || (hot.is_some() && ui.input.mouse_pressed)
    }

    // ---- Inspector ---------------------------------------------------------

    fn inspector_content(&mut self, ui: &mut Ui, area: Rect, start_y: f32) -> f32 {
        let x = area.x + PAD;
        let width = area.w - PAD * 2.0 - 6.0;
        let mut y = start_y;

        let inspected = match &self.inspector_reference_drag {
            Some(InspectorReferenceDrag::Entity {
                inspector_owner: Some(id),
                ..
            }) => Some(*id),
            _ => self.selected,
        };
        let Some(id) = inspected else {
            y = self.section_header(ui, x, width, y, icon::PALETTE, "Scene");
            let r = ui.text_field(
                "scene_name_insp",
                Rect::new(x, y, width, FIELD_H),
                &self.scene.name,
            );
            if r.changed && !r.text.is_empty() {
                self.scene.name = r.text;
                self.mark_dirty();
            }
            y += FIELD_H + 6.0;
            let mut bg = self.scene.background;
            y = self.color_row(
                ui,
                "scene_bg",
                "app.bg",
                &mut bg,
                ColorTarget::Background,
                x,
                width,
                y,
            );
            if bg != self.scene.background {
                self.scene.background = bg;
                self.mark_dirty();
            }
            // Upscaling filter: checked = bilinear (smooth), unchecked =
            // nearest-neighbour (crisp pixel-art, the default).
            self.inspector_label(ui, x, y + 4.0, "Bilinear upscaling", LABEL_W - 6.0);
            let bilinear = !self.scene.nearest_neighbor_scaling;
            if let Some(nv) = ui.checkbox(Rect::new(x + LABEL_W, y, FIELD_H, FIELD_H), bilinear) {
                self.scene.nearest_neighbor_scaling = !nv;
                self.mark_dirty();
            }
            y += FIELD_H + 6.0;
            self.inspector_label(ui, x, y + 4.0, "Anti-aliasing", LABEL_W - 6.0);
            let aa_button = Rect::new(x + LABEL_W, y, (width - LABEL_W).max(40.0), FIELD_H);
            let antialiasing = self.scene.antialiasing.clone();
            if ui.dropdown_button(aa_button, &antialiasing) {
                self.open_scene_antialiasing_menu(aa_button.x, aa_button.bottom() + 2.0);
            }
            ui.tooltip(
                aa_button,
                "Choose off, standard (2x), or high (4x / supersampled text)",
            );
            y += FIELD_H + 6.0;

            // 2D lighting: a per-scene toggle exported as `lighting.*` and
            // previewed live in the viewport.
            y = self.section_header(ui, x, width, y, icon::PALETTE, "Lighting");
            if !self.config.settings.preview_lighting {
                ui.painter.text_clipped(
                    x + 4.0,
                    y,
                    "Viewport preview is off in Editor Settings.",
                    12.0,
                    self.config.theme.text_dim,
                    (width - 8.0).max(0.0),
                );
                y += 20.0;
            }
            self.inspector_label(ui, x, y + 4.0, "Enabled", LABEL_W - 6.0);
            if let Some(nv) = ui.checkbox(
                Rect::new(x + LABEL_W, y, FIELD_H, FIELD_H),
                self.scene.lighting.enabled,
            ) {
                self.scene.lighting.enabled = nv;
                self.mark_dirty();
            }
            y += FIELD_H + 6.0;

            if self.scene.lighting.enabled {
                let mut amb = self.scene.lighting.ambient;
                y = self.color_row(
                    ui,
                    "scene_ambient",
                    "Ambient",
                    &mut amb,
                    ColorTarget::LightingAmbient,
                    x,
                    width,
                    y,
                );
                if amb != self.scene.lighting.ambient {
                    self.scene.lighting.ambient = amb;
                    self.mark_dirty();
                }

                let (ny, v) = self.lighting_num_row(
                    ui,
                    "li_amb_int",
                    "Ambient Int",
                    x,
                    width,
                    y,
                    self.scene.lighting.ambient_intensity,
                );
                y = ny;
                if let Some(v) = v {
                    self.scene.lighting.ambient_intensity = v.max(0.0);
                    self.mark_dirty();
                }

                // Ambient occlusion.
                self.inspector_label(ui, x, y + 4.0, "Occlusion", LABEL_W - 6.0);
                if let Some(nv) = ui.checkbox(
                    Rect::new(x + LABEL_W, y, FIELD_H, FIELD_H),
                    self.scene.lighting.ambient_occlusion,
                ) {
                    self.scene.lighting.ambient_occlusion = nv;
                    self.mark_dirty();
                }
                y += FIELD_H + 6.0;

                // Shadows + softness.
                self.inspector_label(ui, x, y + 4.0, "Shadows", LABEL_W - 6.0);
                if let Some(nv) = ui.checkbox(
                    Rect::new(x + LABEL_W, y, FIELD_H, FIELD_H),
                    self.scene.lighting.shadows,
                ) {
                    self.scene.lighting.shadows = nv;
                    self.mark_dirty();
                }
                y += FIELD_H + 6.0;

                let (ny, v) = self.lighting_num_row(
                    ui,
                    "li_soft",
                    "Softness",
                    x,
                    width,
                    y,
                    self.scene.lighting.soft_shadows,
                );
                y = ny;
                if let Some(v) = v {
                    self.scene.lighting.soft_shadows = v.max(0.0);
                    self.mark_dirty();
                }

                let (ny, v) = self.lighting_num_row(
                    ui,
                    "li_bloom",
                    "Bloom",
                    x,
                    width,
                    y,
                    self.scene.lighting.bloom,
                );
                y = ny;
                if let Some(v) = v {
                    self.scene.lighting.bloom = v.max(0.0);
                    self.mark_dirty();
                }

                let (ny, v) = self.lighting_num_row(
                    ui,
                    "li_exposure",
                    "Exposure",
                    x,
                    width,
                    y,
                    self.scene.lighting.exposure,
                );
                y = ny;
                if let Some(v) = v {
                    self.scene.lighting.exposure = v.max(0.0);
                    self.mark_dirty();
                }

                // Quality cycles low -> medium -> high -> ultra on click.
                self.inspector_label(ui, x, y + 4.0, "Quality", LABEL_W - 6.0);
                let qbtn = Rect::new(x + LABEL_W, y, (width - LABEL_W).max(40.0), FIELD_H);
                if ui.dropdown_button(qbtn, &self.scene.lighting.quality) {
                    let order = ["low", "medium", "high", "ultra"];
                    let current = order
                        .iter()
                        .position(|q| *q == self.scene.lighting.quality)
                        .unwrap_or(1);
                    self.scene.lighting.quality = order[(current + 1) % order.len()].to_string();
                    self.mark_dirty();
                }
                ui.tooltip(qbtn, "Light-map resolution: low, medium, high, or ultra");
                y += FIELD_H + 6.0;
            }

            y = self.post_process_inspector(ui, x, width, y);

            return y + 10.0;
        };
        let Some(mut entity) = self.scene.entity(id).cloned() else {
            self.clear_selection();
            return y + 10.0;
        };
        let mut dirty = false;

        // Active checkbox + name (like Unity's header row).
        if let Some(nv) = ui.checkbox(Rect::new(x, y, FIELD_H, FIELD_H), entity.enabled) {
            entity.enabled = nv;
            dirty = true;
        }
        let r = ui.text_field(
            "ent_name",
            Rect::new(x + FIELD_H + 6.0, y, width - FIELD_H - 6.0, FIELD_H),
            &entity.name,
        );
        if r.changed {
            entity.name = r.text;
            dirty = true;
        }
        y += FIELD_H + 8.0;

        y = self.section_header(ui, x, width, y, icon::VIEW_IN_AR, "Transform");
        if self.scene.kind == SceneKind::ThreeD {
            dirty |= self.num_row(
                ui,
                "ent_position_x",
                "Position X",
                &mut entity.x,
                x,
                width,
                &mut y,
            );
            dirty |= self.num_row(
                ui,
                "ent_position_y",
                "Position Y",
                &mut entity.y,
                x,
                width,
                &mut y,
            );
            dirty |= self.num_row(
                ui,
                "ent_position_z",
                "Position Z",
                &mut entity.position_z,
                x,
                width,
                &mut y,
            );
            dirty |= self.num_row(
                ui,
                "ent_rotation_x",
                "Euler X",
                &mut entity.rotation_x,
                x,
                width,
                &mut y,
            );
            dirty |= self.num_row(
                ui,
                "ent_rotation_y",
                "Euler Y",
                &mut entity.rotation_y,
                x,
                width,
                &mut y,
            );
            dirty |= self.num_row(
                ui,
                "ent_rotation_z",
                "Euler Z",
                &mut entity.rotation_z,
                x,
                width,
                &mut y,
            );
            dirty |= self.num_row(
                ui,
                "ent_scale_x",
                "Scale X",
                &mut entity.scale_x,
                x,
                width,
                &mut y,
            );
            dirty |= self.num_row(
                ui,
                "ent_scale_y",
                "Scale Y",
                &mut entity.scale_y,
                x,
                width,
                &mut y,
            );
            dirty |= self.num_row(
                ui,
                "ent_scale_z",
                "Scale Z",
                &mut entity.scale_z,
                x,
                width,
                &mut y,
            );
        } else {
            dirty |= self.num_row(ui, "ent_x", "X", &mut entity.x, x, width, &mut y);
            dirty |= self.num_row(ui, "ent_y", "Y", &mut entity.y, x, width, &mut y);
            dirty |= self.num_row(ui, "ent_z", "Z (order)", &mut entity.z, x, width, &mut y);
            dirty |= self.num_row(ui, "ent_w", "Width", &mut entity.size_x, x, width, &mut y);
            dirty |= self.num_row(ui, "ent_h", "Height", &mut entity.size_y, x, width, &mut y);
            dirty |= self.num_row(
                ui,
                "ent_rot",
                "Rotation",
                &mut entity.rotation,
                x,
                width,
                &mut y,
            );
            dirty |= self.num_row(
                ui,
                "ent_scale",
                "Scale",
                &mut entity.scale,
                x,
                width,
                &mut y,
            );

            // Advanced 2D transform.
            let adv_key = format!("adv_transform_{id}");
            let expanded = !self.collapsed.contains(&adv_key);
            let hdr = Rect::new(x + 10.0, y, width - 10.0, ROW_H - 2.0);
            let now = ui.collapsing_header(hdr, "Advanced", expanded);
            self.set_collapsed(&adv_key, !now);
            y += ROW_H + 2.0;
            if now {
                dirty |= self.num_row(
                    ui,
                    "ent_ax",
                    "Anchor X",
                    &mut entity.anchor_x,
                    x + 8.0,
                    width - 8.0,
                    &mut y,
                );
                dirty |= self.num_row(
                    ui,
                    "ent_ay",
                    "Anchor Y",
                    &mut entity.anchor_y,
                    x + 8.0,
                    width - 8.0,
                    &mut y,
                );
                dirty |= self.position_pivot_mode_row(
                    ui,
                    "Position Pivot",
                    &mut entity.position_pivot,
                    x + 8.0,
                    width - 8.0,
                    &mut y,
                );
                let (position_default_x, position_default_y) =
                    position_pivot_fraction_from_name(&entity.position_pivot);
                dirty |= self.optional_num_row(
                    ui,
                    "ent_pivot_x",
                    "Pivot X",
                    &mut entity.pivot_x,
                    position_default_x,
                    x + 8.0,
                    width - 8.0,
                    &mut y,
                );
                dirty |= self.optional_num_row(
                    ui,
                    "ent_pivot_y",
                    "Pivot Y",
                    &mut entity.pivot_y,
                    position_default_y,
                    x + 8.0,
                    width - 8.0,
                    &mut y,
                );
                dirty |= self.rotation_pivot_mode_row(
                    ui,
                    "Rotation Pivot",
                    &mut entity.rotation_pivot,
                    x + 8.0,
                    width - 8.0,
                    &mut y,
                );
                let (rotation_default_x, rotation_default_y) =
                    if entity.pivot_x.is_some() || entity.pivot_y.is_some() {
                        (entity.pivot_x.unwrap_or(0.0), entity.pivot_y.unwrap_or(0.0))
                    } else {
                        rotation_pivot_fraction_from_name(&entity.rotation_pivot)
                    };
                dirty |= self.optional_num_row(
                    ui,
                    "ent_rotation_pivot_x",
                    "Rot Pivot X",
                    &mut entity.rotation_pivot_x,
                    rotation_default_x,
                    x + 8.0,
                    width - 8.0,
                    &mut y,
                );
                dirty |= self.optional_num_row(
                    ui,
                    "ent_rotation_pivot_y",
                    "Rot Pivot Y",
                    &mut entity.rotation_pivot_y,
                    rotation_default_y,
                    x + 8.0,
                    width - 8.0,
                    &mut y,
                );
            }
        }
        y += 6.0;

        y = self.attached_values(ui, id, &mut entity.values, x, width, y, &mut dirty);

        y = self.section_header(ui, x, width, y, icon::VIEW_QUILT, "Components");
        let mut remove_component: Option<usize> = None;
        for index in 0..entity.components.len() {
            let comp_label = entity.components[index].label().to_string();
            let glyph = component_icon(&entity.components[index]);
            let key = format!("comp_{id}_{index}");
            let comp_expanded = !self.collapsed.contains(&key);
            // Header row with collapse + copy + remove.
            let tri = if comp_expanded {
                icon::EXPAND_MORE
            } else {
                icon::CHEVRON_RIGHT
            };
            ui.painter.fill_round_rect(
                Rect::new(x, y, width, ROW_H),
                3.0,
                self.config.theme.panel_alt,
            );
            ui.icon(x + 12.0, y + ROW_H / 2.0, tri, 15.0, self.config.theme.text);
            ui.icon(
                x + 28.0,
                y + ROW_H / 2.0,
                glyph,
                15.0,
                self.config.theme.accent,
            );
            ui.painter.text_clipped(
                x + 42.0,
                y + (ROW_H - 14.0) / 2.0,
                &comp_label,
                14.0,
                self.config.theme.text,
                (width - 90.0).max(1.0),
            );
            let collapse_hit = Rect::new(x, y, 22.0, ROW_H);
            if collapse_hit.contains(ui.input.mouse_x, ui.input.mouse_y) && ui.input.mouse_pressed {
                self.set_collapsed(&key, comp_expanded);
            }
            let drag_hit = Rect::new(x + 22.0, y, (width - 70.0).max(0.0), ROW_H);
            ui.tooltip(drag_hit, "Drag component reference");
            if drag_hit.contains(ui.input.mouse_x, ui.input.mouse_y) && ui.input.mouse_pressed {
                self.inspector_reference_drag =
                    Some(InspectorReferenceDrag::Component(ComponentReference {
                        entity: id,
                        component: index,
                    }));
            }
            let copy = Rect::new(x + width - 44.0, y + 2.0, 20.0, ROW_H - 4.0);
            ui.tooltip(copy, "Copy component");
            if ui.icon_toggle(copy, icon::CONTENT_COPY, false, self.config.theme.text_dim) {
                self.component_clipboard = Some(entity.components[index].clone());
                self.status = format!("Copied {comp_label} component");
            }
            let del = Rect::new(x + width - 22.0, y + 2.0, 20.0, ROW_H - 4.0);
            ui.tooltip(del, "Remove component");
            if ui.icon_toggle(del, icon::DELETE, false, self.config.theme.danger) {
                remove_component = Some(index);
            }
            y += ROW_H + 4.0;
            if comp_expanded {
                dirty |= self.component_body(
                    ui,
                    id,
                    index,
                    &mut entity.components[index],
                    x,
                    width,
                    &mut y,
                );
            }
            y += 6.0;
        }
        if let Some(index) = remove_component {
            entity.components.remove(index);
            dirty = true;
        }

        // Add Component (dropdown).
        let add = Rect::new(x, y, width, FIELD_H + 4.0);
        if ui.icon_button(add, icon::ADD, "Add Component") {
            self.open_add_component_menu(id, add.x, add.bottom());
        }
        y += FIELD_H + 12.0;

        if ui.icon_button(
            Rect::new(x, y, width, FIELD_H + 4.0),
            icon::DELETE,
            "Delete Entity",
        ) {
            self.scene.remove_entity(id);
            self.clear_selection();
            self.mark_dirty();
            return y + FIELD_H + 14.0;
        }
        y += FIELD_H + 14.0;

        if dirty {
            self.scene.replace_entity(id, entity);
            if let Some(index) = remove_component {
                self.scene.adjust_component_references(id, index);
            }
            self.mark_dirty();
        }
        y
    }

    fn component_body(
        &mut self,
        ui: &mut Ui,
        entity: u64,
        comp: usize,
        component: &mut Component,
        x: f32,
        width: f32,
        y: &mut f32,
    ) -> bool {
        let mut dirty = false;
        match component {
            Component::Core { name, props } => {
                // Basic props.
                let mut advanced_present = false;
                for pi in 0..props.len() {
                    if props[pi].advanced {
                        advanced_present = true;
                        continue;
                    }
                    dirty |= self.prop_row(ui, entity, comp, pi, &mut props[pi], x, width, y);
                }
                if advanced_present {
                    let key = format!("compadv_{entity}_{comp}");
                    let expanded = !self.collapsed.contains(&key);
                    let hdr = Rect::new(x + 10.0, *y, width - 10.0, ROW_H - 2.0);
                    let now = ui.collapsing_header(hdr, "Advanced", expanded);
                    self.set_collapsed(&key, !now);
                    *y += ROW_H + 2.0;
                    if now {
                        for pi in 0..props.len() {
                            if props[pi].advanced {
                                dirty |= self.prop_row(
                                    ui,
                                    entity,
                                    comp,
                                    pi,
                                    &mut props[pi],
                                    x + 8.0,
                                    width - 8.0,
                                    y,
                                );
                            }
                        }
                    }
                }
                if name == "Tilemap2D" {
                    *y += 4.0;
                    ui.painter
                        .fill_rect(Rect::new(x, *y, width, 1.0), self.config.theme.border);
                    *y += 6.0;
                    self.inspector_label(ui, x, *y + 4.0, "Paint Tile", LABEL_W - 6.0);
                    let tile_field = ui.text_field(
                        &format!("tile_paint_{entity}_{comp}"),
                        Rect::new(x + LABEL_W, *y, (width - LABEL_W - 84.0).max(42.0), FIELD_H),
                        &self.tile_paint_tile.to_string(),
                    );
                    if tile_field.changed {
                        if let Ok(value) = tile_field.text.trim().parse::<i32>() {
                            self.tile_paint_tile = value.max(-1);
                        }
                    }
                    let paint_active = self.tile_paint == Some((entity, comp));
                    let paint_button = Rect::new(x + width - 76.0, *y, 76.0, FIELD_H);
                    if ui.button(
                        paint_button,
                        if paint_active { "Painting" } else { "Paint" },
                    ) {
                        self.tile_paint = if paint_active {
                            None
                        } else {
                            Some((entity, comp))
                        };
                    }
                    ui.tooltip(
                        paint_button,
                        "Paint tiles in the Scene view; use tile -1 to erase",
                    );
                    *y += FIELD_H + 6.0;
                }
            }
            Component::Script { path, variables } => {
                dirty |= self.text_row(ui, &format!("spath_{comp}"), "Script", path, x, width, y);
                dirty |= self.sync_script_variables(path, variables);
                *y = self.script_variables(ui, entity, comp, variables, x, width, *y, &mut dirty);
            }
        }
        dirty
    }

    fn prop_row(
        &mut self,
        ui: &mut Ui,
        entity: u64,
        comp: usize,
        pi: usize,
        prop: &mut Prop,
        x: f32,
        width: f32,
        y: &mut f32,
    ) -> bool {
        let id = format!("p_{entity}_{comp}_{pi}");
        let mut dirty = false;
        let fx = x + LABEL_W;
        let fw = (width - LABEL_W).max(30.0);
        // Collection properties own the whole row and provide their own
        // disclosure header. Scalar properties retain the compact label/field
        // layout used throughout the inspector.
        if !matches!(&prop.value, PropValue::StringList(_)) {
            self.inspector_label(ui, x, *y + 4.0, &prop.label, LABEL_W - 6.0);
        }
        match &mut prop.value {
            PropValue::Number(n) => {
                let r = ui.text_field(&id, Rect::new(fx, *y, fw, FIELD_H), &format_num(*n));
                if r.changed {
                    if let Ok(v) = r.text.trim().parse::<f32>() {
                        *n = v;
                        dirty = true;
                    } else if r.text.trim().is_empty() {
                        *n = 0.0;
                        dirty = true;
                    }
                }
                *y += FIELD_H + 6.0;
            }
            PropValue::Int(iv) => {
                let r = ui.text_field(&id, Rect::new(fx, *y, fw, FIELD_H), &iv.to_string());
                if r.changed {
                    if let Ok(v) = r.text.trim().parse::<i32>() {
                        *iv = v;
                        dirty = true;
                    }
                }
                *y += FIELD_H + 6.0;
            }
            PropValue::Bool(b) => {
                if let Some(nv) = ui.checkbox(Rect::new(fx, *y, FIELD_H, FIELD_H), *b) {
                    *b = nv;
                    dirty = true;
                }
                *y += FIELD_H + 6.0;
            }
            PropValue::Text(s) => {
                let r = ui.text_field(&id, Rect::new(fx, *y, fw, FIELD_H), s);
                if r.changed {
                    *s = r.text;
                    dirty = true;
                }
                *y += FIELD_H + 6.0;
            }
            PropValue::Enum { value, options } => {
                let btn = Rect::new(fx, *y, fw, FIELD_H);
                let current = value.clone();
                if ui.dropdown_button(btn, &current) {
                    self.open_prop_enum_menu(
                        btn.x,
                        btn.bottom() + 2.0,
                        EnumPropMenuTarget {
                            entity,
                            component: comp,
                            prop: pi,
                            options: options.clone(),
                            current,
                        },
                    );
                }
                *y += FIELD_H + 6.0;
            }
            PropValue::StringList(options) => {
                let collapse_key = format!("{id}_string_list");
                let expanded = !self.collapsed.contains(&collapse_key);
                let header = Rect::new(x, *y, width, ROW_H);
                let next = ui.collapsing_header(
                    header,
                    &format!(
                        "{}  [{} option{}]",
                        prop.label,
                        options.len(),
                        if options.len() == 1 { "" } else { "s" }
                    ),
                    expanded,
                );
                self.set_collapsed(&collapse_key, !next);
                *y += ROW_H + 3.0;

                if next {
                    if options.is_empty() {
                        ui.painter.text_clipped(
                            x + 12.0,
                            *y + 3.0,
                            "No options yet.",
                            13.0,
                            self.config.theme.text_dim,
                            (width - 24.0).max(0.0),
                        );
                        *y += 22.0;
                    }

                    let mut remove = None;
                    let mut move_option = None;
                    for index in 0..options.len() {
                        let row_x = x + 10.0;
                        let row_right = x + width;
                        let index_w = 24.0;
                        let controls_w = 68.0;
                        ui.painter.text_clipped(
                            row_x,
                            *y + 4.0,
                            &(index + 1).to_string(),
                            12.0,
                            self.config.theme.text_dim,
                            index_w - 4.0,
                        );
                        let field_x = row_x + index_w;
                        let field_w = (row_right - controls_w - field_x).max(30.0);
                        let response = ui.text_field(
                            &format!("{id}_option_{index}"),
                            Rect::new(field_x, *y, field_w, FIELD_H),
                            &options[index],
                        );
                        if response.changed {
                            options[index] = response.text;
                            dirty = true;
                        }

                        let up = Rect::new(row_right - 66.0, *y, 20.0, FIELD_H);
                        let down = Rect::new(row_right - 44.0, *y, 20.0, FIELD_H);
                        let delete = Rect::new(row_right - 22.0, *y, 20.0, FIELD_H);
                        if index > 0 {
                            if ui.icon_toggle(
                                up,
                                icon::ARROW_UPWARD,
                                false,
                                self.config.theme.text_dim,
                            ) {
                                move_option = Some((index, index - 1));
                            }
                            ui.tooltip(up, "Move option up");
                        } else {
                            ui.icon(
                                up.x + up.w * 0.5,
                                up.y + up.h * 0.5,
                                icon::ARROW_UPWARD,
                                15.0,
                                [
                                    self.config.theme.text_dim[0],
                                    self.config.theme.text_dim[1],
                                    self.config.theme.text_dim[2],
                                    80,
                                ],
                            );
                        }
                        if index + 1 < options.len() {
                            if ui.icon_toggle(
                                down,
                                icon::ARROW_DOWNWARD,
                                false,
                                self.config.theme.text_dim,
                            ) {
                                move_option = Some((index, index + 1));
                            }
                            ui.tooltip(down, "Move option down");
                        } else {
                            ui.icon(
                                down.x + down.w * 0.5,
                                down.y + down.h * 0.5,
                                icon::ARROW_DOWNWARD,
                                15.0,
                                [
                                    self.config.theme.text_dim[0],
                                    self.config.theme.text_dim[1],
                                    self.config.theme.text_dim[2],
                                    80,
                                ],
                            );
                        }
                        if ui.icon_toggle(delete, icon::DELETE, false, self.config.theme.danger) {
                            remove = Some(index);
                        }
                        ui.tooltip(delete, "Delete option");
                        *y += FIELD_H + 3.0;
                    }

                    if let Some(index) = remove {
                        options.remove(index);
                        ui.clear_focus();
                        dirty = true;
                    } else if let Some((from, to)) = move_option {
                        options.swap(from, to);
                        ui.clear_focus();
                        dirty = true;
                    }

                    let add = Rect::new(x + 10.0, *y, (width - 10.0).max(30.0), FIELD_H);
                    if ui.icon_button(add, icon::ADD_CIRCLE, "Add Option") {
                        let next_index = options.len();
                        options.push(String::new());
                        ui.focus_text(&format!("{id}_option_{next_index}"), "");
                        dirty = true;
                    }
                    ui.tooltip(add, "Append a new dropdown option");
                    *y += FIELD_H + 6.0;
                }
            }
            PropValue::Color(c) => {
                let mut col = *c;
                dirty |= self.color_row_inline(
                    ui,
                    &id,
                    fx,
                    fw,
                    *y,
                    &mut col,
                    ColorTarget::Prop {
                        entity,
                        comp,
                        prop: pi,
                    },
                );
                *c = col;
                *y += FIELD_H + 6.0;
            }
            PropValue::Image(s) => {
                dirty |= self.asset_path_row(
                    ui,
                    &id,
                    s,
                    AssetKind::Image,
                    AssetTarget::Prop {
                        entity,
                        component: comp,
                        prop: pi,
                    },
                    fx,
                    fw,
                    *y,
                );
                *y += FIELD_H + 6.0;
            }
            PropValue::Font(s) => {
                dirty |= self.asset_path_row(
                    ui,
                    &id,
                    s,
                    AssetKind::Font,
                    AssetTarget::Prop {
                        entity,
                        component: comp,
                        prop: pi,
                    },
                    fx,
                    fw,
                    *y,
                );
                *y += FIELD_H + 6.0;
            }
            PropValue::Sound(s) => {
                dirty |= self.asset_path_row(
                    ui,
                    &id,
                    s,
                    AssetKind::Sound,
                    AssetTarget::Prop {
                        entity,
                        component: comp,
                        prop: pi,
                    },
                    fx,
                    fw,
                    *y,
                );
                *y += FIELD_H + 6.0;
            }
            PropValue::Mesh(s) => {
                dirty |= self.asset_path_row(
                    ui,
                    &id,
                    s,
                    AssetKind::Mesh,
                    AssetTarget::Prop {
                        entity,
                        component: comp,
                        prop: pi,
                    },
                    fx,
                    fw,
                    *y,
                );
                *y += FIELD_H + 6.0;
            }
            PropValue::Shader(s) => {
                dirty |= self.asset_path_row(
                    ui,
                    &id,
                    s,
                    AssetKind::Shader,
                    AssetTarget::Prop {
                        entity,
                        component: comp,
                        prop: pi,
                    },
                    fx,
                    fw,
                    *y,
                );
                *y += FIELD_H + 6.0;
            }
            PropValue::Animation(s) => {
                dirty |= self.asset_path_row(
                    ui,
                    &id,
                    s,
                    AssetKind::Animation,
                    AssetTarget::Prop {
                        entity,
                        component: comp,
                        prop: pi,
                    },
                    fx,
                    fw,
                    *y,
                );
                *y += FIELD_H + 6.0;
            }
            PropValue::ColorSequence(keypoints) => {
                self.sequence_row(
                    ui,
                    AssetTarget::Prop {
                        entity,
                        component: comp,
                        prop: pi,
                    },
                    SequenceKind::Color,
                    SequenceValue::Colors(keypoints.clone()),
                    fx,
                    fw,
                    *y,
                );
                *y += FIELD_H + 6.0;
            }
            PropValue::NumberSequence(keypoints) => {
                self.sequence_row(
                    ui,
                    AssetTarget::Prop {
                        entity,
                        component: comp,
                        prop: pi,
                    },
                    SequenceKind::Transparency,
                    SequenceValue::Numbers(keypoints.clone()),
                    fx,
                    fw,
                    *y,
                );
                *y += FIELD_H + 6.0;
            }
        }
        dirty
    }

    #[allow(clippy::too_many_arguments)]
    fn sequence_row(
        &mut self,
        ui: &mut Ui,
        target: AssetTarget,
        kind: SequenceKind,
        value: SequenceValue,
        x: f32,
        width: f32,
        y: f32,
    ) {
        let rect = Rect::new(x, y, width, FIELD_H);
        draw_sequence_strip(&mut ui.painter, rect, &value, self.config.theme.field);
        let hovered = rect.contains(ui.input.mouse_x, ui.input.mouse_y);
        ui.painter.stroke_round_rect(
            rect,
            3.0,
            if hovered {
                self.config.theme.accent
            } else {
                self.config.theme.border
            },
        );
        if hovered && ui.input.mouse_pressed {
            self.focus = Some("sequence_time".to_string());
            self.edit_buffer = "0".to_string();
            self.edit_cursor = 1;
            self.edit_selection_anchor = None;
            self.popup = Some(Popup::Sequence {
                target,
                kind,
                value,
                selected: 0,
                dragging: None,
                color_picker: None,
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn asset_path_row(
        &mut self,
        ui: &mut Ui,
        id: &str,
        value: &mut String,
        kind: AssetKind,
        target: AssetTarget,
        x: f32,
        width: f32,
        y: f32,
    ) -> bool {
        let picker_width = 24.0;
        let field_width = (width - picker_width - 4.0).max(24.0);
        let result = ui.text_field(id, Rect::new(x, y, field_width, FIELD_H), value);
        let button = Rect::new(x + field_width + 4.0, y, picker_width, FIELD_H);
        if ui.icon_toggle(button, kind.glyph(), false, self.config.theme.text_dim) {
            let files = self.asset_paths(kind);
            self.focus = Some("asset_picker_search".to_string());
            self.edit_buffer.clear();
            self.edit_cursor = 0;
            self.edit_selection_anchor = None;
            self.popup = Some(Popup::Asset {
                target,
                kind,
                files,
                query: String::new(),
                scroll: 0.0,
            });
        }
        if result.changed {
            *value = result.text;
        }
        result.changed
    }

    fn script_variables(
        &mut self,
        ui: &mut Ui,
        entity: u64,
        comp: usize,
        variables: &mut [ScriptVar],
        x: f32,
        width: f32,
        mut y: f32,
        dirty: &mut bool,
    ) -> f32 {
        ui.icon(
            x + 8.0,
            y + 8.0,
            icon::PLAYLIST_ADD,
            14.0,
            self.config.theme.text_dim,
        );
        ui.painter.text_clipped(
            x + 20.0,
            y,
            "Inspector Variables",
            14.0,
            self.config.theme.text_dim,
            (width - 20.0).max(0.0),
        );
        y += 22.0;
        if variables.is_empty() {
            ui.painter.text_clipped(
                x + 8.0,
                y,
                "No Inspector(...) declarations.",
                14.0,
                self.config.theme.text_dim,
                (width - 16.0).max(0.0),
            );
            return y + 22.0;
        }

        for (index, variable) in variables.iter_mut().enumerate() {
            let base = format!("var_{entity}_{comp}_{index}");
            let label = humanize_identifier(&variable.name);
            self.script_value_editor(
                ui,
                &base,
                &label,
                &mut variable.value,
                &variable.control,
                ValueOwner::ScriptVar {
                    entity,
                    component: comp,
                    var: index,
                },
                &mut Vec::new(),
                x,
                width,
                &mut y,
                dirty,
            );
            y += 4.0;
        }
        y + 2.0
    }

    fn attached_values(
        &mut self,
        ui: &mut Ui,
        entity: u64,
        values: &mut Vec<AttachedValue>,
        x: f32,
        width: f32,
        mut y: f32,
        dirty: &mut bool,
    ) -> f32 {
        y = self.section_header(ui, x, width, y, icon::DATA_OBJECT, "Attached Values");
        if values.is_empty() {
            ui.painter.text_wrapped(
                Rect::new(x + 8.0, y, (width - 16.0).max(0.0), 34.0),
                "Add fields directly to this entity table—no component script required.",
                12.0,
                15.0,
                self.config.theme.text_dim,
            );
            y += 38.0;
        }

        let mut remove = None;
        for index in 0..values.len() {
            let base = format!("attached_{entity}_{index}");
            let panel_x = x + 4.0;
            let panel_w = (width - 4.0).max(40.0);
            ui.painter.fill_round_rect(
                Rect::new(panel_x, y, panel_w, FIELD_H + 4.0),
                3.0,
                self.config.theme.panel_alt,
            );
            ui.icon(
                panel_x + 12.0,
                y + (FIELD_H + 4.0) * 0.5,
                AttachedValueType::from_value(&values[index].value).glyph(),
                14.0,
                self.config.theme.accent,
            );
            let delete_w = 22.0;
            let name_x = panel_x + 24.0;
            let name_w = (panel_w - 24.0 - delete_w - 4.0).max(36.0);
            let name = ui.text_field(
                &format!("{base}_name"),
                Rect::new(name_x, y + 2.0, name_w, FIELD_H),
                &values[index].name,
            );
            if name.changed {
                values[index].name = name.text;
                *dirty = true;
            }
            let delete = Rect::new(panel_x + panel_w - delete_w, y + 2.0, delete_w, FIELD_H);
            if ui.icon_toggle(delete, icon::DELETE, false, self.config.theme.danger) {
                remove = Some(index);
            }
            ui.tooltip(delete, "Remove attached value");
            y += FIELD_H + 8.0;

            self.inspector_label(ui, x + 12.0, y + 4.0, "Type", LABEL_W - 18.0);
            let type_button = Rect::new(x + LABEL_W, y, (width - LABEL_W - 8.0).max(44.0), FIELD_H);
            let current = AttachedValueType::from_value(&values[index].value);
            if ui.dropdown_button(type_button, current.label()) {
                self.open_attached_value_type_menu(
                    entity,
                    index,
                    Vec::new(),
                    current,
                    type_button.x,
                    type_button.bottom() + 2.0,
                );
            }
            ui.tooltip(type_button, "Changing type resets this value");
            y += FIELD_H + 6.0;

            self.script_value_editor(
                ui,
                &base,
                "Value",
                &mut values[index].value,
                &VarControl::Field,
                ValueOwner::AttachedValue {
                    entity,
                    value: index,
                },
                &mut Vec::new(),
                x + 8.0,
                width - 8.0,
                &mut y,
                dirty,
            );
            y += 5.0;
        }
        if let Some(index) = remove {
            values.remove(index);
            *dirty = true;
        }

        let add = Rect::new(x + 4.0, y, width - 4.0, FIELD_H + 2.0);
        if ui.icon_button(add, icon::ADD_CIRCLE, "Add Value") {
            let mut suffix = values.len() + 1;
            let name = loop {
                let candidate = if suffix == 1 {
                    "value".to_string()
                } else {
                    format!("value{suffix}")
                };
                if !values.iter().any(|value| value.name == candidate) {
                    break candidate;
                }
                suffix += 1;
            };
            values.push(AttachedValue {
                name,
                value: VarValue::Number(0.0),
            });
            *dirty = true;
        }
        y + FIELD_H + 10.0
    }

    fn entity_reference_label(&self, reference: Option<u64>) -> String {
        match reference {
            Some(id) => self
                .scene
                .entity(id)
                .map(|entity| entity.name.clone())
                .unwrap_or_else(|| "Missing entity".to_string()),
            None => "None (drag an entity here)".to_string(),
        }
    }

    fn component_reference_label(&self, reference: Option<&ComponentReference>) -> String {
        let Some(reference) = reference else {
            return "None (drag a component here)".to_string();
        };
        let Some(entity) = self.scene.entity(reference.entity) else {
            return "Missing component".to_string();
        };
        let Some(component) = entity.components.get(reference.component) else {
            return "Missing component".to_string();
        };
        format!("{} / {}", entity.name, component.label())
    }

    #[allow(clippy::too_many_arguments)]
    fn script_value_editor(
        &mut self,
        ui: &mut Ui,
        base: &str,
        label: &str,
        value: &mut VarValue,
        control: &VarControl,
        owner: ValueOwner,
        path: &mut Vec<VarPathPart>,
        x: f32,
        width: f32,
        y: &mut f32,
        dirty: &mut bool,
    ) {
        match value {
            VarValue::Number(number) => {
                if let VarControl::Slider {
                    min,
                    max,
                    fractional,
                } = control
                {
                    self.inspector_label(ui, x, *y + 4.0, label, LABEL_W - 6.0);
                    let fx = x + LABEL_W;
                    let value_w = 54.0_f32.min((width - LABEL_W) * 0.35).max(36.0);
                    let slider_w = (width - LABEL_W - value_w - 6.0).max(24.0);
                    if let Some(next) = ui.slider(
                        Rect::new(fx, *y + 2.0, slider_w, FIELD_H - 4.0),
                        *number,
                        *min,
                        *max,
                    ) {
                        let next = if *fractional { next } else { next.round() };
                        if *number != next {
                            *number = next;
                            *dirty = true;
                        }
                    }
                    let field = ui.text_field(
                        &format!("{base}_slider"),
                        Rect::new(fx + slider_w + 6.0, *y, value_w, FIELD_H),
                        &format_num(*number),
                    );
                    if field.changed {
                        if let Ok(next) = field.text.trim().parse::<f32>() {
                            let next = if *fractional { next } else { next.round() };
                            *number = next.clamp(*min, *max);
                            *dirty = true;
                        }
                    }
                    *y += FIELD_H + 6.0;
                } else if self.num_row(ui, base, label, number, x, width, y) {
                    *dirty = true;
                }
            }
            VarValue::Bool(boolean) => {
                self.inspector_label(ui, x, *y + 4.0, label, LABEL_W - 6.0);
                if let Some(next) =
                    ui.checkbox(Rect::new(x + LABEL_W, *y, FIELD_H, FIELD_H), *boolean)
                {
                    *boolean = next;
                    *dirty = true;
                }
                *y += FIELD_H + 6.0;
            }
            VarValue::Text(text) => {
                if self.text_row(ui, base, label, text, x, width, y) {
                    *dirty = true;
                }
            }
            VarValue::Image(asset_path) => {
                self.inspector_label(ui, x, *y + 4.0, label, LABEL_W - 6.0);
                if self.asset_path_row(
                    ui,
                    base,
                    asset_path,
                    AssetKind::Image,
                    owner.asset_target(path),
                    x + LABEL_W,
                    (width - LABEL_W).max(30.0),
                    *y,
                ) {
                    *dirty = true;
                }
                *y += FIELD_H + 6.0;
            }
            VarValue::Audio(asset_path) => {
                self.inspector_label(ui, x, *y + 4.0, label, LABEL_W - 6.0);
                if self.asset_path_row(
                    ui,
                    base,
                    asset_path,
                    AssetKind::Sound,
                    owner.asset_target(path),
                    x + LABEL_W,
                    (width - LABEL_W).max(30.0),
                    *y,
                ) {
                    *dirty = true;
                }
                *y += FIELD_H + 6.0;
            }
            VarValue::Shader(asset_path) => {
                self.inspector_label(ui, x, *y + 4.0, label, LABEL_W - 6.0);
                if self.asset_path_row(
                    ui,
                    base,
                    asset_path,
                    AssetKind::Shader,
                    owner.asset_target(path),
                    x + LABEL_W,
                    (width - LABEL_W).max(30.0),
                    *y,
                ) {
                    *dirty = true;
                }
                *y += FIELD_H + 6.0;
            }
            VarValue::Animation(asset_path) => {
                self.inspector_label(ui, x, *y + 4.0, label, LABEL_W - 6.0);
                if self.asset_path_row(
                    ui,
                    base,
                    asset_path,
                    AssetKind::Animation,
                    owner.asset_target(path),
                    x + LABEL_W,
                    (width - LABEL_W).max(30.0),
                    *y,
                ) {
                    *dirty = true;
                }
                *y += FIELD_H + 6.0;
            }
            VarValue::Color(color) => {
                self.inspector_label(ui, x, *y + 4.0, label, LABEL_W - 6.0);
                let fx = x + LABEL_W;
                let mut next = *color;
                if self.color_row_inline(
                    ui,
                    base,
                    fx,
                    (x + width) - fx,
                    *y,
                    &mut next,
                    owner.color_target(path),
                ) {
                    *color = next;
                    *dirty = true;
                }
                *y += FIELD_H + 6.0;
            }
            VarValue::Entity(reference) => {
                self.inspector_label(ui, x, *y + 4.0, label, LABEL_W - 6.0);
                let field = Rect::new(x + LABEL_W, *y, (width - LABEL_W - 26.0).max(30.0), FIELD_H);
                let hovering = field.contains(ui.input.mouse_x, ui.input.mouse_y);
                ui.painter
                    .fill_round_rect(field, 3.0, self.config.theme.field);
                ui.painter.stroke_round_rect(
                    field,
                    3.0,
                    if hovering
                        && matches!(
                            self.inspector_reference_drag,
                            Some(InspectorReferenceDrag::Entity { .. })
                        )
                    {
                        self.config.theme.accent
                    } else {
                        self.config.theme.border
                    },
                );
                let text = self.entity_reference_label(*reference);
                ui.painter.text_clipped(
                    field.x + 6.0,
                    field.y + 4.0,
                    &text,
                    13.0,
                    self.config.theme.text,
                    field.w - 12.0,
                );
                if hovering && !ui.input.mouse_down {
                    if let Some(InspectorReferenceDrag::Entity {
                        id,
                        inspector_owner,
                    }) = self.inspector_reference_drag.take()
                    {
                        let assigned = self.entity_reference_label(Some(id));
                        *reference = Some(id);
                        self.reparent_drag = None;
                        if let Some(owner) = inspector_owner {
                            self.select_only(owner);
                        }
                        *dirty = true;
                        self.status = format!("Assigned entity {assigned}");
                    }
                }
                let clear = Rect::new(field.right() + 4.0, *y, 22.0, FIELD_H);
                if ui.icon_toggle(clear, icon::DELETE, false, self.config.theme.text_dim)
                    && reference.take().is_some()
                {
                    *dirty = true;
                }
                *y += FIELD_H + 6.0;
            }
            VarValue::Component(reference) => {
                self.inspector_label(ui, x, *y + 4.0, label, LABEL_W - 6.0);
                let field = Rect::new(x + LABEL_W, *y, (width - LABEL_W - 26.0).max(30.0), FIELD_H);
                let hovering = field.contains(ui.input.mouse_x, ui.input.mouse_y);
                ui.painter
                    .fill_round_rect(field, 3.0, self.config.theme.field);
                ui.painter.stroke_round_rect(
                    field,
                    3.0,
                    if hovering
                        && matches!(
                            self.inspector_reference_drag,
                            Some(InspectorReferenceDrag::Component(_))
                        )
                    {
                        self.config.theme.accent
                    } else {
                        self.config.theme.border
                    },
                );
                let text = self.component_reference_label(reference.as_ref());
                ui.painter.text_clipped(
                    field.x + 6.0,
                    field.y + 4.0,
                    &text,
                    13.0,
                    self.config.theme.text,
                    field.w - 12.0,
                );
                if hovering && !ui.input.mouse_down {
                    if let Some(InspectorReferenceDrag::Component(target)) =
                        self.inspector_reference_drag.take()
                    {
                        let assigned = self.component_reference_label(Some(&target));
                        *reference = Some(target);
                        *dirty = true;
                        self.status = format!("Assigned component {assigned}");
                    }
                }
                let clear = Rect::new(field.right() + 4.0, *y, 22.0, FIELD_H);
                if ui.icon_toggle(clear, icon::DELETE, false, self.config.theme.text_dim)
                    && reference.take().is_some()
                {
                    *dirty = true;
                }
                *y += FIELD_H + 6.0;
            }
            VarValue::List(values) => {
                let collapse_key = format!("{base}_list");
                let expanded = !self.collapsed.contains(&collapse_key);
                let header = Rect::new(x, *y, width, ROW_H);
                let next =
                    ui.collapsing_header(header, &format!("{label}  [{}]", values.len()), expanded);
                self.set_collapsed(&collapse_key, !next);
                *y += ROW_H + 3.0;
                if next {
                    let mut remove = None;
                    for index in 0..values.len() {
                        let item_y = *y;
                        path.push(VarPathPart::List(index));
                        if let ValueOwner::AttachedValue {
                            entity,
                            value: root_value,
                        } = owner
                        {
                            let current = AttachedValueType::from_value(&values[index]);
                            let type_rect =
                                Rect::new(x + 12.0, *y, (width - 48.0).max(38.0), FIELD_H);
                            if ui.dropdown_button(type_rect, current.label()) {
                                self.open_attached_value_type_menu(
                                    entity,
                                    root_value,
                                    path.clone(),
                                    current,
                                    type_rect.x,
                                    type_rect.bottom() + 2.0,
                                );
                            }
                            ui.tooltip(type_rect, "Change this list item's type");
                            *y += FIELD_H + 3.0;
                        }
                        self.script_value_editor(
                            ui,
                            &format!("{base}_{index}"),
                            &format!("{}", index + 1),
                            &mut values[index],
                            &VarControl::Field,
                            owner,
                            path,
                            x + 12.0,
                            width - 36.0,
                            y,
                            dirty,
                        );
                        path.pop();
                        if ui.icon_toggle(
                            Rect::new(x + width - 20.0, item_y, 20.0, FIELD_H),
                            icon::DELETE,
                            false,
                            self.config.theme.danger,
                        ) {
                            remove = Some(index);
                        }
                    }
                    if let Some(index) = remove {
                        values.remove(index);
                        *dirty = true;
                    }
                    if ui.icon_button(
                        Rect::new(x + 12.0, *y, width - 12.0, FIELD_H),
                        icon::ADD_CIRCLE,
                        "Add Item",
                    ) {
                        values.push(values.last().cloned().unwrap_or(VarValue::Number(0.0)));
                        *dirty = true;
                    }
                    *y += FIELD_H + 5.0;
                }
            }
            VarValue::Dictionary(entries) => {
                let collapse_key = format!("{base}_dictionary");
                let expanded = !self.collapsed.contains(&collapse_key);
                let header = Rect::new(x, *y, width, ROW_H);
                let next = ui.collapsing_header(
                    header,
                    &format!("{label}  {{{}}}", entries.len()),
                    expanded,
                );
                self.set_collapsed(&collapse_key, !next);
                *y += ROW_H + 3.0;
                if next {
                    let mut remove = None;
                    for index in 0..entries.len() {
                        let entry = &mut entries[index];
                        let key_kind = match &entry.key {
                            VarKey::Number(_) => "#",
                            VarKey::Bool(_) => "Bool",
                            VarKey::Text(_) => "Text",
                        };
                        let key_type = Rect::new(x + 12.0, *y, 48.0, FIELD_H);
                        if ui.button(key_type, key_kind) {
                            entry.key = match &entry.key {
                                VarKey::Text(_) => VarKey::Number(0.0),
                                VarKey::Number(_) => VarKey::Bool(false),
                                VarKey::Bool(_) => VarKey::Text(String::new()),
                            };
                            *dirty = true;
                        }
                        ui.tooltip(key_type, "Cycle table key type: text, number, boolean");
                        let key_x = x + LABEL_W;
                        match &mut entry.key {
                            VarKey::Number(key) => {
                                let result = ui.text_field(
                                    &format!("{base}_{index}_key"),
                                    Rect::new(key_x, *y, width - LABEL_W - 28.0, FIELD_H),
                                    &format_num(*key),
                                );
                                if result.changed {
                                    if let Ok(next) = result.text.trim().parse::<f32>() {
                                        *key = next;
                                        *dirty = true;
                                    }
                                }
                            }
                            VarKey::Bool(key) => {
                                if let Some(next) =
                                    ui.checkbox(Rect::new(key_x, *y, FIELD_H, FIELD_H), *key)
                                {
                                    *key = next;
                                    *dirty = true;
                                }
                            }
                            VarKey::Text(key) => {
                                let result = ui.text_field(
                                    &format!("{base}_{index}_key"),
                                    Rect::new(key_x, *y, width - LABEL_W - 28.0, FIELD_H),
                                    key,
                                );
                                if result.changed {
                                    *key = result.text;
                                    *dirty = true;
                                }
                            }
                        }
                        if ui.icon_toggle(
                            Rect::new(x + width - 20.0, *y, 20.0, FIELD_H),
                            icon::DELETE,
                            false,
                            self.config.theme.danger,
                        ) {
                            remove = Some(index);
                        }
                        *y += FIELD_H + 3.0;
                        path.push(VarPathPart::Dictionary(index));
                        if let ValueOwner::AttachedValue {
                            entity,
                            value: root_value,
                        } = owner
                        {
                            let current = AttachedValueType::from_value(&entry.value);
                            let type_rect =
                                Rect::new(x + 12.0, *y, (width - 24.0).max(38.0), FIELD_H);
                            if ui.dropdown_button(type_rect, current.label()) {
                                self.open_attached_value_type_menu(
                                    entity,
                                    root_value,
                                    path.clone(),
                                    current,
                                    type_rect.x,
                                    type_rect.bottom() + 2.0,
                                );
                            }
                            ui.tooltip(type_rect, "Change this table value's type");
                            *y += FIELD_H + 3.0;
                        }
                        self.script_value_editor(
                            ui,
                            &format!("{base}_{index}_value"),
                            "Value",
                            &mut entry.value,
                            &VarControl::Field,
                            owner,
                            path,
                            x + 12.0,
                            width - 12.0,
                            y,
                            dirty,
                        );
                        path.pop();
                        *y += 2.0;
                    }
                    if let Some(index) = remove {
                        entries.remove(index);
                        *dirty = true;
                    }
                    if ui.icon_button(
                        Rect::new(x + 12.0, *y, width - 12.0, FIELD_H),
                        icon::ADD_CIRCLE,
                        "Add Entry",
                    ) {
                        let mut suffix = entries.len() + 1;
                        let key = loop {
                            let candidate = format!("key{suffix}");
                            if !entries.iter().any(
                                |entry| matches!(&entry.key, VarKey::Text(key) if key == &candidate),
                            ) {
                                break candidate;
                            }
                            suffix += 1;
                        };
                        entries.push(DictionaryEntry {
                            key: VarKey::Text(key),
                            value: VarValue::Number(0.0),
                        });
                        *dirty = true;
                    }
                    *y += FIELD_H + 5.0;
                }
            }
        }
    }

    fn section_header(
        &mut self,
        ui: &mut Ui,
        x: f32,
        width: f32,
        y: f32,
        glyph: char,
        title: &str,
    ) -> f32 {
        ui.painter
            .fill_rect(Rect::new(x, y, width, 1.0), self.config.theme.border);
        let y = y + 6.0;
        ui.icon(x + 8.0, y + 8.0, glyph, 15.0, self.config.theme.text_dim);
        ui.painter.text_clipped(
            x + 20.0,
            y,
            title,
            14.0,
            self.config.theme.text_dim,
            (width - 24.0).max(0.0),
        );
        y + 22.0
    }

    fn inspector_label(&self, ui: &mut Ui, x: f32, y: f32, label: &str, width: f32) {
        ui.painter
            .text_clipped(x, y, label, 14.0, self.config.theme.text, width.max(1.0));
    }

    fn num_row(
        &mut self,
        ui: &mut Ui,
        id: &str,
        label: &str,
        value: &mut f32,
        x: f32,
        width: f32,
        y: &mut f32,
    ) -> bool {
        self.inspector_label(ui, x, *y + 4.0, label, LABEL_W - 6.0);
        let fx = x + LABEL_W;
        let fw = (width - LABEL_W).max(30.0);
        let r = ui.text_field(id, Rect::new(fx, *y, fw, FIELD_H), &format_num(*value));
        let mut dirty = false;
        if r.changed {
            if let Ok(v) = r.text.trim().parse::<f32>() {
                *value = v;
                dirty = true;
            } else if r.text.trim().is_empty() {
                *value = 0.0;
                dirty = true;
            }
        }
        *y += FIELD_H + 6.0;
        dirty
    }

    fn optional_num_row(
        &mut self,
        ui: &mut Ui,
        id: &str,
        label: &str,
        value: &mut Option<f32>,
        default_value: f32,
        x: f32,
        width: f32,
        y: &mut f32,
    ) -> bool {
        self.inspector_label(ui, x, *y + 4.0, label, LABEL_W - 6.0);
        let fx = x + LABEL_W;
        let fw = (width - LABEL_W).max(30.0);
        let clear_w = FIELD_H;
        let field_w = (fw - clear_w - 4.0).max(24.0);
        let current = value.unwrap_or(default_value);
        let r = ui.text_field(
            id,
            Rect::new(fx, *y, field_w, FIELD_H),
            &format_num(current),
        );
        let clear = Rect::new(fx + field_w + 4.0, *y, clear_w, FIELD_H);
        let clear_clicked = ui.icon_toggle(
            clear,
            icon::RESTART_ALT,
            value.is_some(),
            self.config.theme.text_dim,
        );
        ui.tooltip(clear, "Clear numeric override");
        let mut dirty = false;
        if r.changed {
            let trimmed = r.text.trim();
            if trimmed.is_empty() {
                if value.is_some() {
                    *value = None;
                    dirty = true;
                }
            } else if let Ok(v) = trimmed.parse::<f32>() {
                let v = if v.is_finite() { v } else { 0.0 };
                let changed = match *value {
                    Some(old) => (old - v).abs() > f32::EPSILON,
                    None => true,
                };
                if changed {
                    *value = Some(v);
                    dirty = true;
                }
            }
        }
        if clear_clicked && value.is_some() {
            *value = None;
            dirty = true;
        }
        *y += FIELD_H + 6.0;
        dirty
    }

    fn position_pivot_mode_row(
        &mut self,
        ui: &mut Ui,
        label: &str,
        value: &mut String,
        x: f32,
        width: f32,
        y: &mut f32,
    ) -> bool {
        let current = normalized_position_pivot_name(value);
        self.pivot_mode_row(
            ui,
            label,
            value,
            current,
            &[
                ("top_left", "TL", "Top-left pivot"),
                ("center", "Center", "Center pivot"),
                ("top_right", "TR", "Top-right pivot"),
            ],
            x,
            width,
            y,
        )
    }

    fn rotation_pivot_mode_row(
        &mut self,
        ui: &mut Ui,
        label: &str,
        value: &mut String,
        x: f32,
        width: f32,
        y: &mut f32,
    ) -> bool {
        let current = normalized_rotation_pivot_name(value);
        self.pivot_mode_row(
            ui,
            label,
            value,
            current,
            &[
                ("top_left", "TL", "Top-left pivot"),
                ("center", "Center", "Center pivot"),
            ],
            x,
            width,
            y,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn pivot_mode_row(
        &mut self,
        ui: &mut Ui,
        label: &str,
        value: &mut String,
        current: &str,
        options: &[(&str, &str, &str)],
        x: f32,
        width: f32,
        y: &mut f32,
    ) -> bool {
        self.inspector_label(ui, x, *y + 4.0, label, LABEL_W - 6.0);
        let fx = x + LABEL_W;
        let fw = (width - LABEL_W).max(30.0);
        let gap = 4.0;
        let count = options.len().max(1) as f32;
        let button_w = ((fw - gap * (count - 1.0)) / count).max(24.0);
        let mut dirty = false;
        for (index, (key, text, tooltip)) in options.iter().enumerate() {
            let bx = fx + index as f32 * (button_w + gap);
            let rect = Rect::new(bx, *y, button_w, FIELD_H);
            let active = current == *key;
            let base = if active {
                self.config.theme.button_active
            } else {
                self.config.theme.button
            };
            if ui.button_colored(rect, text, base, self.config.theme.text) {
                let next = pivot_storage_value(key);
                if *value != next {
                    *value = next;
                    dirty = true;
                }
            }
            ui.tooltip(rect, tooltip);
        }
        *y += FIELD_H + 6.0;
        dirty
    }

    fn text_row(
        &mut self,
        ui: &mut Ui,
        id: &str,
        label: &str,
        value: &mut String,
        x: f32,
        width: f32,
        y: &mut f32,
    ) -> bool {
        self.inspector_label(ui, x, *y + 4.0, label, LABEL_W - 6.0);
        let fx = x + LABEL_W;
        let fw = (width - LABEL_W).max(30.0);
        let r = ui.text_field(id, Rect::new(fx, *y, fw, FIELD_H), value);
        let mut dirty = false;
        if r.changed {
            *value = r.text;
            dirty = true;
        }
        *y += FIELD_H + 6.0;
        dirty
    }

    /// A color row with a swatch (opens the picker) plus inline RGB fields.
    #[allow(clippy::too_many_arguments)]
    fn color_row(
        &mut self,
        ui: &mut Ui,
        id: &str,
        label: &str,
        color: &mut [u8; 4],
        target: ColorTarget,
        x: f32,
        width: f32,
        y: f32,
    ) -> f32 {
        self.inspector_label(ui, x, y + 4.0, label, LABEL_W - 6.0);
        let fx = x + LABEL_W;
        self.color_row_inline(ui, id, fx, (x + width) - fx, y, color, target);
        y + FIELD_H + 6.0
    }

    /// A labeled numeric field for the scene-lighting panel. Returns the new `y`
    /// and the parsed value when the field changed to a valid number.
    #[allow(clippy::too_many_arguments)]
    fn lighting_num_row(
        &mut self,
        ui: &mut Ui,
        id: &str,
        label: &str,
        x: f32,
        width: f32,
        y: f32,
        value: f32,
    ) -> (f32, Option<f32>) {
        self.inspector_label(ui, x, y + 4.0, label, LABEL_W - 6.0);
        let field = Rect::new(x + LABEL_W, y, (width - LABEL_W).max(40.0), FIELD_H);
        let r = ui.text_field(id, field, &format_num(value));
        let parsed = if r.changed {
            r.text.trim().parse::<f32>().ok()
        } else {
            None
        };
        (y + FIELD_H + 6.0, parsed)
    }

    #[allow(clippy::too_many_arguments)]
    fn post_process_f32_row(
        &mut self,
        ui: &mut Ui,
        id: &str,
        label: &str,
        x: f32,
        width: f32,
        y: f32,
        value: &mut f32,
    ) -> (f32, bool) {
        let (next_y, parsed) = self.lighting_num_row(ui, id, label, x, width, y, *value);
        let Some(parsed) = parsed.filter(|value| value.is_finite()) else {
            return (next_y, false);
        };
        if *value == parsed {
            return (next_y, false);
        }
        *value = parsed;
        (next_y, true)
    }

    #[allow(clippy::too_many_arguments)]
    fn post_process_u32_row(
        &mut self,
        ui: &mut Ui,
        id: &str,
        label: &str,
        x: f32,
        width: f32,
        y: f32,
        value: &mut u32,
        min: u32,
        max: u32,
    ) -> (f32, bool) {
        let (next_y, parsed) = self.lighting_num_row(ui, id, label, x, width, y, *value as f32);
        let Some(parsed) = parsed.filter(|value| value.is_finite()) else {
            return (next_y, false);
        };
        let parsed = parsed.round().clamp(min as f32, max as f32) as u32;
        if *value == parsed {
            return (next_y, false);
        }
        *value = parsed;
        (next_y, true)
    }

    fn post_process_inspector(&mut self, ui: &mut Ui, x: f32, width: f32, mut y: f32) -> f32 {
        y = self.section_header(ui, x, width, y, icon::TUNE, "Post Process");

        self.inspector_label(ui, x, y + 4.0, "Enabled", LABEL_W - 6.0);
        if let Some(enabled) = ui.checkbox(
            Rect::new(x + LABEL_W, y, FIELD_H, FIELD_H),
            self.scene.post_process.enabled,
        ) {
            self.scene.post_process.enabled = enabled;
            self.mark_dirty();
        }
        y += FIELD_H + 8.0;

        let effect_count = self.scene.post_process.effects.len();
        let mut changed_pass = false;
        let mut cycle_kind = None;
        let mut move_pass = None;
        let mut remove_pass = None;

        for index in 0..effect_count {
            let original = self.scene.post_process.effects[index];
            let mut pass = original;
            let row = Rect::new(x, y, width, FIELD_H);
            ui.painter
                .fill_round_rect(row, 3.0, self.config.theme.panel_alt);

            let enabled_rect = Rect::new(x + 2.0, y, FIELD_H, FIELD_H);
            if let Some(enabled) = ui.checkbox(enabled_rect, pass.enabled) {
                pass.enabled = enabled;
            }
            ui.tooltip(
                enabled_rect,
                "Enable this pass without removing its settings",
            );

            let controls_x = row.right() - 66.0;
            let kind_rect = Rect::new(
                x + FIELD_H + 6.0,
                y,
                (controls_x - (x + FIELD_H + 10.0)).max(40.0),
                FIELD_H,
            );
            if ui.dropdown_button(kind_rect, post_process_effect_label(&pass.effect)) {
                cycle_kind = Some(index);
            }
            ui.tooltip(
                kind_rect,
                "Click to cycle through the supported effect kinds",
            );

            let up = Rect::new(controls_x, y, 20.0, FIELD_H);
            let down = Rect::new(controls_x + 22.0, y, 20.0, FIELD_H);
            let delete = Rect::new(controls_x + 44.0, y, 20.0, FIELD_H);
            if index > 0 {
                if ui.icon_toggle(up, icon::ARROW_UPWARD, false, self.config.theme.text_dim) {
                    move_pass = Some((index, index - 1));
                }
                ui.tooltip(up, "Move pass earlier");
            } else {
                ui.icon(
                    up.x + up.w * 0.5,
                    up.y + up.h * 0.5,
                    icon::ARROW_UPWARD,
                    15.0,
                    [
                        self.config.theme.text_dim[0],
                        self.config.theme.text_dim[1],
                        self.config.theme.text_dim[2],
                        80,
                    ],
                );
            }
            if index + 1 < effect_count {
                if ui.icon_toggle(
                    down,
                    icon::ARROW_DOWNWARD,
                    false,
                    self.config.theme.text_dim,
                ) {
                    move_pass = Some((index, index + 1));
                }
                ui.tooltip(down, "Move pass later");
            } else {
                ui.icon(
                    down.x + down.w * 0.5,
                    down.y + down.h * 0.5,
                    icon::ARROW_DOWNWARD,
                    15.0,
                    [
                        self.config.theme.text_dim[0],
                        self.config.theme.text_dim[1],
                        self.config.theme.text_dim[2],
                        80,
                    ],
                );
            }
            if ui.icon_toggle(delete, icon::DELETE, false, self.config.theme.danger) {
                remove_pass = Some(index);
            }
            ui.tooltip(delete, "Remove pass");
            y += FIELD_H + 5.0;

            let prefix = format!("post_process_{index}");
            match &mut pass.effect {
                PostProcessEffect::Bloom(config) => {
                    let (next, changed) = self.post_process_f32_row(
                        ui,
                        &format!("{prefix}_threshold"),
                        "Threshold",
                        x,
                        width,
                        y,
                        &mut config.threshold,
                    );
                    y = next;
                    changed_pass |= changed;
                    let (next, changed) = self.post_process_f32_row(
                        ui,
                        &format!("{prefix}_intensity"),
                        "Intensity",
                        x,
                        width,
                        y,
                        &mut config.intensity,
                    );
                    y = next;
                    changed_pass |= changed;
                    let (next, changed) = self.post_process_u32_row(
                        ui,
                        &format!("{prefix}_radius"),
                        "Radius",
                        x,
                        width,
                        y,
                        &mut config.radius,
                        0,
                        64,
                    );
                    y = next;
                    changed_pass |= changed;
                }
                PostProcessEffect::Pixelate(config) => {
                    let (next, changed) = self.post_process_u32_row(
                        ui,
                        &format!("{prefix}_block_size"),
                        "Block Size",
                        x,
                        width,
                        y,
                        &mut config.block_size,
                        1,
                        4096,
                    );
                    y = next;
                    changed_pass |= changed;
                }
                PostProcessEffect::ChromaticAberration(config) => {
                    let (next, changed) = self.post_process_f32_row(
                        ui,
                        &format!("{prefix}_offset"),
                        "Offset px",
                        x,
                        width,
                        y,
                        &mut config.offset_pixels,
                    );
                    y = next;
                    changed_pass |= changed;
                    let (next, changed) = self.post_process_f32_row(
                        ui,
                        &format!("{prefix}_angle"),
                        "Angle deg",
                        x,
                        width,
                        y,
                        &mut config.angle_degrees,
                    );
                    y = next;
                    changed_pass |= changed;
                }
                PostProcessEffect::MotionBlur(config) => {
                    let (next, changed) = self.post_process_f32_row(
                        ui,
                        &format!("{prefix}_strength"),
                        "Strength",
                        x,
                        width,
                        y,
                        &mut config.strength,
                    );
                    y = next;
                    changed_pass |= changed;
                }
                PostProcessEffect::Quantization(config) => {
                    let mut levels = u32::from(config.levels);
                    let (next, changed) = self.post_process_u32_row(
                        ui,
                        &format!("{prefix}_levels"),
                        "Levels",
                        x,
                        width,
                        y,
                        &mut levels,
                        2,
                        255,
                    );
                    y = next;
                    if changed {
                        config.levels = levels as u8;
                        changed_pass = true;
                    }
                    let (next, changed) = self.post_process_f32_row(
                        ui,
                        &format!("{prefix}_dither"),
                        "Dither",
                        x,
                        width,
                        y,
                        &mut config.dither_strength,
                    );
                    y = next;
                    changed_pass |= changed;
                }
                PostProcessEffect::Vignette(config) => {
                    let (next, changed) = self.post_process_f32_row(
                        ui,
                        &format!("{prefix}_strength"),
                        "Strength",
                        x,
                        width,
                        y,
                        &mut config.strength,
                    );
                    y = next;
                    changed_pass |= changed;
                    let (next, changed) = self.post_process_f32_row(
                        ui,
                        &format!("{prefix}_radius"),
                        "Radius",
                        x,
                        width,
                        y,
                        &mut config.radius,
                    );
                    y = next;
                    changed_pass |= changed;
                    let (next, changed) = self.post_process_f32_row(
                        ui,
                        &format!("{prefix}_softness"),
                        "Softness",
                        x,
                        width,
                        y,
                        &mut config.softness,
                    );
                    y = next;
                    changed_pass |= changed;
                }
                PostProcessEffect::Grayscale(config) => {
                    let (next, changed) = self.post_process_f32_row(
                        ui,
                        &format!("{prefix}_amount"),
                        "Amount",
                        x,
                        width,
                        y,
                        &mut config.amount,
                    );
                    y = next;
                    changed_pass |= changed;
                }
                PostProcessEffect::Invert(config) => {
                    let (next, changed) = self.post_process_f32_row(
                        ui,
                        &format!("{prefix}_amount"),
                        "Amount",
                        x,
                        width,
                        y,
                        &mut config.amount,
                    );
                    y = next;
                    changed_pass |= changed;
                }
                PostProcessEffect::BrightnessContrastSaturation(config) => {
                    let (next, changed) = self.post_process_f32_row(
                        ui,
                        &format!("{prefix}_brightness"),
                        "Brightness",
                        x,
                        width,
                        y,
                        &mut config.brightness,
                    );
                    y = next;
                    changed_pass |= changed;
                    let (next, changed) = self.post_process_f32_row(
                        ui,
                        &format!("{prefix}_contrast"),
                        "Contrast",
                        x,
                        width,
                        y,
                        &mut config.contrast,
                    );
                    y = next;
                    changed_pass |= changed;
                    let (next, changed) = self.post_process_f32_row(
                        ui,
                        &format!("{prefix}_saturation"),
                        "Saturation",
                        x,
                        width,
                        y,
                        &mut config.saturation,
                    );
                    y = next;
                    changed_pass |= changed;
                }
                PostProcessEffect::ExposureTonemap(config) => {
                    let (next, changed) = self.post_process_f32_row(
                        ui,
                        &format!("{prefix}_exposure"),
                        "Exposure",
                        x,
                        width,
                        y,
                        &mut config.exposure,
                    );
                    y = next;
                    changed_pass |= changed;

                    self.inspector_label(ui, x, y + 4.0, "Operator", LABEL_W - 6.0);
                    let operator_rect =
                        Rect::new(x + LABEL_W, y, (width - LABEL_W).max(40.0), FIELD_H);
                    let operator = match config.operator {
                        TonemapOperator::None => "None",
                        TonemapOperator::Reinhard => "Reinhard",
                        TonemapOperator::Aces => "ACES",
                    };
                    if ui.dropdown_button(operator_rect, operator) {
                        config.operator = match config.operator {
                            TonemapOperator::None => TonemapOperator::Reinhard,
                            TonemapOperator::Reinhard => TonemapOperator::Aces,
                            TonemapOperator::Aces => TonemapOperator::None,
                        };
                        changed_pass = true;
                    }
                    ui.tooltip(operator_rect, "Cycle none, Reinhard, and ACES tonemapping");
                    y += FIELD_H + 6.0;

                    let old_gamma = config.gamma;
                    let (next, changed) = self.post_process_f32_row(
                        ui,
                        &format!("{prefix}_gamma"),
                        "Gamma",
                        x,
                        width,
                        y,
                        &mut config.gamma,
                    );
                    y = next;
                    if changed {
                        config.gamma = config.gamma.max(0.01);
                        changed_pass |= config.gamma != old_gamma;
                    }
                }
            }

            if pass != original {
                self.scene.post_process.effects[index] = pass;
                changed_pass = true;
            }
            y += 4.0;
        }

        if changed_pass {
            self.mark_dirty();
        }
        if let Some(index) = cycle_kind {
            self.cycle_post_process_pass_kind(index);
            ui.clear_focus();
        } else if let Some((from, to)) = move_pass {
            self.move_post_process_pass(from, to);
            ui.clear_focus();
        } else if let Some(index) = remove_pass {
            self.remove_post_process_pass(index);
            ui.clear_focus();
        }

        if ui.icon_button(
            Rect::new(x, y, width, FIELD_H + 4.0),
            icon::ADD,
            "Add Effect",
        ) {
            self.add_post_process_pass();
            ui.clear_focus();
        }
        y + FIELD_H + 10.0
    }

    fn color_row_inline(
        &mut self,
        ui: &mut Ui,
        id: &str,
        fx: f32,
        fw: f32,
        y: f32,
        color: &mut [u8; 4],
        target: ColorTarget,
    ) -> bool {
        let mut dirty = false;
        let swatch = Rect::new(fx, y, 22.0, FIELD_H);
        if ui.swatch_button(swatch, *color) {
            let hue = rgb_to_hsv(*color).0;
            if self.config.layout.hsv_picker {
                let value = format!("{:02X}{:02X}{:02X}", color[0], color[1], color[2]);
                self.focus = Some("cp_hex".to_string());
                self.edit_cursor = value.chars().count();
                self.edit_selection_anchor = None;
                self.edit_buffer = value;
            }
            self.popup = Some(Popup::Color {
                target,
                x: fx,
                y: y + FIELD_H + 2.0,
                rgba: *color,
                hue,
            });
        }
        ui.tooltip(swatch, "Open color picker (with alpha)");
        // Four inline cells: R, G, B, and A (alpha / transparency, 0 = fully
        // transparent, 255 = opaque).
        let cells_x = fx + 28.0;
        let avail = (fx + fw) - cells_x;
        let cell_w = ((avail - 12.0) / 4.0).max(18.0);
        for i in 0..4 {
            let cx = cells_x + i as f32 * (cell_w + 4.0);
            let r = ui.text_field(
                &format!("{id}_{i}"),
                Rect::new(cx, y, cell_w, FIELD_H),
                &color[i].to_string(),
            );
            if r.changed {
                if let Ok(v) = r.text.trim().parse::<i32>() {
                    color[i] = v.clamp(0, 255) as u8;
                    dirty = true;
                } else if r.text.trim().is_empty() {
                    color[i] = 0;
                    dirty = true;
                }
            }
        }
        dirty
    }

    // ---- Project bin -------------------------------------------------------

    fn project_bin(&mut self, ui: &mut Ui, area: Rect) {
        ui.painter.fill_rect(area, self.config.theme.panel);
        let header = Rect::new(area.x, area.y, area.w, HEADER_H);
        ui.painter.fill_rect(header, self.config.theme.header);
        ui.icon(
            area.x + 16.0,
            area.y + HEADER_H / 2.0,
            icon::FOLDER_OPEN,
            16.0,
            self.config.theme.text,
        );
        ui.painter.text_clipped(
            area.x + 30.0,
            area.y + (HEADER_H - 14.0) / 2.0,
            "Project",
            14.0,
            self.config.theme.text,
            (area.w - 38.0).min(54.0).max(0.0),
        );

        let rel = self.bin_rel();
        ui.painter.text_clipped(
            area.x + 92.0,
            area.y + (HEADER_H - 13.0) / 2.0,
            &format!("/{rel}"),
            13.0,
            self.config.theme.text_dim,
            area.w - 320.0,
        );

        // Header buttons: new folder, new script, VS Code, reveal, up.
        let mut bx = area.right() - 30.0;
        let btn = |ui: &mut Ui, x: f32, glyph: char, tip: &str| -> bool {
            let r = Rect::new(x, area.y + 3.0, 24.0, HEADER_H - 6.0);
            let c = ui.icon_toggle(r, glyph, false, ui.theme.text);
            ui.tooltip(r, tip);
            c
        };
        if btn(ui, bx, icon::DELETE, "Close Project panel") {
            self.config.layout.show_project = false;
            self.config.layout.undock_project = false;
            self.dirty = true;
        }
        bx -= 28.0;
        if self.config.layout.undock_project {
            if btn(ui, bx, icon::OPEN_IN_NEW, "Dock Project panel") {
                self.dock_widget(EditorWidget::Project);
            }
        } else if btn(ui, bx, icon::OPEN_IN_NEW, "Undock Project panel") {
            self.config.layout.undock_project = true;
            self.dirty = true;
        }
        bx -= 28.0;
        let at_root = self.bin_dir == self.project_root;
        if !at_root && btn(ui, bx, icon::ARROW_UPWARD, "Up one folder") {
            if let Some(parent) = self.bin_dir.parent().map(|p| p.to_path_buf()) {
                self.navigate_bin(parent);
            }
        }
        bx -= 28.0;
        if btn(ui, bx, icon::OPEN_IN_NEW, "Reveal in file manager") {
            self.reveal_in_explorer();
        }
        bx -= 28.0;
        if btn(ui, bx, icon::CODE, "Open project in VS Code") {
            self.open_project_in_vscode();
        }
        bx -= 28.0;
        if btn(ui, bx, icon::CREATE_NEW_FOLDER, "New folder") {
            self.open_prompt("New folder name", Pending::CreateFolder, "NewFolder");
        }
        bx -= 28.0;
        if btn(ui, bx, icon::NOTE_ADD, "New script") {
            self.open_prompt("New script name", Pending::CreateScript, "script.luau");
        }
        ui.painter.stroke_rect(area, self.config.theme.border);

        // Drag an entity from the hierarchy into the bin to save it as a prefab.
        if let Some(drag) = self.reparent_drag {
            if area.contains(ui.input.mouse_x, ui.input.mouse_y) {
                ui.painter
                    .stroke_round_rect(area.shrink(2.0), 4.0, self.config.theme.accent);
                let name = self
                    .scene
                    .entity(drag)
                    .map(|e| e.name.clone())
                    .unwrap_or_default();
                ui.painter.text_clipped(
                    area.x + 210.0,
                    area.y + 6.0,
                    &format!("Drop to save \"{name}\" as a prefab"),
                    13.0,
                    self.config.theme.accent,
                    (area.w - 220.0).max(0.0),
                );
                if !ui.input.mouse_down {
                    self.save_prefab(drag);
                    self.reparent_drag = None;
                }
            }
        }

        let content = Rect::new(
            area.x,
            area.y + HEADER_H,
            area.w,
            (area.h - HEADER_H).max(0.0),
        );
        let listing = self.project_directory_listing();
        let dirs = &listing.dirs;
        let files = &listing.files;

        if content.contains(ui.input.mouse_x, ui.input.mouse_y) && ui.input.scroll != 0.0 {
            self.bin_scroll -= ui.input.scroll * 32.0;
            ui.wants_redraw = true;
        }
        self.bin_scroll = self
            .bin_scroll
            .clamp(0.0, (self.bin_content_h - content.h).max(0.0));

        let prev = ui.painter.push_clip(content);
        ui.set_input_clip(content);
        let row_h = 22.0;
        let mut yy = content.y + 6.0 - self.bin_scroll;
        let mut navigate = None;
        let mut open = None;
        let mut context: Option<(PathBuf, f32, f32)> = None;
        let theme = self.config.theme.clone();
        let draw =
            |ui: &mut Ui, yy: f32, glyph: char, name: &str, accent: bool| -> (bool, bool, bool) {
                let row = Rect::new(content.x + 4.0, yy, content.w - 8.0, row_h);
                let hovered = row.contains(ui.input.mouse_x, ui.input.mouse_y)
                    && content.contains(ui.input.mouse_x, ui.input.mouse_y);
                if hovered {
                    ui.painter.fill_rect(row, theme.panel_alt);
                }
                let c = if accent { theme.accent } else { theme.text_dim };
                ui.icon(row.x + 12.0, yy + row_h / 2.0, glyph, 15.0, c);
                ui.painter.text_clipped(
                    row.x + 26.0,
                    yy + (row_h - 14.0) / 2.0,
                    name,
                    14.0,
                    theme.text,
                    row.w - 30.0,
                );
                (
                    hovered && ui.input.mouse_pressed,
                    hovered && ui.input.double_click,
                    hovered && ui.input.right_pressed,
                )
            };
        for (path, name) in dirs {
            let (click, dbl, rc) = draw(ui, yy, icon::FOLDER, name, true);
            if click || dbl {
                navigate = Some(path.clone());
            }
            if rc {
                context = Some((path.clone(), ui.input.mouse_x, ui.input.mouse_y));
            }
            yy += row_h + 1.0;
        }
        let mut start_prefab_drag: Option<PathBuf> = None;
        let mut start_script_drag: Option<PathBuf> = None;
        let mut start_mesh_drag: Option<PathBuf> = None;
        for (path, name, glyph) in files {
            let (click, dbl, rc) = draw(ui, yy, *glyph, name, false);
            if dbl {
                if path.extension().is_some_and(|e| e == "neoscene") {
                    self.open_scene_path(path.clone());
                } else if path.extension().is_some_and(|e| e == "neoprefab") {
                    self.open_prefab_path(path.clone());
                } else if path.extension().is_some_and(|e| e == "neoanim") {
                    self.open_animation_path(path.clone());
                } else {
                    open = Some(path.clone());
                }
            }
            // Press-and-drag a prefab into the viewport to instantiate it.
            if click && !dbl && path.extension().is_some_and(|e| e == "neoprefab") {
                start_prefab_drag = Some(path.clone());
            }
            if click
                && path
                    .extension()
                    .is_some_and(|extension| extension == "luau" || extension == "lua")
            {
                start_script_drag = Some(path.clone());
            }
            if click && AssetKind::Mesh.accepts(path) {
                start_mesh_drag = Some(path.clone());
            }
            if rc {
                context = Some((path.clone(), ui.input.mouse_x, ui.input.mouse_y));
            }
            yy += row_h + 1.0;
        }
        if let Some(p) = start_prefab_drag {
            self.prefab_drag = Some(p);
        }
        if let Some(path) = start_script_drag {
            self.script_drag = Some(path);
        }
        if let Some(path) = start_mesh_drag {
            self.mesh_drag = Some(path);
        }
        if dirs.is_empty() && files.is_empty() {
            ui.painter.text_clipped(
                content.x + 10.0,
                content.y + 8.0,
                "Empty folder.",
                14.0,
                self.config.theme.text_dim,
                (content.w - 20.0).max(0.0),
            );
            yy += row_h;
        }
        ui.reset_input_clip();
        ui.painter.set_clip_raw(prev);
        self.bin_content_h = yy - (content.y - self.bin_scroll) + 6.0;

        // Right-click empty area of the bin.
        if content.contains(ui.input.mouse_x, ui.input.mouse_y)
            && ui.input.right_pressed
            && context.is_none()
        {
            self.open_project_menu(ui.input.mouse_x, ui.input.mouse_y);
        }
        if let Some((path, mx, my)) = context {
            self.open_path_menu(path, mx, my);
        }
        if let Some(p) = navigate {
            self.navigate_bin(p);
        }
        if let Some(p) = open {
            self.open_path(&p);
        }

        if self.bin_content_h > content.h {
            let thumb_h = (content.h * (content.h / self.bin_content_h)).max(20.0);
            let thumb_y = content.y
                + (self.bin_scroll / (self.bin_content_h - content.h)) * (content.h - thumb_h);
            ui.painter.fill_round_rect(
                Rect::new(content.right() - 6.0, thumb_y, 4.0, thumb_h),
                2.0,
                self.config.theme.text_dim,
            );
        }
    }

    fn bin_rel(&self) -> String {
        self.bin_dir
            .strip_prefix(&self.project_root)
            .ok()
            .map(|p| p.to_string_lossy().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| ".".to_string())
    }

    fn project_directory_listing(&self) -> Rc<ProjectDirectoryListing> {
        let modified = std::fs::metadata(&self.bin_dir)
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        if let Some(entry) = self.project_directory_cache.borrow().get(&self.bin_dir) {
            if entry.modified == modified {
                return entry.listing.clone();
            }
        }

        let (mut dirs, mut files) = (Vec::new(), Vec::new());
        if let Ok(read) = std::fs::read_dir(&self.bin_dir) {
            for entry in read.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                if path.is_dir() {
                    dirs.push((path, name));
                } else {
                    let glyph = file_icon(&name);
                    files.push((path, name, glyph));
                }
            }
        }
        dirs.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
        files.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
        let listing = Rc::new(ProjectDirectoryListing { dirs, files });
        self.project_directory_cache.borrow_mut().insert(
            self.bin_dir.clone(),
            ProjectDirectoryCacheEntry {
                modified,
                listing: listing.clone(),
            },
        );
        listing
    }

    fn navigate_bin(&mut self, dir: PathBuf) {
        if !dir.starts_with(&self.project_root) {
            return;
        }
        if dir != self.bin_dir {
            self.bin_back.push(self.bin_dir.clone());
            self.bin_forward.clear();
            self.bin_dir = dir;
            self.bin_scroll = 0.0;
        }
    }

    /// Back/forward navigation, wired to mouse buttons 4 and 5.
    fn bin_back(&mut self) {
        if let Some(prev) = self.bin_back.pop() {
            self.bin_forward.push(self.bin_dir.clone());
            self.bin_dir = prev;
            self.bin_scroll = 0.0;
        }
    }
    fn bin_forward(&mut self) {
        if let Some(next) = self.bin_forward.pop() {
            self.bin_back.push(self.bin_dir.clone());
            self.bin_dir = next;
            self.bin_scroll = 0.0;
        }
    }

    // ---- Status ------------------------------------------------------------

    fn status_bar(&mut self, ui: &mut Ui, w: f32, h: f32) {
        let bar = Rect::new(0.0, h - STATUS_H, w, STATUS_H);
        ui.painter.fill_rect(bar, self.config.theme.toolbar);
        ui.painter.stroke_rect(bar, self.config.theme.border);
        let snap = if self.config.layout.snap {
            "snap"
        } else {
            "free"
        };
        ui.painter.text_clipped(
            8.0,
            h - STATUS_H + 5.0,
            &format!(
                "{}   |   {} entities   |   grid {}px ({})   |   {}",
                self.status,
                self.scene.entities.len(),
                format_num(self.config.layout.grid),
                snap,
                self.scene_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
            ),
            13.0,
            self.config.theme.text_dim,
            (w - 16.0).max(0.0),
        );
    }

    // ---- Popups ------------------------------------------------------------

    fn open_add_component_menu(&mut self, entity: u64, x: f32, y: f32) {
        let mut entries: Vec<ComponentPickerEntry> = Vec::new();
        if let Some(c) = &self.component_clipboard {
            entries.push(ComponentPickerEntry {
                action: Action::PasteComponent(entity),
                glyph: icon::CONTENT_PASTE,
                label: format!("Paste {}", c.label()),
            });
        }
        entries.extend(CORE_COMPONENTS.iter().map(|name| ComponentPickerEntry {
            action: Action::AddComponent(entity, name.to_string()),
            glyph: core_icon(name),
            label: name.to_string(),
        }));
        // Custom behaviour scripts that opted in via IComponentPicker(Behaviour).
        for (label, path) in self.custom_picker_scripts() {
            entries.push(ComponentPickerEntry {
                action: Action::AddScriptComponent(entity, path),
                glyph: icon::DATA_OBJECT,
                label,
            });
        }
        entries.push(ComponentPickerEntry {
            action: Action::AddComponent(entity, "Script".to_string()),
            glyph: icon::DATA_OBJECT,
            label: "Script".to_string(),
        });
        entries.extend(ADVANCED_COMPONENTS.iter().map(|name| ComponentPickerEntry {
            action: Action::AddComponent(entity, name.to_string()),
            glyph: core_icon(name),
            label: name.to_string(),
        }));
        self.focus = Some("component_picker_search".to_string());
        self.popup = Some(Popup::ComponentPicker {
            x,
            y,
            query: String::new(),
            scroll: 0.0,
            entries,
        });
    }

    /// Discover behaviour scripts in the project that register themselves in the
    /// "Add Component" picker by calling `IComponentPicker(Behaviour)`.
    /// Returns `(display label, project-relative path)` pairs.
    fn custom_picker_scripts(&self) -> Vec<(String, String)> {
        let mut files = Vec::new();
        collect_files_with_extension(&self.project_root, "luau", &mut files);
        collect_files_with_extension(&self.project_root, "lua", &mut files);
        let mut out = Vec::new();
        for path in files {
            // Skip `.d.luau` type-definition files (they only *declare*
            // IComponentPicker) and other non-runtime modules.
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".d.luau"))
            {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            if !source.contains("IComponentPicker") {
                continue;
            }
            let rel = path
                .strip_prefix(&self.project_root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if !script_registers_component_picker(&source, &rel) {
                continue;
            }
            let label = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| rel.clone());
            out.push((label, rel));
        }
        out.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
        out
    }

    fn open_entity_menu(&mut self, id: u64, x: f32, y: f32) {
        let active = self.scene.entity(id).map(|e| e.enabled).unwrap_or(true);
        let items = vec![
            MenuItem {
                action: Action::Rename(id),
                glyph: icon::EDIT,
                label: "Rename".into(),
                danger: false,
            },
            MenuItem {
                action: Action::Duplicate(id),
                glyph: icon::CONTENT_COPY,
                label: "Duplicate".into(),
                danger: false,
            },
            MenuItem {
                action: Action::Copy(id),
                glyph: icon::CONTENT_COPY,
                label: "Copy".into(),
                danger: false,
            },
            MenuItem {
                action: Action::Paste,
                glyph: icon::CONTENT_PASTE,
                label: "Paste".into(),
                danger: false,
            },
            MenuItem {
                action: Action::ToggleActive(id),
                glyph: if active {
                    icon::VISIBILITY_OFF
                } else {
                    icon::VISIBILITY
                },
                label: if active {
                    "Deactivate".into()
                } else {
                    "Activate".into()
                },
                danger: false,
            },
            MenuItem {
                action: Action::FrameSelected(id),
                glyph: icon::CENTER_FOCUS,
                label: "Frame Selected".into(),
                danger: false,
            },
            MenuItem {
                action: Action::ResetTransform(id),
                glyph: icon::RESTART_ALT,
                label: "Reset Transform".into(),
                danger: false,
            },
            MenuItem {
                action: Action::OpenSelectionTools(x, y),
                glyph: icon::SELECT_ALL,
                label: "Selection Tools…".into(),
                danger: false,
            },
            MenuItem {
                action: Action::OpenArrangeTools(x, y),
                glyph: icon::VIEW_QUILT,
                label: "Align & Snap…".into(),
                danger: false,
            },
            MenuItem {
                action: Action::Unparent(id),
                glyph: icon::CHEVRON_LEFT,
                label: "Unparent".into(),
                danger: false,
            },
            MenuItem {
                action: Action::AddEntity(Some(id)),
                glyph: icon::ADD,
                label: "Add Child".into(),
                danger: false,
            },
            MenuItem {
                action: Action::Delete(id),
                glyph: icon::DELETE,
                label: "Delete".into(),
                danger: true,
            },
        ];
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_scene_menu(&mut self, x: f32, y: f32) {
        let items = vec![
            MenuItem {
                action: Action::NewScene,
                glyph: icon::NOTE_ADD,
                label: "New Scene".into(),
                danger: false,
            },
            MenuItem {
                action: Action::LoadScene,
                glyph: icon::FOLDER_OPEN,
                label: "Reload Scene".into(),
                danger: false,
            },
            MenuItem {
                action: Action::SaveScene,
                glyph: icon::SAVE,
                label: "Save Scene     Ctrl+S".into(),
                danger: false,
            },
            MenuItem {
                action: Action::ExportScene,
                glyph: icon::CODE,
                label: "Export Luau".into(),
                danger: false,
            },
            MenuItem {
                action: Action::RunScene,
                glyph: icon::PLAY,
                label: "Run Project".into(),
                danger: false,
            },
            MenuItem {
                action: Action::BuildProject,
                glyph: icon::DATA_OBJECT,
                label: "Build Project…".into(),
                danger: false,
            },
            MenuItem {
                action: Action::OpenMobileEmulator,
                glyph: icon::PHONE_ANDROID,
                label: "Mobile Emulator…".into(),
                danger: false,
            },
            MenuItem {
                action: Action::OpenProjectWindowSettings,
                glyph: icon::TUNE,
                label: "Project Settings…".into(),
                danger: false,
            },
            MenuItem {
                action: Action::OpenEditorSettings,
                glyph: icon::PALETTE,
                label: "Editor Settings…".into(),
                danger: false,
            },
        ];
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_tools_menu(&mut self, x: f32, y: f32) {
        let items = vec![
            MenuItem {
                action: Action::OpenSelectionTools(x, y),
                glyph: icon::SELECT_ALL,
                label: "Selection".into(),
                danger: false,
            },
            MenuItem {
                action: Action::OpenHierarchyTools(x, y),
                glyph: icon::ACCOUNT_TREE,
                label: "Hierarchy".into(),
                danger: false,
            },
            MenuItem {
                action: Action::OpenArrangeTools(x, y),
                glyph: icon::VIEW_QUILT,
                label: "Align & Snap".into(),
                danger: false,
            },
            MenuItem {
                action: Action::OpenViewTools(x, y),
                glyph: icon::CENTER_FOCUS,
                label: "Scene View".into(),
                danger: false,
            },
        ];
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_selection_tools(&mut self, x: f32, y: f32) {
        let items = vec![
            MenuItem {
                action: Action::SelectAll,
                glyph: icon::SELECT_ALL,
                label: "Select All     Ctrl+A".into(),
                danger: false,
            },
            MenuItem {
                action: Action::InvertSelection,
                glyph: icon::SWAP,
                label: "Invert Selection".into(),
                danger: false,
            },
            MenuItem {
                action: Action::SelectChildren,
                glyph: icon::ACCOUNT_TREE,
                label: "Select Descendants".into(),
                danger: false,
            },
            MenuItem {
                action: Action::SelectParent,
                glyph: icon::CHEVRON_LEFT,
                label: "Select Parent".into(),
                danger: false,
            },
            MenuItem {
                action: Action::SelectRoots,
                glyph: icon::ACCOUNT_TREE,
                label: "Select Root Entities".into(),
                danger: false,
            },
            MenuItem {
                action: Action::SelectLeaves,
                glyph: icon::ACCOUNT_TREE,
                label: "Select Leaf Entities".into(),
                danger: false,
            },
            MenuItem {
                action: Action::SelectSiblings,
                glyph: icon::SWAP,
                label: "Select Siblings".into(),
                danger: false,
            },
            MenuItem {
                action: Action::SelectNext,
                glyph: icon::CHEVRON_RIGHT,
                label: "Select Next".into(),
                danger: false,
            },
            MenuItem {
                action: Action::SelectPrevious,
                glyph: icon::CHEVRON_LEFT,
                label: "Select Previous".into(),
                danger: false,
            },
            MenuItem {
                action: Action::SelectActive,
                glyph: icon::VISIBILITY,
                label: "Select Active".into(),
                danger: false,
            },
            MenuItem {
                action: Action::SelectInactive,
                glyph: icon::VISIBILITY_OFF,
                label: "Select Inactive".into(),
                danger: false,
            },
            MenuItem {
                action: Action::SelectVisible,
                glyph: icon::VISIBILITY,
                label: "Select Scene Visible".into(),
                danger: false,
            },
            MenuItem {
                action: Action::SelectHidden,
                glyph: icon::VISIBILITY_OFF,
                label: "Select Scene Hidden".into(),
                danger: false,
            },
            MenuItem {
                action: Action::SelectLocked,
                glyph: icon::LOCK,
                label: "Select Locked".into(),
                danger: false,
            },
            MenuItem {
                action: Action::DuplicateSelection,
                glyph: icon::CONTENT_COPY,
                label: "Duplicate Selection".into(),
                danger: false,
            },
            MenuItem {
                action: Action::GroupSelected,
                glyph: icon::VIEW_IN_AR,
                label: "Group     Ctrl+G".into(),
                danger: false,
            },
            MenuItem {
                action: Action::UnparentSelected,
                glyph: icon::CHEVRON_LEFT,
                label: "Unparent     Ctrl+Shift+G".into(),
                danger: false,
            },
            MenuItem {
                action: Action::ToggleActiveSelection,
                glyph: icon::VISIBILITY,
                label: "Toggle Active".into(),
                danger: false,
            },
            MenuItem {
                action: Action::HideSelected,
                glyph: icon::VISIBILITY_OFF,
                label: "Hide in Scene View     H".into(),
                danger: false,
            },
            MenuItem {
                action: Action::HideUnselected,
                glyph: icon::VISIBILITY_OFF,
                label: "Hide Unselected".into(),
                danger: false,
            },
            MenuItem {
                action: Action::IsolateSelection,
                glyph: icon::CENTER_FOCUS,
                label: "Isolate Selection".into(),
                danger: false,
            },
            MenuItem {
                action: Action::ShowSelected,
                glyph: icon::VISIBILITY,
                label: "Show Selected".into(),
                danger: false,
            },
            MenuItem {
                action: Action::ShowAllHidden,
                glyph: icon::VISIBILITY,
                label: "Show All     Shift+H".into(),
                danger: false,
            },
            MenuItem {
                action: Action::LockSelected,
                glyph: icon::LOCK,
                label: "Lock Picking     L".into(),
                danger: false,
            },
            MenuItem {
                action: Action::LockUnselected,
                glyph: icon::LOCK,
                label: "Lock Unselected".into(),
                danger: false,
            },
            MenuItem {
                action: Action::UnlockSelection,
                glyph: icon::LOCK_OPEN,
                label: "Unlock Selection".into(),
                danger: false,
            },
            MenuItem {
                action: Action::UnlockAll,
                glyph: icon::LOCK_OPEN,
                label: "Unlock All     Shift+L".into(),
                danger: false,
            },
        ];
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_hierarchy_tools(&mut self, x: f32, y: f32) {
        let items = vec![
            MenuItem {
                action: Action::CollapseSelected,
                glyph: icon::CHEVRON_RIGHT,
                label: "Collapse Selected Branches".into(),
                danger: false,
            },
            MenuItem {
                action: Action::ExpandSelected,
                glyph: icon::EXPAND_MORE,
                label: "Expand Selected Branches".into(),
                danger: false,
            },
            MenuItem {
                action: Action::CollapseAll,
                glyph: icon::UNFOLD_LESS,
                label: "Collapse All".into(),
                danger: false,
            },
            MenuItem {
                action: Action::ExpandAll,
                glyph: icon::UNFOLD_MORE,
                label: "Expand All".into(),
                danger: false,
            },
        ];
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_arrange_tools(&mut self, x: f32, y: f32) {
        let items = vec![
            MenuItem {
                action: Action::SnapSelected,
                glyph: icon::GRID_ON,
                label: "Snap Selection to Grid".into(),
                danger: false,
            },
            MenuItem {
                action: Action::SnapSelectedSize,
                glyph: icon::ASPECT_RATIO,
                label: "Snap Selection Size".into(),
                danger: false,
            },
            MenuItem {
                action: Action::ResetSelected,
                glyph: icon::RESTART_ALT,
                label: "Reset Selected Transforms".into(),
                danger: false,
            },
            MenuItem {
                action: Action::ResetSelectedRotation,
                glyph: icon::ROTATE_RIGHT,
                label: "Reset Selected Rotation".into(),
                danger: false,
            },
            MenuItem {
                action: Action::ResetSelectedScale,
                glyph: icon::ASPECT_RATIO,
                label: "Reset Selected Scale".into(),
                danger: false,
            },
            MenuItem {
                action: Action::ResetSelectedAnchors,
                glyph: icon::CENTER_FOCUS,
                label: "Reset Selected Anchors".into(),
                danger: false,
            },
            MenuItem {
                action: Action::NormalizeSelectedSizes,
                glyph: icon::ASPECT_RATIO,
                label: "Normalize Negative Sizes".into(),
                danger: false,
            },
            MenuItem {
                action: Action::FitSelectionToWindow,
                glyph: icon::FULLSCREEN,
                label: "Fit Selection to Window".into(),
                danger: false,
            },
            MenuItem {
                action: Action::CenterSelectionInWindow,
                glyph: icon::CENTER_FOCUS,
                label: "Center Selection in Window".into(),
                danger: false,
            },
            MenuItem {
                action: Action::Align(AlignKind::Left),
                glyph: icon::CHEVRON_LEFT,
                label: "Align Left".into(),
                danger: false,
            },
            MenuItem {
                action: Action::Align(AlignKind::CenterX),
                glyph: icon::CROP_SQUARE,
                label: "Align Horizontal Centers".into(),
                danger: false,
            },
            MenuItem {
                action: Action::Align(AlignKind::Right),
                glyph: icon::CHEVRON_RIGHT,
                label: "Align Right".into(),
                danger: false,
            },
            MenuItem {
                action: Action::Align(AlignKind::Top),
                glyph: icon::ARROW_UPWARD,
                label: "Align Top".into(),
                danger: false,
            },
            MenuItem {
                action: Action::Align(AlignKind::CenterY),
                glyph: icon::CROP_SQUARE,
                label: "Align Vertical Centers".into(),
                danger: false,
            },
            MenuItem {
                action: Action::Align(AlignKind::Bottom),
                glyph: icon::EXPAND_MORE,
                label: "Align Bottom".into(),
                danger: false,
            },
            MenuItem {
                action: Action::BringToFront,
                glyph: icon::UNFOLD_MORE,
                label: "Bring to Front".into(),
                danger: false,
            },
            MenuItem {
                action: Action::SendToBack,
                glyph: icon::UNFOLD_LESS,
                label: "Send to Back".into(),
                danger: false,
            },
            MenuItem {
                action: Action::BringForward,
                glyph: icon::CHEVRON_RIGHT,
                label: "Bring Forward".into(),
                danger: false,
            },
            MenuItem {
                action: Action::SendBackward,
                glyph: icon::CHEVRON_LEFT,
                label: "Send Backward".into(),
                danger: false,
            },
            MenuItem {
                action: Action::NudgeZ(1.0),
                glyph: icon::ARROW_UPWARD,
                label: "Nudge Z +1".into(),
                danger: false,
            },
            MenuItem {
                action: Action::NudgeZ(-1.0),
                glyph: icon::EXPAND_MORE,
                label: "Nudge Z -1".into(),
                danger: false,
            },
        ];
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_view_tools(&mut self, x: f32, y: f32) {
        let items = vec![
            MenuItem {
                action: Action::FrameAll,
                glyph: icon::ZOOM_OUT_MAP,
                label: "Frame All     Home".into(),
                danger: false,
            },
            MenuItem {
                action: Action::Zoom100,
                glyph: icon::CENTER_FOCUS,
                label: "Zoom to 100%".into(),
                danger: false,
            },
            MenuItem {
                action: Action::OpenProjectRoot,
                glyph: icon::FOLDER_OPEN,
                label: "Open Project Root".into(),
                danger: false,
            },
            MenuItem {
                action: Action::RevealSceneFile,
                glyph: icon::ARTICLE,
                label: "Reveal Scene File".into(),
                danger: false,
            },
            MenuItem {
                action: Action::RefreshProject,
                glyph: icon::RESTART_ALT,
                label: "Refresh Project Browser".into(),
                danger: false,
            },
            MenuItem {
                action: Action::ToggleMaximize,
                glyph: if self.maximize_view {
                    icon::FULLSCREEN_EXIT
                } else {
                    icon::FULLSCREEN
                },
                label: if self.maximize_view {
                    "Restore Panels".into()
                } else {
                    "Maximize Scene View     Shift+Space".into()
                },
                danger: false,
            },
            MenuItem {
                action: Action::ToggleProject,
                glyph: icon::FOLDER_OPEN,
                label: if self.config.layout.show_project {
                    "Hide Project Panel".into()
                } else {
                    "Show Project Panel".into()
                },
                danger: false,
            },
        ];
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_window_menu(&mut self, x: f32, y: f32) {
        let items = vec![
            MenuItem {
                action: Action::OpenProjectWindowSettings,
                glyph: icon::TUNE,
                label: "Project Settings".into(),
                danger: false,
            },
            MenuItem {
                action: Action::ToggleProject,
                glyph: icon::FOLDER_OPEN,
                label: if self.config.layout.show_project {
                    "Close Project".into()
                } else {
                    "Show Project".into()
                },
                danger: false,
            },
            MenuItem {
                action: Action::ToggleProjectUndocked,
                glyph: icon::OPEN_IN_NEW,
                label: if self.config.layout.undock_project {
                    "Dock Project".into()
                } else {
                    "Undock Project".into()
                },
                danger: false,
            },
            MenuItem {
                action: Action::ToggleHierarchy,
                glyph: icon::ACCOUNT_TREE,
                label: if self.config.layout.show_hierarchy {
                    "Close Hierarchy".into()
                } else {
                    "Show Hierarchy".into()
                },
                danger: false,
            },
            MenuItem {
                action: Action::ToggleHierarchyUndocked,
                glyph: icon::OPEN_IN_NEW,
                label: if self.config.layout.undock_hierarchy {
                    "Dock Hierarchy".into()
                } else {
                    "Undock Hierarchy".into()
                },
                danger: false,
            },
            MenuItem {
                action: Action::ToggleInspector,
                glyph: icon::TUNE,
                label: if self.config.layout.show_inspector {
                    "Close Inspector".into()
                } else {
                    "Show Inspector".into()
                },
                danger: false,
            },
            MenuItem {
                action: Action::ToggleInspectorUndocked,
                glyph: icon::OPEN_IN_NEW,
                label: if self.config.layout.undock_inspector {
                    "Dock Inspector".into()
                } else {
                    "Undock Inspector".into()
                },
                danger: false,
            },
        ];
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_scene_antialiasing_menu(&mut self, x: f32, y: f32) {
        let current = self.scene.antialiasing.clone();
        let items = ["off", "standard", "high"]
            .into_iter()
            .map(|value| MenuItem {
                action: Action::SetSceneAntialiasing(value.to_string()),
                glyph: if current == value { icon::CHECK } else { '\0' },
                label: value.to_string(),
                danger: false,
            })
            .collect();
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_prop_enum_menu(&mut self, x: f32, y: f32, target: EnumPropMenuTarget) {
        let EnumPropMenuTarget {
            entity,
            component,
            prop,
            options,
            current,
        } = target;
        if options.is_empty() {
            return;
        }
        let items = options
            .into_iter()
            .map(|value| MenuItem {
                glyph: if value == current { icon::CHECK } else { '\0' },
                label: value.clone(),
                action: Action::SetPropEnum {
                    entity,
                    component,
                    prop,
                    value,
                },
                danger: false,
            })
            .collect();
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_attached_value_type_menu(
        &mut self,
        entity: u64,
        value: usize,
        path: Vec<VarPathPart>,
        current: AttachedValueType,
        x: f32,
        y: f32,
    ) {
        let items = AttachedValueType::ALL
            .into_iter()
            .map(|kind| MenuItem {
                action: Action::SetAttachedValueType {
                    entity,
                    value,
                    path: path.clone(),
                    kind,
                },
                glyph: if kind == current {
                    icon::CHECK
                } else {
                    kind.glyph()
                },
                label: kind.label().to_string(),
                danger: false,
            })
            .collect();
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_hierarchy_empty_menu(&mut self, x: f32, y: f32) {
        let items = vec![
            MenuItem {
                action: Action::AddEntity(None),
                glyph: icon::ADD,
                label: "Add Entity".into(),
                danger: false,
            },
            MenuItem {
                action: Action::Paste,
                glyph: icon::CONTENT_PASTE,
                label: "Paste".into(),
                danger: false,
            },
        ];
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_viewport_menu(&mut self, x: f32, y: f32, world_x: f32, world_y: f32) {
        let items = vec![
            MenuItem {
                action: Action::AddEntityAt(world_x, world_y),
                glyph: icon::ADD,
                label: "Add Entity".into(),
                danger: false,
            },
            MenuItem {
                action: Action::Paste,
                glyph: icon::CONTENT_PASTE,
                label: "Paste".into(),
                danger: false,
            },
        ];
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_project_menu(&mut self, x: f32, y: f32) {
        let items = vec![
            MenuItem {
                action: Action::NewFolder,
                glyph: icon::CREATE_NEW_FOLDER,
                label: "New Folder".into(),
                danger: false,
            },
            MenuItem {
                action: Action::NewScript,
                glyph: icon::NOTE_ADD,
                label: "New Script".into(),
                danger: false,
            },
            MenuItem {
                action: Action::NewShader,
                glyph: icon::DATA_OBJECT,
                label: "New Shader".into(),
                danger: false,
            },
            MenuItem {
                action: Action::NewAnimation,
                glyph: icon::PLAY,
                label: "New Animation".into(),
                danger: false,
            },
            MenuItem {
                action: Action::OpenProjectInVscode,
                glyph: icon::CODE,
                label: "Open in VS Code".into(),
                danger: false,
            },
            MenuItem {
                action: Action::RevealInExplorer,
                glyph: icon::OPEN_IN_NEW,
                label: "Reveal in File Manager".into(),
                danger: false,
            },
        ];
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_path_menu(&mut self, path: PathBuf, x: f32, y: f32) {
        let is_dir = path.is_dir();
        let mut items = Vec::new();
        if is_dir {
            items.push(MenuItem {
                action: Action::EnterFolder(path.clone()),
                glyph: icon::FOLDER_OPEN,
                label: "Open Folder".into(),
                danger: false,
            });
        } else if path.extension().is_some_and(|e| e == "neoscene") {
            items.push(MenuItem {
                action: Action::OpenScene(path.clone()),
                glyph: icon::ARTICLE,
                label: "Open Scene".into(),
                danger: false,
            });
        } else if path.extension().is_some_and(|e| e == "neoanim") {
            items.push(MenuItem {
                action: Action::OpenAnimation(path.clone()),
                glyph: icon::PLAY,
                label: "Open Animation".into(),
                danger: false,
            });
        } else {
            items.push(MenuItem {
                action: Action::OpenPath(path.clone()),
                glyph: icon::OPEN_IN_NEW,
                label: "Open".into(),
                danger: false,
            });
        }
        items.push(MenuItem {
            action: Action::RevealInExplorer,
            glyph: icon::FOLDER_OPEN,
            label: "Reveal Folder".into(),
            danger: false,
        });
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_confirm(&mut self, message: &str, action: Pending) {
        self.popup = Some(Popup::Confirm {
            message: message.to_string(),
            action,
        });
    }

    fn open_prompt(&mut self, title: &str, action: Pending, initial: &str) {
        self.focus = Some("prompt_field".to_string());
        self.edit_buffer = initial.to_string();
        self.edit_cursor = initial.chars().count();
        self.edit_selection_anchor = None;
        self.popup = Some(Popup::Prompt {
            title: title.to_string(),
            action,
        });
    }

    fn open_editor_settings(&mut self) {
        let font_path = self.config.settings.font_path.clone();
        self.focus = Some("editor_font_path".to_string());
        self.edit_cursor = font_path.chars().count();
        self.edit_selection_anchor = None;
        self.edit_buffer = font_path.clone();
        self.popup = Some(Popup::EditorSettings {
            theme_name: self.config.settings.theme_name.clone(),
            custom_theme: self.config.custom_theme.clone(),
            original_theme: self.config.theme.clone(),
            font_path,
            show_tooltips: self.config.settings.show_tooltips,
            show_window_bounds: self.config.settings.show_window_bounds,
            show_transform_hud: self.config.settings.show_transform_hud,
            preview_lighting: self.config.settings.preview_lighting,
            autosave_before_run: self.config.settings.autosave_before_run,
            autosave_before_build: self.config.settings.autosave_before_build,
            viewport_camera_sensitivity: self.config.settings.viewport_camera_sensitivity,
            viewport_camera_speed: self.config.settings.viewport_camera_speed,
            viewport_camera_fov: self.config.settings.viewport_camera_fov,
            viewport_invert_mouse_look: self.config.settings.viewport_invert_mouse_look,
        });
    }

    fn open_build_target(&mut self) {
        self.popup = Some(Popup::BuildTarget);
    }

    fn open_mobile_emulator(&mut self) {
        self.popup = Some(Popup::MobileEmulator {
            enabled: self.config.settings.mobile_emulator,
            orientation: self.config.settings.mobile_orientation.clone(),
            wifi: self.config.settings.mobile_wifi,
            cellular: self.config.settings.mobile_cellular,
            low_power: self.config.settings.mobile_low_power,
        });
    }

    fn handle_popup(&mut self, ui: &mut Ui, w: f32, h: f32, interactive: bool) {
        let popup = match self.popup.take() {
            Some(p) => p,
            None => return,
        };
        // Escape closes any popup (but not on the frame it just opened).
        if interactive && ui.input.escape && !matches!(&popup, Popup::EditorSettings { .. }) {
            ui.clear_focus();
            return;
        }
        match popup {
            Popup::Menu { x, y, items } => self.draw_menu(ui, x, y, items, w, h, interactive),
            Popup::ComponentPicker {
                x,
                y,
                query,
                scroll,
                entries,
            } => self.draw_component_picker(ui, x, y, query, scroll, entries, w, h, interactive),
            Popup::Color {
                target,
                x,
                y,
                rgba,
                hue,
            } => self.draw_color_picker(ui, target, x, y, rgba, hue, w, h, interactive),
            Popup::Confirm { message, action } => {
                self.draw_confirm(ui, message, action, w, h, interactive)
            }
            Popup::Prompt { title, action } => {
                self.draw_prompt(ui, title, action, w, h, interactive)
            }
            Popup::Asset {
                target,
                kind,
                files,
                query,
                scroll,
            } => self.draw_asset_picker(ui, target, kind, files, query, scroll, w, h, interactive),
            Popup::Sequence {
                target,
                kind,
                value,
                selected,
                dragging,
                color_picker,
            } => self.draw_sequence_editor(
                ui,
                target,
                kind,
                value,
                selected,
                dragging,
                color_picker,
                w,
                h,
                interactive,
            ),
            Popup::Error { message, copied } => {
                self.draw_error(ui, message, copied, w, h, interactive)
            }
            Popup::BuildTarget => self.draw_build_target_picker(ui, w, h, interactive),
            Popup::ProjectWindow {
                start_scene,
                width,
                height,
                fullscreen,
                resizable,
            } => self.draw_project_window_settings(
                ui,
                start_scene,
                width,
                height,
                fullscreen,
                resizable,
                w,
                h,
                interactive,
            ),
            Popup::MobileEmulator {
                enabled,
                orientation,
                wifi,
                cellular,
                low_power,
            } => self.draw_mobile_emulator(
                ui,
                enabled,
                orientation,
                wifi,
                cellular,
                low_power,
                w,
                h,
                interactive,
            ),
            Popup::EditorSettings {
                theme_name,
                custom_theme,
                original_theme,
                font_path,
                show_tooltips,
                show_window_bounds,
                show_transform_hud,
                preview_lighting,
                autosave_before_run,
                autosave_before_build,
                viewport_camera_sensitivity,
                viewport_camera_speed,
                viewport_camera_fov,
                viewport_invert_mouse_look,
            } => self.draw_editor_settings(
                ui,
                theme_name,
                custom_theme,
                original_theme,
                font_path,
                show_tooltips,
                show_window_bounds,
                show_transform_hud,
                preview_lighting,
                autosave_before_run,
                autosave_before_build,
                viewport_camera_sensitivity,
                viewport_camera_speed,
                viewport_camera_fov,
                viewport_invert_mouse_look,
                w,
                h,
                interactive,
            ),
            Popup::AnimationEditor {
                path,
                clip,
                selected_track,
                selected_key,
            } => self.draw_animation_editor(
                ui,
                path,
                clip,
                selected_track,
                selected_key,
                w,
                h,
                interactive,
            ),
        }
        if self.popup.is_none() {
            ui.clear_focus();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_sequence_editor(
        &mut self,
        ui: &mut Ui,
        target: AssetTarget,
        kind: SequenceKind,
        mut value: SequenceValue,
        mut selected: usize,
        mut dragging: Option<usize>,
        mut color_picker: Option<SequenceColorPicker>,
        w: f32,
        h: f32,
        interactive: bool,
    ) {
        let width = (w - 32.0).min(620.0).max(340.0);
        let height = 230.0_f32.min(h - 24.0).max(200.0);
        let x = (w - width) * 0.5;
        let y = (h - height) * 0.5;
        let panel = Rect::new(x, y, width, height);
        ui.painter
            .fill_rect(Rect::new(0.0, 0.0, w, h), [0, 0, 0, 120]);
        ui.painter
            .fill_round_rect(panel, 6.0, self.config.theme.panel);
        ui.painter
            .stroke_round_rect(panel, 6.0, self.config.theme.accent);
        let picker_was_open = color_picker.is_some();
        let raw_mouse_pressed = ui.input.mouse_pressed;
        let raw_mouse_down = ui.input.mouse_down;
        if picker_was_open {
            // The nested picker owns pointer input while it is open. This also
            // prevents clicks in it from activating obscured sequence controls.
            ui.input.mouse_pressed = false;
            ui.input.mouse_down = false;
        }
        let sequence_interactive = interactive && !picker_was_open;
        let title = match kind {
            SequenceKind::Color => "Particle Color Over Lifetime",
            SequenceKind::Transparency => "Particle Transparency Over Lifetime",
        };
        ui.painter
            .text(x + 14.0, y + 12.0, title, 16.0, self.config.theme.text);

        let strip = Rect::new(x + 18.0, y + 44.0, width - 36.0, 62.0);
        draw_sequence_strip(&mut ui.painter, strip, &value, self.config.theme.field);
        ui.painter.stroke_rect(strip, self.config.theme.border);

        let times: Vec<f32> = match &value {
            SequenceValue::Colors(keypoints) => {
                keypoints.iter().map(|keypoint| keypoint.time).collect()
            }
            SequenceValue::Numbers(keypoints) => {
                keypoints.iter().map(|keypoint| keypoint.time).collect()
            }
        };
        selected = selected.min(times.len().saturating_sub(1));
        let mut changed = false;
        let mut marker_hit = None;
        for (index, time) in times.iter().enumerate() {
            let marker_x = strip.x + strip.w * time.clamp(0.0, 1.0);
            let color = if index == selected {
                self.config.theme.accent
            } else {
                self.config.theme.text
            };
            ui.painter.fill_triangle(
                (marker_x - 6.0, strip.y - 9.0),
                (marker_x + 6.0, strip.y - 9.0),
                (marker_x, strip.y - 1.0),
                color,
            );
            ui.painter.fill_triangle(
                (marker_x - 6.0, strip.bottom() + 9.0),
                (marker_x + 6.0, strip.bottom() + 9.0),
                (marker_x, strip.bottom() + 1.0),
                color,
            );
            if sequence_interactive
                && ui.input.mouse_pressed
                && (ui.input.mouse_x - marker_x).abs() <= 9.0
                && ui.input.mouse_y >= strip.y - 12.0
                && ui.input.mouse_y <= strip.bottom() + 12.0
            {
                marker_hit = Some(index);
            }
        }
        if let Some(index) = marker_hit {
            selected = index;
            dragging = (index > 0 && index + 1 < times.len()).then_some(index);
            ui.clear_focus();
        }

        if let Some(index) = dragging {
            if !ui.input.mouse_down {
                dragging = None;
            } else if index > 0 && index + 1 < times.len() {
                let requested = ((ui.input.mouse_x - strip.x) / strip.w).clamp(0.001, 0.999);
                let lower = times[index - 1] + 0.001;
                let upper = times[index + 1] - 0.001;
                if lower <= upper {
                    let time = requested.clamp(lower, upper);
                    match &mut value {
                        SequenceValue::Colors(keypoints) => keypoints[index].time = time,
                        SequenceValue::Numbers(keypoints) => keypoints[index].time = time,
                    }
                    if (time - times[index]).abs() > f32::EPSILON {
                        changed = true;
                    }
                    selected = index;
                    ui.wants_redraw = true;
                }
            } else {
                dragging = None;
            }
        }

        if sequence_interactive
            && marker_hit.is_none()
            && dragging.is_none()
            && strip.contains(ui.input.mouse_x, ui.input.mouse_y)
            && ui.input.mouse_pressed
        {
            let time = ((ui.input.mouse_x - strip.x) / strip.w).clamp(0.001, 0.999);
            match &mut value {
                SequenceValue::Colors(keypoints) => {
                    let color = sample_color_sequence(keypoints, time);
                    keypoints.push(ColorKeypoint { time, color });
                    keypoints.sort_by(|a, b| a.time.total_cmp(&b.time));
                    selected = nearest_color_keypoint(keypoints, time);
                }
                SequenceValue::Numbers(keypoints) => {
                    let number = sample_number_sequence(keypoints, time);
                    keypoints.push(NumberKeypoint {
                        time,
                        value: number,
                    });
                    keypoints.sort_by(|a, b| a.time.total_cmp(&b.time));
                    selected = nearest_number_keypoint(keypoints, time);
                }
            }
            changed = true;
        }

        let controls_y = y + 130.0;
        ui.painter.text(
            x + 18.0,
            controls_y + 5.0,
            "Time",
            14.0,
            self.config.theme.text_dim,
        );
        let selected_time = match &value {
            SequenceValue::Colors(keypoints) => keypoints
                .get(selected)
                .map(|keypoint| keypoint.time)
                .unwrap_or(0.0),
            SequenceValue::Numbers(keypoints) => keypoints
                .get(selected)
                .map(|keypoint| keypoint.time)
                .unwrap_or(0.0),
        };
        let time_field = ui.text_field(
            "sequence_time",
            Rect::new(x + 56.0, controls_y, 70.0, FIELD_H),
            &format_num(selected_time),
        );
        if time_field.changed {
            if let Ok(mut time) = time_field.text.trim().parse::<f32>() {
                let len = times.len();
                time = if selected == 0 {
                    0.0
                } else if selected + 1 == len {
                    1.0
                } else {
                    time.clamp(0.001, 0.999)
                };
                match &mut value {
                    SequenceValue::Colors(keypoints) => {
                        if let Some(keypoint) = keypoints.get_mut(selected) {
                            keypoint.time = time;
                        }
                        keypoints.sort_by(|a, b| a.time.total_cmp(&b.time));
                        selected = nearest_color_keypoint(keypoints, time);
                    }
                    SequenceValue::Numbers(keypoints) => {
                        if let Some(keypoint) = keypoints.get_mut(selected) {
                            keypoint.time = time;
                        }
                        keypoints.sort_by(|a, b| a.time.total_cmp(&b.time));
                        selected = nearest_number_keypoint(keypoints, time);
                    }
                }
                changed = true;
            }
        }
        ui.painter.text(
            x + 132.0,
            controls_y + 5.0,
            &format!("{}%", (selected_time * 100.0).round() as i32),
            13.0,
            self.config.theme.text_dim,
        );

        match &mut value {
            SequenceValue::Colors(keypoints) => {
                ui.painter.text(
                    x + 178.0,
                    controls_y + 5.0,
                    "Color",
                    14.0,
                    self.config.theme.text_dim,
                );
                if let Some(keypoint) = keypoints.get_mut(selected) {
                    let swatch = Rect::new(x + 222.0, controls_y, 24.0, FIELD_H);
                    if sequence_interactive && ui.swatch_button(swatch, keypoint.color) {
                        color_picker = Some(SequenceColorPicker {
                            rgba: keypoint.color,
                            hue: rgb_to_hsv(keypoint.color).0,
                        });
                        if self.config.layout.hsv_picker {
                            ui.focus_text(
                                "cp_hex",
                                &format!(
                                    "{:02X}{:02X}{:02X}",
                                    keypoint.color[0], keypoint.color[1], keypoint.color[2]
                                ),
                            );
                        } else {
                            ui.clear_focus();
                        }
                    }
                    ui.tooltip(swatch, "Open color picker");
                    let labels = ["R", "G", "B"];
                    for index in 0..3 {
                        let field_x = x + 252.0 + index as f32 * 76.0;
                        ui.painter.text(
                            field_x,
                            controls_y + 5.0,
                            labels[index],
                            12.0,
                            self.config.theme.text_dim,
                        );
                        let response = ui.text_field(
                            &format!("sequence_color_{index}"),
                            Rect::new(field_x + 14.0, controls_y, 54.0, FIELD_H),
                            &keypoint.color[index].to_string(),
                        );
                        if response.changed {
                            if let Ok(channel) = response.text.trim().parse::<u8>() {
                                keypoint.color[index] = channel;
                                keypoint.color[3] = 255;
                                changed = true;
                            }
                        }
                    }
                }
            }
            SequenceValue::Numbers(keypoints) => {
                ui.painter.text(
                    x + 178.0,
                    controls_y + 5.0,
                    "Transparency",
                    14.0,
                    self.config.theme.text_dim,
                );
                if let Some(keypoint) = keypoints.get_mut(selected) {
                    let response = ui.text_field(
                        "sequence_number",
                        Rect::new(x + 278.0, controls_y, 80.0, FIELD_H),
                        &format_num(keypoint.value),
                    );
                    if response.changed {
                        if let Ok(number) = response.text.trim().parse::<f32>() {
                            keypoint.value = number.clamp(0.0, 1.0);
                            changed = true;
                        }
                    }
                    ui.painter.text(
                        x + 364.0,
                        controls_y + 5.0,
                        &format!("{}%", (keypoint.value * 100.0).round() as i32),
                        13.0,
                        self.config.theme.text_dim,
                    );
                }
            }
        }

        let buttons_y = panel.bottom() - 38.0;
        let add = Rect::new(x + 18.0, buttons_y, 80.0, 26.0);
        let delete = Rect::new(x + 104.0, buttons_y, 80.0, 26.0);
        let reset = Rect::new(panel.right() - 184.0, buttons_y, 78.0, 26.0);
        let close = Rect::new(panel.right() - 98.0, buttons_y, 80.0, 26.0);
        if sequence_interactive && ui.button(add, "Add Key") {
            dragging = None;
            match &mut value {
                SequenceValue::Colors(keypoints) => {
                    let time = largest_color_gap_midpoint(keypoints);
                    let color = sample_color_sequence(keypoints, time);
                    keypoints.push(ColorKeypoint { time, color });
                    keypoints.sort_by(|a, b| a.time.total_cmp(&b.time));
                    selected = nearest_color_keypoint(keypoints, time);
                }
                SequenceValue::Numbers(keypoints) => {
                    let time = largest_number_gap_midpoint(keypoints);
                    let number = sample_number_sequence(keypoints, time);
                    keypoints.push(NumberKeypoint {
                        time,
                        value: number,
                    });
                    keypoints.sort_by(|a, b| a.time.total_cmp(&b.time));
                    selected = nearest_number_keypoint(keypoints, time);
                }
            }
            changed = true;
        }
        let sequence_len = match &value {
            SequenceValue::Colors(keypoints) => keypoints.len(),
            SequenceValue::Numbers(keypoints) => keypoints.len(),
        };
        if sequence_interactive
            && ui.button(delete, "Delete")
            && selected > 0
            && selected + 1 < sequence_len
        {
            dragging = None;
            match &mut value {
                SequenceValue::Colors(keypoints) => {
                    keypoints.remove(selected);
                }
                SequenceValue::Numbers(keypoints) => {
                    keypoints.remove(selected);
                }
            }
            selected = selected.saturating_sub(1);
            changed = true;
        }
        if sequence_interactive && ui.button(reset, "Reset") {
            dragging = None;
            color_picker = None;
            value = match kind {
                SequenceKind::Color => SequenceValue::Colors(vec![
                    ColorKeypoint {
                        time: 0.0,
                        color: [255, 184, 76, 255],
                    },
                    ColorKeypoint {
                        time: 1.0,
                        color: [255, 92, 40, 255],
                    },
                ]),
                SequenceKind::Transparency => SequenceValue::Numbers(vec![
                    NumberKeypoint {
                        time: 0.0,
                        value: 0.0,
                    },
                    NumberKeypoint {
                        time: 1.0,
                        value: 1.0,
                    },
                ]),
            };
            selected = 0;
            changed = true;
        }
        let close_clicked = sequence_interactive && ui.button(close, "Close");

        ui.input.mouse_pressed = raw_mouse_pressed;
        ui.input.mouse_down = raw_mouse_down;
        if let Some(picker) = color_picker {
            let response = self.draw_color_picker_panel(
                ui,
                x + 222.0,
                controls_y + FIELD_H + 2.0,
                picker.rgba,
                picker.hue,
                w,
                h,
                interactive && picker_was_open,
            );
            if response.changed {
                if let SequenceValue::Colors(keypoints) = &mut value {
                    if let Some(keypoint) = keypoints.get_mut(selected) {
                        keypoint.color = response.rgba;
                        changed = true;
                    }
                }
            }
            color_picker = response.open.then_some(SequenceColorPicker {
                rgba: response.rgba,
                hue: response.hue,
            });
        }
        if changed {
            self.assign_sequence(target.clone(), &value);
        }
        if close_clicked {
            ui.clear_focus();
        } else {
            self.popup = Some(Popup::Sequence {
                target,
                kind,
                value,
                selected,
                dragging,
                color_picker,
            });
        }
    }

    fn assign_sequence(&mut self, target: AssetTarget, value: &SequenceValue) {
        let AssetTarget::Prop {
            entity,
            component,
            prop,
        } = target
        else {
            return;
        };
        let Some(entity) = self.scene.entity_mut(entity) else {
            return;
        };
        let Some(Component::Core { props, .. }) = entity.components.get_mut(component) else {
            return;
        };
        let Some(prop) = props.get_mut(prop) else {
            return;
        };
        match (&mut prop.value, value) {
            (PropValue::ColorSequence(current), SequenceValue::Colors(next)) => {
                *current = next.clone()
            }
            (PropValue::NumberSequence(current), SequenceValue::Numbers(next)) => {
                *current = next.clone()
            }
            _ => return,
        }
        let label = prop.label.clone();
        self.mark_dirty();
        self.status = format!("Updated {label} sequence");
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_asset_picker(
        &mut self,
        ui: &mut Ui,
        target: AssetTarget,
        kind: AssetKind,
        files: Vec<String>,
        mut query: String,
        mut scroll: f32,
        w: f32,
        h: f32,
        interactive: bool,
    ) {
        let has_preview = matches!(kind, AssetKind::Image | AssetKind::Sound);
        let width = if has_preview {
            (w - 32.0).min(760.0).max(340.0)
        } else {
            (w - 32.0).min(520.0).max(280.0)
        };
        let height = if has_preview {
            (h - 48.0).min(480.0).max(260.0)
        } else {
            (h - 48.0).min(440.0).max(220.0)
        };
        let x = (w - width) * 0.5;
        let y = (h - height) * 0.5;
        let panel = Rect::new(x, y, width, height);
        let preview_w = if has_preview && width >= 620.0 {
            220.0
        } else {
            0.0
        };
        let list_w = if preview_w > 0.0 {
            (width - preview_w - 36.0).max(180.0)
        } else {
            width - 24.0
        };
        ui.painter
            .fill_rect(Rect::new(0.0, 0.0, w, h), [0, 0, 0, 120]);
        ui.painter
            .fill_round_rect(panel, 6.0, self.config.theme.panel);
        ui.painter
            .stroke_round_rect(panel, 6.0, self.config.theme.accent);
        ui.icon(
            x + 20.0,
            y + 20.0,
            kind.glyph(),
            17.0,
            self.config.theme.accent,
        );
        ui.painter.text(
            x + 34.0,
            y + 12.0,
            kind.title(),
            16.0,
            self.config.theme.text,
        );

        let close = Rect::new(panel.right() - 32.0, y + 7.0, 24.0, 24.0);
        let close_clicked =
            interactive && ui.icon_toggle(close, icon::DELETE, false, self.config.theme.text_dim);

        let search = ui.text_field(
            "asset_picker_search",
            Rect::new(x + 12.0, y + 40.0, list_w, FIELD_H + 2.0),
            &query,
        );
        if search.changed {
            query = search.text;
            scroll = 0.0;
        }

        let query_lower = query.trim().to_lowercase();
        let paths: Vec<&String> = files
            .iter()
            .filter(|path| query_lower.is_empty() || path.to_lowercase().contains(&query_lower))
            .collect();

        let list = Rect::new(x + 12.0, y + 72.0, list_w, height - 116.0);
        let row_h = 26.0;
        let content_h = (paths.len() + 1) as f32 * row_h;
        let max_scroll = (content_h - list.h).max(0.0);
        if interactive
            && list.contains(ui.input.mouse_x, ui.input.mouse_y)
            && ui.input.scroll != 0.0
        {
            scroll = (scroll - ui.input.scroll * row_h * 2.0).clamp(0.0, max_scroll);
            ui.wants_redraw = true;
        } else {
            scroll = scroll.clamp(0.0, max_scroll);
        }

        let previous_clip = ui.painter.push_clip(list);
        ui.set_input_clip(list);
        let mut chosen: Option<String> = None;
        let mut preview_asset: Option<String> = None;
        let mut row_y = list.y - scroll;
        let draw_asset = |ui: &mut Ui, row_y: f32, glyph: char, label: &str| -> (bool, bool) {
            let row = Rect::new(list.x, row_y, list.w, row_h - 1.0);
            let hovered = row.contains(ui.input.mouse_x, ui.input.mouse_y);
            if hovered {
                ui.painter.fill_round_rect(row, 3.0, ui.theme.panel_alt);
            }
            ui.icon(
                row.x + 13.0,
                row.y + row.h * 0.5,
                glyph,
                15.0,
                ui.theme.text_dim,
            );
            ui.painter.text_clipped(
                row.x + 28.0,
                row.y + 5.0,
                label,
                14.0,
                ui.theme.text,
                row.w - 34.0,
            );
            (hovered && ui.input.mouse_pressed, hovered)
        };
        if draw_asset(ui, row_y, icon::DELETE, "None").0 && interactive {
            chosen = Some(String::new());
        }
        row_y += row_h;
        for path in &paths {
            let (clicked, hovered) = draw_asset(ui, row_y, kind.glyph(), path);
            if hovered {
                preview_asset = Some((*path).clone());
            }
            if clicked && interactive {
                chosen = Some((*path).clone());
            }
            row_y += row_h;
        }
        ui.reset_input_clip();
        ui.painter.set_clip_raw(previous_clip);
        if preview_asset.is_none() {
            preview_asset = paths.first().map(|path| (*path).clone());
        }

        if paths.is_empty() {
            ui.painter.text_clipped(
                list.x + 32.0,
                list.y + row_h + 8.0,
                "No matching assets in this project.",
                13.0,
                self.config.theme.text_dim,
                (list.w - 40.0).max(0.0),
            );
        }
        if max_scroll > 0.0 {
            let thumb_h = (list.h * list.h / content_h).max(20.0);
            let thumb_y = list.y + (scroll / max_scroll) * (list.h - thumb_h);
            ui.painter.fill_round_rect(
                Rect::new(list.right() - 4.0, thumb_y, 3.0, thumb_h),
                1.5,
                self.config.theme.text_dim,
            );
        }
        if preview_w > 0.0 {
            let preview = Rect::new(list.right() + 12.0, y + 40.0, preview_w, height - 84.0);
            self.draw_asset_preview(ui, preview, kind, preview_asset.as_deref());
        }

        ui.painter.text(
            x + 12.0,
            panel.bottom() - 30.0,
            &format!(
                "{} asset{}",
                paths.len(),
                if paths.len() == 1 { "" } else { "s" }
            ),
            12.0,
            self.config.theme.text_dim,
        );

        if let Some(path) = chosen {
            self.assign_asset(target, kind, path);
            ui.clear_focus();
        } else if close_clicked {
            ui.clear_focus();
        } else {
            self.popup = Some(Popup::Asset {
                target,
                kind,
                files,
                query,
                scroll,
            });
        }
    }

    fn draw_asset_preview(&self, ui: &mut Ui, rect: Rect, kind: AssetKind, path: Option<&str>) {
        ui.painter
            .fill_round_rect(rect, 4.0, self.config.theme.field);
        ui.painter
            .stroke_round_rect(rect, 4.0, self.config.theme.border);
        ui.icon(
            rect.x + 18.0,
            rect.y + 18.0,
            kind.glyph(),
            16.0,
            self.config.theme.accent,
        );
        ui.painter.text(
            rect.x + 32.0,
            rect.y + 10.0,
            "Preview",
            14.0,
            self.config.theme.text,
        );

        let body = Rect::new(rect.x + 10.0, rect.y + 36.0, rect.w - 20.0, rect.h - 62.0);
        ui.painter
            .fill_round_rect(body, 3.0, self.config.theme.panel_alt);

        match (kind, path) {
            (AssetKind::Image, Some(path)) => {
                if let Some(image) = self.load_image(path) {
                    let fit = fit_rect_to_bounds(
                        image.width() as f32,
                        image.height() as f32,
                        body.shrink(8.0),
                    );
                    ui.painter
                        .draw_image(&image, fit, None, [255, 255, 255, 255]);
                    ui.painter.stroke_rect(fit, self.config.theme.border);
                } else {
                    ui.painter.text(
                        body.x + 10.0,
                        body.y + body.h * 0.5 - 7.0,
                        "Image preview unavailable",
                        13.0,
                        self.config.theme.text_dim,
                    );
                }
            }
            (AssetKind::Sound, Some(path)) => {
                if let Some(peaks) = self.load_sound_waveform(path) {
                    draw_waveform_preview(ui, body.shrink(10.0), &peaks);
                } else {
                    ui.painter.text(
                        body.x + 10.0,
                        body.y + body.h * 0.5 - 7.0,
                        "Waveform unavailable",
                        13.0,
                        self.config.theme.text_dim,
                    );
                }
            }
            _ => {
                ui.painter.text(
                    body.x + 10.0,
                    body.y + body.h * 0.5 - 7.0,
                    "Hover an asset to preview",
                    13.0,
                    self.config.theme.text_dim,
                );
            }
        }

        if let Some(path) = path {
            ui.painter.text_clipped(
                rect.x + 10.0,
                rect.bottom() - 19.0,
                path,
                11.0,
                self.config.theme.text_dim,
                rect.w - 20.0,
            );
        }
    }

    fn asset_paths(&self, kind: AssetKind) -> Vec<String> {
        let mut files = Vec::new();
        collect_asset_files(&self.project_root, kind, &mut files);
        let mut paths: Vec<String> = files
            .into_iter()
            .filter_map(|path| {
                Some(
                    path.strip_prefix(&self.project_root)
                        .ok()?
                        .to_string_lossy()
                        .replace('\\', "/"),
                )
            })
            .collect();
        paths.sort_by_key(|path| path.to_lowercase());
        paths
    }

    fn assign_asset(&mut self, target: AssetTarget, kind: AssetKind, path: String) {
        let label = match target {
            AssetTarget::Prop {
                entity,
                component,
                prop,
            } => {
                let Some(entity) = self.scene.entity_mut(entity) else {
                    return;
                };
                let Some(Component::Core { props, .. }) = entity.components.get_mut(component)
                else {
                    return;
                };
                let Some(prop) = props.get_mut(prop) else {
                    return;
                };
                let matches_kind = matches!(
                    (&prop.value, kind),
                    (PropValue::Image(_), AssetKind::Image)
                        | (PropValue::Font(_), AssetKind::Font)
                        | (PropValue::Sound(_), AssetKind::Sound)
                        | (PropValue::Mesh(_), AssetKind::Mesh)
                        | (PropValue::Shader(_), AssetKind::Shader)
                        | (PropValue::Animation(_), AssetKind::Animation)
                );
                if !matches_kind {
                    return;
                }
                match &mut prop.value {
                    PropValue::Image(value)
                    | PropValue::Font(value)
                    | PropValue::Sound(value)
                    | PropValue::Mesh(value)
                    | PropValue::Shader(value)
                    | PropValue::Animation(value) => {
                        *value = path.clone();
                    }
                    _ => return,
                }
                prop.label.clone()
            }
            AssetTarget::ScriptVar {
                entity,
                component,
                var,
                path: var_path,
            } => {
                let Some(entity) = self.scene.entity_mut(entity) else {
                    return;
                };
                let Some(Component::Script { variables, .. }) =
                    entity.components.get_mut(component)
                else {
                    return;
                };
                let Some(variable) = variables.get_mut(var) else {
                    return;
                };
                let Some(value) = var_value_at_path_mut(&mut variable.value, &var_path) else {
                    return;
                };
                let matches_kind = matches!(
                    (&*value, kind),
                    (VarValue::Image(_), AssetKind::Image)
                        | (VarValue::Audio(_), AssetKind::Sound)
                        | (VarValue::Shader(_), AssetKind::Shader)
                        | (VarValue::Animation(_), AssetKind::Animation)
                );
                if !matches_kind {
                    return;
                }
                match value {
                    VarValue::Image(value)
                    | VarValue::Audio(value)
                    | VarValue::Shader(value)
                    | VarValue::Animation(value) => *value = path.clone(),
                    _ => return,
                }
                humanize_identifier(&variable.name)
            }
            AssetTarget::AttachedValue {
                entity,
                value,
                path: value_path,
            } => {
                let Some(entity) = self.scene.entity_mut(entity) else {
                    return;
                };
                let Some(attached) = entity.values.get_mut(value) else {
                    return;
                };
                let Some(value) = var_value_at_path_mut(&mut attached.value, &value_path) else {
                    return;
                };
                let matches_kind = matches!(
                    (&*value, kind),
                    (VarValue::Image(_), AssetKind::Image)
                        | (VarValue::Audio(_), AssetKind::Sound)
                        | (VarValue::Shader(_), AssetKind::Shader)
                        | (VarValue::Animation(_), AssetKind::Animation)
                );
                if !matches_kind {
                    return;
                }
                match value {
                    VarValue::Image(value)
                    | VarValue::Audio(value)
                    | VarValue::Shader(value)
                    | VarValue::Animation(value) => *value = path.clone(),
                    _ => return,
                }
                attached.name.clone()
            }
        };
        self.mark_dirty();
        self.status = if path.is_empty() {
            format!("Cleared {label}")
        } else {
            format!("Selected {path}")
        };
    }

    fn draw_error(
        &mut self,
        ui: &mut Ui,
        message: String,
        mut copied: bool,
        w: f32,
        h: f32,
        interactive: bool,
    ) {
        ui.painter
            .fill_rect(Rect::new(0.0, 0.0, w, h), [0, 0, 0, 140]);
        let width = (w * 0.7).clamp(420.0, 760.0);
        let height = (h * 0.6).clamp(220.0, 460.0);
        let px = (w - width) / 2.0;
        let py = (h - height) / 2.0;
        let rect = Rect::new(px, py, width, height);
        ui.painter
            .fill_round_rect(rect, 6.0, self.config.theme.panel);
        ui.painter
            .stroke_round_rect(rect, 6.0, self.config.theme.danger);
        ui.icon(
            px + 18.0,
            py + 18.0,
            icon::DELETE,
            16.0,
            self.config.theme.danger,
        );
        ui.painter.text(
            px + 32.0,
            py + 11.0,
            "Runtime Error",
            16.0,
            self.config.theme.danger,
        );

        // Message body, wrapped to the panel and clipped.
        let body = Rect::new(px + 12.0, py + 36.0, width - 24.0, height - 84.0);
        ui.painter.fill_rect(body, self.config.theme.field);
        let prev = ui.painter.push_clip(body);
        let mut ty = body.y + 6.0;
        let max_w = body.w - 12.0;
        for raw_line in message.lines() {
            for wrapped in wrap_line(&ui.painter, raw_line, 13.0, max_w) {
                if ty > body.bottom() - 14.0 {
                    break;
                }
                ui.painter
                    .text(body.x + 6.0, ty, &wrapped, 13.0, self.config.theme.text);
                ty += 16.0;
            }
        }
        ui.painter.set_clip_raw(prev);

        let copy = Rect::new(px + width - 200.0, py + height - 36.0, 90.0, 26.0);
        let close = Rect::new(px + width - 104.0, py + height - 36.0, 90.0, 26.0);
        let copy_label = if copied { "Copied!" } else { "Copy" };
        let do_copy = interactive && ui.icon_button(copy, icon::CONTENT_COPY, copy_label);
        let do_close = interactive && (ui.button(close, "Close") || ui.input.escape);
        if do_copy {
            copied = copy_to_clipboard(&message);
            if copied {
                self.status = "Copied error to clipboard".to_string();
            } else {
                // Fall back to a file the user can open.
                let path = self.project_root.join("last_error.txt");
                let _ = std::fs::write(&path, &message);
                self.status = format!("Clipboard unavailable; wrote {}", path.display());
            }
        }
        if !do_close {
            self.popup = Some(Popup::Error { message, copied });
        }
    }

    fn draw_build_target_picker(&mut self, ui: &mut Ui, w: f32, h: f32, interactive: bool) {
        ui.painter
            .fill_rect(Rect::new(0.0, 0.0, w, h), [0, 0, 0, 120]);
        let mut targets = vec![
            (BuildTarget::Desktop, icon::VIEW_IN_AR),
            (BuildTarget::Webasm, icon::CODE),
            (BuildTarget::Android, icon::PHONE_ANDROID),
            (BuildTarget::Ios, icon::PHONE_ANDROID),
        ];
        if cfg!(target_os = "linux") {
            targets.insert(1, (BuildTarget::WindowsDesktop, icon::VIEW_IN_AR));
        } else if cfg!(windows) {
            targets.insert(1, (BuildTarget::LinuxDesktop, icon::VIEW_IN_AR));
        } else {
            targets.insert(1, (BuildTarget::WindowsDesktop, icon::VIEW_IN_AR));
            targets.insert(2, (BuildTarget::LinuxDesktop, icon::VIEW_IN_AR));
        }
        let width = (w - 32.0).min(430.0).max(340.0);
        let height = (96.0 + targets.len() as f32 * 34.0)
            .min(h - 24.0)
            .max(210.0);
        let px = (w - width) * 0.5;
        let py = (h - height) * 0.5;
        let panel = Rect::new(px, py, width, height);
        ui.painter
            .fill_round_rect(panel, 6.0, self.config.theme.panel);
        ui.painter
            .stroke_round_rect(panel, 6.0, self.config.theme.accent);
        ui.icon(
            px + 18.0,
            py + 19.0,
            icon::DATA_OBJECT,
            17.0,
            self.config.theme.accent,
        );
        ui.painter.text(
            px + 38.0,
            py + 12.0,
            "Build Target",
            16.0,
            self.config.theme.text,
        );

        let mut y = py + 46.0;
        let mut chosen = None;
        for (target, glyph) in targets {
            let row = Rect::new(px + 16.0, y, width - 32.0, 28.0);
            if interactive && ui.icon_button(row, glyph, target.label()) {
                chosen = Some(target);
            }
            y += 34.0;
        }

        let cancel = Rect::new(panel.right() - 106.0, panel.bottom() - 36.0, 90.0, 26.0);
        let cancel_clicked = interactive && (ui.button(cancel, "Cancel") || ui.input.escape);
        if let Some(target) = chosen {
            self.build_project(target);
        } else if !cancel_clicked {
            self.popup = Some(Popup::BuildTarget);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_mobile_emulator(
        &mut self,
        ui: &mut Ui,
        mut enabled: bool,
        mut orientation: String,
        mut wifi: bool,
        mut cellular: bool,
        mut low_power: bool,
        w: f32,
        h: f32,
        interactive: bool,
    ) {
        ui.painter
            .fill_rect(Rect::new(0.0, 0.0, w, h), [0, 0, 0, 120]);
        let width = (w - 32.0).min(460.0).max(360.0);
        let height = 282.0_f32.min(h - 24.0).max(250.0);
        let px = (w - width) * 0.5;
        let py = (h - height) * 0.5;
        let panel = Rect::new(px, py, width, height);
        ui.painter
            .fill_round_rect(panel, 6.0, self.config.theme.panel);
        ui.painter
            .stroke_round_rect(panel, 6.0, self.config.theme.accent);
        ui.icon(
            px + 18.0,
            py + 19.0,
            icon::PHONE_ANDROID,
            17.0,
            self.config.theme.accent,
        );
        ui.painter.text(
            px + 38.0,
            py + 12.0,
            "Mobile Emulator",
            16.0,
            self.config.theme.text,
        );

        let mut y = py + 48.0;
        self.inspector_label(ui, px + 18.0, y + 4.0, "Enabled", 170.0);
        if let Some(next) = ui.checkbox(Rect::new(px + 190.0, y, FIELD_H, FIELD_H), enabled) {
            enabled = next;
        }
        y += FIELD_H + 10.0;

        self.inspector_label(ui, px + 18.0, y + 4.0, "Orientation", 170.0);
        ui.icon(
            px + 164.0,
            y + 4.0,
            icon::SCREEN_ROTATION,
            15.0,
            self.config.theme.text_dim,
        );
        let portrait = Rect::new(px + 190.0, y, 88.0, FIELD_H);
        let landscape = Rect::new(px + 284.0, y, 104.0, FIELD_H);
        if interactive
            && ui.button_colored(
                portrait,
                "Portrait",
                if orientation == "portrait" {
                    self.config.theme.button_active
                } else {
                    self.config.theme.button
                },
                self.config.theme.text,
            )
        {
            orientation = "portrait".to_string();
        }
        if interactive
            && ui.button_colored(
                landscape,
                "Landscape",
                if orientation == "landscape" {
                    self.config.theme.button_active
                } else {
                    self.config.theme.button
                },
                self.config.theme.text,
            )
        {
            orientation = "landscape".to_string();
        }
        y += FIELD_H + 10.0;

        let (size_w, size_h) = if orientation == "landscape" {
            (
                crate::mobile_emulation::DEFAULT_HEIGHT,
                crate::mobile_emulation::DEFAULT_WIDTH,
            )
        } else {
            (
                crate::mobile_emulation::DEFAULT_WIDTH,
                crate::mobile_emulation::DEFAULT_HEIGHT,
            )
        };
        self.inspector_label(ui, px + 18.0, y + 4.0, "Locked Size", 170.0);
        ui.painter.text(
            px + 190.0,
            y + 4.0,
            &format!("{size_w} x {size_h}"),
            14.0,
            self.config.theme.text,
        );
        y += FIELD_H + 10.0;

        for (label, value) in [
            ("Wi-Fi", &mut wifi),
            ("Cellular", &mut cellular),
            ("Low Power Mode", &mut low_power),
        ] {
            self.inspector_label(ui, px + 18.0, y + 4.0, label, 170.0);
            if let Some(next) = ui.checkbox(Rect::new(px + 190.0, y, FIELD_H, FIELD_H), *value) {
                *value = next;
            }
            y += FIELD_H + 8.0;
        }

        let save = Rect::new(panel.right() - 204.0, panel.bottom() - 36.0, 92.0, 26.0);
        let cancel = Rect::new(panel.right() - 104.0, panel.bottom() - 36.0, 90.0, 26.0);
        let save_clicked = interactive
            && ui.button_colored(
                save,
                "Save",
                self.config.theme.button,
                self.config.theme.text,
            );
        let cancel_clicked = interactive && (ui.button(cancel, "Cancel") || ui.input.escape);
        if save_clicked {
            self.config.settings.mobile_emulator = enabled;
            self.config.settings.mobile_orientation = if orientation == "landscape" {
                "landscape"
            } else {
                "portrait"
            }
            .to_string();
            self.config.settings.mobile_wifi = wifi;
            self.config.settings.mobile_cellular = cellular;
            self.config.settings.mobile_low_power = low_power;
            self.dirty = true;
            self.status = if enabled {
                format!("Mobile emulator enabled ({} x {})", size_w, size_h)
            } else {
                "Mobile emulator disabled".to_string()
            };
        } else if !cancel_clicked {
            self.popup = Some(Popup::MobileEmulator {
                enabled,
                orientation,
                wifi,
                cellular,
                low_power,
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_menu(
        &mut self,
        ui: &mut Ui,
        x: f32,
        y: f32,
        items: Vec<MenuItem>,
        w: f32,
        h: f32,
        interactive: bool,
    ) {
        let item_h = 26.0;
        let width = items
            .iter()
            .map(|item| ui.painter.text_width(&item.label, 14.0) + 42.0)
            .fold(200.0_f32, f32::max)
            .min(340.0);
        let height = items.len() as f32 * item_h + 8.0;
        let px = x.min(w - width - 4.0).max(2.0);
        let py = y.min(h - height - 4.0).max(2.0);
        let rect = Rect::new(px, py, width, height);
        ui.painter
            .fill_round_rect(rect, 5.0, self.config.theme.panel);
        ui.painter
            .stroke_round_rect(rect, 5.0, self.config.theme.border);

        let mut chosen: Option<Action> = None;
        let mut iy = py + 4.0;
        for item in &items {
            let r = Rect::new(px + 2.0, iy, width - 4.0, item_h);
            if ui.menu_item(r, item.glyph, &item.label, item.danger) {
                chosen = Some(item.action.clone());
            }
            iy += item_h;
        }

        // On the opening frame (`!interactive`) ignore clicks entirely so the
        // click that opened the menu does not immediately close it.
        let clicked_outside = interactive
            && ui.input.mouse_pressed
            && !rect.contains(ui.input.mouse_x, ui.input.mouse_y);
        if let Some(action) = chosen.filter(|_| interactive) {
            self.perform(action);
        } else if !clicked_outside {
            self.popup = Some(Popup::Menu { x, y, items });
        }
    }

    /// The searchable "Add Component" picker: a text field that filters the list
    /// live, auto-focused on open, where Enter adds the top match. Custom scripts
    /// that call `IComponentPicker(Behaviour)` appear alongside core components.
    #[allow(clippy::too_many_arguments)]
    fn draw_component_picker(
        &mut self,
        ui: &mut Ui,
        x: f32,
        y: f32,
        mut query: String,
        mut scroll: f32,
        entries: Vec<ComponentPickerEntry>,
        w: f32,
        h: f32,
        interactive: bool,
    ) {
        let width = 260.0_f32;
        let row_h = 26.0;
        let field_h = FIELD_H + 2.0;
        let pad = 6.0;

        let filter = |query: &str| -> Vec<ComponentPickerEntry> {
            let q = query.trim().to_lowercase();
            entries
                .iter()
                .filter(|entry| q.is_empty() || entry.label.to_lowercase().contains(&q))
                .cloned()
                .collect()
        };

        let visible_rows = filter(&query).len().clamp(1, 10);
        let list_h = visible_rows as f32 * row_h;
        let height = pad + field_h + 6.0 + list_h + pad;
        let px = x.min(w - width - 4.0).max(2.0);
        let py = y.min(h - height - 4.0).max(2.0);
        let rect = Rect::new(px, py, width, height);
        ui.painter
            .fill_round_rect(rect, 5.0, self.config.theme.panel);
        ui.painter
            .stroke_round_rect(rect, 5.0, self.config.theme.border);

        // Enter is captured before drawing the field, which clears focus on Enter.
        let submit = interactive && ui.input.enter && ui.has_focus();

        let field = Rect::new(px + pad, py + pad, width - pad * 2.0, field_h);
        let search = ui.text_field("component_picker_search", field, &query);
        if search.changed {
            query = search.text;
            scroll = 0.0;
        }

        let filtered = filter(&query);
        let list = Rect::new(
            px + pad,
            py + pad + field_h + 6.0,
            width - pad * 2.0,
            list_h,
        );
        let content_h = filtered.len() as f32 * row_h;
        let max_scroll = (content_h - list.h).max(0.0);
        if interactive
            && list.contains(ui.input.mouse_x, ui.input.mouse_y)
            && ui.input.scroll != 0.0
        {
            scroll = (scroll - ui.input.scroll * row_h * 2.0).clamp(0.0, max_scroll);
            ui.wants_redraw = true;
        } else {
            scroll = scroll.clamp(0.0, max_scroll);
        }

        let mut chosen: Option<Action> = None;
        let prev_clip = ui.painter.push_clip(list);
        ui.set_input_clip(list);
        let mut ry = list.y - scroll;
        for entry in &filtered {
            let r = Rect::new(list.x, ry, list.w, row_h);
            if ui.menu_item(r, entry.glyph, &entry.label, false) && interactive {
                chosen = Some(entry.action.clone());
            }
            ry += row_h;
        }
        ui.reset_input_clip();
        ui.painter.set_clip_raw(prev_clip);

        if filtered.is_empty() {
            ui.painter.text_clipped(
                list.x + 8.0,
                list.y + 6.0,
                "No matching components",
                13.0,
                self.config.theme.text_dim,
                (list.w - 16.0).max(0.0),
            );
        }

        // Enter adds the top match.
        if submit && chosen.is_none() {
            if let Some(top) = filtered.first() {
                chosen = Some(top.action.clone());
            }
        }

        let clicked_outside = interactive
            && ui.input.mouse_pressed
            && !rect.contains(ui.input.mouse_x, ui.input.mouse_y);
        if let Some(action) = chosen.filter(|_| interactive) {
            ui.clear_focus();
            self.perform(action);
        } else if clicked_outside {
            ui.clear_focus();
        } else {
            self.popup = Some(Popup::ComponentPicker {
                x,
                y,
                query,
                scroll,
                entries,
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_color_picker(
        &mut self,
        ui: &mut Ui,
        target: ColorTarget,
        x: f32,
        y: f32,
        rgba: [u8; 4],
        hue: f32,
        w: f32,
        h: f32,
        interactive: bool,
    ) {
        let response = self.draw_color_picker_panel(ui, x, y, rgba, hue, w, h, interactive);
        if response.changed {
            self.set_target_color(&target, response.rgba);
            self.mark_dirty();
        }
        if response.open {
            self.popup = Some(Popup::Color {
                target,
                x,
                y,
                rgba: response.rgba,
                hue: response.hue,
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_color_picker_panel(
        &mut self,
        ui: &mut Ui,
        x: f32,
        y: f32,
        mut rgba: [u8; 4],
        mut hue: f32,
        w: f32,
        h: f32,
        interactive: bool,
    ) -> ColorPickerPanelResponse {
        let hsv = self.config.layout.hsv_picker;
        let width = 244.0;
        let height = if hsv { 196.0 } else { 150.0 };
        let px = x.min(w - width - 4.0).max(2.0);
        let py = y.min(h - height - 4.0).max(2.0);
        let rect = Rect::new(px, py, width, height);
        ui.painter
            .fill_round_rect(rect, 5.0, self.config.theme.panel);
        ui.painter
            .stroke_round_rect(rect, 5.0, self.config.theme.border);

        // Mode toggle (HSV square vs RGBA sliders), persisted.
        let toggle = Rect::new(px + width - 56.0, py + 8.0, 48.0, 18.0);
        if interactive && ui.button(toggle, if hsv { "RGB" } else { "HSV" }) {
            self.config.layout.hsv_picker = !hsv;
            self.dirty = true;
        }
        ui.painter.text(
            px + 10.0,
            py + 9.0,
            "Color",
            14.0,
            self.config.theme.text_dim,
        );

        let mut changed = false;
        if hsv {
            // Saturation/Value square for the current hue.
            let sq = Rect::new(px + 10.0, py + 32.0, 150.0, 120.0);
            for yy in 0..(sq.h as i32) {
                for xx in 0..(sq.w as i32) {
                    let s = xx as f32 / sq.w;
                    let v = 1.0 - yy as f32 / sq.h;
                    let c = hsv_to_rgb(hue, s, v);
                    ui.painter
                        .pixel(sq.x + xx as f32, sq.y + yy as f32, [c[0], c[1], c[2], 255]);
                }
            }
            ui.painter.stroke_rect(sq, self.config.theme.border);
            // Hue strip.
            let strip = Rect::new(px + 170.0, py + 32.0, 18.0, 120.0);
            for yy in 0..(strip.h as i32) {
                let c = hsv_to_rgb(yy as f32 / strip.h * 360.0, 1.0, 1.0);
                ui.painter.fill_rect(
                    Rect::new(strip.x, strip.y + yy as f32, strip.w, 1.0),
                    [c[0], c[1], c[2], 255],
                );
            }
            ui.painter.stroke_rect(strip, self.config.theme.border);

            // Interaction.
            let (_, mut s, mut v) = rgb_to_hsv(rgba);
            if interactive && ui.pointer_drag("color-picker-sv", sq) {
                s = ((ui.input.mouse_x - sq.x) / sq.w).clamp(0.0, 1.0);
                v = (1.0 - (ui.input.mouse_y - sq.y) / sq.h).clamp(0.0, 1.0);
                let c = hsv_to_rgb(hue, s, v);
                rgba = [c[0], c[1], c[2], rgba[3]];
                changed = true;
                ui.wants_redraw = true;
            }
            if interactive && ui.pointer_drag("color-picker-hue", strip) {
                hue = ((ui.input.mouse_y - strip.y) / strip.h * 360.0).clamp(0.0, 359.999);
                let c = hsv_to_rgb(hue, s, v);
                rgba = [c[0], c[1], c[2], rgba[3]];
                changed = true;
                ui.wants_redraw = true;
            }
            // SV cursor + hue marker.
            let cur = Rect::new(
                sq.x + s * sq.w - 4.0,
                sq.y + (1.0 - v) * sq.h - 4.0,
                8.0,
                8.0,
            );
            ui.painter.stroke_round_rect(cur, 4.0, [255, 255, 255, 255]);
            ui.painter.fill_rect(
                Rect::new(
                    strip.x - 2.0,
                    strip.y + hue / 360.0 * strip.h - 1.0,
                    strip.w + 4.0,
                    2.0,
                ),
                [255, 255, 255, 255],
            );

            // Preview + alpha slider + hex.
            ui.painter.fill_round_rect(
                Rect::new(px + 196.0, py + 32.0, 38.0, 26.0),
                4.0,
                [rgba[0], rgba[1], rgba[2], 255],
            );
            ui.painter.stroke_round_rect(
                Rect::new(px + 196.0, py + 32.0, 38.0, 26.0),
                4.0,
                self.config.theme.border,
            );
            ui.label(px + 10.0, py + 160.0, "A", self.config.theme.text);
            if let Some(a) = interactive
                .then(|| {
                    ui.slider(
                        Rect::new(px + 26.0, py + 158.0, 130.0, 18.0),
                        rgba[3] as f32,
                        0.0,
                        255.0,
                    )
                })
                .flatten()
            {
                rgba[3] = a.round() as u8;
                changed = true;
            }
            let hexr = ui.text_field(
                "cp_hex",
                Rect::new(px + 164.0, py + 158.0, 70.0, 18.0),
                &format!("{:02X}{:02X}{:02X}", rgba[0], rgba[1], rgba[2]),
            );
            if hexr.changed {
                if let Some(c) = parse_hex(&hexr.text) {
                    rgba = [c[0], c[1], c[2], rgba[3]];
                    hue = rgb_to_hsv(rgba).0;
                    changed = true;
                }
            }
        } else {
            ui.painter.fill_round_rect(
                Rect::new(px + 10.0, py + 32.0, 40.0, 30.0),
                4.0,
                [rgba[0], rgba[1], rgba[2], 255],
            );
            ui.painter.stroke_round_rect(
                Rect::new(px + 10.0, py + 32.0, 40.0, 30.0),
                4.0,
                self.config.theme.border,
            );
            let labels = ["R", "G", "B", "A"];
            for i in 0..4 {
                let ry = py + 32.0 + i as f32 * 26.0;
                ui.label(px + 60.0, ry + 2.0, labels[i], self.config.theme.text);
                if let Some(v) = interactive
                    .then(|| {
                        ui.slider(
                            Rect::new(px + 78.0, ry, 90.0, 18.0),
                            rgba[i] as f32,
                            0.0,
                            255.0,
                        )
                    })
                    .flatten()
                {
                    rgba[i] = v.round() as u8;
                    changed = true;
                }
                let r = ui.text_field(
                    &format!("cp_{i}"),
                    Rect::new(px + 174.0, ry, 40.0, 18.0),
                    &rgba[i].to_string(),
                );
                if r.changed {
                    if let Ok(v) = r.text.trim().parse::<i32>() {
                        rgba[i] = v.clamp(0, 255) as u8;
                        changed = true;
                    }
                }
            }
            hue = rgb_to_hsv(rgba).0;
        }

        let clicked_outside = interactive
            && ui.input.mouse_pressed
            && !rect.contains(ui.input.mouse_x, ui.input.mouse_y);
        ColorPickerPanelResponse {
            rgba,
            hue,
            changed,
            open: !clicked_outside,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_confirm(
        &mut self,
        ui: &mut Ui,
        message: String,
        action: Pending,
        w: f32,
        h: f32,
        interactive: bool,
    ) {
        // Dim background.
        ui.painter
            .fill_rect(Rect::new(0.0, 0.0, w, h), [0, 0, 0, 120]);
        let width = 360.0;
        let height = 120.0;
        let px = (w - width) / 2.0;
        let py = (h - height) / 2.0;
        let rect = Rect::new(px, py, width, height);
        ui.painter
            .fill_round_rect(rect, 6.0, self.config.theme.panel);
        ui.painter
            .stroke_round_rect(rect, 6.0, self.config.theme.accent);
        ui.painter.text_wrapped(
            Rect::new(px + 16.0, py + 14.0, width - 32.0, 52.0),
            &message,
            15.0,
            18.0,
            self.config.theme.text,
        );

        let yes = Rect::new(px + width - 200.0, py + height - 36.0, 90.0, 26.0);
        let no = Rect::new(px + width - 104.0, py + height - 36.0, 90.0, 26.0);
        let confirm = interactive
            && ui.button_colored(yes, "Yes", self.config.theme.danger, [255, 255, 255, 255]);
        let cancel = interactive && ui.button(no, "Cancel");
        if confirm {
            self.perform_pending(action);
        } else if !cancel {
            self.popup = Some(Popup::Confirm { message, action });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_prompt(
        &mut self,
        ui: &mut Ui,
        title: String,
        action: Pending,
        w: f32,
        h: f32,
        interactive: bool,
    ) {
        ui.painter
            .fill_rect(Rect::new(0.0, 0.0, w, h), [0, 0, 0, 120]);
        let width = 340.0;
        let height = 120.0;
        let px = (w - width) / 2.0;
        let py = (h - height) / 2.0;
        let rect = Rect::new(px, py, width, height);
        ui.painter
            .fill_round_rect(rect, 6.0, self.config.theme.panel);
        ui.painter
            .stroke_round_rect(rect, 6.0, self.config.theme.accent);
        ui.painter.text_clipped(
            px + 16.0,
            py + 16.0,
            &title,
            15.0,
            self.config.theme.text,
            width - 32.0,
        );

        let field = Rect::new(px + 16.0, py + 44.0, width - 32.0, 26.0);
        let _ = ui.text_field("prompt_field", field, "");
        let value = ui.last_edit().to_string();

        let ok = Rect::new(px + width - 200.0, py + height - 34.0, 90.0, 24.0);
        let cancel = Rect::new(px + width - 104.0, py + height - 34.0, 90.0, 24.0);
        let submit = interactive
            && (ui.button_colored(ok, "OK", self.config.theme.button, self.config.theme.text)
                || ui.input.enter);
        let cancelled = interactive && ui.button(cancel, "Cancel");
        if submit {
            self.focus = None;
            self.edit_buffer.clear();
            self.edit_cursor = 0;
            self.edit_selection_anchor = None;
            self.perform_pending_with(action, value);
        } else if cancelled {
            self.focus = None;
            self.edit_buffer.clear();
            self.edit_cursor = 0;
            self.edit_selection_anchor = None;
        } else {
            self.popup = Some(Popup::Prompt { title, action });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_editor_settings(
        &mut self,
        ui: &mut Ui,
        mut theme_name: String,
        mut custom_theme: Theme,
        original_theme: Theme,
        mut font_path: String,
        mut show_tooltips: bool,
        mut show_window_bounds: bool,
        mut show_transform_hud: bool,
        mut preview_lighting: bool,
        mut autosave_before_run: bool,
        mut autosave_before_build: bool,
        mut viewport_camera_sensitivity: f32,
        mut viewport_camera_speed: f32,
        mut viewport_camera_fov: f32,
        mut viewport_invert_mouse_look: bool,
        w: f32,
        h: f32,
        interactive: bool,
    ) {
        ui.painter
            .fill_rect(Rect::new(0.0, 0.0, w, h), [0, 0, 0, 120]);
        let width = (w - 24.0).min(640.0).max(400.0);
        let height = 500.0_f32.min(h - 24.0).max(360.0);
        let px = (w - width) * 0.5;
        let py = (h - height) * 0.5;
        let panel = Rect::new(px, py, width, height);
        ui.painter
            .fill_round_rect(panel, 6.0, self.config.theme.panel);
        ui.painter
            .stroke_round_rect(panel, 6.0, self.config.theme.accent);
        ui.icon(
            px + 18.0,
            py + 20.0,
            icon::TUNE,
            17.0,
            self.config.theme.accent,
        );
        ui.painter.text(
            px + 34.0,
            py + 12.0,
            "Editor Settings",
            16.0,
            self.config.theme.text,
        );

        let mut y = py + 42.0;
        ui.painter
            .text(px + 16.0, y, "APPEARANCE", 11.0, self.config.theme.text_dim);
        y += 17.0;
        let preset_width = (width - 32.0) / theme_presets().len() as f32;
        let previous_theme_name = theme_name.clone();
        for (name, label) in theme_presets() {
            let index = theme_presets()
                .iter()
                .position(|(candidate, _)| candidate == name)
                .unwrap_or(0);
            let row = Rect::new(
                px + 16.0 + index as f32 * preset_width,
                y,
                preset_width - 3.0,
                24.0,
            );
            if interactive && ui.list_row(row, label, theme_name == *name, 0.0) {
                if *name == "custom" && previous_theme_name != "custom" {
                    custom_theme = self.config.theme.clone();
                }
                theme_name = (*name).to_string();
                self.config.theme = if *name == "custom" {
                    custom_theme.clone()
                } else {
                    theme_preset(name).unwrap_or_default()
                };
            }
        }
        y += 36.0;

        self.inspector_label(ui, px + 16.0, y + 4.0, "Editor Font", 90.0);
        let browse_w = 76.0;
        let reset_w = FIELD_H;
        let font_result = ui.text_field(
            "editor_font_path",
            Rect::new(
                px + 106.0,
                y,
                width - 122.0 - browse_w - reset_w - 8.0,
                FIELD_H,
            ),
            &font_path,
        );
        if font_result.changed {
            font_path = font_result.text;
        }
        let browse = Rect::new(
            panel.right() - 16.0 - browse_w - reset_w - 4.0,
            y,
            browse_w,
            FIELD_H,
        );
        if interactive && ui.button(browse, "Browse…") {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("Font files", &["ttf", "otf"])
                .pick_file()
            {
                font_path = path.to_string_lossy().into_owned();
                ui.clear_focus();
            }
        }
        ui.tooltip(browse, "Choose a TrueType or OpenType editor font");
        let reset_font = Rect::new(panel.right() - 16.0 - reset_w, y, reset_w, FIELD_H);
        if interactive
            && ui.icon_toggle(
                reset_font,
                icon::RESTART_ALT,
                false,
                self.config.theme.text_dim,
            )
        {
            font_path.clear();
            ui.clear_focus();
        }
        ui.tooltip(reset_font, "Use the bundled editor font");
        y += FIELD_H + 12.0;

        if theme_name == "custom" {
            ui.painter.text(
                px + 16.0,
                y,
                "CUSTOM PALETTE",
                11.0,
                self.config.theme.text_dim,
            );
            y += 17.0;
            let gap = 12.0;
            let column_width = (width - 32.0 - gap) * 0.5;
            let left = px + 16.0;
            let right = left + column_width + gap;
            let mut changed = false;
            changed |= Self::theme_color_field(
                ui,
                "theme_panel",
                "Surface",
                &mut custom_theme.panel,
                Rect::new(left, y, column_width, FIELD_H),
            );
            changed |= Self::theme_color_field(
                ui,
                "theme_raised",
                "Raised",
                &mut custom_theme.panel_alt,
                Rect::new(right, y, column_width, FIELD_H),
            );
            y += FIELD_H + 3.0;
            changed |= Self::theme_color_field(
                ui,
                "theme_toolbar",
                "Toolbar",
                &mut custom_theme.toolbar,
                Rect::new(left, y, column_width, FIELD_H),
            );
            changed |= Self::theme_color_field(
                ui,
                "theme_viewport",
                "Viewport",
                &mut custom_theme.viewport_bg,
                Rect::new(right, y, column_width, FIELD_H),
            );
            y += FIELD_H + 3.0;
            changed |= Self::theme_color_field(
                ui,
                "theme_text",
                "Text",
                &mut custom_theme.text,
                Rect::new(left, y, column_width, FIELD_H),
            );
            changed |= Self::theme_color_field(
                ui,
                "theme_muted",
                "Muted",
                &mut custom_theme.text_dim,
                Rect::new(right, y, column_width, FIELD_H),
            );
            y += FIELD_H + 3.0;
            changed |= Self::theme_color_field(
                ui,
                "theme_accent",
                "Accent",
                &mut custom_theme.accent,
                Rect::new(left, y, column_width, FIELD_H),
            );
            changed |= Self::theme_color_field(
                ui,
                "theme_danger",
                "Danger",
                &mut custom_theme.danger,
                Rect::new(right, y, column_width, FIELD_H),
            );
            y += FIELD_H + 10.0;
            if changed {
                custom_theme.button = custom_theme.accent;
                custom_theme.splitter_hover = custom_theme.accent;
                self.config.theme = custom_theme.clone();
            }
        } else {
            let notice = Rect::new(px + 16.0, y, width - 32.0, 32.0);
            ui.painter
                .fill_round_rect(notice, 4.0, self.config.theme.panel_alt);
            ui.painter.text_clipped(
                notice.x + 10.0,
                notice.y + 8.0,
                "Choose Custom to edit and preview your own palette.",
                12.0,
                self.config.theme.text_dim,
                (notice.w - 20.0).max(0.0),
            );
            y += 42.0;
        }

        ui.painter
            .text(px + 16.0, y, "WORKFLOW", 11.0, self.config.theme.text_dim);
        y += 17.0;
        let preference_width = (width - 32.0) * 0.5;
        for (index, (label, value)) in [
            ("Show tooltips", &mut show_tooltips),
            ("Show default window bounds", &mut show_window_bounds),
            ("Show Scene transform HUD", &mut show_transform_hud),
            ("Preview scene lighting", &mut preview_lighting),
            ("Autosave before Run", &mut autosave_before_run),
            ("Autosave before Build", &mut autosave_before_build),
        ]
        .into_iter()
        .enumerate()
        {
            let column = index % 2;
            let row = index / 2;
            let item_x = px + 16.0 + column as f32 * preference_width;
            let item_y = y + row as f32 * 25.0;
            ui.painter.text_clipped(
                item_x + 28.0,
                item_y + 4.0,
                label,
                12.0,
                self.config.theme.text,
                (preference_width - 36.0).max(0.0),
            );
            if let Some(next) = ui.checkbox(Rect::new(item_x, item_y, FIELD_H, FIELD_H), *value) {
                *value = next;
            }
        }

        y += 82.0;
        ui.painter.text(
            px + 16.0,
            y,
            "VIEWPORT CAMERA",
            11.0,
            self.config.theme.text_dim,
        );
        y += 17.0;
        for (index, (label, value, min, max, suffix)) in [
            (
                "Sensitivity",
                &mut viewport_camera_sensitivity,
                0.05,
                8.0,
                "x",
            ),
            (
                "Move speed",
                &mut viewport_camera_speed,
                0.1,
                1_000.0,
                " u/s",
            ),
            ("Field of view", &mut viewport_camera_fov, 20.0, 140.0, "°"),
        ]
        .into_iter()
        .enumerate()
        {
            let row_y = y + index as f32 * 23.0;
            ui.painter.text_clipped(
                px + 16.0,
                row_y + 3.0,
                label,
                12.0,
                self.config.theme.text,
                104.0,
            );
            let slider = Rect::new(px + 120.0, row_y, (width - 214.0).max(80.0), 18.0);
            if interactive && let Some(next) = ui.slider(slider, *value, min, max) {
                *value = next;
            }
            ui.painter.text_clipped(
                panel.right() - 82.0,
                row_y + 3.0,
                &format!("{}{suffix}", format_num(*value)),
                12.0,
                self.config.theme.text_dim,
                66.0,
            );
        }

        let invert_y = y + 70.0;
        ui.painter.text_clipped(
            px + 44.0,
            invert_y + 4.0,
            "Invert vertical mouse look",
            12.0,
            self.config.theme.text,
            (width - 76.0).max(0.0),
        );
        if let Some(next) = ui.checkbox(
            Rect::new(px + 16.0, invert_y, FIELD_H, FIELD_H),
            viewport_invert_mouse_look,
        ) {
            viewport_invert_mouse_look = next;
        }

        let save = Rect::new(panel.right() - 204.0, panel.bottom() - 36.0, 92.0, 26.0);
        let cancel = Rect::new(panel.right() - 104.0, panel.bottom() - 36.0, 90.0, 26.0);
        let save_clicked = interactive
            && ui.button_colored(
                save,
                "Save",
                self.config.theme.button,
                self.config.theme.text,
            );
        let cancel_clicked = interactive && (ui.button(cancel, "Cancel") || ui.input.escape);
        if save_clicked {
            let next_font_path = font_path.trim().to_string();
            let font_result = if next_font_path.is_empty() {
                super::ui::load_fonts()
            } else {
                super::ui::load_fonts_from_path(Some(Path::new(&next_font_path)))
            };
            if let Err(error) = font_result {
                self.status = error;
                self.popup = Some(Popup::EditorSettings {
                    theme_name,
                    custom_theme,
                    original_theme,
                    font_path,
                    show_tooltips,
                    show_window_bounds,
                    show_transform_hud,
                    preview_lighting,
                    autosave_before_run,
                    autosave_before_build,
                    viewport_camera_sensitivity,
                    viewport_camera_speed,
                    viewport_camera_fov,
                    viewport_invert_mouse_look,
                });
                return;
            }
            let font_changed = self.config.settings.font_path != next_font_path;
            self.config.settings.theme_name = theme_name.clone();
            self.config.settings.font_path = next_font_path.clone();
            self.config.settings.show_tooltips = show_tooltips;
            self.config.settings.show_window_bounds = show_window_bounds;
            self.config.settings.show_transform_hud = show_transform_hud;
            self.config.settings.preview_lighting = preview_lighting;
            self.config.settings.autosave_before_run = autosave_before_run;
            self.config.settings.autosave_before_build = autosave_before_build;
            self.config.settings.viewport_camera_sensitivity =
                viewport_camera_sensitivity.clamp(0.05, 8.0);
            self.config.settings.viewport_camera_speed = viewport_camera_speed.clamp(0.1, 1_000.0);
            self.config.settings.viewport_camera_fov = viewport_camera_fov.clamp(20.0, 140.0);
            self.config.settings.viewport_invert_mouse_look = viewport_invert_mouse_look;
            self.config.custom_theme = custom_theme.clone();
            self.config.theme = if theme_name == "custom" {
                custom_theme
            } else {
                theme_preset(&theme_name).unwrap_or_default()
            };
            if font_changed {
                self.font_reload_request = Some(next_font_path);
            }
            self.dirty = true;
            self.status = format!("Applied editor appearance ({})", theme_label(&theme_name));
        } else if cancel_clicked {
            self.config.theme = original_theme;
        } else {
            self.popup = Some(Popup::EditorSettings {
                theme_name,
                custom_theme,
                original_theme,
                font_path,
                show_tooltips,
                show_window_bounds,
                show_transform_hud,
                preview_lighting,
                autosave_before_run,
                autosave_before_build,
                viewport_camera_sensitivity,
                viewport_camera_speed,
                viewport_camera_fov,
                viewport_invert_mouse_look,
            });
        }
    }

    fn theme_color_field(
        ui: &mut Ui,
        id: &str,
        label: &str,
        color: &mut [u8; 4],
        rect: Rect,
    ) -> bool {
        ui.painter.text_clipped(
            rect.x,
            rect.y + 4.0,
            label,
            12.0,
            ui.theme.text_dim,
            (rect.w - 110.0).max(0.0),
        );
        let swatch = Rect::new(rect.right() - 104.0, rect.y + 1.0, 22.0, rect.h - 2.0);
        ui.painter.fill_round_rect(swatch, 3.0, *color);
        ui.painter.stroke_round_rect(swatch, 3.0, ui.theme.border);
        let field = Rect::new(rect.right() - 78.0, rect.y, 78.0, rect.h);
        let result = ui.text_field(
            id,
            field,
            &format!("#{:02X}{:02X}{:02X}", color[0], color[1], color[2]),
        );
        if result.changed
            && let Some(next) = parse_hex(&result.text)
        {
            *color = [next[0], next[1], next[2], color[3]];
            return true;
        }
        false
    }

    fn draw_project_window_settings(
        &mut self,
        ui: &mut Ui,
        mut start_scene: String,
        mut width: String,
        mut height: String,
        mut fullscreen: bool,
        mut resizable: bool,
        w: f32,
        h: f32,
        interactive: bool,
    ) {
        ui.painter
            .fill_rect(Rect::new(0.0, 0.0, w, h), [0, 0, 0, 120]);
        let panel_w = 452.0;
        let panel_h = 252.0;
        let px = (w - panel_w) * 0.5;
        let py = (h - panel_h) * 0.5;
        let panel = Rect::new(px, py, panel_w, panel_h);
        ui.painter
            .fill_round_rect(panel, 6.0, self.config.theme.panel);
        ui.painter
            .stroke_round_rect(panel, 6.0, self.config.theme.accent);
        ui.icon(
            px + 20.0,
            py + 22.0,
            icon::TUNE,
            16.0,
            self.config.theme.accent,
        );
        ui.painter.text(
            px + 36.0,
            py + 14.0,
            "Project Settings",
            16.0,
            self.config.theme.text,
        );

        let mut y = py + 48.0;
        self.inspector_label(ui, px + 18.0, y + 4.0, "Start Scene", LABEL_W - 6.0);
        let current = Rect::new(panel.right() - 102.0, y, 78.0, FIELD_H);
        let field = Rect::new(px + LABEL_W, y, current.x - (px + LABEL_W) - 8.0, FIELD_H);
        let start_result = ui.text_field("project_start_scene", field, &start_scene);
        if start_result.changed {
            start_scene = start_result.text;
        }
        if interactive && ui.button(current, "Current") {
            start_scene = project_relative_path(&self.project_root, &self.scene_path);
        }
        y += FIELD_H + 12.0;
        self.inspector_label(ui, px + 18.0, y + 4.0, "Width", LABEL_W - 6.0);
        let width_result = ui.text_field(
            "project_window_width",
            Rect::new(px + LABEL_W, y, panel_w - LABEL_W - 24.0, FIELD_H),
            &width,
        );
        if width_result.changed {
            width = width_result.text;
        }
        y += FIELD_H + 8.0;
        self.inspector_label(ui, px + 18.0, y + 4.0, "Height", LABEL_W - 6.0);
        let height_result = ui.text_field(
            "project_window_height",
            Rect::new(px + LABEL_W, y, panel_w - LABEL_W - 24.0, FIELD_H),
            &height,
        );
        if height_result.changed {
            height = height_result.text;
        }
        y += FIELD_H + 8.0;
        self.inspector_label(ui, px + 18.0, y + 4.0, "Fullscreen", LABEL_W - 6.0);
        if let Some(next) = ui.checkbox(Rect::new(px + LABEL_W, y, FIELD_H, FIELD_H), fullscreen) {
            fullscreen = next;
        }
        y += FIELD_H + 8.0;
        self.inspector_label(ui, px + 18.0, y + 4.0, "Resizable", LABEL_W - 6.0);
        if let Some(next) = ui.checkbox(Rect::new(px + LABEL_W, y, FIELD_H, FIELD_H), resizable) {
            resizable = next;
        }

        let save = Rect::new(panel.right() - 204.0, panel.bottom() - 36.0, 92.0, 26.0);
        let cancel = Rect::new(panel.right() - 104.0, panel.bottom() - 36.0, 90.0, 26.0);
        let save_clicked = interactive
            && ui.button_colored(
                save,
                "Save",
                self.config.theme.button,
                self.config.theme.text,
            );
        let cancel_clicked = interactive && (ui.button(cancel, "Cancel") || ui.input.escape);
        if save_clicked {
            let parsed_w = width
                .trim()
                .parse::<f32>()
                .ok()
                .filter(|value| value.is_finite());
            let parsed_h = height
                .trim()
                .parse::<f32>()
                .ok()
                .filter(|value| value.is_finite());
            let parsed_start_scene =
                normalize_start_scene_setting(&self.project_root, &start_scene);
            match (parsed_start_scene, parsed_w, parsed_h) {
                (Ok(next_start_scene), Some(width), Some(height)) => {
                    self.project_window.start_scene = next_start_scene;
                    self.project_window.width = width.clamp(1.0, 16384.0);
                    self.project_window.height = height.clamp(1.0, 16384.0);
                    self.project_window.fullscreen = fullscreen;
                    self.project_window.resizable = resizable;
                    match save_project_window_settings(&self.project_root, &self.project_window) {
                        Ok(()) => {
                            self.status = "Saved project settings".to_string();
                            self.dirty = true;
                        }
                        Err(error) => {
                            self.status = format!("Project settings save failed: {error}")
                        }
                    }
                }
                (Err(error), _, _) => {
                    self.status = error;
                    self.popup = Some(Popup::ProjectWindow {
                        start_scene,
                        width,
                        height,
                        fullscreen,
                        resizable,
                    });
                }
                _ => {
                    self.status = "Window width and height must be numbers".to_string();
                    self.popup = Some(Popup::ProjectWindow {
                        start_scene,
                        width,
                        height,
                        fullscreen,
                        resizable,
                    });
                }
            }
        } else if !cancel_clicked {
            self.popup = Some(Popup::ProjectWindow {
                start_scene,
                width,
                height,
                fullscreen,
                resizable,
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_animation_editor(
        &mut self,
        ui: &mut Ui,
        path: PathBuf,
        mut clip: AnimationClipAsset,
        mut selected_track: usize,
        mut selected_key: usize,
        w: f32,
        h: f32,
        interactive: bool,
    ) {
        normalize_animation_clip(&mut clip);
        selected_track = selected_track.min(clip.tracks.len().saturating_sub(1));
        selected_key = selected_key.min(
            clip.tracks
                .get(selected_track)
                .map(|track| track.keys.len().saturating_sub(1))
                .unwrap_or(0),
        );

        ui.painter
            .fill_rect(Rect::new(0.0, 0.0, w, h), [0, 0, 0, 120]);
        let panel_w = (w - 32.0).min(780.0).max(520.0);
        let panel_h = (h - 32.0).min(500.0).max(360.0);
        let px = (w - panel_w) * 0.5;
        let py = (h - panel_h) * 0.5;
        let panel = Rect::new(px, py, panel_w, panel_h);
        ui.painter
            .fill_round_rect(panel, 6.0, self.config.theme.panel);
        ui.painter
            .stroke_round_rect(panel, 6.0, self.config.theme.accent);
        ui.icon(
            px + 20.0,
            py + 22.0,
            icon::PLAY,
            16.0,
            self.config.theme.accent,
        );
        let title = path
            .strip_prefix(&self.project_root)
            .unwrap_or(&path)
            .to_string_lossy();
        ui.painter.text_clipped(
            px + 36.0,
            py + 14.0,
            &format!("Animation  /{title}"),
            16.0,
            self.config.theme.text,
            panel_w - 60.0,
        );

        let top_y = py + 46.0;
        self.inspector_label(ui, px + 18.0, top_y + 4.0, "Duration", 70.0);
        let duration = ui.text_field(
            "anim_duration",
            Rect::new(px + 88.0, top_y, 72.0, FIELD_H),
            &format_num(clip.duration),
        );
        if duration.changed {
            if let Ok(value) = duration.text.trim().parse::<f32>() {
                clip.duration = value.max(0.001);
            }
        }
        self.inspector_label(ui, px + 178.0, top_y + 4.0, "Looping", 60.0);
        if let Some(next) =
            ui.checkbox(Rect::new(px + 238.0, top_y, FIELD_H, FIELD_H), clip.looping)
        {
            clip.looping = next;
        }

        let list = Rect::new(px + 16.0, py + 82.0, 210.0, panel_h - 132.0);
        ui.painter.fill_rect(list, self.config.theme.field);
        ui.painter.stroke_rect(list, self.config.theme.border);
        ui.painter.text(
            list.x + 8.0,
            list.y + 7.0,
            "Tracks",
            13.0,
            self.config.theme.text_dim,
        );
        let mut row_y = list.y + 28.0;
        for (track_index, track) in clip.tracks.iter().enumerate() {
            let row = Rect::new(list.x + 4.0, row_y, list.w - 8.0, 22.0);
            let selected = track_index == selected_track;
            if ui.list_row(row, &track.property, selected, 6.0) {
                selected_track = track_index;
                selected_key = 0;
            }
            row_y += 23.0;
        }
        let add_track = Rect::new(list.x + 8.0, list.bottom() - 30.0, 92.0, 24.0);
        if interactive && ui.icon_button(add_track, icon::ADD, "Track") {
            clip.tracks.push(AnimationTrackAsset::default());
            selected_track = clip.tracks.len() - 1;
            selected_key = 0;
        }
        let del_track = Rect::new(list.x + 106.0, list.bottom() - 30.0, 92.0, 24.0);
        if interactive && ui.icon_button(del_track, icon::DELETE, "Track") && clip.tracks.len() > 1
        {
            clip.tracks.remove(selected_track);
            selected_track = selected_track.min(clip.tracks.len() - 1);
            selected_key = 0;
        }

        let edit = Rect::new(
            list.right() + 14.0,
            list.y,
            panel.right() - list.right() - 30.0,
            list.h,
        );
        ui.painter.fill_rect(edit, self.config.theme.panel_alt);
        ui.painter.stroke_rect(edit, self.config.theme.border);

        if let Some(track) = clip.tracks.get_mut(selected_track) {
            let mut y = edit.y + 12.0;
            self.inspector_label(ui, edit.x + 12.0, y + 4.0, "Property", 80.0);
            let property = ui.text_field(
                "anim_property",
                Rect::new(edit.x + 92.0, y, edit.w - 104.0, FIELD_H),
                &track.property,
            );
            if property.changed && !property.text.trim().is_empty() {
                track.property = property.text;
            }
            y += FIELD_H + 8.0;
            self.inspector_label(ui, edit.x + 12.0, y + 4.0, "Interpolation", 80.0);
            let interp_button = Rect::new(edit.x + 92.0, y, 120.0, FIELD_H);
            if ui.button(interp_button, &track.interpolation) {
                track.interpolation = match track.interpolation.as_str() {
                    "linear" => "bezier",
                    "bezier" => "step",
                    _ => "linear",
                }
                .to_string();
            }
            y += FIELD_H + 12.0;

            let timeline = Rect::new(edit.x + 12.0, y, edit.w - 24.0, 58.0);
            ui.painter.fill_rect(timeline, self.config.theme.field);
            ui.painter.stroke_rect(timeline, self.config.theme.border);
            let duration = clip.duration.max(0.001);
            let mut clicked_key = None;
            for (key_index, key) in track.keys.iter().enumerate() {
                let x = timeline.x + timeline.w * (key.time / duration).clamp(0.0, 1.0);
                let color = if key_index == selected_key {
                    self.config.theme.accent
                } else {
                    self.config.theme.text
                };
                ui.painter.fill_rect(
                    Rect::new(x - 2.0, timeline.y + 8.0, 4.0, timeline.h - 16.0),
                    color,
                );
                if interactive
                    && ui.input.mouse_pressed
                    && (ui.input.mouse_x - x).abs() <= 7.0
                    && ui.input.mouse_y >= timeline.y
                    && ui.input.mouse_y <= timeline.bottom()
                {
                    clicked_key = Some(key_index);
                }
            }
            if let Some(index) = clicked_key {
                selected_key = index;
            }
            y += timeline.h + 10.0;

            let keys_list = Rect::new(edit.x + 12.0, y, 150.0, edit.bottom() - y - 44.0);
            ui.painter.fill_rect(keys_list, self.config.theme.field);
            ui.painter.stroke_rect(keys_list, self.config.theme.border);
            let mut ky = keys_list.y + 5.0;
            for (key_index, key) in track.keys.iter().enumerate() {
                let row = Rect::new(keys_list.x + 4.0, ky, keys_list.w - 8.0, 22.0);
                if ui.list_row(
                    row,
                    &format!("{}  {}", format_num(key.time), format_num(key.value)),
                    key_index == selected_key,
                    4.0,
                ) {
                    selected_key = key_index;
                }
                ky += 23.0;
            }
            let add_key = Rect::new(keys_list.x, keys_list.bottom() + 8.0, 70.0, 24.0);
            if interactive && ui.icon_button(add_key, icon::ADD, "Key") {
                let time = (clip.duration * 0.5).max(0.0);
                track.keys.push(AnimationKeyAsset::new(time, 0.0));
                track.keys.sort_by(|a, b| a.time.total_cmp(&b.time));
                selected_key = nearest_animation_key(&track.keys, time);
            }
            let del_key = Rect::new(keys_list.x + 76.0, keys_list.bottom() + 8.0, 74.0, 24.0);
            if interactive && ui.icon_button(del_key, icon::DELETE, "Key") && track.keys.len() > 1 {
                track.keys.remove(selected_key);
                selected_key = selected_key.min(track.keys.len() - 1);
            }

            let field_x = keys_list.right() + 16.0;
            let field_w = edit.right() - field_x - 12.0;
            if let Some(key) = track.keys.get_mut(selected_key) {
                let mut fy = y;
                for (label, id, value, min, max) in [
                    (
                        "Time",
                        "anim_key_time",
                        &mut key.time,
                        0.0,
                        clip.duration.max(0.001),
                    ),
                    ("Value", "anim_key_value", &mut key.value, -1.0e9, 1.0e9),
                ] {
                    self.inspector_label(ui, field_x, fy + 4.0, label, 70.0);
                    let result = ui.text_field(
                        id,
                        Rect::new(field_x + 72.0, fy, field_w - 72.0, FIELD_H),
                        &format_num(*value),
                    );
                    if result.changed {
                        if let Ok(next) = result.text.trim().parse::<f32>() {
                            *value = next.clamp(min, max);
                        }
                    }
                    fy += FIELD_H + 7.0;
                }
                if track.interpolation == "bezier" {
                    for (label, id, value) in [
                        ("Out X", "anim_out_x", &mut key.out_x),
                        ("Out Y", "anim_out_y", &mut key.out_y),
                        ("In X", "anim_in_x", &mut key.in_x),
                        ("In Y", "anim_in_y", &mut key.in_y),
                    ] {
                        self.inspector_label(ui, field_x, fy + 4.0, label, 70.0);
                        let result = ui.text_field(
                            id,
                            Rect::new(field_x + 72.0, fy, field_w - 72.0, FIELD_H),
                            &format_num(*value),
                        );
                        if result.changed {
                            if let Ok(next) = result.text.trim().parse::<f32>() {
                                *value = if label.ends_with('X') {
                                    next.clamp(0.0, 1.0)
                                } else {
                                    next
                                };
                            }
                        }
                        fy += FIELD_H + 7.0;
                    }
                }
            }
            track.keys.sort_by(|a, b| a.time.total_cmp(&b.time));
            selected_key = selected_key.min(track.keys.len().saturating_sub(1));
        }

        let save = Rect::new(panel.right() - 204.0, panel.bottom() - 36.0, 92.0, 26.0);
        let close = Rect::new(panel.right() - 104.0, panel.bottom() - 36.0, 90.0, 26.0);
        let save_clicked = interactive
            && ui.button_colored(
                save,
                "Save",
                self.config.theme.button,
                self.config.theme.text,
            );
        let close_clicked = interactive && (ui.button(close, "Close") || ui.input.escape);
        if save_clicked {
            normalize_animation_clip(&mut clip);
            match serde_json::to_string_pretty(&clip)
                .map_err(|error| error.to_string())
                .and_then(|json| std::fs::write(&path, json).map_err(|error| error.to_string()))
            {
                Ok(()) => self.status = format!("Saved {}", path.display()),
                Err(error) => self.status = format!("Animation save failed: {error}"),
            }
        }
        if !close_clicked {
            self.popup = Some(Popup::AnimationEditor {
                path,
                clip,
                selected_track,
                selected_key,
            });
        }
    }

    // ---- Actions -----------------------------------------------------------

    fn perform(&mut self, action: Action) {
        match action {
            Action::NewScene => self.new_scene(),
            Action::SaveScene => self.save(),
            Action::LoadScene => self.load_requested(),
            Action::ExportScene => {
                self.export_luau();
            }
            Action::RunScene => self.run_scene(),
            Action::AddComponent(id, name) => {
                if let Some(e) = self.scene.entity_mut(id) {
                    if name == "Script" {
                        e.components.push(Component::Script {
                            path: "scripts/Behavior".into(),
                            variables: Vec::new(),
                        });
                    } else {
                        e.components.push(Component::core(&name));
                    }
                    self.mark_dirty();
                    self.status = format!("Added {name}");
                }
            }
            Action::AddScriptComponent(id, path) => {
                let full = self.project_root.join(&path);
                self.add_script_component_from_path(id, &full);
            }
            Action::PasteComponent(id) => {
                if let Some(c) = self.component_clipboard.clone() {
                    if let Some(e) = self.scene.entity_mut(id) {
                        let label = c.label().to_string();
                        e.components.push(c);
                        self.mark_dirty();
                        self.status = format!("Pasted {label} component");
                    }
                }
            }
            Action::AddEntity(parent) => self.add_entity(parent),
            Action::AddEntityAt(x, y) => self.add_entity_at(None, x, y),
            Action::Rename(id) => {
                let cur = self
                    .scene
                    .entity(id)
                    .map(|e| e.name.clone())
                    .unwrap_or_default();
                self.open_prompt("Rename entity", Pending::RenameEntity(id), &cur);
            }
            Action::Duplicate(id) => self.duplicate_entity(id),
            Action::Copy(id) => self.copy_entity(id),
            Action::Paste => self.paste_entity(),
            Action::Delete(id) => {
                self.scene.remove_entity(id);
                self.selected_ids.remove(&id);
                if self.selected == Some(id) {
                    self.selected = None;
                    self.selected = self.selection_ids_ordered().into_iter().next();
                }
                self.mark_dirty();
            }
            Action::Unparent(id) => {
                if let Some(e) = self.scene.entity_mut(id) {
                    e.parent = None;
                }
                self.mark_dirty();
            }
            Action::ResetTransform(id) => {
                let kind = self.scene.kind;
                if let Some(e) = self.scene.entity_mut(id) {
                    reset_entity_transform(e, kind);
                }
                self.mark_dirty();
                self.status = "Reset transform".to_string();
            }
            Action::FrameSelected(id) => {
                self.select_only(id);
                self.frame_selected();
            }
            Action::ToggleActive(id) => {
                if let Some(e) = self.scene.entity_mut(id) {
                    e.enabled = !e.enabled;
                }
                self.mark_dirty();
            }
            Action::NewFolder => {
                self.open_prompt("New folder name", Pending::CreateFolder, "NewFolder")
            }
            Action::NewScript => {
                self.open_prompt("New script name", Pending::CreateScript, "script.luau")
            }
            Action::NewShader => {
                self.open_prompt("New shader name", Pending::CreateShader, "shader.frag")
            }
            Action::NewAnimation => self.open_prompt(
                "New animation name",
                Pending::CreateAnimation,
                "animation.neoanim",
            ),
            Action::RevealInExplorer => self.reveal_in_explorer(),
            Action::OpenProjectInVscode => self.open_project_in_vscode(),
            Action::OpenPath(p) => self.open_path(&p),
            Action::OpenAnimation(p) => self.open_animation_path(p),
            Action::OpenScene(p) => self.open_scene_path(p),
            Action::EnterFolder(p) => self.navigate_bin(p),
            Action::OpenSelectionTools(x, y) => self.open_selection_tools(x, y),
            Action::OpenHierarchyTools(x, y) => self.open_hierarchy_tools(x, y),
            Action::OpenArrangeTools(x, y) => self.open_arrange_tools(x, y),
            Action::OpenViewTools(x, y) => self.open_view_tools(x, y),
            Action::OpenEditorSettings => self.open_editor_settings(),
            Action::OpenMobileEmulator => self.open_mobile_emulator(),
            Action::OpenProjectWindowSettings => {
                let start_scene = self.project_window.start_scene.clone();
                self.focus = Some("project_start_scene".to_string());
                self.edit_buffer = start_scene.clone();
                self.edit_cursor = start_scene.chars().count();
                self.edit_selection_anchor = None;
                self.popup = Some(Popup::ProjectWindow {
                    start_scene,
                    width: format_num(self.project_window.width),
                    height: format_num(self.project_window.height),
                    fullscreen: self.project_window.fullscreen,
                    resizable: self.project_window.resizable,
                });
            }
            Action::BuildProject => self.open_build_target(),
            Action::ToggleHierarchy => {
                self.config.layout.show_hierarchy = !self.config.layout.show_hierarchy;
                if !self.config.layout.show_hierarchy {
                    self.config.layout.undock_hierarchy = false;
                }
                self.dirty = true;
            }
            Action::ToggleInspector => {
                self.config.layout.show_inspector = !self.config.layout.show_inspector;
                if !self.config.layout.show_inspector {
                    self.config.layout.undock_inspector = false;
                }
                self.dirty = true;
            }
            Action::ToggleHierarchyUndocked => {
                self.config.layout.show_hierarchy = true;
                self.config.layout.undock_hierarchy = !self.config.layout.undock_hierarchy;
                self.dirty = true;
            }
            Action::ToggleInspectorUndocked => {
                self.config.layout.show_inspector = true;
                self.config.layout.undock_inspector = !self.config.layout.undock_inspector;
                self.dirty = true;
            }
            Action::ToggleProjectUndocked => {
                self.config.layout.show_project = true;
                self.config.layout.undock_project = !self.config.layout.undock_project;
                self.dirty = true;
            }
            Action::SetSceneAntialiasing(value) => {
                if matches!(value.as_str(), "off" | "standard" | "high")
                    && self.scene.antialiasing != value
                {
                    self.scene.antialiasing = value;
                    self.mark_dirty();
                }
            }
            Action::SetPropEnum {
                entity,
                component,
                prop,
                value,
            } => {
                let mut changed = false;
                if let Some(entity) = self.scene.entity_mut(entity) {
                    if let Some(Component::Core { props, .. }) =
                        entity.components.get_mut(component)
                    {
                        if let Some(prop) = props.get_mut(prop) {
                            if let PropValue::Enum {
                                value: current,
                                options,
                            } = &mut prop.value
                            {
                                if options.iter().any(|option| option == &value)
                                    && *current != value
                                {
                                    *current = value;
                                    changed = true;
                                }
                            }
                        }
                    }
                }
                if changed {
                    self.mark_dirty();
                }
            }
            Action::SetAttachedValueType {
                entity,
                value,
                path,
                kind,
            } => {
                let mut changed = false;
                if let Some(entity) = self.scene.entity_mut(entity) {
                    if let Some(attached) = entity.values.get_mut(value) {
                        if let Some(current) = var_value_at_path_mut(&mut attached.value, &path) {
                            if AttachedValueType::from_value(current) != kind {
                                *current = kind.default_value();
                                changed = true;
                            }
                        }
                    }
                }
                if changed {
                    self.mark_dirty();
                    self.status = format!("Changed attached value type to {}", kind.label());
                }
            }
            Action::SelectAll => self.select_all(),
            Action::InvertSelection => self.invert_selection(),
            Action::SelectChildren => self.select_children(),
            Action::SelectParent => self.select_parent(),
            Action::SelectRoots => self
                .select_by_filter("Selected root entities", |app, entity| {
                    entity.parent.is_none() && !app.hidden_ids.contains(&entity.id)
                }),
            Action::SelectLeaves => {
                self.select_by_filter("Selected leaf entities", |app, entity| {
                    app.scene.children_of(Some(entity.id)).is_empty()
                        && !app.hidden_ids.contains(&entity.id)
                })
            }
            Action::SelectVisible => self
                .select_by_filter("Selected Scene-visible entities", |app, entity| {
                    !app.hidden_ids.contains(&entity.id)
                }),
            Action::SelectHidden => self
                .select_by_filter("Selected Scene-hidden entities", |app, entity| {
                    app.hidden_ids.contains(&entity.id)
                }),
            Action::SelectLocked => self
                .select_by_filter("Selected locked entities", |app, entity| {
                    app.locked_ids.contains(&entity.id)
                }),
            Action::SelectActive => {
                self.select_by_filter("Selected active entities", |_app, entity| entity.enabled)
            }
            Action::SelectInactive => {
                self.select_by_filter("Selected inactive entities", |_app, entity| !entity.enabled)
            }
            Action::SelectSiblings => self.select_siblings(),
            Action::SelectNext => self.select_relative(1),
            Action::SelectPrevious => self.select_relative(-1),
            Action::DuplicateSelection => self.duplicate_selection(),
            Action::GroupSelected => self.group_selected(),
            Action::UnparentSelected => self.unparent_selected(),
            Action::HideSelected => self.hide_selected(),
            Action::HideUnselected => self.hide_unselected(),
            Action::IsolateSelection => {
                self.hide_unselected();
                self.frame_selected();
            }
            Action::ShowAllHidden => {
                self.hidden_ids.clear();
                self.status = "Revealed all Scene-view objects".to_string();
            }
            Action::ShowSelected => {
                for id in self.selection_ids_ordered() {
                    self.hidden_ids.remove(&id);
                }
                self.status = "Revealed selected Scene-view objects".to_string();
            }
            Action::LockSelected => self.lock_selected(),
            Action::LockUnselected => self.lock_unselected(),
            Action::UnlockSelection => {
                for id in self.selection_ids_ordered() {
                    self.locked_ids.remove(&id);
                }
                self.status = "Unlocked selection".to_string();
            }
            Action::UnlockAll => {
                self.locked_ids.clear();
                self.status = "Unlocked all Scene-view objects".to_string();
            }
            Action::ToggleActiveSelection => self.toggle_active_selection(),
            Action::CollapseSelected => {
                self.hierarchy_collapsed
                    .extend(self.selection_ids_ordered());
                self.status = "Collapsed selected branches".to_string();
            }
            Action::ExpandSelected => {
                for id in self.selection_ids_ordered() {
                    self.hierarchy_collapsed.remove(&id);
                }
                self.status = "Expanded selected branches".to_string();
            }
            Action::CollapseAll => {
                self.hierarchy_collapsed = self
                    .scene
                    .entities
                    .iter()
                    .filter(|entity| !self.scene.children_of(Some(entity.id)).is_empty())
                    .map(|entity| entity.id)
                    .collect();
                self.status = "Collapsed hierarchy".to_string();
            }
            Action::ExpandAll => {
                self.hierarchy_collapsed.clear();
                self.status = "Expanded hierarchy".to_string();
            }
            Action::SnapSelected => self.snap_selected(),
            Action::SnapSelectedSize => self.snap_selected_size(),
            Action::ResetSelected => self.reset_selected(),
            Action::ResetSelectedRotation => self.reset_selected_rotation(),
            Action::ResetSelectedScale => self.reset_selected_scale(),
            Action::ResetSelectedAnchors => self.reset_selected_anchors(),
            Action::FitSelectionToWindow => self.fit_selection_to_window(),
            Action::CenterSelectionInWindow => self.center_selection_in_window(),
            Action::NormalizeSelectedSizes => self.normalize_selected_sizes(),
            Action::Align(kind) => self.align_selected(kind),
            Action::BringToFront => self.move_selection_z(ZMove::Front),
            Action::SendToBack => self.move_selection_z(ZMove::Back),
            Action::BringForward => self.move_selection_z(ZMove::Forward),
            Action::SendBackward => self.move_selection_z(ZMove::Backward),
            Action::NudgeZ(delta) => self.nudge_selection_z(delta),
            Action::RefreshProject => self.refresh_project_browser(),
            Action::RevealSceneFile => {
                let path = self.scene_path.clone();
                self.open_path(&path);
            }
            Action::OpenProjectRoot => {
                let path = self.project_root.clone();
                self.open_path(&path);
            }
            Action::FrameAll => self.frame_all(),
            Action::Zoom100 => self.zoom_100(),
            Action::ToggleMaximize => self.maximize_view = !self.maximize_view,
            Action::ToggleProject => {
                self.config.layout.show_project = !self.config.layout.show_project;
                if !self.config.layout.show_project {
                    self.config.layout.undock_project = false;
                }
                self.dirty = true;
            }
        }
    }

    fn perform_pending(&mut self, action: Pending) {
        match action {
            Pending::LoadScene => self.load(),
            Pending::Quit => self.should_quit = true,
            Pending::CloseDocument(index) => self.close_document(index),
            Pending::UpdateEngine => self.launch_update(),
            _ => {}
        }
    }

    fn perform_pending_with(&mut self, action: Pending, value: String) {
        match action {
            Pending::RenameScene => self.rename_scene(value),
            Pending::CreateFolder => self.create_folder(&value),
            Pending::CreateScript => self.create_script(&value),
            Pending::CreateShader => self.create_shader(&value),
            Pending::CreateAnimation => self.create_animation(&value),
            Pending::RenameEntity(id) => {
                if let Some(e) = self.scene.entity_mut(id) {
                    e.name = value;
                    self.mark_dirty();
                }
            }
            _ => {}
        }
    }

    fn add_entity(&mut self, parent: Option<u64>) {
        self.add_entity_at(parent, 96.0, 96.0);
    }

    fn add_entity_at(&mut self, parent: Option<u64>, x: f32, y: f32) {
        let n = self.scene.entities.len() + 1;
        let mut e = self.scene.add_entity(format!("Entity {n}"), x, y);
        e.parent = parent;
        let id = e.id;
        self.scene.replace_entity(id, e);
        self.select_only(id);
        self.mark_dirty();
        self.status = "Added entity".to_string();
    }

    fn copy_entity(&mut self, id: u64) {
        if let Some(e) = self.scene.entity(id) {
            self.clipboard = Some(e.clone());
            self.status = format!("Copied {}", e.name);
        }
    }

    fn paste_entity(&mut self) {
        if let Some(mut e) = self.clipboard.clone() {
            e.x += 16.0;
            e.y += 16.0;
            e.parent = None;
            e.name = format!("{} Copy", e.name);
            let id = self.scene.insert_entity(e);
            self.select_only(id);
            self.mark_dirty();
            self.status = "Pasted entity".to_string();
        } else {
            self.status = "Clipboard is empty".to_string();
        }
    }

    fn duplicate_entity(&mut self, id: u64) {
        if let Some(e) = self.scene.entity(id) {
            let mut clone = e.clone();
            clone.x += 16.0;
            clone.y += 16.0;
            clone.name = format!("{} Copy", clone.name);
            let new_id = self.scene.insert_entity(clone);
            self.select_only(new_id);
            self.mark_dirty();
            self.status = "Duplicated entity".to_string();
        }
    }

    // ---- Color target plumbing --------------------------------------------

    fn set_target_color(&mut self, target: &ColorTarget, color: [u8; 4]) {
        match target {
            ColorTarget::Background => self.scene.background = color,
            ColorTarget::LightingAmbient => self.scene.lighting.ambient = color,
            ColorTarget::Prop { entity, comp, prop } => {
                if let Some(e) = self.scene.entity_mut(*entity) {
                    if let Some(Component::Core { props, .. }) = e.components.get_mut(*comp) {
                        if let Some(p) = props.get_mut(*prop) {
                            p.value = PropValue::Color(color);
                        }
                    }
                }
            }
            ColorTarget::Var {
                entity,
                comp,
                var,
                path,
            } => {
                if let Some(e) = self.scene.entity_mut(*entity) {
                    if let Some(Component::Script { variables, .. }) = e.components.get_mut(*comp) {
                        if let Some(v) = variables.get_mut(*var) {
                            if let Some(value) = var_value_at_path_mut(&mut v.value, path) {
                                *value = VarValue::Color(color);
                            }
                        }
                    }
                }
            }
            ColorTarget::AttachedValue {
                entity,
                value,
                path,
            } => {
                if let Some(entity) = self.scene.entity_mut(*entity) {
                    if let Some(attached) = entity.values.get_mut(*value) {
                        if let Some(value) = var_value_at_path_mut(&mut attached.value, path) {
                            *value = VarValue::Color(color);
                        }
                    }
                }
            }
        }
    }

    fn set_collapsed(&mut self, key: &str, collapsed: bool) {
        if collapsed {
            self.collapsed.insert(key.to_string());
        } else {
            self.collapsed.remove(key);
        }
    }

    // ---- File / scene actions ---------------------------------------------

    fn new_scene(&mut self) {
        let kind = self.scene.kind;
        let mut number = self.documents.len() + 1;
        let mut path = self.project_root.join(format!("scene_{number}.neoscene"));
        while self.documents.iter().any(|document| document.path == path) || path.exists() {
            number += 1;
            path = self.project_root.join(format!("scene_{number}.neoscene"));
        }
        let mut scene = Scene::new_for_kind(kind);
        scene.name = format!("Scene {number}");
        self.add_document(path, scene, DocumentKind::Scene);
        self.mark_dirty();
        self.status = "New scene tab".to_string();
    }

    fn load_requested(&mut self) {
        if !self.scene_path.exists() {
            self.status = format!("No scene file at {}", self.scene_path.display());
            return;
        }
        if self.scene_dirty {
            self.open_confirm(
                "Discard unsaved changes and load the saved scene?",
                Pending::LoadScene,
            );
        } else {
            self.load();
        }
    }

    fn save(&mut self) {
        let result = match self.document_kind {
            DocumentKind::Scene => self.scene.save(&self.scene_path),
            DocumentKind::Prefab => {
                let mut entities = self.scene.entities.clone();
                for entity in &mut entities {
                    entity.prefab_source = None;
                }
                save_prefab_file(&self.scene_path, &entities)
            }
        };
        match result {
            Ok(()) => {
                self.scene_dirty = false;
                if let Some(document) = self.documents.get_mut(self.active_document) {
                    document.scene = self.scene.clone();
                    document.dirty = false;
                }
                if self.document_kind == DocumentKind::Prefab {
                    self.propagate_saved_prefab();
                }
                self.status = format!("Saved {}", self.scene_path.display());
            }
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    fn load(&mut self) {
        self.load_scene_file(self.scene_path.clone());
    }

    fn open_scene_path(&mut self, path: PathBuf) {
        if !path.starts_with(&self.project_root) {
            self.status = "Scene is outside the project".to_string();
            return;
        }
        if path.extension().is_none_or(|ext| ext != "neoscene") {
            self.open_path(&path);
            return;
        }
        self.load_scene_file(path);
    }

    fn load_scene_file(&mut self, path: PathBuf) {
        match Scene::load(&path) {
            Ok(scene) => {
                self.add_document(path.clone(), scene, DocumentKind::Scene);
                self.status = format!("Loaded {}", self.scene_path.display());
            }
            Err(e) => self.status = format!("Load failed: {e}"),
        }
    }

    fn open_prefab_path(&mut self, path: PathBuf) {
        if !path.starts_with(&self.project_root) {
            self.status = "Prefab is outside the project".to_string();
            return;
        }
        match load_prefab_file(&path) {
            Ok(entities) if !entities.is_empty() => {
                let name = path
                    .file_stem()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Prefab".to_string());
                self.add_document(
                    path.clone(),
                    Scene::from_prefab(name, entities),
                    DocumentKind::Prefab,
                );
                self.status = format!("Editing prefab {}", path.display());
            }
            Ok(_) => self.status = "Prefab contains no entities".to_string(),
            Err(error) => self.status = format!("Prefab load failed: {error}"),
        }
    }

    fn prefab_source_key(&self, path: &Path) -> String {
        path.strip_prefix(&self.project_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn propagate_saved_prefab(&mut self) {
        let source = self.prefab_source_key(&self.scene_path);
        let prototype = self.scene.entities.clone();
        self.sync_active_document();
        let mut updated = 0usize;
        for document in &mut self.documents {
            if document.kind != DocumentKind::Scene {
                continue;
            }
            let count = document.scene.refresh_prefab_instances(&source, &prototype);
            if count > 0 {
                document.dirty = true;
                if let Err(error) = document.scene.save(&document.path) {
                    self.status = format!("Prefab propagated, but scene save failed: {error}");
                } else {
                    document.dirty = false;
                }
                updated += count;
            }
        }
        let active = self.documents[self.active_document].clone();
        self.scene = active.scene;
        self.scene_dirty = active.dirty;

        let mut disk_paths = Vec::new();
        collect_files_with_extension(&self.project_root, "neoscene", &mut disk_paths);
        for path in disk_paths {
            if self.documents.iter().any(|document| document.path == path) {
                continue;
            }
            if let Ok(mut scene) = Scene::load(&path) {
                let count = scene.refresh_prefab_instances(&source, &prototype);
                if count > 0 && scene.save(&path).is_ok() {
                    updated += count;
                }
            }
        }
        if updated > 0 {
            self.status = format!("Saved prefab and refreshed {updated} linked instance(s)");
        }
    }

    fn rename_scene(&mut self, new_name: String) {
        let trimmed = new_name.trim();
        if trimmed.is_empty() {
            return;
        }
        self.scene.name = trimmed.to_string();
        // Rename the on-disk file to match (slugified).
        let slug = slugify(trimmed);
        let new_path = self.project_root.join(format!("{slug}.neoscene"));
        if self.scene_path.exists() && new_path != self.scene_path {
            if let Err(e) = std::fs::rename(&self.scene_path, &new_path) {
                self.status = format!("Renamed scene; file rename failed: {e}");
                self.scene_path = new_path;
                return;
            }
        }
        self.scene_path = new_path;
        let _ = self.scene.save(&self.scene_path);
        self.scene_dirty = false;
        self.status = format!("Renamed scene to {}", self.scene_path.display());
    }

    fn create_folder(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let path = self.bin_dir.join(name);
        match std::fs::create_dir_all(&path) {
            Ok(()) => self.status = format!("Created folder {name}"),
            Err(e) => self.status = format!("Create folder failed: {e}"),
        }
    }

    fn create_script(&mut self, name: &str) {
        let mut name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        if !name.ends_with(".luau") && !name.ends_with(".lua") {
            name.push_str(".luau");
        }
        let path = self.bin_dir.join(&name);
        let template = "--!strict\n-- Values wrapped in Inspector appear in the visual editor.\n\nlocal Behaviour = {\n\tspeed = Inspector(100),\n\ttint = Inspector(Color4(255, 255, 255)),\n}\n\nfunction Behaviour.awake(entity, self)\nend\n\nfunction Behaviour.update(entity, self, dt)\nend\n\nreturn Behaviour\n";
        match std::fs::write(&path, template) {
            Ok(()) => {
                self.status = format!("Created script {name}");
                self.open_path(&path);
            }
            Err(e) => self.status = format!("Create script failed: {e}"),
        }
    }

    fn create_shader(&mut self, name: &str) {
        let mut name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        if !name.ends_with(".frag") && !name.ends_with(".glsl") && !name.ends_with(".shader") {
            name.push_str(".frag");
        }
        let path = self.bin_dir.join(&name);
        let template = "#version 450\n\nuniform sampler2D Texture;\n\nvoid main() {\n    gl_FragColor = texture2D(Texture, uv) * color;\n}\n";
        match std::fs::write(&path, template) {
            Ok(()) => {
                self.status = format!("Created shader {name}");
                self.open_path(&path);
            }
            Err(e) => self.status = format!("Create shader failed: {e}"),
        }
    }

    fn create_animation(&mut self, name: &str) {
        let mut name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        if !name.ends_with(".neoanim") {
            name.push_str(".neoanim");
        }
        let path = self.bin_dir.join(&name);
        let clip = AnimationClipAsset::default();
        match serde_json::to_string_pretty(&clip)
            .map_err(|error| error.to_string())
            .and_then(|json| std::fs::write(&path, json).map_err(|error| error.to_string()))
        {
            Ok(()) => {
                self.status = format!("Created animation {name}");
                let duration = format_num(clip.duration);
                self.focus = Some("anim_duration".to_string());
                self.edit_cursor = duration.chars().count();
                self.edit_selection_anchor = None;
                self.edit_buffer = duration;
                self.popup = Some(Popup::AnimationEditor {
                    path,
                    clip,
                    selected_track: 0,
                    selected_key: 0,
                });
            }
            Err(error) => self.status = format!("Create animation failed: {error}"),
        }
    }

    fn open_animation_path(&mut self, path: PathBuf) {
        match std::fs::read_to_string(&path)
            .map_err(|error| error.to_string())
            .and_then(|text| {
                serde_json::from_str::<AnimationClipAsset>(&text).map_err(|error| error.to_string())
            }) {
            Ok(mut clip) => {
                normalize_animation_clip(&mut clip);
                let duration = format_num(clip.duration);
                self.focus = Some("anim_duration".to_string());
                self.edit_cursor = duration.chars().count();
                self.edit_selection_anchor = None;
                self.edit_buffer = duration;
                self.popup = Some(Popup::AnimationEditor {
                    path,
                    clip,
                    selected_track: 0,
                    selected_key: 0,
                });
            }
            Err(error) => self.status = format!("Animation open failed: {error}"),
        }
    }

    fn add_script_component_from_path(&mut self, entity_id: u64, path: &Path) -> bool {
        let source = match std::fs::read_to_string(path) {
            Ok(source) => source,
            Err(error) => {
                self.status = format!("Script read failed: {error}");
                return false;
            }
        };
        let display_path = path
            .strip_prefix(&self.project_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let variables = match parse_inspector_variables(&source, &display_path) {
            Ok(variables) => variables,
            Err(error) => {
                self.status = error;
                return false;
            }
        };
        let Some(entity) = self.scene.entity_mut(entity_id) else {
            return false;
        };
        entity.components.push(Component::Script {
            path: display_path.clone(),
            variables,
        });
        let entity_name = entity.name.clone();
        self.mark_dirty();
        self.status = format!("Added {display_path} to {entity_name}");
        true
    }

    fn sync_script_variables(&mut self, script_path: &str, variables: &mut Vec<ScriptVar>) -> bool {
        if script_path.trim().is_empty() {
            return false;
        }
        let path = self.project_root.join(script_path);
        let modified = std::fs::metadata(&path)
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        let cached = self
            .script_schema_cache
            .get(script_path)
            .filter(|(previous, _)| *previous == modified)
            .map(|(_, schema)| schema.clone());
        let parsed = match cached {
            Some(schema) => schema,
            None => {
                let schema = std::fs::read_to_string(&path)
                    .map_err(|error| format!("Inspector refresh failed for {script_path}: {error}"))
                    .and_then(|source| parse_inspector_variables(&source, script_path));
                self.script_schema_cache
                    .insert(script_path.to_string(), (modified, schema.clone()));
                schema
            }
        };
        let mut schema = match parsed {
            Ok(schema) => schema,
            Err(error) => {
                self.status = error;
                return false;
            }
        };
        for declared in &mut schema {
            let Some(existing) = variables.iter().find(|existing| {
                existing.name == declared.name
                    && std::mem::discriminant(&existing.value)
                        == std::mem::discriminant(&declared.value)
            }) else {
                continue;
            };
            declared.value = existing.value.clone();
            if let (
                VarValue::Number(value),
                VarControl::Slider {
                    min,
                    max,
                    fractional,
                },
            ) = (&mut declared.value, &declared.control)
            {
                *value = (*value).clamp(*min, *max);
                if !fractional {
                    *value = value.round();
                }
            }
        }
        if *variables == schema {
            return false;
        }
        *variables = schema;
        self.status = format!("Refreshed Inspector variables from {script_path}");
        true
    }

    fn reveal_in_explorer(&mut self) {
        let dir = self.bin_dir.clone();
        self.open_path(&dir);
    }

    fn open_project_in_vscode(&mut self) {
        for command in ["code", "code-insiders", "codium"] {
            match std::process::Command::new(command)
                .arg("--reuse-window")
                .arg(&self.project_root)
                .spawn()
            {
                Ok(_) => {
                    self.status = format!("Opened project in {command}");
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    self.status = format!("Failed to open VS Code: {error}");
                    return;
                }
            }
        }

        for (app_id, label) in [
            ("com.visualstudio.code", "VS Code Flatpak"),
            ("com.visualstudio.code.insiders", "VS Code Insiders Flatpak"),
            ("com.vscodium.codium", "VSCodium Flatpak"),
        ] {
            match flatpak_app_installed(app_id) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => {
                    self.status = format!("Failed to query Flatpak VS Code apps: {error}");
                    return;
                }
            }

            match std::process::Command::new("flatpak")
                .arg("run")
                .arg(app_id)
                .arg("--reuse-window")
                .arg(&self.project_root)
                .spawn()
            {
                Ok(_) => {
                    self.status = format!("Opened project in {label}");
                    return;
                }
                Err(error) => {
                    self.status = format!("Failed to open {label}: {error}");
                    return;
                }
            }
        }

        self.status = "VS Code command not found on PATH or Flatpak".to_string();
    }

    /// Open a file or folder with the OS default handler.
    fn open_path(&mut self, path: &Path) {
        if path
            .extension()
            .is_some_and(|extension| extension == "neoanim")
        {
            self.open_animation_path(path.to_path_buf());
            return;
        }
        #[cfg(target_os = "macos")]
        let cmd = "open";
        #[cfg(target_os = "windows")]
        let cmd = "explorer";
        #[cfg(all(unix, not(target_os = "macos")))]
        let cmd = "xdg-open";
        match std::process::Command::new(cmd).arg(path).spawn() {
            Ok(_) => self.status = format!("Opened {}", path.display()),
            Err(e) => self.status = format!("Open failed: {e}"),
        }
    }

    fn configured_start_scene_path(&self) -> PathBuf {
        resolve_project_start_scene_path(&self.project_root, &self.project_window.start_scene)
    }

    fn scene_for_export(&mut self) -> Result<(PathBuf, Scene), String> {
        self.sync_active_document();
        let start_path = self.configured_start_scene_path();
        if self.document_kind == DocumentKind::Scene && self.scene_path == start_path {
            return Ok((start_path, self.scene.clone()));
        }
        if let Some(document) = self
            .documents
            .iter()
            .find(|document| document.kind == DocumentKind::Scene && document.path == start_path)
        {
            return Ok((start_path, document.scene.clone()));
        }
        if start_path.exists() {
            return Scene::load(&start_path)
                .map(|scene| (start_path.clone(), scene))
                .map_err(|error| {
                    format!(
                        "Failed to load start scene {}: {error}",
                        start_path.display()
                    )
                });
        }
        Err(format!("Start scene not found: {}", start_path.display()))
    }

    fn export_luau(&mut self) -> bool {
        let (scene_path, scene) = match self.scene_for_export() {
            Ok(scene) => scene,
            Err(error) => {
                self.status = error;
                return false;
            }
        };
        let path = self.project_root.join("main.luau");
        if let Err(error) = ensure_editor_owned_output(&path) {
            self.status = error;
            return false;
        }
        // The generated `main.luau` loads the start scene at runtime, so its
        // `.neoscene` must exist and be current on disk regardless of the
        // autosave setting. Persist it before writing the loader.
        if let Err(e) = scene.save(&scene_path) {
            self.status = format!("Export failed (start scene): {e}");
            return false;
        }
        let rel = project_relative_path(&self.project_root, &scene_path);
        if let Err(e) = std::fs::write(&path, scene.to_luau_loader(&rel)) {
            self.status = format!("Export failed: {e}");
            return false;
        }
        // `loadScene` inlines its own image handles at runtime, so the shared
        // image-cache module is no longer referenced. Clean up any stale copy
        // (and the older assets.luau) left by previous editor builds.
        remove_generated_file(&self.project_root.join("images.luau"));
        remove_generated_file(&self.project_root.join("assets.luau"));
        self.status = format!("Exported {} (loads {rel})", path.display());
        true
    }

    /// Save an entity (and its descendants) as a `.neoprefab` in the current
    /// project-bin folder.
    fn save_prefab(&mut self, id: u64) {
        let mut entities = self.scene.subtree(id);
        let name = self
            .scene
            .entity(id)
            .map(|e| e.name.clone())
            .unwrap_or_else(|| "prefab".to_string());
        let path = self.bin_dir.join(format!("{}.neoprefab", slugify(&name)));
        for entity in &mut entities {
            entity.prefab_source = None;
        }
        match save_prefab_file(&path, &entities) {
            Ok(()) => {
                let source = self.prefab_source_key(&path);
                if let Some(root) = self.scene.entity_mut(id) {
                    root.prefab_source = Some(source);
                }
                self.mark_dirty();
                self.status = format!("Saved linked prefab {}", path.display());
            }
            Err(e) => self.status = format!("Save prefab failed: {e}"),
        }
    }

    /// Flush unsaved edits in every open document to disk. The runtime loads
    /// non-start scenes from disk with `loadScene`, so persisting only the
    /// active tab leaves those scenes stale — a just-assigned asset reverts to
    /// unset and the loaded scene can crash (e.g. `audio.play(nil)`). Returns
    /// false if any save failed, in which case the caller should abort.
    fn save_all_open_documents(&mut self) -> bool {
        // Persist the active tab first; `save` also finalizes prefab
        // propagation and refreshes its document copy from the live scene.
        if self.scene_dirty {
            self.save();
            if self.scene_dirty {
                return false;
            }
        } else {
            self.sync_active_document();
        }
        let mut ok = true;
        for index in 0..self.documents.len() {
            if index == self.active_document || !self.documents[index].dirty {
                continue;
            }
            let path = self.documents[index].path.clone();
            let result = match self.documents[index].kind {
                DocumentKind::Scene => self.documents[index].scene.save(&path),
                DocumentKind::Prefab => {
                    let mut entities = self.documents[index].scene.entities.clone();
                    for entity in &mut entities {
                        entity.prefab_source = None;
                    }
                    save_prefab_file(&path, &entities)
                }
            };
            match result {
                Ok(()) => self.documents[index].dirty = false,
                Err(error) => {
                    self.status = format!("Save failed for {}: {error}", path.display());
                    ok = false;
                }
            }
        }
        ok
    }

    fn run_scene(&mut self) {
        if self.config.settings.autosave_before_run && !self.save_all_open_documents() {
            return;
        }
        if !self.export_luau() {
            return;
        }
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                self.status = format!("Run failed: {e}");
                return;
            }
        };
        let root = self.project_root.clone();
        let mobile_profile = self.mobile_emulation_profile();
        // Open a loopback IPC session for the live logger window before
        // launching, so the game can connect back and stream logs + snapshots.
        let ipc_addr = match crate::editor_ipc::LoggerSession::start() {
            Ok(session) => {
                let addr = session.addr.clone();
                self.pending_logger_session = Some(session);
                Some(addr)
            }
            Err(error) => {
                eprintln!("warning: failed to start logger session: {error}");
                None
            }
        };
        let (tx, rx) = std::sync::mpsc::channel();
        // Run the game on a worker thread, capturing its output, so the editor
        // stays responsive and can surface a startup error when it exits.
        std::thread::spawn(move || {
            let mut command = std::process::Command::new(exe);
            command.arg("run").arg(&root).env("RUST_BACKTRACE", "1");
            crate::mobile_emulation::apply_env(&mut command, &mobile_profile);
            if let Some(addr) = &ipc_addr {
                command.env("NEOLOVE_EDITOR_IPC", addr);
            }
            let outcome = match command.output() {
                Ok(out) if out.status.success() => None,
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    Some(if stderr.is_empty() {
                        format!("The game exited with {}.", out.status)
                    } else {
                        stderr
                    })
                }
                Err(e) => Some(format!("Failed to launch the game: {e}")),
            };
            let _ = tx.send(outcome);
        });
        self.run_rx = Some(rx);
        self.status = "Running preview…".to_string();
    }

    fn build_project(&mut self, target: BuildTarget) {
        if self.build_rx.is_some() {
            self.status = "Build already running".to_string();
            return;
        }
        if self.config.settings.autosave_before_build && !self.save_all_open_documents() {
            return;
        }
        if !self.export_luau() {
            return;
        }
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                self.status = format!("Build failed: {e}");
                return;
            }
        };
        let root = self.project_root.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut command = std::process::Command::new(exe);
            command.arg("build").arg(&root);
            if let Some(arg) = target.cli_arg() {
                command.arg(arg);
            }
            let outcome = match command.output() {
                Ok(out) if out.status.success() => {
                    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    Ok(if stdout.is_empty() {
                        "Build complete".to_string()
                    } else {
                        stdout
                    })
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    Err(if !stderr.is_empty() {
                        stderr
                    } else if !stdout.is_empty() {
                        stdout
                    } else {
                        format!("Build exited with {}.", out.status)
                    })
                }
                Err(e) => Err(format!("Failed to launch build: {e}")),
            };
            let _ = tx.send(outcome);
        });
        self.build_rx = Some(rx);
        self.status = format!("Building {}…", target.label());
    }

    /// True while a launched preview is still running.
    pub fn run_pending(&self) -> bool {
        self.run_rx.is_some()
    }

    pub fn build_pending(&self) -> bool {
        self.build_rx.is_some()
    }

    /// Take a pending logger session (set when a run starts) so the windowing
    /// layer can open/show the live logger window. Returns `None` if none.
    pub fn take_logger_session(&mut self) -> Option<crate::editor_ipc::LoggerSession> {
        self.pending_logger_session.take()
    }

    /// Poll the running preview; returns true if it just finished (so the
    /// caller should redraw). On a failure an error popup is opened.
    pub fn poll_run(&mut self) -> bool {
        use std::sync::mpsc::TryRecvError;
        let result = match &self.run_rx {
            Some(rx) => rx.try_recv(),
            None => return false,
        };
        match result {
            Ok(outcome) => {
                self.run_rx = None;
                match outcome {
                    Some(message) => {
                        self.status = "Preview exited with an error".to_string();
                        self.popup = Some(Popup::Error {
                            message,
                            copied: false,
                        });
                    }
                    None => self.status = "Preview closed".to_string(),
                }
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.run_rx = None;
                true
            }
        }
    }

    pub fn poll_build(&mut self) -> bool {
        use std::sync::mpsc::TryRecvError;
        let result = match &self.build_rx {
            Some(rx) => rx.try_recv(),
            None => return false,
        };
        match result {
            Ok(outcome) => {
                self.build_rx = None;
                match outcome {
                    Ok(message) => {
                        self.status = message
                            .lines()
                            .last()
                            .unwrap_or("Build complete")
                            .to_string()
                    }
                    Err(message) => {
                        self.status = "Build failed".to_string();
                        self.popup = Some(Popup::Error {
                            message,
                            copied: false,
                        });
                    }
                }
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.build_rx = None;
                true
            }
        }
    }
}

/// Cached viewport light grid plus the inputs it was built from. The editor
/// redraws continuously; while the camera, viewport, and lighting inputs are
/// unchanged the grid (and its shadow rays) is reused instead of recomputed.
struct PreviewLightGrid {
    cam_x: f32,
    cam_y: f32,
    cam_zoom: f32,
    /// Grid origin in screen space and its dimensions/step footprint.
    gx0: f32,
    gy0: f32,
    gw: usize,
    gh: usize,
    config: crate::lighting::LightConfig,
    lights: Vec<crate::lighting::Light>,
    occluders: Vec<crate::lighting::Occluder>,
    /// Bilinearly upsampled by the composite: one `(r, g, b)` light multiplier
    /// per grid node, row-major over `gw × gh`.
    grid: Vec<(f32, f32, f32)>,
}

#[derive(Clone, Copy, Debug)]
struct EditorWorldTransform {
    x: f32,
    y: f32,
    scale: f32,
    /// Accumulated rotation (radians) down the parent chain. The preview
    /// rotates each entity's own quad by this; it does not orbit child
    /// positions around rotated parents (left to the runtime, like the rest of
    /// the editor's simplified transform).
    rotation: f32,
}

#[derive(Clone, Copy, Debug)]
struct EditorLocalTransform {
    x: f32,
    y: f32,
    scale: f32,
    anchor_x: f32,
    anchor_y: f32,
    pivot_x: f32,
    pivot_y: f32,
    rotation_pivot_x: f32,
    rotation_pivot_y: f32,
}

fn default_editor_camera_3d(fov: f32) -> RenderCamera3D {
    RenderCamera3D {
        position: Vec3::new(4.5, 3.5, 8.0),
        euler: Vec3::new(-18.0, 28.0, 0.0),
        projection: Projection3D::Perspective,
        fov: fov.clamp(20.0, 140.0),
        orthographic_size: 10.0,
        near_clip: 0.05,
        far_clip: 2_000.0,
    }
}

/// Bounded CPU representation of an effectively infinite editor ground grid.
/// The coarse level reaches toward the camera far plane while a denser level
/// follows the camera. This avoids both a visible fixed 40x40 square and an
/// unbounded number of converging sub-pixel lines near the horizon.
#[derive(Clone, Copy, Debug)]
struct Grid3DLayout {
    fine_step: f32,
    fine_half_lines: i32,
    coarse_step: f32,
    coarse_half_lines: i32,
    min_x: f32,
    max_x: f32,
    min_z: f32,
    max_z: f32,
}

fn grid_3d_layout(camera: RenderCamera3D, area: Rect) -> Grid3DLayout {
    const FINE_HALF_LINES: i32 = 32;
    const COARSE_HALF_LINES: i32 = 48;
    const TARGET_FINE_PIXELS: f32 = 32.0;

    let forward = camera_forward(camera.euler);
    let ground_height = camera.position.y.abs().max(0.25);
    // The distance to the ground near the viewport centre is a better scale
    // estimate than camera height alone when looking almost horizontally.
    let centre_distance = (ground_height / forward.y.abs().max(0.15))
        .clamp(camera.near_clip.max(0.01), camera.far_clip.max(1.0));
    let vertical_span =
        centre_distance * 2.0 * (camera.fov.clamp(1.0, 179.0).to_radians() * 0.5).tan();
    let fine_step =
        nice_grid_step(vertical_span / area.h.max(1.0) * TARGET_FINE_PIXELS).clamp(0.01, 1_000.0);

    let desired_extent = (centre_distance * 32.0)
        .max(centre_distance * (area.w / area.h.max(1.0)).max(1.0) * 24.0)
        .max(256.0)
        .max(fine_step * FINE_HALF_LINES as f32 * 8.0)
        .min(camera.far_clip.max(256.0) * 0.9);
    let coarse_step = nice_grid_step(desired_extent / COARSE_HALF_LINES as f32)
        .max(fine_step)
        .clamp(0.01, 10_000.0);
    let half_span = coarse_step * COARSE_HALF_LINES as f32;
    let center_x = (camera.position.x / coarse_step).round() * coarse_step;
    let center_z = (camera.position.z / coarse_step).round() * coarse_step;

    Grid3DLayout {
        fine_step,
        fine_half_lines: FINE_HALF_LINES,
        coarse_step,
        coarse_half_lines: COARSE_HALF_LINES,
        min_x: center_x - half_span,
        max_x: center_x + half_span,
        min_z: center_z - half_span,
        max_z: center_z + half_span,
    }
}

/// Round a positive distance up to a stable 1/2/5 power-of-ten interval.
fn nice_grid_step(minimum: f32) -> f32 {
    if !minimum.is_finite() || minimum <= 0.0 {
        return 1.0;
    }
    let exponent = minimum.log10().floor();
    let magnitude = 10.0_f32.powf(exponent);
    let normalized = minimum / magnitude;
    let nice = if normalized <= 1.0 {
        1.0
    } else if normalized <= 2.0 {
        2.0
    } else if normalized <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * magnitude
}

fn grid_line_aligned(value: f32, coarser_step: f32, tolerance_step: f32) -> bool {
    let nearest = (value / coarser_step).round() * coarser_step;
    (value - nearest).abs() <= tolerance_step.abs().max(f32::EPSILON) * 0.01
}

fn add_vec3(left: Vec3, right: Vec3) -> Vec3 {
    Vec3::new(left.x + right.x, left.y + right.y, left.z + right.z)
}

fn sub_vec3(left: Vec3, right: Vec3) -> Vec3 {
    Vec3::new(left.x - right.x, left.y - right.y, left.z - right.z)
}

fn scale_vec3(value: Vec3, amount: f32) -> Vec3 {
    Vec3::new(value.x * amount, value.y * amount, value.z * amount)
}

fn length_vec3(value: Vec3) -> f32 {
    (value.x * value.x + value.y * value.y + value.z * value.z).sqrt()
}

fn normalized_vec3(value: Vec3) -> Vec3 {
    let length = length_vec3(value);
    if length <= f32::EPSILON || !length.is_finite() {
        Vec3::ZERO
    } else {
        scale_vec3(value, length.recip())
    }
}

fn camera_forward(euler: Vec3) -> Vec3 {
    normalized_vec3(
        Mat4::rotation_euler_degrees(euler).transform_direction(Vec3::new(0.0, 0.0, -1.0)),
    )
}

fn camera_right(euler: Vec3) -> Vec3 {
    normalized_vec3(
        Mat4::rotation_euler_degrees(euler).transform_direction(Vec3::new(1.0, 0.0, 0.0)),
    )
}

fn camera_up(euler: Vec3) -> Vec3 {
    normalized_vec3(
        Mat4::rotation_euler_degrees(euler).transform_direction(Vec3::new(0.0, 1.0, 0.0)),
    )
}

fn viewport_drop_position_3d(
    camera: RenderCamera3D,
    area: Rect,
    mouse_x: f32,
    mouse_y: f32,
) -> Vec3 {
    let ndc_x = ((mouse_x - area.x) / area.w.max(1.0)) * 2.0 - 1.0;
    let ndc_y = 1.0 - ((mouse_y - area.y) / area.h.max(1.0)) * 2.0;
    let tangent = (camera.fov.to_radians() * 0.5).tan();
    let aspect = area.w / area.h.max(1.0);
    let direction = normalized_vec3(add_vec3(
        camera_forward(camera.euler),
        add_vec3(
            scale_vec3(camera_right(camera.euler), ndc_x * tangent * aspect),
            scale_vec3(camera_up(camera.euler), ndc_y * tangent),
        ),
    ));
    // Prefer the conventional XZ ground plane. If the camera is parallel to
    // it or points away, place the asset a comfortable distance down the ray.
    let ground_t = if direction.y.abs() > 1.0e-5 {
        -camera.position.y / direction.y
    } else {
        -1.0
    };
    let distance = if ground_t.is_finite() && ground_t > 0.05 {
        ground_t
    } else {
        6.0
    };
    add_vec3(camera.position, scale_vec3(direction, distance))
}

fn scene_world_model_3d_cached(
    scene: &Scene,
    id: u64,
    visiting: &mut HashSet<u64>,
    cache: &mut HashMap<u64, Mat4>,
) -> Option<Mat4> {
    if let Some(model) = cache.get(&id).copied() {
        return Some(model);
    }
    if !visiting.insert(id) {
        return None;
    }
    let entity = scene.entity(id)?;
    let local = Mat4::trs(
        Vec3::new(entity.x, entity.y, entity.position_z),
        Vec3::new(entity.rotation_x, entity.rotation_y, entity.rotation_z),
        Vec3::new(entity.scale_x, entity.scale_y, entity.scale_z),
    );
    let world = if let Some(parent) = entity.parent {
        scene_world_model_3d_cached(scene, parent, visiting, cache)?.mul(local)
    } else {
        local
    };
    visiting.remove(&id);
    cache.insert(id, world);
    Some(world)
}

fn project_world_point(view_projection: Mat4, point: Vec3, area: Rect) -> Option<(f32, f32, f32)> {
    let clip = view_projection.transform_vec4([point.x, point.y, point.z, 1.0]);
    if clip[3] <= 1.0e-5 || !clip.iter().all(|value| value.is_finite()) {
        return None;
    }
    let inverse_w = clip[3].recip();
    let ndc = [
        clip[0] * inverse_w,
        clip[1] * inverse_w,
        clip[2] * inverse_w,
    ];
    if !(0.0..=1.0).contains(&ndc[2]) || ndc[0].abs() > 100.0 || ndc[1].abs() > 100.0 {
        return None;
    }
    Some((
        area.x + (ndc[0] * 0.5 + 0.5) * area.w,
        area.y + (0.5 - ndc[1] * 0.5) * area.h,
        ndc[2],
    ))
}

/// Clip a world-space line in homogeneous coordinates before perspective
/// division. Projecting endpoints first makes any segment that crosses the
/// eye/near plane appear to fold across the viewport; clipping against the
/// Vulkan/WebGPU frustum keeps grid lines continuous and bounded.
fn project_world_segment_clipped(
    view_projection: Mat4,
    start: Vec3,
    end: Vec3,
    area: Rect,
) -> Option<((f32, f32), (f32, f32))> {
    let start_clip = view_projection.transform_vec4([start.x, start.y, start.z, 1.0]);
    let end_clip = view_projection.transform_vec4([end.x, end.y, end.z, 1.0]);
    if !start_clip
        .iter()
        .chain(end_clip.iter())
        .all(|value| value.is_finite())
    {
        return None;
    }

    let plane_distances = |clip: [f32; 4]| {
        [
            clip[0] + clip[3], // left: x >= -w
            clip[3] - clip[0], // right: x <= w
            clip[1] + clip[3], // bottom: y >= -w
            clip[3] - clip[1], // top: y <= w
            clip[2],           // near: z >= 0
            clip[3] - clip[2], // far: z <= w
            clip[3] - 1.0e-5,  // stay in front of the eye
        ]
    };
    let start_planes = plane_distances(start_clip);
    let end_planes = plane_distances(end_clip);
    let mut enter = 0.0_f32;
    let mut exit = 1.0_f32;
    for (start_distance, end_distance) in start_planes.into_iter().zip(end_planes) {
        if start_distance < 0.0 && end_distance < 0.0 {
            return None;
        }
        let delta = end_distance - start_distance;
        if start_distance < 0.0 {
            if delta <= 0.0 {
                return None;
            }
            enter = enter.max((-start_distance / delta).clamp(0.0, 1.0));
        } else if end_distance < 0.0 {
            if delta >= 0.0 {
                return None;
            }
            exit = exit.min((-start_distance / delta).clamp(0.0, 1.0));
        }
        if enter > exit {
            return None;
        }
    }

    let interpolate = |from: [f32; 4], to: [f32; 4], amount: f32| {
        std::array::from_fn(|index| from[index] + (to[index] - from[index]) * amount)
    };
    let to_screen = |clip: [f32; 4]| {
        let inverse_w = clip[3].recip();
        let x = clip[0] * inverse_w;
        let y = clip[1] * inverse_w;
        (
            area.x + (x * 0.5 + 0.5) * area.w,
            area.y + (0.5 - y * 0.5) * area.h,
        )
    };
    Some((
        to_screen(interpolate(start_clip, end_clip, enter)),
        to_screen(interpolate(start_clip, end_clip, exit)),
    ))
}

fn viewport_display_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale.clamp(0.5, 8.0)
    } else {
        1.0
    }
}

fn viewport_triangle_budget(area: Rect) -> usize {
    const MIN_TRIANGLES: usize = 30_000;
    const MAX_TRIANGLES: usize = 250_000;
    const LOGICAL_PIXELS_PER_TRIANGLE: f64 = 8.0;

    let logical_pixels = f64::from(area.w.max(0.0)) * f64::from(area.h.max(0.0));
    if !logical_pixels.is_finite() {
        return MAX_TRIANGLES;
    }
    (logical_pixels / LOGICAL_PIXELS_PER_TRIANGLE)
        .round()
        .clamp(MIN_TRIANGLES as f64, MAX_TRIANGLES as f64) as usize
}

fn recycle_viewport_scratch<T>(slot: &mut Vec<T>, mut scratch: Vec<T>) {
    scratch.clear();
    *slot = scratch;
}

fn triangle_screen_bounds(points: [(f32, f32); 3]) -> Rect {
    let min_x = points
        .iter()
        .map(|point| point.0)
        .fold(f32::INFINITY, f32::min);
    let min_y = points
        .iter()
        .map(|point| point.1)
        .fold(f32::INFINITY, f32::min);
    let max_x = points
        .iter()
        .map(|point| point.0)
        .fold(f32::NEG_INFINITY, f32::max);
    let max_y = points
        .iter()
        .map(|point| point.1)
        .fold(f32::NEG_INFINITY, f32::max);
    Rect::new(min_x, min_y, max_x - min_x, max_y - min_y)
}

fn point_in_triangle_3d_preview(point: (f32, f32), triangle: [(f32, f32); 3]) -> bool {
    let edge = |a: (f32, f32), b: (f32, f32), p: (f32, f32)| {
        (p.0 - a.0) * (b.1 - a.1) - (p.1 - a.1) * (b.0 - a.0)
    };
    let first = edge(triangle[0], triangle[1], point);
    let second = edge(triangle[1], triangle[2], point);
    let third = edge(triangle[2], triangle[0], point);
    let has_negative = first < 0.0 || second < 0.0 || third < 0.0;
    let has_positive = first > 0.0 || second > 0.0 || third > 0.0;
    !(has_negative && has_positive)
}

fn viewport_hit_3d(
    triangles: &[Viewport3DHit],
    proxies: &[Viewport3DProxyHit],
    mouse_x: f32,
    mouse_y: f32,
) -> Option<u64> {
    let mut closest: Option<(f32, u64)> = None;
    for hit in triangles {
        if hit.bounds.contains(mouse_x, mouse_y)
            && point_in_triangle_3d_preview((mouse_x, mouse_y), hit.points)
            && closest.is_none_or(|(depth, _)| hit.depth < depth)
        {
            closest = Some((hit.depth, hit.id));
        }
    }
    for hit in proxies {
        let dx = mouse_x - hit.x;
        let dy = mouse_y - hit.y;
        if dx * dx + dy * dy <= hit.radius * hit.radius
            && closest.is_none_or(|(depth, _)| hit.depth < depth)
        {
            closest = Some((hit.depth, hit.id));
        }
    }
    closest.map(|(_, id)| id)
}

fn gizmo_axis(gizmo: Viewport3DGizmo, axis: Viewport3DAxis) -> Option<Viewport3DGizmoAxis> {
    gizmo
        .axes
        .into_iter()
        .flatten()
        .find(|candidate| candidate.axis == axis)
}

fn vector2_length(value: (f32, f32)) -> f32 {
    (value.0 * value.0 + value.1 * value.1).sqrt()
}

fn normalized_vec2(value: (f32, f32)) -> (f32, f32) {
    let length = vector2_length(value);
    if length <= 1.0e-5 || !length.is_finite() {
        (1.0, 0.0)
    } else {
        (value.0 / length, value.1 / length)
    }
}

fn point_segment_distance_squared(point: (f32, f32), start: (f32, f32), end: (f32, f32)) -> f32 {
    let direction = (end.0 - start.0, end.1 - start.1);
    let length_squared = direction.0 * direction.0 + direction.1 * direction.1;
    if length_squared <= 1.0e-6 || !length_squared.is_finite() {
        let delta = (point.0 - start.0, point.1 - start.1);
        return delta.0 * delta.0 + delta.1 * delta.1;
    }
    let amount = (((point.0 - start.0) * direction.0 + (point.1 - start.1) * direction.1)
        / length_squared)
        .clamp(0.0, 1.0);
    let nearest = (
        start.0 + direction.0 * amount,
        start.1 + direction.1 * amount,
    );
    let delta = (point.0 - nearest.0, point.1 - nearest.1);
    delta.0 * delta.0 + delta.1 * delta.1
}

fn viewport_rotation_ring_hit_3d(
    gizmo: Viewport3DGizmo,
    mouse_x: f32,
    mouse_y: f32,
    display_scale: f32,
) -> Option<Viewport3DRotationDragHit> {
    let point = (mouse_x, mouse_y);
    let hit_distance_squared = (7.0 * display_scale).powi(2);
    let mut closest: Option<(f32, Viewport3DRotationDragHit)> = None;

    for ring in gizmo.rotation_rings {
        let mut perimeter = 0.0;
        for index in 0..ROTATION_RING_SAMPLES {
            let next = (index + 1) % ROTATION_RING_SAMPLES;
            if let (Some(start), Some(end)) = (ring.points[index], ring.points[next]) {
                perimeter += vector2_length((end.0 - start.0, end.1 - start.1));
            }
        }
        if !perimeter.is_finite() || perimeter <= 1.0 {
            continue;
        }

        for index in 0..ROTATION_RING_SAMPLES {
            let next = (index + 1) % ROTATION_RING_SAMPLES;
            let (Some(start), Some(end)) = (ring.points[index], ring.points[next]) else {
                continue;
            };
            let segment = (end.0 - start.0, end.1 - start.1);
            if vector2_length(segment) <= 0.25 {
                continue;
            }
            let distance = point_segment_distance_squared(point, start, end);
            if distance <= hit_distance_squared
                && closest.is_none_or(|(best_distance, _)| distance < best_distance)
            {
                closest = Some((
                    distance,
                    Viewport3DRotationDragHit {
                        axis: ring.axis,
                        screen_tangent: normalized_vec2(segment),
                        // One full traversal of the projected ring maps to one
                        // authored turn. Bounds keep an almost edge-on ellipse
                        // responsive without allowing a single pixel to fling
                        // the Euler field by hundreds of degrees.
                        degrees_per_pixel: (360.0 / perimeter).clamp(0.1, 4.0),
                    },
                ));
            }
        }
    }
    closest.map(|(_, hit)| hit)
}

fn viewport_gizmo_hit_3d(
    gizmo: Viewport3DGizmo,
    tool: ViewTool,
    mouse_x: f32,
    mouse_y: f32,
    display_scale: f32,
) -> Option<Viewport3DGizmoHit> {
    let point = (mouse_x, mouse_y);
    let endpoint_radius = 11.0 * display_scale;
    if matches!(tool, ViewTool::Scale | ViewTool::Transform) {
        let mut closest: Option<(f32, Viewport3DAxis)> = None;
        for projected in gizmo.axes.into_iter().flatten() {
            let distance = vector2_length((point.0 - projected.end.0, point.1 - projected.end.1));
            if distance <= endpoint_radius && closest.is_none_or(|(best, _)| distance < best) {
                closest = Some((distance, projected.axis));
            }
        }
        if let Some((_, axis)) = closest {
            return Some(Viewport3DGizmoHit::ScaleAxis(axis));
        }
    }

    let center_distance = vector2_length((point.0 - gizmo.origin.0, point.1 - gizmo.origin.1));
    match tool {
        ViewTool::Scale if center_distance <= 11.0 * display_scale => {
            return Some(Viewport3DGizmoHit::ScaleUniform);
        }
        ViewTool::Transform if center_distance <= 6.0 * display_scale => {
            return Some(Viewport3DGizmoHit::MoveFree);
        }
        ViewTool::Transform if center_distance <= 14.0 * display_scale => {
            return Some(Viewport3DGizmoHit::ScaleUniform);
        }
        ViewTool::Move if center_distance <= 10.0 * display_scale => {
            return Some(Viewport3DGizmoHit::MoveFree);
        }
        _ => {}
    }

    if matches!(tool, ViewTool::Rotate | ViewTool::Transform)
        && let Some(hit) = viewport_rotation_ring_hit_3d(gizmo, mouse_x, mouse_y, display_scale)
    {
        return Some(Viewport3DGizmoHit::RotateAxis(hit.axis));
    }

    // The combined tool reserves endpoints for scale and its centre for free
    // movement. Dedicated Move provides the larger axis-arm hit targets.
    if tool == ViewTool::Move {
        let line_radius_squared = (8.0 * display_scale).powi(2);
        let mut closest: Option<(f32, Viewport3DAxis)> = None;
        for projected in gizmo.axes.into_iter().flatten() {
            let distance = point_segment_distance_squared(point, gizmo.origin, projected.end);
            if distance <= line_radius_squared && closest.is_none_or(|(best, _)| distance < best) {
                closest = Some((distance, projected.axis));
            }
        }
        if let Some((_, axis)) = closest {
            return Some(Viewport3DGizmoHit::MoveAxis(axis));
        }
    }
    None
}

fn vec3_axis(value: Vec3, axis: Viewport3DAxis) -> f32 {
    match axis {
        Viewport3DAxis::X => value.x,
        Viewport3DAxis::Y => value.y,
        Viewport3DAxis::Z => value.z,
    }
}

fn entity_position_axis_from_vec3(value: Vec3, axis: Viewport3DAxis) -> f32 {
    vec3_axis(value, axis)
}

fn entity_position_axis_3d(entity: &Entity, axis: Viewport3DAxis) -> f32 {
    match axis {
        Viewport3DAxis::X => entity.x,
        Viewport3DAxis::Y => entity.y,
        Viewport3DAxis::Z => entity.position_z,
    }
}

fn set_entity_position_axis_3d(entity: &mut Entity, axis: Viewport3DAxis, value: f32) {
    if !value.is_finite() {
        return;
    }
    match axis {
        Viewport3DAxis::X => entity.x = value,
        Viewport3DAxis::Y => entity.y = value,
        Viewport3DAxis::Z => entity.position_z = value,
    }
}

fn entity_scale_axis_3d(entity: &Entity, axis: Viewport3DAxis) -> f32 {
    match axis {
        Viewport3DAxis::X => entity.scale_x,
        Viewport3DAxis::Y => entity.scale_y,
        Viewport3DAxis::Z => entity.scale_z,
    }
}

fn set_entity_scale_axis_3d(entity: &mut Entity, axis: Viewport3DAxis, value: f32) {
    if !value.is_finite() {
        return;
    }
    match axis {
        Viewport3DAxis::X => entity.scale_x = value,
        Viewport3DAxis::Y => entity.scale_y = value,
        Viewport3DAxis::Z => entity.scale_z = value,
    }
}

fn entity_rotation_axis_3d(entity: &Entity, axis: Viewport3DAxis) -> f32 {
    match axis {
        Viewport3DAxis::X => entity.rotation_x,
        Viewport3DAxis::Y => entity.rotation_y,
        Viewport3DAxis::Z => entity.rotation_z,
    }
}

fn set_entity_rotation_axis_3d(entity: &mut Entity, axis: Viewport3DAxis, value: f32) {
    if !value.is_finite() {
        return;
    }
    match axis {
        Viewport3DAxis::X => entity.rotation_x = value,
        Viewport3DAxis::Y => entity.rotation_y = value,
        Viewport3DAxis::Z => entity.rotation_z = value,
    }
}

fn stable_drag_scale_3d(start: f32, factor: f32, snap: bool) -> f32 {
    const MIN_ABS_SCALE: f32 = 0.001;
    const MAX_ABS_SCALE: f32 = 1_000.0;
    let sign = if start.is_sign_negative() { -1.0 } else { 1.0 };
    let mut magnitude = start.abs().max(MIN_ABS_SCALE) * factor.clamp(0.01, 32.0);
    if snap {
        magnitude = (magnitude / 0.1).round() * 0.1;
    }
    sign * magnitude.clamp(MIN_ABS_SCALE, MAX_ABS_SCALE)
}

fn stable_drag_rotation_3d(start: f32, delta_degrees: f32, snap: bool) -> f32 {
    const MAX_ABS_ROTATION: f32 = 1_000_000.0;
    let mut value = (start + delta_degrees.clamp(-36_000.0, 36_000.0))
        .clamp(-MAX_ABS_ROTATION, MAX_ABS_ROTATION);
    if snap {
        value = (value / 15.0).round() * 15.0;
    }
    if value.is_finite() {
        value
    } else {
        start.clamp(-MAX_ABS_ROTATION, MAX_ABS_ROTATION)
    }
}

fn entity_proxy_radius_3d(entity: &Entity) -> f32 {
    if entity.components.iter().any(|component| {
        matches!(component, Component::Core { name, .. } if matches!(name.as_str(), "Camera3D" | "Light3D" | "Collider3D"))
    }) {
        12.0
    } else {
        7.0
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_wire_box_3d(
    painter: &mut Painter<'_>,
    area: Rect,
    view_projection: Mat4,
    model: Mat4,
    offset: Vec3,
    half: Vec3,
    color: Rgba,
) {
    let local = [
        Vec3::new(offset.x - half.x, offset.y - half.y, offset.z - half.z),
        Vec3::new(offset.x + half.x, offset.y - half.y, offset.z - half.z),
        Vec3::new(offset.x + half.x, offset.y + half.y, offset.z - half.z),
        Vec3::new(offset.x - half.x, offset.y + half.y, offset.z - half.z),
        Vec3::new(offset.x - half.x, offset.y - half.y, offset.z + half.z),
        Vec3::new(offset.x + half.x, offset.y - half.y, offset.z + half.z),
        Vec3::new(offset.x + half.x, offset.y + half.y, offset.z + half.z),
        Vec3::new(offset.x - half.x, offset.y + half.y, offset.z + half.z),
    ];
    let points =
        local.map(|point| project_world_point(view_projection, model.transform_point(point), area));
    for (start, end) in [
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 0),
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 4),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ] {
        if let (Some(start), Some(end)) = (points[start], points[end]) {
            painter.stroke_line(start.0, start.1, end.0, end.1, color);
        }
    }
}

fn draw_camera_proxy_3d(
    painter: &mut Painter<'_>,
    area: Rect,
    view_projection: Mat4,
    model: Mat4,
    fov_degrees: f32,
    color: Rgba,
    display_scale: f32,
) {
    // Solid camera body in local space, followed by a perspective frustum.
    draw_wire_box_3d(
        painter,
        area,
        view_projection,
        model,
        Vec3::ZERO,
        Vec3::new(0.36, 0.23, 0.32),
        color,
    );

    let near_distance = 0.56;
    let far_distance = 1.75;
    let tangent = (fov_degrees.to_radians() * 0.5).tan().clamp(0.15, 3.5);
    let near_half_y = tangent * near_distance * 0.55;
    let near_half_x = near_half_y * 1.6;
    let far_half_y = tangent * far_distance * 0.55;
    let far_half_x = far_half_y * 1.6;
    let local = [
        Vec3::new(-near_half_x, -near_half_y, -near_distance),
        Vec3::new(near_half_x, -near_half_y, -near_distance),
        Vec3::new(near_half_x, near_half_y, -near_distance),
        Vec3::new(-near_half_x, near_half_y, -near_distance),
        Vec3::new(-far_half_x, -far_half_y, -far_distance),
        Vec3::new(far_half_x, -far_half_y, -far_distance),
        Vec3::new(far_half_x, far_half_y, -far_distance),
        Vec3::new(-far_half_x, far_half_y, -far_distance),
    ];
    let world = local.map(|point| model.transform_point(point));
    for (start, end) in [
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 0),
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 4),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ] {
        if let Some((from, to)) =
            project_world_segment_clipped(view_projection, world[start], world[end], area)
        {
            stroke_line_hidpi(painter, from, to, color, display_scale);
        }
    }

    // Lens and raised viewfinder make the proxy recognizable even when the
    // frustum is edge-on.
    if let Some(lens) = project_world_point(
        view_projection,
        model.transform_point(Vec3::new(0.0, 0.0, -0.37)),
        area,
    ) {
        painter.fill_circle(lens.0, lens.1, 4.0 * display_scale, color);
        painter.fill_circle(lens.0, lens.1, 1.8 * display_scale, [18, 45, 58, 255]);
    }
    let finder = [
        model.transform_point(Vec3::new(-0.19, 0.23, 0.08)),
        model.transform_point(Vec3::new(0.0, 0.48, 0.02)),
        model.transform_point(Vec3::new(0.19, 0.23, 0.08)),
    ];
    for (start, end) in [(0, 1), (1, 2), (2, 0)] {
        if let Some((from, to)) =
            project_world_segment_clipped(view_projection, finder[start], finder[end], area)
        {
            stroke_line_hidpi(painter, from, to, color, display_scale);
        }
    }
}

fn stroke_line_hidpi(
    painter: &mut Painter<'_>,
    start: (f32, f32),
    end: (f32, f32),
    color: Rgba,
    display_scale: f32,
) {
    painter.stroke_line(start.0, start.1, end.0, end.1, color);
    if display_scale >= 1.5 {
        painter.stroke_line(start.0 + 0.75, start.1, end.0 + 0.75, end.1, color);
    }
    if display_scale >= 2.5 {
        painter.stroke_line(start.0, start.1 + 0.75, end.0, end.1 + 0.75, color);
    }
}

#[derive(Clone)]
struct EditorImageCacheEntry {
    modified: Option<SystemTime>,
    image: Option<Rc<image::RgbaImage>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EditorMeshFileStamp {
    modified: Option<SystemTime>,
    len: Option<u64>,
}

#[derive(Clone)]
struct EditorMeshCacheEntry {
    stamp: EditorMeshFileStamp,
    mesh: Option<crate::mesh::MeshHandle>,
    last_used: u64,
}

#[derive(Clone)]
struct EditorWaveformCacheEntry {
    modified: Option<SystemTime>,
    peaks: Option<Rc<Vec<f32>>>,
}

#[derive(Clone)]
struct ProjectDirectoryListing {
    dirs: Vec<(PathBuf, String)>,
    files: Vec<(PathBuf, String, char)>,
}

#[derive(Clone)]
struct ProjectDirectoryCacheEntry {
    modified: Option<SystemTime>,
    listing: Rc<ProjectDirectoryListing>,
}

fn decode_waveform_peaks(path: &Path, buckets: usize) -> Result<Vec<f32>, String> {
    if buckets == 0 {
        return Ok(Vec::new());
    }
    let file =
        File::open(path).map_err(|error| format!("failed to open {}: {error}", path.display()))?;
    let decoder = rodio::Decoder::new(BufReader::new(file))
        .map_err(|error| format!("failed to decode {}: {error}", path.display()))?;
    let channels = decoder.channels().max(1) as usize;
    let samples = decoder.convert_samples::<f32>().collect::<Vec<_>>();
    if samples.is_empty() {
        return Ok(vec![0.0; buckets]);
    }
    let frames = (samples.len() / channels).max(1);
    let mut peaks = vec![0.0_f32; buckets];
    for frame in 0..frames {
        let bucket = (frame * buckets / frames).min(buckets - 1);
        let start = frame * channels;
        let mut level = 0.0_f32;
        for channel in 0..channels {
            level += samples
                .get(start + channel)
                .copied()
                .unwrap_or_default()
                .abs();
        }
        level /= channels as f32;
        peaks[bucket] = peaks[bucket].max(level);
    }
    let max_peak = peaks.iter().copied().fold(0.0_f32, f32::max);
    if max_peak > f32::EPSILON {
        for peak in &mut peaks {
            *peak = (*peak / max_peak).clamp(0.0, 1.0);
        }
    }
    Ok(peaks)
}

fn fit_rect_to_bounds(width: f32, height: f32, bounds: Rect) -> Rect {
    if width <= 0.0 || height <= 0.0 || bounds.w <= 0.0 || bounds.h <= 0.0 {
        return bounds;
    }
    let scale = (bounds.w / width).min(bounds.h / height);
    let w = width * scale;
    let h = height * scale;
    Rect::new(
        bounds.x + (bounds.w - w) * 0.5,
        bounds.y + (bounds.h - h) * 0.5,
        w,
        h,
    )
}

fn draw_waveform_preview(ui: &mut Ui, rect: Rect, peaks: &[f32]) {
    if rect.w <= 0.0 || rect.h <= 0.0 {
        return;
    }
    let center_y = rect.y + rect.h * 0.5;
    ui.painter.fill_rect(
        Rect::new(rect.x, center_y, rect.w, 1.0),
        [
            ui.theme.text_dim[0],
            ui.theme.text_dim[1],
            ui.theme.text_dim[2],
            90,
        ],
    );
    if peaks.is_empty() {
        return;
    }
    let columns = rect.w.floor().max(1.0) as usize;
    for column in 0..columns {
        let sample = column * peaks.len() / columns;
        let peak = peaks[sample].clamp(0.0, 1.0);
        let height = (peak * rect.h * 0.46).max(1.0);
        let color = [
            ui.theme.accent[0],
            ui.theme.accent[1],
            ui.theme.accent[2],
            220,
        ];
        ui.painter.fill_rect(
            Rect::new(rect.x + column as f32, center_y - height, 1.0, height * 2.0),
            color,
        );
    }
}

fn editor_entity_scale(entity: &Entity) -> f32 {
    if entity.scale.is_finite() {
        entity.scale.max(0.0)
    } else {
        1.0
    }
}

fn editor_entity_position_pivot_fraction(entity: &Entity) -> (f32, f32) {
    if entity.pivot_x.is_some() || entity.pivot_y.is_some() {
        (entity.pivot_x.unwrap_or(0.0), entity.pivot_y.unwrap_or(0.0))
    } else {
        position_pivot_fraction_from_name(&entity.position_pivot)
    }
}

fn editor_entity_rotation_pivot_fraction(entity: &Entity) -> (f32, f32) {
    if entity.rotation_pivot_x.is_some() || entity.rotation_pivot_y.is_some() {
        (
            entity.rotation_pivot_x.unwrap_or(0.0),
            entity.rotation_pivot_y.unwrap_or(0.0),
        )
    } else if entity.pivot_x.is_some() || entity.pivot_y.is_some() {
        (entity.pivot_x.unwrap_or(0.0), entity.pivot_y.unwrap_or(0.0))
    } else {
        rotation_pivot_fraction_from_name(&entity.rotation_pivot)
    }
}

fn editor_parent_size(scene: &Scene, entity: &Entity, root_size: (f32, f32)) -> (f32, f32) {
    editor_parent_size_inner(scene, entity, root_size, &mut HashSet::new())
}

fn editor_parent_size_inner(
    scene: &Scene,
    entity: &Entity,
    root_size: (f32, f32),
    visiting: &mut HashSet<u64>,
) -> (f32, f32) {
    match entity.parent {
        Some(parent_id) => scene
            .entity(parent_id)
            .map(|parent| editor_entity_size_inner(scene, parent, root_size, visiting))
            .unwrap_or((0.0, 0.0)),
        None => root_size,
    }
}

fn editor_entity_size(scene: &Scene, entity: &Entity, root_size: (f32, f32)) -> (f32, f32) {
    editor_entity_size_inner(scene, entity, root_size, &mut HashSet::new())
}

fn editor_entity_size_inner(
    scene: &Scene,
    entity: &Entity,
    root_size: (f32, f32),
    visiting: &mut HashSet<u64>,
) -> (f32, f32) {
    let mut size = (entity.size_x, entity.size_y);
    if !visiting.insert(entity.id) {
        return size;
    }
    for component in &entity.components {
        let Component::Core { name, props } = component else {
            continue;
        };
        if name != "EntityScaler" || prop_bool(props, &["enabled"]).is_some_and(|enabled| !enabled)
        {
            continue;
        }

        let (parent_w, parent_h) = editor_parent_size_inner(scene, entity, root_size, visiting);
        let size_x_percent = prop_number(props, &["size_x_percent", "sizeXPercent"])
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        let size_y_percent = prop_number(props, &["size_y_percent", "sizeYPercent"])
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        if size_x_percent > 0.0 {
            size.0 = parent_w * size_x_percent;
        }
        if size_y_percent > 0.0 {
            size.1 = parent_h * size_y_percent;
        }
        break;
    }
    visiting.remove(&entity.id);
    size
}

fn editor_anchor_offset(
    scene: &Scene,
    entity: &Entity,
    local: EditorLocalTransform,
    root_size: (f32, f32),
) -> (f32, f32) {
    let (parent_w, parent_h) = editor_parent_size(scene, entity, root_size);
    (parent_w * local.anchor_x, parent_h * local.anchor_y)
}

fn editor_entity_local_transform(entity: &Entity) -> EditorLocalTransform {
    let (pivot_x, pivot_y) = editor_entity_position_pivot_fraction(entity);
    let (rotation_pivot_x, rotation_pivot_y) = editor_entity_rotation_pivot_fraction(entity);
    let mut transform = EditorLocalTransform {
        x: entity.x,
        y: entity.y,
        scale: editor_entity_scale(entity),
        anchor_x: entity.anchor_x,
        anchor_y: entity.anchor_y,
        pivot_x,
        pivot_y,
        rotation_pivot_x,
        rotation_pivot_y,
    };

    for component in &entity.components {
        let Component::Core { name, props } = component else {
            continue;
        };
        if name != "EntityScaler" || prop_bool(props, &["enabled"]).is_some_and(|enabled| !enabled)
        {
            continue;
        }

        transform.anchor_x =
            prop_number(props, &["x_percent", "xPercent", "percent_x", "percentX"])
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
        transform.anchor_y =
            prop_number(props, &["y_percent", "yPercent", "percent_y", "percentY"])
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
        transform.x = prop_number(props, &["offset_x", "offsetX"]).unwrap_or(0.0);
        transform.y = prop_number(props, &["offset_y", "offsetY"]).unwrap_or(0.0);
        let scaler_pivot_x = prop_number(props, &["pivot_x", "pivotX", "anchor_x", "anchorX"])
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        let scaler_pivot_y = prop_number(props, &["pivot_y", "pivotY", "anchor_y", "anchorY"])
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        transform.pivot_x = scaler_pivot_x;
        transform.pivot_y = scaler_pivot_y;
        if entity.rotation_pivot_x.is_none() && entity.rotation_pivot_y.is_none() {
            transform.rotation_pivot_x = scaler_pivot_x;
            transform.rotation_pivot_y = scaler_pivot_y;
        }
        break;
    }

    transform
}

fn editor_pivot_origin_compensation(
    size_x: f32,
    size_y: f32,
    local: EditorLocalTransform,
    rotation: f32,
) -> (f32, f32) {
    let position_pivot_x = size_x * local.scale * local.pivot_x;
    let position_pivot_y = size_y * local.scale * local.pivot_y;
    let rotation_pivot_x = size_x * local.scale * local.rotation_pivot_x;
    let rotation_pivot_y = size_y * local.scale * local.rotation_pivot_y;
    let (rotated_pivot_x, rotated_pivot_y) =
        rotate_vector(rotation_pivot_x, rotation_pivot_y, rotation);
    (
        -position_pivot_x + rotation_pivot_x - rotated_pivot_x,
        -position_pivot_y + rotation_pivot_y - rotated_pivot_y,
    )
}

fn scene_world_transform(
    scene: &Scene,
    id: u64,
    root_size: (f32, f32),
) -> Option<EditorWorldTransform> {
    let mut visiting = HashSet::new();
    let mut cache = HashMap::new();
    scene_world_transform_cached(scene, id, root_size, &mut visiting, &mut cache)
}

fn scene_world_transform_cached(
    scene: &Scene,
    id: u64,
    root_size: (f32, f32),
    visiting: &mut HashSet<u64>,
    cache: &mut HashMap<u64, EditorWorldTransform>,
) -> Option<EditorWorldTransform> {
    if let Some(transform) = cache.get(&id).copied() {
        return Some(transform);
    }
    if !visiting.insert(id) {
        return None;
    }

    let entity = scene.entity(id)?;
    let local = editor_entity_local_transform(entity);
    let parent_transform = entity
        .parent
        .and_then(|parent| scene_world_transform_cached(scene, parent, root_size, visiting, cache))
        .unwrap_or(EditorWorldTransform {
            x: 0.0,
            y: 0.0,
            scale: 1.0,
            rotation: 0.0,
        });
    let (anchor_x, anchor_y) = editor_anchor_offset(scene, entity, local, root_size);
    let (size_x, size_y) = editor_entity_size(scene, entity, root_size);
    let (pivot_offset_x, pivot_offset_y) =
        editor_pivot_origin_compensation(size_x, size_y, local, entity.rotation);
    // The local origin offset is expressed in the parent's unrotated frame, so
    // rotate it by the parent's world rotation before adding it. This mirrors
    // the runtime's `get_global_transform` and makes children orbit a rotated
    // parent instead of drifting away from its rotation pivot.
    let offset_x = (anchor_x + local.x + pivot_offset_x) * parent_transform.scale;
    let offset_y = (anchor_y + local.y + pivot_offset_y) * parent_transform.scale;
    let (offset_x, offset_y) = rotate_vector(offset_x, offset_y, parent_transform.rotation);
    let transform = EditorWorldTransform {
        x: parent_transform.x + offset_x,
        y: parent_transform.y + offset_y,
        scale: parent_transform.scale * local.scale,
        rotation: parent_transform.rotation + entity.rotation,
    };
    visiting.remove(&id);
    cache.insert(id, transform);
    Some(transform)
}

fn scene_world_origin_to_local_position(
    scene: &Scene,
    entity_id: u64,
    world_x: f32,
    world_y: f32,
    root_size: (f32, f32),
) -> Option<(f32, f32)> {
    let entity = scene.entity(entity_id)?;
    let local = editor_entity_local_transform(entity);
    let parent_transform = entity
        .parent
        .and_then(|parent| scene_world_transform(scene, parent, root_size))
        .unwrap_or(EditorWorldTransform {
            x: 0.0,
            y: 0.0,
            scale: 1.0,
            rotation: 0.0,
        });
    let parent_scale = if parent_transform.scale.abs() < f32::EPSILON {
        1.0
    } else {
        parent_transform.scale
    };
    let (anchor_x, anchor_y) = editor_anchor_offset(scene, entity, local, root_size);
    let (size_x, size_y) = editor_entity_size(scene, entity, root_size);
    let (pivot_offset_x, pivot_offset_y) =
        editor_pivot_origin_compensation(size_x, size_y, local, entity.rotation);
    // Invert the parent-rotation applied in `scene_world_transform_cached`
    // before removing the scale, anchor and pivot compensation.
    let (unrotated_x, unrotated_y) = rotate_vector(
        world_x - parent_transform.x,
        world_y - parent_transform.y,
        -parent_transform.rotation,
    );
    Some((
        unrotated_x / parent_scale - anchor_x - pivot_offset_x,
        unrotated_y / parent_scale - anchor_y - pivot_offset_y,
    ))
}

#[derive(Clone, Copy)]
struct TextPreviewDefaults {
    default_scale: f32,
    default_align_x: TextAlignX,
    default_align_y: TextAlignY,
    default_text_scale: TextScaleMode,
    default_wrap: TextWrapMode,
    default_size_mode_uses_entity: bool,
    color_names: &'static [&'static str],
    fallback_color: [u8; 4],
}

fn compare_editor_entity_order(a: &Entity, b: &Entity) -> Ordering {
    match a.z.partial_cmp(&b.z).unwrap_or(Ordering::Equal) {
        Ordering::Equal => a.id.cmp(&b.id),
        other => other,
    }
}

fn text_preview_request(
    project_root: &Path,
    props: &[Prop],
    rect: Rect,
    zoom: f32,
    defaults: TextPreviewDefaults,
) -> Option<TextRenderRequest> {
    let text = prop_string_like(props, &["text"])?;
    if text.is_empty() {
        return None;
    }

    let zoom = zoom.max(0.01);
    let padding = prop_number(props, &["padding"]).unwrap_or(0.0).max(0.0);
    let padding_x = prop_number(props, &["padding_x", "paddingX"])
        .unwrap_or(padding)
        .max(0.0)
        * zoom;
    let padding_y = prop_number(props, &["padding_y", "paddingY"])
        .unwrap_or(padding)
        .max(0.0)
        * zoom;
    let size_mode_uses_entity =
        text_size_mode_uses_entity(props, defaults.default_size_mode_uses_entity);
    let legacy_scale_x = prop_number(props, &["scale_x", "scaleX"]).unwrap_or(0.0);
    let legacy_scale_y = prop_number(props, &["scale_y", "scaleY"]).unwrap_or(0.0);
    let use_legacy_stretch = !size_mode_uses_entity && legacy_scale_x > 0.0 && legacy_scale_y > 0.0;

    let scale = if use_legacy_stretch {
        legacy_scale_y.max(1.0)
    } else {
        prop_number(props, &["scale"])
            .unwrap_or(defaults.default_scale)
            .max(1.0)
    } * zoom;
    let min_scale = (prop_number(props, &["min_scale", "minScale"])
        .unwrap_or(1.0)
        .max(1.0)
        * zoom)
        .max(1.0)
        .min(scale.max(1.0));

    let bounds = if size_mode_uses_entity {
        RenderRect {
            x: rect.x,
            y: rect.y,
            w: rect.w.max(0.0),
            h: rect.h.max(0.0),
        }
    } else {
        RenderRect {
            x: rect.x,
            y: rect.y,
            w: 0.0,
            h: 0.0,
        }
    };
    let color = prop_color_value(props, defaults.color_names).unwrap_or(defaults.fallback_color);

    Some(TextRenderRequest {
        text,
        bounds,
        // The editor viewport preview is axis-aligned today, like the rest of
        // its component preview path. Reuse runtime layout/rasterization while
        // leaving full transform rotation to the game runtime.
        rotation: 0.0,
        pivot: RenderVec2 {
            x: rect.x,
            y: rect.y,
        },
        color: Color::rgba(color[0], color[1], color[2], color[3]),
        font: prop_string_like(props, &["font"])
            .map(|font| resolve_editor_font_path(project_root, &font))
            .unwrap_or(FontHandle::Default),
        scale: scale.max(1.0),
        min_scale,
        text_scale: prop_string_like(props, &["text_scale", "textScale"])
            .map(|value| parse_preview_text_scale(&value))
            .unwrap_or(defaults.default_text_scale),
        align_x: prop_string_like(props, &["align_x", "alignX", "align"])
            .map(|value| parse_preview_align_x(&value))
            .unwrap_or(defaults.default_align_x),
        align_y: prop_string_like(
            props,
            &["align_y", "alignY", "vertical_align", "verticalAlign"],
        )
        .map(|value| parse_preview_align_y(&value))
        .unwrap_or(defaults.default_align_y),
        wrap: prop_wrap_mode(props).unwrap_or(defaults.default_wrap),
        padding_x,
        padding_y,
        line_spacing: prop_number(props, &["line_spacing", "lineSpacing"]).unwrap_or(1.0),
        letter_spacing: prop_number(props, &["letter_spacing", "letterSpacing"]).unwrap_or(0.0)
            * zoom,
        tab_size: prop_number(props, &["tab_size", "tabSize", "tab_width", "tabWidth"])
            .unwrap_or(4.0),
        stretch_width: if use_legacy_stretch {
            legacy_scale_x * zoom
        } else {
            0.0
        },
        stretch_height: if use_legacy_stretch {
            legacy_scale_y * zoom
        } else {
            0.0
        },
        rich_text: Vec::new(),
        antialiasing: match prop_string_like(props, &["antialiasing"]).as_deref() {
            Some("off" | "none" | "pixel") => TextAntialiasing::Off,
            Some("standard" | "fast" | "normal") => TextAntialiasing::Standard,
            _ => TextAntialiasing::High,
        },
    })
}

fn prop_by_name<'a>(props: &'a [Prop], names: &[&str]) -> Option<&'a Prop> {
    props
        .iter()
        .find(|prop| names.iter().any(|name| prop.name == *name))
}

fn prop_number(props: &[Prop], names: &[&str]) -> Option<f32> {
    prop_by_name(props, names).and_then(|prop| match &prop.value {
        PropValue::Number(value) => Some(*value),
        PropValue::Int(value) => Some(*value as f32),
        PropValue::Text(value) => value.trim().parse::<f32>().ok(),
        _ => None,
    })
}

fn prop_bool(props: &[Prop], names: &[&str]) -> Option<bool> {
    prop_by_name(props, names).and_then(|prop| match &prop.value {
        PropValue::Bool(value) => Some(*value),
        PropValue::Text(value) => value.trim().parse::<bool>().ok(),
        _ => None,
    })
}

#[derive(Clone, Copy, Debug)]
struct EntityScalerEditorState {
    edit_with_percent: bool,
    x_percent: f32,
    y_percent: f32,
    size_x_percent: f32,
    size_y_percent: f32,
    offset_x: f32,
    offset_y: f32,
}

fn entity_scaler_editor_state(entity: &Entity) -> Option<EntityScalerEditorState> {
    entity.components.iter().find_map(|component| {
        let Component::Core { name, props } = component else {
            return None;
        };
        if name != "EntityScaler" || prop_bool(props, &["enabled"]).is_some_and(|enabled| !enabled)
        {
            return None;
        }
        Some(EntityScalerEditorState {
            edit_with_percent: prop_bool(props, &["edit_with_percent", "editWithPercent"])
                .unwrap_or(true),
            x_percent: prop_number(props, &["x_percent", "xPercent", "percent_x", "percentX"])
                .unwrap_or(0.0)
                .clamp(0.0, 1.0),
            y_percent: prop_number(props, &["y_percent", "yPercent", "percent_y", "percentY"])
                .unwrap_or(0.0)
                .clamp(0.0, 1.0),
            size_x_percent: prop_number(props, &["size_x_percent", "sizeXPercent"])
                .unwrap_or(0.0)
                .clamp(0.0, 1.0),
            size_y_percent: prop_number(props, &["size_y_percent", "sizeYPercent"])
                .unwrap_or(0.0)
                .clamp(0.0, 1.0),
            offset_x: prop_number(props, &["offset_x", "offsetX"]).unwrap_or(0.0),
            offset_y: prop_number(props, &["offset_y", "offsetY"]).unwrap_or(0.0),
        })
    })
}

fn set_entity_scaler_numbers(entity: &mut Entity, values: &[(&str, &str, f32)]) -> bool {
    let Some(props) = entity
        .components
        .iter_mut()
        .find_map(|component| match component {
            Component::Core { name, props } if name == "EntityScaler" => Some(props),
            _ => None,
        })
    else {
        return false;
    };

    let mut changed = false;
    for (name, label, value) in values {
        if let Some(prop) = props.iter_mut().find(|prop| prop.name == *name) {
            if !matches!(prop.value, PropValue::Number(current) if current == *value) {
                prop.value = PropValue::Number(*value);
                changed = true;
            }
        } else {
            props.push(Prop {
                name: (*name).to_string(),
                label: (*label).to_string(),
                value: PropValue::Number(*value),
                advanced: false,
                optional: false,
            });
            changed = true;
        }
    }
    changed
}

fn prop_string_like(props: &[Prop], names: &[&str]) -> Option<String> {
    prop_by_name(props, names).and_then(|prop| match &prop.value {
        PropValue::Text(value)
        | PropValue::Image(value)
        | PropValue::Font(value)
        | PropValue::Sound(value)
        | PropValue::Mesh(value)
        | PropValue::Shader(value)
        | PropValue::Animation(value) => Some(value.clone()),
        PropValue::Enum { value, .. } => Some(value.clone()),
        PropValue::Number(value) => Some(format_num(*value)),
        PropValue::Int(value) => Some(value.to_string()),
        PropValue::Bool(value) => Some(value.to_string()),
        PropValue::Color(_)
        | PropValue::StringList(_)
        | PropValue::ColorSequence(_)
        | PropValue::NumberSequence(_) => None,
    })
}

fn prop_color_value(props: &[Prop], names: &[&str]) -> Option<[u8; 4]> {
    prop_by_name(props, names).and_then(|prop| match &prop.value {
        PropValue::Color(color) => Some(*color),
        _ => None,
    })
}

fn prop_wrap_mode(props: &[Prop]) -> Option<TextWrapMode> {
    prop_by_name(props, &["wrap"]).and_then(|prop| match &prop.value {
        PropValue::Bool(true) => Some(TextWrapMode::Word),
        PropValue::Bool(false) => Some(TextWrapMode::None),
        PropValue::Text(value) | PropValue::Enum { value, .. } => Some(parse_preview_wrap(value)),
        _ => None,
    })
}

fn text_size_mode_uses_entity(props: &[Prop], default_uses_entity: bool) -> bool {
    match prop_string_like(
        props,
        &["size_mode", "sizeMode", "bounds_mode", "boundsMode"],
    ) {
        Some(value) => match value.trim().to_ascii_lowercase().as_str() {
            "entity" | "box" | "bounds" => true,
            "content" | "label" => false,
            _ => !prop_bool(props, &["auto_size", "autoSize"]).unwrap_or(true),
        },
        None => default_uses_entity,
    }
}

fn parse_preview_text_scale(raw: &str) -> TextScaleMode {
    match raw.trim().to_ascii_lowercase().as_str() {
        "fit" | "contain" => TextScaleMode::Fit,
        "fit_width" | "fitwidth" | "width" => TextScaleMode::FitWidth,
        "fit_height" | "fitheight" | "height" => TextScaleMode::FitHeight,
        _ => TextScaleMode::None,
    }
}

fn parse_preview_align_x(raw: &str) -> TextAlignX {
    match raw.trim().to_ascii_lowercase().as_str() {
        "center" | "centre" | "middle" => TextAlignX::Center,
        "right" | "end" => TextAlignX::Right,
        _ => TextAlignX::Left,
    }
}

fn parse_preview_align_y(raw: &str) -> TextAlignY {
    match raw.trim().to_ascii_lowercase().as_str() {
        "center" | "centre" | "middle" => TextAlignY::Center,
        "bottom" | "end" => TextAlignY::Bottom,
        _ => TextAlignY::Top,
    }
}

fn parse_preview_wrap(raw: &str) -> TextWrapMode {
    match raw.trim().to_ascii_lowercase().as_str() {
        "word" | "words" => TextWrapMode::Word,
        "char" | "character" | "characters" => TextWrapMode::Char,
        _ => TextWrapMode::None,
    }
}

/// Remove a previously generated artifact, but only if it still carries our
/// "Generated by the NeoLOVE visual editor" header, so a hand-edited file is
/// never silently deleted.
fn remove_generated_file(path: &Path) {
    match std::fs::read_to_string(path) {
        Ok(contents) if contents.starts_with("-- Generated by the NeoLOVE visual editor") => {
            let _ = std::fs::remove_file(path);
        }
        _ => {}
    }
}

const EDITOR_GENERATED_HEADER: &[u8] = b"-- Generated by the NeoLOVE visual editor";

/// Refuse to replace an entry point unless it was previously generated by the
/// visual editor. In particular, opening a code-first project and pressing Run
/// must never turn its `main.luau` into a scene loader.
fn ensure_editor_owned_output(path: &Path) -> Result<(), String> {
    match std::fs::read(path) {
        Ok(contents) if contents.starts_with(EDITOR_GENERATED_HEADER) => Ok(()),
        Ok(_) => Err(format!(
            "Export stopped: {} contains user-authored code and was left unchanged. Move or rename it before exporting a visual scene.",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "Export stopped: could not inspect {}: {error}",
            path.display()
        )),
    }
}

fn normalize_editor_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::Normal(part) => normalized.push(part),
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                normalized.push(component.as_os_str())
            }
        }
    }
    normalized
}

fn resolve_editor_font_path(root: &Path, input: &str) -> FontHandle {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("default") {
        return FontHandle::Default;
    }

    let path = PathBuf::from(trimmed);
    let candidate = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    let resolved = normalize_editor_path(&candidate);
    if !resolved.starts_with(root) {
        return FontHandle::Default;
    }
    FontHandle::Path(resolved.to_string_lossy().into_owned())
}

/// Draw a 9-slice image into `dest`: corners keep their (zoom-scaled) size
/// while edges and the center stretch — matching the engine's renderer.
fn draw_nine_slice(
    p: &mut Painter,
    img: &image::RgbaImage,
    dest: Rect,
    l: f32,
    r: f32,
    t: f32,
    b: f32,
    tint: [u8; 4],
    z: f32,
) {
    let iw = img.width() as f32;
    let ih = img.height() as f32;
    // Source slice sizes, clamped so they don't overlap.
    let sl = l.max(0.0).min(iw);
    let sr = r.max(0.0).min(iw - sl);
    let st = t.max(0.0).min(ih);
    let sb = b.max(0.0).min(ih - st);
    // Destination corner sizes, clamped to the destination.
    let dl = (sl * z).min(dest.w);
    let dr = (sr * z).min(dest.w - dl);
    let dt = (st * z).min(dest.h);
    let db = (sb * z).min(dest.h - dt);
    let sx = [0.0, sl, iw - sr];
    let sw = [sl, (iw - sl - sr).max(0.0), sr];
    let sy = [0.0, st, ih - sb];
    let sh = [st, (ih - st - sb).max(0.0), sb];
    let dx = [dest.x, dest.x + dl, dest.right() - dr];
    let dw = [dl, (dest.w - dl - dr).max(0.0), dr];
    let dy = [dest.y, dest.y + dt, dest.bottom() - db];
    let dh = [dt, (dest.h - dt - db).max(0.0), db];
    for row in 0..3 {
        for col in 0..3 {
            if sw[col] <= 0.0 || sh[row] <= 0.0 || dw[col] <= 0.0 || dh[row] <= 0.0 {
                continue;
            }
            p.draw_image(
                img,
                Rect::new(dx[col], dy[row], dw[col], dh[row]),
                Some(Rect::new(sx[col], sy[row], sw[col], sh[row])),
                tint,
            );
        }
    }
}

/// Tile an image across `dest` at the given (zoom-scaled) tile size.
fn draw_tiled(
    p: &mut Painter,
    img: &image::RgbaImage,
    dest: Rect,
    tile_w: f32,
    tile_h: f32,
    tint: [u8; 4],
    z: f32,
) {
    let tw = (tile_w * z).max(2.0);
    let th = (tile_h * z).max(2.0);
    let mut y = dest.y;
    while y < dest.bottom() {
        let mut x = dest.x;
        while x < dest.right() {
            let cw = tw.min(dest.right() - x);
            let ch = th.min(dest.bottom() - y);
            let src = Rect::new(
                0.0,
                0.0,
                img.width() as f32 * (cw / tw),
                img.height() as f32 * (ch / th),
            );
            p.draw_image(img, Rect::new(x, y, cw, ch), Some(src), tint);
            x += tw;
        }
        y += th;
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_tilemap(
    p: &mut Painter,
    img: &image::RgbaImage,
    dest: Rect,
    columns: usize,
    rows: usize,
    tile_w: f32,
    tile_h: f32,
    spacing: f32,
    margin: f32,
    data: &str,
    tint: [u8; 4],
) {
    let tile_w = tile_w.max(1.0);
    let tile_h = tile_h.max(1.0);
    let atlas_columns = (((img.width() as f32 - margin * 2.0 + spacing) / (tile_w + spacing))
        .floor() as i32)
        .max(1) as usize;
    let atlas_rows = (((img.height() as f32 - margin * 2.0 + spacing) / (tile_h + spacing)).floor()
        as i32)
        .max(1) as usize;
    let atlas_len = atlas_columns * atlas_rows;
    let ids: Vec<i32> = data
        .split(|character: char| character == ',' || character.is_whitespace())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect();
    let cell_w = dest.w / columns as f32;
    let cell_h = dest.h / rows as f32;
    for row in 0..rows {
        for column in 0..columns {
            let id = ids.get(row * columns + column).copied().unwrap_or(-1);
            if id < 0 || id as usize >= atlas_len {
                continue;
            }
            let id = id as usize;
            p.draw_image(
                img,
                Rect::new(
                    dest.x + column as f32 * cell_w,
                    dest.y + row as f32 * cell_h,
                    cell_w,
                    cell_h,
                ),
                Some(Rect::new(
                    margin + (id % atlas_columns) as f32 * (tile_w + spacing),
                    margin + (id / atlas_columns) as f32 * (tile_h + spacing),
                    tile_w,
                    tile_h,
                )),
                tint,
            );
        }
    }
}

fn parse_tile_ids(data: &str, len: usize) -> Vec<i32> {
    let mut ids: Vec<i32> = data
        .split(|character: char| character == ',' || character.is_whitespace())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect();
    ids.resize(len, -1);
    ids
}

fn format_tile_ids(ids: &[i32], columns: usize) -> String {
    let columns = columns.max(1);
    let mut out = String::new();
    for (index, id) in ids.iter().enumerate() {
        if index > 0 {
            if index % columns == 0 {
                out.push('\n');
            } else {
                out.push_str(", ");
            }
        }
        out.push_str(&id.to_string());
    }
    out
}

fn normalize_animation_clip(clip: &mut AnimationClipAsset) {
    if !clip.duration.is_finite() || clip.duration <= 0.0 {
        clip.duration = 1.0;
    }
    if clip.tracks.is_empty() {
        clip.tracks.push(AnimationTrackAsset::default());
    }
    for track in &mut clip.tracks {
        if track.property.trim().is_empty() {
            track.property = "x".to_string();
        }
        match track.interpolation.as_str() {
            "linear" | "step" | "hold" | "bezier" => {}
            _ => track.interpolation = "linear".to_string(),
        }
        if track.keys.is_empty() {
            track.keys.push(AnimationKeyAsset::new(0.0, 0.0));
        }
        for key in &mut track.keys {
            if !key.time.is_finite() {
                key.time = 0.0;
            }
            key.time = key.time.clamp(0.0, clip.duration);
            if !key.value.is_finite() {
                key.value = 0.0;
            }
            key.out_x = key.out_x.clamp(0.0, 1.0);
            key.in_x = key.in_x.clamp(0.0, 1.0);
            if !key.out_y.is_finite() {
                key.out_y = 0.0;
            }
            if !key.in_y.is_finite() {
                key.in_y = 1.0;
            }
        }
        track.keys.sort_by(|a, b| a.time.total_cmp(&b.time));
    }
}

fn nearest_animation_key(keys: &[AnimationKeyAsset], time: f32) -> usize {
    keys.iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| (a.time - time).abs().total_cmp(&(b.time - time).abs()))
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn sample_color_sequence(keypoints: &[ColorKeypoint], time: f32) -> [u8; 4] {
    let Some(first) = keypoints.first() else {
        return [255, 255, 255, 255];
    };
    let time = time.clamp(0.0, 1.0);
    if time <= first.time {
        return first.color;
    }
    for pair in keypoints.windows(2) {
        if time <= pair[1].time {
            let span = (pair[1].time - pair[0].time).max(f32::EPSILON);
            let amount = ((time - pair[0].time) / span).clamp(0.0, 1.0);
            let mut color = [0; 4];
            for channel in 0..4 {
                color[channel] = (pair[0].color[channel] as f32
                    + (pair[1].color[channel] as f32 - pair[0].color[channel] as f32) * amount)
                    .round() as u8;
            }
            return color;
        }
    }
    keypoints
        .last()
        .map(|keypoint| keypoint.color)
        .unwrap_or(first.color)
}

fn sample_number_sequence(keypoints: &[NumberKeypoint], time: f32) -> f32 {
    let Some(first) = keypoints.first() else {
        return 0.0;
    };
    let time = time.clamp(0.0, 1.0);
    if time <= first.time {
        return first.value;
    }
    for pair in keypoints.windows(2) {
        if time <= pair[1].time {
            let span = (pair[1].time - pair[0].time).max(f32::EPSILON);
            let amount = ((time - pair[0].time) / span).clamp(0.0, 1.0);
            return pair[0].value + (pair[1].value - pair[0].value) * amount;
        }
    }
    keypoints
        .last()
        .map(|keypoint| keypoint.value)
        .unwrap_or(first.value)
}

fn draw_sequence_strip(
    painter: &mut Painter,
    rect: Rect,
    value: &SequenceValue,
    fallback: [u8; 4],
) {
    let tile = 8.0;
    let columns = (rect.w / tile).ceil() as usize;
    let rows = (rect.h / tile).ceil() as usize;
    for row in 0..rows {
        for column in 0..columns {
            let shade = if (row + column) % 2 == 0 { 78 } else { 46 };
            painter.fill_rect(
                Rect::new(
                    rect.x + column as f32 * tile,
                    rect.y + row as f32 * tile,
                    tile.min(rect.right() - (rect.x + column as f32 * tile)),
                    tile.min(rect.bottom() - (rect.y + row as f32 * tile)),
                ),
                [shade, shade, shade, 255],
            );
        }
    }
    let steps = rect.w.max(1.0).ceil() as usize;
    for step in 0..steps {
        let time = (step as f32 + 0.5) / steps as f32;
        let color = match value {
            SequenceValue::Colors(keypoints) => sample_color_sequence(keypoints, time),
            SequenceValue::Numbers(keypoints) => {
                let alpha = ((1.0 - sample_number_sequence(keypoints, time).clamp(0.0, 1.0))
                    * 255.0)
                    .round() as u8;
                [255, 255, 255, alpha]
            }
        };
        painter.fill_rect(Rect::new(rect.x + step as f32, rect.y, 1.0, rect.h), color);
    }
    if steps == 0 {
        painter.fill_rect(rect, fallback);
    }
}

fn nearest_color_keypoint(keypoints: &[ColorKeypoint], time: f32) -> usize {
    keypoints
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| (a.time - time).abs().total_cmp(&(b.time - time).abs()))
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn nearest_number_keypoint(keypoints: &[NumberKeypoint], time: f32) -> usize {
    keypoints
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| (a.time - time).abs().total_cmp(&(b.time - time).abs()))
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn largest_color_gap_midpoint(keypoints: &[ColorKeypoint]) -> f32 {
    keypoints
        .windows(2)
        .max_by(|a, b| (a[1].time - a[0].time).total_cmp(&(b[1].time - b[0].time)))
        .map(|pair| (pair[0].time + pair[1].time) * 0.5)
        .unwrap_or(0.5)
}

fn largest_number_gap_midpoint(keypoints: &[NumberKeypoint]) -> f32 {
    keypoints
        .windows(2)
        .max_by(|a, b| (a[1].time - a[0].time).total_cmp(&(b[1].time - b[0].time)))
        .map(|pair| (pair[0].time + pair[1].time) * 0.5)
        .unwrap_or(0.5)
}

/// Word-wrap one line to `max_w` pixels, hard-breaking over-long words.
fn wrap_line(painter: &Painter, text: &str, size: f32, max_w: f32) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split(' ') {
        let candidate = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if painter.text_width(&candidate, size) <= max_w || cur.is_empty() {
            // Hard-break a single word that is itself too wide.
            if cur.is_empty() && painter.text_width(word, size) > max_w {
                let mut chunk = String::new();
                for ch in word.chars() {
                    if painter.text_width(&format!("{chunk}{ch}"), size) > max_w
                        && !chunk.is_empty()
                    {
                        lines.push(std::mem::take(&mut chunk));
                    }
                    chunk.push(ch);
                }
                cur = chunk;
            } else {
                cur = candidate;
            }
        } else {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_string();
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Copy text to the OS clipboard via the first available helper tool.
fn copy_to_clipboard(text: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};

    #[cfg(all(not(target_arch = "wasm32"), not(target_os = "android")))]
    {
        if arboard::Clipboard::new()
            .and_then(|mut clipboard| clipboard.set_text(text.to_string()))
            .is_ok()
        {
            return true;
        }
    }

    const POWERSHELL_WRITE: &str = "Set-Clipboard -Value ([Console]::In.ReadToEnd())";
    let candidates: &[(&str, &[&str])] = &[
        ("powershell", &["-NoProfile", "-Command", POWERSHELL_WRITE]),
        (
            "powershell.exe",
            &["-NoProfile", "-Command", POWERSHELL_WRITE],
        ),
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
        ("pbcopy", &[]),
        ("clip", &[]),
    ];
    for (cmd, args) in candidates {
        if let Ok(mut child) = Command::new(cmd)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                if stdin.write_all(text.as_bytes()).is_err() {
                    let _ = child.kill();
                    continue;
                }
            }
            if child.wait().map(|s| s.success()).unwrap_or(false) {
                return true;
            }
        }
    }
    false
}

fn component_icon(component: &Component) -> char {
    match component {
        Component::Core { name, .. } => core_icon(name),
        Component::Script { .. } => icon::DATA_OBJECT,
    }
}

fn core_icon(name: &str) -> char {
    match name {
        "Rect2D" | "Shape2D" => icon::CROP_SQUARE,
        "ParticleSystem2D" => icon::PALETTE,
        "SpatialSound2D" => icon::AUDIOTRACK,
        "TextBox" | "TextLabel" | "RudimentaryTextLabel" | "TextInput" => icon::TITLE,
        "Sprite2D" | "SpriteSheet2D" | "Image2D" | "NineSliceSprite2D" | "TileTexture2D"
        | "Tilemap2D" | "Spritebox2D" => icon::IMAGE,
        "Collider2D" => icon::BORDER_ALL,
        "Rigidbody2D" => icon::VIEW_IN_AR,
        "Camera" => icon::VIDEOCAM,
        "EntityScaler" | "Bolt2D" | "Rope2D" | "LegacyBolt2D" | "String2D" => icon::TUNE,
        "Frame" | "Panel" | "ScrollList" => icon::VIEW_QUILT,
        "Button" => icon::ADD_CIRCLE,
        "Dropdown" => icon::EXPAND_MORE,
        "Slider" => icon::TUNE,
        _ => icon::VIEW_QUILT,
    }
}

/// Convert an RGBA color to (hue 0..360, saturation 0..1, value 0..1).
fn rgb_to_hsv(c: [u8; 4]) -> (f32, f32, f32) {
    let r = c[0] as f32 / 255.0;
    let g = c[1] as f32 / 255.0;
    let b = c[2] as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let mut h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    if h < 0.0 {
        h += 360.0;
    }
    let s = if max == 0.0 { 0.0 } else { d / max };
    (h, s, max)
}

/// Convert (hue 0..360, saturation 0..1, value 0..1) to an opaque RGB color.
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 4] {
    let c = v * s;
    let hp = (h % 360.0) / 60.0;
    let xx = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as i32 {
        0 => (c, xx, 0.0),
        1 => (xx, c, 0.0),
        2 => (0.0, c, xx),
        3 => (0.0, xx, c),
        4 => (xx, 0.0, c),
        _ => (c, 0.0, xx),
    };
    let m = v - c;
    [
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8,
        255,
    ]
}

/// Parse a 6-digit hex color (`RRGGBB`); returns opaque RGBA.
fn parse_hex(s: &str) -> Option<[u8; 4]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some([r, g, b, 255])
}

fn prop_color(props: &[Prop], name: &str) -> Option<[u8; 4]> {
    props
        .iter()
        .find(|p| p.name == name)
        .and_then(|p| match p.value {
            PropValue::Color(c) => Some(c),
            _ => None,
        })
}

fn var_value_at_path_mut<'a>(
    mut value: &'a mut VarValue,
    path: &[VarPathPart],
) -> Option<&'a mut VarValue> {
    for part in path {
        value = match (part, value) {
            (VarPathPart::List(index), VarValue::List(values)) => values.get_mut(*index)?,
            (VarPathPart::Dictionary(index), VarValue::Dictionary(entries)) => {
                &mut entries.get_mut(*index)?.value
            }
            _ => return None,
        };
    }
    Some(value)
}

fn clamp_range(v: f32, lo: f32, hi: f32) -> f32 {
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    v.clamp(lo, hi)
}

fn collect_files_with_extension(root: &Path, extension: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .is_some_and(|name| name == "dist" || name == "target" || name == ".git")
        {
            continue;
        }
        if path.is_dir() {
            collect_files_with_extension(&path, extension, out);
        } else if path
            .extension()
            .is_some_and(|value| value.eq_ignore_ascii_case(extension))
        {
            out.push(path);
        }
    }
}

fn matches_ignore_ascii_case(value: &str, choices: &[&str]) -> bool {
    choices
        .iter()
        .any(|choice| value.eq_ignore_ascii_case(choice))
}

fn collect_asset_files(root: &Path, kind: AssetKind, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with('.'))
        {
            continue;
        }
        if path.is_dir() {
            collect_asset_files(&path, kind, out);
        } else if kind.accepts(&path) {
            out.push(path);
        }
    }
}

fn slugify(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if ch == ' ' || ch == '-' || ch == '_' {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push_str("scene");
    }
    out
}

fn file_icon(name: &str) -> char {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "png" | "bmp" | "tga" | "webp" | "jpg" | "jpeg" | "pnm" | "ppm" | "pgm" | "gif" | "tif"
        | "tiff" | "hdr" | "dds" => icon::IMAGE,
        "wav" | "mp3" | "ogg" | "flac" | "aac" | "m4a" | "aiff" => icon::AUDIOTRACK,
        "ttf" | "otf" => icon::FONT_DOWNLOAD,
        "glsl" | "frag" | "vert" | "fs" | "vs" | "shader" => icon::DATA_OBJECT,
        "neoanim" | "animation" | "anim" => icon::PLAY,
        "luau" | "lua" => icon::DATA_OBJECT,
        "toml" | "json" | "txt" | "md" | "neoscene" => icon::ARTICLE,
        "neoprefab" => icon::VIEW_IN_AR,
        _ => icon::INSERT_DRIVE_FILE,
    }
}

pub fn global_config_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Some(appdata) = std::env::var_os("APPDATA").filter(|value| !value.is_empty()) {
            return PathBuf::from(appdata).join("NeoLOVE").join("editor.json");
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("NeoLOVE")
                .join("editor.json");
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(config_home) =
            std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty())
        {
            return PathBuf::from(config_home)
                .join("neolove")
                .join("editor.json");
        }
        if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
            return PathBuf::from(home)
                .join(".config")
                .join("neolove")
                .join("editor.json");
        }
    }

    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("editor.json")
}

pub fn load_config_with_fallback(global_path: &Path, legacy_path: &Path) -> EditorConfig {
    if global_path.exists() {
        return load_config(global_path);
    }
    load_config(legacy_path)
}

pub fn load_config(path: &Path) -> EditorConfig {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let has_settings = text.contains("\"settings\"");
            let has_custom_theme = text.contains("\"custom_theme\"");
            let mut config = serde_json::from_str::<EditorConfig>(&text).unwrap_or_else(|error| {
                eprintln!("warning: failed to parse {}: {error}", path.display());
                EditorConfig::default()
            });
            if !has_settings && text.contains("\"theme\"") {
                config.settings.theme_name = "custom".to_string();
            }
            if !has_custom_theme && config.settings.theme_name == "custom" {
                config.custom_theme = config.theme.clone();
            }
            normalize_config(&mut config);
            config
        }
        Err(_) => {
            let mut config = EditorConfig::default();
            normalize_config(&mut config);
            config
        }
    }
}

pub fn save_config(path: &Path, config: &EditorConfig) -> Result<(), String> {
    let text = serde_json::to_string_pretty(config)
        .map_err(|e| format!("failed to serialize config: {e}"))?;
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("failed to write {}: {e}", path.display()))
}

fn normalize_config(config: &mut EditorConfig) {
    if config.settings.theme_name.trim().is_empty() {
        config.settings.theme_name = "dark_plus".to_string();
    }
    if config.settings.theme_name == "custom" {
        config.theme = config.custom_theme.clone();
    } else if let Some(theme) = theme_preset(&config.settings.theme_name) {
        config.theme = theme;
    }
    config.settings.viewport_camera_sensitivity =
        finite_or(config.settings.viewport_camera_sensitivity, 1.0).clamp(0.05, 8.0);
    config.settings.viewport_camera_speed =
        finite_or(config.settings.viewport_camera_speed, 10.0).clamp(0.1, 1_000.0);
    config.settings.viewport_camera_fov =
        finite_or(config.settings.viewport_camera_fov, 60.0).clamp(20.0, 140.0);
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

pub(crate) fn theme_preset(name: &str) -> Option<Theme> {
    match name {
        "dark_plus" => Some(Theme::default()),
        "gruvbox_dark" => Some(Theme {
            panel: [40, 40, 40, 255],
            panel_alt: [50, 48, 47, 255],
            toolbar: [60, 56, 54, 255],
            viewport_bg: [29, 32, 33, 255],
            border: [80, 73, 69, 255],
            text: [235, 219, 178, 255],
            text_dim: [168, 153, 132, 255],
            button: [69, 133, 136, 255],
            button_hover: [104, 157, 106, 255],
            button_active: [7, 102, 120, 255],
            field: [60, 56, 54, 255],
            field_focus: [50, 48, 47, 255],
            accent: [250, 189, 47, 255],
            selection: [250, 189, 47, 255],
            danger: [251, 73, 52, 255],
            splitter: [80, 73, 69, 255],
            splitter_hover: [250, 189, 47, 255],
            header: [50, 48, 47, 255],
            grid: [235, 219, 178, 18],
            corner_radius: 4.0,
        }),
        "dracula" => Some(Theme {
            panel: [40, 42, 54, 255],
            panel_alt: [48, 50, 65, 255],
            toolbar: [33, 34, 44, 255],
            viewport_bg: [30, 31, 40, 255],
            border: [68, 71, 90, 255],
            text: [248, 248, 242, 255],
            text_dim: [191, 194, 205, 255],
            button: [98, 114, 164, 255],
            button_hover: [121, 135, 190, 255],
            button_active: [68, 71, 90, 255],
            field: [53, 55, 70, 255],
            field_focus: [68, 71, 90, 255],
            accent: [255, 184, 108, 255],
            selection: [139, 233, 253, 255],
            danger: [255, 85, 85, 255],
            splitter: [68, 71, 90, 255],
            splitter_hover: [255, 121, 198, 255],
            header: [33, 34, 44, 255],
            grid: [248, 248, 242, 18],
            corner_radius: 4.0,
        }),
        "monokai" => Some(Theme {
            panel: [39, 40, 34, 255],
            panel_alt: [48, 49, 43, 255],
            toolbar: [32, 33, 28, 255],
            viewport_bg: [28, 29, 25, 255],
            border: [73, 72, 62, 255],
            text: [248, 248, 242, 255],
            text_dim: [187, 181, 150, 255],
            button: [73, 134, 156, 255],
            button_hover: [90, 160, 184, 255],
            button_active: [73, 72, 62, 255],
            field: [55, 56, 48, 255],
            field_focus: [65, 66, 57, 255],
            accent: [253, 151, 31, 255],
            selection: [253, 151, 31, 255],
            danger: [249, 38, 114, 255],
            splitter: [73, 72, 62, 255],
            splitter_hover: [253, 151, 31, 255],
            header: [32, 33, 28, 255],
            grid: [248, 248, 242, 16],
            corner_radius: 4.0,
        }),
        "solarized_dark" => Some(Theme {
            panel: [0, 43, 54, 255],
            panel_alt: [7, 54, 66, 255],
            toolbar: [0, 33, 42, 255],
            viewport_bg: [0, 27, 34, 255],
            border: [88, 110, 117, 255],
            text: [211, 222, 224, 255],
            text_dim: [147, 166, 170, 255],
            button: [38, 139, 210, 255],
            button_hover: [42, 161, 152, 255],
            button_active: [7, 54, 66, 255],
            field: [7, 54, 66, 255],
            field_focus: [0, 43, 54, 255],
            accent: [203, 153, 0, 255],
            selection: [42, 161, 152, 255],
            danger: [220, 50, 47, 255],
            splitter: [88, 110, 117, 255],
            splitter_hover: [203, 153, 0, 255],
            header: [0, 33, 42, 255],
            grid: [211, 222, 224, 15],
            corner_radius: 4.0,
        }),
        "light_plus" => Some(Theme {
            panel: [243, 243, 243, 255],
            panel_alt: [230, 230, 230, 255],
            toolbar: [221, 221, 221, 255],
            viewport_bg: [250, 250, 250, 255],
            border: [198, 198, 198, 255],
            text: [30, 30, 30, 255],
            text_dim: [98, 98, 98, 255],
            button: [0, 122, 204, 255],
            button_hover: [17, 119, 187, 255],
            button_active: [204, 232, 255, 255],
            field: [255, 255, 255, 255],
            field_focus: [245, 245, 245, 255],
            accent: [0, 122, 204, 255],
            selection: [204, 128, 0, 255],
            danger: [196, 43, 28, 255],
            splitter: [198, 198, 198, 255],
            splitter_hover: [0, 122, 204, 255],
            header: [230, 230, 230, 255],
            grid: [0, 0, 0, 18],
            corner_radius: 4.0,
        }),
        _ => None,
    }
}

pub(crate) fn theme_label(name: &str) -> &'static str {
    match name {
        "dark_plus" => "Dark+",
        "gruvbox_dark" => "Gruvbox Dark",
        "dracula" => "Dracula",
        "monokai" => "Monokai",
        "solarized_dark" => "Solarized Dark",
        "light_plus" => "Light+",
        "custom" => "Custom",
        _ => "Custom",
    }
}

pub(crate) fn theme_presets() -> &'static [(&'static str, &'static str)] {
    &[
        ("dark_plus", "Dark+"),
        ("gruvbox_dark", "Gruvbox Dark"),
        ("dracula", "Dracula"),
        ("monokai", "Monokai"),
        ("solarized_dark", "Solarized Dark"),
        ("light_plus", "Light+"),
        ("custom", "Custom"),
    ]
}

fn parse_toml_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

fn parse_toml_string(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() < 2 || !value.starts_with('"') || !value.ends_with('"') {
        return None;
    }
    let value = &value[1..value.len() - 1];
    Some(value.replace("\\\"", "\"").replace("\\\\", "\\"))
}

fn toml_string_literal(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn default_start_scene_setting() -> String {
    super::DEFAULT_SCENE_FILE.to_string()
}

fn project_relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn resolve_project_start_scene_path(root: &Path, setting: &str) -> PathBuf {
    let setting = setting.trim();
    let path = if setting.is_empty() {
        PathBuf::from(default_start_scene_setting())
    } else {
        PathBuf::from(setting)
    };
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

fn normalize_start_scene_setting(root: &Path, setting: &str) -> Result<String, String> {
    let mut setting = setting.trim().replace('\\', "/");
    while let Some(stripped) = setting.strip_prefix("./") {
        setting = stripped.to_string();
    }
    if setting.is_empty() {
        setting = default_start_scene_setting();
    }

    let path = PathBuf::from(&setting);
    let relative = if path.is_absolute() {
        path.strip_prefix(root)
            .map_err(|_| "Start scene must be inside the project".to_string())?
            .to_path_buf()
    } else {
        path
    };
    if relative.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        )
    }) {
        return Err("Start scene must stay inside the project".to_string());
    }
    if relative.extension().is_none_or(|ext| ext != "neoscene") {
        return Err("Start scene must be a .neoscene file".to_string());
    }
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

pub(crate) fn configured_start_scene_path(root: &Path) -> PathBuf {
    let settings = load_project_window_settings(root);
    normalize_start_scene_setting(root, &settings.start_scene)
        .map(|setting| resolve_project_start_scene_path(root, &setting))
        .unwrap_or_else(|_| root.join(super::DEFAULT_SCENE_FILE))
}

fn flatpak_app_installed(app_id: &str) -> std::io::Result<bool> {
    std::process::Command::new("flatpak")
        .arg("info")
        .arg(app_id)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
}

fn load_project_window_settings(root: &Path) -> ProjectWindowSettings {
    let mut settings = ProjectWindowSettings::default();
    let Ok(text) = std::fs::read_to_string(root.join("neolove.toml")) else {
        return settings;
    };
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_ascii_lowercase();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        match (section.as_str(), key.as_str()) {
            ("project", "start_scene") => {
                if let Some(value) = parse_toml_string(value)
                    && let Ok(value) = normalize_start_scene_setting(root, &value)
                {
                    settings.start_scene = value;
                }
            }
            ("window", "width") => {
                if let Ok(value) = value.trim().parse::<f32>()
                    && value.is_finite()
                {
                    settings.width = value.clamp(1.0, 16384.0);
                }
            }
            ("window", "height") => {
                if let Ok(value) = value.trim().parse::<f32>()
                    && value.is_finite()
                {
                    settings.height = value.clamp(1.0, 16384.0);
                }
            }
            ("window", "fullscreen") => {
                if let Some(value) = parse_toml_bool(value) {
                    settings.fullscreen = value;
                }
            }
            ("window", "resizable") => {
                if let Some(value) = parse_toml_bool(value) {
                    settings.resizable = value;
                }
            }
            _ => {}
        }
    }
    settings
}

fn save_project_window_settings(
    root: &Path,
    settings: &ProjectWindowSettings,
) -> Result<(), String> {
    let path = root.join("neolove.toml");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<String> = if existing.is_empty() {
        Vec::new()
    } else {
        existing.lines().map(ToString::to_string).collect()
    };

    ensure_toml_section(&mut lines, "project");
    upsert_toml_section_key(
        &mut lines,
        "project",
        "start_scene",
        &toml_string_literal(&settings.start_scene),
    );

    ensure_toml_section(&mut lines, "window");
    for (key, value) in [
        ("width", format!("{}", settings.width.round() as i32)),
        ("height", format!("{}", settings.height.round() as i32)),
        ("fullscreen", settings.fullscreen.to_string()),
        ("resizable", settings.resizable.to_string()),
    ] {
        upsert_toml_section_key(&mut lines, "window", key, &value);
    }

    let mut out = lines.join("\n");
    out.push('\n');
    std::fs::write(&path, out)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn ensure_toml_section(lines: &mut Vec<String>, section: &str) {
    let header = format!("[{section}]");
    if lines
        .iter()
        .any(|line| line.trim().eq_ignore_ascii_case(&header))
    {
        return;
    }
    if !lines.is_empty() && lines.last().is_some_and(|line| !line.trim().is_empty()) {
        lines.push(String::new());
    }
    lines.push(header);
}

fn upsert_toml_section_key(lines: &mut Vec<String>, section: &str, key: &str, value: &str) {
    let target_header = format!("[{section}]");
    let mut in_section = false;
    let mut insert_at = lines.len();
    for (index, line) in lines.iter_mut().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_section {
                insert_at = index;
                break;
            }
            in_section = trimmed.eq_ignore_ascii_case(&target_header);
            continue;
        }
        if in_section {
            insert_at = index + 1;
            if let Some((line_key, _)) = trimmed.split_once('=')
                && line_key.trim().eq_ignore_ascii_case(key)
            {
                *line = format!("{key} = {value}");
                return;
            }
        }
    }
    lines.insert(insert_at, format!("{key} = {value}"));
}

fn format_num(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        let mut s = format!("{value:.3}");
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
        s
    }
}

fn humanize_identifier(identifier: &str) -> String {
    let mut words = Vec::<String>::new();
    let mut current = String::new();
    for character in identifier.chars() {
        if matches!(character, '_' | '-' | ' ') {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            continue;
        }
        if character.is_uppercase()
            && current
                .chars()
                .last()
                .is_some_and(|previous| previous.is_lowercase() || previous.is_ascii_digit())
        {
            words.push(std::mem::take(&mut current));
        }
        current.push(character);
    }
    if !current.is_empty() {
        words.push(current);
    }

    words
        .into_iter()
        .map(|word| {
            if word.len() > 1 && word.chars().all(|character| character.is_uppercase()) {
                return word;
            }
            let mut characters = word.to_lowercase().chars().collect::<Vec<_>>();
            if let Some(first) = characters.first_mut() {
                first.make_ascii_uppercase();
            }
            characters.into_iter().collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::ui::{Fonts, FrameInput, Painter, Ui, load_fonts};

    struct Harness {
        app: EditorApp,
        fonts: Fonts,
        w: usize,
        h: usize,
        buffer: Vec<u32>,
    }

    impl Harness {
        fn new(scene: Scene) -> Self {
            Self::with_size(scene, 1280, 760)
        }
        fn with_size(scene: Scene, w: usize, h: usize) -> Self {
            let dir =
                std::env::temp_dir().join(format!("neolove_editor_test_{}", std::process::id()));
            let _ = std::fs::create_dir_all(&dir);
            let app = EditorApp::new(
                dir.clone(),
                dir.join("scene.neoscene"),
                scene,
                EditorConfig::default(),
            );
            Self {
                app,
                fonts: load_fonts().expect("fonts"),
                w,
                h,
                buffer: vec![0u32; w * h],
            }
        }
        fn frame(&mut self, input: FrameInput) {
            let painter = Painter::new(&mut self.buffer, self.w, self.h, self.fonts.clone());
            let theme = self.app.theme();
            let mut ui = Ui::new(
                painter,
                input,
                theme,
                self.app.take_focus(),
                self.app.take_edit_buffer(),
                self.app.take_edit_cursor(),
                self.app.take_edit_selection_anchor(),
                self.app.take_pointer_capture(),
            );
            self.app.frame(&mut ui);
            let (f, e, c, a, p) = ui.into_focus_state();
            self.app.set_focus(f, e, c, a, p);
        }
        fn click(&mut self, x: f32, y: f32) {
            self.frame(FrameInput {
                mouse_x: x,
                mouse_y: y,
                mouse_pressed: true,
                mouse_down: true,
                ..Default::default()
            });
            self.frame(FrameInput {
                mouse_x: x,
                mouse_y: y,
                ..Default::default()
            });
        }
        fn prop_row_frame(
            &mut self,
            prop: &mut Prop,
            input: FrameInput,
            x: f32,
            width: f32,
            start_y: f32,
        ) -> (bool, f32) {
            let painter = Painter::new(&mut self.buffer, self.w, self.h, self.fonts.clone());
            let theme = self.app.theme();
            let mut ui = Ui::new(
                painter,
                input,
                theme,
                self.app.take_focus(),
                self.app.take_edit_buffer(),
                self.app.take_edit_cursor(),
                self.app.take_edit_selection_anchor(),
                self.app.take_pointer_capture(),
            );
            let mut y = start_y;
            let dirty = self.app.prop_row(&mut ui, 1, 0, 0, prop, x, width, &mut y);
            let (focus, edit, cursor, anchor, pointer) = ui.into_focus_state();
            self.app.set_focus(focus, edit, cursor, anchor, pointer);
            (dirty, y)
        }
    }

    fn unique_test_path(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!(
            "neolove_editor_{label}_{}_{}",
            std::process::id(),
            nanos
        ))
    }

    #[test]
    fn default_scene_has_no_components() {
        let h = Harness::new(Scene::default());
        assert!(h.app.scene.entities[0].components.is_empty());
    }

    #[test]
    fn editor_preview_gathers_scene_lights_when_enabled() {
        let mut scene = Scene::default();
        scene.lighting.enabled = true;
        scene.lighting.ambient = [0, 0, 0, 255];
        scene.lighting.ambient_intensity = 0.0;
        let mut torch = scene.add_entity("Torch", 200.0, 150.0);
        torch.components.push(Component::core("Light2D"));
        let id = torch.id;
        scene.replace_entity(id, torch);

        let mut h = Harness::new(scene);
        let (config, lights, occluders) =
            h.app.gather_scene_lighting().expect("lighting is enabled");
        let sampler = crate::lighting::LightSampler::new(&config, &lights, &occluders);
        // The light illuminates its own position but not points beyond its reach.
        let (near, _, _) = sampler.sample(200.0, 150.0);
        assert!(near > 0.5, "light should brighten its position, got {near}");
        let (far, _, _) = sampler.sample(200.0 + 5000.0, 150.0);
        assert!(far < 0.01, "beyond the radius should stay dark, got {far}");

        // The editor preference suppresses only the viewport preview; it does
        // not mutate the scene's authored lighting settings.
        h.app.config.settings.preview_lighting = false;
        assert!(h.app.gather_scene_lighting().is_none());
        assert!(h.app.scene.lighting.enabled);

        // A scene with lighting disabled produces no preview at all.
        let mut off = Scene::default();
        off.lighting.enabled = false;
        let h2 = Harness::new(off);
        assert!(h2.app.gather_scene_lighting().is_none());
    }

    #[test]
    fn lighting_preview_skips_transform_work_for_ordinary_entities() {
        let mut scene = Scene::default();
        scene.lighting.enabled = true;
        for index in 0..256 {
            let mut entity = scene.add_entity(format!("Sprite {index}"), index as f32, 0.0);
            entity.components.push(Component::core("Rect2D"));
            let id = entity.id;
            scene.replace_entity(id, entity);
        }
        let mut light = scene.add_entity("Light", 32.0, 48.0);
        light.components.push(Component::core("Light2D"));
        let light_id = light.id;
        scene.replace_entity(light_id, light);

        let h = Harness::new(scene);
        let (_, lights, _) = h.app.gather_scene_lighting().expect("lighting preview");
        assert_eq!(lights.len(), 1);
        assert!(
            h.app.world_transform_cache.borrow().len() <= 1,
            "ordinary entities should not populate the transform cache"
        );
    }

    #[test]
    fn preview_light_grid_caches_and_tracks_camera() {
        let mut scene = Scene::default();
        scene.lighting.enabled = true;
        scene.lighting.ambient = [0, 0, 0, 255];
        scene.lighting.ambient_intensity = 0.0;
        let mut torch = scene.add_entity("Torch", 200.0, 150.0);
        torch.components.push(Component::core("Light2D"));
        let id = torch.id;
        scene.replace_entity(id, torch);

        let mut h = Harness::new(scene);
        h.frame(FrameInput::default());
        {
            let cache = h.app.preview_light_cache.borrow();
            let cached = cache.as_ref().expect("grid cached after a lit frame");
            assert!(!cached.grid.is_empty(), "grid should be populated");
            assert_eq!(cached.cam_zoom, h.app.cam_zoom);
        }

        // Zooming must invalidate the cached grid, or the preview would show
        // lighting at the wrong scale. The key tracks the live camera zoom.
        h.app.cam_zoom += 0.5;
        h.frame(FrameInput::default());
        let cache = h.app.preview_light_cache.borrow();
        let cached = cache.as_ref().expect("grid rebuilt after zoom");
        assert_eq!(cached.cam_zoom, h.app.cam_zoom, "cache tracks current zoom");
    }

    #[test]
    fn attached_value_types_can_be_changed_at_the_root_and_inside_tables() {
        let mut scene = Scene::default();
        let entity = scene.entities[0].id;
        scene.entity_mut(entity).expect("entity").values = vec![AttachedValue {
            name: "payload".into(),
            value: VarValue::List(vec![VarValue::Number(7.0)]),
        }];
        let mut h = Harness::new(scene);

        h.app.perform(Action::SetAttachedValueType {
            entity,
            value: 0,
            path: vec![VarPathPart::List(0)],
            kind: AttachedValueType::String,
        });
        assert!(matches!(
            &h.app.scene.entity(entity).expect("entity").values[0].value,
            VarValue::List(values) if values == &vec![VarValue::Text(String::new())]
        ));

        h.app.perform(Action::SetAttachedValueType {
            entity,
            value: 0,
            path: Vec::new(),
            kind: AttachedValueType::Table,
        });
        assert!(matches!(
            h.app.scene.entity(entity).expect("entity").values[0].value,
            VarValue::Dictionary(ref entries) if entries.is_empty()
        ));
        assert!(h.app.scene_dirty);
    }

    #[test]
    fn config_loading_prefers_global_and_preserves_legacy_custom_theme() {
        let dir = unique_test_path("config");
        let global = dir.join("global").join("editor.json");
        let legacy = dir.join("legacy").join("editor.json");
        std::fs::create_dir_all(legacy.parent().expect("legacy parent")).expect("legacy dir");
        std::fs::write(
            &legacy,
            r#"{"theme":{"panel":[1,2,3,255],"button":[4,5,6,255]},"layout":{"grid":24}}"#,
        )
        .expect("write legacy config");

        let config = load_config_with_fallback(&global, &legacy);
        assert_eq!(config.settings.theme_name, "custom");
        assert_eq!(config.theme.panel, [1, 2, 3, 255]);
        assert_eq!(config.theme.button, [4, 5, 6, 255]);
        assert_eq!(config.custom_theme.panel, [1, 2, 3, 255]);
        assert_eq!(config.layout.grid, 24.0);

        std::fs::create_dir_all(global.parent().expect("global parent")).expect("global dir");
        std::fs::write(
            &global,
            r#"{"custom_theme":{"panel":[9,8,7,255]},"settings":{"theme_name":"gruvbox_dark"}}"#,
        )
        .expect("write global config");
        let config = load_config_with_fallback(&global, &legacy);
        assert_eq!(config.settings.theme_name, "gruvbox_dark");
        assert_eq!(config.theme.panel, [40, 40, 40, 255]);
        assert_eq!(config.custom_theme.panel, [9, 8, 7, 255]);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn viewport_camera_settings_round_trip_and_are_clamped() {
        let dir = unique_test_path("viewport_camera_config");
        let path = dir.join("editor.json");
        let mut config = EditorConfig::default();
        config.settings.viewport_camera_sensitivity = 2.25;
        config.settings.viewport_camera_speed = 48.0;
        config.settings.viewport_camera_fov = 82.0;
        config.settings.viewport_invert_mouse_look = true;
        save_config(&path, &config).expect("save viewport camera config");

        let loaded = load_config(&path);
        assert_eq!(loaded.settings.viewport_camera_sensitivity, 2.25);
        assert_eq!(loaded.settings.viewport_camera_speed, 48.0);
        assert_eq!(loaded.settings.viewport_camera_fov, 82.0);
        assert!(loaded.settings.viewport_invert_mouse_look);

        std::fs::write(
            &path,
            r#"{"settings":{"viewport_camera_sensitivity":99.0,"viewport_camera_speed":0.0,"viewport_camera_fov":180.0}}"#,
        )
        .expect("write out-of-range viewport camera config");
        let clamped = load_config(&path);
        assert_eq!(clamped.settings.viewport_camera_sensitivity, 8.0);
        assert_eq!(clamped.settings.viewport_camera_speed, 0.1);
        assert_eq!(clamped.settings.viewport_camera_fov, 140.0);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn project_window_settings_preserve_resizable() {
        let dir = unique_test_path("project_window");
        std::fs::create_dir_all(&dir).expect("create project dir");
        std::fs::write(
            dir.join("neolove.toml"),
            "[project]\nstart_scene = \"levels/title.neoscene\"\n\n[window]\nwidth = 900\nheight = 500\nfullscreen = false\nresizable = false\n",
        )
        .expect("write project settings");

        let loaded = load_project_window_settings(&dir);
        assert_eq!(loaded.start_scene, "levels/title.neoscene");
        assert_eq!(loaded.width, 900.0);
        assert_eq!(loaded.height, 500.0);
        assert!(!loaded.fullscreen);
        assert!(!loaded.resizable);

        let updated = ProjectWindowSettings {
            start_scene: "scene.neoscene".to_string(),
            width: 1024.0,
            height: 768.0,
            fullscreen: true,
            resizable: true,
        };
        save_project_window_settings(&dir, &updated).expect("save project settings");
        let saved = std::fs::read_to_string(dir.join("neolove.toml")).expect("read saved settings");
        assert!(saved.contains("start_scene = \"scene.neoscene\""));
        assert!(saved.contains("resizable = true"));
        assert!(load_project_window_settings(&dir).resizable);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn export_luau_uses_project_start_scene() {
        let dir = unique_test_path("project_start_scene_export");
        let levels = dir.join("levels");
        std::fs::create_dir_all(&levels).expect("create project dir");
        std::fs::write(
            dir.join("neolove.toml"),
            "[project]\nstart_scene = \"levels/title.neoscene\"\n",
        )
        .expect("write project settings");

        let mut start_scene = Scene::default();
        start_scene.name = "Configured Start".to_string();
        start_scene.entities[0].name = "ConfiguredStartEntity".to_string();
        start_scene
            .save(&levels.join("title.neoscene"))
            .expect("save start scene");

        let mut active_scene = Scene::default();
        active_scene.name = "Active Scene".to_string();
        active_scene.entities[0].name = "ActiveSceneEntity".to_string();

        let mut app = EditorApp::new(
            dir.clone(),
            dir.join("scene.neoscene"),
            active_scene,
            EditorConfig::default(),
        );
        assert!(app.export_luau());
        // main.luau is now a loader that points at the configured start scene,
        // not the active scene, and inlines no entity construction.
        let luau = std::fs::read_to_string(dir.join("main.luau")).expect("read exported main");
        assert!(luau.contains("ecs.loadScene(\"levels/title.neoscene\")"));
        assert!(!luau.contains("ConfiguredStartEntity"));
        assert!(!luau.contains("ActiveSceneEntity"));
        assert!(!luau.contains("AddComponent"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn export_luau_never_overwrites_a_user_authored_entry_point() {
        let dir = unique_test_path("protect_user_main");
        std::fs::create_dir_all(&dir).expect("create project dir");
        let source = "local score = 42\nprint(score)\n";
        std::fs::write(dir.join("main.luau"), source).expect("write user entry point");

        let mut app = EditorApp::new(
            dir.clone(),
            dir.join("scene.neoscene"),
            Scene::default(),
            EditorConfig::default(),
        );
        assert!(!app.export_luau());
        assert_eq!(
            std::fs::read_to_string(dir.join("main.luau")).expect("read protected entry point"),
            source
        );
        assert!(!dir.join("scene.neoscene").exists());
        assert!(app.status.contains("user-authored code"));
        assert!(app.status.contains("left unchanged"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn export_luau_can_refresh_an_editor_generated_entry_point() {
        let dir = unique_test_path("refresh_generated_main");
        std::fs::create_dir_all(&dir).expect("create project dir");
        std::fs::write(
            dir.join("main.luau"),
            "-- Generated by the NeoLOVE visual editor. Edits may be overwritten.\nold\n",
        )
        .expect("write generated entry point");

        let mut app = EditorApp::new(
            dir.clone(),
            dir.join("scene.neoscene"),
            Scene::default(),
            EditorConfig::default(),
        );
        assert!(app.export_luau());
        let source =
            std::fs::read_to_string(dir.join("main.luau")).expect("read refreshed entry point");
        assert!(source.contains("ecs.loadScene(\"scene.neoscene\")"));
        assert!(!source.contains("old"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn save_config_accepts_plain_relative_file_paths() {
        let name = format!(
            ".neolove_editor_config_{}_{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        );
        let path = PathBuf::from(&name);
        save_config(&path, &EditorConfig::default()).expect("save config");
        assert!(path.exists());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn docking_actions_can_redock_detached_widgets() {
        let mut h = Harness::new(Scene::default());
        h.app.config.layout.show_inspector = true;
        h.app.config.layout.undock_inspector = true;
        h.app.dirty = false;
        h.app.dock_widget(EditorWidget::Inspector);
        assert!(h.app.config.layout.show_inspector);
        assert!(!h.app.config.layout.undock_inspector);
        assert!(h.app.dirty);

        h.app.config.layout.show_project = true;
        h.app.config.layout.undock_project = true;
        h.app.perform(Action::ToggleProject);
        assert!(!h.app.config.layout.show_project);
        assert!(!h.app.config.layout.undock_project);
    }

    #[test]
    fn enum_dropdown_actions_update_scene_and_component_values() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        scene
            .entity_mut(id)
            .expect("entity")
            .components
            .push(Component::core("Shape2D"));
        let mut h = Harness::new(scene);

        h.app
            .perform(Action::SetSceneAntialiasing("standard".to_string()));
        assert_eq!(h.app.scene.antialiasing, "standard");
        assert!(h.app.scene_dirty);
        h.app.scene_dirty = false;

        h.app
            .perform(Action::SetSceneAntialiasing("invalid".to_string()));
        assert_eq!(h.app.scene.antialiasing, "standard");
        assert!(!h.app.scene_dirty);

        let shape_prop = match &h.app.scene.entity(id).expect("entity").components[0] {
            Component::Core { props, .. } => props
                .iter()
                .position(|prop| prop.name == "shape")
                .expect("shape prop"),
            _ => panic!("core component"),
        };
        h.app.perform(Action::SetPropEnum {
            entity: id,
            component: 0,
            prop: shape_prop,
            value: "circle".to_string(),
        });
        let Component::Core { props, .. } = &h.app.scene.entity(id).expect("entity").components[0]
        else {
            panic!("core component");
        };
        assert!(matches!(
            &props[shape_prop].value,
            PropValue::Enum { value, .. } if value == "circle"
        ));
    }

    #[test]
    fn dropdown_option_editor_adds_edits_reorders_and_deletes_items() {
        let mut harness = Harness::new(Scene::default());
        let mut prop = Prop {
            name: "options".into(),
            label: "Options".into(),
            value: PropValue::StringList(Vec::new()),
            advanced: false,
            optional: false,
        };
        let x = 20.0;
        let width = 280.0;
        let start_y = 20.0;

        // Empty state: header (24), gap (3), message (22), then Add Option.
        let (dirty, _) = harness.prop_row_frame(
            &mut prop,
            FrameInput {
                mouse_x: 100.0,
                mouse_y: 80.0,
                mouse_pressed: true,
                mouse_down: true,
                ..Default::default()
            },
            x,
            width,
            start_y,
        );
        assert!(dirty);
        assert!(matches!(
            &prop.value,
            PropValue::StringList(values)
                if values.iter().map(String::as_str).eq([""].into_iter())
        ));

        // The newly appended field is focused automatically.
        let (dirty, _) = harness.prop_row_frame(
            &mut prop,
            FrameInput {
                typed: "Play".into(),
                ..Default::default()
            },
            x,
            width,
            start_y,
        );
        assert!(dirty);

        // With one item, Add Option starts at y=72.
        harness.prop_row_frame(
            &mut prop,
            FrameInput {
                mouse_x: 100.0,
                mouse_y: 82.0,
                mouse_pressed: true,
                mouse_down: true,
                ..Default::default()
            },
            x,
            width,
            start_y,
        );
        harness.prop_row_frame(
            &mut prop,
            FrameInput {
                typed: "Quit".into(),
                ..Default::default()
            },
            x,
            width,
            start_y,
        );

        // Move the second item up, then delete the first item.
        harness.prop_row_frame(
            &mut prop,
            FrameInput {
                mouse_x: 244.0,
                mouse_y: 82.0,
                mouse_pressed: true,
                mouse_down: true,
                ..Default::default()
            },
            x,
            width,
            start_y,
        );
        assert!(matches!(
            &prop.value,
            PropValue::StringList(values)
                if values.iter().map(String::as_str).eq(["Quit", "Play"].into_iter())
        ));
        harness.prop_row_frame(
            &mut prop,
            FrameInput {
                mouse_x: 288.0,
                mouse_y: 58.0,
                mouse_pressed: true,
                mouse_down: true,
                ..Default::default()
            },
            x,
            width,
            start_y,
        );
        assert!(matches!(
            &prop.value,
            PropValue::StringList(values)
                if values.iter().map(String::as_str).eq(["Play"].into_iter())
        ));
    }

    #[test]
    fn inspector_identifiers_are_humanized() {
        assert_eq!(humanize_identifier("per_second"), "Per Second");
        assert_eq!(humanize_identifier("isEnabled"), "Is Enabled");
        assert_eq!(humanize_identifier("max_FPS"), "Max FPS");
    }

    #[test]
    fn asset_picker_filters_supported_types_and_assigns_typed_paths() {
        assert!(AssetKind::Image.accepts(Path::new("sprite.PNG")));
        assert!(AssetKind::Font.accepts(Path::new("ui.otf")));
        assert!(AssetKind::Sound.accepts(Path::new("music.flac")));
        assert!(AssetKind::Mesh.accepts(Path::new("models/robot.FBX")));
        assert!(AssetKind::Mesh.accepts(Path::new("models/level.glb")));
        assert!(AssetKind::Shader.accepts(Path::new("glow.GLSL")));
        assert!(!AssetKind::Sound.accepts(Path::new("notes.txt")));

        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        scene
            .entity_mut(id)
            .expect("entity")
            .components
            .push(Component::core("SpatialSound2D"));
        scene.entity_mut(id).expect("entity").values = vec![
            AttachedValue {
                name: "gallery".into(),
                value: VarValue::List(vec![VarValue::Image(String::new())]),
            },
            AttachedValue {
                name: "accent".into(),
                value: VarValue::Color([0, 0, 0, 255]),
            },
        ];
        let mut harness = Harness::new(scene);
        harness.app.assign_asset(
            AssetTarget::Prop {
                entity: id,
                component: 0,
                prop: 0,
            },
            AssetKind::Sound,
            "assets/effect.wav".into(),
        );
        let Component::Core { props, .. } = &harness.app.scene.entities[0].components[0] else {
            unreachable!()
        };
        assert_eq!(props[0].value, PropValue::Sound("assets/effect.wav".into()));

        harness.app.assign_asset(
            AssetTarget::AttachedValue {
                entity: id,
                value: 0,
                path: vec![VarPathPart::List(0)],
            },
            AssetKind::Image,
            "assets/portrait.png".into(),
        );
        assert!(matches!(
            &harness.app.scene.entity(id).expect("entity").values[0].value,
            VarValue::List(values)
                if values == &vec![VarValue::Image("assets/portrait.png".into())]
        ));
        harness.app.set_target_color(
            &ColorTarget::AttachedValue {
                entity: id,
                value: 1,
                path: Vec::new(),
            },
            [12, 34, 56, 78],
        );
        assert_eq!(
            harness.app.scene.entity(id).expect("entity").values[1].value,
            VarValue::Color([12, 34, 56, 78])
        );
        assert!(harness.app.scene_dirty);
    }

    #[test]
    fn running_flushes_unsaved_background_scene_documents() {
        // A scene the game loads with `loadScene` (not the active tab) must be
        // written to disk before running, or its unsaved edits — like a freshly
        // assigned IAudio script variable — are lost and `audio.play(nil)`
        // crashes the game at runtime.
        use crate::scene::{Component, ScriptVar, VarControl};
        let mut harness = Harness::new(Scene::default());
        let root = harness.app.project_root.clone();
        let level_path = root.join("level_bg.neoscene");

        // Stale on-disk version: the audio variable is unset.
        let mut disk = Scene::default();
        disk.entities[0].components.push(Component::Script {
            path: "scripts/Jump.luau".into(),
            variables: vec![ScriptVar {
                name: "sfx".into(),
                value: VarValue::Audio(String::new()),
                control: VarControl::Field,
            }],
        });
        disk.save(&level_path).expect("write stale scene");

        // A background document holding the assignment, not yet saved to disk.
        let mut edited = disk.clone();
        if let Component::Script { variables, .. } = &mut edited.entities[0].components[0] {
            variables[0].value = VarValue::Audio("assets/jump.wav".into());
        }
        harness.app.documents.push(OpenDocument {
            path: level_path.clone(),
            scene: edited,
            kind: DocumentKind::Scene,
            dirty: true,
        });

        assert!(harness.app.save_all_open_documents());

        let reloaded = Scene::load(&level_path).expect("reload scene");
        let Component::Script { variables, .. } = &reloaded.entities[0].components[0] else {
            panic!("not a script component");
        };
        assert_eq!(
            variables[0].value,
            VarValue::Audio("assets/jump.wav".into()),
            "background scene edits were not flushed to disk before running"
        );
    }

    #[test]
    fn particle_sequences_interpolate_at_authored_times() {
        let colors = vec![
            ColorKeypoint {
                time: 0.0,
                color: [0, 20, 40, 255],
            },
            ColorKeypoint {
                time: 0.5,
                color: [100, 120, 140, 255],
            },
            ColorKeypoint {
                time: 1.0,
                color: [200, 220, 240, 255],
            },
        ];
        let transparency = vec![
            NumberKeypoint {
                time: 0.0,
                value: 0.0,
            },
            NumberKeypoint {
                time: 0.5,
                value: 0.25,
            },
            NumberKeypoint {
                time: 1.0,
                value: 1.0,
            },
        ];
        assert_eq!(sample_color_sequence(&colors, 0.25), [50, 70, 90, 255]);
        assert!((sample_number_sequence(&transparency, 0.75) - 0.625).abs() < 0.0001);
        assert_eq!(largest_color_gap_midpoint(&colors), 0.75);
    }

    #[test]
    fn inspector_reference_fields_accept_entity_and_component_drops() {
        let mut scene = Scene::default();
        let owner = scene.entities[0].id;
        let mut source = scene.add_entity("Source", 0.0, 0.0);
        let source_id = source.id;
        source.components.push(Component::core("Rect2D"));
        scene.replace_entity(source_id, source);
        let mut h = Harness::new(scene);

        let x = 900.0;
        let width = 300.0;
        let field_x = x + LABEL_W + 4.0;

        let mut entity_value = VarValue::Entity(None);
        h.app.inspector_reference_drag = Some(InspectorReferenceDrag::Entity {
            id: source_id,
            inspector_owner: Some(owner),
        });
        {
            let painter = Painter::new(&mut h.buffer, h.w, h.h, h.fonts.clone());
            let mut ui = Ui::new(
                painter,
                FrameInput {
                    mouse_x: field_x,
                    mouse_y: 104.0,
                    ..Default::default()
                },
                h.app.theme(),
                None,
                String::new(),
                0,
                None,
                None,
            );
            let mut y = 100.0;
            let mut dirty = false;
            h.app.script_value_editor(
                &mut ui,
                "entity_ref",
                "Entity",
                &mut entity_value,
                &VarControl::Field,
                ValueOwner::ScriptVar {
                    entity: owner,
                    component: 0,
                    var: 0,
                },
                &mut Vec::new(),
                x,
                width,
                &mut y,
                &mut dirty,
            );
            assert!(dirty);
        }
        assert_eq!(entity_value, VarValue::Entity(Some(source_id)));

        let mut component_value = VarValue::Component(None);
        h.app.inspector_reference_drag =
            Some(InspectorReferenceDrag::Component(ComponentReference {
                entity: source_id,
                component: 0,
            }));
        {
            let painter = Painter::new(&mut h.buffer, h.w, h.h, h.fonts.clone());
            let mut ui = Ui::new(
                painter,
                FrameInput {
                    mouse_x: field_x,
                    mouse_y: 104.0,
                    ..Default::default()
                },
                h.app.theme(),
                None,
                String::new(),
                0,
                None,
                None,
            );
            let mut y = 100.0;
            let mut dirty = false;
            h.app.script_value_editor(
                &mut ui,
                "component_ref",
                "Component",
                &mut component_value,
                &VarControl::Field,
                ValueOwner::ScriptVar {
                    entity: owner,
                    component: 0,
                    var: 0,
                },
                &mut Vec::new(),
                x,
                width,
                &mut y,
                &mut dirty,
            );
            assert!(dirty);
        }
        assert_eq!(
            component_value,
            VarValue::Component(Some(ComponentReference {
                entity: source_id,
                component: 0,
            }))
        );
    }

    #[test]
    fn renders_without_panicking() {
        let mut h = Harness::new(Scene::default());
        h.frame(FrameInput::default());
        let first = h.buffer[0];
        assert!(h.buffer.iter().any(|&p| p != first));
    }

    #[test]
    fn renders_rotated_selected_entity_without_panicking() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        {
            let entity = scene.entity_mut(id).expect("entity");
            entity.x = 500.0;
            entity.y = 300.0;
            entity.size_x = 120.0;
            entity.size_y = 80.0;
            entity.rotation = 0.6;
            entity.components.push(Component::Core {
                name: "Rect2D".into(),
                props: vec![Prop {
                    name: "color".into(),
                    label: "Color".into(),
                    value: PropValue::Color([200, 120, 80, 255]),
                    advanced: false,
                    optional: false,
                }],
            });
        }
        let mut h = Harness::new(scene);
        h.app.selected = Some(id);
        // Exercises the rotated fill + gizmo + selection-handle draw paths.
        h.frame(FrameInput::default());
        let first = h.buffer[0];
        assert!(h.buffer.iter().any(|&p| p != first));
    }

    #[test]
    fn renders_shape2d_variants_without_panicking() {
        for shape in ["box", "circle", "triangle"] {
            let mut scene = Scene::default();
            let id = scene.entities[0].id;
            {
                let entity = scene.entity_mut(id).expect("entity");
                entity.x = 400.0;
                entity.y = 300.0;
                entity.size_x = 120.0;
                entity.size_y = 90.0;
                entity.rotation = 0.4;
                entity.components.push(Component::Core {
                    name: "Shape2D".into(),
                    props: vec![
                        Prop {
                            name: "color".into(),
                            label: "Color".into(),
                            value: PropValue::Color([120, 200, 160, 255]),
                            advanced: false,
                            optional: false,
                        },
                        Prop {
                            name: "shape".into(),
                            label: "Shape".into(),
                            value: PropValue::Enum {
                                value: shape.into(),
                                options: vec!["box".into(), "circle".into(), "triangle".into()],
                            },
                            advanced: false,
                            optional: false,
                        },
                    ],
                });
            }
            let mut h = Harness::new(scene);
            h.frame(FrameInput::default());
            let first = h.buffer[0];
            assert!(
                h.buffer.iter().any(|&p| p != first),
                "shape {shape} should draw something"
            );
        }
    }

    #[test]
    fn renders_particle_system_preview_without_panicking() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        let entity = scene.entity_mut(id).expect("entity");
        entity.x = 400.0;
        entity.y = 350.0;
        entity.components.push(Component::core("ParticleSystem2D"));
        let mut harness = Harness::new(scene);
        harness.frame(FrameInput::default());
        let background = harness.buffer[0];
        assert!(harness.buffer.iter().any(|pixel| *pixel != background));
    }

    #[test]
    fn add_entity_via_toolbar() {
        let mut h = Harness::new(Scene::default());
        let before = h.app.scene.entities.len();
        h.click(250.0, 20.0);
        assert_eq!(h.app.scene.entities.len(), before + 1);
    }

    #[test]
    fn compact_toolbar_scene_menu_contains_low_frequency_actions() {
        let mut h = Harness::new(Scene::default());
        h.click(20.0, 20.0);
        let Popup::Menu { items, .. } = h.app.popup.as_ref().expect("scene menu should open")
        else {
            panic!("expected scene menu");
        };
        let labels = items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>();
        assert!(labels.contains(&"Export Luau"));
        assert!(labels.contains(&"Build Project…"));
        assert!(labels.contains(&"Editor Settings…"));
    }

    #[test]
    fn right_click_opens_context_menu_then_closes() {
        let mut h = Harness::new(Scene::default());
        h.app.selected = Some(h.app.scene.entities[0].id);
        // Right-click in the viewport center.
        h.frame(FrameInput {
            mouse_x: 600.0,
            mouse_y: 400.0,
            right_pressed: true,
            ..Default::default()
        });
        assert!(h.app.popup.is_some());
        // Escape closes it.
        h.frame(FrameInput {
            escape: true,
            ..Default::default()
        });
        assert!(h.app.popup.is_none());
    }

    #[test]
    fn popup_survives_mouse_move_after_opening() {
        let mut h = Harness::new(Scene::default());
        let id = h.app.scene.entities[0].id;
        // Open a menu the way a click would (mid-frame), with a press present.
        h.app.open_entity_menu(id, 600.0, 300.0);
        h.frame(FrameInput {
            mouse_x: 600.0,
            mouse_y: 300.0,
            mouse_pressed: true,
            mouse_down: true,
            ..Default::default()
        });
        assert!(h.app.popup.is_some(), "menu closed on the frame it opened");
        // A subsequent mouse-move (no press) must not close it.
        h.frame(FrameInput {
            mouse_x: 620.0,
            mouse_y: 320.0,
            ..Default::default()
        });
        assert!(h.app.popup.is_some(), "menu closed after a mouse move");
    }

    #[test]
    fn drag_selects_multiple_viewport_entities() {
        let mut scene = Scene::default();
        let first = scene.entities[0].id;
        let second = scene.add_entity("B", 340.0, 150.0).id;
        let mut h = Harness::new(scene);

        // Start in empty viewport space just above the entities, drag across both.
        h.frame(FrameInput {
            mouse_x: 420.0,
            mouse_y: 170.0,
            mouse_pressed: true,
            mouse_down: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: 705.0,
            mouse_y: 315.0,
            mouse_down: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: 705.0,
            mouse_y: 315.0,
            ..Default::default()
        });

        let selected = h.app.selection_ids_ordered();
        assert!(
            selected.contains(&first),
            "first entity was not marquee-selected"
        );
        assert!(
            selected.contains(&second),
            "second entity was not marquee-selected"
        );
    }

    #[test]
    fn dragging_selected_entity_moves_selection_group() {
        let mut scene = Scene::default();
        let first = scene.entities[0].id;
        let second = scene.add_entity("B", 340.0, 150.0).id;
        let mut h = Harness::new(scene);
        h.app.config.layout.snap = false;
        h.app.select_many(vec![first, second], false);
        h.app.selected = Some(first);

        h.frame(FrameInput {
            mouse_x: 450.0,
            mouse_y: 200.0,
            mouse_pressed: true,
            mouse_down: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: 490.0,
            mouse_y: 200.0,
            mouse_down: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: 490.0,
            mouse_y: 200.0,
            ..Default::default()
        });

        let first_entity = h.app.scene.entity(first).expect("first");
        let second_entity = h.app.scene.entity(second).expect("second");
        assert!(
            (first_entity.x - 240.0).abs() < 1.0,
            "first x was {}",
            first_entity.x
        );
        assert!(
            (second_entity.x - 380.0).abs() < 1.0,
            "second x was {}",
            second_entity.x
        );
    }

    /// World-space centre of an entity (its origin plus the rotated half-size).
    fn world_center(app: &EditorApp, id: u64) -> (f32, f32) {
        let t = app.entity_world_transform(id).expect("transform");
        let e = app.scene.entity(id).expect("entity");
        let (sx, sy) = editor_entity_size(&app.scene, e, app.preview_root_size());
        let (hw, hh) = (sx * t.scale / 2.0, sy * t.scale / 2.0);
        let (sin, cos) = (t.rotation.sin(), t.rotation.cos());
        (t.x + hw * cos - hh * sin, t.y + hw * sin + hh * cos)
    }

    #[test]
    fn rotate_gizmo_pivots_about_entity_center() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        {
            let e = scene.entity_mut(id).expect("entity");
            e.x = 200.0;
            e.y = 150.0;
            e.size_x = 120.0;
            e.size_y = 80.0;
            e.rotation = 0.0;
        }
        let mut h = Harness::new(scene);
        h.app.config.layout.snap = false;
        h.app.config.layout.view_tool = ViewTool::Rotate;
        h.app.select_only(id);

        // Render once so the viewport rect and camera are populated.
        h.frame(FrameInput::default());
        let area = h.app.last_viewport;
        let e = h.app.scene.entity(id).expect("entity");
        let rect = h.app.entity_screen_rect(e, area).expect("rect");
        let (kx, ky) = h.app.rotate_handle_knob(rect, 0.0);
        let (cx, cy) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
        let center_before = world_center(&h.app, id);

        // Grab the knob, then swing the cursor out to the entity's right side.
        h.frame(FrameInput {
            mouse_x: kx,
            mouse_y: ky,
            mouse_pressed: true,
            mouse_down: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: cx + 80.0,
            mouse_y: cy,
            mouse_down: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: cx + 80.0,
            mouse_y: cy,
            ..Default::default()
        });

        let after = h.app.scene.entity(id).expect("entity");
        assert!(
            after.rotation.abs() > 0.1,
            "entity did not rotate: {}",
            after.rotation
        );
        let center_after = world_center(&h.app, id);
        assert!(
            (center_after.0 - center_before.0).abs() < 1.0,
            "center x drifted {} -> {}",
            center_before.0,
            center_after.0
        );
        assert!(
            (center_after.1 - center_before.1).abs() < 1.0,
            "center y drifted {} -> {}",
            center_before.1,
            center_after.1
        );
    }

    #[test]
    fn editor_world_transform_applies_entity_pivots() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        {
            let e = scene.entity_mut(id).expect("entity");
            e.x = 50.0;
            e.y = 25.0;
            e.size_x = 100.0;
            e.size_y = 50.0;
            e.position_pivot = "center".to_string();
        }

        let transform = scene_world_transform(&scene, id, (1280.0, 720.0)).expect("transform");
        assert!((transform.x - 0.0).abs() < 0.001, "x was {}", transform.x);
        assert!((transform.y - 0.0).abs() < 0.001, "y was {}", transform.y);

        {
            let e = scene.entity_mut(id).expect("entity");
            e.x = 0.0;
            e.y = 0.0;
            e.position_pivot.clear();
            e.rotation_pivot = "center".to_string();
            e.rotation = std::f32::consts::FRAC_PI_2;
        }

        let transform = scene_world_transform(&scene, id, (1280.0, 720.0)).expect("transform");
        assert!((transform.x - 75.0).abs() < 0.001, "x was {}", transform.x);
        assert!((transform.y + 25.0).abs() < 0.001, "y was {}", transform.y);
        let (local_x, local_y) = scene_world_origin_to_local_position(
            &scene,
            id,
            transform.x,
            transform.y,
            (1280.0, 720.0),
        )
        .expect("local position");
        assert!((local_x - 0.0).abs() < 0.001, "local x was {}", local_x);
        assert!((local_y - 0.0).abs() < 0.001, "local y was {}", local_y);
    }

    #[test]
    fn editor_world_transform_orbits_children_about_rotated_parent() {
        // Mirror the runtime's `get_global_transform`: a child's local offset
        // must be rotated by the parent's world rotation so children orbit the
        // parent's rotation pivot instead of staying axis-aligned.
        let mut scene = Scene::default();
        let parent_id = scene.entities[0].id;
        {
            let parent = scene.entity_mut(parent_id).expect("parent");
            parent.x = 100.0;
            parent.y = 100.0;
            parent.size_x = 100.0;
            parent.size_y = 100.0;
            parent.rotation_pivot = "center".to_string();
            parent.rotation = std::f32::consts::FRAC_PI_2;
        }
        let child_id = scene.add_entity("Child", 30.0, 0.0).id;
        {
            let child = scene.entity_mut(child_id).expect("child");
            child.parent = Some(parent_id);
            child.size_x = 20.0;
            child.size_y = 20.0;
        }

        let transform =
            scene_world_transform(&scene, child_id, (1280.0, 720.0)).expect("child transform");
        // Parent centre is (150,150); child origin (130,100) is (-20,-50) from it,
        // which rotates by +90deg to (50,-20), giving world (200,130).
        assert!((transform.x - 200.0).abs() < 0.01, "x was {}", transform.x);
        assert!((transform.y - 130.0).abs() < 0.01, "y was {}", transform.y);

        let (local_x, local_y) = scene_world_origin_to_local_position(
            &scene,
            child_id,
            transform.x,
            transform.y,
            (1280.0, 720.0),
        )
        .expect("local round-trip");
        assert!((local_x - 30.0).abs() < 0.01, "local x was {}", local_x);
        assert!((local_y - 0.0).abs() < 0.01, "local y was {}", local_y);
    }

    #[test]
    fn move_gizmo_x_arrow_constrains_to_x_axis() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        {
            let e = scene.entity_mut(id).expect("entity");
            e.x = 200.0;
            e.y = 150.0;
            e.size_x = 100.0;
            e.size_y = 100.0;
            e.rotation = 0.0;
        }
        let mut h = Harness::new(scene);
        h.app.config.layout.snap = false;
        h.app.config.layout.view_tool = ViewTool::Move;
        h.app.select_only(id);

        h.frame(FrameInput::default());
        let area = h.app.last_viewport;
        let e = h.app.scene.entity(id).expect("entity");
        let rect = h.app.entity_screen_rect(e, area).expect("rect");
        let (cx, cy) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);

        // Grab the +X (right) arrow arm, then drag diagonally: only x moves.
        h.frame(FrameInput {
            mouse_x: cx + 22.0,
            mouse_y: cy,
            mouse_pressed: true,
            mouse_down: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: cx + 62.0,
            mouse_y: cy + 40.0,
            mouse_down: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: cx + 62.0,
            mouse_y: cy + 40.0,
            ..Default::default()
        });

        let after = h.app.scene.entity(id).expect("entity");
        assert!(
            (after.x - 240.0).abs() < 1.0,
            "x should move +40, was {}",
            after.x
        );
        assert!(
            (after.y - 150.0).abs() < 0.01,
            "y should stay put, was {}",
            after.y
        );
    }

    #[test]
    fn entity_scaler_drag_defaults_to_percent_position() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        scene
            .entity_mut(id)
            .expect("entity")
            .components
            .push(Component::core("EntityScaler"));
        let mut h = Harness::new(scene);
        h.app.config.layout.snap = false;
        h.app.config.layout.view_tool = ViewTool::Move;
        h.app.select_only(id);

        // The scaler starts at viewport (240,40), sized 100x100. Moving by
        // 128x72 is exactly 10% of the 1280x720 preview root.
        h.frame(FrameInput {
            mouse_x: 290.0,
            mouse_y: 90.0,
            mouse_pressed: true,
            mouse_down: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: 418.0,
            mouse_y: 162.0,
            mouse_down: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: 418.0,
            mouse_y: 162.0,
            ..Default::default()
        });

        let scaler = entity_scaler_editor_state(h.app.scene.entity(id).expect("entity"))
            .expect("entity scaler");
        assert!((scaler.x_percent - 0.1).abs() < 0.001);
        assert!((scaler.y_percent - 0.1).abs() < 0.001);
        assert_eq!(scaler.offset_x, 0.0);
        assert_eq!(scaler.offset_y, 0.0);
    }

    #[test]
    fn entity_scaler_resize_defaults_to_size_percent() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        scene
            .entity_mut(id)
            .expect("entity")
            .components
            .push(Component::core("EntityScaler"));
        let mut h = Harness::new(scene);
        h.app.config.layout.snap = false;
        h.app.config.layout.view_tool = ViewTool::Scale;
        h.app.select_only(id);

        // Resize 100x100 to 128x144: 10% and 20% of the preview root.
        h.frame(FrameInput {
            mouse_x: 340.0,
            mouse_y: 140.0,
            mouse_pressed: true,
            mouse_down: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: 368.0,
            mouse_y: 184.0,
            mouse_down: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: 368.0,
            mouse_y: 184.0,
            ..Default::default()
        });

        let scaler = entity_scaler_editor_state(h.app.scene.entity(id).expect("entity"))
            .expect("entity scaler");
        assert!((scaler.size_x_percent - 0.1).abs() < 0.001);
        assert!((scaler.size_y_percent - 0.2).abs() < 0.001);
        assert_eq!(h.app.scene.entity(id).expect("entity").size_x, 100.0);
        assert_eq!(h.app.scene.entity(id).expect("entity").size_y, 100.0);
    }

    #[test]
    fn entity_scaler_offset_editing_updates_offsets_and_pixel_size() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        let mut scaler = Component::core("EntityScaler");
        if let Component::Core { props, .. } = &mut scaler {
            props
                .iter_mut()
                .find(|prop| prop.name == "edit_with_percent")
                .expect("edit toggle")
                .value = PropValue::Bool(false);
        }
        scene
            .entity_mut(id)
            .expect("entity")
            .components
            .push(scaler);
        let mut h = Harness::new(scene);
        h.app.config.layout.snap = false;
        h.app.select_only(id);

        h.frame(FrameInput {
            mouse_x: 290.0,
            mouse_y: 90.0,
            mouse_pressed: true,
            mouse_down: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: 330.0,
            mouse_y: 90.0,
            mouse_down: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: 330.0,
            mouse_y: 90.0,
            ..Default::default()
        });

        // The move puts the rect at x=280, so its bottom-right handle is 380,140.
        h.app.config.layout.view_tool = ViewTool::Scale;
        h.frame(FrameInput {
            mouse_x: 380.0,
            mouse_y: 140.0,
            mouse_pressed: true,
            mouse_down: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: 420.0,
            mouse_y: 180.0,
            mouse_down: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: 420.0,
            mouse_y: 180.0,
            ..Default::default()
        });

        let entity = h.app.scene.entity(id).expect("entity");
        let scaler = entity_scaler_editor_state(entity).expect("entity scaler");
        assert_eq!(scaler.offset_x, 40.0);
        assert_eq!(scaler.offset_y, 0.0);
        assert_eq!(scaler.size_x_percent, 0.0);
        assert_eq!(scaler.size_y_percent, 0.0);
        assert!((entity.size_x - 140.0).abs() < 1.0);
        assert!((entity.size_y - 140.0).abs() < 1.0);
    }

    #[test]
    fn copy_paste_entity() {
        let mut h = Harness::new(Scene::default());
        let id = h.app.scene.entities[0].id;
        h.app.copy_entity(id);
        let before = h.app.scene.entities.len();
        h.app.paste_entity();
        assert_eq!(h.app.scene.entities.len(), before + 1);
    }

    #[test]
    fn reparent_respects_cycles() {
        let mut scene = Scene::default();
        let a = scene.entities[0].id;
        let b = scene.add_entity("B", 0.0, 0.0).id;
        let mut h = Harness::new(scene);
        // Make B a child of A.
        if let Some(e) = h.app.scene.entity_mut(b) {
            e.parent = Some(a);
        }
        // A cannot become a child of B.
        assert!(h.app.scene.would_cycle(a, b));
    }

    #[test]
    fn splitter_drag_resizes_panel() {
        let mut h = Harness::new(Scene::default());
        let before = h.app.config.layout.left_w;
        // Press on the left splitter (~x=left_w) and drag right.
        let edge = before;
        h.frame(FrameInput {
            mouse_x: edge,
            mouse_y: 300.0,
            mouse_pressed: true,
            mouse_down: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: edge + 60.0,
            mouse_y: 300.0,
            mouse_down: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: edge + 60.0,
            mouse_y: 300.0,
            ..Default::default()
        });
        assert!((h.app.config.layout.left_w - before).abs() > 20.0);
    }

    #[test]
    fn add_component_menu_adds_real_core_component() {
        let scene = Scene::default();
        let id = scene.entities[0].id;
        let mut h = Harness::new(scene);
        h.app
            .perform(Action::AddComponent(id, "TextBox".to_string()));
        let e = h.app.scene.entity(id).expect("entity");
        assert!(
            matches!(e.components.last(), Some(Component::Core { name, .. }) if name == "TextBox")
        );
    }

    #[test]
    fn component_picker_discovers_icomponentpicker_scripts() {
        let mut h = Harness::new(Scene::default());
        std::fs::write(
            h.app.project_root.join("Health.luau"),
            "local Behaviour = { hp = Inspector(100) }\nIComponentPicker(Behaviour)\nreturn Behaviour\n",
        )
        .expect("write registered script");
        std::fs::write(
            h.app.project_root.join("Plain.luau"),
            "local Behaviour = { hp = Inspector(100) }\nreturn Behaviour\n",
        )
        .expect("write plain script");

        let scripts = h.app.custom_picker_scripts();
        assert!(
            scripts
                .iter()
                .any(|(label, path)| label == "Health" && path == "Health.luau"),
            "Health should be discovered, got {scripts:?}"
        );
        assert!(
            !scripts.iter().any(|(label, _)| label == "Plain"),
            "Plain must not be offered in the picker"
        );

        // Opening the picker lists it and auto-focuses the search field.
        let id = h.app.scene.entities[0].id;
        h.app.open_add_component_menu(id, 10.0, 10.0);
        let Some(Popup::ComponentPicker { entries, .. }) = h.app.popup.as_ref() else {
            panic!("component picker should open");
        };
        assert!(entries.iter().any(|entry| entry.label == "Health"));
        assert_eq!(h.app.focus.as_deref(), Some("component_picker_search"));
    }

    #[test]
    fn dropping_script_on_hierarchy_entity_adds_inspector_component() {
        let mut h = Harness::new(Scene::default());
        let id = h.app.scene.entities[0].id;
        let script_path = h.app.project_root.join("movement.luau");
        std::fs::write(
            &script_path,
            "local Component = { speed = Inspector(2, 8), tint = Inspector(Color4(1, 2, 3)) }\nreturn Component\n",
        )
        .expect("write script");
        h.app.script_drag = Some(script_path);

        // The first hierarchy row is at roughly y=104 after its search field.
        h.frame(FrameInput {
            mouse_x: 100.0,
            mouse_y: 114.0,
            ..Default::default()
        });

        let entity = h.app.scene.entity(id).expect("entity");
        let Component::Script { path, variables } = entity.components.last().expect("component")
        else {
            panic!("expected script component");
        };
        assert_eq!(path, "movement.luau");
        assert_eq!(variables.len(), 2);
        assert!(matches!(variables[0].control, VarControl::Slider { .. }));
        assert!(matches!(
            variables[1].value,
            VarValue::Color([1, 2, 3, 255])
        ));
    }

    #[test]
    fn dragging_a_corner_handle_resizes_the_entity() {
        // Default entity sits at world (200,150) sized 100x100. With the left
        // panel 240px wide and body starting at y=40, its screen rect is
        // (440,190)-(540,290); the bottom-right handle is at (540,290).
        let mut h = Harness::new(Scene::default());
        h.app.config.layout.snap = false;
        h.app.config.layout.view_tool = ViewTool::Scale;
        let id = h.app.scene.entities[0].id;
        h.app.selected = Some(id);
        // Press the bottom-right handle, drag +40,+40, release.
        h.frame(FrameInput {
            mouse_x: 540.0,
            mouse_y: 290.0,
            mouse_pressed: true,
            mouse_down: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: 580.0,
            mouse_y: 330.0,
            mouse_down: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: 580.0,
            mouse_y: 330.0,
            ..Default::default()
        });
        let e = h.app.scene.entity(id).expect("entity");
        assert!((e.size_x - 140.0).abs() < 2.0, "size_x was {}", e.size_x);
        assert!((e.size_y - 140.0).abs() < 2.0, "size_y was {}", e.size_y);
    }

    #[test]
    fn ctrl_corner_resize_preserves_aspect_ratio() {
        let mut scene = Scene::default();
        scene.entities[0].size_x = 100.0;
        scene.entities[0].size_y = 50.0;
        let mut h = Harness::new(scene);
        h.app.config.layout.snap = false;
        h.app.config.layout.view_tool = ViewTool::Scale;
        let id = h.app.scene.entities[0].id;
        h.app.select_only(id);

        // Bottom-right starts at (540,240). Dragging unevenly with Ctrl held
        // must keep the original 2:1 aspect ratio.
        h.frame(FrameInput {
            mouse_x: 540.0,
            mouse_y: 240.0,
            mouse_pressed: true,
            mouse_down: true,
            ctrl: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: 640.0,
            mouse_y: 340.0,
            mouse_down: true,
            ctrl: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: 640.0,
            mouse_y: 340.0,
            ctrl: true,
            ..Default::default()
        });

        let entity = h.app.scene.entity(id).expect("entity");
        assert!((entity.size_x / entity.size_y - 2.0).abs() < 0.001);
        assert_eq!((entity.x, entity.y), (200.0, 150.0));
    }

    #[test]
    fn image_component_exports_load_call() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        scene
            .entity_mut(id)
            .expect("e")
            .components
            .push(Component::core("Sprite2D"));
        // Default image path present -> loaded once in the shared image cache.
        let images = scene.to_images_luau().expect("images emitted");
        assert!(
            images.contains("assets.loadImage(\"assets/sprite.png\")"),
            "got: {images}"
        );
        // main.luau references the cached handle, not a raw path or inline load.
        let luau = scene.to_luau();
        assert!(
            luau.contains(".image = Images[\"assets/sprite.png\"]"),
            "got: {luau}"
        );
        assert!(
            !luau.contains(".image = \"assets/sprite.png\""),
            "exported raw string path"
        );
        assert!(
            !luau.contains("loadImage"),
            "main.luau should not load images inline"
        );
    }

    #[test]
    fn empty_image_is_omitted_from_export() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        scene
            .entity_mut(id)
            .expect("e")
            .components
            .push(Component::core("Sprite2D"));
        if let Some(Component::Core { props, .. }) =
            scene.entity_mut(id).expect("e").components.last_mut()
        {
            if let Some(p) = props.iter_mut().find(|p| p.name == "image") {
                p.value = PropValue::Image(String::new());
            }
        }
        assert!(!scene.to_luau().contains(".image ="));
    }

    #[test]
    fn copy_paste_component_between_entities() {
        let mut scene = Scene::default();
        let a = scene.entities[0].id;
        scene
            .entity_mut(a)
            .expect("a")
            .components
            .push(Component::core("Rect2D"));
        let b = scene.add_entity("B", 10.0, 10.0).id;
        let mut h = Harness::new(scene);
        h.app.component_clipboard = h
            .app
            .scene
            .entity(a)
            .expect("a")
            .components
            .first()
            .cloned();
        h.app.perform(Action::PasteComponent(b));
        assert_eq!(h.app.scene.entity(b).expect("b").components.len(), 1);
    }

    #[test]
    fn instantiate_prefab_remaps_ids_and_parents() {
        // Build a two-entity prefab (parent + child) and instantiate it twice.
        let mut src = Scene::default();
        let p = src.entities[0].id;
        let c = src.add_entity("Child", 5.0, 5.0).id;
        src.entity_mut(c).expect("c").parent = Some(p);
        let proto = src.subtree(p);

        let mut scene = Scene::default();
        let before = scene.entities.len();
        let root1 = scene.instantiate(proto.clone()).expect("root1");
        let root2 = scene.instantiate(proto).expect("root2");
        assert_ne!(root1, root2);
        assert_eq!(scene.entities.len(), before + 4);
        // Each instance's child points at its own (new) root.
        let kids: Vec<u64> = scene
            .entities
            .iter()
            .filter(|e| e.parent == Some(root1))
            .map(|e| e.id)
            .collect();
        assert_eq!(kids.len(), 1);
    }

    #[test]
    fn prefab_import_offsets_only_roots() {
        let mut h = Harness::new(Scene::default());
        let path = h.app.bin_dir.join("nested.neoprefab");
        let mut root = Entity::new(10, "Root", 25.0, 30.0);
        root.size_x = 40.0;
        root.size_y = 40.0;
        let mut child = Entity::new(11, "Child", 8.0, 9.0);
        child.parent = Some(root.id);
        let json = serde_json::to_string_pretty(&vec![root, child]).expect("serialize prefab");
        std::fs::write(&path, json).expect("write prefab");

        h.app.instantiate_prefab(&path, 100.0, 120.0);
        let root_id = h.app.selected.expect("root selected");
        let root = h.app.scene.entity(root_id).expect("root");
        assert_eq!((root.x, root.y), (100.0, 120.0));
        let child = h
            .app
            .scene
            .entities
            .iter()
            .find(|entity| entity.parent == Some(root_id))
            .expect("child");
        assert_eq!((child.x, child.y), (8.0, 9.0));
    }

    #[test]
    fn opening_scene_path_loads_scene_document() {
        let mut h = Harness::new(Scene::default());
        let path = h.app.bin_dir.join("other.neoscene");
        let mut scene = Scene::default();
        scene.name = "Other".into();
        scene.save(&path).expect("save scene");

        h.app.open_scene_path(path.clone());
        assert_eq!(h.app.scene.name, "Other");
        assert_eq!(h.app.scene_path, path);
        assert_eq!(h.app.documents.len(), 2);
        h.app.switch_document(0);
        assert_ne!(h.app.scene.name, "Other");
        h.app.switch_document(1);
        assert_eq!(h.app.scene.name, "Other");
    }

    #[test]
    fn document_tabs_close_with_middle_click() {
        let mut h = Harness::new(Scene::default());
        let mut second = Scene::default();
        second.name = "Second".into();
        let path = h.app.project_root.join("second.neoscene");
        h.app.add_document(path, second, DocumentKind::Scene);
        assert_eq!(h.app.documents.len(), 2);
        assert_eq!(h.app.active_document, 1);

        h.frame(FrameInput {
            mouse_x: 35.0,
            mouse_y: h.h as f32 - STATUS_H + 10.0,
            middle_pressed: true,
            middle_down: true,
            ..Default::default()
        });
        assert_eq!(h.app.documents.len(), 1);
        assert_eq!(h.app.scene.name, "Second");
    }

    #[test]
    fn project_settings_dialog_focuses_its_first_textbox() {
        let mut h = Harness::new(Scene::default());
        h.app.perform(Action::OpenProjectWindowSettings);
        assert_eq!(h.app.focus.as_deref(), Some("project_start_scene"));
    }

    #[test]
    fn opening_prefab_uses_isolated_prefab_tab() {
        let mut h = Harness::new(Scene::default());
        let path = h.app.project_root.join("button.neoprefab");
        let entities = vec![Entity::new(40, "Button", 0.0, 0.0)];
        std::fs::write(
            &path,
            serde_json::to_string(&entities).expect("serialize entities"),
        )
        .expect("write prefab file");
        h.app.open_prefab_path(path.clone());
        assert_eq!(h.app.document_kind, DocumentKind::Prefab);
        assert_eq!(h.app.scene.entities.len(), 1);
        assert_eq!(h.app.scene_path, path);
        assert_eq!(h.app.documents.len(), 2);
    }

    #[test]
    fn image_cache_reloads_when_file_changes() {
        let h = Harness::new(Scene::default());
        let assets = h.app.project_root.join("assets");
        std::fs::create_dir_all(&assets).expect("assets dir");
        let path = assets.join("sprite.png");
        image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]))
            .save(&path)
            .expect("save red");
        let first = h.app.load_image("assets/sprite.png").expect("load first");
        assert_eq!(first.get_pixel(0, 0).0, [255, 0, 0, 255]);

        std::thread::sleep(std::time::Duration::from_millis(25));
        image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 255, 0, 255]))
            .save(&path)
            .expect("save green");
        let second = h.app.load_image("assets/sprite.png").expect("load second");
        assert_eq!(second.get_pixel(0, 0).0, [0, 255, 0, 255]);
    }

    #[test]
    fn add_entity_at_uses_world_position() {
        let mut h = Harness::new(Scene::default());
        let before = h.app.scene.entities.len();
        h.app.perform(Action::AddEntityAt(320.0, 240.0));
        assert_eq!(h.app.scene.entities.len(), before + 1);
        let e = h.app.scene.entities.last().expect("entity");
        assert_eq!((e.x, e.y), (320.0, 240.0));
    }

    #[test]
    fn ui_widgets_are_offered_but_legacy_ones_are_not() {
        for c in [
            "TextInput",
            "Panel",
            "Button",
            "Slider",
            "Dropdown",
            "Camera",
        ] {
            assert!(CORE_COMPONENTS.contains(&c), "{c} not offered");
        }
        assert!(CORE_COMPONENTS.contains(&"ScrollList"));
        // `Frame` remains a hidden compatibility alias for `Panel`.
        for c in ["Frame"] {
            assert!(!CORE_COMPONENTS.contains(&c), "{c} unexpectedly offered");
            assert!(
                !ADVANCED_COMPONENTS.contains(&c),
                "{c} unexpectedly advanced"
            );
        }
    }

    #[test]
    fn error_popup_renders_without_panicking() {
        let mut h = Harness::new(Scene::default());
        h.app.popup = Some(Popup::Error {
            message: "thread 'main' panicked\n".repeat(40),
            copied: false,
        });
        h.frame(FrameInput {
            mouse_x: 10.0,
            mouse_y: 10.0,
            ..Default::default()
        });
        assert!(h.app.popup.is_some());
    }

    #[test]
    fn particle_sequence_popup_renders_without_panicking() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        scene
            .entity_mut(id)
            .expect("entity")
            .components
            .push(Component::core("ParticleSystem2D"));
        let mut h = Harness::new(scene);
        h.app.popup = Some(Popup::Sequence {
            target: AssetTarget::Prop {
                entity: id,
                component: 0,
                prop: 13,
            },
            kind: SequenceKind::Color,
            value: SequenceValue::Colors(vec![
                ColorKeypoint {
                    time: 0.0,
                    color: [255, 0, 0, 255],
                },
                ColorKeypoint {
                    time: 1.0,
                    color: [0, 0, 255, 255],
                },
            ]),
            selected: 0,
            dragging: None,
            color_picker: None,
        });
        h.frame(FrameInput::default());
        assert!(matches!(h.app.popup, Some(Popup::Sequence { .. })));
    }

    #[test]
    fn particle_sequence_keypoint_can_be_dragged() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        scene
            .entity_mut(id)
            .expect("entity")
            .components
            .push(Component::core("ParticleSystem2D"));
        let value = SequenceValue::Colors(vec![
            ColorKeypoint {
                time: 0.0,
                color: [255, 0, 0, 255],
            },
            ColorKeypoint {
                time: 0.5,
                color: [0, 255, 0, 255],
            },
            ColorKeypoint {
                time: 1.0,
                color: [0, 0, 255, 255],
            },
        ]);
        let mut h = Harness::new(scene);
        h.app.popup = Some(Popup::Sequence {
            target: AssetTarget::Prop {
                entity: id,
                component: 0,
                prop: 13,
            },
            kind: SequenceKind::Color,
            value,
            selected: 1,
            dragging: None,
            color_picker: None,
        });

        // At 1280x760 the strip runs from x=348 to x=932. Grab the middle
        // marker above the strip, then drag it from 50% to 75%.
        h.frame(FrameInput {
            mouse_x: 640.0,
            mouse_y: 304.0,
            mouse_pressed: true,
            mouse_down: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: 786.0,
            mouse_y: 304.0,
            mouse_down: true,
            ..Default::default()
        });

        let Some(Popup::Sequence {
            value: SequenceValue::Colors(keypoints),
            dragging,
            ..
        }) = &h.app.popup
        else {
            panic!("sequence editor should remain open");
        };
        assert_eq!(*dragging, Some(1));
        assert!((keypoints[1].time - 0.75).abs() < 0.002);
        let Component::Core { props, .. } = &h.app.scene.entity(id).expect("entity").components[0]
        else {
            panic!("particle component");
        };
        let PropValue::ColorSequence(saved) = &props[13].value else {
            panic!("color sequence property");
        };
        assert!((saved[1].time - 0.75).abs() < 0.002);
    }

    #[test]
    fn particle_sequence_swatch_opens_picker_and_recolors_keypoint() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        scene
            .entity_mut(id)
            .expect("entity")
            .components
            .push(Component::core("ParticleSystem2D"));
        let mut h = Harness::new(scene);
        h.app.popup = Some(Popup::Sequence {
            target: AssetTarget::Prop {
                entity: id,
                component: 0,
                prop: 13,
            },
            kind: SequenceKind::Color,
            value: SequenceValue::Colors(vec![
                ColorKeypoint {
                    time: 0.0,
                    color: [255, 0, 0, 255],
                },
                ColorKeypoint {
                    time: 1.0,
                    color: [0, 0, 255, 255],
                },
            ]),
            selected: 0,
            dragging: None,
            color_picker: None,
        });

        // Click the sequence color swatch, then choose a point in the HSV
        // saturation/value square on the following frame.
        h.frame(FrameInput {
            mouse_x: 564.0,
            mouse_y: 406.0,
            mouse_pressed: true,
            mouse_down: true,
            ..Default::default()
        });
        assert!(matches!(
            &h.app.popup,
            Some(Popup::Sequence {
                color_picker: Some(_),
                ..
            })
        ));
        h.frame(FrameInput {
            mouse_x: 637.0,
            mouse_y: 513.0,
            mouse_down: true,
            ..Default::default()
        });

        let Some(Popup::Sequence {
            value: SequenceValue::Colors(keypoints),
            ..
        }) = &h.app.popup
        else {
            panic!("sequence editor should remain open");
        };
        assert_ne!(keypoints[0].color, [255, 0, 0, 255]);
        let Component::Core { props, .. } = &h.app.scene.entity(id).expect("entity").components[0]
        else {
            panic!("particle component");
        };
        let PropValue::ColorSequence(saved) = &props[13].value else {
            panic!("color sequence property");
        };
        assert_eq!(saved[0].color, keypoints[0].color);
    }

    #[test]
    fn hsv_rgb_round_trips() {
        for c in [
            [255, 0, 0, 255],
            [10, 180, 90, 255],
            [33, 66, 200, 255],
            [128, 128, 128, 255],
        ] {
            let (h, s, v) = rgb_to_hsv(c);
            let back = hsv_to_rgb(h, s, v);
            for i in 0..3 {
                assert!(
                    (back[i] as i32 - c[i] as i32).abs() <= 2,
                    "channel {i}: {} vs {}",
                    back[i],
                    c[i]
                );
            }
        }
        assert_eq!(parse_hex("#1A2B3C"), Some([0x1a, 0x2b, 0x3c, 255]));
    }

    #[test]
    fn dragging_entity_to_bin_saves_a_prefab() {
        let mut h = Harness::new(Scene::default());
        let id = h.app.scene.entities[0].id;
        if let Some(e) = h.app.scene.entity_mut(id) {
            e.name = "Hero".into();
            e.components.push(Component::core("Rect2D"));
        }
        h.app.save_prefab(id);
        let path = h.app.bin_dir.join("hero.neoprefab");
        assert!(path.exists(), "prefab file not written");
        let entities = load_prefab_file(&path).expect("read prefab");
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].name, "Hero");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn middle_pan_does_not_jump_by_hover_movement() {
        let mut h = Harness::new(Scene::default());
        // Hover far across the viewport with no button held.
        h.frame(FrameInput {
            mouse_x: 300.0,
            mouse_y: 300.0,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: 900.0,
            mouse_y: 300.0,
            ..Default::default()
        });
        assert_eq!(h.app.cam_x, 0.0, "hover moved the camera");
        // Begin a middle-drag: first frame anchors, no movement applied.
        h.frame(FrameInput {
            mouse_x: 900.0,
            mouse_y: 300.0,
            middle_down: true,
            ..Default::default()
        });
        assert_eq!(h.app.cam_x, 0.0);
        // Drag 20px right -> camera moves exactly 20, not by the hover distance.
        h.frame(FrameInput {
            mouse_x: 920.0,
            mouse_y: 300.0,
            middle_down: true,
            ..Default::default()
        });
        assert!(
            (h.app.cam_x - 20.0).abs() < 0.01,
            "cam_x was {}",
            h.app.cam_x
        );
    }

    #[test]
    fn middle_pan_starts_only_inside_viewport_and_uses_sensitivity() {
        let mut h = Harness::new(Scene::default());

        h.frame(FrameInput {
            mouse_x: 20.0,
            mouse_y: 300.0,
            middle_down: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: 60.0,
            mouse_y: 300.0,
            middle_down: true,
            ..Default::default()
        });
        assert_eq!(h.app.cam_x, 0.0);
        h.frame(FrameInput::default());

        h.app.config.settings.viewport_camera_sensitivity = 2.0;
        h.frame(FrameInput {
            mouse_x: 900.0,
            mouse_y: 300.0,
            middle_down: true,
            ..Default::default()
        });
        h.frame(FrameInput {
            mouse_x: 910.0,
            mouse_y: 300.0,
            middle_down: true,
            ..Default::default()
        });
        assert!(
            (h.app.cam_x - 20.0).abs() < 0.01,
            "cam_x was {}",
            h.app.cam_x
        );
    }

    #[test]
    fn undo_redo_round_trips_an_edit() {
        let mut h = Harness::new(Scene::default());
        let before = h.app.scene.entities.len();
        // Add an entity (settles on the release frame, recording one undo step).
        h.app.add_entity(None);
        h.frame(FrameInput::default());
        assert_eq!(h.app.scene.entities.len(), before + 1);
        h.app.undo();
        assert_eq!(
            h.app.scene.entities.len(),
            before,
            "undo did not revert add"
        );
        h.app.redo();
        assert_eq!(
            h.app.scene.entities.len(),
            before + 1,
            "redo did not re-apply"
        );
    }

    #[test]
    fn inactive_entities_are_excluded_from_export() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        scene
            .entity_mut(id)
            .expect("e")
            .components
            .push(Component::core("Rect2D"));
        scene.entity_mut(id).expect("e").name = "Hidden".into();
        scene.entity_mut(id).expect("e").enabled = false;
        let luau = scene.to_luau();
        assert!(!luau.contains("\"Hidden\""), "disabled entity was exported");
    }

    #[test]
    fn zoom_keeps_hit_testing_consistent() {
        let mut h = Harness::new(Scene::default());
        let id = h.app.scene.entities[0].id;
        h.app.cam_zoom = 2.0;
        // Entity at world (200,150); at zoom 2 with viewport x=240,y=40 its
        // center maps to screen (240+400+100, 40+300+100) = (740, 440).
        let hit = h
            .app
            .viewport_hit(Rect::new(240.0, 40.0, 800.0, 600.0), 740.0, 440.0);
        assert_eq!(hit, Some(id));
    }

    #[test]
    fn child_screen_rect_uses_parent_relative_transform() {
        let mut scene = Scene::default();
        let parent = scene.entities[0].id;
        {
            let entity = scene.entity_mut(parent).expect("parent");
            entity.x = 100.0;
            entity.y = 50.0;
            entity.size_x = 100.0;
            entity.size_y = 100.0;
            entity.scale = 2.0;
        }
        let child = scene.add_entity("child", 10.0, 5.0).id;
        {
            let entity = scene.entity_mut(child).expect("child");
            entity.parent = Some(parent);
            entity.size_x = 20.0;
            entity.size_y = 30.0;
            entity.scale = 1.5;
        }

        let h = Harness::new(scene);
        let rect = h
            .app
            .entity_screen_rect(
                h.app.scene.entity(child).expect("child"),
                Rect::new(0.0, 0.0, 400.0, 300.0),
            )
            .expect("screen rect");
        assert_eq!(rect.x, 120.0);
        assert_eq!(rect.y, 60.0);
        assert_eq!(rect.w, 60.0);
        assert_eq!(rect.h, 90.0);
    }

    #[test]
    fn root_entity_scaler_uses_preview_root_bounds() {
        let mut scene = Scene::default();
        let entity_id = scene.entities[0].id;
        {
            let entity = scene.entity_mut(entity_id).expect("entity");
            entity.size_x = 200.0;
            entity.size_y = 100.0;
            entity.components.push(Component::Core {
                name: "EntityScaler".into(),
                props: vec![
                    Prop {
                        name: "enabled".into(),
                        label: "Enabled".into(),
                        value: PropValue::Bool(true),
                        advanced: false,
                        optional: false,
                    },
                    Prop {
                        name: "x_percent".into(),
                        label: "X Pos %".into(),
                        value: PropValue::Number(0.5),
                        advanced: false,
                        optional: false,
                    },
                    Prop {
                        name: "y_percent".into(),
                        label: "Y Pos %".into(),
                        value: PropValue::Number(0.5),
                        advanced: false,
                        optional: false,
                    },
                    Prop {
                        name: "size_x_percent".into(),
                        label: "Size X %".into(),
                        value: PropValue::Number(0.25),
                        advanced: false,
                        optional: false,
                    },
                    Prop {
                        name: "size_y_percent".into(),
                        label: "Size Y %".into(),
                        value: PropValue::Number(0.5),
                        advanced: false,
                        optional: false,
                    },
                    Prop {
                        name: "offset_x".into(),
                        label: "X Offset".into(),
                        value: PropValue::Number(0.0),
                        advanced: false,
                        optional: false,
                    },
                    Prop {
                        name: "offset_y".into(),
                        label: "Y Offset".into(),
                        value: PropValue::Number(0.0),
                        advanced: false,
                        optional: false,
                    },
                    Prop {
                        name: "pivot_x".into(),
                        label: "Pivot X".into(),
                        value: PropValue::Number(0.5),
                        advanced: false,
                        optional: false,
                    },
                    Prop {
                        name: "pivot_y".into(),
                        label: "Pivot Y".into(),
                        value: PropValue::Number(0.5),
                        advanced: false,
                        optional: false,
                    },
                ],
            });
        }

        let h = Harness::new(scene);
        let rect = h
            .app
            .entity_screen_rect(
                h.app.scene.entity(entity_id).expect("entity"),
                Rect::new(0.0, 0.0, 400.0, 300.0),
            )
            .expect("screen rect");

        assert_eq!(rect.x, 480.0);
        assert_eq!(rect.y, 180.0);
        assert_eq!(rect.w, 320.0);
        assert_eq!(rect.h, 360.0);
    }

    #[test]
    fn root_anchor_uses_preview_root_bounds() {
        let mut scene = Scene::default();
        let entity_id = scene.entities[0].id;
        {
            let entity = scene.entity_mut(entity_id).expect("entity");
            entity.x = 10.0;
            entity.y = -20.0;
            entity.anchor_x = 0.5;
            entity.anchor_y = 0.5;
            entity.size_x = 80.0;
            entity.size_y = 40.0;
        }

        let h = Harness::new(scene);
        let rect = h
            .app
            .entity_screen_rect(
                h.app.scene.entity(entity_id).expect("entity"),
                Rect::new(0.0, 0.0, 400.0, 300.0),
            )
            .expect("screen rect");
        assert_eq!(rect.x, 650.0);
        assert_eq!(rect.y, 340.0);

        let (local_x, local_y) = h
            .app
            .world_origin_to_local_position(entity_id, 650.0, 340.0)
            .expect("local position");
        assert_eq!(local_x, 10.0);
        assert_eq!(local_y, -20.0);
    }

    #[test]
    fn viewport_hit_uses_parent_relative_transform() {
        let mut scene = Scene::default();
        let parent = scene.entities[0].id;
        {
            let entity = scene.entity_mut(parent).expect("parent");
            entity.x = 100.0;
            entity.y = 50.0;
            entity.size_x = 10.0;
            entity.size_y = 10.0;
        }
        let child = scene.add_entity("child", 20.0, 30.0).id;
        {
            let entity = scene.entity_mut(child).expect("child");
            entity.parent = Some(parent);
            entity.size_x = 20.0;
            entity.size_y = 20.0;
            entity.z = 5.0;
        }

        let h = Harness::new(scene);
        let viewport = Rect::new(0.0, 0.0, 400.0, 300.0);
        assert_eq!(h.app.viewport_hit(viewport, 121.0, 81.0), Some(child));
        assert_ne!(
            h.app.viewport_hit(viewport, 21.0, 31.0),
            Some(child),
            "child local coordinates should not be treated as viewport world coordinates"
        );
    }

    #[test]
    fn world_to_local_conversion_preserves_child_local_coordinates() {
        let mut scene = Scene::default();
        let parent = scene.entities[0].id;
        {
            let entity = scene.entity_mut(parent).expect("parent");
            entity.x = 100.0;
            entity.y = 50.0;
            entity.scale = 2.0;
        }
        let child = scene.add_entity("child", 10.0, 5.0).id;
        scene.entity_mut(child).expect("child").parent = Some(parent);

        let (local_x, local_y) =
            scene_world_origin_to_local_position(&scene, child, 150.0, 70.0, (1280.0, 720.0))
                .expect("local position");
        assert_eq!(local_x, 25.0);
        assert_eq!(local_y, 10.0);
    }

    #[test]
    fn viewport_hit_prefers_frontmost_draw_order() {
        let mut scene = Scene::default();
        let back = scene.entities[0].id;
        {
            let entity = scene.entity_mut(back).expect("back");
            entity.x = 0.0;
            entity.y = 0.0;
            entity.size_x = 100.0;
            entity.size_y = 100.0;
            entity.z = 1.0;
        }
        let front = scene.add_entity("front", 0.0, 0.0).id;
        {
            let entity = scene.entity_mut(front).expect("front");
            entity.size_x = 100.0;
            entity.size_y = 100.0;
            entity.z = 5.0;
        }

        let mut h = Harness::new(scene);
        let viewport = Rect::new(0.0, 0.0, 400.0, 300.0);
        assert_eq!(h.app.viewport_hit(viewport, 20.0, 20.0), Some(front));

        h.app.scene.entity_mut(front).expect("front").z = 1.0;
        assert_eq!(
            h.app.viewport_hit(viewport, 20.0, 20.0),
            Some(front),
            "equal z should use the runtime entity-id tie-breaker"
        );
    }

    #[test]
    fn viewport_hit_respects_entity_rotation() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        {
            let entity = scene.entity_mut(id).expect("entity");
            entity.x = 0.0;
            entity.y = 0.0;
            entity.size_x = 100.0;
            entity.size_y = 100.0;
            entity.rotation = std::f32::consts::FRAC_PI_4; // 45° about its top-left
        }
        let h = Harness::new(scene);
        let viewport = Rect::new(0.0, 0.0, 400.0, 300.0);
        // Inside the axis-aligned bounds but rotated out of the quad.
        assert_ne!(h.app.viewport_hit(viewport, 60.0, 10.0), Some(id));
        // Inside the rotated quad but outside the axis-aligned bounds.
        assert_eq!(h.app.viewport_hit(viewport, -20.0, 70.0), Some(id));
    }

    #[test]
    fn world_rotation_accumulates_down_the_parent_chain() {
        let mut scene = Scene::default();
        let parent = scene.entities[0].id;
        scene.entity_mut(parent).expect("parent").rotation = 0.5;
        let child = scene.add_entity("child", 0.0, 0.0).id;
        {
            let entity = scene.entity_mut(child).expect("child");
            entity.parent = Some(parent);
            entity.rotation = 0.25;
        }
        let world = scene_world_transform(&scene, child, (1280.0, 720.0)).expect("transform");
        assert!((world.rotation - 0.75).abs() < 1e-6);
    }

    #[test]
    fn text_preview_request_uses_runtime_compatible_fields() {
        let props = vec![
            Prop {
                name: "text".into(),
                label: "Text".into(),
                value: PropValue::Text("Hello".into()),
                advanced: false,
                optional: false,
            },
            Prop {
                name: "font".into(),
                label: "Font".into(),
                value: PropValue::Text("assets/game.ttf".into()),
                advanced: false,
                optional: false,
            },
            Prop {
                name: "alignX".into(),
                label: "Align X".into(),
                value: PropValue::Enum {
                    value: "centre".into(),
                    options: vec![],
                },
                advanced: false,
                optional: false,
            },
            Prop {
                name: "verticalAlign".into(),
                label: "Align Y".into(),
                value: PropValue::Enum {
                    value: "end".into(),
                    options: vec![],
                },
                advanced: false,
                optional: false,
            },
            Prop {
                name: "textScale".into(),
                label: "Text Scale".into(),
                value: PropValue::Enum {
                    value: "fitwidth".into(),
                    options: vec![],
                },
                advanced: false,
                optional: false,
            },
            Prop {
                name: "wrap".into(),
                label: "Wrap".into(),
                value: PropValue::Enum {
                    value: "characters".into(),
                    options: vec![],
                },
                advanced: false,
                optional: false,
            },
            Prop {
                name: "scale".into(),
                label: "Scale".into(),
                value: PropValue::Number(12.0),
                advanced: false,
                optional: false,
            },
            Prop {
                name: "padding_x".into(),
                label: "Padding X".into(),
                value: PropValue::Number(3.0),
                advanced: false,
                optional: false,
            },
        ];
        let defaults = TextPreviewDefaults {
            default_scale: 32.0,
            default_align_x: TextAlignX::Left,
            default_align_y: TextAlignY::Top,
            default_text_scale: TextScaleMode::None,
            default_wrap: TextWrapMode::None,
            default_size_mode_uses_entity: true,
            color_names: &["color"],
            fallback_color: [1, 2, 3, 255],
        };

        let request = text_preview_request(
            Path::new("/tmp/neolove-test-project"),
            &props,
            Rect::new(10.0, 20.0, 100.0, 50.0),
            2.0,
            defaults,
        )
        .expect("text request");

        assert_eq!(request.align_x, TextAlignX::Center);
        assert_eq!(request.align_y, TextAlignY::Bottom);
        assert_eq!(request.text_scale, TextScaleMode::FitWidth);
        assert_eq!(request.wrap, TextWrapMode::Char);
        assert_eq!(request.scale, 24.0);
        assert_eq!(request.padding_x, 6.0);
        match request.font {
            FontHandle::Path(ref path) => assert!(
                std::path::Path::new(path).ends_with("neolove-test-project/assets/game.ttf"),
                "unexpected font path: {path}"
            ),
            _ => panic!("expected a Path font handle"),
        }
    }

    #[test]
    fn selection_quality_of_life_commands_cover_whole_scene() {
        let mut scene = Scene::default();
        let first = scene.entities[0].id;
        let second = scene.add_entity("Second", 20.0, 30.0).id;
        let third = scene.add_entity("Child", 4.0, 5.0).id;
        scene.entity_mut(third).expect("child").parent = Some(second);
        let mut harness = Harness::new(scene);

        harness.app.select_only(second);
        harness.app.select_children();
        assert_eq!(harness.app.selection_ids_ordered(), vec![second, third]);
        harness.app.select_parent();
        assert_eq!(harness.app.selection_ids_ordered(), vec![second]);
        harness.app.select_all();
        assert_eq!(harness.app.selection_count(), 3);
        harness.app.select_only(first);
        harness.app.invert_selection();
        assert_eq!(harness.app.selection_ids_ordered(), vec![second, third]);
    }

    #[test]
    fn expanded_quality_of_life_actions_mutate_selection_state() {
        let mut scene = Scene::default();
        let first = scene.entities[0].id;
        {
            let entity = scene.entity_mut(first).expect("first");
            entity.x = 10.0;
            entity.y = 20.0;
            entity.size_x = 13.0;
            entity.size_y = 17.0;
            entity.rotation = 0.75;
            entity.scale = 2.0;
            entity.anchor_x = 0.5;
            entity.anchor_y = 0.5;
            entity.z = 0.0;
        }
        let second = scene.add_entity("Second", 100.0, 100.0).id;
        scene.entity_mut(second).expect("second").z = 2.0;
        let child = scene.add_entity("Child", 5.0, 6.0).id;
        {
            let entity = scene.entity_mut(child).expect("child");
            entity.parent = Some(second);
            entity.z = 3.0;
        }
        let disabled = scene.add_entity("Disabled", 0.0, 0.0).id;
        {
            let entity = scene.entity_mut(disabled).expect("disabled");
            entity.enabled = false;
            entity.z = -2.0;
        }

        let mut harness = Harness::new(scene);
        harness.app.hidden_ids.insert(second);
        harness.app.locked_ids.insert(child);
        harness.app.perform(Action::SelectHidden);
        assert_eq!(harness.app.selection_ids_ordered(), vec![second]);
        harness.app.perform(Action::SelectLocked);
        assert_eq!(harness.app.selection_ids_ordered(), vec![child]);
        harness.app.perform(Action::SelectInactive);
        assert_eq!(harness.app.selection_ids_ordered(), vec![disabled]);
        harness.app.select_only(first);
        harness.app.perform(Action::SelectNext);
        assert_eq!(harness.app.selected, Some(second));
        harness.app.perform(Action::SelectPrevious);
        assert_eq!(harness.app.selected, Some(first));

        harness.app.select_only(first);
        harness.app.perform(Action::HideUnselected);
        assert!(!harness.app.hidden_ids.contains(&first));
        assert!(harness.app.hidden_ids.contains(&second));
        assert!(harness.app.hidden_ids.contains(&child));
        assert!(harness.app.hidden_ids.contains(&disabled));
        harness.app.perform(Action::LockUnselected);
        assert!(!harness.app.locked_ids.contains(&first));
        assert!(harness.app.locked_ids.contains(&second));
        assert!(harness.app.locked_ids.contains(&child));
        assert!(harness.app.locked_ids.contains(&disabled));
        harness.app.perform(Action::ToggleActiveSelection);
        assert!(!harness.app.scene.entity(first).expect("first").enabled);
        harness.app.perform(Action::ToggleActiveSelection);
        assert!(harness.app.scene.entity(first).expect("first").enabled);

        harness.app.config.layout.grid = 16.0;
        harness.app.perform(Action::SnapSelectedSize);
        let first_entity = harness.app.scene.entity(first).expect("first");
        assert_eq!((first_entity.size_x, first_entity.size_y), (16.0, 16.0));
        harness.app.perform(Action::ResetSelectedRotation);
        harness.app.perform(Action::ResetSelectedScale);
        harness.app.perform(Action::ResetSelectedAnchors);
        let first_entity = harness.app.scene.entity(first).expect("first");
        assert_eq!(first_entity.rotation, 0.0);
        assert_eq!(first_entity.scale, 1.0);
        assert_eq!((first_entity.anchor_x, first_entity.anchor_y), (0.0, 0.0));

        {
            let entity = harness.app.scene.entity_mut(first).expect("first");
            entity.x = 10.0;
            entity.y = 20.0;
            entity.size_x = -20.0;
            entity.size_y = -30.0;
        }
        harness.app.perform(Action::NormalizeSelectedSizes);
        let first_entity = harness.app.scene.entity(first).expect("first");
        assert_eq!((first_entity.x, first_entity.y), (-10.0, -10.0));
        assert_eq!((first_entity.size_x, first_entity.size_y), (20.0, 30.0));

        harness.app.perform(Action::FitSelectionToWindow);
        let first_entity = harness.app.scene.entity(first).expect("first");
        assert_eq!((first_entity.x, first_entity.y), (0.0, 0.0));
        assert_eq!((first_entity.size_x, first_entity.size_y), (1280.0, 720.0));
        {
            let entity = harness.app.scene.entity_mut(first).expect("first");
            entity.size_x = 100.0;
            entity.size_y = 50.0;
        }
        harness.app.perform(Action::CenterSelectionInWindow);
        let first_entity = harness.app.scene.entity(first).expect("first");
        assert_eq!((first_entity.x, first_entity.y), (590.0, 335.0));

        harness.app.perform(Action::BringToFront);
        let front_z = harness.app.scene.entity(first).expect("first").z;
        assert!(front_z > 3.0);
        harness.app.perform(Action::SendBackward);
        assert_eq!(
            harness.app.scene.entity(first).expect("first").z,
            front_z - 1.0
        );
        harness.app.perform(Action::NudgeZ(0.5));
        assert_eq!(
            harness.app.scene.entity(first).expect("first").z,
            front_z - 0.5
        );
    }

    #[test]
    fn scene_visibility_and_picking_are_editor_only() {
        let scene = Scene::default();
        let id = scene.entities[0].id;
        let mut harness = Harness::new(scene);
        let viewport = Rect::new(240.0, 40.0, 800.0, 600.0);
        assert_eq!(harness.app.viewport_hit(viewport, 450.0, 200.0), Some(id));

        harness.app.select_only(id);
        harness.app.lock_selected();
        assert_eq!(harness.app.viewport_hit(viewport, 450.0, 200.0), None);
        harness.app.locked_ids.clear();
        harness.app.hide_selected();
        assert!(harness.app.hidden_ids.contains(&id));
        assert_eq!(harness.app.viewport_hit(viewport, 450.0, 200.0), None);
        assert!(harness.app.scene.to_luau().contains("Entity"));
    }

    #[test]
    fn grouping_preserves_world_positions_and_duplicate_handles_subtrees() {
        let mut scene = Scene::default();
        let first = scene.entities[0].id;
        scene.entity_mut(first).expect("first").x = 10.0;
        scene.entity_mut(first).expect("first").y = 20.0;
        let second = scene.add_entity("Second", 50.0, 70.0).id;
        let child = scene.add_entity("Child", 2.0, 3.0).id;
        scene.entity_mut(child).expect("child").parent = Some(first);
        let mut harness = Harness::new(scene);
        harness.app.select_many(vec![first, second], false);
        harness.app.group_selected();
        let group = harness.app.selected.expect("group selected");
        assert_eq!(
            harness.app.scene.entity(first).expect("first").parent,
            Some(group)
        );
        assert_eq!(
            harness.app.scene.entity(second).expect("second").parent,
            Some(group)
        );
        let first_world = harness
            .app
            .entity_world_transform(first)
            .expect("first world");
        let second_world = harness
            .app
            .entity_world_transform(second)
            .expect("second world");
        assert!((first_world.x - 10.0).abs() < 0.001 && (first_world.y - 20.0).abs() < 0.001);
        assert!((second_world.x - 50.0).abs() < 0.001 && (second_world.y - 70.0).abs() < 0.001);

        let before = harness.app.scene.entities.len();
        harness.app.select_only(first);
        harness.app.duplicate_selection();
        assert_eq!(
            harness.app.scene.entities.len(),
            before + 2,
            "root and child should duplicate"
        );
    }

    #[test]
    fn arrange_commands_align_snap_and_reset_multi_selection() {
        let mut scene = Scene::default();
        let first = scene.entities[0].id;
        {
            let entity = scene.entity_mut(first).expect("first");
            entity.x = 11.0;
            entity.y = 13.0;
            entity.size_x = 10.0;
            entity.size_y = 10.0;
        }
        let second = scene.add_entity("Second", 53.0, 61.0).id;
        {
            let entity = scene.entity_mut(second).expect("second");
            entity.size_x = 20.0;
            entity.size_y = 20.0;
        }
        let mut harness = Harness::new(scene);
        harness.app.select_many(vec![first, second], false);
        harness.app.align_selected(AlignKind::Right);
        assert_eq!(harness.app.scene.entity(first).expect("first").x, 63.0);
        harness.app.config.layout.grid = 16.0;
        harness.app.snap_selected();
        assert_eq!(harness.app.scene.entity(first).expect("first").x, 64.0);
        harness.app.reset_selected();
        assert_eq!(harness.app.scene.entity(first).expect("first").x, 0.0);
        assert_eq!(harness.app.scene.entity(second).expect("second").y, 0.0);
    }

    #[test]
    fn three_d_transform_resets_are_dimension_aware() {
        let mut scene = Scene::default();
        scene.kind = SceneKind::ThreeD;
        let id = scene.entities[0].id;
        {
            let entity = scene.entity_mut(id).expect("entity");
            entity.x = 12.0;
            entity.y = 13.0;
            entity.position_z = 14.0;
            entity.rotation_x = 15.0;
            entity.rotation_y = 16.0;
            entity.rotation_z = 17.0;
            entity.scale_x = 2.0;
            entity.scale_y = 3.0;
            entity.scale_z = 4.0;

            // Inactive 2D state should survive a 3D transform reset.
            entity.z = 21.0;
            entity.rotation = 22.0;
            entity.scale = 5.0;
            entity.anchor_x = 0.25;
            entity.anchor_y = 0.75;
        }
        let mut harness = Harness::new(scene);
        harness.app.select_only(id);
        harness.app.reset_selected();

        let entity = harness.app.scene.entity(id).expect("entity");
        assert_eq!((entity.x, entity.y, entity.position_z), (0.0, 0.0, 0.0));
        assert_eq!(
            (entity.rotation_x, entity.rotation_y, entity.rotation_z),
            (0.0, 0.0, 0.0)
        );
        assert_eq!(
            (entity.scale_x, entity.scale_y, entity.scale_z),
            (1.0, 1.0, 1.0)
        );
        assert_eq!((entity.z, entity.rotation, entity.scale), (21.0, 22.0, 5.0));
        assert_eq!((entity.anchor_x, entity.anchor_y), (0.25, 0.75));

        {
            let entity = harness.app.scene.entity_mut(id).expect("entity");
            entity.x = 7.0;
            entity.position_z = 8.0;
            entity.rotation_y = 9.0;
            entity.scale_z = 10.0;
        }
        harness.app.perform(Action::ResetTransform(id));
        let entity = harness.app.scene.entity(id).expect("entity");
        assert_eq!((entity.x, entity.position_z), (0.0, 0.0));
        assert_eq!(entity.rotation_y, 0.0);
        assert_eq!(entity.scale_z, 1.0);
        assert_eq!((entity.z, entity.rotation, entity.scale), (21.0, 22.0, 5.0));
    }

    #[test]
    fn two_d_transform_reset_preserves_inactive_three_d_state() {
        let mut entity = Entity::new(1, "Entity", 10.0, 20.0);
        entity.position_z = 30.0;
        entity.rotation_x = 40.0;
        entity.rotation_y = 50.0;
        entity.rotation_z = 60.0;
        entity.scale_x = 2.0;
        entity.scale_y = 3.0;
        entity.scale_z = 4.0;
        entity.z = 5.0;
        entity.rotation = 45.0;
        entity.scale = 2.0;
        entity.anchor_x = 0.5;
        entity.anchor_y = 0.5;

        reset_entity_transform(&mut entity, SceneKind::TwoD);

        assert_eq!((entity.x, entity.y, entity.z), (0.0, 0.0, 0.0));
        assert_eq!((entity.rotation, entity.scale), (0.0, 1.0));
        assert_eq!((entity.anchor_x, entity.anchor_y), (0.0, 0.0));
        assert_eq!(entity.position_z, 30.0);
        assert_eq!(
            (entity.rotation_x, entity.rotation_y, entity.rotation_z),
            (40.0, 50.0, 60.0)
        );
        assert_eq!(
            (entity.scale_x, entity.scale_y, entity.scale_z),
            (2.0, 3.0, 4.0)
        );
    }

    #[test]
    fn new_scene_preserves_project_dimension() {
        let mut scene = Scene::default();
        scene.kind = SceneKind::ThreeD;
        let mut harness = Harness::new(scene);

        harness.app.new_scene();

        assert_eq!(harness.app.scene.kind, SceneKind::ThreeD);
        assert_eq!(
            harness.app.documents[harness.app.active_document]
                .scene
                .kind,
            SceneKind::ThreeD
        );
    }

    #[test]
    fn post_process_kind_cycle_covers_every_runtime_effect() {
        let mut effect = default_post_process_effect(0);
        let mut labels = Vec::new();
        for _ in 0..10 {
            labels.push(post_process_effect_label(&effect));
            effect = next_post_process_effect(&effect);
        }
        assert_eq!(
            labels,
            vec![
                "Bloom",
                "Pixelate",
                "Chromatic Aberration",
                "Motion Blur",
                "Quantization",
                "Vignette",
                "Grayscale",
                "Invert",
                "Color Adjustment",
                "Exposure / Tonemap",
            ]
        );
        assert!(matches!(effect, PostProcessEffect::Bloom(_)));
    }

    #[test]
    fn post_process_editor_actions_preserve_order_and_dirty_the_scene() {
        let mut harness = Harness::new(Scene::default());
        assert!(harness.app.scene.post_process.effects.is_empty());

        harness.app.add_post_process_pass();
        harness.app.add_post_process_pass();
        assert_eq!(harness.app.scene.post_process.effects.len(), 2);
        assert!(harness.app.cycle_post_process_pass_kind(1));
        assert!(matches!(
            harness.app.scene.post_process.effects[1].effect,
            PostProcessEffect::Pixelate(_)
        ));

        assert!(harness.app.move_post_process_pass(1, 0));
        assert!(matches!(
            harness.app.scene.post_process.effects[0].effect,
            PostProcessEffect::Pixelate(_)
        ));
        assert!(harness.app.remove_post_process_pass(1));
        assert_eq!(harness.app.scene.post_process.effects.len(), 1);
        assert!(!harness.app.remove_post_process_pass(9));
        assert!(!harness.app.move_post_process_pass(0, 0));
        assert!(harness.app.scene_dirty);
        assert!(harness.app.documents[harness.app.active_document].dirty);
    }

    #[test]
    fn post_process_inspector_renders_all_effect_controls_at_small_width() {
        let mut scene = Scene::default();
        scene.post_process.effects = (0..10)
            .map(|index| PostProcessEffectPass::new(default_post_process_effect(index)))
            .collect();
        let mut harness = Harness::with_size(scene, 320, 480);

        // No selected entity opens the Scene Inspector, including the complete
        // ordered post-process stack. This is a regression check for narrow
        // panels and every effect-specific control path.
        harness.app.clear_selection();
        harness.frame(FrameInput::default());
        assert!(harness.app.inspector_content_h > 0.0);
        assert_eq!(harness.app.scene.post_process.effects.len(), 10);
    }

    #[test]
    fn hierarchy_and_view_quality_of_life_state_toggles() {
        let mut scene = Scene::default();
        let root = scene.entities[0].id;
        let child = scene.add_entity("Child", 0.0, 0.0).id;
        scene.entity_mut(child).expect("child").parent = Some(root);
        let mut harness = Harness::new(scene);
        harness.app.select_only(root);
        harness.app.perform(Action::CollapseSelected);
        assert!(harness.app.hierarchy_collapsed.contains(&root));
        harness.app.perform(Action::ExpandAll);
        assert!(harness.app.hierarchy_collapsed.is_empty());
        harness.app.perform(Action::ToggleProject);
        assert!(!harness.app.config.layout.show_project);
        harness.app.perform(Action::ToggleMaximize);
        assert!(harness.app.maximize_view);
        harness.app.last_viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        harness.app.cam_zoom = 2.0;
        harness.app.zoom_100();
        assert_eq!(harness.app.cam_zoom, 1.0);
    }

    #[test]
    fn responsive_at_small_sizes() {
        for (w, h) in [(640, 420), (320, 240), (200, 160)] {
            let mut scene = Scene::default();
            let id = scene.entities[0].id;
            scene
                .entity_mut(id)
                .expect("entity")
                .components
                .push(Component::core("TextBox"));
            let mut harness = Harness::with_size(scene, w, h);
            harness.app.selected = Some(id);
            harness.frame(FrameInput::default());
        }
    }

    #[test]
    fn available_update_opens_confirmation_popup() {
        let mut harness = Harness::new(Scene::default());
        harness.app.offer_update(AvailableUpdate {
            current_revision: "1111111111111111".to_string(),
            latest_revision: "2222222222222222".to_string(),
            branch: "main".to_string(),
        });
        assert!(matches!(
            harness.app.popup,
            Some(Popup::Confirm {
                action: Pending::UpdateEngine,
                ..
            })
        ));
        assert!(harness.app.status.contains("22222222"));
    }

    #[test]
    fn update_refuses_to_close_with_unsaved_documents() {
        let mut harness = Harness::new(Scene::default());
        harness.app.scene_dirty = true;
        harness.app.documents[0].dirty = true;
        harness.app.launch_update();
        assert!(!harness.app.should_quit);
        assert!(matches!(harness.app.popup, Some(Popup::Error { .. })));
    }

    #[test]
    fn three_d_camera_uses_fov_sensitivity_speed_and_held_controls() {
        let mut harness = Harness::new(Scene::new_for_kind(SceneKind::ThreeD));
        harness.app.config.settings.viewport_camera_fov = 82.0;
        harness.app.config.settings.viewport_camera_sensitivity = 2.0;
        harness.app.config.settings.viewport_camera_speed = 20.0;
        harness.app.viewport_3d_last_frame = Instant::now() - std::time::Duration::from_millis(40);
        let start = harness.app.viewport_camera_3d.position;
        let start_yaw = harness.app.viewport_camera_3d.euler.y;

        harness.frame(FrameInput {
            mouse_x: 640.0,
            mouse_y: 300.0,
            right_down: true,
            key_w: true,
            ..Default::default()
        });
        assert_eq!(harness.app.viewport_camera_3d.fov, 82.0);
        assert!(
            length_vec3(sub_vec3(harness.app.viewport_camera_3d.position, start)) > 0.1,
            "held W should move using the configured units-per-second speed"
        );

        harness.frame(FrameInput {
            mouse_x: 660.0,
            mouse_y: 300.0,
            right_down: true,
            ..Default::default()
        });
        assert!((harness.app.viewport_camera_3d.euler.y - start_yaw - 8.0).abs() < 0.01);
        harness.frame(FrameInput::default());
        assert!(harness.app.viewport_3d_look.is_none());
    }

    #[test]
    fn three_d_projection_hit_prefers_nearest_visible_triangle() {
        let far = Viewport3DHit {
            id: 1,
            points: [(10.0, 10.0), (90.0, 10.0), (50.0, 90.0)],
            bounds: Rect::new(10.0, 10.0, 80.0, 80.0),
            depth: 0.8,
        };
        let near = Viewport3DHit {
            id: 2,
            depth: 0.2,
            ..far
        };
        assert_eq!(viewport_hit_3d(&[far, near], &[], 50.0, 40.0), Some(2));
        assert_eq!(viewport_hit_3d(&[far], &[], 5.0, 5.0), None);
    }

    #[test]
    fn three_d_triangle_budget_tracks_logical_area_and_is_bounded() {
        assert_eq!(
            viewport_triangle_budget(Rect::new(0.0, 0.0, 320.0, 240.0)),
            30_000
        );
        assert_eq!(
            viewport_triangle_budget(Rect::new(0.0, 0.0, 1_280.0, 720.0)),
            115_200
        );
        assert_eq!(
            viewport_triangle_budget(Rect::new(0.0, 0.0, 4_096.0, 2_160.0)),
            250_000
        );
        assert_eq!(
            viewport_triangle_budget(Rect::new(0.0, 0.0, f32::INFINITY, 1.0)),
            250_000
        );
    }

    #[test]
    fn three_d_viewport_scratch_recycling_retains_capacity() {
        let mut slot = Vec::new();
        let mut used = Vec::with_capacity(128);
        used.extend([1u8, 2, 3]);

        recycle_viewport_scratch(&mut slot, used);

        assert!(slot.is_empty());
        assert!(slot.capacity() >= 128);
        let reused = std::mem::take(&mut slot);
        assert!(reused.capacity() >= 128);
    }

    #[test]
    fn three_d_parent_model_applies_full_trs_to_children() {
        let mut scene = Scene::default();
        scene.kind = SceneKind::ThreeD;
        let parent = scene.entities[0].id;
        {
            let parent = scene.entity_mut(parent).expect("parent");
            parent.x = 2.0;
            parent.y = 0.0;
            parent.position_z = 3.0;
            parent.rotation_z = 90.0;
        }
        let child = scene.add_entity("Child", 1.0, 0.0).id;
        scene.entity_mut(child).expect("child").parent = Some(parent);
        let harness = Harness::new(scene);
        let world = harness
            .app
            .entity_world_model_3d(child)
            .expect("child model")
            .transform_point(Vec3::ZERO);
        assert!((world.x - 2.0).abs() < 0.0001);
        assert!((world.y - 1.0).abs() < 0.0001);
        assert!((world.z - 3.0).abs() < 0.0001);
    }

    #[test]
    fn viewport_mesh_cache_reloads_changed_files_and_caches_failures() {
        let harness = Harness::new(Scene::new_for_kind(SceneKind::ThreeD));
        let assets = harness.app.project_root.join("assets");
        std::fs::create_dir_all(&assets).expect("assets directory");
        let path = assets.join("preview.obj");
        std::fs::write(&path, "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").expect("first mesh");
        let first = harness
            .app
            .load_viewport_mesh("assets/preview.obj")
            .expect("first import");
        let first_identity = first.identity();

        std::fs::write(
            &path,
            "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1 2 3\nf 1 3 4\n",
        )
        .expect("changed mesh");
        let second = harness
            .app
            .load_viewport_mesh("assets/preview.obj")
            .expect("reimport changed mesh");
        assert_ne!(first_identity, second.identity());
        assert_eq!(second.snapshot().expect("snapshot").mesh.indices.len(), 6);

        assert!(
            harness
                .app
                .load_viewport_mesh("assets/missing.obj")
                .is_none()
        );
        assert!(
            harness
                .app
                .load_viewport_mesh("assets/missing.obj")
                .is_none()
        );
        assert!(
            harness
                .app
                .mesh_cache
                .borrow()
                .contains_key("assets/missing.obj")
        );

        for index in 0..=VIEWPORT_MESH_CACHE_LIMIT {
            let relative = format!("assets/cache_{index}.obj");
            std::fs::write(
                harness.app.project_root.join(&relative),
                "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n",
            )
            .expect("cache mesh");
            assert!(harness.app.load_viewport_mesh(&relative).is_some());
        }
        assert_eq!(
            harness.app.mesh_cache.borrow().len(),
            VIEWPORT_MESH_CACHE_LIMIT
        );
    }

    #[test]
    fn three_d_viewport_rasterizes_imported_mesh_triangles() {
        let mut scene = Scene::new_for_kind(SceneKind::ThreeD);
        let id = scene.add_entity("Preview", 0.0, 0.0).id;
        let mut renderer = Component::core("MeshRenderer3D");
        if let Component::Core { props, .. } = &mut renderer {
            props
                .iter_mut()
                .find(|prop| prop.name == "mesh_path")
                .expect("mesh path")
                .value = PropValue::Mesh("assets/visible.obj".into());
            props
                .iter_mut()
                .find(|prop| prop.name == "color")
                .expect("mesh tint")
                .value = PropValue::Color([255, 20, 20, 255]);
        }
        scene
            .entity_mut(id)
            .expect("mesh entity")
            .components
            .push(renderer);
        let mut harness = Harness::new(scene);
        let assets = harness.app.project_root.join("assets");
        std::fs::create_dir_all(&assets).expect("assets directory");
        std::fs::write(
            assets.join("visible.obj"),
            "v -1 -1 0\nv 1 -1 0\nv 0 1 0\nf 1 2 3\n",
        )
        .expect("preview mesh");
        harness.frame(FrameInput::default());

        let red_pixels = harness
            .buffer
            .iter()
            .filter(|pixel| {
                let red = (**pixel >> 16) & 0xff;
                let green = (**pixel >> 8) & 0xff;
                red > 80 && red > green.saturating_mul(2)
            })
            .count();
        assert!(
            red_pixels > 50,
            "expected projected triangle pixels, got {red_pixels}"
        );
    }

    #[test]
    fn three_d_screen_plane_drag_updates_xy_and_preserves_z() {
        let mut scene = Scene::new_for_kind(SceneKind::ThreeD);
        let id = scene.add_entity("Draggable", 0.0, 0.0).id;
        scene.entity_mut(id).expect("entity").position_z = 1.25;
        let mut harness = Harness::new(scene);
        harness.app.config.layout.snap = false;
        harness.frame(FrameInput::default());
        let area = harness.app.last_viewport;
        let projection = harness
            .app
            .viewport_camera_3d
            .view_projection(area.w / area.h.max(1.0));
        let model = harness.app.entity_world_model_3d(id).expect("model");
        let point = project_world_point(projection, model.transform_point(Vec3::ZERO), area)
            .expect("projected entity");

        harness.frame(FrameInput {
            mouse_x: point.0,
            mouse_y: point.1,
            mouse_pressed: true,
            mouse_down: true,
            ..Default::default()
        });
        assert_eq!(harness.app.selected, Some(id));
        harness.frame(FrameInput {
            mouse_x: point.0 + 24.0,
            mouse_y: point.1 + 12.0,
            mouse_down: true,
            ..Default::default()
        });
        harness.frame(FrameInput {
            mouse_x: point.0 + 24.0,
            mouse_y: point.1 + 12.0,
            ..Default::default()
        });
        let entity = harness.app.scene.entity(id).expect("dragged entity");
        assert!(entity.x.abs() > 0.01 || entity.y.abs() > 0.01);
        assert_eq!(entity.position_z, 1.25);
    }

    #[test]
    fn three_d_gizmo_size_is_independent_of_entity_scale() {
        let mut scene = Scene::new_for_kind(SceneKind::ThreeD);
        let id = scene.add_entity("Scaled", 0.0, 0.0).id;
        let mut harness = Harness::new(scene);
        harness.app.config.layout.view_tool = ViewTool::Scale;
        harness.app.select_only(id);
        harness.frame(FrameInput::default());
        let area = harness.app.last_viewport;
        let projection = harness
            .app
            .viewport_camera_3d
            .view_projection(area.w / area.h.max(1.0));
        let model = harness.app.entity_world_model_3d(id).expect("model");
        let normal = harness
            .app
            .transform_gizmo_3d(area, projection, id, model)
            .expect("normal gizmo");
        let normal_x = gizmo_axis(normal, Viewport3DAxis::X).expect("normal x axis");
        let normal_length = vector2_length((
            normal_x.end.0 - normal.origin.0,
            normal_x.end.1 - normal.origin.1,
        ));

        let entity = harness.app.scene.entity_mut(id).expect("scaled entity");
        entity.scale_x = 500.0;
        entity.scale_y = 0.002;
        entity.scale_z = 40.0;
        harness.app.world_model_3d_cache.borrow_mut().clear();
        let model = harness.app.entity_world_model_3d(id).expect("scaled model");
        let scaled = harness
            .app
            .transform_gizmo_3d(area, projection, id, model)
            .expect("scaled gizmo");
        let scaled_x = gizmo_axis(scaled, Viewport3DAxis::X).expect("scaled x axis");
        let scaled_length = vector2_length((
            scaled_x.end.0 - scaled.origin.0,
            scaled_x.end.1 - scaled.origin.1,
        ));
        assert!((normal_length - scaled_length).abs() < 0.01);
        assert!(normal_length > 30.0 && normal_length < 140.0);
    }

    #[test]
    fn three_d_move_axis_handle_is_clickable_and_constrained() {
        let mut scene = Scene::new_for_kind(SceneKind::ThreeD);
        let id = scene.add_entity("Mover", 0.0, 0.0).id;
        scene.entity_mut(id).expect("mover").position_z = 0.75;
        let mut harness = Harness::new(scene);
        harness.app.config.layout.view_tool = ViewTool::Move;
        harness.app.config.layout.snap = true;
        harness.app.select_only(id);
        harness.frame(FrameInput::default());
        let area = harness.app.last_viewport;
        let projection = harness
            .app
            .viewport_camera_3d
            .view_projection(area.w / area.h.max(1.0));
        let model = harness.app.entity_world_model_3d(id).expect("model");
        let gizmo = harness
            .app
            .transform_gizmo_3d(area, projection, id, model)
            .expect("gizmo");
        let handle = gizmo_axis(gizmo, Viewport3DAxis::X).expect("x handle");
        let direction =
            normalized_vec2((handle.end.0 - gizmo.origin.0, handle.end.1 - gizmo.origin.1));

        harness.frame(FrameInput {
            mouse_x: handle.end.0,
            mouse_y: handle.end.1,
            mouse_pressed: true,
            mouse_down: true,
            ..Default::default()
        });
        assert!(matches!(
            harness.app.viewport_3d_drag.as_ref().map(|drag| drag.mode),
            Some(Viewport3DDragMode::MoveAxis {
                axis: Viewport3DAxis::X,
                ..
            })
        ));
        harness.frame(FrameInput {
            mouse_x: handle.end.0 + direction.0 * 80.0,
            mouse_y: handle.end.1 + direction.1 * 80.0,
            mouse_down: true,
            ..Default::default()
        });
        harness.frame(FrameInput {
            mouse_x: handle.end.0 + direction.0 * 80.0,
            mouse_y: handle.end.1 + direction.1 * 80.0,
            ..Default::default()
        });
        let entity = harness.app.scene.entity(id).expect("moved entity");
        assert!(entity.x > 0.1, "x={}", entity.x);
        assert_eq!(entity.y, 0.0);
        assert_eq!(entity.position_z, 0.75);
    }

    #[test]
    fn three_d_scale_handle_is_axis_only_finite_and_non_inverting() {
        let mut scene = Scene::new_for_kind(SceneKind::ThreeD);
        let id = scene.add_entity("Scaler", 0.0, 0.0).id;
        let mut harness = Harness::new(scene);
        harness.app.config.layout.view_tool = ViewTool::Scale;
        harness.app.config.layout.snap = false;
        harness.app.select_only(id);
        harness.frame(FrameInput::default());
        let area = harness.app.last_viewport;
        let projection = harness
            .app
            .viewport_camera_3d
            .view_projection(area.w / area.h.max(1.0));
        let model = harness.app.entity_world_model_3d(id).expect("model");
        let gizmo = harness
            .app
            .transform_gizmo_3d(area, projection, id, model)
            .expect("gizmo");
        let handle = gizmo_axis(gizmo, Viewport3DAxis::X).expect("x handle");
        let direction =
            normalized_vec2((handle.end.0 - gizmo.origin.0, handle.end.1 - gizmo.origin.1));

        // A point near the visible box remains a hit at HiDPI scales.
        assert_eq!(
            viewport_gizmo_hit_3d(
                gizmo,
                ViewTool::Scale,
                handle.end.0 + 12.0,
                handle.end.1,
                2.0,
            ),
            Some(Viewport3DGizmoHit::ScaleAxis(Viewport3DAxis::X))
        );
        harness.frame(FrameInput {
            mouse_x: handle.end.0,
            mouse_y: handle.end.1,
            mouse_pressed: true,
            mouse_down: true,
            ..Default::default()
        });
        assert!(matches!(
            harness.app.viewport_3d_drag.as_ref().map(|drag| drag.mode),
            Some(Viewport3DDragMode::ScaleAxis {
                axis: Viewport3DAxis::X,
                ..
            })
        ));

        harness.frame(FrameInput {
            mouse_x: handle.end.0 - direction.0 * 1_000_000.0,
            mouse_y: handle.end.1 - direction.1 * 1_000_000.0,
            mouse_down: true,
            ..Default::default()
        });
        let entity = harness.app.scene.entity(id).expect("min-scaled entity");
        assert!(entity.scale_x.is_finite() && entity.scale_x > 0.0);
        assert_eq!(entity.scale_y, 1.0);
        assert_eq!(entity.scale_z, 1.0);

        harness.frame(FrameInput {
            mouse_x: handle.end.0 + direction.0 * 1_000_000.0,
            mouse_y: handle.end.1 + direction.1 * 1_000_000.0,
            mouse_down: true,
            ..Default::default()
        });
        let entity = harness.app.scene.entity(id).expect("max-scaled entity");
        assert!(entity.scale_x.is_finite() && entity.scale_x <= 32.0);
        assert_eq!(entity.scale_y, 1.0);
        assert_eq!(entity.scale_z, 1.0);
    }

    fn rotation_ring_test_target(
        gizmo: Viewport3DGizmo,
        axis: Viewport3DAxis,
    ) -> ((f32, f32), Viewport3DRotationDragHit) {
        let ring = gizmo
            .rotation_rings
            .into_iter()
            .find(|ring| ring.axis == axis)
            .expect("rotation ring");
        for index in 0..ROTATION_RING_SAMPLES {
            let next = (index + 1) % ROTATION_RING_SAMPLES;
            let (Some(start), Some(end)) = (ring.points[index], ring.points[next]) else {
                continue;
            };
            let point = ((start.0 + end.0) * 0.5, (start.1 + end.1) * 0.5);
            if let Some(hit) = viewport_rotation_ring_hit_3d(gizmo, point.0, point.1, 1.0)
                && hit.axis == axis
            {
                return (point, hit);
            }
        }
        panic!("no unambiguous {axis:?} rotation-ring segment");
    }

    #[test]
    fn three_d_rotation_rings_edit_only_the_target_euler_axis() {
        for axis in Viewport3DAxis::ALL {
            let mut scene = Scene::new_for_kind(SceneKind::ThreeD);
            let id = scene.add_entity("Rotator", 0.0, 0.0).id;
            let mut harness = Harness::new(scene);
            harness.app.config.layout.view_tool = ViewTool::Rotate;
            harness.app.config.layout.snap = true;
            harness.app.select_only(id);
            harness.frame(FrameInput::default());
            let area = harness.app.last_viewport;
            let projection = harness
                .app
                .viewport_camera_3d
                .view_projection(area.w / area.h.max(1.0));
            let model = harness.app.entity_world_model_3d(id).expect("model");
            let gizmo = harness
                .app
                .transform_gizmo_3d(area, projection, id, model)
                .expect("gizmo");
            let (point, hit) = rotation_ring_test_target(gizmo, axis);
            assert_eq!(
                viewport_gizmo_hit_3d(gizmo, ViewTool::Rotate, point.0, point.1, 1.0),
                Some(Viewport3DGizmoHit::RotateAxis(axis))
            );

            harness.frame(FrameInput {
                mouse_x: point.0,
                mouse_y: point.1,
                mouse_pressed: true,
                mouse_down: true,
                ..Default::default()
            });
            assert!(matches!(
                harness.app.viewport_3d_drag.as_ref().map(|drag| drag.mode),
                Some(Viewport3DDragMode::RotateAxis { axis: active, .. }) if active == axis
            ));
            harness.frame(FrameInput {
                mouse_x: point.0 + hit.screen_tangent.0 * 80.0,
                mouse_y: point.1 + hit.screen_tangent.1 * 80.0,
                mouse_down: true,
                ..Default::default()
            });
            let entity = harness.app.scene.entity(id).expect("rotated entity");
            let rotations = [entity.rotation_x, entity.rotation_y, entity.rotation_z];
            for (index, value) in rotations.into_iter().enumerate() {
                if index == axis as usize {
                    assert!(value.abs() >= 15.0 && value.is_finite(), "axis={axis:?}");
                    assert_eq!(value % 15.0, 0.0);
                } else {
                    assert_eq!(value, 0.0, "wrong Euler axis changed for {axis:?}");
                }
            }
        }
    }

    #[test]
    fn three_d_uniform_scale_center_is_bounded_and_undoable() {
        let mut scene = Scene::new_for_kind(SceneKind::ThreeD);
        let id = scene.add_entity("Uniform scaler", 0.0, 0.0).id;
        let mut harness = Harness::new(scene);
        harness.app.config.layout.view_tool = ViewTool::Scale;
        harness.app.config.layout.snap = false;
        harness.app.select_only(id);
        harness.frame(FrameInput::default());
        let area = harness.app.last_viewport;
        let projection = harness
            .app
            .viewport_camera_3d
            .view_projection(area.w / area.h.max(1.0));
        let model = harness.app.entity_world_model_3d(id).expect("model");
        let gizmo = harness
            .app
            .transform_gizmo_3d(area, projection, id, model)
            .expect("gizmo");
        assert_eq!(
            viewport_gizmo_hit_3d(gizmo, ViewTool::Scale, gizmo.origin.0, gizmo.origin.1, 2.0,),
            Some(Viewport3DGizmoHit::ScaleUniform)
        );

        harness.frame(FrameInput {
            mouse_x: gizmo.origin.0,
            mouse_y: gizmo.origin.1,
            mouse_pressed: true,
            mouse_down: true,
            ..Default::default()
        });
        harness.frame(FrameInput {
            mouse_x: gizmo.origin.0 + 1_000_000.0,
            mouse_y: gizmo.origin.1 - 1_000_000.0,
            mouse_down: true,
            ..Default::default()
        });
        let entity = harness.app.scene.entity(id).expect("scaled entity");
        assert_eq!(
            (entity.scale_x, entity.scale_y, entity.scale_z),
            (32.0, 32.0, 32.0)
        );
        harness.frame(FrameInput::default());
        harness.frame(FrameInput::default());
        harness.app.undo();
        let entity = harness.app.scene.entity(id).expect("undo restored entity");
        assert_eq!(
            (entity.scale_x, entity.scale_y, entity.scale_z),
            (1.0, 1.0, 1.0)
        );
    }

    #[test]
    fn three_d_transform_center_separates_move_and_uniform_scale() {
        let mut scene = Scene::new_for_kind(SceneKind::ThreeD);
        let id = scene.add_entity("Transformable", 0.0, 0.0).id;
        let mut harness = Harness::new(scene);
        harness.app.config.layout.view_tool = ViewTool::Transform;
        harness.app.select_only(id);
        harness.frame(FrameInput::default());
        let area = harness.app.last_viewport;
        let projection = harness
            .app
            .viewport_camera_3d
            .view_projection(area.w / area.h.max(1.0));
        let model = harness.app.entity_world_model_3d(id).expect("model");
        let gizmo = harness
            .app
            .transform_gizmo_3d(area, projection, id, model)
            .expect("gizmo");
        assert_eq!(
            viewport_gizmo_hit_3d(
                gizmo,
                ViewTool::Transform,
                gizmo.origin.0,
                gizmo.origin.1,
                1.0,
            ),
            Some(Viewport3DGizmoHit::MoveFree)
        );
        assert_eq!(
            viewport_gizmo_hit_3d(
                gizmo,
                ViewTool::Transform,
                gizmo.origin.0 + 10.0,
                gizmo.origin.1,
                1.0,
            ),
            Some(Viewport3DGizmoHit::ScaleUniform)
        );
    }

    #[test]
    fn clipped_grid_segments_never_fold_across_the_eye_plane() {
        let area = Rect::new(0.0, 0.0, 200.0, 200.0);
        let projection = Mat4::perspective(60.0, 1.0, 0.1, 100.0);

        let crossing = project_world_segment_clipped(
            projection,
            Vec3::new(-1.0, 0.0, 1.0),
            Vec3::new(1.0, 0.0, -3.0),
            area,
        )
        .expect("eye-plane crossing should clip to its visible portion");
        for point in [crossing.0, crossing.1] {
            assert!(point.0 >= -0.01 && point.0 <= 200.01, "x={}", point.0);
            assert!(point.1 >= -0.01 && point.1 <= 200.01, "y={}", point.1);
        }

        assert!(
            project_world_segment_clipped(
                projection,
                Vec3::new(-1.0, 0.0, 1.0),
                Vec3::new(1.0, 0.0, 2.0),
                area,
            )
            .is_none()
        );
        let wide = project_world_segment_clipped(
            projection,
            Vec3::new(-100.0, 0.0, -2.0),
            Vec3::new(100.0, 0.0, -2.0),
            area,
        )
        .expect("wide visible line");
        assert!((wide.0.0 - area.x).abs() < 0.01);
        assert!((wide.1.0 - area.right()).abs() < 0.01);
    }

    #[test]
    fn three_d_grid_follows_camera_and_keeps_a_bounded_infinite_footprint() {
        let area = Rect::new(0.0, 0.0, 1600.0, 900.0);
        let mut camera = default_editor_camera_3d(84.0);
        camera.position = Vec3::new(5_002.25, 4.7, -6_998.75);
        let first = grid_3d_layout(camera, area);

        assert!(first.fine_step.is_finite() && first.fine_step > 0.0);
        assert!(first.coarse_step >= first.fine_step);
        assert!(first.max_x - first.min_x >= 400.0);
        assert!(first.max_z - first.min_z >= 400.0);
        assert!(camera.position.x >= first.min_x && camera.position.x <= first.max_x);
        assert!(camera.position.z >= first.min_z && camera.position.z <= first.max_z);
        assert!(first.fine_half_lines <= 32);
        assert!(first.coarse_half_lines <= 48);

        camera.position.x += 2_000.0;
        camera.position.z -= 3_000.0;
        let moved = grid_3d_layout(camera, area);
        assert!(camera.position.x >= moved.min_x && camera.position.x <= moved.max_x);
        assert!(camera.position.z >= moved.min_z && camera.position.z <= moved.max_z);
        assert_ne!((first.min_x, first.min_z), (moved.min_x, moved.min_z));
    }

    #[test]
    fn grid_steps_use_stable_one_two_five_intervals() {
        assert_eq!(nice_grid_step(0.011), 0.02);
        assert_eq!(nice_grid_step(0.21), 0.5);
        assert_eq!(nice_grid_step(3.1), 5.0);
        assert_eq!(nice_grid_step(51.0), 100.0);
        assert_eq!(nice_grid_step(f32::NAN), 1.0);
    }

    #[test]
    fn three_d_mouse_look_inversion_and_dpi_scaling_are_consistent() {
        let mut normal = Harness::new(Scene::new_for_kind(SceneKind::ThreeD));
        let normal_start = normal.app.viewport_camera_3d.euler.x;
        normal.frame(FrameInput {
            mouse_x: 640.0,
            mouse_y: 300.0,
            right_pressed: true,
            right_down: true,
            display_scale: 1.0,
            ..Default::default()
        });
        normal.frame(FrameInput {
            mouse_x: 640.0,
            mouse_y: 310.0,
            right_down: true,
            display_scale: 1.0,
            ..Default::default()
        });
        let normal_delta = normal.app.viewport_camera_3d.euler.x - normal_start;

        let mut inverted = Harness::new(Scene::new_for_kind(SceneKind::ThreeD));
        inverted.app.config.settings.viewport_invert_mouse_look = true;
        let inverted_start = inverted.app.viewport_camera_3d.euler.x;
        inverted.frame(FrameInput {
            mouse_x: 640.0,
            mouse_y: 300.0,
            right_pressed: true,
            right_down: true,
            display_scale: 2.0,
            ..Default::default()
        });
        inverted.frame(FrameInput {
            mouse_x: 640.0,
            mouse_y: 320.0,
            right_down: true,
            display_scale: 2.0,
            ..Default::default()
        });
        let inverted_delta = inverted.app.viewport_camera_3d.euler.x - inverted_start;
        assert!((normal_delta + inverted_delta).abs() < 0.001);
        assert!(normal_delta > 0.0 && inverted_delta < 0.0);
    }

    #[test]
    fn three_d_rmb_click_opens_context_but_look_drag_does_not() {
        let scene = Scene::new_for_kind(SceneKind::ThreeD);
        let camera_id = scene
            .entities
            .iter()
            .find(|entity| {
                entity.components.iter().any(
                    |component| matches!(component, Component::Core { name, .. } if name == "Camera3D"),
                )
            })
            .expect("starter camera")
            .id;
        let mut click = Harness::new(scene.clone());
        click.frame(FrameInput::default());
        let area = click.app.last_viewport;
        let projection = click
            .app
            .viewport_camera_3d
            .view_projection(area.w / area.h.max(1.0));
        let point = project_world_point(
            projection,
            click
                .app
                .entity_world_model_3d(camera_id)
                .expect("camera model")
                .transform_point(Vec3::ZERO),
            area,
        )
        .expect("camera proxy projection");
        click.frame(FrameInput {
            mouse_x: point.0,
            mouse_y: point.1,
            right_pressed: true,
            right_down: true,
            ..Default::default()
        });
        click.frame(FrameInput {
            mouse_x: point.0,
            mouse_y: point.1,
            right_released: true,
            ..Default::default()
        });
        assert_eq!(click.app.selected, Some(camera_id));
        assert!(matches!(click.app.popup, Some(Popup::Menu { .. })));

        let mut drag = Harness::new(scene);
        drag.frame(FrameInput::default());
        drag.frame(FrameInput {
            mouse_x: 620.0,
            mouse_y: 260.0,
            right_pressed: true,
            right_down: true,
            ..Default::default()
        });
        drag.frame(FrameInput {
            mouse_x: 640.0,
            mouse_y: 260.0,
            right_down: true,
            ..Default::default()
        });
        drag.frame(FrameInput {
            mouse_x: 640.0,
            mouse_y: 260.0,
            right_released: true,
            ..Default::default()
        });
        assert!(drag.app.popup.is_none());
    }

    #[test]
    fn camera_proxy_draws_body_lens_viewfinder_and_frustum_at_hidpi() {
        let fonts = load_fonts().expect("fonts");
        let mut buffer = vec![0u32; 400 * 400];
        let mut painter = Painter::new(&mut buffer, 400, 400, fonts);
        let area = Rect::new(0.0, 0.0, 400.0, 400.0);
        let camera = RenderCamera3D {
            position: Vec3::new(0.0, 0.0, 5.0),
            ..RenderCamera3D::default()
        };
        painter.clear([18, 18, 18, 255]);
        draw_camera_proxy_3d(
            &mut painter,
            area,
            camera.view_projection(1.0),
            Mat4::identity(),
            60.0,
            [90, 205, 235, 255],
            2.0,
        );
        drop(painter);
        let cyan = buffer
            .iter()
            .filter(|pixel| {
                let red = (**pixel >> 16) & 0xff;
                let green = (**pixel >> 8) & 0xff;
                let blue = **pixel & 0xff;
                green > red + 50 && blue > red + 70
            })
            .count();
        assert!(
            cyan > 80,
            "camera proxy should be visually substantial, got {cyan} pixels"
        );
    }
}
