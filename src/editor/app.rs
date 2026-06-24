//! Editor application state and per-frame UI layout.
//!
//! [`EditorApp`] owns the scene being edited and the editor configuration
//! (theme + dock layout). It renders the classic three-panel layout — a
//! hierarchy and an inspector that can be docked to either side and resized
//! with draggable splitters, plus a 2D viewport in the middle — with a toolbar
//! across the top. Panels scroll when their contents overflow, and the viewport
//! supports grid snapping.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::scene::{Component, Entity, Scene, ScriptVar, VarValue};
use super::ui::{icon, Rect, Theme, Ui};

const TOOLBAR_H: f32 = 40.0;
const STATUS_H: f32 = 24.0;
const HEADER_H: f32 = 26.0;
const ROW_H: f32 = 24.0;
const FIELD_H: f32 = 22.0;
const PAD: f32 = 10.0;
const LABEL_W: f32 = 74.0;
const MIN_PANEL_W: f32 = 150.0;
const MIN_VIEWPORT_W: f32 = 160.0;
const SPLIT_HALF: f32 = 3.0;

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

/// The two dockable panels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Panel {
    Hierarchy,
    Inspector,
}

/// Which splitter the user is currently dragging.
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
    /// Vertical split ratio (top panel share) when two panels share the left.
    pub left_split: f32,
    pub right_split: f32,
    pub snap: bool,
    pub grid: f32,
    /// Whether the viewport grid overlay is drawn.
    pub show_grid: bool,
    /// Height of the bottom project bin, in pixels.
    pub bin_h: f32,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            left_w: 230.0,
            right_w: 320.0,
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

/// The editor's on-disk configuration: a theme plus the dock layout.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorConfig {
    pub theme: Theme,
    pub layout: Layout,
}

/// The editor's mutable runtime state.
pub struct EditorApp {
    project_root: PathBuf,
    scene_path: PathBuf,
    config_path: PathBuf,
    scene: Scene,
    config: EditorConfig,
    selected: Option<u64>,
    /// Active viewport drag: (entity id, grab offset x, grab offset y).
    dragging: Option<(u64, f32, f32)>,
    active_splitter: Option<Splitter>,
    hierarchy_scroll: f32,
    inspector_scroll: f32,
    hierarchy_content_h: f32,
    inspector_content_h: f32,
    /// Current directory shown in the bottom project bin.
    bin_dir: PathBuf,
    bin_scroll: f32,
    bin_content_h: f32,
    status: String,
    /// Set when the layout/config changes and should be written to disk.
    dirty: bool,
    focus: Option<String>,
    edit_buffer: String,
}

