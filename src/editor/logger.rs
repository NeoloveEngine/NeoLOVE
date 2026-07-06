//! The live logger window shown while a scene runs from the editor.
//!
//! It renders three regions fed by [`LoggerState`] (updated over IPC by the
//! running game): a live hierarchy of the running scene, an inspector for the
//! selected entity, and a streaming console of the game's `print`/log output.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::editor_ipc::LoggerState;
use crate::window::EntitySnapshot;

use super::ui::{Rect, Ui};

const HEADER_H: f32 = 30.0;
const ROW_H: f32 = 20.0;
const PAD: f32 = 8.0;
const TEXT: f32 = 13.0;

/// Per-window UI state for the logger. The live data lives behind `state`.
pub struct LoggerWindow {
    state: Arc<Mutex<LoggerState>>,
    selected: Option<usize>,
    tree_scroll: f32,
    inspector_scroll: f32,
    log_scroll: f32,
    /// Keep the console pinned to the newest line until the user scrolls up.
    log_follow: bool,
}

impl LoggerWindow {
    pub fn new(state: Arc<Mutex<LoggerState>>) -> Self {
        Self {
            state,
            selected: None,
            tree_scroll: 0.0,
            inspector_scroll: 0.0,
            log_scroll: 0.0,
            log_follow: true,
        }
    }

    pub fn frame(&mut self, ui: &mut Ui) {
        let w = ui.painter.width();
        let h = ui.painter.height();
        ui.painter.clear(ui.theme.viewport_bg);

        // Snapshot the shared state for this frame to keep the lock brief.
        let (logs, entities, connected, finished) = {
            let guard = self.state.lock().expect("logger state poisoned");
            (
                guard
                    .logs
                    .iter()
                    .map(|line| (line.level.clone(), line.message.clone()))
                    .collect::<Vec<_>>(),
                guard.entities.clone(),
                guard.connected,
                guard.finished,
            )
        };

        self.draw_header(ui, w, connected, finished, entities.len());

        let log_h = (h * 0.34).clamp(90.0, (h - HEADER_H - 80.0).max(90.0));
        let main_top = HEADER_H;
        let main_bottom = h - log_h;
        let split = (w * 0.46).round();

        let tree_rect = Rect::new(0.0, main_top, split, main_bottom - main_top);
        let inspector_rect =
            Rect::new(split + 1.0, main_top, w - split - 1.0, main_bottom - main_top);
        let log_rect = Rect::new(0.0, main_bottom, w, log_h);

        self.draw_hierarchy(ui, tree_rect, &entities);
        self.draw_inspector(ui, inspector_rect, &entities);
        self.draw_console(ui, log_rect, &logs);
    }

    fn draw_header(&mut self, ui: &mut Ui, w: f32, connected: bool, finished: bool, count: usize) {
        ui.painter
            .fill_rect(Rect::new(0.0, 0.0, w, HEADER_H), ui.theme.toolbar);
        let (status, color) = if finished {
            ("Game exited", ui.theme.text_dim)
        } else if connected {
            ("Connected", ui.theme.accent)
        } else {
            ("Waiting for game…", ui.theme.text_dim)
        };
        ui.label(PAD, 8.0, status, color);
        ui.label(
            150.0,
            8.0,
            &format!("{count} entities"),
            ui.theme.text_dim,
        );

        let clear = Rect::new(w - 90.0, 5.0, 80.0, HEADER_H - 10.0);
        if ui.button(clear, "Clear log") {
            if let Ok(mut guard) = self.state.lock() {
                guard.clear_logs();
            }
            self.log_scroll = 0.0;
            self.log_follow = true;
        }
    }

    fn draw_hierarchy(&mut self, ui: &mut Ui, rect: Rect, entities: &[EntitySnapshot]) {
        ui.painter.fill_rect(rect, ui.theme.panel);
        ui.painter.fill_rect(
            Rect::new(rect.x, rect.y, rect.w, ROW_H),
            ui.theme.header,
        );
        ui.label(rect.x + PAD, rect.y + 4.0, "Hierarchy", ui.theme.text_dim);

        let body = Rect::new(
            rect.x,
            rect.y + ROW_H,
            rect.w,
            (rect.h - ROW_H).max(0.0),
        );
        let ordered = ordered_with_depth(entities);
        let content_h = ordered.len() as f32 * ROW_H;
        self.tree_scroll = apply_scroll(ui, body, self.tree_scroll, content_h);

        let clip = ui.painter.push_clip(body);
        let mut y = body.y - self.tree_scroll;
        for (index, depth) in ordered {
            let entity = &entities[index];
            if y + ROW_H >= body.y && y <= body.bottom() {
                let row = Rect::new(body.x, y, body.w, ROW_H);
                let label = if entity.name.is_empty() {
                    format!("Entity {}", entity.id)
                } else {
                    entity.name.clone()
                };
                let selected = self.selected == Some(entity.id);
                if ui.list_row(row, &label, selected, depth as f32 * 14.0) {
                    self.selected = Some(entity.id);
                }
            }
            y += ROW_H;
        }
        ui.painter.set_clip_raw(clip);
    }

