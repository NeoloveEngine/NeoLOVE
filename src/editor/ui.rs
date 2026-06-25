//! A tiny immediate-mode UI toolkit rendered into a software framebuffer.
//!
//! The toolkit is intentionally minimal: it draws (optionally rounded)
//! rectangles, outlines and text into a `u32` buffer (the `0x00RRGGBB` format
//! softbuffer expects) and tracks just enough mouse and keyboard state to drive
//! buttons, checkboxes and editable text fields. It carries a [`Theme`] so the
//! whole editor can be recolored from a config file, and supports a clip
//! rectangle so panels can scroll their contents without bleeding over one
//! another. It has no external GUI dependency, keeping the editor self-contained
//! within the engine crate.

use std::sync::Arc;

use fontdue::Font;
use serde::{Deserialize, Serialize};

/// Open Sans — a clean, highly legible proportional sans-serif used for all
/// editor chrome (labels, buttons, fields). Pairs naturally with Material Icons.
const EDITOR_FONT_BYTES: &[u8] = include_bytes!("assets/OpenSans-Regular.ttf");

/// Google Material Icons, rendered by codepoint for editor glyphs.
const ICON_FONT_BYTES: &[u8] = include_bytes!("assets/MaterialIcons-Regular.ttf");

/// Named Google Material Icons codepoints used across the editor.
pub mod icon {
    pub const ADD: char = '\u{e145}';
    pub const SAVE: char = '\u{e161}';
    pub const FOLDER_OPEN: char = '\u{e2c8}';
    pub const DELETE: char = '\u{e872}';
    pub const PLAY: char = '\u{e037}';
    pub const CODE: char = '\u{e86f}';
    pub const GRID_ON: char = '\u{e3ec}';
    pub const GRID_OFF: char = '\u{e3eb}';
    pub const CROP_SQUARE: char = '\u{e3c6}';
    pub const TITLE: char = '\u{e264}';
    pub const IMAGE: char = '\u{e3f4}';
    pub const DATA_OBJECT: char = '\u{ead3}';
    pub const TUNE: char = '\u{e429}';
    pub const VIEW_IN_AR: char = '\u{e9fe}';
    pub const NOTE_ADD: char = '\u{e89c}';
    pub const ADD_CIRCLE: char = '\u{e147}';
    pub const ACCOUNT_TREE: char = '\u{e97a}';
    pub const VIEW_QUILT: char = '\u{e8f1}';
    pub const PLAYLIST_ADD: char = '\u{e03b}';
    pub const BORDER_ALL: char = '\u{e228}';
    pub const EXPAND_MORE: char = '\u{e5cf}';
    pub const CHEVRON_RIGHT: char = '\u{e5cc}';
    pub const CHEVRON_LEFT: char = '\u{e5cb}';
    pub const CONTENT_COPY: char = '\u{e14d}';
    pub const CONTENT_PASTE: char = '\u{e14f}';
    pub const CREATE_NEW_FOLDER: char = '\u{e2cc}';
    pub const EDIT: char = '\u{e3c9}';
    pub const OPEN_IN_NEW: char = '\u{e89e}';
    pub const FOLDER: char = '\u{e2c7}';
    pub const ARROW_UPWARD: char = '\u{e5d8}';
    pub const INSERT_DRIVE_FILE: char = '\u{e24d}';
    pub const AUDIOTRACK: char = '\u{e3a1}';
    pub const FONT_DOWNLOAD: char = '\u{e167}';
    pub const ARTICLE: char = '\u{ef42}';
    pub const PALETTE: char = '\u{e40a}';
    pub const SEARCH: char = '\u{e8b6}';
    pub const VISIBILITY: char = '\u{e8f4}';
    pub const VISIBILITY_OFF: char = '\u{e8f5}';
    pub const RESTART_ALT: char = '\u{f053}';
    pub const CENTER_FOCUS: char = '\u{e3b4}';
    pub const MY_LOCATION: char = '\u{e55c}';
    /// Used for the "swap dock side" affordance on a panel header.
    pub const SWAP: char = '\u{e8f1}';
}

/// An RGBA color in `[r, g, b, a]` byte order.
pub type Rgba = [u8; 4];

