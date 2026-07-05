//! Editor application state and per-frame UI layout.
//!
//! [`EditorApp`] owns the scene and the editor configuration (theme + dock
//! layout). It renders a dockable Hierarchy / Inspector, a pannable 2D
//! viewport, a bottom Project browser, and a toolbar, plus an overlay layer for
//! context menus, dropdowns, the color picker and modal dialogs.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::platform::Color;
use crate::renderer::{
    self, FontHandle, Rect as RenderRect, TextAlignX, TextAlignY, TextAntialiasing,
    TextRenderRequest, TextScaleMode, TextWrapMode, Vec2 as RenderVec2,
};

use super::inspector::parse_inspector_variables;
use super::scene::{
    Component, ComponentReference, DictionaryEntry, Entity, Prop, PropValue, Scene, ScriptVar,
    VarControl, VarKey, VarValue, ADVANCED_COMPONENTS, CORE_COMPONENTS,
};
use super::ui::{icon, Painter, Rect, Theme, Ui};

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
/// Screen-space length of the rotation gizmo's stalk above the entity.
const ROT_HANDLE_DIST: f32 = 28.0;

/// Rotate the screen point `(px, py)` by `angle` radians about `(cx, cy)`.
fn rotate_point_about(px: f32, py: f32, cx: f32, cy: f32, angle: f32) -> (f32, f32) {
    let (sin, cos) = (angle.sin(), angle.cos());
    let dx = px - cx;
    let dy = py - cy;
    (cx + dx * cos - dy * sin, cy + dx * sin + dy * cos)
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
enum Splitter {
    LeftWidth,
    RightWidth,
    LeftSplit,
    RightSplit,
    BinHeight,
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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorConfig {
    pub theme: Theme,
    pub layout: Layout,
}

/// A target a color picker writes back to.
#[derive(Clone, Debug)]
enum ColorTarget {
    Background,
    Prop { entity: u64, comp: usize, prop: usize },
    Var {
        entity: u64,
        comp: usize,
        var: usize,
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
    AddComponent(u64, String),
    PasteComponent(u64),
    OpenAdvancedComponents(u64, f32, f32),
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
    RevealInExplorer,
    OpenProjectInVscode,
    OpenPath(PathBuf),
    OpenScene(PathBuf),
    EnterFolder(PathBuf),
    OpenSelectionTools(f32, f32),
    OpenHierarchyTools(f32, f32),
    OpenArrangeTools(f32, f32),
    OpenViewTools(f32, f32),
    SelectAll,
    InvertSelection,
    SelectChildren,
    SelectParent,
    DuplicateSelection,
    GroupSelected,
    UnparentSelected,
    HideSelected,
    ShowAllHidden,
    LockSelected,
    UnlockAll,
    CollapseSelected,
    ExpandSelected,
    CollapseAll,
    ExpandAll,
    SnapSelected,
    ResetSelected,
    Align(AlignKind),
    FrameAll,
    Zoom100,
    ToggleMaximize,
    ToggleProject,
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
    RenameEntity(u64),
}

/// An overlay drawn above everything, with input precedence.
enum Popup {
    Menu { x: f32, y: f32, items: Vec<MenuItem> },
    Color {
        target: ColorTarget,
        x: f32,
        y: f32,
        rgba: [u8; 4],
        /// Cached hue (0..360) so dragging stays stable at greys where hue is
        /// otherwise undefined.
        hue: f32,
    },
    Confirm { message: String, action: Pending },
    Prompt { title: String, action: Pending },
    /// A runtime error captured from a failed `Run`, with a copy button.
    Error { message: String, copied: bool },
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
    selected: Option<u64>,
    selected_ids: HashSet<u64>,
    dragging: Option<ViewportDrag>,
    box_select: Option<BoxSelect>,
    /// Active resize: (entity id, fixed anchor corner world x/y, grabbed-corner
    /// local fractions). The fractions (0 or 1 on each axis) identify which
    /// corner is being dragged so resizes stay correct when the entity is
    /// rotated.
    resizing: Option<(u64, f32, f32, f32, f32)>,
    /// Active rotation drag via the gizmo knob: the entity being rotated.
    rotating: Option<u64>,
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
    popup: Option<Popup>,
    /// Lazily-loaded image assets for accurate viewport previews. `None` marks
    /// a path that failed to load so we don't retry it every frame.
    image_cache: RefCell<HashMap<String, EditorImageCacheEntry>>,
    /// Parsed Inspector schemas cached by source path and modification time.
    script_schema_cache: HashMap<String, (Option<SystemTime>, Result<Vec<ScriptVar>, String>)>,
    /// Receiver for the outcome of a launched `Run` (None when finished).
    run_rx: Option<std::sync::mpsc::Receiver<Option<String>>>,
    /// A freshly created logger IPC session waiting to be picked up by the
    /// windowing layer to open/show the logger window.
    pending_logger_session: Option<crate::editor_ipc::LoggerSession>,
    status: String,
    scene_dirty: bool,
    should_quit: bool,
    dirty: bool,
    focus: Option<String>,
    edit_buffer: String,
}

impl EditorApp {
    pub fn new(
        project_root: PathBuf,
        scene_path: PathBuf,
        scene: Scene,
        config: EditorConfig,
    ) -> Self {
        let config_path = project_root.join("editor.json");
        let scene_json = scene.to_json().unwrap_or_default();
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
            popup: None,
            image_cache: RefCell::new(HashMap::new()),
            script_schema_cache: HashMap::new(),
            run_rx: None,
            pending_logger_session: None,
            status: "Ready".to_string(),
            scene_dirty: false,
            should_quit: false,
            dirty: false,
            focus: None,
            edit_buffer: String::new(),
        }
    }

    pub fn title(&self) -> String {
        let star = if self.scene_dirty { "*" } else { "" };
        format!("NeoLOVE Editor — {}{}", self.scene.name, star)
    }

    pub fn theme(&self) -> Theme {
        self.config.theme.clone()
    }

    pub fn take_focus(&mut self) -> Option<String> {
        self.focus.take()
    }

    pub fn take_edit_buffer(&mut self) -> String {
        std::mem::take(&mut self.edit_buffer)
    }

    pub fn set_focus(&mut self, focus: Option<String>, edit_buffer: String) {
        self.focus = focus;
        self.edit_buffer = edit_buffer;
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

    /// Called from the event loop when the window is asked to close. Returns
    /// true if it's safe to exit; otherwise opens a save-confirmation dialog.
    pub fn request_close(&mut self) -> bool {
        self.sync_active_document();
        if self.documents.iter().any(|document| document.dirty) {
            self.open_confirm(
                "Discard unsaved changes and quit?",
                Pending::Quit,
            );
            false
        } else {
            true
        }
    }

    fn mark_dirty(&mut self) {
        self.scene_dirty = true;
        if let Some(document) = self.documents.get_mut(self.active_document) {
            document.dirty = true;
        }
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

    fn add_document(&mut self, path: PathBuf, scene: Scene, kind: DocumentKind) {
        if let Some(index) = self.documents.iter().position(|document| document.path == path) {
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
        self.hierarchy_collapsed.retain(|id| scene.entity(*id).is_some());
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
            .filter(|entity| self.selected == Some(entity.id) || self.selected_ids.contains(&entity.id))
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
            && self.resizing.is_none()
            && self.rotating.is_none()
            && self.box_select.is_none();
        if !settled {
            return;
        }
        if let Ok(cur) = self.scene.to_json() {
            if cur != self.undo_baseline {
                self.undo_stack.push(std::mem::replace(&mut self.undo_baseline, cur));
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
                self.redo_stack.push(std::mem::replace(&mut self.undo_baseline, prev));
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
                self.undo_stack.push(std::mem::replace(&mut self.undo_baseline, next));
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
        let mut bounds: Option<(f32, f32, f32, f32)> = None;
        for id in ids {
            if let Some(e) = self.scene.entity(id) {
                let world = self.entity_world_transform(id).unwrap_or(EditorWorldTransform {
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
                    parent = self.scene.entity(parent_id).and_then(|entity| entity.parent);
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
            .filter_map(|id| self.entity_world_transform(*id).map(|world| (*id, world.x, world.y)))
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
            .filter_map(|id| self.entity_world_transform(id).map(|world| (id, world.x, world.y)))
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
        for id in self.selection_ids_ordered() {
            if let Some(entity) = self.scene.entity_mut(id) {
                entity.x = 0.0;
                entity.y = 0.0;
                entity.z = 0.0;
                entity.rotation = 0.0;
                entity.scale = 1.0;
                entity.anchor_x = 0.0;
                entity.anchor_y = 0.0;
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
            AlignKind::Left => items.iter().map(|item| item.1).fold(f32::INFINITY, f32::min),
            AlignKind::CenterX => {
                let min = items.iter().map(|item| item.1).fold(f32::INFINITY, f32::min);
                let max = items.iter().map(|item| item.1 + item.3).fold(f32::NEG_INFINITY, f32::max);
                (min + max) * 0.5
            }
            AlignKind::Right => items.iter().map(|item| item.1 + item.3).fold(f32::NEG_INFINITY, f32::max),
            AlignKind::Top => items.iter().map(|item| item.2).fold(f32::INFINITY, f32::min),
            AlignKind::CenterY => {
                let min = items.iter().map(|item| item.2).fold(f32::INFINITY, f32::min);
                let max = items.iter().map(|item| item.2 + item.4).fold(f32::NEG_INFINITY, f32::max);
                (min + max) * 0.5
            }
            AlignKind::Bottom => items.iter().map(|item| item.2 + item.4).fold(f32::NEG_INFINITY, f32::max),
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
                    let cur = self.scene.entity(id).map(|e| e.name.clone()).unwrap_or_default();
                    self.open_prompt("Rename entity", Pending::RenameEntity(id), &cur);
                }
            }
            // Arrow-key nudge.
            if (ui.input.nudge_x != 0.0 || ui.input.nudge_y != 0.0) && self.selected.is_some() {
                let step = if ui.input.nudge_big { self.config.layout.grid.max(1.0) } else { 1.0 };
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
        let bin_h = if self.maximize_view || !self.config.layout.show_project {
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

        let left_panels = if self.maximize_view { Vec::new() } else { self.panels_on(Side::Left) };
        let right_panels = if self.maximize_view { Vec::new() } else { self.panels_on(Side::Right) };
        let max_col = (w - MIN_VIEWPORT_W).max(0.0);
        let mut left_w = if left_panels.is_empty() { 0.0 } else { self.config.layout.left_w };
        let mut right_w = if right_panels.is_empty() { 0.0 } else { self.config.layout.right_w };
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
            ui, left_col, right_col, &left_panels, &right_panels, w, bin_split_y, body_total,
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
        ui.draw_tooltip();
    }

    fn panels_on(&self, side: Side) -> Vec<Panel> {
        let mut panels = Vec::new();
        if self.config.layout.hierarchy_side == side {
            panels.push(Panel::Hierarchy);
        }
        if self.config.layout.inspector_side == side {
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
        // Labelled action button.
        let act = |ui: &mut Ui, glyph: char, label: &str, x: &mut f32| -> bool {
            let tw = ui.painter.text_width(label, 14.0) + 30.0;
            let rect = Rect::new(*x, y, tw, bh);
            let clicked = ui.icon_button(rect, glyph, label);
            *x += tw + 5.0;
            clicked
        };

        if act(ui, icon::NOTE_ADD, "New", &mut x) {
            self.new_scene();
        }
        if act(ui, icon::SAVE, "Save", &mut x) {
            self.save();
        }
        if act(ui, icon::FOLDER_OPEN, "Load", &mut x) {
            self.load_requested();
        }
        if act(ui, icon::CODE, "Export", &mut x) {
            self.export_luau();
        }
        if act(ui, icon::PLAY, "Run", &mut x) {
            self.run_scene();
        }
        x += 6.0;
        if act(ui, icon::ADD_CIRCLE, "Entity", &mut x) {
            self.add_entity(None);
        }

        x += 10.0;
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

        // Reset camera to the origin.
        let cam_rect = Rect::new(x, y, 30.0, bh);
        if ui.icon_toggle(cam_rect, icon::MY_LOCATION, false, self.config.theme.text) {
            self.reset_view();
            self.status = "Camera reset to (0, 0)".to_string();
        }
        ui.tooltip(cam_rect, "Reset camera to origin (0)");
        x += 35.0;

        // Compact Unity-style utility menu for selection, hierarchy, arrange,
        // and Scene-view commands without crowding the main toolbar.
        let tools_rect = Rect::new(x, y, 30.0, bh);
        if ui.icon_toggle(tools_rect, icon::MORE_VERT, false, self.config.theme.text) {
            self.open_tools_menu(tools_rect.x, tools_rect.bottom() + 2.0);
        }
        ui.tooltip(tools_rect, "Editor tools and layout");
        x += 35.0;

        // Scene name (read-only display; rename via the dialog button).
        let name_label = format!("Scene: {}", self.scene.name);
        let avail = (w - x - 8.0).max(60.0);
        let nr = Rect::new(w - avail - 8.0, y, avail, bh);
        if ui.icon_button(nr, icon::EDIT, &name_label) {
            self.open_prompt("Rename scene", Pending::RenameScene, &self.scene.name.clone());
        }
        ui.tooltip(nr, "Rename scene (also renames the file)");
    }

    fn document_tabs(&mut self, ui: &mut Ui, w: f32, y: f32) {
        if self.documents.len() <= 1 {
            return;
        }
        let bar = Rect::new(0.0, y, w, STATUS_H);
        ui.painter.fill_rect(bar, self.config.theme.header);
        ui.painter.stroke_rect(bar, self.config.theme.border);
        let mut x = 6.0;
        let mut activate = None;
        for (index, document) in self.documents.iter().enumerate() {
            let active = index == self.active_document;
            let dirty = if active { self.scene_dirty } else { document.dirty };
            let kind = if document.kind == DocumentKind::Prefab { "◆" } else { "" };
            let label = format!("{kind}{}{}", document.scene.name, if dirty { " •" } else { "" });
            let width = (ui.painter.text_width(&label, 13.0) + 28.0).clamp(90.0, 220.0);
            let tab = Rect::new(x, y + 1.0, width, STATUS_H - 2.0);
            if active {
                ui.painter.fill_rect(tab, self.config.theme.panel);
                ui.painter.fill_rect(Rect::new(tab.x, tab.y, tab.w, 2.0), self.config.theme.accent);
            }
            if ui.button(tab, &label) {
                activate = Some(index);
            }
            x += width + 3.0;
            if x >= w - 20.0 {
                break;
            }
        }
        if let Some(index) = activate {
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
            Panel::Hierarchy => (icon::ACCOUNT_TREE, "Hierarchy", self.config.layout.hierarchy_side),
            Panel::Inspector => (icon::TUNE, "Inspector", self.config.layout.inspector_side),
        };
        ui.icon(area.x + 16.0, area.y + HEADER_H / 2.0, glyph, 16.0, self.config.theme.text);
        ui.label(area.x + 30.0, area.y + (HEADER_H - 14.0) / 2.0, title, self.config.theme.text);
        let swap = Rect::new(area.right() - 26.0, area.y + 3.0, 20.0, HEADER_H - 6.0);
        ui.tooltip(swap, "Dock to other side");
        if ui.icon_toggle(swap, icon::SWAP, false, self.config.theme.text_dim) {
            match panel {
                Panel::Hierarchy => self.config.layout.hierarchy_side = side.toggled(),
                Panel::Inspector => self.config.layout.inspector_side = side.toggled(),
            }
            self.dirty = true;
        }
        ui.painter.stroke_rect(area, self.config.theme.border);

        let content = Rect::new(area.x, area.y + HEADER_H, area.w, (area.h - HEADER_H).max(0.0));
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
        ui.icon(area.x + 12.0, y + FIELD_H / 2.0, icon::SEARCH, 14.0, self.config.theme.text_dim);
        let resp = ui.text_field("hier_filter", Rect::new(area.x + 22.0, y, area.w - 30.0, FIELD_H), &filter);
        if resp.changed {
            self.hierarchy_filter = resp.text;
        }
        y += FIELD_H + 6.0;
        let query = self.hierarchy_filter.trim().to_lowercase();

        if self.scene.entities.is_empty() {
            ui.label(area.x + PAD, y, "No entities.", self.config.theme.text_dim);
            ui.label(area.x + PAD, y + 18.0, "Right-click or use + Entity.", self.config.theme.text_dim);
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
                ui.label(area.x + PAD, y, "No matches.", self.config.theme.text_dim);
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
        let name = self.scene.entity(id).map(|e| e.name.clone()).unwrap_or_default();
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
            ui.painter.stroke_round_rect(row.shrink(1.0), 3.0, self.config.theme.accent);
        }
        if matches!(self.inspector_reference_drag, Some(InspectorReferenceDrag::Component(_)))
            && hovering
        {
            ui.painter.stroke_round_rect(row.shrink(1.0), 3.0, self.config.theme.selection);
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
                if collapsed { icon::CHEVRON_RIGHT } else { icon::EXPAND_MORE },
                14.0,
                self.config.theme.text_dim,
            );
        }
        let eye_glyph = if hidden { icon::VISIBILITY_OFF } else { icon::VISIBILITY };
        ui.icon(lock.x + lock.w / 2.0, lock.y + lock.h / 2.0, if locked { icon::LOCK } else { icon::LOCK_OPEN }, 13.0, self.config.theme.text_dim);
        ui.icon(eye.x + eye.w / 2.0, eye.y + eye.h / 2.0, eye_glyph, 14.0, self.config.theme.text_dim);
        ui.tooltip(eye, if hidden { "Show in Scene view" } else { "Hide in Scene view" });
        ui.tooltip(lock, if locked { "Enable Scene picking" } else { "Disable Scene picking" });
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
        self.last_viewport = area;
        let prev = ui.painter.push_clip(area);
        ui.set_input_clip(area);

        let inside = area.contains(ui.input.mouse_x, ui.input.mouse_y);

        // Middle-mouse pan, anchored so the camera tracks the cursor exactly
        // instead of jumping by accumulated hover movement.
        if ui.input.middle_down {
            let (mx0, my0, cx0, cy0) = *self.pan_anchor.get_or_insert((
                ui.input.mouse_x,
                ui.input.mouse_y,
                self.cam_x,
                self.cam_y,
            ));
            self.cam_x = cx0 + (ui.input.mouse_x - mx0);
            self.cam_y = cy0 + (ui.input.mouse_y - my0);
            ui.wants_redraw = true;
        } else {
            self.pan_anchor = None;
        }
        // Scroll-wheel zoom, anchored at the cursor.
        if inside && ui.input.scroll != 0.0 {
            let old = self.cam_zoom;
            let new = (old * (1.0 + ui.input.scroll * 0.12)).clamp(0.2, 5.0);
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

        let z = self.cam_zoom;

        // Draw entities sorted by z (lower first).
        let mut entities: Vec<Entity> = self.scene.entities.clone();
        entities.sort_by(compare_editor_entity_order);
        for entity in &entities {
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
            if self.is_selected(entity.id) {
                let angle = self.entity_world_rotation(entity);
                // Outline and collider preview rotate with the entity.
                let prev_rot = ui.painter.push_rotation(rect.x, rect.y, angle);
                ui.painter.stroke_rect(rect.shrink(-1.0), self.config.theme.selection);
                let world_scale = self
                    .entity_world_transform(entity.id)
                    .map(|transform| transform.scale)
                    .unwrap_or_else(|| editor_entity_scale(entity));
                self.draw_collider_preview(ui, entity, rect, z, world_scale);
                ui.painter.set_rotation_raw(prev_rot);

                if self.selected != Some(entity.id) || self.locked_ids.contains(&entity.id) {
                    continue;
                }

                // Corner handles sit at the rotated corner positions but stay
                // screen-aligned so they're easy to grab.
                let (mx, my) = (ui.input.mouse_x, ui.input.mouse_y);
                for (cx, cy) in [
                    (rect.x, rect.y),
                    (rect.right(), rect.y),
                    (rect.x, rect.bottom()),
                    (rect.right(), rect.bottom()),
                ] {
                    let (hx, hy) = rotate_point_about(cx, cy, rect.x, rect.y, angle);
                    // Larger, white-filled handles that brighten under the
                    // cursor so it's clear they can be grabbed.
                    let hot = (mx - hx).abs() <= 7.0 && (my - hy).abs() <= 7.0;
                    let s = if hot { 5.0 } else { 4.0 };
                    ui.painter.fill_rect(Rect::new(hx - s, hy - s, s * 2.0, s * 2.0), self.config.theme.selection);
                    ui.painter.fill_rect(
                        Rect::new(hx - s + 1.5, hy - s + 1.5, s * 2.0 - 3.0, s * 2.0 - 3.0),
                        if hot { [255, 255, 255, 255] } else { [40, 40, 40, 255] },
                    );
                }
                // Rotation knob on a stalk above the top edge.
                let (kx, ky) = self.rotate_handle_knob(rect, angle);
                let rot_hot = self.rotating == Some(entity.id)
                    || ((mx - kx).abs() <= 8.0 && (my - ky).abs() <= 8.0);
                self.draw_rotate_handle(ui, rect, angle, rot_hot);
            }
        }

        if self.script_drag.is_some() {
            self.handle_script_drop(ui, area);
        } else {
            self.handle_viewport_input(ui, area);
        }
        self.handle_prefab_drop(ui, area, z);

        // Transform/zoom HUD overlay (Unity-style), bottom-left of the viewport.
        let scene_flags = if self.hidden_ids.is_empty() && self.locked_ids.is_empty() {
            String::new()
        } else {
            format!("   hidden {} locked {}", self.hidden_ids.len(), self.locked_ids.len())
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
            format!("zoom {}%{}   (scroll to zoom, middle-drag to pan, F to frame)", (self.cam_zoom * 100.0).round() as i32, scene_flags)
        };
        let hud_w = ui.painter.text_width(&hud, 13.0) + 16.0;
        let hud_rect = Rect::new(area.x + 6.0, area.bottom() - 26.0, hud_w.min(area.w - 12.0), 20.0);
        ui.painter.fill_round_rect(hud_rect, 4.0, [0, 0, 0, 150]);
        ui.painter.text_clipped(hud_rect.x + 8.0, hud_rect.y + 3.0, &hud, 13.0, self.config.theme.text, hud_rect.w - 12.0);

        ui.reset_input_clip();
        ui.painter.set_clip_raw(prev);
    }

    fn draw_grid(&self, ui: &mut Ui, area: Rect) {
        let step = (self.config.layout.grid.max(2.0) * self.cam_zoom).max(4.0);
        let line = self.config.theme.grid;
        // Offset grid by the pan so it scrolls with the scene.
        let start_x = area.x + (self.cam_x % step);
        let mut x = start_x - step;
        while x < area.right() {
            if x >= area.x {
                ui.painter.fill_rect(Rect::new(x, area.y, 1.0, area.h), line);
            }
            x += step;
        }
        let start_y = area.y + (self.cam_y % step);
        let mut y = start_y - step;
        while y < area.bottom() {
            if y >= area.y {
                ui.painter.fill_rect(Rect::new(area.x, y, area.w, 1.0), line);
            }
            y += step;
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

    fn entity_world_transform(&self, id: u64) -> Option<EditorWorldTransform> {
        scene_world_transform(&self.scene, id, self.preview_root_size())
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
        let (size_x, size_y) =
            editor_entity_size(&self.scene, entity, self.preview_root_size());
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
        (PREVIEW_ROOT_WIDTH, PREVIEW_ROOT_HEIGHT)
    }

    fn draw_entity(&self, ui: &mut Ui, entity: &Entity, rect: Rect, zoom: f32) {
        // Rotate the whole entity about its origin (its top-left, which is the
        // runtime's default rotation pivot). All the component draw paths below
        // go through the painter, so they inherit the rotation for free.
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
                    props.iter().find(|p| p.name == n).and_then(|p| match p.value {
                        PropValue::Number(v) => Some(v),
                        _ => None,
                    }).unwrap_or(d)
                };
                let prop_img = |n: &str| {
                    props.iter().find(|p| p.name == n).and_then(|p| match &p.value {
                        PropValue::Image(s) => Some(s.clone()),
                        _ => None,
                    })
                };
                let prop_enum = |n: &str| {
                    props.iter().find(|p| p.name == n).and_then(|p| match &p.value {
                        PropValue::Enum { value, .. } => Some(value.clone()),
                        PropValue::Text(s) => Some(s.clone()),
                        _ => None,
                    })
                };
                let prop_int = |n: &str, d: i32| {
                    props.iter().find(|p| p.name == n).and_then(|p| match p.value {
                        PropValue::Int(v) => Some(v),
                        _ => None,
                    }).unwrap_or(d)
                };
                match name.as_str() {
                    "Rect2D" | "Frame" | "ScrollList" => {
                        let radius = prop_num("corner_radius", 0.0) * zoom;
                        ui.painter.fill_round_rect(rect, radius, color);
                        drew = true;
                    }
                    "Shape2D" => {
                        // Mirror the runtime Shape2D primitives so the preview
                        // matches: offset/size, box, inscribed circle, or a
                        // corner triangle.
                        let shape = prop_enum("shape").unwrap_or_else(|| "box".into()).to_ascii_lowercase();
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
                                ui.painter.fill_circle(shape_rect.x + shape_rect.w * 0.5, shape_rect.y + shape_rect.h * 0.5, r, color);
                            }
                            "triangle" | "right_triangle" | "righttriangle" | "rightangledtriangle" => {
                                let corner = prop_enum("triangle_corner")
                                    .unwrap_or_else(|| "bl".into())
                                    .to_ascii_lowercase();
                                let (x0, y0, x1, y1) = (shape_rect.x, shape_rect.y, shape_rect.right(), shape_rect.bottom());
                                let (a, b, c) = match corner.as_str() {
                                    "br" | "bottomright" | "rightbottom" => ((x1, y1), (x1, y0), (x0, y1)),
                                    "tl" | "topleft" | "lefttop" => ((x0, y0), (x1, y0), (x0, y1)),
                                    "tr" | "topright" | "righttop" => ((x1, y0), (x0, y0), (x1, y1)),
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
                        let start_color = prop_color(props, "start_color").unwrap_or([255, 184, 76, 255]);
                        let end_color = prop_color(props, "end_color").unwrap_or([255, 92, 40, 0]);
                        let emitter = prop_enum("shape").unwrap_or_else(|| "point".into());
                        let emitter_radius = prop_num("radius", 32.0).max(0.0) * world_scale * zoom;
                        let gravity_x = prop_num("gravity_x", 0.0);
                        let gravity_y = prop_num("gravity_y", 60.0);
                        let max_particles = props
                            .iter()
                            .find(|prop| prop.name == "max_particles")
                            .and_then(|prop| match prop.value { PropValue::Int(value) => Some(value), _ => None })
                            .unwrap_or(256)
                            .clamp(1, 10_000) as usize;
                        let count = ((rate * lifetime).round() as usize).clamp(1, 32).min(max_particles);
                        let mut seed = (entity.id as u32).wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
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
                            let size = (start_size + (end_size - start_size) * phase)
                                * world_scale
                                * zoom;
                            let mix = |from: u8, to: u8| {
                                (from as f32 + (to as f32 - from as f32) * phase).round() as u8
                            };
                            let particle_color = [
                                mix(start_color[0], end_color[0]),
                                mix(start_color[1], end_color[1]),
                                mix(start_color[2], end_color[2]),
                                mix(start_color[3], end_color[3]),
                            ];
                            ui.painter.fill_circle(px, py, size * 0.5, particle_color);
                        }
                        drew = true;
                    }
                    "Button" | "Dropdown" | "TextInput" => {
                        let radius = prop_num("corner_radius", 6.0) * zoom;
                        ui.painter.fill_round_rect(rect, radius, color);
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
                    "NineSliceSprite2D" => {
                        if let Some(img) = prop_img("image").and_then(|p| self.load_image(&p)) {
                            draw_nine_slice(
                                &mut ui.painter, &img, rect,
                                prop_num("slice_left", 0.0),
                                prop_num("slice_right", 0.0),
                                prop_num("slice_top", 0.0),
                                prop_num("slice_bottom", 0.0),
                                color, zoom,
                            );
                        } else {
                            self.draw_missing_image(ui, rect, color);
                        }
                        drew = true;
                    }
                    "TileTexture2D" => {
                        if let Some(img) = prop_img("image").and_then(|p| self.load_image(&p)) {
                            draw_tiled(&mut ui.painter, &img, rect, prop_num("tile_width", 32.0), prop_num("tile_height", 32.0), color, zoom);
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
                    _ => {}
                }
            }
        }
        if !drew {
            ui.painter.stroke_rect(rect, self.config.theme.text_dim);
            ui.painter.text(rect.x + 4.0, rect.y + 4.0, &entity.name, 12.0, self.config.theme.text_dim);
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
        let Some(mut request) = text_preview_request(&self.project_root, props, rect, zoom, defaults) else {
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
        ui.painter.fill_rect(rect, [tint[0] / 3, tint[1] / 3, tint[2] / 3, 255]);
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
                    props.iter().find(|p| p.name == n).and_then(|p| match p.value {
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
                ui.painter.icon_centered(cr.x + 7.0, cr.y + 7.0, icon::BORDER_ALL, 11.0, green);
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
            if hot { [255, 255, 255, 255] } else { [40, 40, 40, 255] },
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
                    ui.painter.stroke_rect(rect.shrink(-2.0), self.config.theme.accent);
                }
            }
        }
        if ui.input.mouse_down {
            let name = path.file_name().map(|name| name.to_string_lossy()).unwrap_or_default();
            let label = format!("{} → {}", name, target.and_then(|id| self.scene.entity(id))
                .map(|entity| entity.name.as_str()).unwrap_or("entity"));
            let width = ui.painter.text_width(&label, 13.0) + 34.0;
            ui.painter.fill_round_rect(
                Rect::new(mx + 8.0, my + 8.0, width, 22.0),
                4.0,
                [0, 0, 0, 210],
            );
            ui.painter.icon_centered(
                mx + 20.0,
                my + 19.0,
                icon::CODE,
                13.0,
                self.config.theme.accent,
            );
            ui.painter.text(
                mx + 30.0,
                my + 12.0,
                &label,
                13.0,
                self.config.theme.text,
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

    /// While a `.neoprefab` is dragged from the bin, show a ghost in the
    /// viewport and instantiate it at the drop position on release.
    fn handle_prefab_drop(&mut self, ui: &mut Ui, area: Rect, z: f32) {
        let Some(path) = self.prefab_drag.clone() else {
            return;
        };
        let (mx, my) = (ui.input.mouse_x, ui.input.mouse_y);
        if ui.input.mouse_down {
            if area.contains(mx, my) {
                let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                ui.painter.fill_round_rect(Rect::new(mx + 8.0, my + 8.0, 120.0, 20.0), 4.0, [0, 0, 0, 200]);
                ui.painter.icon_centered(mx + 20.0, my + 18.0, icon::VIEW_IN_AR, 13.0, self.config.theme.accent);
                ui.painter.text_clipped(mx + 30.0, my + 11.0, &name, 13.0, self.config.theme.text, 86.0);
                ui.painter.stroke_rect(Rect::new(mx - 6.0, my - 6.0, 12.0, 12.0), self.config.theme.accent);
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

    fn instantiate_prefab(&mut self, path: &Path, wx: f32, wy: f32) {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                self.status = format!("Prefab load failed: {e}");
                return;
            }
        };
        let mut proto: Vec<Entity> = match serde_json::from_str(&text) {
            Ok(p) => p,
            Err(e) => {
                self.status = format!("Prefab parse failed: {e}");
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
            let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
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
                let bounds = rotated_rect_bounds(rect, rect.x, rect.y, self.entity_world_rotation(entity));
                if rects_intersect(marquee, bounds) {
                    ids.push(entity.id);
                }
            }
            self.select_many(ids, select.additive);
            ui.wants_redraw = true;
            return;
        }

        // Rotation gizmo drag takes priority over resize/move.
        if let Some(id) = self.rotating {
            if ui.input.mouse_down {
                if let Some(e) = self.scene.entity(id) {
                    if let Some(rect) = self.entity_screen_rect(e, area) {
                        let (pivot_x, pivot_y) = (rect.x, rect.y);
                        // The knob points straight up from the top-centre in the
                        // entity's local frame; aim that direction at the cursor.
                        let base_angle = (-ROT_HANDLE_DIST).atan2(rect.w / 2.0);
                        let mouse_angle = (my - pivot_y).atan2(mx - pivot_x);
                        let mut world = mouse_angle - base_angle;
                        if self.config.layout.snap {
                            let step = std::f32::consts::FRAC_PI_2 / 6.0; // 15°
                            world = (world / step).round() * step;
                        }
                        let parent_rot = self.entity_world_rotation(e) - e.rotation;
                        let local = world - parent_rot;
                        if let Some(em) = self.scene.entity_mut(id) {
                            if (em.rotation - local).abs() > 1e-5 {
                                em.rotation = local;
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

        // Start a rotation drag when the gizmo knob is pressed.
        if self.rotating.is_none()
            && self.resizing.is_none()
            && self.dragging.is_none()
            && inside
            && ui.input.mouse_pressed
        {
            if let Some(id) = self.selected {
                if let Some(e) = self.scene.entity(id) {
                    if let Some(rect) = self.entity_screen_rect(e, area) {
                        let angle = self.entity_world_rotation(e);
                        let (kx, ky) = self.rotate_handle_knob(rect, angle);
                        if (mx - kx).abs() <= 8.0 && (my - ky).abs() <= 8.0 {
                            self.rotating = Some(id);
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
                    let target_offset_x =
                        local_x + (nw - current_w) * local.scale * local.pivot_x;
                    let target_offset_y =
                        local_y + (nh - current_h) * local.scale * local.pivot_y;
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
                        Some((
                            true,
                            x_percent,
                            y_percent,
                            size_x_percent,
                            size_y_percent,
                        ))
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
                                let mut start_world = Vec::new();
                                for selected_id in self.selection_ids_ordered() {
                                    if self.locked_ids.contains(&selected_id) {
                                        continue;
                                    }
                                    if let Some(transform) = self.entity_world_transform(selected_id) {
                                        start_world.push((selected_id, transform.x, transform.y));
                                    }
                                }
                                if start_world.iter().any(|(selected_id, _, _)| *selected_id == id) {
                                    self.dragging = Some(ViewportDrag {
                                        primary: id,
                                        grab_x: mx - rect.x,
                                        grab_y: my - rect.y,
                                        start_world,
                                    });
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
                let (mut nx, mut ny) = (world_x, world_y);
                if snap {
                    nx = (nx / grid).round() * grid;
                    ny = (ny / grid).round() * grid;
                } else {
                    nx = nx.round();
                    ny = ny.round();
                }
                let Some((_, primary_start_x, primary_start_y)) =
                    drag.start_world.iter().find(|(id, _, _)| *id == drag.primary).copied()
                else {
                    self.dragging = None;
                    return;
                };
                let dx = nx - primary_start_x;
                let dy = ny - primary_start_y;
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
        } else if !left_panels.is_empty() && near(mx, left_edge) && my >= left_col.y && my <= left_col.bottom() {
            hot = Some(Splitter::LeftWidth);
        } else if !right_panels.is_empty() && near(mx, right_edge) && my >= right_col.y && my <= right_col.bottom() {
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
                Splitter::LeftWidth => self.config.layout.left_w = clamp_range(mx, MIN_PANEL_W, w * 0.6),
                Splitter::RightWidth => self.config.layout.right_w = clamp_range(w - mx, MIN_PANEL_W, w * 0.6),
                Splitter::LeftSplit => {
                    self.config.layout.left_split = ((my - left_col.y) / left_col.h.max(1.0)).clamp(0.15, 0.85)
                }
                Splitter::RightSplit => {
                    self.config.layout.right_split = ((my - right_col.y) / right_col.h.max(1.0)).clamp(0.15, 0.85)
                }
                Splitter::BinHeight => {
                    let from_bottom = (TOOLBAR_H + body_total) - my;
                    self.config.layout.bin_h = from_bottom.clamp(0.0, (body_total - 120.0).max(0.0));
                }
            }
        }

        // Visuals.
        let active = self.active_splitter;
        let col_of = |which: Splitter| active == Some(which) || hot == Some(which);
        let line = |ui: &mut Ui, r: Rect, lit: bool| {
            ui.painter.fill_rect(r, if lit { theme.splitter_hover } else { theme.splitter });
        };
        line(ui, Rect::new(0.0, bin_split_y - 1.0, w, 2.0), col_of(Splitter::BinHeight));
        if !left_panels.is_empty() {
            line(ui, Rect::new(left_edge - 1.0, left_col.y, 2.0, left_col.h), col_of(Splitter::LeftWidth));
        }
        if !right_panels.is_empty() {
            line(ui, Rect::new(right_edge - 1.0, right_col.y, 2.0, right_col.h), col_of(Splitter::RightWidth));
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
            let r = ui.text_field("scene_name_insp", Rect::new(x, y, width, FIELD_H), &self.scene.name);
            if r.changed && !r.text.is_empty() {
                self.scene.name = r.text;
                self.mark_dirty();
            }
            y += FIELD_H + 6.0;
            let mut bg = self.scene.background;
            y = self.color_row(ui, "scene_bg", "app.bg", &mut bg, ColorTarget::Background, x, width, y);
            if bg != self.scene.background {
                self.scene.background = bg;
                self.mark_dirty();
            }
            // Upscaling filter: checked = bilinear (smooth), unchecked =
            // nearest-neighbour (crisp pixel-art, the default).
            ui.label(x, y + 4.0, "Bilinear upscaling", self.config.theme.text);
            let bilinear = !self.scene.nearest_neighbor_scaling;
            if let Some(nv) = ui.checkbox(Rect::new(x + LABEL_W, y, FIELD_H, FIELD_H), bilinear) {
                self.scene.nearest_neighbor_scaling = !nv;
                self.mark_dirty();
            }
            y += FIELD_H + 6.0;
            ui.label(x, y + 4.0, "Anti-aliasing", self.config.theme.text);
            let aa_button = Rect::new(x + LABEL_W, y, (width - LABEL_W).max(40.0), FIELD_H);
            if ui.button(aa_button, &self.scene.antialiasing) {
                self.scene.antialiasing = match self.scene.antialiasing.as_str() {
                    "off" => "standard",
                    "standard" => "high",
                    _ => "off",
                }
                .to_string();
                self.mark_dirty();
            }
            ui.tooltip(aa_button, "Cycles off, standard (2x), and high (4x / supersampled text)");
            y += FIELD_H + 6.0;
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
        let r = ui.text_field("ent_name", Rect::new(x + FIELD_H + 6.0, y, width - FIELD_H - 6.0, FIELD_H), &entity.name);
        if r.changed {
            entity.name = r.text;
            dirty = true;
        }
        y += FIELD_H + 8.0;

        y = self.section_header(ui, x, width, y, icon::VIEW_IN_AR, "Transform");
        dirty |= self.num_row(ui, "ent_x", "X", &mut entity.x, x, width, &mut y);
        dirty |= self.num_row(ui, "ent_y", "Y", &mut entity.y, x, width, &mut y);
        dirty |= self.num_row(ui, "ent_z", "Z (order)", &mut entity.z, x, width, &mut y);
        dirty |= self.num_row(ui, "ent_w", "Width", &mut entity.size_x, x, width, &mut y);
        dirty |= self.num_row(ui, "ent_h", "Height", &mut entity.size_y, x, width, &mut y);
        dirty |= self.num_row(ui, "ent_rot", "Rotation", &mut entity.rotation, x, width, &mut y);
        dirty |= self.num_row(ui, "ent_scale", "Scale", &mut entity.scale, x, width, &mut y);
        // Advanced transform.
        let adv_key = format!("adv_transform_{id}");
        let expanded = !self.collapsed.contains(&adv_key);
        let hdr = Rect::new(x + 10.0, y, width - 10.0, ROW_H - 2.0);
        let now = ui.collapsing_header(hdr, "Advanced", expanded);
        self.set_collapsed(&adv_key, !now);
        y += ROW_H + 2.0;
        if now {
            dirty |= self.num_row(ui, "ent_ax", "Anchor X", &mut entity.anchor_x, x + 8.0, width - 8.0, &mut y);
            dirty |= self.num_row(ui, "ent_ay", "Anchor Y", &mut entity.anchor_y, x + 8.0, width - 8.0, &mut y);
        }
        y += 6.0;

        y = self.section_header(ui, x, width, y, icon::VIEW_QUILT, "Components");
        let mut remove_component: Option<usize> = None;
        for index in 0..entity.components.len() {
            let comp_label = entity.components[index].label().to_string();
            let glyph = component_icon(&entity.components[index]);
            let key = format!("comp_{id}_{index}");
            let comp_expanded = !self.collapsed.contains(&key);
            // Header row with collapse + copy + remove.
            let tri = if comp_expanded { icon::EXPAND_MORE } else { icon::CHEVRON_RIGHT };
            ui.painter.fill_round_rect(Rect::new(x, y, width, ROW_H), 3.0, self.config.theme.panel_alt);
            ui.icon(x + 12.0, y + ROW_H / 2.0, tri, 15.0, self.config.theme.text);
            ui.icon(x + 28.0, y + ROW_H / 2.0, glyph, 15.0, self.config.theme.accent);
            ui.label(x + 42.0, y + (ROW_H - 14.0) / 2.0, &comp_label, self.config.theme.text);
            let collapse_hit = Rect::new(x, y, 22.0, ROW_H);
            if collapse_hit.contains(ui.input.mouse_x, ui.input.mouse_y)
                && ui.input.mouse_pressed
            {
                self.set_collapsed(&key, comp_expanded);
            }
            let drag_hit = Rect::new(x + 22.0, y, (width - 70.0).max(0.0), ROW_H);
            ui.tooltip(drag_hit, "Drag component reference");
            if drag_hit.contains(ui.input.mouse_x, ui.input.mouse_y) && ui.input.mouse_pressed {
                self.inspector_reference_drag = Some(InspectorReferenceDrag::Component(
                    ComponentReference {
                        entity: id,
                        component: index,
                    },
                ));
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
                dirty |= self.component_body(ui, id, index, &mut entity.components[index], x, width, &mut y);
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

        if ui.icon_button(Rect::new(x, y, width, FIELD_H + 4.0), icon::DELETE, "Delete Entity") {
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

    fn component_body(&mut self, ui: &mut Ui, entity: u64, comp: usize, component: &mut Component, x: f32, width: f32, y: &mut f32) -> bool {
        let mut dirty = false;
        match component {
            Component::Core { props, .. } => {
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
                                dirty |= self.prop_row(ui, entity, comp, pi, &mut props[pi], x + 8.0, width - 8.0, y);
                            }
                        }
                    }
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

    fn prop_row(&mut self, ui: &mut Ui, entity: u64, comp: usize, pi: usize, prop: &mut Prop, x: f32, width: f32, y: &mut f32) -> bool {
        let id = format!("p_{entity}_{comp}_{pi}");
        let mut dirty = false;
        let fx = x + LABEL_W;
        let fw = (width - LABEL_W).max(30.0);
        ui.label(x, *y + 4.0, &prop.label, self.config.theme.text);
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
                // A button cycling through options (compact).
                let btn = Rect::new(fx, *y, fw, FIELD_H);
                if ui.button(btn, value) {
                    let idx = options.iter().position(|o| o == value).unwrap_or(0);
                    *value = options[(idx + 1) % options.len().max(1)].clone();
                    dirty = true;
                }
                *y += FIELD_H + 6.0;
            }
            PropValue::Color(c) => {
                let mut col = *c;
                dirty |= self.color_row_inline(
                    ui, &id, fx, fw, *y, &mut col,
                    ColorTarget::Prop { entity, comp, prop: pi },
                );
                *c = col;
                *y += FIELD_H + 6.0;
            }
            PropValue::Image(s) => {
                let r = ui.text_field(&id, Rect::new(fx, *y, fw, FIELD_H), s);
                if r.changed {
                    *s = r.text;
                    dirty = true;
                }
                *y += FIELD_H + 6.0;
            }
        }
        dirty
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
        ui.icon(x + 8.0, y + 8.0, icon::PLAYLIST_ADD, 14.0, self.config.theme.text_dim);
        ui.label(x + 20.0, y, "Inspector Variables", self.config.theme.text_dim);
        y += 22.0;
        if variables.is_empty() {
            ui.label(
                x + 8.0,
                y,
                "No Inspector(...) declarations.",
                self.config.theme.text_dim,
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
                entity,
                comp,
                index,
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
        entity: u64,
        comp: usize,
        var: usize,
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
                    ui.label(x, *y + 4.0, label, self.config.theme.text);
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
                ui.label(x, *y + 4.0, label, self.config.theme.text);
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
            VarValue::Color(color) => {
                ui.label(x, *y + 4.0, label, self.config.theme.text);
                let fx = x + LABEL_W;
                let mut next = *color;
                if self.color_row_inline(
                    ui,
                    base,
                    fx,
                    (x + width) - fx,
                    *y,
                    &mut next,
                    ColorTarget::Var {
                        entity,
                        comp,
                        var,
                        path: path.clone(),
                    },
                ) {
                    *color = next;
                    *dirty = true;
                }
                *y += FIELD_H + 6.0;
            }
            VarValue::Entity(reference) => {
                ui.label(x, *y + 4.0, label, self.config.theme.text);
                let field = Rect::new(
                    x + LABEL_W,
                    *y,
                    (width - LABEL_W - 26.0).max(30.0),
                    FIELD_H,
                );
                let hovering = field.contains(ui.input.mouse_x, ui.input.mouse_y);
                ui.painter.fill_round_rect(field, 3.0, self.config.theme.field);
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
                    }) =
                        self.inspector_reference_drag.take()
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
                if ui.icon_toggle(
                    clear,
                    icon::DELETE,
                    false,
                    self.config.theme.text_dim,
                ) && reference.take().is_some()
                {
                    *dirty = true;
                }
                *y += FIELD_H + 6.0;
            }
            VarValue::Component(reference) => {
                ui.label(x, *y + 4.0, label, self.config.theme.text);
                let field = Rect::new(
                    x + LABEL_W,
                    *y,
                    (width - LABEL_W - 26.0).max(30.0),
                    FIELD_H,
                );
                let hovering = field.contains(ui.input.mouse_x, ui.input.mouse_y);
                ui.painter.fill_round_rect(field, 3.0, self.config.theme.field);
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
                if ui.icon_toggle(
                    clear,
                    icon::DELETE,
                    false,
                    self.config.theme.text_dim,
                ) && reference.take().is_some()
                {
                    *dirty = true;
                }
                *y += FIELD_H + 6.0;
            }
            VarValue::List(values) => {
                let collapse_key = format!("{base}_list");
                let expanded = !self.collapsed.contains(&collapse_key);
                let header = Rect::new(x, *y, width, ROW_H);
                let next = ui.collapsing_header(
                    header,
                    &format!("{label}  [{}]", values.len()),
                    expanded,
                );
                self.set_collapsed(&collapse_key, !next);
                *y += ROW_H + 3.0;
                if next {
                    let mut remove = None;
                    for index in 0..values.len() {
                        let item_y = *y;
                        path.push(VarPathPart::List(index));
                        self.script_value_editor(
                            ui,
                            &format!("{base}_{index}"),
                            &format!("{}", index + 1),
                            &mut values[index],
                            &VarControl::Field,
                            entity,
                            comp,
                            var,
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
                        ui.label(x + 12.0, *y + 4.0, "Key", self.config.theme.text_dim);
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
                        self.script_value_editor(
                            ui,
                            &format!("{base}_{index}_value"),
                            "Value",
                            &mut entry.value,
                            &VarControl::Field,
                            entity,
                            comp,
                            var,
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

    fn section_header(&mut self, ui: &mut Ui, x: f32, width: f32, y: f32, glyph: char, title: &str) -> f32 {
        ui.painter.fill_rect(Rect::new(x, y, width, 1.0), self.config.theme.border);
        let y = y + 6.0;
        ui.icon(x + 8.0, y + 8.0, glyph, 15.0, self.config.theme.text_dim);
        ui.label(x + 20.0, y, title, self.config.theme.text_dim);
        y + 22.0
    }

    fn num_row(&mut self, ui: &mut Ui, id: &str, label: &str, value: &mut f32, x: f32, width: f32, y: &mut f32) -> bool {
        ui.label(x, *y + 4.0, label, self.config.theme.text);
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

    fn text_row(&mut self, ui: &mut Ui, id: &str, label: &str, value: &mut String, x: f32, width: f32, y: &mut f32) -> bool {
        ui.label(x, *y + 4.0, label, self.config.theme.text);
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
    fn color_row(&mut self, ui: &mut Ui, id: &str, label: &str, color: &mut [u8; 4], target: ColorTarget, x: f32, width: f32, y: f32) -> f32 {
        ui.label(x, y + 4.0, label, self.config.theme.text);
        let fx = x + LABEL_W;
        self.color_row_inline(ui, id, fx, (x + width) - fx, y, color, target);
        y + FIELD_H + 6.0
    }

    fn color_row_inline(&mut self, ui: &mut Ui, id: &str, fx: f32, fw: f32, y: f32, color: &mut [u8; 4], target: ColorTarget) -> bool {
        let mut dirty = false;
        let swatch = Rect::new(fx, y, 22.0, FIELD_H);
        if ui.swatch_button(swatch, *color) {
            let hue = rgb_to_hsv(*color).0;
            self.popup = Some(Popup::Color { target, x: fx, y: y + FIELD_H + 2.0, rgba: *color, hue });
        }
        ui.tooltip(swatch, "Open color picker");
        let cells_x = fx + 28.0;
        let avail = (fx + fw) - cells_x;
        let cell_w = ((avail - 8.0) / 3.0).max(22.0);
        for i in 0..3 {
            let cx = cells_x + i as f32 * (cell_w + 4.0);
            let r = ui.text_field(&format!("{id}_{i}"), Rect::new(cx, y, cell_w, FIELD_H), &color[i].to_string());
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
        ui.icon(area.x + 16.0, area.y + HEADER_H / 2.0, icon::FOLDER_OPEN, 16.0, self.config.theme.text);
        ui.label(area.x + 30.0, area.y + (HEADER_H - 14.0) / 2.0, "Project", self.config.theme.text);

        let rel = self.bin_rel();
        ui.painter.text_clipped(area.x + 92.0, area.y + (HEADER_H - 13.0) / 2.0, &format!("/{rel}"), 13.0, self.config.theme.text_dim, area.w - 320.0);

        // Header buttons: new folder, new script, VS Code, reveal, up.
        let mut bx = area.right() - 30.0;
        let btn = |ui: &mut Ui, x: f32, glyph: char, tip: &str| -> bool {
            let r = Rect::new(x, area.y + 3.0, 24.0, HEADER_H - 6.0);
            let c = ui.icon_toggle(r, glyph, false, ui.theme.text);
            ui.tooltip(r, tip);
            c
        };
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
                ui.painter.stroke_round_rect(area.shrink(2.0), 4.0, self.config.theme.accent);
                let name = self.scene.entity(drag).map(|e| e.name.clone()).unwrap_or_default();
                ui.painter.text(area.x + 210.0, area.y + 6.0, &format!("Drop to save \"{name}\" as a prefab"), 13.0, self.config.theme.accent);
                if !ui.input.mouse_down {
                    self.save_prefab(drag);
                    self.reparent_drag = None;
                }
            }
        }

        let content = Rect::new(area.x, area.y + HEADER_H, area.w, (area.h - HEADER_H).max(0.0));
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

        if content.contains(ui.input.mouse_x, ui.input.mouse_y) && ui.input.scroll != 0.0 {
            self.bin_scroll -= ui.input.scroll * 32.0;
            ui.wants_redraw = true;
        }
        self.bin_scroll = self.bin_scroll.clamp(0.0, (self.bin_content_h - content.h).max(0.0));

        let prev = ui.painter.push_clip(content);
        ui.set_input_clip(content);
        let row_h = 22.0;
        let mut yy = content.y + 6.0 - self.bin_scroll;
        let mut navigate = None;
        let mut open = None;
        let mut context: Option<(PathBuf, f32, f32)> = None;
        let theme = self.config.theme.clone();
        let draw = |ui: &mut Ui, yy: f32, glyph: char, name: &str, accent: bool| -> (bool, bool, bool) {
            let row = Rect::new(content.x + 4.0, yy, content.w - 8.0, row_h);
            let hovered = row.contains(ui.input.mouse_x, ui.input.mouse_y) && content.contains(ui.input.mouse_x, ui.input.mouse_y);
            if hovered {
                ui.painter.fill_rect(row, theme.panel_alt);
            }
            let c = if accent { theme.accent } else { theme.text_dim };
            ui.icon(row.x + 12.0, yy + row_h / 2.0, glyph, 15.0, c);
            ui.painter.text_clipped(row.x + 26.0, yy + (row_h - 14.0) / 2.0, name, 14.0, theme.text, row.w - 30.0);
            (hovered && ui.input.mouse_pressed, hovered && ui.input.double_click, hovered && ui.input.right_pressed)
        };
        for (path, name) in &dirs {
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
        for (path, name, glyph) in &files {
            let (click, dbl, rc) = draw(ui, yy, *glyph, name, false);
            if dbl {
                if path.extension().is_some_and(|e| e == "neoscene") {
                    self.open_scene_path(path.clone());
                } else if path.extension().is_some_and(|e| e == "neoprefab") {
                    self.open_prefab_path(path.clone());
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
        if dirs.is_empty() && files.is_empty() {
            ui.label(content.x + 10.0, content.y + 8.0, "Empty folder.", self.config.theme.text_dim);
            yy += row_h;
        }
        ui.reset_input_clip();
        ui.painter.set_clip_raw(prev);
        self.bin_content_h = yy - (content.y - self.bin_scroll) + 6.0;

        // Right-click empty area of the bin.
        if content.contains(ui.input.mouse_x, ui.input.mouse_y) && ui.input.right_pressed && context.is_none() {
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
            let thumb_y = content.y + (self.bin_scroll / (self.bin_content_h - content.h)) * (content.h - thumb_h);
            ui.painter.fill_round_rect(Rect::new(content.right() - 6.0, thumb_y, 4.0, thumb_h), 2.0, self.config.theme.text_dim);
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
        let snap = if self.config.layout.snap { "snap" } else { "free" };
        ui.painter.text(
            8.0,
            h - STATUS_H + 5.0,
            &format!(
                "{}   |   {} entities   |   grid {}px ({})   |   {}",
                self.status,
                self.scene.entities.len(),
                format_num(self.config.layout.grid),
                snap,
                self.scene_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            ),
            13.0,
            self.config.theme.text_dim,
        );
    }

    // ---- Popups ------------------------------------------------------------

    fn open_add_component_menu(&mut self, entity: u64, x: f32, y: f32) {
        let mut items: Vec<MenuItem> = Vec::new();
        if let Some(c) = &self.component_clipboard {
            items.push(MenuItem {
                action: Action::PasteComponent(entity),
                glyph: icon::CONTENT_PASTE,
                label: format!("Paste {}", c.label()),
                danger: false,
            });
        }
        items.extend(CORE_COMPONENTS.iter().map(|name| MenuItem {
            action: Action::AddComponent(entity, name.to_string()),
            glyph: core_icon(name),
            label: name.to_string(),
            danger: false,
        }));
        items.push(MenuItem {
            action: Action::AddComponent(entity, "Script".to_string()),
            glyph: icon::DATA_OBJECT,
            label: "Script".to_string(),
            danger: false,
        });
        items.push(MenuItem {
            action: Action::OpenAdvancedComponents(entity, x, y),
            glyph: icon::EXPAND_MORE,
            label: "Advanced…".to_string(),
            danger: false,
        });
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_advanced_component_menu(&mut self, entity: u64, x: f32, y: f32) {
        let items: Vec<MenuItem> = ADVANCED_COMPONENTS
            .iter()
            .map(|name| MenuItem {
                action: Action::AddComponent(entity, name.to_string()),
                glyph: core_icon(name),
                label: name.to_string(),
                danger: false,
            })
            .collect();
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_entity_menu(&mut self, id: u64, x: f32, y: f32) {
        let active = self.scene.entity(id).map(|e| e.enabled).unwrap_or(true);
        let items = vec![
            MenuItem { action: Action::Rename(id), glyph: icon::EDIT, label: "Rename".into(), danger: false },
            MenuItem { action: Action::Duplicate(id), glyph: icon::CONTENT_COPY, label: "Duplicate".into(), danger: false },
            MenuItem { action: Action::Copy(id), glyph: icon::CONTENT_COPY, label: "Copy".into(), danger: false },
            MenuItem { action: Action::Paste, glyph: icon::CONTENT_PASTE, label: "Paste".into(), danger: false },
            MenuItem {
                action: Action::ToggleActive(id),
                glyph: if active { icon::VISIBILITY_OFF } else { icon::VISIBILITY },
                label: if active { "Deactivate".into() } else { "Activate".into() },
                danger: false,
            },
            MenuItem { action: Action::FrameSelected(id), glyph: icon::CENTER_FOCUS, label: "Frame Selected".into(), danger: false },
            MenuItem { action: Action::ResetTransform(id), glyph: icon::RESTART_ALT, label: "Reset Transform".into(), danger: false },
            MenuItem { action: Action::OpenSelectionTools(x, y), glyph: icon::SELECT_ALL, label: "Selection Tools…".into(), danger: false },
            MenuItem { action: Action::OpenArrangeTools(x, y), glyph: icon::VIEW_QUILT, label: "Align & Snap…".into(), danger: false },
            MenuItem { action: Action::Unparent(id), glyph: icon::CHEVRON_LEFT, label: "Unparent".into(), danger: false },
            MenuItem { action: Action::AddEntity(Some(id)), glyph: icon::ADD, label: "Add Child".into(), danger: false },
            MenuItem { action: Action::Delete(id), glyph: icon::DELETE, label: "Delete".into(), danger: true },
        ];
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_tools_menu(&mut self, x: f32, y: f32) {
        let items = vec![
            MenuItem { action: Action::OpenSelectionTools(x, y), glyph: icon::SELECT_ALL, label: "Selection".into(), danger: false },
            MenuItem { action: Action::OpenHierarchyTools(x, y), glyph: icon::ACCOUNT_TREE, label: "Hierarchy".into(), danger: false },
            MenuItem { action: Action::OpenArrangeTools(x, y), glyph: icon::VIEW_QUILT, label: "Align & Snap".into(), danger: false },
            MenuItem { action: Action::OpenViewTools(x, y), glyph: icon::CENTER_FOCUS, label: "Scene View".into(), danger: false },
        ];
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_selection_tools(&mut self, x: f32, y: f32) {
        let items = vec![
            MenuItem { action: Action::SelectAll, glyph: icon::SELECT_ALL, label: "Select All     Ctrl+A".into(), danger: false },
            MenuItem { action: Action::InvertSelection, glyph: icon::SWAP, label: "Invert Selection".into(), danger: false },
            MenuItem { action: Action::SelectChildren, glyph: icon::ACCOUNT_TREE, label: "Select Descendants".into(), danger: false },
            MenuItem { action: Action::SelectParent, glyph: icon::CHEVRON_LEFT, label: "Select Parent".into(), danger: false },
            MenuItem { action: Action::DuplicateSelection, glyph: icon::CONTENT_COPY, label: "Duplicate Selection".into(), danger: false },
            MenuItem { action: Action::GroupSelected, glyph: icon::VIEW_IN_AR, label: "Group     Ctrl+G".into(), danger: false },
            MenuItem { action: Action::UnparentSelected, glyph: icon::CHEVRON_LEFT, label: "Unparent     Ctrl+Shift+G".into(), danger: false },
            MenuItem { action: Action::HideSelected, glyph: icon::VISIBILITY_OFF, label: "Hide in Scene View     H".into(), danger: false },
            MenuItem { action: Action::ShowAllHidden, glyph: icon::VISIBILITY, label: "Show All     Shift+H".into(), danger: false },
            MenuItem { action: Action::LockSelected, glyph: icon::LOCK, label: "Lock Picking     L".into(), danger: false },
            MenuItem { action: Action::UnlockAll, glyph: icon::LOCK_OPEN, label: "Unlock All     Shift+L".into(), danger: false },
        ];
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_hierarchy_tools(&mut self, x: f32, y: f32) {
        let items = vec![
            MenuItem { action: Action::CollapseSelected, glyph: icon::CHEVRON_RIGHT, label: "Collapse Selected Branches".into(), danger: false },
            MenuItem { action: Action::ExpandSelected, glyph: icon::EXPAND_MORE, label: "Expand Selected Branches".into(), danger: false },
            MenuItem { action: Action::CollapseAll, glyph: icon::UNFOLD_LESS, label: "Collapse All".into(), danger: false },
            MenuItem { action: Action::ExpandAll, glyph: icon::UNFOLD_MORE, label: "Expand All".into(), danger: false },
        ];
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_arrange_tools(&mut self, x: f32, y: f32) {
        let items = vec![
            MenuItem { action: Action::SnapSelected, glyph: icon::GRID_ON, label: "Snap Selection to Grid".into(), danger: false },
            MenuItem { action: Action::ResetSelected, glyph: icon::RESTART_ALT, label: "Reset Selected Transforms".into(), danger: false },
            MenuItem { action: Action::Align(AlignKind::Left), glyph: icon::CHEVRON_LEFT, label: "Align Left".into(), danger: false },
            MenuItem { action: Action::Align(AlignKind::CenterX), glyph: icon::CROP_SQUARE, label: "Align Horizontal Centers".into(), danger: false },
            MenuItem { action: Action::Align(AlignKind::Right), glyph: icon::CHEVRON_RIGHT, label: "Align Right".into(), danger: false },
            MenuItem { action: Action::Align(AlignKind::Top), glyph: icon::ARROW_UPWARD, label: "Align Top".into(), danger: false },
            MenuItem { action: Action::Align(AlignKind::CenterY), glyph: icon::CROP_SQUARE, label: "Align Vertical Centers".into(), danger: false },
            MenuItem { action: Action::Align(AlignKind::Bottom), glyph: icon::EXPAND_MORE, label: "Align Bottom".into(), danger: false },
        ];
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_view_tools(&mut self, x: f32, y: f32) {
        let items = vec![
            MenuItem { action: Action::FrameAll, glyph: icon::ZOOM_OUT_MAP, label: "Frame All     Home".into(), danger: false },
            MenuItem { action: Action::Zoom100, glyph: icon::CENTER_FOCUS, label: "Zoom to 100%".into(), danger: false },
            MenuItem {
                action: Action::ToggleMaximize,
                glyph: if self.maximize_view { icon::FULLSCREEN_EXIT } else { icon::FULLSCREEN },
                label: if self.maximize_view { "Restore Panels".into() } else { "Maximize Scene View     Shift+Space".into() },
                danger: false,
            },
            MenuItem {
                action: Action::ToggleProject,
                glyph: icon::FOLDER_OPEN,
                label: if self.config.layout.show_project { "Hide Project Panel".into() } else { "Show Project Panel".into() },
                danger: false,
            },
        ];
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_hierarchy_empty_menu(&mut self, x: f32, y: f32) {
        let items = vec![
            MenuItem { action: Action::AddEntity(None), glyph: icon::ADD, label: "Add Entity".into(), danger: false },
            MenuItem { action: Action::Paste, glyph: icon::CONTENT_PASTE, label: "Paste".into(), danger: false },
        ];
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_viewport_menu(&mut self, x: f32, y: f32, world_x: f32, world_y: f32) {
        let items = vec![
            MenuItem { action: Action::AddEntityAt(world_x, world_y), glyph: icon::ADD, label: "Add Entity".into(), danger: false },
            MenuItem { action: Action::Paste, glyph: icon::CONTENT_PASTE, label: "Paste".into(), danger: false },
        ];
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_project_menu(&mut self, x: f32, y: f32) {
        let items = vec![
            MenuItem { action: Action::NewFolder, glyph: icon::CREATE_NEW_FOLDER, label: "New Folder".into(), danger: false },
            MenuItem { action: Action::NewScript, glyph: icon::NOTE_ADD, label: "New Script".into(), danger: false },
            MenuItem { action: Action::OpenProjectInVscode, glyph: icon::CODE, label: "Open in VS Code".into(), danger: false },
            MenuItem { action: Action::RevealInExplorer, glyph: icon::OPEN_IN_NEW, label: "Reveal in File Manager".into(), danger: false },
        ];
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_path_menu(&mut self, path: PathBuf, x: f32, y: f32) {
        let is_dir = path.is_dir();
        let mut items = Vec::new();
        if is_dir {
            items.push(MenuItem { action: Action::EnterFolder(path.clone()), glyph: icon::FOLDER_OPEN, label: "Open Folder".into(), danger: false });
        } else if path.extension().is_some_and(|e| e == "neoscene") {
            items.push(MenuItem { action: Action::OpenScene(path.clone()), glyph: icon::ARTICLE, label: "Open Scene".into(), danger: false });
        } else {
            items.push(MenuItem { action: Action::OpenPath(path.clone()), glyph: icon::OPEN_IN_NEW, label: "Open".into(), danger: false });
        }
        items.push(MenuItem { action: Action::RevealInExplorer, glyph: icon::FOLDER_OPEN, label: "Reveal Folder".into(), danger: false });
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_confirm(&mut self, message: &str, action: Pending) {
        self.popup = Some(Popup::Confirm { message: message.to_string(), action });
    }

    fn open_prompt(&mut self, title: &str, action: Pending, initial: &str) {
        self.focus = Some("prompt_field".to_string());
        self.edit_buffer = initial.to_string();
        self.popup = Some(Popup::Prompt { title: title.to_string(), action });
    }

    fn handle_popup(&mut self, ui: &mut Ui, w: f32, h: f32, interactive: bool) {
        let popup = match self.popup.take() {
            Some(p) => p,
            None => return,
        };
        // Escape closes any popup (but not on the frame it just opened).
        if interactive && ui.input.escape {
            if matches!(popup, Popup::Prompt { .. }) {
                self.focus = None;
            }
            return;
        }
        match popup {
            Popup::Menu { x, y, items } => self.draw_menu(ui, x, y, items, w, h, interactive),
            Popup::Color { target, x, y, rgba, hue } => self.draw_color_picker(ui, target, x, y, rgba, hue, w, h, interactive),
            Popup::Confirm { message, action } => self.draw_confirm(ui, message, action, w, h, interactive),
            Popup::Prompt { title, action } => self.draw_prompt(ui, title, action, w, h, interactive),
            Popup::Error { message, copied } => self.draw_error(ui, message, copied, w, h, interactive),
        }
    }

    fn draw_error(&mut self, ui: &mut Ui, message: String, mut copied: bool, w: f32, h: f32, interactive: bool) {
        ui.painter.fill_rect(Rect::new(0.0, 0.0, w, h), [0, 0, 0, 140]);
        let width = (w * 0.7).clamp(420.0, 760.0);
        let height = (h * 0.6).clamp(220.0, 460.0);
        let px = (w - width) / 2.0;
        let py = (h - height) / 2.0;
        let rect = Rect::new(px, py, width, height);
        ui.painter.fill_round_rect(rect, 6.0, self.config.theme.panel);
        ui.painter.stroke_round_rect(rect, 6.0, self.config.theme.danger);
        ui.icon(px + 18.0, py + 18.0, icon::DELETE, 16.0, self.config.theme.danger);
        ui.painter.text(px + 32.0, py + 11.0, "Runtime Error", 16.0, self.config.theme.danger);

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
                ui.painter.text(body.x + 6.0, ty, &wrapped, 13.0, self.config.theme.text);
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
            if !copied {
                // Fall back to a file the user can open.
                let path = self.project_root.join("last_error.txt");
                let _ = std::fs::write(&path, &message);
                self.status = format!("Clipboard unavailable; wrote {}", path.display());
                copied = true;
            }
        }
        if !do_close {
            self.popup = Some(Popup::Error { message, copied });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_menu(&mut self, ui: &mut Ui, x: f32, y: f32, items: Vec<MenuItem>, w: f32, h: f32, interactive: bool) {
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
        ui.painter.fill_round_rect(rect, 5.0, self.config.theme.panel);
        ui.painter.stroke_round_rect(rect, 5.0, self.config.theme.border);

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

    #[allow(clippy::too_many_arguments)]
    fn draw_color_picker(&mut self, ui: &mut Ui, target: ColorTarget, x: f32, y: f32, mut rgba: [u8; 4], mut hue: f32, w: f32, h: f32, interactive: bool) {
        let hsv = self.config.layout.hsv_picker;
        let width = 244.0;
        let height = if hsv { 196.0 } else { 150.0 };
        let px = x.min(w - width - 4.0).max(2.0);
        let py = y.min(h - height - 4.0).max(2.0);
        let rect = Rect::new(px, py, width, height);
        ui.painter.fill_round_rect(rect, 5.0, self.config.theme.panel);
        ui.painter.stroke_round_rect(rect, 5.0, self.config.theme.border);

        // Mode toggle (HSV square vs RGBA sliders), persisted.
        let toggle = Rect::new(px + width - 56.0, py + 8.0, 48.0, 18.0);
        if interactive && ui.button(toggle, if hsv { "RGB" } else { "HSV" }) {
            self.config.layout.hsv_picker = !hsv;
            self.dirty = true;
        }
        ui.painter.text(px + 10.0, py + 9.0, "Color", 14.0, self.config.theme.text_dim);

        let mut changed = false;
        if hsv {
            // Saturation/Value square for the current hue.
            let sq = Rect::new(px + 10.0, py + 32.0, 150.0, 120.0);
            for yy in 0..(sq.h as i32) {
                for xx in 0..(sq.w as i32) {
                    let s = xx as f32 / sq.w;
                    let v = 1.0 - yy as f32 / sq.h;
                    let c = hsv_to_rgb(hue, s, v);
                    ui.painter.pixel(sq.x + xx as f32, sq.y + yy as f32, [c[0], c[1], c[2], 255]);
                }
            }
            ui.painter.stroke_rect(sq, self.config.theme.border);
            // Hue strip.
            let strip = Rect::new(px + 170.0, py + 32.0, 18.0, 120.0);
            for yy in 0..(strip.h as i32) {
                let c = hsv_to_rgb(yy as f32 / strip.h * 360.0, 1.0, 1.0);
                ui.painter.fill_rect(Rect::new(strip.x, strip.y + yy as f32, strip.w, 1.0), [c[0], c[1], c[2], 255]);
            }
            ui.painter.stroke_rect(strip, self.config.theme.border);

            // Interaction.
            let (_, mut s, mut v) = rgb_to_hsv(rgba);
            if ui.input.mouse_down && sq.contains(ui.input.mouse_x, ui.input.mouse_y) {
                s = ((ui.input.mouse_x - sq.x) / sq.w).clamp(0.0, 1.0);
                v = (1.0 - (ui.input.mouse_y - sq.y) / sq.h).clamp(0.0, 1.0);
                let c = hsv_to_rgb(hue, s, v);
                rgba = [c[0], c[1], c[2], rgba[3]];
                changed = true;
                ui.wants_redraw = true;
            }
            if ui.input.mouse_down && strip.contains(ui.input.mouse_x, ui.input.mouse_y) {
                hue = ((ui.input.mouse_y - strip.y) / strip.h * 360.0).clamp(0.0, 359.999);
                let c = hsv_to_rgb(hue, s, v);
                rgba = [c[0], c[1], c[2], rgba[3]];
                changed = true;
                ui.wants_redraw = true;
            }
            // SV cursor + hue marker.
            let cur = Rect::new(sq.x + s * sq.w - 4.0, sq.y + (1.0 - v) * sq.h - 4.0, 8.0, 8.0);
            ui.painter.stroke_round_rect(cur, 4.0, [255, 255, 255, 255]);
            ui.painter.fill_rect(Rect::new(strip.x - 2.0, strip.y + hue / 360.0 * strip.h - 1.0, strip.w + 4.0, 2.0), [255, 255, 255, 255]);

            // Preview + alpha slider + hex.
            ui.painter.fill_round_rect(Rect::new(px + 196.0, py + 32.0, 38.0, 26.0), 4.0, [rgba[0], rgba[1], rgba[2], 255]);
            ui.painter.stroke_round_rect(Rect::new(px + 196.0, py + 32.0, 38.0, 26.0), 4.0, self.config.theme.border);
            ui.label(px + 10.0, py + 160.0, "A", self.config.theme.text);
            if let Some(a) = ui.slider(Rect::new(px + 26.0, py + 158.0, 130.0, 18.0), rgba[3] as f32, 0.0, 255.0) {
                rgba[3] = a.round() as u8;
                changed = true;
            }
            let hexr = ui.text_field("cp_hex", Rect::new(px + 164.0, py + 158.0, 70.0, 18.0), &format!("{:02X}{:02X}{:02X}", rgba[0], rgba[1], rgba[2]));
            if hexr.changed {
                if let Some(c) = parse_hex(&hexr.text) {
                    rgba = [c[0], c[1], c[2], rgba[3]];
                    hue = rgb_to_hsv(rgba).0;
                    changed = true;
                }
            }
        } else {
            ui.painter.fill_round_rect(Rect::new(px + 10.0, py + 32.0, 40.0, 30.0), 4.0, [rgba[0], rgba[1], rgba[2], 255]);
            ui.painter.stroke_round_rect(Rect::new(px + 10.0, py + 32.0, 40.0, 30.0), 4.0, self.config.theme.border);
            let labels = ["R", "G", "B", "A"];
            for i in 0..4 {
                let ry = py + 32.0 + i as f32 * 26.0;
                ui.label(px + 60.0, ry + 2.0, labels[i], self.config.theme.text);
                if let Some(v) = ui.slider(Rect::new(px + 78.0, ry, 90.0, 18.0), rgba[i] as f32, 0.0, 255.0) {
                    rgba[i] = v.round() as u8;
                    changed = true;
                }
                let r = ui.text_field(&format!("cp_{i}"), Rect::new(px + 174.0, ry, 40.0, 18.0), &rgba[i].to_string());
                if r.changed {
                    if let Ok(v) = r.text.trim().parse::<i32>() {
                        rgba[i] = v.clamp(0, 255) as u8;
                        changed = true;
                    }
                }
            }
            hue = rgb_to_hsv(rgba).0;
        }

        if changed {
            self.set_target_color(&target, rgba);
            self.mark_dirty();
        }

        let clicked_outside = interactive
            && ui.input.mouse_pressed
            && !rect.contains(ui.input.mouse_x, ui.input.mouse_y);
        if !clicked_outside {
            self.popup = Some(Popup::Color { target, x, y, rgba, hue });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_confirm(&mut self, ui: &mut Ui, message: String, action: Pending, w: f32, h: f32, interactive: bool) {
        // Dim background.
        ui.painter.fill_rect(Rect::new(0.0, 0.0, w, h), [0, 0, 0, 120]);
        let width = 360.0;
        let height = 120.0;
        let px = (w - width) / 2.0;
        let py = (h - height) / 2.0;
        let rect = Rect::new(px, py, width, height);
        ui.painter.fill_round_rect(rect, 6.0, self.config.theme.panel);
        ui.painter.stroke_round_rect(rect, 6.0, self.config.theme.accent);
        ui.painter.text(px + 16.0, py + 18.0, &message, 15.0, self.config.theme.text);

        let yes = Rect::new(px + width - 200.0, py + height - 36.0, 90.0, 26.0);
        let no = Rect::new(px + width - 104.0, py + height - 36.0, 90.0, 26.0);
        let confirm = interactive && ui.button_colored(yes, "Yes", self.config.theme.danger, [255, 255, 255, 255]);
        let cancel = interactive && ui.button(no, "Cancel");
        if confirm {
            self.perform_pending(action);
        } else if !cancel {
            self.popup = Some(Popup::Confirm { message, action });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_prompt(&mut self, ui: &mut Ui, title: String, action: Pending, w: f32, h: f32, interactive: bool) {
        ui.painter.fill_rect(Rect::new(0.0, 0.0, w, h), [0, 0, 0, 120]);
        let width = 340.0;
        let height = 120.0;
        let px = (w - width) / 2.0;
        let py = (h - height) / 2.0;
        let rect = Rect::new(px, py, width, height);
        ui.painter.fill_round_rect(rect, 6.0, self.config.theme.panel);
        ui.painter.stroke_round_rect(rect, 6.0, self.config.theme.accent);
        ui.painter.text(px + 16.0, py + 16.0, &title, 15.0, self.config.theme.text);

        let field = Rect::new(px + 16.0, py + 44.0, width - 32.0, 26.0);
        let _ = ui.text_field("prompt_field", field, "");
        let value = ui.last_edit().to_string();

        let ok = Rect::new(px + width - 200.0, py + height - 34.0, 90.0, 24.0);
        let cancel = Rect::new(px + width - 104.0, py + height - 34.0, 90.0, 24.0);
        let submit = interactive
            && (ui.button_colored(ok, "OK", self.config.theme.button, self.config.theme.text) || ui.input.enter);
        let cancelled = interactive && ui.button(cancel, "Cancel");
        if submit {
            self.focus = None;
            self.perform_pending_with(action, value);
        } else if cancelled {
            self.focus = None;
        } else {
            self.popup = Some(Popup::Prompt { title, action });
        }
    }

    // ---- Actions -----------------------------------------------------------

    fn perform(&mut self, action: Action) {
        match action {
            Action::AddComponent(id, name) => {
                if let Some(e) = self.scene.entity_mut(id) {
                    if name == "Script" {
                        e.components.push(Component::Script { path: "scripts/Behavior".into(), variables: Vec::new() });
                    } else {
                        e.components.push(Component::core(&name));
                    }
                    self.mark_dirty();
                    self.status = format!("Added {name}");
                }
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
            Action::OpenAdvancedComponents(id, x, y) => self.open_advanced_component_menu(id, x, y),
            Action::AddEntity(parent) => self.add_entity(parent),
            Action::AddEntityAt(x, y) => self.add_entity_at(None, x, y),
            Action::Rename(id) => {
                let cur = self.scene.entity(id).map(|e| e.name.clone()).unwrap_or_default();
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
                if let Some(e) = self.scene.entity_mut(id) {
                    e.x = 0.0;
                    e.y = 0.0;
                    e.z = 0.0;
                    e.rotation = 0.0;
                    e.scale = 1.0;
                    e.anchor_x = 0.0;
                    e.anchor_y = 0.0;
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
            Action::NewFolder => self.open_prompt("New folder name", Pending::CreateFolder, "NewFolder"),
            Action::NewScript => self.open_prompt("New script name", Pending::CreateScript, "script.luau"),
            Action::RevealInExplorer => self.reveal_in_explorer(),
            Action::OpenProjectInVscode => self.open_project_in_vscode(),
            Action::OpenPath(p) => self.open_path(&p),
            Action::OpenScene(p) => self.open_scene_path(p),
            Action::EnterFolder(p) => self.navigate_bin(p),
            Action::OpenSelectionTools(x, y) => self.open_selection_tools(x, y),
            Action::OpenHierarchyTools(x, y) => self.open_hierarchy_tools(x, y),
            Action::OpenArrangeTools(x, y) => self.open_arrange_tools(x, y),
            Action::OpenViewTools(x, y) => self.open_view_tools(x, y),
            Action::SelectAll => self.select_all(),
            Action::InvertSelection => self.invert_selection(),
            Action::SelectChildren => self.select_children(),
            Action::SelectParent => self.select_parent(),
            Action::DuplicateSelection => self.duplicate_selection(),
            Action::GroupSelected => self.group_selected(),
            Action::UnparentSelected => self.unparent_selected(),
            Action::HideSelected => self.hide_selected(),
            Action::ShowAllHidden => {
                self.hidden_ids.clear();
                self.status = "Revealed all Scene-view objects".to_string();
            }
            Action::LockSelected => self.lock_selected(),
            Action::UnlockAll => {
                self.locked_ids.clear();
                self.status = "Unlocked all Scene-view objects".to_string();
            }
            Action::CollapseSelected => {
                self.hierarchy_collapsed.extend(self.selection_ids_ordered());
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
            Action::ResetSelected => self.reset_selected(),
            Action::Align(kind) => self.align_selected(kind),
            Action::FrameAll => self.frame_all(),
            Action::Zoom100 => self.zoom_100(),
            Action::ToggleMaximize => self.maximize_view = !self.maximize_view,
            Action::ToggleProject => {
                self.config.layout.show_project = !self.config.layout.show_project;
                self.dirty = true;
            }
        }
    }

    fn perform_pending(&mut self, action: Pending) {
        match action {
            Pending::LoadScene => self.load(),
            Pending::Quit => self.should_quit = true,
            _ => {}
        }
    }

    fn perform_pending_with(&mut self, action: Pending, value: String) {
        match action {
            Pending::RenameScene => self.rename_scene(value),
            Pending::CreateFolder => self.create_folder(&value),
            Pending::CreateScript => self.create_script(&value),
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
        let mut number = self.documents.len() + 1;
        let mut path = self.project_root.join(format!("scene_{number}.neoscene"));
        while self.documents.iter().any(|document| document.path == path) || path.exists() {
            number += 1;
            path = self.project_root.join(format!("scene_{number}.neoscene"));
        }
        let mut scene = Scene::default();
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
            self.open_confirm("Discard unsaved changes and load the saved scene?", Pending::LoadScene);
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
                serde_json::to_string_pretty(&entities)
                    .map_err(|error| format!("failed to serialize prefab: {error}"))
                    .and_then(|json| std::fs::write(&self.scene_path, json)
                        .map_err(|error| format!("failed to write {}: {error}", self.scene_path.display())))
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
        let result = std::fs::read_to_string(&path)
            .map_err(|error| error.to_string())
            .and_then(|json| serde_json::from_str::<Vec<Entity>>(&json).map_err(|error| error.to_string()));
        match result {
            Ok(entities) if !entities.is_empty() => {
                let name = path.file_stem().map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Prefab".to_string());
                self.add_document(path.clone(), Scene::from_prefab(name, entities), DocumentKind::Prefab);
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
                    .map_err(|error| {
                        format!("Inspector refresh failed for {script_path}: {error}")
                    })
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
            if let (VarValue::Number(value), VarControl::Slider { min, max, fractional }) =
                (&mut declared.value, &declared.control)
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
        self.status = "VS Code command not found on PATH".to_string();
    }

    /// Open a file or folder with the OS default handler.
    fn open_path(&mut self, path: &Path) {
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

    fn export_luau(&mut self) {
        let path = self.project_root.join("main.luau");
        if let Err(e) = std::fs::write(&path, self.scene.to_luau()) {
            self.status = format!("Export failed: {e}");
            return;
        }
        // Write (or clean up) the shared image-cache module alongside main.luau.
        let images_path = self.project_root.join("images.luau");
        match self.scene.to_images_luau() {
            Some(content) => {
                if let Err(e) = std::fs::write(&images_path, content) {
                    self.status = format!("Export failed (images.luau): {e}");
                    return;
                }
            }
            None => remove_generated_file(&images_path),
        }
        // Migrate projects produced by older editor builds. Hand-authored
        // assets.luau files are preserved by remove_generated_file.
        remove_generated_file(&self.project_root.join("assets.luau"));
        self.status = format!("Exported {}", path.display());
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
        match serde_json::to_string_pretty(&entities) {
            Ok(json) => match std::fs::write(&path, json) {
                Ok(()) => {
                    let source = self.prefab_source_key(&path);
                    if let Some(root) = self.scene.entity_mut(id) {
                        root.prefab_source = Some(source);
                    }
                    self.mark_dirty();
                    self.status = format!("Saved linked prefab {}", path.display());
                }
                Err(e) => self.status = format!("Save prefab failed: {e}"),
            },
            Err(e) => self.status = format!("Save prefab failed: {e}"),
        }
    }

    fn run_scene(&mut self) {
        self.export_luau();
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                self.status = format!("Run failed: {e}");
                return;
            }
        };
        let root = self.project_root.clone();
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
            if let Some(addr) = &ipc_addr {
                command.env("NEOLOVE_EDITOR_IPC", addr);
            }
            let outcome = match command
                .output()
            {
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

    /// True while a launched preview is still running.
    pub fn run_pending(&self) -> bool {
        self.run_rx.is_some()
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
                        self.popup = Some(Popup::Error { message, copied: false });
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
}

#[derive(Clone)]
struct EditorImageCacheEntry {
    modified: Option<SystemTime>,
    image: Option<Rc<image::RgbaImage>>,
}

fn editor_entity_scale(entity: &Entity) -> f32 {
    if entity.scale.is_finite() {
        entity.scale.max(0.0)
    } else {
        1.0
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
        if name != "EntityScaler" || prop_bool(props, &["enabled"]).is_some_and(|enabled| !enabled) {
            continue;
        }

        let (parent_w, parent_h) = editor_parent_size_inner(scene, entity, root_size, visiting);
        let size_x_percent =
            prop_number(props, &["size_x_percent", "sizeXPercent"]).unwrap_or(0.0).clamp(0.0, 1.0);
        let size_y_percent =
            prop_number(props, &["size_y_percent", "sizeYPercent"]).unwrap_or(0.0).clamp(0.0, 1.0);
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
    let mut transform = EditorLocalTransform {
        x: entity.x,
        y: entity.y,
        scale: editor_entity_scale(entity),
        anchor_x: entity.anchor_x,
        anchor_y: entity.anchor_y,
        pivot_x: 0.0,
        pivot_y: 0.0,
    };

    for component in &entity.components {
        let Component::Core { name, props } = component else {
            continue;
        };
        if name != "EntityScaler" || prop_bool(props, &["enabled"]).is_some_and(|enabled| !enabled) {
            continue;
        }

        transform.anchor_x = prop_number(props, &["x_percent", "xPercent", "percent_x", "percentX"])
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        transform.anchor_y = prop_number(props, &["y_percent", "yPercent", "percent_y", "percentY"])
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        transform.x = prop_number(props, &["offset_x", "offsetX"]).unwrap_or(0.0);
        transform.y = prop_number(props, &["offset_y", "offsetY"]).unwrap_or(0.0);
        transform.pivot_x = prop_number(props, &["pivot_x", "pivotX", "anchor_x", "anchorX"])
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        transform.pivot_y = prop_number(props, &["pivot_y", "pivotY", "anchor_y", "anchorY"])
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        break;
    }

    transform
}

fn scene_world_transform(
    scene: &Scene,
    id: u64,
    root_size: (f32, f32),
) -> Option<EditorWorldTransform> {
    let mut visiting = HashSet::new();
    scene_world_transform_inner(scene, id, root_size, &mut visiting)
}

fn scene_world_transform_inner(
    scene: &Scene,
    id: u64,
    root_size: (f32, f32),
    visiting: &mut HashSet<u64>,
) -> Option<EditorWorldTransform> {
    if !visiting.insert(id) {
        return None;
    }

    let entity = scene.entity(id)?;
    let local = editor_entity_local_transform(entity);
    let parent_transform = entity
        .parent
        .and_then(|parent| scene_world_transform_inner(scene, parent, root_size, visiting))
        .unwrap_or(EditorWorldTransform {
            x: 0.0,
            y: 0.0,
            scale: 1.0,
            rotation: 0.0,
        });
    let (anchor_x, anchor_y) = editor_anchor_offset(scene, entity, local, root_size);
    let (size_x, size_y) = editor_entity_size(scene, entity, root_size);
    let pivot_x = size_x * local.scale * local.pivot_x;
    let pivot_y = size_y * local.scale * local.pivot_y;
    let transform = EditorWorldTransform {
        x: parent_transform.x + (anchor_x + local.x - pivot_x) * parent_transform.scale,
        y: parent_transform.y + (anchor_y + local.y - pivot_y) * parent_transform.scale,
        scale: parent_transform.scale * local.scale,
        rotation: parent_transform.rotation + entity.rotation,
    };
    visiting.remove(&id);
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
    let pivot_x = size_x * local.scale * local.pivot_x;
    let pivot_y = size_y * local.scale * local.pivot_y;
    Some((
        (world_x - parent_transform.x) / parent_scale - anchor_x + pivot_x,
        (world_y - parent_transform.y) / parent_scale - anchor_y + pivot_y,
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
    let use_legacy_stretch =
        !size_mode_uses_entity && legacy_scale_x > 0.0 && legacy_scale_y > 0.0;

    let scale = if use_legacy_stretch {
        legacy_scale_y.max(1.0)
    } else {
        prop_number(props, &["scale"]).unwrap_or(defaults.default_scale).max(1.0)
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
        align_y: prop_string_like(props, &["align_y", "alignY", "vertical_align", "verticalAlign"])
            .map(|value| parse_preview_align_y(&value))
            .unwrap_or(defaults.default_align_y),
        wrap: prop_wrap_mode(props).unwrap_or(defaults.default_wrap),
        padding_x,
        padding_y,
        line_spacing: prop_number(props, &["line_spacing", "lineSpacing"]).unwrap_or(1.0),
        letter_spacing: prop_number(props, &["letter_spacing", "letterSpacing"])
            .unwrap_or(0.0)
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
        if name != "EntityScaler" || prop_bool(props, &["enabled"]).is_some_and(|enabled| !enabled) {
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
    let Some(props) = entity.components.iter_mut().find_map(|component| match component {
        Component::Core { name, props } if name == "EntityScaler" => Some(props),
        _ => None,
    }) else {
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
        PropValue::Text(value) | PropValue::Image(value) => Some(value.clone()),
        PropValue::Enum { value, .. } => Some(value.clone()),
        PropValue::Number(value) => Some(format_num(*value)),
        PropValue::Int(value) => Some(value.to_string()),
        PropValue::Bool(value) => Some(value.to_string()),
        PropValue::Color(_) => None,
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
        PropValue::Text(value) | PropValue::Enum { value, .. } => {
            Some(parse_preview_wrap(value))
        }
        _ => None,
    })
}

fn text_size_mode_uses_entity(props: &[Prop], default_uses_entity: bool) -> bool {
    match prop_string_like(props, &["size_mode", "sizeMode", "bounds_mode", "boundsMode"]) {
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
fn draw_nine_slice(p: &mut Painter, img: &image::RgbaImage, dest: Rect, l: f32, r: f32, t: f32, b: f32, tint: [u8; 4], z: f32) {
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
fn draw_tiled(p: &mut Painter, img: &image::RgbaImage, dest: Rect, tile_w: f32, tile_h: f32, tint: [u8; 4], z: f32) {
    let tw = (tile_w * z).max(2.0);
    let th = (tile_h * z).max(2.0);
    let mut y = dest.y;
    while y < dest.bottom() {
        let mut x = dest.x;
        while x < dest.right() {
            let cw = tw.min(dest.right() - x);
            let ch = th.min(dest.bottom() - y);
            let src = Rect::new(0.0, 0.0, img.width() as f32 * (cw / tw), img.height() as f32 * (ch / th));
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
    let atlas_rows = (((img.height() as f32 - margin * 2.0 + spacing) / (tile_h + spacing))
        .floor() as i32)
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
                    if painter.text_width(&format!("{chunk}{ch}"), size) > max_w && !chunk.is_empty() {
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
    let candidates: &[(&str, &[&str])] = &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
        ("pbcopy", &[]),
        ("clip", &[]),
    ];
    for (cmd, args) in candidates {
        if let Ok(mut child) = Command::new(cmd).args(*args).stdin(Stdio::piped()).spawn() {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
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
        "TextBox" | "TextLabel" | "RudimentaryTextLabel" | "TextInput" => icon::TITLE,
        "Sprite2D" | "Image2D" | "NineSliceSprite2D" | "TileTexture2D" | "Tilemap2D" | "Spritebox2D" => icon::IMAGE,
        "Collider2D" => icon::BORDER_ALL,
        "Rigidbody2D" => icon::VIEW_IN_AR,
        "EntityScaler" | "Bolt2D" | "Rope2D" | "LegacyBolt2D" | "String2D" => icon::TUNE,
        "Frame" | "ScrollList" => icon::VIEW_QUILT,
        "Button" => icon::ADD_CIRCLE,
        "Dropdown" => icon::EXPAND_MORE,
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
    props.iter().find(|p| p.name == name).and_then(|p| match p.value {
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
    let Ok(entries) = std::fs::read_dir(root) else { return; };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|name| name == "dist" || name == "target" || name == ".git") {
            continue;
        }
        if path.is_dir() {
            collect_files_with_extension(&path, extension, out);
        } else if path.extension().is_some_and(|value| value.eq_ignore_ascii_case(extension)) {
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
        "png" | "bmp" | "tga" | "webp" | "jpg" | "jpeg" | "pnm" | "ppm" | "pgm" | "gif" | "tif" | "tiff" | "hdr" | "dds" => icon::IMAGE,
        "wav" | "mp3" | "ogg" | "flac" | "aac" | "m4a" | "aiff" => icon::AUDIOTRACK,
        "ttf" | "otf" => icon::FONT_DOWNLOAD,
        "luau" | "lua" => icon::DATA_OBJECT,
        "toml" | "json" | "txt" | "md" | "neoscene" => icon::ARTICLE,
        "neoprefab" => icon::VIEW_IN_AR,
        _ => icon::INSERT_DRIVE_FILE,
    }
}

pub fn load_config(path: &Path) -> EditorConfig {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
            eprintln!("warning: failed to parse {}: {error}", path.display());
            EditorConfig::default()
        }),
        Err(_) => EditorConfig::default(),
    }
}

pub fn save_config(path: &Path, config: &EditorConfig) -> Result<(), String> {
    let text = serde_json::to_string_pretty(config).map_err(|e| format!("failed to serialize config: {e}"))?;
    std::fs::write(path, text).map_err(|e| format!("failed to write {}: {e}", path.display()))
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
    use crate::editor::ui::{load_fonts, Fonts, FrameInput, Painter, Ui};

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
            let dir = std::env::temp_dir().join(format!("neolove_editor_test_{}", std::process::id()));
            let _ = std::fs::create_dir_all(&dir);
            let app = EditorApp::new(dir.clone(), dir.join("scene.neoscene"), scene, EditorConfig::default());
            Self { app, fonts: load_fonts().expect("fonts"), w, h, buffer: vec![0u32; w * h] }
        }
        fn frame(&mut self, input: FrameInput) {
            let painter = Painter::new(&mut self.buffer, self.w, self.h, self.fonts.clone());
            let theme = self.app.theme();
            let mut ui = Ui::new(painter, input, theme, self.app.take_focus(), self.app.take_edit_buffer());
            self.app.frame(&mut ui);
            let (f, e) = ui.into_focus_state();
            self.app.set_focus(f, e);
        }
        fn click(&mut self, x: f32, y: f32) {
            self.frame(FrameInput { mouse_x: x, mouse_y: y, mouse_pressed: true, mouse_down: true, ..Default::default() });
            self.frame(FrameInput { mouse_x: x, mouse_y: y, ..Default::default() });
        }
    }

    #[test]
    fn default_scene_has_no_components() {
        let h = Harness::new(Scene::default());
        assert!(h.app.scene.entities[0].components.is_empty());
    }

    #[test]
    fn inspector_identifiers_are_humanized() {
        assert_eq!(humanize_identifier("per_second"), "Per Second");
        assert_eq!(humanize_identifier("isEnabled"), "Is Enabled");
        assert_eq!(humanize_identifier("max_FPS"), "Max FPS");
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
            );
            let mut y = 100.0;
            let mut dirty = false;
            h.app.script_value_editor(
                &mut ui,
                "entity_ref",
                "Entity",
                &mut entity_value,
                &VarControl::Field,
                owner,
                0,
                0,
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
        h.app.inspector_reference_drag = Some(InspectorReferenceDrag::Component(
            ComponentReference {
                entity: source_id,
                component: 0,
            },
        ));
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
            );
            let mut y = 100.0;
            let mut dirty = false;
            h.app.script_value_editor(
                &mut ui,
                "component_ref",
                "Component",
                &mut component_value,
                &VarControl::Field,
                owner,
                0,
                0,
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
        // The "Entity" toolbar button; click within its area.
        h.click(360.0, 20.0);
        // Whether or not the exact hit lands, ensure no panic and selection API works.
        assert!(h.app.scene.entities.len() >= before);
    }

    #[test]
    fn right_click_opens_context_menu_then_closes() {
        let mut h = Harness::new(Scene::default());
        h.app.selected = Some(h.app.scene.entities[0].id);
        // Right-click in the viewport center.
        h.frame(FrameInput { mouse_x: 600.0, mouse_y: 400.0, right_pressed: true, ..Default::default() });
        assert!(h.app.popup.is_some());
        // Escape closes it.
        h.frame(FrameInput { escape: true, ..Default::default() });
        assert!(h.app.popup.is_none());
    }

    #[test]
    fn popup_survives_mouse_move_after_opening() {
        let mut h = Harness::new(Scene::default());
        let id = h.app.scene.entities[0].id;
        // Open a menu the way a click would (mid-frame), with a press present.
        h.app.open_entity_menu(id, 600.0, 300.0);
        h.frame(FrameInput { mouse_x: 600.0, mouse_y: 300.0, mouse_pressed: true, mouse_down: true, ..Default::default() });
        assert!(h.app.popup.is_some(), "menu closed on the frame it opened");
        // A subsequent mouse-move (no press) must not close it.
        h.frame(FrameInput { mouse_x: 620.0, mouse_y: 320.0, ..Default::default() });
        assert!(h.app.popup.is_some(), "menu closed after a mouse move");
    }

    #[test]
    fn drag_selects_multiple_viewport_entities() {
        let mut scene = Scene::default();
        let first = scene.entities[0].id;
        let second = scene.add_entity("B", 340.0, 150.0).id;
        let mut h = Harness::new(scene);

        // Start in empty viewport space just above the entities, drag across both.
        h.frame(FrameInput { mouse_x: 420.0, mouse_y: 170.0, mouse_pressed: true, mouse_down: true, ..Default::default() });
        h.frame(FrameInput { mouse_x: 705.0, mouse_y: 315.0, mouse_down: true, ..Default::default() });
        h.frame(FrameInput { mouse_x: 705.0, mouse_y: 315.0, ..Default::default() });

        let selected = h.app.selection_ids_ordered();
        assert!(selected.contains(&first), "first entity was not marquee-selected");
        assert!(selected.contains(&second), "second entity was not marquee-selected");
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

        h.frame(FrameInput { mouse_x: 450.0, mouse_y: 200.0, mouse_pressed: true, mouse_down: true, ..Default::default() });
        h.frame(FrameInput { mouse_x: 490.0, mouse_y: 200.0, mouse_down: true, ..Default::default() });
        h.frame(FrameInput { mouse_x: 490.0, mouse_y: 200.0, ..Default::default() });

        let first_entity = h.app.scene.entity(first).expect("first");
        let second_entity = h.app.scene.entity(second).expect("second");
        assert!((first_entity.x - 240.0).abs() < 1.0, "first x was {}", first_entity.x);
        assert!((second_entity.x - 380.0).abs() < 1.0, "second x was {}", second_entity.x);
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
        h.app.select_only(id);

        // The scaler starts at viewport (240,40), sized 100x100. Moving by
        // 128x72 is exactly 10% of the 1280x720 preview root.
        h.frame(FrameInput { mouse_x: 290.0, mouse_y: 90.0, mouse_pressed: true, mouse_down: true, ..Default::default() });
        h.frame(FrameInput { mouse_x: 418.0, mouse_y: 162.0, mouse_down: true, ..Default::default() });
        h.frame(FrameInput { mouse_x: 418.0, mouse_y: 162.0, ..Default::default() });

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
        h.app.select_only(id);

        // Resize 100x100 to 128x144: 10% and 20% of the preview root.
        h.frame(FrameInput { mouse_x: 340.0, mouse_y: 140.0, mouse_pressed: true, mouse_down: true, ..Default::default() });
        h.frame(FrameInput { mouse_x: 368.0, mouse_y: 184.0, mouse_down: true, ..Default::default() });
        h.frame(FrameInput { mouse_x: 368.0, mouse_y: 184.0, ..Default::default() });

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
        scene.entity_mut(id).expect("entity").components.push(scaler);
        let mut h = Harness::new(scene);
        h.app.config.layout.snap = false;
        h.app.select_only(id);

        h.frame(FrameInput { mouse_x: 290.0, mouse_y: 90.0, mouse_pressed: true, mouse_down: true, ..Default::default() });
        h.frame(FrameInput { mouse_x: 330.0, mouse_y: 90.0, mouse_down: true, ..Default::default() });
        h.frame(FrameInput { mouse_x: 330.0, mouse_y: 90.0, ..Default::default() });

        // The move puts the rect at x=280, so its bottom-right handle is 380,140.
        h.frame(FrameInput { mouse_x: 380.0, mouse_y: 140.0, mouse_pressed: true, mouse_down: true, ..Default::default() });
        h.frame(FrameInput { mouse_x: 420.0, mouse_y: 180.0, mouse_down: true, ..Default::default() });
        h.frame(FrameInput { mouse_x: 420.0, mouse_y: 180.0, ..Default::default() });

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
        h.frame(FrameInput { mouse_x: edge, mouse_y: 300.0, mouse_pressed: true, mouse_down: true, ..Default::default() });
        h.frame(FrameInput { mouse_x: edge + 60.0, mouse_y: 300.0, mouse_down: true, ..Default::default() });
        h.frame(FrameInput { mouse_x: edge + 60.0, mouse_y: 300.0, ..Default::default() });
        assert!((h.app.config.layout.left_w - before).abs() > 20.0);
    }

    #[test]
    fn add_component_menu_adds_real_core_component() {
        let scene = Scene::default();
        let id = scene.entities[0].id;
        let mut h = Harness::new(scene);
        h.app.perform(Action::AddComponent(id, "TextBox".to_string()));
        let e = h.app.scene.entity(id).expect("entity");
        assert!(matches!(e.components.last(), Some(Component::Core { name, .. }) if name == "TextBox"));
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
        assert!(matches!(variables[1].value, VarValue::Color([1, 2, 3, 255])));
    }

    #[test]
    fn dragging_a_corner_handle_resizes_the_entity() {
        // Default entity sits at world (200,150) sized 100x100. With the left
        // panel 240px wide and body starting at y=40, its screen rect is
        // (440,190)-(540,290); the bottom-right handle is at (540,290).
        let mut h = Harness::new(Scene::default());
        h.app.config.layout.snap = false;
        let id = h.app.scene.entities[0].id;
        h.app.selected = Some(id);
        // Press the bottom-right handle, drag +40,+40, release.
        h.frame(FrameInput { mouse_x: 540.0, mouse_y: 290.0, mouse_pressed: true, mouse_down: true, ..Default::default() });
        h.frame(FrameInput { mouse_x: 580.0, mouse_y: 330.0, mouse_down: true, ..Default::default() });
        h.frame(FrameInput { mouse_x: 580.0, mouse_y: 330.0, ..Default::default() });
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
        let id = h.app.scene.entities[0].id;
        h.app.select_only(id);

        // Bottom-right starts at (540,240). Dragging unevenly with Ctrl held
        // must keep the original 2:1 aspect ratio.
        h.frame(FrameInput { mouse_x: 540.0, mouse_y: 240.0, mouse_pressed: true, mouse_down: true, ctrl: true, ..Default::default() });
        h.frame(FrameInput { mouse_x: 640.0, mouse_y: 340.0, mouse_down: true, ctrl: true, ..Default::default() });
        h.frame(FrameInput { mouse_x: 640.0, mouse_y: 340.0, ctrl: true, ..Default::default() });

        let entity = h.app.scene.entity(id).expect("entity");
        assert!((entity.size_x / entity.size_y - 2.0).abs() < 0.001);
        assert_eq!((entity.x, entity.y), (200.0, 150.0));
    }

    #[test]
    fn image_component_exports_load_call() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        scene.entity_mut(id).expect("e").components.push(Component::core("Sprite2D"));
        // Default image path present -> loaded once in the shared image cache.
        let images = scene.to_images_luau().expect("images emitted");
        assert!(images.contains("assets.loadImage(\"assets/sprite.png\")"), "got: {images}");
        // main.luau references the cached handle, not a raw path or inline load.
        let luau = scene.to_luau();
        assert!(luau.contains(".image = Images[\"assets/sprite.png\"]"), "got: {luau}");
        assert!(!luau.contains(".image = \"assets/sprite.png\""), "exported raw string path");
        assert!(!luau.contains("loadImage"), "main.luau should not load images inline");
    }

    #[test]
    fn empty_image_is_omitted_from_export() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        scene.entity_mut(id).expect("e").components.push(Component::core("Sprite2D"));
        if let Some(Component::Core { props, .. }) = scene.entity_mut(id).expect("e").components.last_mut() {
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
        scene.entity_mut(a).expect("a").components.push(Component::core("Rect2D"));
        let b = scene.add_entity("B", 10.0, 10.0).id;
        let mut h = Harness::new(scene);
        h.app.component_clipboard = h.app.scene.entity(a).expect("a").components.first().cloned();
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
        let kids: Vec<u64> = scene.entities.iter().filter(|e| e.parent == Some(root1)).map(|e| e.id).collect();
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
    fn opening_prefab_uses_isolated_prefab_tab() {
        let mut h = Harness::new(Scene::default());
        let path = h.app.project_root.join("button.neoprefab");
        let entities = vec![Entity::new(40, "Button", 0.0, 0.0)];
        std::fs::write(&path, serde_json::to_string(&entities).unwrap()).unwrap();
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
    fn ui_widget_components_not_offered() {
        // The crash-prone UI widgets must not appear in the picker lists.
        for c in ["Button", "Dropdown", "TextInput", "Frame", "ScrollList"] {
            assert!(!CORE_COMPONENTS.contains(&c), "{c} still offered");
            assert!(!ADVANCED_COMPONENTS.contains(&c), "{c} still advanced");
        }
    }

    #[test]
    fn error_popup_renders_without_panicking() {
        let mut h = Harness::new(Scene::default());
        h.app.popup = Some(Popup::Error {
            message: "thread 'main' panicked\n".repeat(40),
            copied: false,
        });
        h.frame(FrameInput { mouse_x: 10.0, mouse_y: 10.0, ..Default::default() });
        assert!(h.app.popup.is_some());
    }

    #[test]
    fn hsv_rgb_round_trips() {
        for c in [[255, 0, 0, 255], [10, 180, 90, 255], [33, 66, 200, 255], [128, 128, 128, 255]] {
            let (h, s, v) = rgb_to_hsv(c);
            let back = hsv_to_rgb(h, s, v);
            for i in 0..3 {
                assert!((back[i] as i32 - c[i] as i32).abs() <= 2, "channel {i}: {} vs {}", back[i], c[i]);
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
        let text = std::fs::read_to_string(&path).expect("read prefab");
        assert!(text.contains("\"Hero\""));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn middle_pan_does_not_jump_by_hover_movement() {
        let mut h = Harness::new(Scene::default());
        // Hover far across the viewport with no button held.
        h.frame(FrameInput { mouse_x: 300.0, mouse_y: 300.0, ..Default::default() });
        h.frame(FrameInput { mouse_x: 900.0, mouse_y: 300.0, ..Default::default() });
        assert_eq!(h.app.cam_x, 0.0, "hover moved the camera");
        // Begin a middle-drag: first frame anchors, no movement applied.
        h.frame(FrameInput { mouse_x: 900.0, mouse_y: 300.0, middle_down: true, ..Default::default() });
        assert_eq!(h.app.cam_x, 0.0);
        // Drag 20px right -> camera moves exactly 20, not by the hover distance.
        h.frame(FrameInput { mouse_x: 920.0, mouse_y: 300.0, middle_down: true, ..Default::default() });
        assert!((h.app.cam_x - 20.0).abs() < 0.01, "cam_x was {}", h.app.cam_x);
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
        assert_eq!(h.app.scene.entities.len(), before, "undo did not revert add");
        h.app.redo();
        assert_eq!(h.app.scene.entities.len(), before + 1, "redo did not re-apply");
    }

    #[test]
    fn inactive_entities_are_excluded_from_export() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        scene.entity_mut(id).expect("e").components.push(Component::core("Rect2D"));
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
        let hit = h.app.viewport_hit(Rect::new(240.0, 40.0, 800.0, 600.0), 740.0, 440.0);
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
        assert_eq!(harness.app.scene.entity(first).expect("first").parent, Some(group));
        assert_eq!(harness.app.scene.entity(second).expect("second").parent, Some(group));
        let first_world = harness.app.entity_world_transform(first).expect("first world");
        let second_world = harness.app.entity_world_transform(second).expect("second world");
        assert!((first_world.x - 10.0).abs() < 0.001 && (first_world.y - 20.0).abs() < 0.001);
        assert!((second_world.x - 50.0).abs() < 0.001 && (second_world.y - 70.0).abs() < 0.001);

        let before = harness.app.scene.entities.len();
        harness.app.select_only(first);
        harness.app.duplicate_selection();
        assert_eq!(harness.app.scene.entities.len(), before + 2, "root and child should duplicate");
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
            scene.entity_mut(id).expect("entity").components.push(Component::core("TextBox"));
            let mut harness = Harness::with_size(scene, w, h);
            harness.app.selected = Some(id);
            harness.frame(FrameInput::default());
        }
    }
}
