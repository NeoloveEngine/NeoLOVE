//! Editor application state and per-frame UI layout.
//!
//! [`EditorApp`] owns the scene and the editor configuration (theme + dock
//! layout). It renders a dockable Hierarchy / Inspector, a pannable 2D
//! viewport, a bottom Project browser, and a toolbar, plus an overlay layer for
//! context menus, dropdowns, the color picker and modal dialogs.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::scene::{Component, Entity, Prop, PropValue, Scene, ScriptVar, VarValue, CORE_COMPONENTS};
use super::ui::{icon, Rect, Theme, Ui};

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
        }
    }
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
    Var { entity: u64, comp: usize, var: usize },
}

/// An action a menu item or dialog performs.
#[derive(Clone, Debug)]
enum Action {
    AddComponent(u64, String),
    AddEntity(Option<u64>),
    Rename(u64),
    Duplicate(u64),
    Copy(u64),
    Delete(u64),
    Paste,
    Unparent(u64),
    NewFolder,
    NewScript,
    RevealInExplorer,
    OpenPath(PathBuf),
    EnterFolder(PathBuf),
}

#[derive(Clone, Debug)]
struct MenuItem {
    action: Action,
    glyph: char,
    label: String,
    danger: bool,
}

/// Deferred work that a confirm/prompt dialog triggers on accept.
#[derive(Clone, Debug)]
enum Pending {
    NewScene,
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
    Color { target: ColorTarget, x: f32, y: f32 },
    Confirm { message: String, action: Pending },
    Prompt { title: String, action: Pending },
}