/// A rectangle in framebuffer pixel coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    pub fn shrink(&self, amount: f32) -> Rect {
        Rect::new(
            self.x + amount,
            self.y + amount,
            (self.w - amount * 2.0).max(0.0),
            (self.h - amount * 2.0).max(0.0),
        )
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// The editor color palette. Serializable so it can be loaded from and written
/// to `editor_theme.json`; the [`Default`] is a Visual Studio Code "Dark+"
/// inspired scheme.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    /// Side panel background.
    pub panel: Rgba,
    /// Alternate/recessed panel background (rows, sub-headers).
    pub panel_alt: Rgba,
    /// Title bar / toolbar background.
    pub toolbar: Rgba,
    /// 2D viewport backdrop behind the scene area.
    pub viewport_bg: Rgba,
    /// Hairline borders.
    pub border: Rgba,
    /// Primary text.
    pub text: Rgba,
    /// Secondary/disabled text.
    pub text_dim: Rgba,
    /// Button face.
    pub button: Rgba,
    /// Button face when hovered.
    pub button_hover: Rgba,
    /// Selected/active highlight (e.g. selected list row).
    pub button_active: Rgba,
    /// Text input background.
    pub field: Rgba,
    /// Text input background when focused.
    pub field_focus: Rgba,
    /// Accent color (focus rings, highlights).
    pub accent: Rgba,
    /// Selection outline in the viewport.
    pub selection: Rgba,
    /// Destructive action color.
    pub danger: Rgba,
    /// Splitter handle color.
    pub splitter: Rgba,
    /// Splitter handle color when hovered/dragged.
    pub splitter_hover: Rgba,
    /// Panel header strip background.
    pub header: Rgba,
    /// Faint viewport grid lines.
    pub grid: Rgba,
    /// Corner radius for buttons and fields, in pixels.
    pub corner_radius: f32,
}

impl Default for Theme {
    fn default() -> Self {
        // Visual Studio Code "Dark+" palette.
        Self {
            panel: [37, 37, 38, 255],         // #252526 side bar
            panel_alt: [45, 45, 45, 255],     // #2d2d2d
            toolbar: [60, 60, 60, 255],       // #3c3c3c title bar
            viewport_bg: [30, 30, 30, 255],   // #1e1e1e editor
            border: [69, 69, 69, 255],        // #454545
            text: [212, 212, 212, 255],       // #d4d4d4
            text_dim: [133, 133, 133, 255],   // #858585
            button: [14, 99, 156, 255],       // #0e639c
            button_hover: [17, 119, 187, 255], // #1177bb
            button_active: [9, 71, 113, 255], // #094771 selection
            field: [60, 60, 60, 255],         // #3c3c3c input
            field_focus: [45, 45, 45, 255],
            accent: [0, 122, 204, 255],        // #007acc
            selection: [255, 199, 89, 255],    // viewport gizmo amber
            danger: [241, 76, 76, 255],        // #f14c4c
            splitter: [51, 51, 51, 255],
            splitter_hover: [0, 122, 204, 255],
            header: [51, 51, 51, 255],
            grid: [255, 255, 255, 16],
            corner_radius: 4.0,
        }
    }
}

/// Pack an RGBA color into the `0x00RRGGBB` word softbuffer presents.
fn pack(color: Rgba) -> u32 {
    (color[2] as u32) | ((color[1] as u32) << 8) | ((color[0] as u32) << 16)
}

/// Alpha-blend `src` over an existing packed `dst` pixel.
fn blend(dst: u32, src: Rgba) -> u32 {
    let a = src[3] as u32;
    if a == 0 {
        return dst;
    }
    if a == 255 {
        return pack(src);
    }
    let inv = 255 - a;
    let dr = (dst >> 16) & 0xff;
    let dg = (dst >> 8) & 0xff;
    let db = dst & 0xff;
    let r = (src[0] as u32 * a + dr * inv) / 255;
    let g = (src[1] as u32 * a + dg * inv) / 255;
    let b = (src[2] as u32 * a + db * inv) / 255;
    b | (g << 8) | (r << 16)
}

/// Signed distance from a point to a rounded rectangle's edge (negative
/// inside). Used to render antialiased rounded corners.
fn round_rect_sdf(rect: Rect, radius: f32, cx: f32, cy: f32) -> f32 {
    let hw = rect.w * 0.5;
    let hh = rect.h * 0.5;
    let r = radius.min(hw).min(hh).max(0.0);
    let px = cx - (rect.x + hw);
    let py = cy - (rect.y + hh);
    let dx = px.abs() - (hw - r);
    let dy = py.abs() - (hh - r);
    let ax = dx.max(0.0);
    let ay = dy.max(0.0);
    (ax * ax + ay * ay).sqrt() + dx.max(dy).min(0.0) - r
}

/// A pair of fonts (UI text + Material Icons), loaded once and shared cheaply.
#[derive(Clone)]
pub struct Fonts {
    pub text: Arc<Font>,
    pub icons: Arc<Font>,
}