impl EditorApp {
    pub fn new(project_root: PathBuf, scene_path: PathBuf, scene: Scene, config: EditorConfig) -> Self {
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
            bin_scroll: 0.0,
            bin_content_h: 0.0,
            status: "Ready".to_string(),
            dirty: false,
            focus: None,
            edit_buffer: String::new(),
        }
    }

    pub fn title(&self) -> String {
        format!("NeoLOVE Editor — {}", self.scene.name)
    }

    pub fn theme(&self) -> Theme {
        self.config.theme.clone()
    }

    #[cfg(test)]
    fn selected_id(&self) -> Option<u64> {
        self.selected
    }

    #[cfg(test)]
    fn entity_count(&self) -> usize {
        self.scene.entities.len()
    }

    #[cfg(test)]
    fn snap_enabled(&self) -> bool {
        self.config.layout.snap
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

    /// Persist the editor config if it changed this frame.
    pub fn flush_config(&mut self) {
        if self.dirty {
            if let Err(error) = save_config(&self.config_path, &self.config) {
                eprintln!("warning: failed to save editor config: {error}");
            }
            self.dirty = false;
        }
    }

    /// Render and process one frame of the editor UI.
    pub fn frame(&mut self, ui: &mut Ui) {
        let w = ui.painter.width();
        let h = ui.painter.height();

        ui.painter.clear(self.config.theme.panel_alt);

        // Delete the selection with the Delete key when no field is focused.
        if !ui.has_focus() && ui.input.delete {
            if let Some(id) = self.selected.take() {
                self.scene.remove_entity(id);
                self.status = "Deleted entity".to_string();
            }
        }

        self.toolbar(ui, w);

        let body_top = TOOLBAR_H;
        let body_total = (h - TOOLBAR_H - STATUS_H).max(0.0);

        // Reserve a full-width project bin at the bottom, resizable via a
        // horizontal splitter. The rest of the body holds the docked columns
        // and the central viewport.
        let bin_h = self
            .config
            .layout
            .bin_h
            .clamp(0.0, (body_total - 120.0).max(0.0));
        let body_h = (body_total - bin_h - SPLIT_HALF * 2.0).max(0.0);
        let bin_split_y = body_top + body_h;
        let bin_rect = Rect::new(0.0, bin_split_y + SPLIT_HALF * 2.0, w, bin_h);

        // Resolve dock columns and the central viewport, clamped so the layout
        // stays usable at any window size.
        let left_panels = self.panels_on(Side::Left);
        let right_panels = self.panels_on(Side::Right);
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
        // Clamp each column and guarantee a minimum viewport width.
        let max_col = (w - MIN_VIEWPORT_W).max(0.0);
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

        self.handle_splitters(
            ui,
            left_col,
            right_col,
            &left_panels,
            &right_panels,
            w,
            bin_split_y,
            body_total,
        );

        self.status_bar(ui, w, h);

        // Tooltips render last so they sit above every panel.
        ui.draw_tooltip();
    }

    /// Panels currently docked to `side`, top-to-bottom.
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

    /// Lay out one dock column, splitting vertically when it holds two panels.
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
                // Horizontal splitter handle.
                let handle = Rect::new(col.x, col.y + top_h - SPLIT_HALF, col.w, SPLIT_HALF * 2.0);
                ui.painter.fill_rect(handle, self.config.theme.splitter);
            }
        }
    }

    fn render_panel(&mut self, ui: &mut Ui, area: Rect, panel: Panel) {
        ui.painter.fill_rect(area, self.config.theme.panel);
        // Header with icon, title and a dock-swap button.
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
        ui.icon(area.x + 16.0, area.y + HEADER_H / 2.0, glyph, 16.0, self.config.theme.text);
        ui.label(area.x + 30.0, area.y + (HEADER_H - 14.0) / 2.0, title, self.config.theme.text);

        let swap = Rect::new(area.right() - 26.0, area.y + 3.0, 20.0, HEADER_H - 6.0);
        ui.tooltip(swap, "Dock to other side");
        if ui.icon_toggle(swap, icon::SWAP, false, self.config.theme.text_dim) {
            match panel {
                Panel::Hierarchy => {
                    self.config.layout.hierarchy_side = side.toggled();
                }
                Panel::Inspector => {
                    self.config.layout.inspector_side = side.toggled();
                }
            }
            self.dirty = true;
        }

        ui.painter.stroke_rect(area, self.config.theme.border);

        let content = Rect::new(
            area.x,
            area.y + HEADER_H,
            area.w,
            (area.h - HEADER_H).max(0.0),
        );

        // Apply scroll wheel when hovering this panel's content.
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

        // Scrollbar indicator.
        if total > content.h {
            let track_h = content.h;
            let thumb_h = (track_h * (content.h / total)).max(20.0);
            let thumb_y = content.y + (scroll_value / (total - content.h)) * (track_h - thumb_h);
            ui.painter.fill_round_rect(
                Rect::new(content.right() - 6.0, thumb_y, 4.0, thumb_h),
                2.0,
                self.config.theme.text_dim,
            );
        }
    }

    fn hierarchy_content(&mut self, ui: &mut Ui, area: Rect, start_y: f32) -> f32 {
        let mut y = start_y;
        let ids: Vec<u64> = self.scene.entities.iter().map(|e| e.id).collect();
        if ids.is_empty() {
            ui.label(area.x + PAD, y, "No entities yet.", self.config.theme.text_dim);
            ui.label(
                area.x + PAD,
                y + 18.0,
                "Use the + button in the toolbar.",
                self.config.theme.text_dim,
            );
            return y + 40.0;
        }
        for id in ids {
            let name = self.scene.entity(id).map(|e| e.name.clone()).unwrap_or_default();
            let row = Rect::new(area.x + 2.0, y, area.w - 4.0, ROW_H);
            let selected = self.selected == Some(id);
            if ui.list_row(row, &name, selected, 4.0) {
                self.selected = Some(id);
            }
            y += ROW_H + 1.0;
        }
        y + PAD
    }

    fn toolbar(&mut self, ui: &mut Ui, w: f32) {
        ui.painter
            .fill_rect(Rect::new(0.0, 0.0, w, TOOLBAR_H), self.config.theme.toolbar);
        ui.painter
            .stroke_rect(Rect::new(0.0, 0.0, w, TOOLBAR_H), self.config.theme.border);

        let y = 6.0;
        let bh = TOOLBAR_H - 12.0;
        let bw = 30.0;
        let mut x = 8.0;
        // Draws an action button with a hover tooltip.
        let icon_btn = |ui: &mut Ui, glyph: char, tip: &str, x: &mut f32| -> bool {
            let rect = Rect::new(*x, y, bw, bh);
            let clicked = ui.icon_toggle(rect, glyph, false, ui.theme.text);
            ui.tooltip(rect, tip);
            *x += bw + 5.0;
            clicked
        };

        if icon_btn(ui, icon::NOTE_ADD, "New scene", &mut x) {
            self.scene = Scene::default();
            self.selected = None;
            self.status = "New scene".to_string();
        }
        if icon_btn(ui, icon::SAVE, "Save scene", &mut x) {
            self.save();
        }
        if icon_btn(ui, icon::FOLDER_OPEN, "Load scene", &mut x) {
            self.load();
        }
        if icon_btn(ui, icon::CODE, "Export main.luau", &mut x) {
            self.export_luau();
        }
        if icon_btn(ui, icon::PLAY, "Run scene preview", &mut x) {
            self.run_scene();
        }

        x += 8.0; // separator
        if icon_btn(ui, icon::ADD_CIRCLE, "Add entity", &mut x) {
            self.add_entity();
        }

        x += 8.0; // separator
        // Snap-to-grid toggle.
        let snap = self.config.layout.snap;
        let snap_glyph = if snap { icon::GRID_ON } else { icon::GRID_OFF };
        let snap_rect = Rect::new(x, y, bw, bh);
        if ui.icon_toggle(snap_rect, snap_glyph, snap, self.config.theme.text) {
            self.config.layout.snap = !snap;
            self.dirty = true;
            self.status = if self.config.layout.snap {
                "Grid snapping on".to_string()
            } else {
                "Grid snapping off".to_string()
            };
        }
        ui.tooltip(snap_rect, "Snap to grid");
        x += bw + 5.0;

        // Grid-visibility toggle.
        let show_grid = self.config.layout.show_grid;
        let grid_rect = Rect::new(x, y, bw, bh);
        if ui.icon_toggle(grid_rect, icon::BORDER_ALL, show_grid, self.config.theme.text) {
            self.config.layout.show_grid = !show_grid;
            self.dirty = true;
            self.status = if self.config.layout.show_grid {
                "Grid shown".to_string()
            } else {
                "Grid hidden".to_string()
            };
        }
        ui.tooltip(grid_rect, "Show grid");
        x += bw + 5.0;

        let grid_field = Rect::new(x, y, 48.0, bh);
        let grid_str = format_num(self.config.layout.grid);
        let r = ui.text_field("grid_size", grid_field, &grid_str);
        if r.changed {
            if let Ok(v) = r.text.trim().parse::<f32>() {
                self.config.layout.grid = v.clamp(1.0, 512.0);
                self.dirty = true;
            }
        }
        ui.tooltip(grid_field, "Grid cell size (px)");
        x += 48.0 + 5.0;

        // Scene name field on the right.
        let field_w = (w * 0.22).clamp(120.0, 260.0);
        let fx = (w - field_w - 8.0).max(x + 8.0);
        let name_rect = Rect::new(fx, y, field_w, bh);
        let resp = ui.text_field("scene_name", name_rect, &self.scene.name);
        if resp.changed && !resp.text.is_empty() {
            self.scene.name = resp.text;
        }
        ui.tooltip(name_rect, "Scene name");
    }

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

        let ox = area.x;
        let oy = area.y;
        let entities: Vec<Entity> = self.scene.entities.clone();
        for entity in &entities {
            let rect = Rect::new(ox + entity.x, oy + entity.y, entity.size_x, entity.size_y);
            self.draw_entity(ui, entity, rect);
            if self.selected == Some(entity.id) {
                ui.painter.stroke_rect(rect.shrink(-1.0), self.config.theme.selection);
                for (hx, hy) in [
                    (rect.x, rect.y),
                    (rect.right(), rect.y),
                    (rect.x, rect.bottom()),
                    (rect.right(), rect.bottom()),
                ] {
                    ui.painter.fill_rect(
                        Rect::new(hx - 3.0, hy - 3.0, 6.0, 6.0),
                        self.config.theme.selection,
                    );
                }
            }
        }

        self.handle_viewport_drag(ui, area);

        ui.reset_input_clip();
        ui.painter.set_clip_raw(prev);
    }

    fn draw_grid(&self, ui: &mut Ui, area: Rect) {
        let step = self.config.layout.grid.max(2.0);
        let line = self.config.theme.grid;
        let mut x = area.x;
        while x < area.right() {
            ui.painter.fill_rect(Rect::new(x, area.y, 1.0, area.h), line);
            x += step;
        }
        let mut y = area.y;
        while y < area.bottom() {
            ui.painter.fill_rect(Rect::new(area.x, y, area.w, 1.0), line);
            y += step;
        }
    }

    fn draw_entity(&self, ui: &mut Ui, entity: &Entity, rect: Rect) {
        let mut drew = false;
        for component in &entity.components {
            match component {
                Component::Rect2D { color } => {
                    ui.painter.fill_rect(rect, *color);
                    drew = true;
                }
                Component::Text2D { text, color, size } => {
                    ui.painter.text(rect.x, rect.y, text, size.max(6.0), *color);
                    drew = true;
                }
                Component::Sprite { tint, .. } => {
                    ui.painter.fill_rect(rect, *tint);
                    ui.painter.stroke_rect(rect, self.config.theme.accent);
                    ui.painter.icon_centered(
                        rect.x + rect.w / 2.0,
                        rect.y + rect.h / 2.0,
                        icon::IMAGE,
                        (rect.w.min(rect.h) * 0.5).clamp(12.0, 48.0),
                        [255, 255, 255, 180],
                    );
                    drew = true;
                }
                Component::Script { .. } => {}
            }
        }
        if !drew {
            ui.painter.stroke_rect(rect, self.config.theme.text_dim);
            ui.painter
                .text(rect.x + 4.0, rect.y + 4.0, &entity.name, 13.0, self.config.theme.text_dim);
        }
    }

    fn handle_viewport_drag(&mut self, ui: &mut Ui, area: Rect) {
        let mx = ui.input.mouse_x;
        let my = ui.input.mouse_y;

        if ui.input.mouse_pressed && area.contains(mx, my) {
            let world_x = mx - area.x;
            let world_y = my - area.y;
            let mut hit = None;
            for entity in self.scene.entities.iter().rev() {
                let r = Rect::new(entity.x, entity.y, entity.size_x, entity.size_y);
                if r.contains(world_x, world_y) {
                    hit = Some((entity.id, world_x - entity.x, world_y - entity.y));
                    break;
                }
            }
            if let Some((id, gx, gy)) = hit {
                self.selected = Some(id);
                self.dragging = Some((id, gx, gy));
            }
        }

        if let Some((id, gx, gy)) = self.dragging {
            if ui.input.mouse_down {
                let world_x = mx - area.x;
                let world_y = my - area.y;
                let snap = self.config.layout.snap;
                let grid = self.config.layout.grid.max(1.0);
                if let Some(entity) = self.scene.entity_mut(id) {
                    let mut nx = world_x - gx;
                    let mut ny = world_y - gy;
                    if snap {
                        nx = (nx / grid).round() * grid;
                        ny = (ny / grid).round() * grid;
                    } else {
                        nx = nx.round();
                        ny = ny.round();
                    }
                    entity.x = nx;
                    entity.y = ny;
                }
                ui.wants_redraw = true;
            } else {
                self.dragging = None;
            }
        }
    }

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
    ) {
        let mx = ui.input.mouse_x;
        let my = ui.input.mouse_y;
        let theme = self.config.theme.clone();

        // Candidate splitter hot-zones.
        let left_edge = left_col.right();
        let right_edge = right_col.x;

        let mut hot: Option<Splitter> = None;
        if (my - bin_split_y).abs() <= SPLIT_HALF + 1.0 && my >= TOOLBAR_H {
            hot = Some(Splitter::BinHeight);
        } else if !left_panels.is_empty()
            && (mx - left_edge).abs() <= SPLIT_HALF + 1.0
            && my >= left_col.y
            && my <= left_col.bottom()
        {
            hot = Some(Splitter::LeftWidth);
        } else if !right_panels.is_empty()
            && (mx - right_edge).abs() <= SPLIT_HALF + 1.0
            && my >= right_col.y
            && my <= right_col.bottom()
        {
            hot = Some(Splitter::RightWidth);
        } else if left_panels.len() == 2 {
            let sy = left_col.y + left_col.h * self.config.layout.left_split.clamp(0.15, 0.85);
            if (my - sy).abs() <= SPLIT_HALF + 1.0 && mx >= left_col.x && mx <= left_col.right() {
                hot = Some(Splitter::LeftSplit);
            }
        }
        if hot.is_none() && right_panels.len() == 2 {
            let sy = right_col.y + right_col.h * self.config.layout.right_split.clamp(0.15, 0.85);
            if (my - sy).abs() <= SPLIT_HALF + 1.0 && mx >= right_col.x && mx <= right_col.right() {
                hot = Some(Splitter::RightSplit);
            }
        }

        if ui.input.mouse_pressed {
            if let Some(splitter) = hot {
                self.active_splitter = Some(splitter);
            }
        }
        if !ui.input.mouse_down {
            if self.active_splitter.take().is_some() {
                self.dirty = true;
            }
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
                    self.config.layout.bin_h = from_bottom.clamp(0.0, (body_total - 120.0).max(0.0));
                }
            }
        }

        // Draw the full-width bin splitter handle.
        {
            let highlight = active_or_hot(self.active_splitter, hot, Splitter::BinHeight);
            let color = if highlight { theme.splitter_hover } else { theme.splitter };
            ui.painter
                .fill_rect(Rect::new(0.0, bin_split_y - 1.0, w, 2.0), color);
        }

        // Draw vertical splitter handles, highlighted when hot or active.
        let active = self.active_splitter;
        if !left_panels.is_empty() {
            let highlight = active == Some(Splitter::LeftWidth) || hot == Some(Splitter::LeftWidth);
            let color = if highlight { theme.splitter_hover } else { theme.splitter };
            ui.painter.fill_rect(
                Rect::new(left_edge - 1.0, left_col.y, 2.0, left_col.h),
                color,
            );
        }
        if !right_panels.is_empty() {
            let highlight =
                active == Some(Splitter::RightWidth) || hot == Some(Splitter::RightWidth);
            let color = if highlight { theme.splitter_hover } else { theme.splitter };
            ui.painter.fill_rect(
                Rect::new(right_edge - 1.0, right_col.y, 2.0, right_col.h),
                color,
            );
        }
    }

    // ---- Project bin -------------------------------------------------------

    /// The bottom file browser, Unity's "Project" window. Lists the contents of
    /// the current directory (starting at the project root) with type icons and
    /// lets the user navigate folders.
    fn project_bin(&mut self, ui: &mut Ui, area: Rect) {
        ui.painter.fill_rect(area, self.config.theme.panel);
        let header = Rect::new(area.x, area.y, area.w, HEADER_H);
        ui.painter.fill_rect(header, self.config.theme.header);
        ui.icon(area.x + 16.0, area.y + HEADER_H / 2.0, icon::FOLDER_OPEN, 16.0, self.config.theme.text);
        ui.label(area.x + 30.0, area.y + (HEADER_H - 14.0) / 2.0, "Project", self.config.theme.text);

        // Breadcrumb showing the path relative to the project root.
        let rel = self
            .bin_dir
            .strip_prefix(&self.project_root)
            .ok()
            .map(|p| p.to_string_lossy().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| ".".to_string());
        ui.painter.text_clipped(
            area.x + 96.0,
            area.y + (HEADER_H - 13.0) / 2.0,
            &format!("/{rel}"),
            13.0,
            self.config.theme.text_dim,
            area.w - 200.0,
        );

        let at_root = self.bin_dir == self.project_root;
        if !at_root {
            let up = Rect::new(area.right() - 28.0, area.y + 3.0, 22.0, HEADER_H - 6.0);
            if ui.icon_toggle(up, icon::ARROW_UPWARD, false, self.config.theme.text) {
                if let Some(parent) = self.bin_dir.parent() {
                    if parent.starts_with(&self.project_root) || parent == self.project_root {
                        self.bin_dir = parent.to_path_buf();
                        self.bin_scroll = 0.0;
                    }
                }
            }
            ui.tooltip(up, "Up one folder");
        }
        ui.painter.stroke_rect(area, self.config.theme.border);

        let content = Rect::new(area.x, area.y + HEADER_H, area.w, (area.h - HEADER_H).max(0.0));

        // Collect directory entries once, folders first then files, sorted.
        let mut dirs: Vec<(PathBuf, String)> = Vec::new();
        let mut files: Vec<(PathBuf, String, char)> = Vec::new();
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

        // Scroll handling.
        if content.contains(ui.input.mouse_x, ui.input.mouse_y) && ui.input.scroll != 0.0 {
            self.bin_scroll -= ui.input.scroll * 32.0;
            ui.wants_redraw = true;
        }
        let max_scroll = (self.bin_content_h - content.h).max(0.0);
        self.bin_scroll = self.bin_scroll.clamp(0.0, max_scroll);

        let prev = ui.painter.push_clip(content);
        ui.set_input_clip(content);

        let row_h = 22.0;
        let mut y = content.y + 6.0 - self.bin_scroll;
        let mut navigate: Option<PathBuf> = None;
        let mut picked: Option<String> = None;

        let theme = self.config.theme.clone();
        let draw_row = |ui: &mut Ui, y: f32, glyph: char, name: &str, accent: bool| -> bool {
            let row = Rect::new(content.x + 4.0, y, content.w - 8.0, row_h);
            let hovered = row.contains(ui.input.mouse_x, ui.input.mouse_y)
                && content.contains(ui.input.mouse_x, ui.input.mouse_y);
            if hovered {
                ui.painter.fill_rect(row, theme.panel_alt);
            }
            let color = if accent { theme.accent } else { theme.text_dim };
            ui.icon(row.x + 12.0, y + row_h / 2.0, glyph, 15.0, color);
            ui.painter
                .text_clipped(row.x + 26.0, y + (row_h - 14.0) / 2.0, name, 14.0, theme.text, row.w - 30.0);
            hovered && ui.input.mouse_pressed
        };

        for (path, name) in &dirs {
            if draw_row(ui, y, icon::FOLDER, name, true) {
                navigate = Some(path.clone());
            }
            y += row_h + 1.0;
        }
        for (_path, name, glyph) in &files {
            if draw_row(ui, y, *glyph, name, false) {
                picked = Some(name.clone());
            }
            y += row_h + 1.0;
        }
        if dirs.is_empty() && files.is_empty() {
            ui.label(content.x + 10.0, content.y + 8.0, "Empty folder.", self.config.theme.text_dim);
            y += row_h;
        }

        ui.reset_input_clip();
        ui.painter.set_clip_raw(prev);
        self.bin_content_h = y - (content.y - self.bin_scroll) + 6.0;

        if let Some(path) = navigate {
            self.bin_dir = path;
            self.bin_scroll = 0.0;
        }
        if let Some(name) = picked {
            let rel_path = self
                .bin_dir
                .join(&name)
                .strip_prefix(&self.project_root)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or(name);
            self.status = format!("Selected asset: {rel_path}");
        }

        // Scrollbar.
        if self.bin_content_h > content.h {
            let thumb_h = (content.h * (content.h / self.bin_content_h)).max(20.0);
            let thumb_y =
                content.y + (self.bin_scroll / (self.bin_content_h - content.h)) * (content.h - thumb_h);
            ui.painter.fill_round_rect(
                Rect::new(content.right() - 6.0, thumb_y, 4.0, thumb_h),
                2.0,
                self.config.theme.text_dim,
            );
        }
    }

    // ---- Inspector ---------------------------------------------------------

    fn inspector_content(&mut self, ui: &mut Ui, area: Rect, start_y: f32) -> f32 {
        let x = area.x + PAD;
        let width = area.w - PAD * 2.0 - 6.0; // leave room for scrollbar
        let mut y = start_y;

        let Some(id) = self.selected else {
            // With nothing selected, edit scene-wide properties. The background
            // here is exported as `app.bg` and also fills the viewport.
            y = self.section_header(ui, x, width, y, icon::PALETTE, "Scene");
            let r = ui.text_field("scene_name_insp", Rect::new(x, y, width, FIELD_H), &self.scene.name);
            if r.changed && !r.text.is_empty() {
                self.scene.name = r.text;
            }
            y += FIELD_H + 6.0;
            let mut bg = self.scene.background;
            y = self.color_row(ui, "scene_bg", "app.bg", &mut bg, x, width, y);
            self.scene.background = bg;
            ui.label(
                x,
                y + 2.0,
                "Select an entity to edit it.",
                self.config.theme.text_dim,
            );
            return y + 26.0;
        };
        let Some(mut entity) = self.scene.entity(id).cloned() else {
            self.selected = None;
            return y + 10.0;
        };

        // Name.
        let r = ui.text_field("ent_name", Rect::new(x, y, width, FIELD_H), &entity.name);
        if r.changed {
            entity.name = r.text;
        }
        y += FIELD_H + 8.0;

        // Transform section.
        y = self.section_header(ui, x, width, y, icon::VIEW_IN_AR, "Transform");
        y = self.num_row(ui, "ent_x", "X", &mut entity.x, x, width, y);
        y = self.num_row(ui, "ent_y", "Y", &mut entity.y, x, width, y);
        y = self.num_row(ui, "ent_w", "Width", &mut entity.size_x, x, width, y);
        y = self.num_row(ui, "ent_h", "Height", &mut entity.size_y, x, width, y);
        y = self.num_row(ui, "ent_r", "Rotation", &mut entity.rotation, x, width, y);
        y += 6.0;

        // Components section.
        y = self.section_header(ui, x, width, y, icon::VIEW_QUILT, "Components");

        let mut remove_component: Option<usize> = None;
        for index in 0..entity.components.len() {
            let glyph = component_icon(&entity.components[index]);
            let label = entity.components[index].label();
            let header = Rect::new(x, y, width, ROW_H);
            ui.painter.fill_round_rect(header, 3.0, self.config.theme.panel_alt);
            ui.icon(x + 14.0, y + ROW_H / 2.0, glyph, 15.0, self.config.theme.accent);
            ui.label(x + 28.0, y + (ROW_H - 14.0) / 2.0, label, self.config.theme.text);
            let del = Rect::new(x + width - 22.0, y + 2.0, 20.0, ROW_H - 4.0);
            ui.tooltip(del, "Remove component");
            if ui.icon_toggle(del, icon::DELETE, false, self.config.theme.danger) {
                remove_component = Some(index);
            }
            y += ROW_H + 4.0;
            y = self.component_fields(ui, index, &mut entity.components[index], x, width, y);
            y += 6.0;
        }
        if let Some(index) = remove_component {
            entity.components.remove(index);
        }

        // Add-component buttons (two rows of two).
        let half = (width - 6.0) / 2.0;
        if ui.icon_button(Rect::new(x, y, half, FIELD_H + 2.0), icon::CROP_SQUARE, "Rect2D") {
            entity.components.push(Component::Rect2D { color: [200, 200, 200, 255] });
        }
        if ui.icon_button(
            Rect::new(x + half + 6.0, y, half, FIELD_H + 2.0),
            icon::TITLE,
            "Text2D",
        ) {
            entity.components.push(Component::Text2D {
                text: "Text".to_string(),
                color: [255, 255, 255, 255],
                size: 20.0,
            });
        }
        y += FIELD_H + 8.0;
        if ui.icon_button(Rect::new(x, y, half, FIELD_H + 2.0), icon::IMAGE, "Sprite") {
            entity.components.push(Component::Sprite {
                image: "assets/sprite.png".to_string(),
                tint: [255, 255, 255, 255],
            });
        }
        if ui.icon_button(
            Rect::new(x + half + 6.0, y, half, FIELD_H + 2.0),
            icon::DATA_OBJECT,
            "Script",
        ) {
            entity.components.push(Component::Script {
                path: "scripts/Behavior".to_string(),
                variables: Vec::new(),
            });
        }
        y += FIELD_H + 14.0;

        // Delete entity.
        if ui.icon_button(Rect::new(x, y, width, FIELD_H + 4.0), icon::DELETE, "Delete Entity") {
            self.scene.remove_entity(id);
            self.selected = None;
            self.status = "Deleted entity".to_string();
            return y + FIELD_H + 14.0;
        }
        y += FIELD_H + 14.0;

        self.scene.replace_entity(id, entity);
        y
    }

    fn section_header(&mut self, ui: &mut Ui, x: f32, width: f32, y: f32, glyph: char, title: &str) -> f32 {
        ui.painter
            .fill_rect(Rect::new(x, y, width, 1.0), self.config.theme.border);
        let y = y + 6.0;
        ui.icon(x + 8.0, y + 8.0, glyph, 15.0, self.config.theme.text_dim);
        ui.label(x + 20.0, y, title, self.config.theme.text_dim);
        y + 22.0
    }

    fn component_fields(
        &mut self,
        ui: &mut Ui,
        index: usize,
        component: &mut Component,
        x: f32,
        width: f32,
        mut y: f32,
    ) -> f32 {
        match component {
            Component::Rect2D { color } => {
                y = self.color_row(ui, &format!("rect_{index}"), "Color", color, x, width, y);
            }
            Component::Text2D { text, color, size } => {
                y = self.text_row(ui, &format!("txt_{index}"), "Text", text, x, width, y);
                y = self.num_row(ui, &format!("txtsz_{index}"), "Size", size, x, width, y);
                y = self.color_row(ui, &format!("txtcol_{index}"), "Color", color, x, width, y);
            }
            Component::Sprite { image, tint } => {
                y = self.text_row(ui, &format!("img_{index}"), "Image", image, x, width, y);
                y = self.color_row(ui, &format!("tint_{index}"), "Tint", tint, x, width, y);
            }
            Component::Script { path, variables } => {
                y = self.text_row(ui, &format!("spath_{index}"), "Script", path, x, width, y);
                y = self.script_variables(ui, index, variables, x, width, y);
            }
        }
        y
    }

    /// Render the Unity-style public-variable editor for a script component.
    fn script_variables(
        &mut self,
        ui: &mut Ui,
        comp_index: usize,
        variables: &mut Vec<ScriptVar>,
        x: f32,
        width: f32,
        mut y: f32,
    ) -> f32 {
        ui.icon(x + 8.0, y + 8.0, icon::PLAYLIST_ADD, 14.0, self.config.theme.text_dim);
        ui.label(x + 20.0, y, "Public Variables", self.config.theme.text_dim);
        y += 20.0;

        let mut remove_var: Option<usize> = None;
        for vi in 0..variables.len() {
            let base = format!("var_{comp_index}_{vi}");
            // Row 1: name field + type-cycle button + remove.
            let name_w = width - 90.0;
            let r = ui.text_field(
                &format!("{base}_name"),
                Rect::new(x, y, name_w.max(40.0), FIELD_H),
                &variables[vi].name,
            );
            if r.changed {
                variables[vi].name = r.text;
            }
            let type_btn = Rect::new(x + name_w + 4.0, y, 60.0, FIELD_H);
            if ui.button(type_btn, variables[vi].value.type_label()) {
                variables[vi].value = cycle_var_type(&variables[vi].value);
            }
            let del = Rect::new(x + width - 20.0, y, 20.0, FIELD_H);
            if ui.icon_toggle(del, icon::DELETE, false, self.config.theme.danger) {
                remove_var = Some(vi);
            }
            y += FIELD_H + 4.0;

            // Row 2: value editor for the variable's type.
            let vx = x + 16.0;
            let vw = width - 16.0;
            match &mut variables[vi].value {
                VarValue::Number(n) => {
                    y = self.num_row(ui, &format!("{base}_num"), "Value", n, vx, vw, y);
                }
                VarValue::Bool(b) => {
                    ui.label(vx, y + 4.0, "Value", self.config.theme.text);
                    if let Some(nv) = ui.checkbox(Rect::new(vx + LABEL_W, y, FIELD_H, FIELD_H), *b) {
                        *b = nv;
                    }
                    y += FIELD_H + 6.0;
                }
                VarValue::Text(s) => {
                    y = self.text_row(ui, &format!("{base}_txt"), "Value", s, vx, vw, y);
                }
                VarValue::Color(c) => {
                    y = self.color_row(ui, &format!("{base}_col"), "Value", c, vx, vw, y);
                }
            }
            y += 4.0;
        }
        if let Some(vi) = remove_var {
            variables.remove(vi);
        }

        if ui.icon_button(
            Rect::new(x, y, width, FIELD_H),
            icon::ADD_CIRCLE,
            "Add Variable",
        ) {
            variables.push(ScriptVar {
                name: format!("var{}", variables.len() + 1),
                value: VarValue::Number(0.0),
            });
        }
        y + FIELD_H + 6.0
    }

    fn num_row(&mut self, ui: &mut Ui, id: &str, label: &str, value: &mut f32, x: f32, width: f32, y: f32) -> f32 {
        ui.label(x, y + 4.0, label, self.config.theme.text);
        let fx = x + LABEL_W;
        let fw = (width - LABEL_W).max(30.0);
        let current = format_num(*value);
        let r = ui.text_field(id, Rect::new(fx, y, fw, FIELD_H), &current);
        if r.changed {
            if let Ok(parsed) = r.text.trim().parse::<f32>() {
                *value = parsed;
            } else if r.text.trim().is_empty() {
                *value = 0.0;
            }
        }
        y + FIELD_H + 6.0
    }

    fn text_row(&mut self, ui: &mut Ui, id: &str, label: &str, value: &mut String, x: f32, width: f32, y: f32) -> f32 {
        ui.label(x, y + 4.0, label, self.config.theme.text);
        let fx = x + LABEL_W;
        let fw = (width - LABEL_W).max(30.0);
        let r = ui.text_field(id, Rect::new(fx, y, fw, FIELD_H), value);
        if r.changed {
            *value = r.text;
        }
        y + FIELD_H + 6.0
    }

    fn color_row(&mut self, ui: &mut Ui, id: &str, label: &str, color: &mut [u8; 4], x: f32, width: f32, y: f32) -> f32 {
        ui.label(x, y + 4.0, label, self.config.theme.text);
        let fx = x + LABEL_W;
        // Swatch.
        ui.painter
            .fill_round_rect(Rect::new(fx, y, 22.0, FIELD_H), 3.0, [color[0], color[1], color[2], 255]);
        ui.painter
            .stroke_round_rect(Rect::new(fx, y, 22.0, FIELD_H), 3.0, self.config.theme.border);
        let cells_x = fx + 28.0;
        let avail = (x + width) - cells_x;
        let cell_w = ((avail - 8.0) / 3.0).max(24.0);
        let labels = ["R", "G", "B"];
        for (i, _lab) in labels.iter().enumerate() {
            let cx = cells_x + i as f32 * (cell_w + 4.0);
            let field_id = format!("{id}_{i}");
            let r = ui.text_field(&field_id, Rect::new(cx, y, cell_w, FIELD_H), &color[i].to_string());
            if r.changed {
                if let Ok(parsed) = r.text.trim().parse::<i32>() {
                    color[i] = parsed.clamp(0, 255) as u8;
                } else if r.text.trim().is_empty() {
                    color[i] = 0;
                }
            }
        }
        y + FIELD_H + 6.0
    }

    fn status_bar(&mut self, ui: &mut Ui, w: f32, h: f32) {
        let bar = Rect::new(0.0, h - STATUS_H, w, STATUS_H);
        ui.painter.fill_rect(bar, self.config.theme.toolbar);
        ui.painter.stroke_rect(bar, self.config.theme.border);
        let snap = if self.config.layout.snap { "snap on" } else { "snap off" };
        ui.painter.text(
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
                    .unwrap_or_default()
            ),
            13.0,
            self.config.theme.text_dim,
        );
    }

    // ---- Actions -----------------------------------------------------------

    fn add_entity(&mut self) {
        let n = self.scene.entities.len() + 1;
        let mut entity = self.scene.add_entity(format!("Entity {n}"), 96.0, 96.0);
        entity.components.push(Component::Rect2D { color: [120, 180, 120, 255] });
        let id = entity.id;
        self.scene.replace_entity(id, entity);
        self.selected = Some(id);
        self.status = "Added entity".to_string();
    }

    fn save(&mut self) {
        match self.scene.save(&self.scene_path) {
            Ok(()) => self.status = format!("Saved {}", self.scene_path.display()),
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    fn load(&mut self) {
        if !self.scene_path.exists() {
            self.status = format!("No scene file at {}", self.scene_path.display());
            return;
        }
        match Scene::load(&self.scene_path) {
            Ok(scene) => {
                self.scene = scene;
                self.selected = None;
                self.status = format!("Loaded {}", self.scene_path.display());
            }
            Err(e) => self.status = format!("Load failed: {e}"),
        }
    }

    fn export_luau(&mut self) {
        let path = self.project_root.join("main.luau");
        match std::fs::write(&path, self.scene.to_luau()) {
            Ok(()) => self.status = format!("Exported {}", path.display()),
            Err(e) => self.status = format!("Export failed: {e}"),
        }
    }

    /// Export the scene and launch the runtime to preview it.
    fn run_scene(&mut self) {
        self.export_luau();
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                self.status = format!("Run failed: {e}");
                return;
            }
        };
        match std::process::Command::new(exe)
            .arg("run")
            .arg(&self.project_root)
            .spawn()
        {
            Ok(_) => self.status = "Launched scene preview".to_string(),
            Err(e) => self.status = format!("Run failed: {e}"),
        }
    }
}

