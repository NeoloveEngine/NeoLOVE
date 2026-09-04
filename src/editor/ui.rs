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

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use fontdue::{Font, Metrics};
use serde::{Deserialize, Serialize};

/// Open Sans — a clean, highly legible proportional sans-serif used for all
/// editor chrome (labels, buttons, fields). Pairs naturally with Material Icons.
const EDITOR_FONT_BYTES: &[u8] = include_bytes!("assets/OpenSans-Regular.ttf");

/// Google Material Icons, rendered by codepoint for editor glyphs.
const ICON_FONT_BYTES: &[u8] = include_bytes!("assets/MaterialIcons-Regular.ttf");

/// UI labels reuse a very small set of font sizes and characters. Caching the
/// coverage masks avoids asking `fontdue` to rasterize the same glyph hundreds
/// of times every redraw. Both limits are deliberately conservative: custom
/// fonts and unusual Unicode input cannot grow editor memory without bound.
const GLYPH_CACHE_ENTRY_LIMIT: usize = 1_024;
const GLYPH_CACHE_BYTE_LIMIT: usize = 8 * 1024 * 1024;

/// Named Google Material Icons codepoints used across the editor.
pub mod icon {
    pub const ADD: char = '\u{e145}';
    pub const SAVE: char = '\u{e161}';
    pub const FOLDER_OPEN: char = '\u{e2c8}';
    pub const DELETE: char = '\u{e872}';
    pub const PLAY: char = '\u{e037}';
    pub const PAUSE: char = '\u{e034}';
    pub const STOP: char = '\u{e047}';
    pub const SKIP_NEXT: char = '\u{e044}';
    pub const REPLAY: char = '\u{e042}';
    pub const CODE: char = '\u{e86f}';
    pub const GRID_ON: char = '\u{e3ec}';
    pub const GRID_OFF: char = '\u{e3eb}';
    pub const CROP_SQUARE: char = '\u{e3c6}';
    pub const TITLE: char = '\u{e264}';
    pub const TEXT_FIELDS: char = '\u{e262}';
    pub const NUMBERS: char = '\u{eac7}';
    pub const CHECK_BOX: char = '\u{e834}';
    pub const EXTENSION: char = '\u{e87b}';
    pub const FORMAT_LIST_BULLETED: char = '\u{e241}';
    pub const TABLE_ROWS: char = '\u{f101}';
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
    pub const ARROW_DOWNWARD: char = '\u{e5db}';
    pub const INSERT_DRIVE_FILE: char = '\u{e24d}';
    pub const AUDIOTRACK: char = '\u{e3a1}';
    pub const FONT_DOWNLOAD: char = '\u{e167}';
    pub const ARTICLE: char = '\u{ef42}';
    pub const PALETTE: char = '\u{e40a}';
    pub const SEARCH: char = '\u{e8b6}';
    pub const HISTORY: char = '\u{e889}';
    pub const VISIBILITY: char = '\u{e8f4}';
    pub const VISIBILITY_OFF: char = '\u{e8f5}';
    pub const RESTART_ALT: char = '\u{f053}';
    pub const CENTER_FOCUS: char = '\u{e3b4}';
    pub const MY_LOCATION: char = '\u{e55c}';
    /// Used for the "swap dock side" affordance on a panel header.
    pub const SWAP: char = '\u{e8f1}';
    pub const CHECK: char = '\u{e5ca}';
    pub const MORE_VERT: char = '\u{e5d4}';
    pub const LOCK: char = '\u{e897}';
    pub const LOCK_OPEN: char = '\u{e898}';
    pub const UNFOLD_MORE: char = '\u{e5d7}';
    pub const UNFOLD_LESS: char = '\u{e5d6}';
    pub const FULLSCREEN: char = '\u{e5d0}';
    pub const FULLSCREEN_EXIT: char = '\u{e5d1}';
    pub const SELECT_ALL: char = '\u{e162}';
    pub const ZOOM_OUT_MAP: char = '\u{e56b}';
    pub const OPEN_WITH: char = '\u{e89f}';
    pub const ASPECT_RATIO: char = '\u{e85b}';
    pub const ROTATE_RIGHT: char = '\u{e419}';
    pub const TRANSFORM: char = '\u{e428}';
    pub const PHONE_ANDROID: char = '\u{e324}';
    pub const VIDEOCAM: char = '\u{e04b}';
    pub const SCREEN_ROTATION: char = '\u{e1c1}';
    pub const CLOSE: char = '\u{e5cd}';
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
            panel: [37, 37, 38, 255],          // #252526 side bar
            panel_alt: [45, 45, 45, 255],      // #2d2d2d
            toolbar: [60, 60, 60, 255],        // #3c3c3c title bar
            viewport_bg: [30, 30, 30, 255],    // #1e1e1e editor
            border: [69, 69, 69, 255],         // #454545
            text: [212, 212, 212, 255],        // #d4d4d4
            text_dim: [133, 133, 133, 255],    // #858585
            button: [14, 99, 156, 255],        // #0e639c
            button_hover: [17, 119, 187, 255], // #1177bb
            button_active: [9, 71, 113, 255],  // #094771 selection
            field: [60, 60, 60, 255],          // #3c3c3c input
            field_focus: [45, 45, 45, 255],
            accent: [0, 122, 204, 255],     // #007acc
            selection: [255, 199, 89, 255], // viewport gizmo amber
            danger: [241, 76, 76, 255],     // #f14c4c
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

/// Twice the signed area of triangle `(a, b, c)`. Used as an edge function for
/// point-in-triangle tests.
fn edge(a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> f32 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
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
    glyph_cache: Arc<Mutex<GlyphRasterCache>>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum FontFace {
    Text,
    Icons,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct GlyphCacheKey {
    face: FontFace,
    character: char,
    size_bits: u32,
}

#[derive(Debug)]
struct CachedGlyph {
    metrics: Metrics,
    bitmap: Box<[u8]>,
}

#[derive(Debug)]
struct GlyphCacheEntry {
    glyph: Arc<CachedGlyph>,
    last_used: u64,
}

#[derive(Debug)]
struct GlyphRasterCache {
    entries: HashMap<GlyphCacheKey, GlyphCacheEntry>,
    bitmap_bytes: usize,
    clock: u64,
    entry_limit: usize,
    byte_limit: usize,
}

impl GlyphRasterCache {
    fn new(entry_limit: usize, byte_limit: usize) -> Self {
        Self {
            entries: HashMap::new(),
            bitmap_bytes: 0,
            clock: 0,
            entry_limit,
            byte_limit,
        }
    }

    fn get(&mut self, key: GlyphCacheKey) -> Option<Arc<CachedGlyph>> {
        self.clock = self.clock.wrapping_add(1);
        let entry = self.entries.get_mut(&key)?;
        entry.last_used = self.clock;
        Some(Arc::clone(&entry.glyph))
    }

    fn insert(&mut self, key: GlyphCacheKey, glyph: Arc<CachedGlyph>) {
        let bytes = glyph.bitmap.len();
        if self.entry_limit == 0 || bytes > self.byte_limit {
            return;
        }
        if let Some(existing) = self.entries.remove(&key) {
            self.bitmap_bytes = self
                .bitmap_bytes
                .saturating_sub(existing.glyph.bitmap.len());
        }
        while !self.entries.is_empty()
            && (self.entries.len() >= self.entry_limit
                || self.bitmap_bytes.saturating_add(bytes) > self.byte_limit)
        {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| *key)
            else {
                break;
            };
            if let Some(removed) = self.entries.remove(&oldest) {
                self.bitmap_bytes = self.bitmap_bytes.saturating_sub(removed.glyph.bitmap.len());
            }
        }
        self.clock = self.clock.wrapping_add(1);
        self.bitmap_bytes = self.bitmap_bytes.saturating_add(bytes);
        self.entries.insert(
            key,
            GlyphCacheEntry {
                glyph,
                last_used: self.clock,
            },
        );
    }
}

impl Fonts {
    fn rasterize_cached(&self, face: FontFace, character: char, size: f32) -> Arc<CachedGlyph> {
        let key = GlyphCacheKey {
            face,
            character,
            size_bits: size.to_bits(),
        };
        if let Ok(mut cache) = self.glyph_cache.lock()
            && let Some(glyph) = cache.get(key)
        {
            return glyph;
        }

        // Rasterize outside the lock so separate editor windows never block
        // each other on the expensive part of a cache miss.
        let font = match face {
            FontFace::Text => &self.text,
            FontFace::Icons => &self.icons,
        };
        let (metrics, bitmap) = font.rasterize(character, size);
        let glyph = Arc::new(CachedGlyph {
            metrics,
            bitmap: bitmap.into_boxed_slice(),
        });
        if let Ok(mut cache) = self.glyph_cache.lock() {
            // Another window may have populated the same key meanwhile. Reuse
            // that allocation and discard this duplicate if so.
            if let Some(existing) = cache.get(key) {
                return existing;
            }
            cache.insert(key, Arc::clone(&glyph));
        }
        glyph
    }
}

/// An active rotation applied to subsequent drawing: rotate by `angle`
/// (encoded as `sin`/`cos`) about the screen-space pivot `(px, py)`.
#[derive(Clone, Copy)]
pub struct RotXform {
    px: f32,
    py: f32,
    sin: f32,
    cos: f32,
}

/// Draws shapes and text into a borrowed framebuffer with an optional clip.
pub struct Painter<'a> {
    buffer: &'a mut [u32],
    width: usize,
    height: usize,
    font: Arc<Font>,
    fonts: Fonts,
    /// Clip bounds in pixels: `[x0, y0, x1, y1)`.
    clip: [i64; 4],
    /// When set, fills and images are rasterized rotated about a pivot so the
    /// editor preview can mirror the runtime's per-entity rotation.
    rot: Option<RotXform>,
}

impl<'a> Painter<'a> {
    pub fn new(buffer: &'a mut [u32], width: usize, height: usize, fonts: Fonts) -> Self {
        Self {
            buffer,
            width,
            height,
            font: Arc::clone(&fonts.text),
            fonts,
            clip: [0, 0, width as i64, height as i64],
            rot: None,
        }
    }