/// Draws shapes and text into a borrowed framebuffer with an optional clip.
pub struct Painter<'a> {
    buffer: &'a mut [u32],
    width: usize,
    height: usize,
    font: Arc<Font>,
    icon_font: Arc<Font>,
    /// Clip bounds in pixels: `[x0, y0, x1, y1)`.
    clip: [i64; 4],
}

impl<'a> Painter<'a> {
    pub fn new(buffer: &'a mut [u32], width: usize, height: usize, fonts: Fonts) -> Self {
        Self {
            buffer,
            width,
            height,
            font: fonts.text,
            icon_font: fonts.icons,
            clip: [0, 0, width as i64, height as i64],
        }
    }

    pub fn width(&self) -> f32 {
        self.width as f32
    }

    pub fn height(&self) -> f32 {
        self.height as f32
    }

    /// Restrict drawing to `rect` (intersected with the framebuffer). Returns
    /// the previous clip so it can be restored with [`Painter::set_clip_raw`].
    pub fn push_clip(&mut self, rect: Rect) -> [i64; 4] {
        let prev = self.clip;
        let x0 = rect.x.floor() as i64;
        let y0 = rect.y.floor() as i64;
        let x1 = rect.right().ceil() as i64;
        let y1 = rect.bottom().ceil() as i64;
        self.clip = [
            x0.max(prev[0]).max(0),
            y0.max(prev[1]).max(0),
            x1.min(prev[2]).min(self.width as i64),
            y1.min(prev[3]).min(self.height as i64),
        ];
        prev
    }

    pub fn set_clip_raw(&mut self, clip: [i64; 4]) {
        self.clip = clip;
    }

    fn in_clip(&self, x: i64, y: i64) -> bool {
        x >= self.clip[0] && x < self.clip[2] && y >= self.clip[1] && y < self.clip[3]
    }

    /// Fill the entire framebuffer with a solid color (ignores clip).
    pub fn clear(&mut self, color: Rgba) {
        let packed = pack(color);
        for pixel in self.buffer.iter_mut() {
            *pixel = packed;
        }
    }

    fn put(&mut self, x: i64, y: i64, color: Rgba) {
        if color[3] == 0 || !self.in_clip(x, y) {
            return;
        }
        let idx = y as usize * self.width + x as usize;
        self.buffer[idx] = blend(self.buffer[idx], color);
    }

    /// Fill a rectangle, clipped and alpha-blended.
    pub fn fill_rect(&mut self, rect: Rect, color: Rgba) {
        if color[3] == 0 {
            return;
        }
        let x0 = (rect.x.floor() as i64).max(self.clip[0]);
        let y0 = (rect.y.floor() as i64).max(self.clip[1]);
        let x1 = (rect.right().ceil() as i64).min(self.clip[2]);
        let y1 = (rect.bottom().ceil() as i64).min(self.clip[3]);
        for y in y0..y1 {
            let row = y as usize * self.width;
            for x in x0..x1 {
                let idx = row + x as usize;
                self.buffer[idx] = blend(self.buffer[idx], color);
            }
        }
    }

    /// Fill a rounded rectangle with antialiased corners.
    pub fn fill_round_rect(&mut self, rect: Rect, radius: f32, color: Rgba) {
        if radius <= 0.5 {
            self.fill_rect(rect, color);
            return;
        }
        if color[3] == 0 {
            return;
        }
        let x0 = (rect.x.floor() as i64).max(self.clip[0]);
        let y0 = (rect.y.floor() as i64).max(self.clip[1]);
        let x1 = (rect.right().ceil() as i64).min(self.clip[2]);
        let y1 = (rect.bottom().ceil() as i64).min(self.clip[3]);
        for y in y0..y1 {
            for x in x0..x1 {
                let sdf = round_rect_sdf(rect, radius, x as f32 + 0.5, y as f32 + 0.5);
                let coverage = (0.5 - sdf).clamp(0.0, 1.0);
                if coverage <= 0.0 {
                    continue;
                }
                let alpha = (color[3] as f32 * coverage) as u8;
                self.put(x, y, [color[0], color[1], color[2], alpha]);
            }
        }
    }

    /// Stroke a one-pixel rectangle outline.
    pub fn stroke_rect(&mut self, rect: Rect, color: Rgba) {
        self.fill_rect(Rect::new(rect.x, rect.y, rect.w, 1.0), color);
        self.fill_rect(Rect::new(rect.x, rect.bottom() - 1.0, rect.w, 1.0), color);
        self.fill_rect(Rect::new(rect.x, rect.y, 1.0, rect.h), color);
        self.fill_rect(Rect::new(rect.right() - 1.0, rect.y, 1.0, rect.h), color);
    }