    fn draw_inspector(&mut self, ui: &mut Ui, rect: Rect, entities: &[EntitySnapshot]) {
        ui.painter.fill_rect(rect, ui.theme.panel_alt);
        ui.painter.fill_rect(
            Rect::new(rect.x, rect.y, rect.w, ROW_H),
            ui.theme.header,
        );
        ui.label(rect.x + PAD, rect.y + 4.0, "Inspector", ui.theme.text_dim);

        let body = Rect::new(rect.x, rect.y + ROW_H, rect.w, (rect.h - ROW_H).max(0.0));
        let Some(entity) = self
            .selected
            .and_then(|id| entities.iter().find(|e| e.id == id))
        else {
            ui.label(
                body.x + PAD,
                body.y + 6.0,
                "Select an entity to inspect.",
                ui.theme.text_dim,
            );
            return;
        };

        // Lay the inspector out into flat lines, then scroll/clip them.
        let mut lines: Vec<(String, bool)> = Vec::new();
        lines.push((
            if entity.name.is_empty() {
                format!("Entity {}", entity.id)
            } else {
                format!("{}  (id {})", entity.name, entity.id)
            },
            true,
        ));
        lines.push((format!("x = {:.2}    y = {:.2}", entity.x, entity.y), false));
        lines.push((
            format!("rotation = {:.3}    scale = {:.3}", entity.rotation, entity.scale),
            false,
        ));
        lines.push((format!("enabled = {}", entity.enabled), false));
        for component in &entity.components {
            lines.push((String::new(), false));
            lines.push((component.name.clone(), true));
            for (key, value) in &component.fields {
                lines.push((format!("    {key} = {value}"), false));
            }
        }

        let content_h = lines.len() as f32 * ROW_H;
        self.inspector_scroll = apply_scroll(ui, body, self.inspector_scroll, content_h);
        let clip = ui.painter.push_clip(body);
        let mut y = body.y + 4.0 - self.inspector_scroll;
        for (text, header) in &lines {
            if y + ROW_H >= body.y && y <= body.bottom() && !text.is_empty() {
                let color = if *header { ui.theme.text } else { ui.theme.text_dim };
                ui.painter
                    .text_clipped(body.x + PAD, y, text, TEXT, color, body.w - PAD * 2.0);
            }
            y += ROW_H;
        }
        ui.painter.set_clip_raw(clip);
    }

    fn draw_console(&mut self, ui: &mut Ui, rect: Rect, logs: &[(String, String)]) {
        ui.painter.fill_rect(rect, ui.theme.panel);
        ui.painter
            .fill_rect(Rect::new(rect.x, rect.y, rect.w, ROW_H), ui.theme.header);
        ui.label(rect.x + PAD, rect.y + 4.0, "Console", ui.theme.text_dim);

        let body = Rect::new(rect.x, rect.y + ROW_H, rect.w, (rect.h - ROW_H).max(0.0));
        let line_h = 16.0;
        let content_h = logs.len() as f32 * line_h;
        let max_scroll = (content_h - body.h).max(0.0);

        // Follow the tail unless the user scrolled up; any wheel input releases.
        if hovered(ui, body) && ui.input.scroll != 0.0 {
            self.log_scroll -= ui.input.scroll * line_h * 3.0;
            self.log_follow = false;
        }
        if self.log_follow {
            self.log_scroll = max_scroll;
        }
        self.log_scroll = self.log_scroll.clamp(0.0, max_scroll);
        if (self.log_scroll - max_scroll).abs() < 0.5 {
            self.log_follow = true;
        }

        let clip = ui.painter.push_clip(body);
        let mut y = body.y + 2.0 - self.log_scroll;
        for (level, message) in logs {
            if y + line_h >= body.y && y <= body.bottom() {
                let color = if level == "error" {
                    ui.theme.danger
                } else {
                    ui.theme.text
                };
                ui.painter
                    .text_clipped(body.x + PAD, y, message, TEXT, color, body.w - PAD * 2.0);
            }
            y += line_h;
        }
        ui.painter.set_clip_raw(clip);
    }
}

/// True if the pointer is within `rect` this frame.
fn hovered(ui: &Ui, rect: Rect) -> bool {
    rect.contains(ui.input.mouse_x, ui.input.mouse_y)
}

/// Apply wheel scrolling to a panel and return the clamped offset.
fn apply_scroll(ui: &Ui, body: Rect, mut scroll: f32, content_h: f32) -> f32 {
    let max_scroll = (content_h - body.h).max(0.0);
    if hovered(ui, body) {
        scroll -= ui.input.scroll * ROW_H * 3.0;
    }
    scroll.clamp(0.0, max_scroll)
}

/// Flatten the snapshot into display order (`depth`-first from the roots) so the
/// hierarchy reads like a tree. Entities parented to the implicit root (`0`) or
/// to a missing parent are treated as top level.
fn ordered_with_depth(entities: &[EntitySnapshot]) -> Vec<(usize, usize)> {
    let index_by_id: HashMap<usize, usize> = entities
        .iter()
        .enumerate()
        .map(|(index, entity)| (entity.id, index))
        .collect();
    let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut roots: Vec<usize> = Vec::new();
    for (index, entity) in entities.iter().enumerate() {
        match entity.parent {
            Some(parent) if parent != 0 && index_by_id.contains_key(&parent) => {
                children.entry(parent).or_default().push(index);
            }
            _ => roots.push(index),
        }
    }

    let mut out = Vec::with_capacity(entities.len());
    let mut stack: Vec<(usize, usize)> = roots.into_iter().rev().map(|index| (index, 0)).collect();
    while let Some((index, depth)) = stack.pop() {
        out.push((index, depth));
        if let Some(kids) = children.get(&entities[index].id) {
            for &child in kids.iter().rev() {
                stack.push((child, depth + 1));
            }
        }
    }
    out
}