/// Whether a splitter is the active (dragged) or hovered handle.
fn active_or_hot(active: Option<Splitter>, hot: Option<Splitter>, which: Splitter) -> bool {
    active == Some(which) || hot == Some(which)
}

/// Clamp `v` into `[lo, hi]`, tolerating inverted bounds (which arise when the
/// window is too small for a panel's preferred minimum). `f32::clamp` panics if
/// `lo > hi`; this never does.
fn clamp_range(v: f32, lo: f32, hi: f32) -> f32 {
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    v.clamp(lo, hi)
}

/// The Material icon representing a file, chosen by extension.
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

/// The Material icon representing a component kind.
fn component_icon(component: &Component) -> char {
    match component {
        Component::Rect2D { .. } => icon::CROP_SQUARE,
        Component::Text2D { .. } => icon::TITLE,
        Component::Sprite { .. } => icon::IMAGE,
        Component::Script { .. } => icon::DATA_OBJECT,
    }
}

/// Advance a variable's value to the next type, preserving a sensible default.
fn cycle_var_type(value: &VarValue) -> VarValue {
    match value {
        VarValue::Number(_) => VarValue::Bool(false),
        VarValue::Bool(_) => VarValue::Text(String::new()),
        VarValue::Text(_) => VarValue::Color([255, 255, 255, 255]),
        VarValue::Color(_) => VarValue::Number(0.0),
    }
}