    /// Stroke an antialiased rounded rectangle outline.
    pub fn stroke_round_rect(&mut self, rect: Rect, radius: f32, color: Rgba) {
        if radius <= 0.5 {
            self.stroke_rect(rect, color);
            return;
        }
        let x0 = ((rect.x - 1.0).floor() as i64).max(self.clip[0]);
        let y0 = ((rect.y - 1.0).floor() as i64).max(self.clip[1]);
        let x1 = ((rect.right() + 1.0).ceil() as i64).min(self.clip[2]);
        let y1 = ((rect.bottom() + 1.0).ceil() as i64).min(self.clip[3]);
        for y in y0..y1 {
            for x in x0..x1 {
                let sdf = round_rect_sdf(rect, radius, x as f32 + 0.5, y as f32 + 0.5);
                let coverage = (1.0 - sdf.abs()).clamp(0.0, 1.0);
                if coverage <= 0.0 {
                    continue;
                }
                let alpha = (color[3] as f32 * coverage) as u8;
                self.put(x, y, [color[0], color[1], color[2], alpha]);
            }
        }
    }

    /// Measure the advance width of a string at the given pixel size.
    pub fn text_width(&self, text: &str, size: f32) -> f32 {
        let mut width = 0.0;
        for ch in text.chars() {
            width += self.font.metrics(ch, size).advance_width;
        }
        width
    }

    /// Draw left-aligned text with `x`/`y` marking the top-left of the line.
    pub fn text(&mut self, x: f32, y: f32, text: &str, size: f32, color: Rgba) {
        let line = self.font.horizontal_line_metrics(size);
        let ascent = line.map(|m| m.ascent).unwrap_or(size);
        let baseline = y + ascent;
        let mut pen = x;
        for ch in text.chars() {
            let (metrics, bitmap) = self.font.rasterize(ch, size);
            let gx = pen + metrics.xmin as f32;
            let gy = baseline - (metrics.height as f32 + metrics.ymin as f32);
            self.blit_coverage(gx, gy, metrics.width, metrics.height, &bitmap, color);
            pen += metrics.advance_width;
        }
    }

    /// Draw text clipped to `max_width`, appending an ellipsis when truncated.
    pub fn text_clipped(
        &mut self,
        x: f32,
        y: f32,
        text: &str,
        size: f32,
        color: Rgba,
        max_width: f32,
    ) {
        if self.text_width(text, size) <= max_width {
            self.text(x, y, text, size, color);
            return;
        }
        let ell_w = self.text_width("…", size);
        let mut truncated = String::new();
        let mut width = 0.0;
        for ch in text.chars() {
            let cw = self.font.metrics(ch, size).advance_width;
            if width + cw + ell_w > max_width {
                break;
            }
            truncated.push(ch);
            width += cw;
        }
        truncated.push('…');
        self.text(x, y, &truncated, size, color);
    }

    /// Draw a Material Icons glyph, with its box centered on `(cx, cy)`.
    pub fn icon_centered(&mut self, cx: f32, cy: f32, glyph: char, size: f32, color: Rgba) {
        let (metrics, bitmap) = self.icon_font.rasterize(glyph, size);
        let gx = cx - metrics.width as f32 / 2.0;
        let gy = cy - metrics.height as f32 / 2.0;
        self.blit_coverage(gx, gy, metrics.width, metrics.height, &bitmap, color);
    }

    fn blit_coverage(&mut self, x: f32, y: f32, w: usize, h: usize, bitmap: &[u8], color: Rgba) {
        let ox = x.round() as i64;
        let oy = y.round() as i64;
        for gy in 0..h {
            let py = oy + gy as i64;
            for gx in 0..w {
                let px = ox + gx as i64;
                let coverage = bitmap[gy * w + gx];
                if coverage == 0 {
                    continue;
                }
                let alpha = (coverage as u32 * color[3] as u32 / 255) as u8;
                self.put(px, py, [color[0], color[1], color[2], alpha]);
            }
        }
    }
}