    /// Rotate subsequent drawing by `angle` radians about the screen point
    /// `(px, py)`. Returns the previous transform to restore afterwards with
    /// [`Painter::set_rotation_raw`]. A near-zero angle clears rotation so the
    /// fast axis-aligned paths stay in use.
    pub fn push_rotation(&mut self, px: f32, py: f32, angle: f32) -> Option<RotXform> {
        let prev = self.rot;
        self.rot = if angle.abs() < 1e-4 {
            None
        } else {
            Some(RotXform {
                px,
                py,
                sin: angle.sin(),
                cos: angle.cos(),
            })
        };
        prev
    }

    pub fn set_rotation_raw(&mut self, rot: Option<RotXform>) {
        self.rot = rot;
    }

    /// Axis-aligned screen bounds of `rect` after the active rotation, or
    /// `rect` unchanged when there is no rotation. Used to widen clips so they
    /// don't crop rotated content.
    pub fn rotated_bounds(&self, rect: Rect) -> Rect {
        let Some(rot) = self.rot else {
            return rect;
        };
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
            let dx = x - rot.px;
            let dy = y - rot.py;
            let sx = rot.px + dx * rot.cos - dy * rot.sin;
            let sy = rot.py + dx * rot.sin + dy * rot.cos;
            min_x = min_x.min(sx);
            min_y = min_y.min(sy);
            max_x = max_x.max(sx);
            max_y = max_y.max(sy);
        }
        Rect::new(min_x, min_y, max_x - min_x, max_y - min_y)
    }

    /// Rasterize a rotated quad: walk the screen pixels covering `bounds`
    /// (the rotated extent of `rect`), inverse-rotate each back into `rect`'s
    /// unrotated space, and blend whatever color `sample` returns there. Only
    /// meaningful while `self.rot` is `Some`.
    fn rasterize_rotated(
        &mut self,
        bounds: Rect,
        mut sample: impl FnMut(f32, f32) -> Option<Rgba>,
    ) {
        let Some(rot) = self.rot else {
            return;
        };
        let x0 = (bounds.x.floor() as i64).max(self.clip[0]);
        let y0 = (bounds.y.floor() as i64).max(self.clip[1]);
        let x1 = (bounds.right().ceil() as i64).min(self.clip[2]);
        let y1 = (bounds.bottom().ceil() as i64).min(self.clip[3]);
        for py in y0..y1 {
            for px in x0..x1 {
                let dx = px as f32 + 0.5 - rot.px;
                let dy = py as f32 + 0.5 - rot.py;
                // Inverse rotation (by -angle) back into unrotated space.
                let lx = rot.px + dx * rot.cos + dy * rot.sin;
                let ly = rot.py - dx * rot.sin + dy * rot.cos;
                if let Some(color) = sample(lx, ly) {
                    self.put(px, py, color);
                }
            }
        }
    }

    /// Rasterize a shape whose unrotated extent is `bounds`, evaluating
    /// `sample` at each pixel centre (in unrotated space). Honors the active
    /// rotation transparently, so callers describe shapes once and get rotation
    /// for free.
    fn rasterize_shape(&mut self, bounds: Rect, mut sample: impl FnMut(f32, f32) -> Option<Rgba>) {
        if self.rot.is_some() {
            self.rasterize_rotated(bounds, sample);
            return;
        }
        let x0 = (bounds.x.floor() as i64).max(self.clip[0]);
        let y0 = (bounds.y.floor() as i64).max(self.clip[1]);
        let x1 = (bounds.right().ceil() as i64).min(self.clip[2]);
        let y1 = (bounds.bottom().ceil() as i64).min(self.clip[3]);
        for py in y0..y1 {
            for px in x0..x1 {
                if let Some(color) = sample(px as f32 + 0.5, py as f32 + 0.5) {
                    self.put(px, py, color);
                }
            }
        }
    }