/// Read the editor config from disk, falling back to defaults when absent or
/// invalid. Missing fields are filled by `#[serde(default)]`.
pub fn load_config(path: &std::path::Path) -> EditorConfig {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(config) => config,
            Err(error) => {
                eprintln!("warning: failed to parse {}: {error}", path.display());
                EditorConfig::default()
            }
        },
        Err(_) => EditorConfig::default(),
    }
}

/// Write the editor config to disk as pretty JSON.
pub fn save_config(path: &std::path::Path, config: &EditorConfig) -> Result<(), String> {
    let text =
        serde_json::to_string_pretty(config).map_err(|e| format!("failed to serialize config: {e}"))?;
    std::fs::write(path, text).map_err(|e| format!("failed to write {}: {e}", path.display()))
}

/// Format a float for display, trimming trailing zeros.
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

    /// A headless driver that runs editor frames against an in-memory
    /// framebuffer, threading focus state exactly as the real event loop does.
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
            let app = EditorApp::new(
                dir.clone(),
                dir.join("scene.neoscene"),
                scene,
                EditorConfig::default(),
            );
            Self {
                app,
                fonts: load_fonts().expect("load fonts"),
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
            );
            self.app.frame(&mut ui);
            let (focus, edit) = ui.into_focus_state();
            self.app.set_focus(focus, edit);
        }

        /// Simulate a full click (press frame, then release frame).
        fn click(&mut self, x: f32, y: f32) {
            let press = FrameInput {
                mouse_x: x,
                mouse_y: y,
                mouse_pressed: true,
                mouse_down: true,
                ..Default::default()
            };
            self.frame(press);
            let release = FrameInput {
                mouse_x: x,
                mouse_y: y,
                ..Default::default()
            };
            self.frame(release);
        }
    }

    #[test]
    fn renders_default_scene_without_panicking() {
        let mut h = Harness::new(Scene::default());
        h.frame(FrameInput::default());
        // The viewport should contain the blue starter box, so the buffer is
        // not a single flat color.
        let first = h.buffer[0];
        assert!(h.buffer.iter().any(|&p| p != first));
    }

    #[test]
    fn hovering_a_toolbar_button_shows_a_tooltip() {
        let mut h = Harness::new(Scene::default());
        // Hovering the "Save scene" button exercises the tooltip path without
        // panicking.
        h.frame(FrameInput {
            mouse_x: 58.0,
            mouse_y: 20.0,
            ..Default::default()
        });
    }

    #[test]
    fn clicking_toolbar_add_entity_adds_an_entity() {
        let mut h = Harness::new(Scene::default());
        assert_eq!(h.app.entity_count(), 1);
        // The "+ entity" (add_circle) toolbar button center.
        h.click(206.0, 20.0);
        assert_eq!(h.app.entity_count(), 2);
        assert!(h.app.selected_id().is_some());
    }

    #[test]
    fn clicking_a_hierarchy_row_selects_the_entity() {
        let mut h = Harness::new(Scene::default());
        assert!(h.app.selected_id().is_none());
        // First hierarchy row sits just below the panel header.
        h.click(115.0, 88.0);
        assert!(h.app.selected_id().is_some());
    }

    #[test]
    fn toolbar_grid_toggle_flips_snapping() {
        let mut h = Harness::new(Scene::default());
        assert!(h.app.snap_enabled());
        h.click(249.0, 20.0);
        assert!(!h.app.snap_enabled());
    }

    #[test]
    fn inspector_renders_every_component_kind() {
        // A scene whose entity carries one of each component, including a script
        // with all public-variable types, must render without panicking.
        let mut scene = Scene::default();
        let mut entity = scene.add_entity("Rich", 64.0, 64.0);
        entity.components.push(Component::Rect2D {
            color: [10, 20, 30, 255],
        });
        entity.components.push(Component::Text2D {
            text: "Hi".into(),
            color: [255, 255, 255, 255],
            size: 18.0,
        });
        entity.components.push(Component::Sprite {
            image: "a.png".into(),
            tint: [255, 255, 255, 255],
        });
        entity.components.push(Component::Script {
            path: "scripts/Rich".into(),
            variables: vec![
                ScriptVar {
                    name: "speed".into(),
                    value: VarValue::Number(5.0),
                },
                ScriptVar {
                    name: "on".into(),
                    value: VarValue::Bool(true),
                },
                ScriptVar {
                    name: "title".into(),
                    value: VarValue::Text("x".into()),
                },
                ScriptVar {
                    name: "tint".into(),
                    value: VarValue::Color([1, 2, 3, 255]),
                },
            ],
        });
        let id = entity.id;
        scene.replace_entity(id, entity);

        let mut h = Harness::new(scene);
        // Select the second hierarchy row ("Rich").
        h.click(115.0, 113.0);
        assert_eq!(h.app.selected_id(), Some(id));
        // Render several more frames; the inspector now draws all editors.
        for _ in 0..3 {
            h.frame(FrameInput::default());
        }
    }

    #[test]
    fn layout_is_responsive_at_small_resolutions() {
        // The clamping layout must render without panicking at any window size,
        // including sizes smaller than the requested minimum.
        for (w, h) in [(640, 420), (500, 360), (320, 240), (200, 160)] {
            let mut scene = Scene::default();
            let mut e = scene.add_entity("Rich", 10.0, 10.0);
            e.components.push(Component::Script {
                path: "s".into(),
                variables: vec![ScriptVar {
                    name: "v".into(),
                    value: VarValue::Color([1, 2, 3, 255]),
                }],
            });
            let id = e.id;
            scene.replace_entity(id, e);

            let mut h_ = Harness::with_size(scene, w, h);
            // Select to force the (tall) inspector to render in a tiny window.
            h_.app.selected = Some(id);
            h_.frame(FrameInput::default());
        }
    }

    #[test]
    fn editing_scene_background_with_no_selection() {
        let mut h = Harness::new(Scene::default());
        // Nothing selected: the inspector shows the Scene panel. Rendering it
        // exercises the app.bg color editor without panicking.
        assert!(h.app.selected_id().is_none());
        h.frame(FrameInput::default());
    }
}