/// Per-frame input gathered from winit events before the UI runs.
#[derive(Clone, Debug, Default)]
pub struct FrameInput {
    pub mouse_x: f32,
    pub mouse_y: f32,
    /// Left button transitioned to pressed this frame.
    pub mouse_pressed: bool,
    /// Left button is currently held.
    pub mouse_down: bool,
    /// Left button double-clicked this frame.
    pub double_click: bool,
    /// Right button pressed this frame (opens context menus).
    pub right_pressed: bool,
    /// Middle button is currently held (pans the viewport).
    pub middle_down: bool,
    /// Back (mouse 4) pressed this frame.
    pub back_pressed: bool,
    /// Forward (mouse 5) pressed this frame.
    pub forward_pressed: bool,
    /// Accumulated vertical scroll for this frame (positive = scroll up).
    pub scroll: f32,
    /// Printable characters typed this frame.
    pub typed: String,
    pub backspace: bool,
    pub enter: bool,
    pub escape: bool,
    /// The Delete key was pressed (used to remove the selection).
    pub delete: bool,
    /// Ctrl/Cmd shortcut requests for this frame.
    pub copy: bool,
    pub paste: bool,
    pub save: bool,
    pub duplicate: bool,
    pub undo: bool,
    pub redo: bool,
    /// `F` frames the selection; `F2` renames it; `0` resets the view.
    pub focus_selection: bool,
    pub rename: bool,
    pub reset_view: bool,
    /// Arrow-key nudge direction (-1/0/1); `nudge_big` uses the grid step.
    pub nudge_x: f32,
    pub nudge_y: f32,
    pub nudge_big: bool,
}

/// The result of running an editable text field for one frame.
pub struct TextFieldResponse {
    /// The field's text after this frame's edits.
    pub text: String,
    /// The text changed this frame.
    pub changed: bool,
}

/// The immediate-mode context threaded through a frame: a painter plus input,
/// the active theme, and the single piece of retained UI state we need — the
/// focused text field.
pub struct Ui<'a> {
    pub painter: Painter<'a>,
    pub input: FrameInput,
    pub theme: Theme,
    focus: Option<String>,
    edit_buffer: String,
    /// When set, pointer interactions only register inside this rectangle. Used
    /// so widgets scrolled out of a clipped panel cannot be clicked.
    input_clip: Option<Rect>,
    /// Tooltip text to display this frame (set by hovering a tipped widget).
    pending_tooltip: Option<String>,
    /// Set when something wants continuous redraws (editing, dragging).
    pub wants_redraw: bool,
}

impl<'a> Ui<'a> {
    pub fn new(
        painter: Painter<'a>,
        input: FrameInput,
        theme: Theme,
        focus: Option<String>,
        edit_buffer: String,
    ) -> Self {
        Self {
            painter,
            input,
            theme,
            focus,
            edit_buffer,
            input_clip: None,
            pending_tooltip: None,
            wants_redraw: false,
        }
    }

    /// Pull the retained focus state back out at the end of a frame.
    pub fn into_focus_state(self) -> (Option<String>, String) {
        (self.focus, self.edit_buffer)
    }

    pub fn has_focus(&self) -> bool {
        self.focus.is_some()
    }

    /// The current edit buffer (the text of whatever field is/was focused this
    /// frame). Used by modal prompts to read their value reliably.
    pub fn last_edit(&self) -> &str {
        &self.edit_buffer
    }

    /// Restrict pointer interaction to `rect` until [`Ui::reset_input_clip`].
    pub fn set_input_clip(&mut self, rect: Rect) {
        self.input_clip = Some(rect);
    }

    pub fn reset_input_clip(&mut self) {
        self.input_clip = None;
    }

    fn hovered(&self, rect: Rect) -> bool {
        if let Some(clip) = self.input_clip {
            if !clip.contains(self.input.mouse_x, self.input.mouse_y) {
                return false;
            }
        }
        rect.contains(self.input.mouse_x, self.input.mouse_y)
    }

    /// Draw a clickable button. Returns true on the frame it is pressed.
    pub fn button(&mut self, rect: Rect, label: &str) -> bool {
        let base = self.theme.button;
        let text = self.theme.text;
        self.button_colored(rect, label, base, text)
    }

    pub fn button_colored(&mut self, rect: Rect, label: &str, base: Rgba, text_color: Rgba) -> bool {
        let hovered = self.hovered(rect);
        let bg = if hovered {
            lighten(base, 0.12)
        } else {
            base
        };
        let radius = self.theme.corner_radius;
        self.painter.fill_round_rect(rect, radius, bg);
        let size = 14.0;
        let tw = self.painter.text_width(label, size);
        let tx = rect.x + (rect.w - tw) / 2.0;
        let ty = rect.y + (rect.h - size) / 2.0;
        self.painter
            .text_clipped(tx.max(rect.x + 4.0), ty, label, size, text_color, rect.w - 8.0);
        hovered && self.input.mouse_pressed
    }