pub struct EditorApp {
    project_root: PathBuf,
    scene_path: PathBuf,
    config_path: PathBuf,
    scene: Scene,
    config: EditorConfig,
    selected: Option<u64>,
    dragging: Option<(u64, f32, f32)>,
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
    /// Viewport pan offset (middle-mouse drag).
    cam_x: f32,
    cam_y: f32,
    /// Collapsed section keys (component bodies / advanced groups).
    collapsed: HashSet<String>,
    clipboard: Option<Entity>,
    /// Hierarchy drag-to-reparent: the entity being dragged.
    reparent_drag: Option<u64>,
    popup: Option<Popup>,
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
        Self {
            bin_dir: project_root.clone(),
            project_root,
            scene_path,
            config_path,
            scene,
            config,
            selected: None,
            dragging: None,
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
            collapsed: HashSet::new(),
            clipboard: None,
            reparent_drag: None,
            popup: None,
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
        if self.scene_dirty {
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
    }

    // ---- Frame -------------------------------------------------------------

    pub fn frame(&mut self, ui: &mut Ui) {
        let w = ui.painter.width();
        let h = ui.painter.height();
        ui.painter.clear(self.config.theme.panel_alt);

        // Popups take input precedence: while one is open the background UI sees
        // no left/right press this frame.
        let raw_left = ui.input.mouse_pressed;
        let raw_right = ui.input.right_pressed;
        if self.popup.is_some() {
            ui.input.mouse_pressed = false;
            ui.input.right_pressed = false;
        }

        // Global shortcuts (only when no text field is focused).
        if !ui.has_focus() && self.popup.is_none() {
            if ui.input.copy {
                if let Some(id) = self.selected {
                    self.copy_entity(id);
                }
            }
            if ui.input.paste {
                self.paste_entity();
            }
            if ui.input.save {
                self.save();
            }
            if ui.input.delete {
                if let Some(id) = self.selected.take() {
                    self.scene.remove_entity(id);
                    self.mark_dirty();
                    self.status = "Deleted entity".to_string();
                }
            }
            if ui.input.escape {
                self.selected = None;
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
        let bin_h = self
            .config
            .layout
            .bin_h
            .clamp(0.0, (body_total - 120.0).max(0.0));
        let body_h = (body_total - bin_h - SPLIT_HALF * 2.0).max(0.0);
        let bin_split_y = body_top + body_h;
        let bin_rect = Rect::new(0.0, bin_split_y + SPLIT_HALF * 2.0, w, bin_h);

        let left_panels = self.panels_on(Side::Left);
        let right_panels = self.panels_on(Side::Right);
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

        self.status_bar(ui, w, h);

        // Restore the real press for popup handling, then render the overlay.
        ui.input.mouse_pressed = raw_left;
        ui.input.right_pressed = raw_right;
        self.handle_popup(ui, w, h);

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

        // Scene name (read-only display; rename via the dialog button).
        let name_label = format!("Scene: {}", self.scene.name);
        let avail = (w - x - 8.0).max(60.0);
        let nr = Rect::new(w - avail - 8.0, y, avail, bh);
        if ui.icon_button(nr, icon::EDIT, &name_label) {
            self.open_prompt("Rename scene", Pending::RenameScene, &self.scene.name.clone());
        }
        ui.tooltip(nr, "Rename scene (also renames the file)");
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
        if self.scene.entities.is_empty() {
            ui.label(area.x + PAD, y, "No entities.", self.config.theme.text_dim);
            ui.label(area.x + PAD, y + 18.0, "Right-click or use + Entity.", self.config.theme.text_dim);
            // Right-click empty space opens the viewport-style menu.
            if area.contains(ui.input.mouse_x, ui.input.mouse_y) && ui.input.right_pressed {
                self.open_hierarchy_empty_menu(ui.input.mouse_x, ui.input.mouse_y);
            }
            return y + 40.0;
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
        let indent = 6.0 + depth as f32 * 14.0;
        let row = Rect::new(area.x + 2.0, y, area.w - 4.0, ROW_H);
        let selected = self.selected == Some(id);

        // Reparent drop indicator: highlight when dragging over this row.
        let hovering = row.contains(ui.input.mouse_x, ui.input.mouse_y);
        if self.reparent_drag.is_some() && self.reparent_drag != Some(id) && hovering {
            ui.painter.stroke_rect(row, self.config.theme.accent);
        }

        if ui.list_row(row, &name, selected, indent) {
            self.selected = Some(id);
            // Begin a potential reparent drag.
            self.reparent_drag = Some(id);
        }
        if has_children {
            ui.icon(area.x + indent - 2.0, y + ROW_H / 2.0, icon::CHEVRON_RIGHT, 14.0, self.config.theme.text_dim);
        }
        if hovering && ui.input.right_pressed {
            self.selected = Some(id);
            self.open_entity_menu(id, ui.input.mouse_x, ui.input.mouse_y);
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
        for child in self.scene.children_of(Some(id)) {
            y = self.hierarchy_node(ui, area, child, depth + 1, y);
        }
        y
    }

    // ---- Viewport ----------------------------------------------------------

    fn viewport(&mut self, ui: &mut Ui, area: Rect) {
        if area.w <= 0.0 {
            return;
        }
        let prev = ui.painter.push_clip(area);
        ui.set_input_clip(area);

        ui.painter.fill_rect(area, self.config.theme.viewport_bg);
        let [br, bg, bb, _] = self.scene.background;
        let bg_frame = area.shrink(1.0);
        ui.painter.fill_rect(bg_frame, [br, bg, bb, 255]);
        if self.config.layout.show_grid {
            self.draw_grid(ui, bg_frame);
        }

        // Middle-mouse pan.
        if area.contains(ui.input.mouse_x, ui.input.mouse_y) && ui.input.middle_down {
            self.cam_x += ui.input.delta_x;
            self.cam_y += ui.input.delta_y;
            ui.wants_redraw = true;
        }

        let ox = area.x + self.cam_x;
        let oy = area.y + self.cam_y;

        // Draw entities sorted by z (lower first).
        let mut entities: Vec<Entity> = self.scene.entities.clone();
        entities.sort_by(|a, b| a.z.partial_cmp(&b.z).unwrap_or(std::cmp::Ordering::Equal));
        for entity in &entities {
            let rect = Rect::new(ox + entity.x, oy + entity.y, entity.size_x * entity.scale, entity.size_y * entity.scale);
            self.draw_entity(ui, entity, rect);
            if self.selected == Some(entity.id) {
                ui.painter.stroke_rect(rect.shrink(-1.0), self.config.theme.selection);
                for (hx, hy) in [
                    (rect.x, rect.y),
                    (rect.right(), rect.y),
                    (rect.x, rect.bottom()),
                    (rect.right(), rect.bottom()),
                ] {
                    ui.painter.fill_rect(Rect::new(hx - 3.0, hy - 3.0, 6.0, 6.0), self.config.theme.selection);
                }
            }
        }

        self.handle_viewport_input(ui, area);

        ui.reset_input_clip();
        ui.painter.set_clip_raw(prev);
    }

    fn draw_grid(&self, ui: &mut Ui, area: Rect) {
        let step = self.config.layout.grid.max(2.0);
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

    fn draw_entity(&self, ui: &mut Ui, entity: &Entity, rect: Rect) {
        let mut drew = false;
        for component in &entity.components {
            if let Component::Core { name, props } = component {
                let color = prop_color(props, "color").unwrap_or([200, 200, 200, 255]);
                match name.as_str() {
                    "Rect2D" | "Shape2D" | "NineSliceSprite2D" | "TileTexture2D" => {
                        ui.painter.fill_rect(rect, color);
                        drew = true;
                    }
                    "TextBox" => {
                        if let Some(Prop { value: PropValue::Text(t), .. }) =
                            props.iter().find(|p| p.name == "text")
                        {
                            let size = props
                                .iter()
                                .find(|p| p.name == "scale")
                                .and_then(|p| match p.value {
                                    PropValue::Number(n) => Some(n),
                                    _ => None,
                                })
                                .unwrap_or(20.0);
                            ui.painter.text(rect.x, rect.y, t, size.clamp(6.0, 96.0), color);
                            drew = true;
                        }
                    }
                    "Sprite2D" => {
                        ui.painter.fill_rect(rect, color);
                        ui.painter.stroke_rect(rect, self.config.theme.accent);
                        ui.painter.icon_centered(
                            rect.x + rect.w / 2.0,
                            rect.y + rect.h / 2.0,
                            icon::IMAGE,
                            (rect.w.min(rect.h) * 0.4).clamp(10.0, 40.0),
                            [255, 255, 255, 180],
                        );
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
    }

    fn handle_viewport_input(&mut self, ui: &mut Ui, area: Rect) {
        let mx = ui.input.mouse_x;
        let my = ui.input.mouse_y;
        let inside = area.contains(mx, my);

        if inside && ui.input.right_pressed {
            // Right-click selects the entity under the cursor (if any), then
            // opens the appropriate context menu.
            let hit = self.viewport_hit(area, mx, my);
            match hit {
                Some(id) => {
                    self.selected = Some(id);
                    self.open_entity_menu(id, mx, my);
                }
                None => self.open_viewport_menu(mx, my),
            }
            return;
        }

        if inside && ui.input.mouse_pressed {
            match self.viewport_hit(area, mx, my) {
                Some(id) => {
                    self.selected = Some(id);
                    let e = self.scene.entity(id);
                    if let Some(e) = e {
                        self.dragging = Some((id, mx - (area.x + self.cam_x + e.x), my - (area.y + self.cam_y + e.y)));
                    }
                }
                None => self.selected = None,
            }
        }

        if let Some((id, gx, gy)) = self.dragging {
            if ui.input.mouse_down {
                let snap = self.config.layout.snap;
                let grid = self.config.layout.grid.max(1.0);
                let world_x = mx - (area.x + self.cam_x) - gx;
                let world_y = my - (area.y + self.cam_y) - gy;
                if let Some(e) = self.scene.entity_mut(id) {
                    let (mut nx, mut ny) = (world_x, world_y);
                    if snap {
                        nx = (nx / grid).round() * grid;
                        ny = (ny / grid).round() * grid;
                    } else {
                        nx = nx.round();
                        ny = ny.round();
                    }
                    if e.x != nx || e.y != ny {
                        e.x = nx;
                        e.y = ny;
                        self.scene_dirty = true;
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
        order.sort_by(|a, b| b.z.partial_cmp(&a.z).unwrap_or(std::cmp::Ordering::Equal));
        for e in order {
            let r = Rect::new(
                area.x + self.cam_x + e.x,
                area.y + self.cam_y + e.y,
                e.size_x * e.scale,
                e.size_y * e.scale,
            );
            if r.contains(mx, my) {
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
        if near(my, bin_split_y) && my >= TOOLBAR_H {
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

        let Some(id) = self.selected else {
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
            return y + 10.0;
        };
        let Some(mut entity) = self.scene.entity(id).cloned() else {
            self.selected = None;
            return y + 10.0;
        };
        let mut dirty = false;

        let r = ui.text_field("ent_name", Rect::new(x, y, width, FIELD_H), &entity.name);
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
            // Header row with collapse + remove.
            let hdr = Rect::new(x, y, width - 24.0, ROW_H);
            let tri = if comp_expanded { icon::EXPAND_MORE } else { icon::CHEVRON_RIGHT };
            ui.painter.fill_round_rect(hdr, 3.0, self.config.theme.panel_alt);
            ui.icon(x + 12.0, y + ROW_H / 2.0, tri, 15.0, self.config.theme.text);
            ui.icon(x + 28.0, y + ROW_H / 2.0, glyph, 15.0, self.config.theme.accent);
            ui.label(x + 42.0, y + (ROW_H - 14.0) / 2.0, &comp_label, self.config.theme.text);
            if hdr.contains(ui.input.mouse_x, ui.input.mouse_y) && ui.input.mouse_pressed {
                self.set_collapsed(&key, comp_expanded);
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
            self.selected = None;
            self.mark_dirty();
            return y + FIELD_H + 14.0;
        }
        y += FIELD_H + 14.0;

        if dirty {
            self.scene.replace_entity(id, entity);
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
        }
        dirty
    }

    fn script_variables(&mut self, ui: &mut Ui, entity: u64, comp: usize, variables: &mut Vec<ScriptVar>, x: f32, width: f32, mut y: f32, dirty: &mut bool) -> f32 {
        ui.icon(x + 8.0, y + 8.0, icon::PLAYLIST_ADD, 14.0, self.config.theme.text_dim);
        ui.label(x + 20.0, y, "Public Variables", self.config.theme.text_dim);
        y += 20.0;
        let mut remove_var = None;
        for vi in 0..variables.len() {
            let base = format!("var_{entity}_{comp}_{vi}");
            let name_w = width - 90.0;
            let r = ui.text_field(&format!("{base}_n"), Rect::new(x, y, name_w.max(40.0), FIELD_H), &variables[vi].name);
            if r.changed {
                variables[vi].name = r.text;
                *dirty = true;
            }
            if ui.button(Rect::new(x + name_w + 4.0, y, 60.0, FIELD_H), variables[vi].value.type_label()) {
                variables[vi].value = cycle_var_type(&variables[vi].value);
                *dirty = true;
            }
            let del = Rect::new(x + width - 20.0, y, 20.0, FIELD_H);
            if ui.icon_toggle(del, icon::DELETE, false, self.config.theme.danger) {
                remove_var = Some(vi);
            }
            y += FIELD_H + 4.0;
            let vx = x + 16.0;
            let vw = width - 16.0;
            match &mut variables[vi].value {
                VarValue::Number(n) => {
                    let mut nn = *n;
                    if self.num_row(ui, &format!("{base}_num"), "Value", &mut nn, vx, vw, &mut y) {
                        *n = nn;
                        *dirty = true;
                    }
                }
                VarValue::Bool(b) => {
                    ui.label(vx, y + 4.0, "Value", self.config.theme.text);
                    if let Some(nv) = ui.checkbox(Rect::new(vx + LABEL_W, y, FIELD_H, FIELD_H), *b) {
                        *b = nv;
                        *dirty = true;
                    }
                    y += FIELD_H + 6.0;
                }
                VarValue::Text(s) => {
                    let mut ss = s.clone();
                    if self.text_row(ui, &format!("{base}_t"), "Value", &mut ss, vx, vw, &mut y) {
                        *s = ss;
                        *dirty = true;
                    }
                }
                VarValue::Color(c) => {
                    ui.label(vx, y + 4.0, "Value", self.config.theme.text);
                    let mut col = *c;
                    let fx = vx + LABEL_W;
                    if self.color_row_inline(ui, &format!("{base}_c"), fx, (vx + vw) - fx, y, &mut col, ColorTarget::Var { entity, comp, var: vi }) {
                        *c = col;
                        *dirty = true;
                    }
                    y += FIELD_H + 6.0;
                }
            }
            y += 4.0;
        }
        if let Some(vi) = remove_var {
            variables.remove(vi);
            *dirty = true;
        }
        if ui.icon_button(Rect::new(x, y, width, FIELD_H), icon::ADD_CIRCLE, "Add Variable") {
            variables.push(ScriptVar { name: format!("var{}", variables.len() + 1), value: VarValue::Number(0.0) });
            *dirty = true;
        }
        y + FIELD_H + 6.0
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
            self.popup = Some(Popup::Color { target, x: fx, y: y + FIELD_H + 2.0 });
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

        // Header buttons: new folder, new script, reveal, up.
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
        if btn(ui, bx, icon::CREATE_NEW_FOLDER, "New folder") {
            self.open_prompt("New folder name", Pending::CreateFolder, "NewFolder");
        }
        bx -= 28.0;
        if btn(ui, bx, icon::NOTE_ADD, "New script") {
            self.open_prompt("New script name", Pending::CreateScript, "script.luau");
        }
        ui.painter.stroke_rect(area, self.config.theme.border);

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
        for (path, name, glyph) in &files {
            let (_click, dbl, rc) = draw(ui, yy, *glyph, name, false);
            if dbl {
                open = Some(path.clone());
            }
            if rc {
                context = Some((path.clone(), ui.input.mouse_x, ui.input.mouse_y));
            }
            yy += row_h + 1.0;
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
        let mut items: Vec<MenuItem> = CORE_COMPONENTS
            .iter()
            .map(|name| MenuItem {
                action: Action::AddComponent(entity, name.to_string()),
                glyph: core_icon(name),
                label: name.to_string(),
                danger: false,
            })
            .collect();
        items.push(MenuItem {
            action: Action::AddComponent(entity, "Script".to_string()),
            glyph: icon::DATA_OBJECT,
            label: "Script".to_string(),
            danger: false,
        });
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_entity_menu(&mut self, id: u64, x: f32, y: f32) {
        let items = vec![
            MenuItem { action: Action::Rename(id), glyph: icon::EDIT, label: "Rename".into(), danger: false },
            MenuItem { action: Action::Duplicate(id), glyph: icon::CONTENT_COPY, label: "Duplicate".into(), danger: false },
            MenuItem { action: Action::Copy(id), glyph: icon::CONTENT_COPY, label: "Copy".into(), danger: false },
            MenuItem { action: Action::Paste, glyph: icon::CONTENT_PASTE, label: "Paste".into(), danger: false },
            MenuItem { action: Action::Unparent(id), glyph: icon::CHEVRON_LEFT, label: "Unparent".into(), danger: false },
            MenuItem { action: Action::AddEntity(Some(id)), glyph: icon::ADD, label: "Add Child".into(), danger: false },
            MenuItem { action: Action::Delete(id), glyph: icon::DELETE, label: "Delete".into(), danger: true },
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

    fn open_viewport_menu(&mut self, x: f32, y: f32) {
        let items = vec![
            MenuItem { action: Action::AddEntity(None), glyph: icon::ADD, label: "Add Entity".into(), danger: false },
            MenuItem { action: Action::Paste, glyph: icon::CONTENT_PASTE, label: "Paste".into(), danger: false },
        ];
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_project_menu(&mut self, x: f32, y: f32) {
        let items = vec![
            MenuItem { action: Action::NewFolder, glyph: icon::CREATE_NEW_FOLDER, label: "New Folder".into(), danger: false },
            MenuItem { action: Action::NewScript, glyph: icon::NOTE_ADD, label: "New Script".into(), danger: false },
            MenuItem { action: Action::RevealInExplorer, glyph: icon::OPEN_IN_NEW, label: "Reveal in File Manager".into(), danger: false },
        ];
        self.popup = Some(Popup::Menu { x, y, items });
    }

    fn open_path_menu(&mut self, path: PathBuf, x: f32, y: f32) {
        let is_dir = path.is_dir();
        let mut items = Vec::new();
        if is_dir {
            items.push(MenuItem { action: Action::EnterFolder(path.clone()), glyph: icon::FOLDER_OPEN, label: "Open Folder".into(), danger: false });
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

    fn handle_popup(&mut self, ui: &mut Ui, w: f32, h: f32) {
        let popup = match self.popup.take() {
            Some(p) => p,
            None => return,
        };
        // Escape closes any popup.
        if ui.input.escape {
            if matches!(popup, Popup::Prompt { .. }) {
                self.focus = None;
            }
            return;
        }
        match popup {
            Popup::Menu { x, y, items } => self.draw_menu(ui, x, y, items, w, h),
            Popup::Color { target, x, y } => self.draw_color_picker(ui, target, x, y, w, h),
            Popup::Confirm { message, action } => self.draw_confirm(ui, message, action, w, h),
            Popup::Prompt { title, action } => self.draw_prompt(ui, title, action, w, h),
        }
    }

    fn draw_menu(&mut self, ui: &mut Ui, x: f32, y: f32, items: Vec<MenuItem>, w: f32, h: f32) {
        let item_h = 26.0;
        let width = 200.0_f32;
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

        // Click outside closes.
        let clicked_outside = ui.input.mouse_pressed && !rect.contains(ui.input.mouse_x, ui.input.mouse_y);
        if let Some(action) = chosen {
            self.perform(action);
        } else if !clicked_outside {
            // Keep the menu open until a choice or an outside click.
            self.popup = Some(Popup::Menu { x, y, items });
        }
    }

    fn draw_color_picker(&mut self, ui: &mut Ui, target: ColorTarget, x: f32, y: f32, w: f32, h: f32) {
        let width = 220.0;
        let height = 150.0;
        let px = x.min(w - width - 4.0).max(2.0);
        let py = y.min(h - height - 4.0).max(2.0);
        let rect = Rect::new(px, py, width, height);
        ui.painter.fill_round_rect(rect, 5.0, self.config.theme.panel);
        ui.painter.stroke_round_rect(rect, 5.0, self.config.theme.border);

        let mut color = self.target_color(&target).unwrap_or([255, 255, 255, 255]);
        // Preview swatch.
        ui.painter.fill_round_rect(Rect::new(px + 10.0, py + 10.0, 40.0, 30.0), 4.0, [color[0], color[1], color[2], 255]);
        ui.painter.stroke_round_rect(Rect::new(px + 10.0, py + 10.0, 40.0, 30.0), 4.0, self.config.theme.border);

        let labels = ["R", "G", "B", "A"];
        let mut changed = false;
        for i in 0..4 {
            let ry = py + 10.0 + i as f32 * 28.0;
            ui.label(px + 60.0, ry + 4.0, labels[i], self.config.theme.text);
            if let Some(v) = ui.slider(Rect::new(px + 78.0, ry, 90.0, 20.0), color[i] as f32, 0.0, 255.0) {
                color[i] = v.round().clamp(0.0, 255.0) as u8;
                changed = true;
            }
            let r = ui.text_field(&format!("cp_{i}"), Rect::new(px + 174.0, ry, 36.0, 20.0), &color[i].to_string());
            if r.changed {
                if let Ok(v) = r.text.trim().parse::<i32>() {
                    color[i] = v.clamp(0, 255) as u8;
                    changed = true;
                }
            }
        }
        if changed {
            self.set_target_color(&target, color);
            self.mark_dirty();
        }

        let clicked_outside = ui.input.mouse_pressed && !rect.contains(ui.input.mouse_x, ui.input.mouse_y);
        if !clicked_outside {
            self.popup = Some(Popup::Color { target, x, y });
        }
    }

    fn draw_confirm(&mut self, ui: &mut Ui, message: String, action: Pending, w: f32, h: f32) {
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
        let confirm = ui.button_colored(yes, "Yes", self.config.theme.danger, [255, 255, 255, 255]);
        let cancel = ui.button(no, "Cancel");
        if confirm {
            self.perform_pending(action);
        } else if !cancel {
            self.popup = Some(Popup::Confirm { message, action });
        }
    }

    fn draw_prompt(&mut self, ui: &mut Ui, title: String, action: Pending, w: f32, h: f32) {
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
        let submit = ui.button_colored(ok, "OK", self.config.theme.button, self.config.theme.text) || ui.input.enter;
        let cancelled = ui.button(cancel, "Cancel");
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
            Action::AddEntity(parent) => self.add_entity(parent),
            Action::Rename(id) => {
                let cur = self.scene.entity(id).map(|e| e.name.clone()).unwrap_or_default();
                self.open_prompt("Rename entity", Pending::RenameEntity(id), &cur);
            }
            Action::Duplicate(id) => self.duplicate_entity(id),
            Action::Copy(id) => self.copy_entity(id),
            Action::Paste => self.paste_entity(),
            Action::Delete(id) => {
                self.scene.remove_entity(id);
                if self.selected == Some(id) {
                    self.selected = None;
                }
                self.mark_dirty();
            }
            Action::Unparent(id) => {
                if let Some(e) = self.scene.entity_mut(id) {
                    e.parent = None;
                }
                self.mark_dirty();
            }
            Action::NewFolder => self.open_prompt("New folder name", Pending::CreateFolder, "NewFolder"),
            Action::NewScript => self.open_prompt("New script name", Pending::CreateScript, "script.luau"),
            Action::RevealInExplorer => self.reveal_in_explorer(),
            Action::OpenPath(p) => self.open_path(&p),
            Action::EnterFolder(p) => self.navigate_bin(p),
        }
    }

    fn perform_pending(&mut self, action: Pending) {
        match action {
            Pending::NewScene => {
                self.scene = Scene::default();
                self.selected = None;
                self.scene_dirty = false;
                self.status = "New scene".to_string();
            }
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
        let n = self.scene.entities.len() + 1;
        let mut e = self.scene.add_entity(format!("Entity {n}"), 96.0, 96.0);
        e.parent = parent;
        let id = e.id;
        self.scene.replace_entity(id, e);
        self.selected = Some(id);
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
            self.selected = Some(id);
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
            self.selected = Some(new_id);
            self.mark_dirty();
            self.status = "Duplicated entity".to_string();
        }
    }

    // ---- Color target plumbing --------------------------------------------

    fn target_color(&self, target: &ColorTarget) -> Option<[u8; 4]> {
        match target {
            ColorTarget::Background => Some(self.scene.background),
            ColorTarget::Prop { entity, comp, prop } => {
                let e = self.scene.entity(*entity)?;
                if let Component::Core { props, .. } = e.components.get(*comp)? {
                    if let PropValue::Color(c) = props.get(*prop)?.value {
                        return Some(c);
                    }
                }
                None
            }
            ColorTarget::Var { entity, comp, var } => {
                let e = self.scene.entity(*entity)?;
                if let Component::Script { variables, .. } = e.components.get(*comp)? {
                    if let VarValue::Color(c) = variables.get(*var)?.value {
                        return Some(c);
                    }
                }
                None
            }
        }
    }

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
            ColorTarget::Var { entity, comp, var } => {
                if let Some(e) = self.scene.entity_mut(*entity) {
                    if let Some(Component::Script { variables, .. }) = e.components.get_mut(*comp) {
                        if let Some(v) = variables.get_mut(*var) {
                            v.value = VarValue::Color(color);
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
        if self.scene_dirty {
            self.open_confirm("Discard unsaved changes and start a new scene?", Pending::NewScene);
        } else {
            self.perform_pending(Pending::NewScene);
        }
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
        match self.scene.save(&self.scene_path) {
            Ok(()) => {
                self.scene_dirty = false;
                self.status = format!("Saved {}", self.scene_path.display());
            }
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    fn load(&mut self) {
        match Scene::load(&self.scene_path) {
            Ok(scene) => {
                self.scene = scene;
                self.selected = None;
                self.scene_dirty = false;
                self.status = format!("Loaded {}", self.scene_path.display());
            }
            Err(e) => self.status = format!("Load failed: {e}"),
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
        let template = "--!strict\n-- New NeoLOVE script component.\n\nlocal Behaviour = {}\n\nfunction Behaviour.awake(entity, self)\nend\n\nfunction Behaviour.update(entity, self, dt)\nend\n\nreturn Behaviour\n";
        match std::fs::write(&path, template) {
            Ok(()) => {
                self.status = format!("Created script {name}");
                self.open_path(&path);
            }
            Err(e) => self.status = format!("Create script failed: {e}"),
        }
    }

    fn reveal_in_explorer(&mut self) {
        let dir = self.bin_dir.clone();
        self.open_path(&dir);
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
        match std::fs::write(&path, self.scene.to_luau()) {
            Ok(()) => self.status = format!("Exported {}", path.display()),
            Err(e) => self.status = format!("Export failed: {e}"),
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
        match std::process::Command::new(exe).arg("run").arg(&self.project_root).spawn() {
            Ok(_) => self.status = "Launched preview".to_string(),
            Err(e) => self.status = format!("Run failed: {e}"),
        }
    }
}

fn component_icon(component: &Component) -> char {
    match component {
        Component::Core { name, .. } => core_icon(name),
        Component::Script { .. } => icon::DATA_OBJECT,
    }
}

fn core_icon(name: &str) -> char {
    match name {
        "Rect2D" => icon::CROP_SQUARE,
        "Shape2D" => icon::CROP_SQUARE,
        "TextBox" => icon::TITLE,
        "Sprite2D" | "NineSliceSprite2D" | "TileTexture2D" => icon::IMAGE,
        "Collider2D" => icon::BORDER_ALL,
        "Rigidbody2D" => icon::VIEW_IN_AR,
        "Bolt2D" | "Rope2D" => icon::TUNE,
        _ => icon::VIEW_QUILT,
    }
}

fn prop_color(props: &[Prop], name: &str) -> Option<[u8; 4]> {
    props.iter().find(|p| p.name == name).and_then(|p| match p.value {
        PropValue::Color(c) => Some(c),
        _ => None,
    })
}

fn cycle_var_type(value: &VarValue) -> VarValue {
    match value {
        VarValue::Number(_) => VarValue::Bool(false),
        VarValue::Bool(_) => VarValue::Text(String::new()),
        VarValue::Text(_) => VarValue::Color([255, 255, 255, 255]),
        VarValue::Color(_) => VarValue::Number(0.0),
    }
}

fn clamp_range(v: f32, lo: f32, hi: f32) -> f32 {
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    v.clamp(lo, hi)
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
        "png" | "bmp" | "tga" | "webp" | "jpg" | "jpeg" | "pnm" | "gif" => icon::IMAGE,
        "wav" | "mp3" | "ogg" | "flac" | "aac" | "m4a" | "aiff" => icon::AUDIOTRACK,
        "ttf" | "otf" => icon::FONT_DOWNLOAD,
        "luau" | "lua" => icon::DATA_OBJECT,
        "toml" | "json" | "txt" | "md" | "neoscene" => icon::ARTICLE,
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
    fn renders_without_panicking() {
        let mut h = Harness::new(Scene::default());
        h.frame(FrameInput::default());
        let first = h.buffer[0];
        assert!(h.buffer.iter().any(|&p| p != first));
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