    /// Fill an antialiased circle centred on `(cx, cy)`.
    pub fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, color: Rgba) {
        if color[3] == 0 || radius <= 0.0 {
            return;
        }
        let pad = radius + 1.0;
        let bounds = Rect::new(cx - pad, cy - pad, pad * 2.0, pad * 2.0);
        self.rasterize_shape(bounds, |x, y| {
            let dist = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
            let coverage = (radius + 0.5 - dist).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                None
            } else {
                Some([
                    color[0],
                    color[1],
                    color[2],
                    (color[3] as f32 * coverage) as u8,
                ])
            }
        });
    }

    /// Fill a triangle given its three (screen-space) vertices.
    pub fn fill_triangle(
        &mut self,
        mut p0: (f32, f32),
        mut p1: (f32, f32),
        mut p2: (f32, f32),
        color: Rgba,
    ) {
        if color[3] == 0
            || ![p0.0, p0.1, p1.0, p1.1, p2.0, p2.1]
                .iter()
                .all(|value| value.is_finite())
        {
            return;
        }

        if let Some(rot) = self.rot {
            let transform = |point: (f32, f32)| {
                let dx = point.0 - rot.px;
                let dy = point.1 - rot.py;
                (
                    rot.px + dx * rot.cos - dy * rot.sin,
                    rot.py + dx * rot.sin + dy * rot.cos,
                )
            };
            p0 = transform(p0);
            p1 = transform(p1);
            p2 = transform(p2);
        }

        let area = edge(p0, p1, p2);
        if area.abs() < 1e-6 {
            return;
        }
        let sign = area.signum();
        let x0 = (p0.0.min(p1.0).min(p2.0).floor() as i64).max(self.clip[0]);
        let x1 = (p0.0.max(p1.0).max(p2.0).ceil() as i64).min(self.clip[2]);
        let y0 = (p0.1.min(p1.1).min(p2.1).floor() as i64).max(self.clip[1]);
        let y1 = (p0.1.max(p1.1).max(p2.1).ceil() as i64).min(self.clip[3]);
        if x1 <= x0 || y1 <= y0 {
            return;
        }

        // Edge functions are affine, so advance them with additions instead
        // of recomputing three cross products for every framebuffer pixel.
        // Large projected quads commonly cover the whole 3D viewport; this
        // keeps their raster cost close to the unavoidable number of writes.
        let sample = (x0 as f32 + 0.5, y0 as f32 + 0.5);
        let row_w0 = edge(p1, p2, sample) * sign;
        let row_w1 = edge(p2, p0, sample) * sign;
        let row_w2 = edge(p0, p1, sample) * sign;
        let x_step0 = -(p2.1 - p1.1) * sign;
        let x_step1 = -(p0.1 - p2.1) * sign;
        let x_step2 = -(p1.1 - p0.1) * sign;
        let y_step0 = (p2.0 - p1.0) * sign;
        let y_step1 = (p0.0 - p2.0) * sign;
        let y_step2 = (p1.0 - p0.0) * sign;
        let opaque = color[3] == 255;
        let packed = pack(color);

        for y in y0..y1 {
            let row_offset = y as usize * self.width;
            let row_delta = (y - y0) as f32;
            let mut w0 = row_w0 + y_step0 * row_delta;
            let mut w1 = row_w1 + y_step1 * row_delta;
            let mut w2 = row_w2 + y_step2 * row_delta;
            for x in x0..x1 {
                if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                    let pixel = &mut self.buffer[row_offset + x as usize];
                    if opaque {
                        *pixel = packed;
                    } else {
                        *pixel = blend(*pixel, color);
                    }
                }
                w0 += x_step0;
                w1 += x_step1;
                w2 += x_step2;
            }
        }
    }

    /// Fill a screen-space triangle using the same zero-to-one depth convention
    /// as the 3D renderers. `depth_buffer` is framebuffer-sized and is shared by
    /// every triangle in one Scene View pass, so intersecting/adjacent faces are
    /// resolved per pixel instead of by an unreliable average triangle depth.
    pub fn fill_triangle_depth_tested(
        &mut self,
        points: [(f32, f32); 3],
        depths: [f32; 3],
        color: Rgba,
        depth_buffer: &mut [f32],
    ) {
        if color[3] == 0
            || depth_buffer.len() != self.width.saturating_mul(self.height)
            || points
                .iter()
                .flat_map(|point| [point.0, point.1])
                .chain(depths)
                .any(|value| !value.is_finite())
        {
            return;
        }

        let [p0, p1, p2] = points;
        let area = edge(p0, p1, p2);
        if area.abs() < 1e-6 {
            return;
        }
        let sign = area.signum();
        let inverse_area = area.abs().recip();
        let x0 = (p0.0.min(p1.0).min(p2.0).floor() as i64).max(self.clip[0]);
        let x1 = (p0.0.max(p1.0).max(p2.0).ceil() as i64).min(self.clip[2]);
        let y0 = (p0.1.min(p1.1).min(p2.1).floor() as i64).max(self.clip[1]);
        let y1 = (p0.1.max(p1.1).max(p2.1).ceil() as i64).min(self.clip[3]);
        if x1 <= x0 || y1 <= y0 {
            return;
        }

        let sample = (x0 as f32 + 0.5, y0 as f32 + 0.5);
        let row_w0 = edge(p1, p2, sample) * sign;
        let row_w1 = edge(p2, p0, sample) * sign;
        let row_w2 = edge(p0, p1, sample) * sign;
        let x_step0 = -(p2.1 - p1.1) * sign;
        let x_step1 = -(p0.1 - p2.1) * sign;
        let x_step2 = -(p1.1 - p0.1) * sign;
        let y_step0 = (p2.0 - p1.0) * sign;
        let y_step1 = (p0.0 - p2.0) * sign;
        let y_step2 = (p1.0 - p0.0) * sign;
        let opaque = color[3] == 255;
        let packed = pack(color);

        for y in y0..y1 {
            let row_offset = y as usize * self.width;
            let row_delta = (y - y0) as f32;
            let mut w0 = row_w0 + y_step0 * row_delta;
            let mut w1 = row_w1 + y_step1 * row_delta;
            let mut w2 = row_w2 + y_step2 * row_delta;
            for x in x0..x1 {
                if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                    let barycentric = [
                        w0 * inverse_area,
                        w1 * inverse_area,
                        w2 * inverse_area,
                    ];
                    let depth = depths[0] * barycentric[0]
                        + depths[1] * barycentric[1]
                        + depths[2] * barycentric[2];
                    let pixel_index = row_offset + x as usize;
                    if (0.0..=1.0).contains(&depth) && depth < depth_buffer[pixel_index] {
                        depth_buffer[pixel_index] = depth;
                        let pixel = &mut self.buffer[pixel_index];
                        if opaque {
                            *pixel = packed;
                        } else {
                            *pixel = blend(*pixel, color);
                        }
                    }
                }
                w0 += x_step0;
                w1 += x_step1;
                w2 += x_step2;
            }
        }
    }

    /// Stroke an antialiased one-pixel line segment.
    pub fn stroke_line(&mut self, mut x0: f32, mut y0: f32, mut x1: f32, mut y1: f32, color: Rgba) {
        if color[3] == 0 || ![x0, y0, x1, y1].iter().all(|value| value.is_finite()) {
            return;
        }

        // A line remains a line under the painter's rigid transform. Applying
        // it to the two endpoints up front lets the fast screen-space walker
        // below replace the old rotated-bounding-box scan as well.
        if let Some(rot) = self.rot {
            let transform = |x: f32, y: f32| {
                let dx = x - rot.px;
                let dy = y - rot.py;
                (
                    rot.px + dx * rot.cos - dy * rot.sin,
                    rot.py + dx * rot.sin + dy * rot.cos,
                )
            };
            (x0, y0) = transform(x0, y0);
            (x1, y1) = transform(x1, y1);
        }

        let dx = x1 - x0;
        let dy = y1 - y0;
        let len_sq = dx * dx + dy * dy;
        if len_sq <= 1e-6 {
            let previous = self.rot.take();
            self.fill_circle(x0, y0, 0.75, color);
            self.rot = previous;
            return;
        }

        // Only visit a narrow strip around the segment. The previous generic
        // shape rasterizer visited every pixel in a line's axis-aligned bounds;
        // a viewport-sized diagonal therefore did O(width * height) work. This
        // path is O(max(width, height)) while retaining the same exact
        // distance-to-segment antialiasing at each candidate pixel.
        let clip = self.clip;
        let mut plot = |x: i64, y: i64| {
            if x < clip[0] || x >= clip[2] || y < clip[1] || y >= clip[3] {
                return;
            }
            let sample_x = x as f32 + 0.5;
            let sample_y = y as f32 + 0.5;
            let t = (((sample_x - x0) * dx + (sample_y - y0) * dy) / len_sq).clamp(0.0, 1.0);
            let px = x0 + dx * t;
            let py = y0 + dy * t;
            let dist = ((sample_x - px).powi(2) + (sample_y - py).powi(2)).sqrt();
            let coverage = (1.0 - dist).clamp(0.0, 1.0);
            if coverage > 0.0 {
                self.put(
                    x,
                    y,
                    [
                        color[0],
                        color[1],
                        color[2],
                        (color[3] as f32 * coverage) as u8,
                    ],
                );
            }
        };

        if dx.abs() >= dy.abs() {
            let start = ((x0.min(x1) - 1.0).floor() as i64).max(clip[0]);
            let end = ((x0.max(x1) + 1.0).ceil() as i64).min(clip[2] - 1);
            for x in start..=end {
                let t = ((x as f32 + 0.5 - x0) / dx).clamp(0.0, 1.0);
                let centre = (y0 + dy * t - 0.5).floor() as i64;
                for y in centre - 1..=centre + 1 {
                    plot(x, y);
                }
            }
        } else {
            let start = ((y0.min(y1) - 1.0).floor() as i64).max(clip[1]);
            let end = ((y0.max(y1) + 1.0).ceil() as i64).min(clip[3] - 1);
            for y in start..=end {
                let t = ((y as f32 + 0.5 - y0) / dy).clamp(0.0, 1.0);
                let centre = (x0 + dx * t - 0.5).floor() as i64;
                for x in centre - 1..=centre + 1 {
                    plot(x, y);
                }
            }
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

    /// Plot a single pixel (used by gradient widgets like the color picker).
    pub fn pixel(&mut self, x: f32, y: f32, color: Rgba) {
        self.put(x.floor() as i64, y.floor() as i64, color);
    }

    /// Multiply the scene already rasterized within `rect` by a per-pixel light
    /// color, mirroring the runtime's deferred light composite: `scene × light`
    /// (`light` already scaled by exposure), plus bloom on over-bright light,
    /// clamped to `[0, 1]`. `sample` receives pixel centres in screen space and
    /// returns the light multiplier `(r, g, b)`. The editor's lighting preview
    /// uses this so lights reveal the scene's true colors — background, gradients
    /// across objects, and occluder shadows alike — exactly as the game will,
    /// rather than laying a flat tint on each object.
    ///
    /// The composite covers every viewport pixel regardless of light count, so
    /// it is split across worker threads (like the runtime's own composite) to
    /// keep pans and zooms responsive.
    pub fn composite_light(
        &mut self,
        rect: Rect,
        bloom: f32,
        sample: impl Fn(f32, f32) -> (f32, f32, f32) + Sync,
    ) {
        let width = self.width;
        let height = self.height;
        let x0 = (rect.x.floor() as i64).max(self.clip[0]).max(0);
        let y0 = (rect.y.floor() as i64).max(self.clip[1]).max(0);
        let x1 = (rect.right().ceil() as i64)
            .min(self.clip[2])
            .min(width as i64);
        let y1 = (rect.bottom().ceil() as i64)
            .min(self.clip[3])
            .min(height as i64);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let (x0, y0, x1, y1) = (x0 as usize, y0 as usize, x1 as usize, y1 as usize);

        let sample = &sample;
        // Composite one band of framebuffer rows (`row_start` is the global row
        // index of the band's first row). Rows/columns outside the lit rect are
        // skipped so only the viewport is touched.
        let composite_band = move |chunk: &mut [u32], row_start: usize| {
            let rows = chunk.len() / width;
            for local in 0..rows {
                let py = row_start + local;
                if py < y0 || py >= y1 {
                    continue;
                }
                let base = local * width;
                for px in x0..x1 {
                    let (lr, lg, lb) = sample(px as f32 + 0.5, py as f32 + 0.5);
                    let idx = base + px;
                    let dst = chunk[idx];
                    let sr = ((dst >> 16) & 0xff) as f32 / 255.0;
                    let sg = ((dst >> 8) & 0xff) as f32 / 255.0;
                    let sb = (dst & 0xff) as f32 / 255.0;
                    let mut or = sr * lr;
                    let mut og = sg * lg;
                    let mut ob = sb * lb;
                    if bloom > 0.0 {
                        or += (lr - 1.0).max(0.0) * bloom * sr;
                        og += (lg - 1.0).max(0.0) * bloom * sg;
                        ob += (lb - 1.0).max(0.0) * bloom * sb;
                    }
                    let r = (or.clamp(0.0, 1.0) * 255.0).round() as u32;
                    let g = (og.clamp(0.0, 1.0) * 255.0).round() as u32;
                    let b = (ob.clamp(0.0, 1.0) * 255.0).round() as u32;
                    chunk[idx] = b | (g << 8) | (r << 16);
                }
            }
        };

        let buffer: &mut [u32] = &mut *self.buffer;
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .clamp(1, 16)
            .min((y1 - y0).max(1));
        if workers <= 1 || (x1 - x0) * (y1 - y0) < 16_384 {
            composite_band(buffer, 0);
        } else {
            let rows_per = height.div_ceil(workers).max(1);
            let band = rows_per * width;
            let composite_band = &composite_band;
            std::thread::scope(|scope| {
                for (index, chunk) in buffer.chunks_mut(band).enumerate() {
                    scope.spawn(move || composite_band(chunk, index * rows_per));
                }
            });
        }
    }

    /// Draw an image into `dest`, sampling the optional `src` sub-rectangle
    /// (in image pixels) with nearest-neighbour, multiplied by `tint`. Clipped
    /// and alpha-blended. Used for accurate sprite/9-slice/tile previews.
    pub fn draw_image(
        &mut self,
        img: &image::RgbaImage,
        dest: Rect,
        src: Option<Rect>,
        tint: Rgba,
    ) {
        let (iw, ih) = (img.width() as f32, img.height() as f32);
        if iw <= 0.0 || ih <= 0.0 || dest.w <= 0.0 || dest.h <= 0.0 {
            return;
        }
        let src = src.unwrap_or(Rect::new(0.0, 0.0, iw, ih));
        if src.w <= 0.0 || src.h <= 0.0 {
            return;
        }
        if self.rot.is_some() {
            let bounds = self.rotated_bounds(dest);
            self.rasterize_rotated(bounds, |x, y| {
                if x < dest.x || x >= dest.right() || y < dest.y || y >= dest.bottom() {
                    return None;
                }
                let u = (x - dest.x) / dest.w;
                let v = (y - dest.y) / dest.h;
                let sx = (src.x + u * src.w).clamp(0.0, iw - 1.0) as u32;
                let sy = (src.y + v * src.h).clamp(0.0, ih - 1.0) as u32;
                let p = img.get_pixel(sx, sy).0;
                let a = (p[3] as u32 * tint[3] as u32 / 255) as u8;
                if a == 0 {
                    return None;
                }
                Some([
                    (p[0] as u32 * tint[0] as u32 / 255) as u8,
                    (p[1] as u32 * tint[1] as u32 / 255) as u8,
                    (p[2] as u32 * tint[2] as u32 / 255) as u8,
                    a,
                ])
            });
            return;
        }
        let x0 = (dest.x.floor() as i64).max(self.clip[0]);
        let y0 = (dest.y.floor() as i64).max(self.clip[1]);
        let x1 = (dest.right().ceil() as i64).min(self.clip[2]);
        let y1 = (dest.bottom().ceil() as i64).min(self.clip[3]);
        for py in y0..y1 {
            let v = (py as f32 + 0.5 - dest.y) / dest.h;
            let sy = (src.y + v * src.h).clamp(0.0, ih - 1.0) as u32;
            for px in x0..x1 {
                let u = (px as f32 + 0.5 - dest.x) / dest.w;
                let sx = (src.x + u * src.w).clamp(0.0, iw - 1.0) as u32;
                let p = img.get_pixel(sx, sy).0;
                let a = (p[3] as u32 * tint[3] as u32 / 255) as u8;
                if a == 0 {
                    continue;
                }
                let c = [
                    (p[0] as u32 * tint[0] as u32 / 255) as u8,
                    (p[1] as u32 * tint[1] as u32 / 255) as u8,
                    (p[2] as u32 * tint[2] as u32 / 255) as u8,
                    a,
                ];
                if a == 255 {
                    self.buffer[py as usize * self.width + px as usize] = pack(c);
                } else {
                    self.put(px, py, c);
                }
            }
        }
    }

    /// Fill a rectangle, clipped and alpha-blended.
    pub fn fill_rect(&mut self, rect: Rect, color: Rgba) {
        if color[3] == 0 {
            return;
        }
        if self.rot.is_some() {
            let bounds = self.rotated_bounds(rect);
            self.rasterize_rotated(bounds, |x, y| {
                if x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom() {
                    Some(color)
                } else {
                    None
                }
            });
            return;
        }
        let x0 = (rect.x.floor() as i64).max(self.clip[0]);
        let y0 = (rect.y.floor() as i64).max(self.clip[1]);
        let x1 = (rect.right().ceil() as i64).min(self.clip[2]);
        let y1 = (rect.bottom().ceil() as i64).min(self.clip[3]);
        // Empty and reversed rectangles used to be harmless because the
        // per-pixel ranges below simply had no iterations.  Keep that
        // behaviour before constructing an opaque row slice, whose bounds
        // must be ordered.
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        if color[3] == 255 {
            let packed = pack(color);
            for y in y0..y1 {
                let row = y as usize * self.width;
                self.buffer[row + x0 as usize..row + x1 as usize].fill(packed);
            }
            return;
        }
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
        if self.rot.is_some() {
            let bounds = self.rotated_bounds(rect);
            self.rasterize_rotated(bounds, |x, y| {
                let coverage = (0.5 - round_rect_sdf(rect, radius, x, y)).clamp(0.0, 1.0);
                if coverage <= 0.0 {
                    None
                } else {
                    Some([
                        color[0],
                        color[1],
                        color[2],
                        (color[3] as f32 * coverage) as u8,
                    ])
                }
            });
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
        if self.rot.is_some() {
            // Grow the sampled extent by a pixel so the antialiased outer band
            // of the outline isn't clipped at the rotated rect edge.
            let outer = Rect::new(rect.x - 1.0, rect.y - 1.0, rect.w + 2.0, rect.h + 2.0);
            let bounds = self.rotated_bounds(outer);
            self.rasterize_rotated(bounds, |x, y| {
                let coverage = (1.0 - round_rect_sdf(rect, radius, x, y).abs()).clamp(0.0, 1.0);
                if coverage <= 0.0 {
                    None
                } else {
                    Some([
                        color[0],
                        color[1],
                        color[2],
                        (color[3] as f32 * coverage) as u8,
                    ])
                }
            });
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
            let glyph = self.fonts.rasterize_cached(FontFace::Text, ch, size);
            let metrics = glyph.metrics;
            let gx = pen + metrics.xmin as f32;
            let gy = baseline - (metrics.height as f32 + metrics.ymin as f32);
            self.blit_coverage(gx, gy, metrics.width, metrics.height, &glyph.bitmap, color);
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
        if !max_width.is_finite() || max_width <= 0.0 {
            return;
        }
        // Advance widths do not include every glyph's raster overhang. Keep a
        // real clip in addition to choosing a fitting string so italics,
        // custom fonts and the ellipsis can never bleed into adjacent widgets.
        let previous_clip = self.push_clip(Rect::new(x, 0.0, max_width, self.height as f32));
        if self.text_width(text, size) <= max_width {
            self.text(x, y, text, size, color);
            self.set_clip_raw(previous_clip);
            return;
        }
        let ell_w = self.text_width("…", size);
        if ell_w > max_width {
            self.set_clip_raw(previous_clip);
            return;
        }
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
        self.set_clip_raw(previous_clip);
    }

    /// Draw text word-wrapped inside `rect`, hard-breaking a single overlong
    /// word and adding an ellipsis when the available height is exhausted.
    pub fn text_wrapped(
        &mut self,
        rect: Rect,
        text: &str,
        size: f32,
        line_height: f32,
        color: Rgba,
    ) {
        if rect.w <= 0.0 || rect.h <= 0.0 || line_height <= 0.0 {
            return;
        }
        let max_lines = (rect.h / line_height).floor() as usize;
        if max_lines == 0 {
            return;
        }
        let mut lines = wrap_text_lines(self, text, size, rect.w);
        let vertically_truncated = lines.len() > max_lines;
        lines.truncate(max_lines);
        if vertically_truncated {
            if let Some(last) = lines.last_mut() {
                last.push('…');
            }
        }

        let previous_clip = self.push_clip(rect);
        for (index, line) in lines.iter().enumerate() {
            self.text_clipped(
                rect.x,
                rect.y + index as f32 * line_height,
                line,
                size,
                color,
                rect.w,
            );
        }
        self.set_clip_raw(previous_clip);
    }

    /// Draw a Material Icons glyph, with its box centered on `(cx, cy)`.
    pub fn icon_centered(&mut self, cx: f32, cy: f32, glyph: char, size: f32, color: Rgba) {
        let glyph = self.fonts.rasterize_cached(FontFace::Icons, glyph, size);
        let metrics = glyph.metrics;
        let gx = cx - metrics.width as f32 / 2.0;
        let gy = cy - metrics.height as f32 / 2.0;
        self.blit_coverage(gx, gy, metrics.width, metrics.height, &glyph.bitmap, color);
    }

    fn blit_coverage(&mut self, x: f32, y: f32, w: usize, h: usize, bitmap: &[u8], color: Rgba) {
        if w == 0 || h == 0 {
            return;
        }
        if self.rot.is_some() {
            // Sample the glyph's coverage mask in its own (unrotated) space so
            // text rotates along with the entity it belongs to.
            let glyph = Rect::new(x, y, w as f32, h as f32);
            let bounds = self.rotated_bounds(glyph);
            self.rasterize_rotated(bounds, |lx, ly| {
                let gx = (lx - x).floor() as i64;
                let gy = (ly - y).floor() as i64;
                if gx < 0 || gy < 0 || gx >= w as i64 || gy >= h as i64 {
                    return None;
                }
                let coverage = bitmap[gy as usize * w + gx as usize];
                if coverage == 0 {
                    return None;
                }
                Some([
                    color[0],
                    color[1],
                    color[2],
                    (coverage as u32 * color[3] as u32 / 255) as u8,
                ])
            });
            return;
        }
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

/// Wrap text to measured pixel width. Whitespace between words is normalized
/// for display, while explicit line breaks are preserved. A word wider than
/// the available width is split at Unicode scalar boundaries.
fn wrap_text_lines(painter: &Painter<'_>, text: &str, size: f32, max_width: f32) -> Vec<String> {
    if max_width <= 0.0 || !max_width.is_finite() {
        return Vec::new();
    }

    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            lines.push(String::new());
            continue;
        }

        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            if painter.text_width(&candidate, size) <= max_width {
                current = candidate;
                continue;
            }

            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            if painter.text_width(word, size) <= max_width {
                current.push_str(word);
                continue;
            }

            // Hard-break a path, identifier, or other unspaced token.
            let mut part = String::new();
            for character in word.chars() {
                let mut next = part.clone();
                next.push(character);
                if !part.is_empty() && painter.text_width(&next, size) > max_width {
                    lines.push(std::mem::take(&mut part));
                }
                part.push(character);
            }
            current = part;
        }
        if !current.is_empty() {
            lines.push(current);
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
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
    /// Right button is currently held. The 3D scene view uses this for mouse
    /// look; the existing one-shot `right_pressed` context-menu path remains
    /// unchanged for 2D scenes.
    pub right_down: bool,
    /// Right button transitioned to released this frame. The 3D viewport uses
    /// this edge to distinguish a context click from a fly-look drag.
    pub right_released: bool,
    /// Pointer/fly input crossed the context-click threshold during this RMB
    /// gesture, including when press and release were coalesced into one frame.
    pub right_dragged: bool,
    /// Middle button is currently held (pans the viewport).
    pub middle_down: bool,
    /// Middle button transitioned to pressed this frame.
    pub middle_pressed: bool,
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
    pub left: bool,
    pub right: bool,
    pub home: bool,
    pub end: bool,
    /// Ctrl/Cmd shortcut requests for this frame.
    pub copy: bool,
    pub paste: bool,
    pub cut: bool,
    pub save: bool,
    pub duplicate: bool,
    pub undo: bool,
    pub redo: bool,
    pub select_all: bool,
    pub invert_selection: bool,
    pub group_selection: bool,
    pub unparent_selection: bool,
    pub hide_selection: bool,
    pub show_all: bool,
    pub lock_selection: bool,
    pub unlock_all: bool,
    pub frame_all: bool,
    pub maximize_view: bool,
    pub toggle_grid: bool,
    pub toggle_snap: bool,
    /// Modifier keys held during this frame.
    pub ctrl: bool,
    pub shift: bool,
    /// Alt/Option enables orbit-around-selection in the 3D Scene View.
    pub alt: bool,
    /// `F` frames the selection; `F2` renames it; `0` resets the view.
    pub focus_selection: bool,
    pub rename: bool,
    pub reset_view: bool,
    /// Arrow-key nudge direction (-1/0/1); `nudge_big` uses the grid step.
    pub nudge_x: f32,
    pub nudge_y: f32,
    pub nudge_big: bool,
    /// Continuous 3D fly-camera movement keys. They are intentionally kept
    /// separate from text input and one-shot editor shortcuts.
    pub key_w: bool,
    pub key_a: bool,
    pub key_s: bool,
    pub key_d: bool,
    pub key_q: bool,
    pub key_e: bool,
    /// Complete runtime key state from the editor window. Scene View ignores
    /// this; the embedded 3D Game View forwards it to the isolated runtime.
    pub runtime_keys_down: Vec<String>,
    /// Physical pixels per logical window point. Viewport interaction divides
    /// pointer travel by this so camera sensitivity is DPI-independent.
    pub display_scale: f32,
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
    edit_cursor: usize,
    edit_selection_anchor: Option<usize>,
    /// The control which owns the current left-button drag. Keeping this across
    /// frames lets sliders, colour pickers, and text selection keep responding
    /// after the pointer leaves their bounds.
    pointer_capture: Option<String>,
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
        edit_cursor: usize,
        edit_selection_anchor: Option<usize>,
        pointer_capture: Option<String>,
    ) -> Self {
        let pointer_capture = if input.mouse_down {
            pointer_capture
        } else {
            None
        };
        Self {
            painter,
            input,
            theme,
            focus,
            edit_buffer,
            edit_cursor,
            edit_selection_anchor,
            pointer_capture,
            input_clip: None,
            pending_tooltip: None,
            wants_redraw: false,
        }
    }

    /// Pull the retained focus state back out at the end of a frame.
    pub fn into_focus_state(
        self,
    ) -> (Option<String>, String, usize, Option<usize>, Option<String>) {
        (
            self.focus,
            self.edit_buffer,
            self.edit_cursor,
            self.edit_selection_anchor,
            self.pointer_capture,
        )
    }

    pub fn has_focus(&self) -> bool {
        self.focus.is_some()
    }

    /// Release the active text field, used when a modal containing that field
    /// closes.
    pub fn clear_focus(&mut self) {
        self.focus = None;
        self.edit_buffer.clear();
        self.edit_cursor = 0;
        self.edit_selection_anchor = None;
    }

    /// Focus a text field from a dialog or interaction that opens during the
    /// current frame, with the caret placed at the end of its initial value.
    pub fn focus_text(&mut self, id: &str, value: &str) {
        self.focus = Some(id.to_string());
        self.edit_buffer = value.to_string();
        self.edit_cursor = char_len(value);
        self.edit_selection_anchor = None;
        self.wants_redraw = true;
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

    /// Capture a pointer drag that began inside `rect`. Once captured, the
    /// control remains active until release even if the cursor crosses a panel,
    /// window, or widget boundary. Callers should clamp the resulting value.
    pub fn pointer_drag(&mut self, id: &str, rect: Rect) -> bool {
        // `mouse_down` also covers a control which appears under an already
        // held pointer (for example, opening a colour picker by pressing its
        // swatch). With no existing owner it is safe for that new control to
        // claim the drag immediately.
        if self.pointer_capture.is_none() && self.input.mouse_down && self.hovered(rect) {
            self.pointer_capture = Some(id.to_string());
        }
        if !self.input.mouse_down {
            if self.pointer_capture.as_deref() == Some(id) {
                self.pointer_capture = None;
            }
            return false;
        }
        let active = self.pointer_capture.as_deref() == Some(id);
        if active {
            self.wants_redraw = true;
        }
        active
    }

    /// Draw a clickable button. Returns true on the frame it is pressed.
    pub fn button(&mut self, rect: Rect, label: &str) -> bool {
        let base = self.theme.button;
        let text = readable_text_color(base, self.theme.text);
        self.button_colored(rect, label, base, text)
    }

    pub fn button_colored(
        &mut self,
        rect: Rect,
        label: &str,
        base: Rgba,
        text_color: Rgba,
    ) -> bool {
        let hovered = self.hovered(rect);
        let bg = if hovered { lighten(base, 0.12) } else { base };
        let text_color = readable_text_color(bg, text_color);
        let radius = self.theme.corner_radius;
        self.painter.fill_round_rect(rect, radius, bg);
        let size = 14.0;
        let tw = self.painter.text_width(label, size);
        let tx = rect.x + (rect.w - tw) / 2.0;
        let ty = rect.y + (rect.h - size) / 2.0;
        self.painter.text_clipped(
            tx.max(rect.x + 4.0),
            ty,
            label,
            size,
            text_color,
            rect.w - 8.0,
        );
        hovered && self.input.mouse_pressed
    }

    /// A field-like button with a dropdown chevron.
    pub fn dropdown_button(&mut self, rect: Rect, label: &str) -> bool {
        let hovered = self.hovered(rect);
        let bg = if hovered {
            lighten(self.theme.button, 0.12)
        } else {
            self.theme.button
        };
        let text_color = readable_text_color(bg, self.theme.text);
        self.painter
            .fill_round_rect(rect, self.theme.corner_radius, bg);
        let icon_w = 22.0_f32.min(rect.w.max(0.0));
        self.painter.text_clipped(
            rect.x + 8.0,
            rect.y + (rect.h - 14.0) / 2.0,
            label,
            14.0,
            text_color,
            (rect.w - icon_w - 12.0).max(0.0),
        );
        self.painter.icon_centered(
            rect.right() - icon_w * 0.5,
            rect.y + rect.h * 0.5,
            icon::EXPAND_MORE,
            16.0,
            text_color,
        );
        hovered && self.input.mouse_pressed
    }

    /// A button with a leading Material icon and a text label.
    pub fn icon_button(&mut self, rect: Rect, glyph: char, label: &str) -> bool {
        let hovered = self.hovered(rect);
        let base = self.theme.button;
        let bg = if hovered { lighten(base, 0.12) } else { base };
        self.painter
            .fill_round_rect(rect, self.theme.corner_radius, bg);
        let text_color = readable_text_color(bg, self.theme.text);
        let icon_size = 16.0;
        let cy = rect.y + rect.h / 2.0;
        let label_w = self.painter.text_width(label, 14.0);
        // Center the icon+label group as a unit.
        let group_w = icon_size + 4.0 + label_w;
        let start = rect.x + (rect.w - group_w).max(0.0) / 2.0 + 8.0;
        self.painter
            .icon_centered(start, cy, glyph, icon_size, text_color);
        let label_x = start + icon_size / 2.0 + 4.0;
        self.painter.text_clipped(
            label_x,
            cy - 7.0,
            label,
            14.0,
            text_color,
            (rect.right() - label_x - 6.0).max(0.0),
        );
        hovered && self.input.mouse_pressed
    }

    /// A compact square button containing only a Material icon. `active`
    /// highlights it (e.g. a toggle that is on).
    pub fn icon_toggle(
        &mut self,
        rect: Rect,
        glyph: char,
        active: bool,
        tooltip_color: Rgba,
    ) -> bool {
        let hovered = self.hovered(rect);
        let base = if active {
            self.theme.accent
        } else {
            self.theme.button
        };
        let bg = if hovered { lighten(base, 0.12) } else { base };
        self.painter
            .fill_round_rect(rect, self.theme.corner_radius, bg);
        let color = readable_text_color(
            bg,
            if active {
                [255, 255, 255, 255]
            } else {
                tooltip_color
            },
        );
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
        let tri = if expanded {
            icon::EXPAND_MORE
        } else {
            icon::CHEVRON_RIGHT
        };
        self.painter.icon_centered(
            rect.x + 12.0,
            rect.y + rect.h / 2.0,
            tri,
            16.0,
            self.theme.text,
        );
        self.painter.text_clipped(
            rect.x + 24.0,
            rect.y + (rect.h - 14.0) / 2.0,
            label,
            14.0,
            self.theme.text,
            (rect.w - 32.0).max(0.0),
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
        let color = if danger {
            self.theme.danger
        } else {
            self.theme.text
        };
        let mut tx = rect.x + 10.0;
        if glyph != '\0' {
            self.painter
                .icon_centered(rect.x + 14.0, rect.y + rect.h / 2.0, glyph, 15.0, color);
            tx = rect.x + 28.0;
        }
        self.painter.text_clipped(
            tx,
            rect.y + (rect.h - 14.0) / 2.0,
            label,
            14.0,
            color,
            (rect.right() - tx - 8.0).max(0.0),
        );
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
        self.painter.fill_round_rect(
            Rect::new(knob_x - 5.0, rect.y + rect.h / 2.0 - 6.0, 10.0, 12.0),
            5.0,
            self.theme.text,
        );
        let drag_id = format!(
            "slider:{:08x}:{:08x}:{:08x}:{:08x}",
            rect.x.to_bits(),
            rect.y.to_bits(),
            rect.w.to_bits(),
            rect.h.to_bits()
        );
        if self.pointer_drag(&drag_id, rect) {
            let nt = ((self.input.mouse_x - rect.x) / rect.w.max(1.0)).clamp(0.0, 1.0);
            Some(min + nt * (max - min))
        } else {
            None
        }
    }

    /// A clickable color swatch (opens a picker). Returns true when clicked.
    pub fn swatch_button(&mut self, rect: Rect, color: Rgba) -> bool {
        // Checkerboard backdrop so partial alpha (transparency) is visible: the
        // colour is drawn with its real alpha on top, letting the pattern show
        // through translucent swatches.
        let prev = self.painter.push_clip(rect);
        self.painter
            .fill_round_rect(rect, 3.0, [255, 255, 255, 255]);
        let cell = (rect.h * 0.5).max(3.0);
        let cols = (rect.w / cell).ceil() as i32;
        let rows = (rect.h / cell).ceil() as i32;
        for r in 0..rows {
            for c in 0..cols {
                if (r + c) % 2 == 1 {
                    self.painter.fill_rect(
                        Rect::new(
                            rect.x + c as f32 * cell,
                            rect.y + r as f32 * cell,
                            cell,
                            cell,
                        ),
                        [176, 176, 176, 255],
                    );
                }
            }
        }
        self.painter.set_clip_raw(prev);
        self.painter.fill_round_rect(rect, 3.0, color);
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
        let line_height = 16.0;
        let pad = 7.0;
        let viewport_w = self.painter.width();
        let viewport_h = self.painter.height();
        let max_w = (viewport_w - 8.0).min(360.0);
        let max_h = viewport_h - 8.0;
        if max_w <= pad * 2.0 || max_h <= pad * 2.0 {
            return;
        }
        let lines = wrap_text_lines(&self.painter, &text, size, max_w - pad * 2.0);
        let visible_lines = lines
            .len()
            .min(((max_h - pad * 2.0) / line_height).floor().max(1.0) as usize);
        let content_w = lines
            .iter()
            .take(visible_lines)
            .map(|line| self.painter.text_width(line, size))
            .fold(0.0_f32, f32::max)
            .min(max_w - pad * 2.0);
        let w = (content_w + pad * 2.0).max(40.0).min(max_w);
        let h = (visible_lines as f32 * line_height + pad * 2.0).min(max_h);
        // Position below-right of the cursor, nudged on-screen.
        let mut x = self.input.mouse_x + 14.0;
        let mut y = self.input.mouse_y + 18.0;
        if x + w > viewport_w {
            x = viewport_w - w - 2.0;
        }
        if y + h > viewport_h {
            y = self.input.mouse_y - h - 6.0;
        }
        x = x.max(2.0);
        y = y.clamp(2.0, (viewport_h - h - 2.0).max(2.0));
        let rect = Rect::new(x, y, w, h);
        self.painter.fill_round_rect(rect, 4.0, [20, 20, 22, 240]);
        self.painter.stroke_round_rect(rect, 4.0, self.theme.accent);
        self.painter.text_wrapped(
            Rect::new(x + pad, y + pad, w - pad * 2.0, h - pad * 2.0),
            &text,
            size,
            line_height,
            self.theme.text,
        );
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
        let text_color = readable_text_color(
            bg,
            if selected {
                [255, 255, 255, 255]
            } else {
                self.theme.text
            },
        );
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
        let base = if value {
            self.theme.accent
        } else {
            self.theme.field
        };
        let bg = if hovered { lighten(base, 0.1) } else { base };
        self.painter.fill_round_rect(rect, radius, bg);
        self.painter
            .stroke_round_rect(rect, radius, self.theme.border);
        if value {
            self.painter.icon_centered(
                rect.x + rect.w * 0.5,
                rect.y + rect.h * 0.5,
                icon::CHECK,
                rect.w.min(rect.h) * 0.72,
                [255, 255, 255, 255],
            );
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
        let size = 14.0;
        let text_x = rect.x + 6.0;
        let focused = self.focus.as_deref() == Some(id);
        let hovered = self.hovered(rect);
        let drag_id = format!("text:{id}");
        let dragging = self.pointer_drag(&drag_id, rect);

        if self.input.mouse_pressed {
            if hovered {
                let was_focused = focused;
                if !focused {
                    self.focus = Some(id.to_string());
                    self.edit_buffer = value.to_string();
                    self.edit_cursor = char_len(&self.edit_buffer);
                    self.edit_selection_anchor = None;
                }
                let clicked = cursor_index_at_x(
                    &self.painter,
                    &self.edit_buffer,
                    size,
                    text_x,
                    self.input.mouse_x,
                );
                if self.input.double_click {
                    self.edit_selection_anchor = Some(0);
                    self.edit_cursor = char_len(&self.edit_buffer);
                } else if self.input.shift && was_focused {
                    if self.edit_selection_anchor.is_none() {
                        self.edit_selection_anchor = Some(self.edit_cursor);
                    }
                    self.edit_cursor = clicked;
                } else {
                    self.edit_cursor = clicked;
                    self.edit_selection_anchor = None;
                }
            } else if focused {
                self.focus = None;
            }
        }

        let focused = self.focus.as_deref() == Some(id);
        let mut changed = false;

        if focused {
            self.wants_redraw = true;
            clamp_edit_state(
                &self.edit_buffer,
                &mut self.edit_cursor,
                &mut self.edit_selection_anchor,
            );

            if dragging && !self.input.mouse_pressed {
                if self.edit_selection_anchor.is_none() {
                    self.edit_selection_anchor = Some(self.edit_cursor);
                }
                self.edit_cursor = cursor_index_at_x(
                    &self.painter,
                    &self.edit_buffer,
                    size,
                    text_x,
                    self.input.mouse_x,
                );
                clamp_empty_selection(&mut self.edit_selection_anchor, self.edit_cursor);
            }

            if self.input.select_all {
                self.edit_selection_anchor = Some(0);
                self.edit_cursor = char_len(&self.edit_buffer);
            }
            if self.input.copy
                && let Some((start, end)) =
                    selection_range(self.edit_cursor, self.edit_selection_anchor)
            {
                let selected = slice_char_range(&self.edit_buffer, start, end);
                write_clipboard(&selected);
            }
            if self.input.cut
                && let Some((start, end)) =
                    selection_range(self.edit_cursor, self.edit_selection_anchor)
            {
                let selected = slice_char_range(&self.edit_buffer, start, end);
                write_clipboard(&selected);
                replace_char_range(&mut self.edit_buffer, start, end, "");
                self.edit_cursor = start;
                self.edit_selection_anchor = None;
                changed = true;
            }
            if self.input.paste
                && let Some(text) = read_clipboard()
            {
                let pasted = normalize_pasted_text(&text);
                if !pasted.is_empty() {
                    insert_text_at_cursor(
                        &mut self.edit_buffer,
                        &mut self.edit_cursor,
                        &mut self.edit_selection_anchor,
                        &pasted,
                    );
                    changed = true;
                }
            }

            let len = char_len(&self.edit_buffer);
            let extend_selection = self.input.shift;
            let move_cursor =
                |next: usize, cursor: &mut usize, selection_anchor: &mut Option<usize>| {
                    if extend_selection {
                        if selection_anchor.is_none() {
                            *selection_anchor = Some(*cursor);
                        }
                    } else {
                        *selection_anchor = None;
                    }
                    *cursor = next.min(len);
                    clamp_empty_selection(selection_anchor, *cursor);
                };
            if self.input.home {
                move_cursor(0, &mut self.edit_cursor, &mut self.edit_selection_anchor);
            }
            if self.input.end {
                move_cursor(len, &mut self.edit_cursor, &mut self.edit_selection_anchor);
            }
            if self.input.left {
                move_cursor(
                    self.edit_cursor.saturating_sub(1),
                    &mut self.edit_cursor,
                    &mut self.edit_selection_anchor,
                );
            }
            if self.input.right {
                move_cursor(
                    (self.edit_cursor + 1).min(len),
                    &mut self.edit_cursor,
                    &mut self.edit_selection_anchor,
                );
            }

            for ch in self.input.typed.chars() {
                if !self.input.ctrl && !ch.is_control() {
                    let mut text = [0; 4];
                    insert_text_at_cursor(
                        &mut self.edit_buffer,
                        &mut self.edit_cursor,
                        &mut self.edit_selection_anchor,
                        ch.encode_utf8(&mut text),
                    );
                    changed = true;
                }
            }
            if self.input.backspace {
                if delete_selection_or_range(
                    &mut self.edit_buffer,
                    &mut self.edit_cursor,
                    &mut self.edit_selection_anchor,
                    true,
                ) {
                    changed = true;
                }
            }
            if self.input.delete {
                if delete_selection_or_range(
                    &mut self.edit_buffer,
                    &mut self.edit_cursor,
                    &mut self.edit_selection_anchor,
                    false,
                ) {
                    changed = true;
                }
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

        let ty = rect.y + (rect.h - size) / 2.0;
        let text_color = self.theme.text;
        if focused {
            let clip = Rect::new(
                rect.x + 4.0,
                rect.y + 1.0,
                (rect.w - 8.0).max(0.0),
                rect.h - 2.0,
            );
            let previous_clip = self.painter.push_clip(clip);
            if let Some((start, end)) =
                selection_range(self.edit_cursor, self.edit_selection_anchor)
            {
                let start_x = text_x + text_prefix_width(&self.painter, &display, size, start);
                let end_x = text_x + text_prefix_width(&self.painter, &display, size, end);
                let selection_rect = Rect::new(
                    start_x,
                    rect.y + 3.0,
                    (end_x - start_x).max(1.0),
                    rect.h - 6.0,
                );
                self.painter.fill_rect(
                    selection_rect,
                    [
                        self.theme.accent[0],
                        self.theme.accent[1],
                        self.theme.accent[2],
                        110,
                    ],
                );
            }
            self.painter.text(text_x, ty, &display, size, text_color);
            let caret_x = (text_x
                + text_prefix_width(&self.painter, &display, size, self.edit_cursor))
            .min(rect.right() - 4.0)
            .max(rect.x + 4.0);
            self.painter.fill_rect(
                Rect::new(caret_x, rect.y + 4.0, 1.0, rect.h - 8.0),
                text_color,
            );
            self.painter.set_clip_raw(previous_clip);
        } else {
            self.painter
                .text_clipped(text_x, ty, &display, size, text_color, rect.w - 12.0);
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

fn char_len(text: &str) -> usize {
    text.chars().count()
}

fn byte_index_for_char(text: &str, char_index: usize) -> usize {
    if char_index == 0 {
        return 0;
    }
    text.char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or(text.len())
}

fn clamp_edit_state(text: &str, cursor: &mut usize, selection_anchor: &mut Option<usize>) {
    let len = char_len(text);
    *cursor = (*cursor).min(len);
    if let Some(anchor) = selection_anchor.as_mut() {
        *anchor = (*anchor).min(len);
    }
    clamp_empty_selection(selection_anchor, *cursor);
}

fn clamp_empty_selection(selection_anchor: &mut Option<usize>, cursor: usize) {
    if selection_anchor.is_some_and(|anchor| anchor == cursor) {
        *selection_anchor = None;
    }
}

fn selection_range(cursor: usize, selection_anchor: Option<usize>) -> Option<(usize, usize)> {
    let anchor = selection_anchor?;
    if anchor == cursor {
        return None;
    }
    Some((anchor.min(cursor), anchor.max(cursor)))
}

fn slice_char_range(text: &str, start: usize, end: usize) -> String {
    let start = byte_index_for_char(text, start);
    let end = byte_index_for_char(text, end);
    text[start..end].to_string()
}

fn replace_char_range(text: &mut String, start: usize, end: usize, replacement: &str) {
    let start = byte_index_for_char(text, start);
    let end = byte_index_for_char(text, end);
    text.replace_range(start..end, replacement);
}

fn insert_text_at_cursor(
    text: &mut String,
    cursor: &mut usize,
    selection_anchor: &mut Option<usize>,
    inserted: &str,
) {
    let (start, end) = selection_range(*cursor, *selection_anchor).unwrap_or((*cursor, *cursor));
    replace_char_range(text, start, end, inserted);
    *cursor = start + char_len(inserted);
    *selection_anchor = None;
}

fn delete_selection_or_range(
    text: &mut String,
    cursor: &mut usize,
    selection_anchor: &mut Option<usize>,
    backspace: bool,
) -> bool {
    if let Some((start, end)) = selection_range(*cursor, *selection_anchor) {
        replace_char_range(text, start, end, "");
        *cursor = start;
        *selection_anchor = None;
        return true;
    }
    if backspace {
        if *cursor == 0 {
            return false;
        }
        replace_char_range(text, *cursor - 1, *cursor, "");
        *cursor -= 1;
        return true;
    }
    if *cursor >= char_len(text) {
        return false;
    }
    replace_char_range(text, *cursor, *cursor + 1, "");
    true
}

fn text_prefix_width(painter: &Painter<'_>, text: &str, size: f32, char_count: usize) -> f32 {
    let end = byte_index_for_char(text, char_count);
    painter.text_width(&text[..end], size)
}

fn cursor_index_at_x(painter: &Painter<'_>, text: &str, size: f32, start_x: f32, x: f32) -> usize {
    if x <= start_x {
        return 0;
    }
    let mut pen = start_x;
    for (index, ch) in text.chars().enumerate() {
        let width = painter.text_width(&ch.to_string(), size);
        if x < pen + width * 0.5 {
            return index;
        }
        pen += width;
        if x < pen {
            return index + 1;
        }
    }
    char_len(text)
}

fn normalize_pasted_text(text: &str) -> String {
    text.chars()
        .map(|ch| if matches!(ch, '\r' | '\n') { ' ' } else { ch })
        .collect::<String>()
}

fn read_clipboard() -> Option<String> {
    if let Some(text) = read_native_clipboard() {
        return Some(text);
    }
    read_clipboard_with_helper()
}

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "android")))]
fn read_native_clipboard() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok()
}

#[cfg(not(all(not(target_arch = "wasm32"), not(target_os = "android"))))]
fn read_native_clipboard() -> Option<String> {
    None
}

fn read_clipboard_with_helper() -> Option<String> {
    read_clipboard_with_platform_helper()
}

#[cfg(target_os = "macos")]
fn read_clipboard_with_platform_helper() -> Option<String> {
    try_read_clipboard_command("pbpaste", &[])
}

#[cfg(windows)]
fn read_clipboard_with_platform_helper() -> Option<String> {
    const READ: &str =
        "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; Get-Clipboard -Raw";
    try_read_clipboard_command("powershell", &["-NoProfile", "-Command", READ])
        .or_else(|| try_read_clipboard_command("powershell.exe", &["-NoProfile", "-Command", READ]))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn read_clipboard_with_platform_helper() -> Option<String> {
    try_read_clipboard_command("wl-paste", &["--no-newline"])
        .or_else(|| try_read_clipboard_command("xclip", &["-selection", "clipboard", "-o"]))
        .or_else(|| try_read_clipboard_command("xsel", &["--clipboard", "--output"]))
}

fn try_read_clipboard_command(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if output.status.success() {
        String::from_utf8(output.stdout).ok()
    } else {
        None
    }
}

fn write_clipboard(text: &str) -> bool {
    if write_native_clipboard(text) {
        return true;
    }
    write_clipboard_with_helper(text)
}

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "android")))]
fn write_native_clipboard(text: &str) -> bool {
    arboard::Clipboard::new()
        .and_then(|mut clipboard| clipboard.set_text(text.to_string()))
        .is_ok()
}

#[cfg(not(all(not(target_arch = "wasm32"), not(target_os = "android"))))]
fn write_native_clipboard(_text: &str) -> bool {
    false
}

fn write_clipboard_with_helper(text: &str) -> bool {
    write_clipboard_with_platform_helper(text)
}

#[cfg(target_os = "macos")]
fn write_clipboard_with_platform_helper(text: &str) -> bool {
    try_write_clipboard_command("pbcopy", &[], text)
}

#[cfg(windows)]
fn write_clipboard_with_platform_helper(text: &str) -> bool {
    const WRITE: &str = "Set-Clipboard -Value ([Console]::In.ReadToEnd())";
    try_write_clipboard_command("powershell", &["-NoProfile", "-Command", WRITE], text)
        || try_write_clipboard_command("powershell.exe", &["-NoProfile", "-Command", WRITE], text)
        || try_write_clipboard_command("clip", &[], text)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn write_clipboard_with_platform_helper(text: &str) -> bool {
    try_write_clipboard_command("wl-copy", &[], text)
        || try_write_clipboard_command("xclip", &["-selection", "clipboard"], text)
        || try_write_clipboard_command("xsel", &["--clipboard", "--input"], text)
}

fn try_write_clipboard_command(program: &str, args: &[&str], text: &str) -> bool {
    let mut child = match Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };
    let mut wrote = true;
    if let Some(stdin) = child.stdin.as_mut() {
        wrote = stdin.write_all(text.as_bytes()).is_ok();
    }
    wrote && child.wait().is_ok_and(|status| status.success())
}

/// Lighten a color toward white by `t` in `[0, 1]`, preserving alpha.
fn lighten(color: Rgba, t: f32) -> Rgba {
    let mix = |c: u8| {
        (c as f32 + (255.0 - c as f32) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    [mix(color[0]), mix(color[1]), mix(color[2]), color[3]]
}

fn linear_channel(channel: u8) -> f32 {
    let value = channel as f32 / 255.0;
    if value <= 0.03928 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn relative_luminance(color: Rgba) -> f32 {
    0.2126 * linear_channel(color[0])
        + 0.7152 * linear_channel(color[1])
        + 0.0722 * linear_channel(color[2])
}

fn contrast_ratio(a: Rgba, b: Rgba) -> f32 {
    let a = relative_luminance(a);
    let b = relative_luminance(b);
    let (lighter, darker) = if a >= b { (a, b) } else { (b, a) };
    (lighter + 0.05) / (darker + 0.05)
}

fn readable_text_color(background: Rgba, preferred: Rgba) -> Rgba {
    if contrast_ratio(background, preferred) >= 4.5 {
        return preferred;
    }
    let dark = [22, 24, 28, preferred[3]];
    let light = [255, 255, 255, preferred[3]];
    if contrast_ratio(background, dark) >= contrast_ratio(background, light) {
        dark
    } else {
        light
    }
}

/// Load the editor's text and icon fonts once. Returns shareable handles so the
/// painter can be rebuilt cheaply each frame.
pub fn load_fonts() -> Result<Fonts, String> {
    load_fonts_from_path(None)
}

pub fn load_fonts_from_path(text_font_path: Option<&Path>) -> Result<Fonts, String> {
    let text = match text_font_path.filter(|path| !path.as_os_str().is_empty()) {
        Some(path) => {
            let bytes = std::fs::read(path)
                .map_err(|e| format!("failed to read editor font {}: {e}", path.display()))?;
            Font::from_bytes(bytes, fontdue::FontSettings::default())
                .map(Arc::new)
                .map_err(|e| format!("failed to load editor font {}: {e}", path.display()))?
        }
        None => Font::from_bytes(EDITOR_FONT_BYTES, fontdue::FontSettings::default())
            .map(Arc::new)
            .map_err(|e| format!("failed to load editor font: {e}"))?,
    };
    let icons = Font::from_bytes(ICON_FONT_BYTES, fontdue::FontSettings::default())
        .map(Arc::new)
        .map_err(|e| format!("failed to load icon font: {e}"))?;
    Ok(Fonts {
        text,
        icons,
        glyph_cache: Arc::new(Mutex::new(GlyphRasterCache::new(
            GLYPH_CACHE_ENTRY_LIMIT,
            GLYPH_CACHE_BYTE_LIMIT,
        ))),
    })
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
    fn glyph_rasters_are_shared_and_font_faces_stay_separate() {
        let fonts = load_fonts().expect("load fonts");
        let first = fonts.rasterize_cached(FontFace::Text, 'A', 14.0);
        let second = fonts.rasterize_cached(FontFace::Text, 'A', 14.0);
        let icon = fonts.rasterize_cached(FontFace::Icons, 'A', 14.0);

        assert!(Arc::ptr_eq(&first, &second));
        assert!(!Arc::ptr_eq(&first, &icon));
        assert_eq!(
            fonts.glyph_cache.lock().expect("glyph cache").entries.len(),
            2
        );
    }

    #[test]
    fn glyph_raster_cache_evicts_lru_entries_to_stay_bounded() {
        let mut cache = GlyphRasterCache::new(2, 5);
        let key = |character| GlyphCacheKey {
            face: FontFace::Text,
            character,
            size_bits: 14.0f32.to_bits(),
        };
        let glyph = || {
            Arc::new(CachedGlyph {
                metrics: Metrics::default(),
                bitmap: vec![255, 128].into_boxed_slice(),
            })
        };

        cache.insert(key('a'), glyph());
        cache.insert(key('b'), glyph());
        assert!(cache.get(key('a')).is_some(), "refresh a's LRU age");
        cache.insert(key('c'), glyph());

        assert!(cache.entries.contains_key(&key('a')));
        assert!(!cache.entries.contains_key(&key('b')));
        assert!(cache.entries.contains_key(&key('c')));
        assert!(cache.entries.len() <= 2);
        assert!(cache.bitmap_bytes <= 5);
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
    fn fill_circle_covers_centre_not_corners() {
        let mut buf = vec![0u32; 11 * 11];
        let fonts = load_fonts().expect("load fonts");
        let mut painter = Painter::new(&mut buf, 11, 11, fonts);
        painter.fill_circle(5.5, 5.5, 4.0, [255, 255, 255, 255]);
        assert_ne!(buf[5 * 11 + 5], 0, "centre should be filled");
        assert_eq!(buf[0], 0, "top-left corner is outside the circle");
        assert_eq!(buf[10 * 11 + 10], 0, "bottom-right corner is outside");
    }

    #[test]
    fn fill_triangle_uses_point_in_triangle_test() {
        let mut buf = vec![0u32; 12 * 12];
        let fonts = load_fonts().expect("load fonts");
        let mut painter = Painter::new(&mut buf, 12, 12, fonts);
        // Right triangle with the right angle at the bottom-left.
        painter.fill_triangle((0.0, 0.0), (0.0, 10.0), (10.0, 10.0), [255, 255, 255, 255]);
        assert_ne!(buf[8 * 12 + 2], 0, "(2,8) is inside the triangle");
        assert_eq!(buf[2 * 12 + 8], 0, "(8,2) is outside the triangle");
    }

    #[test]
    fn depth_tested_triangles_keep_the_nearest_fragment_regardless_of_draw_order() {
        let fonts = load_fonts().expect("load fonts");
        for near_first in [false, true] {
            let mut buffer = vec![0u32; 12 * 12];
            let mut depth = vec![f32::INFINITY; buffer.len()];
            let mut painter = Painter::new(&mut buffer, 12, 12, fonts.clone());
            let points = [(1.0, 1.0), (1.0, 11.0), (11.0, 11.0)];
            let mut draw = |depth_value, color| {
                painter.fill_triangle_depth_tested(
                    points,
                    [depth_value; 3],
                    color,
                    &mut depth,
                );
            };
            if near_first {
                draw(0.2, [255, 0, 0, 255]);
                draw(0.8, [0, 0, 255, 255]);
            } else {
                draw(0.8, [0, 0, 255, 255]);
                draw(0.2, [255, 0, 0, 255]);
            }
            drop(draw);
            drop(painter);
            assert_eq!(buffer[8 * 12 + 3], 0xff0000);
            assert!((depth[8 * 12 + 3] - 0.2).abs() < 1e-6);
        }
    }

    #[test]
    fn diagonal_line_only_rasterizes_its_narrow_coverage_strip() {
        let mut buf = vec![0u32; 128 * 96];
        let fonts = load_fonts().expect("load fonts");
        let mut painter = Painter::new(&mut buf, 128, 96, fonts);
        painter.stroke_line(2.0, 2.0, 125.0, 92.0, [255, 255, 255, 255]);

        assert_ne!(buf[47 * 128 + 64], 0, "the diagonal midpoint is covered");
        assert_eq!(buf[90 * 128 + 4], 0, "pixels far from the line stay clear");
        assert!(
            buf.iter().filter(|pixel| **pixel != 0).count() < 512,
            "a one-pixel line should touch O(length) pixels"
        );
    }

    #[test]
    fn opaque_fill_rect_replaces_only_the_clipped_rows() {
        let mut buf = vec![0x010203u32; 6 * 4];
        let fonts = load_fonts().expect("load fonts");
        let mut painter = Painter::new(&mut buf, 6, 4, fonts);
        let previous = painter.push_clip(Rect::new(2.0, 1.0, 3.0, 2.0));
        painter.fill_rect(Rect::new(0.0, 0.0, 6.0, 4.0), [0x12, 0x34, 0x56, 255]);
        painter.set_clip_raw(previous);

        assert_eq!(buf[1 * 6 + 2], 0x123456);
        assert_eq!(buf[2 * 6 + 4], 0x123456);
        assert_eq!(buf[0], 0x010203);
        assert_eq!(buf[3 * 6 + 5], 0x010203);
    }

    #[test]
    fn opaque_fill_rect_ignores_empty_and_reversed_rectangles() {
        let original = vec![0x010203u32; 6 * 4];
        let mut buf = original.clone();
        let fonts = load_fonts().expect("load fonts");
        let mut painter = Painter::new(&mut buf, 6, 4, fonts);

        painter.fill_rect(Rect::new(4.0, 1.0, -3.0, 2.0), [255, 255, 255, 255]);
        painter.fill_rect(Rect::new(1.0, 3.0, 2.0, -2.0), [255, 255, 255, 255]);
        painter.fill_rect(Rect::new(2.0, 2.0, 0.0, 1.0), [255, 255, 255, 255]);

        assert_eq!(buf, original);
    }

    #[test]
    fn composite_light_multiplies_scene_by_light() {
        let mut buf = vec![0u32; 4 * 4];
        let fonts = load_fonts().expect("load fonts");
        let mut painter = Painter::new(&mut buf, 4, 4, fonts);
        // Fill mid-gray, then light: left half fully dark, right half over-bright.
        painter.fill_rect(Rect::new(0.0, 0.0, 4.0, 4.0), [128, 128, 128, 255]);
        painter.composite_light(Rect::new(0.0, 0.0, 4.0, 4.0), 0.0, |px, _py| {
            if px < 2.0 {
                (0.0, 0.0, 0.0)
            } else {
                (4.0, 4.0, 4.0)
            }
        });
        // Dark side is multiplied to black; bright side clamps to full white.
        assert_eq!(
            buf[0] & 0xff,
            0,
            "unlit background is darkened, not left bright"
        );
        assert_eq!(buf[3] & 0xff, 0xff, "over-bright light clamps to full");
    }

    #[test]
    fn rotation_moves_painted_pixels() {
        let mut buf = vec![0u32; 5 * 5];
        let fonts = load_fonts().expect("load fonts");
        let mut painter = Painter::new(&mut buf, 5, 5, fonts);
        // A 90° rotation about (2,2) turns the top row into a column at x=3.
        painter.push_rotation(2.0, 2.0, std::f32::consts::FRAC_PI_2);
        painter.fill_rect(Rect::new(0.0, 0.0, 5.0, 1.0), [255, 255, 255, 255]);
        assert_ne!(buf[2 * 5 + 3], 0, "rotated strip lands on the x=3 column");
        assert_eq!(buf[0], 0, "the unrotated top-left is no longer painted");
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

    #[test]
    fn clipped_text_with_no_room_draws_nothing() {
        let mut buffer = vec![0u32; 80 * 24];
        let fonts = load_fonts().expect("load fonts");
        let mut painter = Painter::new(&mut buffer, 80, 24, fonts);
        painter.text_clipped(
            10.0,
            4.0,
            "This must not leak",
            14.0,
            [255, 255, 255, 255],
            0.0,
        );
        assert!(buffer.iter().all(|pixel| *pixel == 0));
    }

    #[test]
    fn wrapping_hard_breaks_long_tokens_to_the_available_width() {
        let mut buffer = vec![0u32; 100 * 24];
        let fonts = load_fonts().expect("load fonts");
        let painter = Painter::new(&mut buffer, 100, 24, fonts);
        let max_width = 42.0;
        let lines = wrap_text_lines(
            &painter,
            "short words and/a/very/long/unbroken/path",
            13.0,
            max_width,
        );
        assert!(lines.len() > 2);
        assert!(
            lines
                .iter()
                .all(|line| painter.text_width(line, 13.0) <= max_width + 0.01),
            "wrapped lines exceeded {max_width}px: {lines:?}"
        );
    }

    #[test]
    fn icon_button_clips_an_overlong_label_to_its_bounds() {
        let width = 150usize;
        let height = 40usize;
        let mut buffer = vec![0u32; width * height];
        let fonts = load_fonts().expect("load fonts");
        let painter = Painter::new(&mut buffer, width, height, fonts);
        let mut ui = Ui::new(
            painter,
            FrameInput::default(),
            Theme::default(),
            None,
            String::new(),
            0,
            None,
            None,
        );
        let rect = Rect::new(10.0, 8.0, 62.0, 24.0);
        ui.icon_button(
            rect,
            icon::ADD,
            "An extremely long button label that used to overflow",
        );
        drop(ui);

        for y in 0..height {
            assert!(
                buffer[y * width + 72..(y + 1) * width]
                    .iter()
                    .all(|pixel| *pixel == 0),
                "button painted beyond its right edge on row {y}"
            );
        }
    }

    #[test]
    fn slider_keeps_pointer_capture_and_clamps_outside_its_bounds() {
        let fonts = load_fonts().expect("load fonts");
        let rect = Rect::new(20.0, 20.0, 100.0, 20.0);
        let mut capture = None;

        let mut buffer = vec![0u32; 200 * 60];
        let painter = Painter::new(&mut buffer, 200, 60, fonts.clone());
        let mut ui = Ui::new(
            painter,
            FrameInput {
                mouse_x: 70.0,
                mouse_y: 30.0,
                mouse_pressed: true,
                mouse_down: true,
                ..Default::default()
            },
            Theme::default(),
            None,
            String::new(),
            0,
            None,
            capture,
        );
        assert_eq!(ui.slider(rect, 0.0, 0.0, 1.0), Some(0.5));
        let (_, _, _, _, next_capture) = ui.into_focus_state();
        capture = next_capture;

        let mut buffer = vec![0u32; 200 * 60];
        let painter = Painter::new(&mut buffer, 200, 60, fonts.clone());
        let mut ui = Ui::new(
            painter,
            FrameInput {
                mouse_x: 180.0,
                mouse_y: 50.0,
                mouse_down: true,
                ..Default::default()
            },
            Theme::default(),
            None,
            String::new(),
            0,
            None,
            capture,
        );
        assert_eq!(ui.slider(rect, 0.5, 0.0, 1.0), Some(1.0));
        let (_, _, _, _, capture) = ui.into_focus_state();
        assert!(capture.is_some());

        let mut buffer = vec![0u32; 200 * 60];
        let painter = Painter::new(&mut buffer, 200, 60, fonts);
        let mut ui = Ui::new(
            painter,
            FrameInput {
                mouse_x: 180.0,
                mouse_y: 50.0,
                ..Default::default()
            },
            Theme::default(),
            None,
            String::new(),
            0,
            None,
            capture,
        );
        assert_eq!(ui.slider(rect, 1.0, 0.0, 1.0), None);
        let (_, _, _, _, capture) = ui.into_focus_state();
        assert!(capture.is_none());
    }

    #[test]
    fn text_selection_continues_to_nearest_character_outside_field() {
        let fonts = load_fonts().expect("load fonts");
        let rect = Rect::new(20.0, 20.0, 100.0, 24.0);
        let value = "abcdef";
        let mut buffer = vec![0u32; 220 * 64];
        let painter = Painter::new(&mut buffer, 220, 64, fonts.clone());
        let mut ui = Ui::new(
            painter,
            FrameInput {
                mouse_x: 42.0,
                mouse_y: 30.0,
                mouse_pressed: true,
                mouse_down: true,
                ..Default::default()
            },
            Theme::default(),
            None,
            String::new(),
            0,
            None,
            None,
        );
        ui.text_field("field", rect, value);
        let (focus, edit, cursor, anchor, capture) = ui.into_focus_state();

        let mut buffer = vec![0u32; 220 * 64];
        let painter = Painter::new(&mut buffer, 220, 64, fonts);
        let mut ui = Ui::new(
            painter,
            FrameInput {
                mouse_x: 210.0,
                mouse_y: 55.0,
                mouse_down: true,
                ..Default::default()
            },
            Theme::default(),
            focus,
            edit,
            cursor,
            anchor,
            capture,
        );
        ui.text_field("field", rect, value);
        let (_, _, cursor, anchor, capture) = ui.into_focus_state();
        assert_eq!(cursor, value.chars().count());
        assert!(anchor.is_some());
        assert!(capture.is_some());
    }
}