    /// A button with a leading Material icon and a text label.
    pub fn icon_button(&mut self, rect: Rect, glyph: char, label: &str) -> bool {
        let hovered = self.hovered(rect);
        let base = self.theme.button;
        let bg = if hovered { lighten(base, 0.12) } else { base };
        self.painter.fill_round_rect(rect, self.theme.corner_radius, bg);
        let text_color = self.theme.text;
        let icon_size = 16.0;
        let cy = rect.y + rect.h / 2.0;
        let label_w = self.painter.text_width(label, 14.0);
        // Center the icon+label group as a unit.
        let group_w = icon_size + 4.0 + label_w;
        let start = rect.x + (rect.w - group_w).max(0.0) / 2.0 + 8.0;
        self.painter
            .icon_centered(start, cy, glyph, icon_size, text_color);
        self.painter
            .text(start + icon_size / 2.0 + 4.0, cy - 7.0, label, 14.0, text_color);
        hovered && self.input.mouse_pressed
    }

    /// A compact square button containing only a Material icon. `active`
    /// highlights it (e.g. a toggle that is on).
    pub fn icon_toggle(&mut self, rect: Rect, glyph: char, active: bool, tooltip_color: Rgba) -> bool {
        let hovered = self.hovered(rect);
        let base = if active { self.theme.accent } else { self.theme.button };
        let bg = if hovered { lighten(base, 0.12) } else { base };
        self.painter.fill_round_rect(rect, self.theme.corner_radius, bg);
        let color = if active { [255, 255, 255, 255] } else { tooltip_color };
        self.painter.icon_centered(
            rect.x + rect.w / 2.0,
            rect.y + rect.h / 2.0,
            glyph,
            17.0,
            color,
        );
        hovered && self.input.mouse_pressed
    }

    /// Draw a Material icon at a label position (no interaction).
    pub fn icon(&mut self, cx: f32, cy: f32, glyph: char, size: f32, color: Rgba) {
        self.painter.icon_centered(cx, cy, glyph, size, color);
    }

    /// A collapsing section header with a disclosure triangle. Returns the new
    /// expanded state (toggles when clicked).
    pub fn collapsing_header(&mut self, rect: Rect, label: &str, expanded: bool) -> bool {
        let hovered = self.hovered(rect);
        let bg = if hovered {
            self.theme.panel_alt
        } else {
            self.theme.header
        };
        self.painter.fill_round_rect(rect, 3.0, bg);
        let tri = if expanded { icon::EXPAND_MORE } else { icon::CHEVRON_RIGHT };
        self.painter
            .icon_centered(rect.x + 12.0, rect.y + rect.h / 2.0, tri, 16.0, self.theme.text);
        self.painter.text(
            rect.x + 24.0,
            rect.y + (rect.h - 14.0) / 2.0,
            label,
            14.0,
            self.theme.text,
        );
        if hovered && self.input.mouse_pressed {
            !expanded
        } else {
            expanded
        }
    }

    /// A row in a popup menu. Returns true when clicked. A leading icon is
    /// optional (pass `'\0'` to omit).
    pub fn menu_item(&mut self, rect: Rect, glyph: char, label: &str, danger: bool) -> bool {
        let hovered = self.hovered(rect);
        if hovered {
            self.painter.fill_rect(rect, self.theme.button_active);
        }
        let color = if danger { self.theme.danger } else { self.theme.text };
        let mut tx = rect.x + 10.0;
        if glyph != '\0' {
            self.painter
                .icon_centered(rect.x + 14.0, rect.y + rect.h / 2.0, glyph, 15.0, color);
            tx = rect.x + 28.0;
        }
        self.painter
            .text(tx, rect.y + (rect.h - 14.0) / 2.0, label, 14.0, color);
        hovered && self.input.mouse_pressed
    }

    /// A horizontal slider in `[min, max]`. Returns `Some(new)` when dragged.
    pub fn slider(&mut self, rect: Rect, value: f32, min: f32, max: f32) -> Option<f32> {
        let track = Rect::new(rect.x, rect.y + rect.h / 2.0 - 2.0, rect.w, 4.0);
        self.painter.fill_round_rect(track, 2.0, self.theme.field);
        let t = if (max - min).abs() < f32::EPSILON {
            0.0
        } else {
            ((value - min) / (max - min)).clamp(0.0, 1.0)
        };
        let knob_x = rect.x + t * rect.w;
        self.painter.fill_round_rect(
            Rect::new(rect.x, rect.y + rect.h / 2.0 - 2.0, t * rect.w, 4.0),
            2.0,
            self.theme.accent,
        );
        self.painter
            .fill_round_rect(Rect::new(knob_x - 5.0, rect.y + rect.h / 2.0 - 6.0, 10.0, 12.0), 5.0, self.theme.text);
        if self.hovered(rect) && self.input.mouse_down {
            let nt = ((self.input.mouse_x - rect.x) / rect.w.max(1.0)).clamp(0.0, 1.0);
            self.wants_redraw = true;
            Some(min + nt * (max - min))
        } else {
            None
        }
    }

    /// A clickable color swatch (opens a picker). Returns true when clicked.
    pub fn swatch_button(&mut self, rect: Rect, color: Rgba) -> bool {
        self.painter
            .fill_round_rect(rect, 3.0, [color[0], color[1], color[2], 255]);
        let border = if self.hovered(rect) {
            self.theme.accent
        } else {
            self.theme.border
        };
        self.painter.stroke_round_rect(rect, 3.0, border);
        self.hovered(rect) && self.input.mouse_pressed
    }

    /// Register `text` as the tooltip to show this frame if `rect` is hovered.
    pub fn tooltip(&mut self, rect: Rect, text: &str) {
        if !text.is_empty() && self.hovered(rect) {
            self.pending_tooltip = Some(text.to_string());
        }
    }

    /// Draw the pending tooltip near the cursor. Call once at the very end of a
    /// frame so it sits above all other content.
    pub fn draw_tooltip(&mut self) {
        let Some(text) = self.pending_tooltip.take() else {
            return;
        };
        let size = 13.0;
        let pad = 6.0;
        let tw = self.painter.text_width(&text, size);
        let w = tw + pad * 2.0;
        let h = size + pad * 2.0;
        // Position below-right of the cursor, nudged on-screen.
        let mut x = self.input.mouse_x + 14.0;
        let mut y = self.input.mouse_y + 18.0;
        if x + w > self.painter.width() {
            x = self.painter.width() - w - 2.0;
        }
        if y + h > self.painter.height() {
            y = self.input.mouse_y - h - 6.0;
        }
        let rect = Rect::new(x, y, w, h);
        self.painter.fill_round_rect(rect, 4.0, [20, 20, 22, 240]);
        self.painter.stroke_round_rect(rect, 4.0, self.theme.accent);
        self.painter
            .text(x + pad, y + pad, &text, size, self.theme.text);
    }

    /// A selectable list row. Returns true when clicked.
    pub fn list_row(&mut self, rect: Rect, label: &str, selected: bool, indent: f32) -> bool {
        let hovered = self.hovered(rect);
        let bg = if selected {
            self.theme.button_active
        } else if hovered {
            self.theme.panel_alt
        } else {
            self.theme.panel
        };
        self.painter.fill_rect(rect, bg);
        if selected {
            // Accent strip on the active row, VSCode style.
            self.painter
                .fill_rect(Rect::new(rect.x, rect.y, 2.0, rect.h), self.theme.accent);
        }
        let text_color = if selected { [255, 255, 255, 255] } else { self.theme.text };
        self.painter.text_clipped(
            rect.x + 8.0 + indent,
            rect.y + (rect.h - 14.0) / 2.0,
            label,
            14.0,
            text_color,
            rect.w - 16.0 - indent,
        );
        hovered && self.input.mouse_pressed
    }

    /// Draw a static label.
    pub fn label(&mut self, x: f32, y: f32, text: &str, color: Rgba) {
        self.painter.text(x, y, text, 14.0, color);
    }

    /// A checkbox. Returns `Some(new_value)` when toggled this frame.
    pub fn checkbox(&mut self, rect: Rect, value: bool) -> Option<bool> {
        let hovered = self.hovered(rect);
        let radius = (self.theme.corner_radius - 1.0).max(0.0);
        let bg = if value { self.theme.accent } else { self.theme.field };
        self.painter.fill_round_rect(rect, radius, bg);
        self.painter
            .stroke_round_rect(rect, radius, self.theme.border);
        if value {
            // Simple check mark.
            let cx = rect.x + rect.w * 0.5;
            let cy = rect.y + rect.h * 0.5;
            self.painter.fill_rect(
                Rect::new(cx - 3.0, cy - 1.0, 3.0, 2.0),
                [255, 255, 255, 255],
            );
            self.painter
                .fill_rect(Rect::new(cx - 1.0, cy, 5.0, 2.0), [255, 255, 255, 255]);
        }
        if hovered && self.input.mouse_pressed {
            Some(!value)
        } else {
            None
        }
    }

    /// An editable single-line text field identified by a stable `id`. The
    /// caller passes the current `value`; the response reports the edited text.
    pub fn text_field(&mut self, id: &str, rect: Rect, value: &str) -> TextFieldResponse {
        let focused = self.focus.as_deref() == Some(id);
        let hovered = self.hovered(rect);

        if self.input.mouse_pressed {
            if hovered {
                if !focused {
                    self.focus = Some(id.to_string());
                    self.edit_buffer = value.to_string();
                }
            } else if focused {
                self.focus = None;
            }
        }

        let focused = self.focus.as_deref() == Some(id);
        let mut changed = false;

        if focused {
            self.wants_redraw = true;
            for ch in self.input.typed.chars() {
                if !ch.is_control() {
                    self.edit_buffer.push(ch);
                    changed = true;
                }
            }
            if self.input.backspace {
                self.edit_buffer.pop();
                changed = true;
            }
            if self.input.escape {
                self.focus = None;
            }
            if self.input.enter {
                self.focus = None;
            }
        }

        let display = if focused {
            self.edit_buffer.clone()
        } else {
            value.to_string()
        };

        let radius = (self.theme.corner_radius - 1.0).max(0.0);
        let bg = if focused {
            self.theme.field_focus
        } else {
            self.theme.field
        };
        self.painter.fill_round_rect(rect, radius, bg);
        let border = if focused {
            self.theme.accent
        } else {
            self.theme.border
        };
        self.painter.stroke_round_rect(rect, radius, border);

        let size = 14.0;
        let ty = rect.y + (rect.h - size) / 2.0;
        let text_color = self.theme.text;
        self.painter
            .text_clipped(rect.x + 6.0, ty, &display, size, text_color, rect.w - 12.0);
        if focused {
            let caret_x = (rect.x + 6.0 + self.painter.text_width(&display, size))
                .min(rect.right() - 4.0);
            self.painter
                .fill_rect(Rect::new(caret_x, rect.y + 4.0, 1.0, rect.h - 8.0), text_color);
        }

        TextFieldResponse {
            text: if focused {
                self.edit_buffer.clone()
            } else {
                value.to_string()
            },
            changed,
        }
    }
}

/// Lighten a color toward white by `t` in `[0, 1]`, preserving alpha.
fn lighten(color: Rgba, t: f32) -> Rgba {
    let mix = |c: u8| (c as f32 + (255.0 - c as f32) * t).round().clamp(0.0, 255.0) as u8;
    [mix(color[0]), mix(color[1]), mix(color[2]), color[3]]
}

/// Load the editor's text and icon fonts once. Returns shareable handles so the
/// painter can be rebuilt cheaply each frame.
pub fn load_fonts() -> Result<Fonts, String> {
    let text = Font::from_bytes(EDITOR_FONT_BYTES, fontdue::FontSettings::default())
        .map(Arc::new)
        .map_err(|e| format!("failed to load editor font: {e}"))?;
    let icons = Font::from_bytes(ICON_FONT_BYTES, fontdue::FontSettings::default())
        .map(Arc::new)
        .map_err(|e| format!("failed to load icon font: {e}"))?;
    Ok(Fonts { text, icons })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_contains_is_half_open() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains(0.0, 0.0));
        assert!(r.contains(9.9, 9.9));
        assert!(!r.contains(10.0, 5.0));
        assert!(!r.contains(-0.1, 5.0));
    }

    #[test]
    fn blend_handles_opaque_and_transparent() {
        assert_eq!(blend(0x000000, [255, 255, 255, 255]), 0xffffff);
        assert_eq!(blend(0x123456, [255, 255, 255, 0]), 0x123456);
    }

    #[test]
    fn pack_orders_channels_for_softbuffer() {
        assert_eq!(pack([0x12, 0x34, 0x56, 0xff]), 0x123456);
    }

    #[test]
    fn theme_round_trips_through_json() {
        let theme = Theme::default();
        let json = serde_json::to_string(&theme).expect("serialize theme");
        let restored: Theme = serde_json::from_str(&json).expect("deserialize theme");
        assert_eq!(restored.accent, theme.accent);
        assert_eq!(restored.corner_radius, theme.corner_radius);
    }

    #[test]
    fn theme_fills_missing_fields_with_defaults() {
        // A partial theme file should still parse thanks to `#[serde(default)]`.
        let restored: Theme =
            serde_json::from_str("{\"accent\":[1,2,3,255]}").expect("parse partial theme");
        assert_eq!(restored.accent, [1, 2, 3, 255]);
        assert_eq!(restored.text, Theme::default().text);
    }

    #[test]
    fn clip_blocks_drawing_outside_region() {
        let mut buf = vec![0u32; 16];
        let fonts = load_fonts().expect("load fonts");
        let mut painter = Painter::new(&mut buf, 4, 4, fonts);
        painter.push_clip(Rect::new(0.0, 0.0, 2.0, 2.0));
        painter.fill_rect(Rect::new(0.0, 0.0, 4.0, 4.0), [255, 255, 255, 255]);
        // Inside clip is painted; outside stays zero.
        assert_eq!(buf[0], 0xffffff);
        assert_eq!(buf[3], 0); // top-right, outside clip
        assert_eq!(buf[12], 0); // bottom-left, outside clip
    }
}
