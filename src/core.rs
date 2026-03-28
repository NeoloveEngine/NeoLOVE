use crate::assets::ImageHandle;
use crate::lua_error::protect_lua_call;
use crate::platform::{Color, InputState, SharedPlatformState, WindowState};
use crate::renderer::{
    DrawCommand, FontHandle, Rect, RenderState, SharedRenderState, TextAlignX, TextAlignY,
    TextRenderRequest, TextScaleMode, TextWrapMode, TextureFilter, Vec2,
};
use mlua::{AnyUserData, Function, Lua, Table, Value};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};

fn color4(lua: &Lua, r: u8, g: u8, b: u8, a: u8) -> mlua::Result<Table> {
    let color = lua.create_table()?;
    color.set("r", r)?;
    color.set("g", g)?;
    color.set("b", b)?;
    color.set("a", a)?;
    Ok(color)
}

fn color4_to_color(color4: Table) -> mlua::Result<Color> {
    let r: f32 = color4.get("r")?;
    let g: f32 = color4.get("g")?;
    let b: f32 = color4.get("b")?;
    let a: f32 = color4.get("a")?;
    Ok(Color::rgba(
        r.clamp(0.0, 255.0) as u8,
        g.clamp(0.0, 255.0) as u8,
        b.clamp(0.0, 255.0) as u8,
        a.clamp(0.0, 255.0) as u8,
    ))
}

fn rotate_local(x: f32, y: f32, rotation: f32) -> (f32, f32) {
    let cos_r = rotation.cos();
    let sin_r = rotation.sin();
    (x * cos_r - y * sin_r, x * sin_r + y * cos_r)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn resolve_font_path(root: &Path, input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("default") {
        return None;
    }

    let path = PathBuf::from(trimmed);
    let candidate = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    let resolved = normalize_path(&candidate);
    if !resolved.starts_with(root) {
        return None;
    }
    Some(resolved.to_string_lossy().into_owned())
}

fn parse_font_handle(root: &Path, value: Value) -> FontHandle {
    match value {
        Value::String(value) => value
            .to_str()
            .ok()
            .and_then(|value| resolve_font_path(root, &value))
            .map(FontHandle::Path)
            .unwrap_or(FontHandle::Default),
        Value::Table(table) => {
            if let Ok(path) = table
                .get::<String>("path")
                .or_else(|_| table.get::<String>("file"))
                .or_else(|_| table.get::<String>("source"))
            {
                if let Some(path) = resolve_font_path(root, &path) {
                    return FontHandle::Path(path);
                }
            }

            if let Ok(builtin) = table
                .get::<String>("builtin")
                .or_else(|_| table.get::<String>("name"))
            {
                if builtin.trim().eq_ignore_ascii_case("default") {
                    return FontHandle::Default;
                }
            }

            FontHandle::Default
        }
        _ => FontHandle::Default,
    }
}

fn parse_text_scale_mode(raw: &str) -> TextScaleMode {
    match raw.trim().to_ascii_lowercase().as_str() {
        "fit" | "contain" => TextScaleMode::Fit,
        "fit_width" | "fitwidth" | "width" => TextScaleMode::FitWidth,
        "fit_height" | "fitheight" | "height" => TextScaleMode::FitHeight,
        _ => TextScaleMode::None,
    }
}

fn parse_align_x(raw: &str) -> TextAlignX {
    match raw.trim().to_ascii_lowercase().as_str() {
        "center" | "centre" | "middle" => TextAlignX::Center,
        "right" | "end" => TextAlignX::Right,
        _ => TextAlignX::Left,
    }
}

fn parse_align_y(raw: &str) -> TextAlignY {
    match raw.trim().to_ascii_lowercase().as_str() {
        "center" | "centre" | "middle" => TextAlignY::Center,
        "bottom" | "end" => TextAlignY::Bottom,
        _ => TextAlignY::Top,
    }
}

fn parse_wrap_mode(value: Value) -> TextWrapMode {
    match value {
        Value::Boolean(true) => TextWrapMode::Word,
        Value::String(value) => match value.to_str() {
            Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
                "word" | "words" => TextWrapMode::Word,
                "char" | "character" | "characters" => TextWrapMode::Char,
                _ => TextWrapMode::None,
            },
            Err(_) => TextWrapMode::None,
        },
        _ => TextWrapMode::None,
    }
}

fn uses_entity_text_bounds(component: &Table) -> bool {
    let size_mode = component
        .get::<String>("size_mode")
        .or_else(|_| component.get::<String>("bounds_mode"))
        .unwrap_or_else(|_| "content".to_string());
    match size_mode.trim().to_ascii_lowercase().as_str() {
        "entity" | "box" | "bounds" => true,
        "content" | "label" => false,
        _ => !component.get::<bool>("auto_size").unwrap_or(true),
    }
}

fn app_texture_filter(lua: &Lua) -> TextureFilter {
    let nearest = lua
        .globals()
        .get::<Table>("app")
        .ok()
        .and_then(|app| app.get::<bool>("nearestNeighborScaling").ok())
        .unwrap_or(true);
    if nearest {
        TextureFilter::Nearest
    } else {
        TextureFilter::Linear
    }
}

#[derive(Clone, Copy, Debug)]
struct EntityDrawContext {
    bounds: Rect,
    pivot: Vec2,
    rotation: f32,
}

#[derive(Clone, Debug)]
struct UiPanelStyle {
    background_color: Color,
    border_color: Color,
    border_width: f32,
    corner_radius: f32,
    background_image: Option<ImageHandle>,
    slice_left: f32,
    slice_right: f32,
    slice_top: f32,
    slice_bottom: f32,
    filter: TextureFilter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UiIconSide {
    Left,
    Right,
}

#[derive(Clone, Debug)]
struct UiInlineImage {
    image: ImageHandle,
    tint: Color,
    source: Option<Rect>,
    width: f32,
    height: f32,
    gap: f32,
    side: UiIconSide,
}

#[derive(Clone, Debug)]
struct UiInlineImageLayout {
    image: ImageHandle,
    tint: Color,
    source: Option<Rect>,
    bounds: Rect,
}

#[derive(Clone, Debug)]
struct UiListItem {
    text: String,
    value: String,
    image: Option<ImageHandle>,
    image_tint: Color,
    image_source: Option<Rect>,
}

#[derive(Clone, Debug)]
struct UiInputSnapshot {
    mouse: Vec2,
    input: InputState,
    window: WindowState,
}

#[derive(Clone, Debug)]
struct UiPopupRegion {
    owner: String,
    bounds: Rect,
    pivot: Vec2,
    rotation: f32,
}

#[derive(Default)]
struct UiFrameState {
    active_popups: Vec<UiPopupRegion>,
    next_popups: Vec<UiPopupRegion>,
}

fn ui_frame_state() -> &'static Mutex<UiFrameState> {
    static STATE: OnceLock<Mutex<UiFrameState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(UiFrameState::default()))
}

fn upsert_popup(popups: &mut Vec<UiPopupRegion>, popup: UiPopupRegion) {
    if let Some(existing) = popups
        .iter_mut()
        .find(|existing| existing.owner == popup.owner)
    {
        *existing = popup;
    } else {
        popups.push(popup);
    }
}

pub(crate) fn begin_ui_frame() {
    if let Ok(mut state) = ui_frame_state().lock() {
        state.active_popups = std::mem::take(&mut state.next_popups);
    }
}

fn current_input_snapshot(platform: &SharedPlatformState) -> mlua::Result<UiInputSnapshot> {
    let platform = platform
        .lock()
        .map_err(|_| mlua::Error::external("platform lock poisoned"))?;
    let mouse = platform.mouse();
    Ok(UiInputSnapshot {
        mouse: Vec2 {
            x: mouse.x,
            y: mouse.y,
        },
        input: platform.input().clone(),
        window: platform.window(),
    })
}

fn get_entity_draw_context(entity: &Table) -> mlua::Result<EntityDrawContext> {
    let (x, y, rotation) = crate::window::get_global_transform(entity)?;
    let (w, h) = crate::window::get_global_size(entity)?;
    let (bounds, pivot) = if crate::window::uses_middle_pivot(entity) {
        let (px, py) = crate::window::get_global_rotation_pivot(entity)?;
        (
            Rect {
                x: px - w * 0.5,
                y: py - h * 0.5,
                w,
                h,
            },
            Vec2 { x: px, y: py },
        )
    } else {
        (Rect { x, y, w, h }, Vec2 { x, y })
    };
    Ok(EntityDrawContext {
        bounds,
        pivot,
        rotation,
    })
}

fn rect_offset(bounds: Rect, pivot: Vec2) -> Vec2 {
    Vec2 {
        x: if bounds.w.abs() <= f32::EPSILON {
            0.0
        } else {
            (pivot.x - bounds.x) / bounds.w
        },
        y: if bounds.h.abs() <= f32::EPSILON {
            0.0
        } else {
            (pivot.y - bounds.y) / bounds.h
        },
    }
}

fn local_point_to_world(bounds: Rect, pivot: Vec2, rotation: f32, lx: f32, ly: f32) -> Vec2 {
    let world_x = bounds.x + lx;
    let world_y = bounds.y + ly;
    let (rx, ry) = rotate_local(world_x - pivot.x, world_y - pivot.y, rotation);
    Vec2 {
        x: pivot.x + rx,
        y: pivot.y + ry,
    }
}

fn world_point_to_local(point: Vec2, pivot: Vec2, rotation: f32) -> Vec2 {
    let (local_x, local_y) = rotate_local(point.x - pivot.x, point.y - pivot.y, -rotation);
    Vec2 {
        x: pivot.x + local_x,
        y: pivot.y + local_y,
    }
}

fn point_in_bounds(point: Vec2, bounds: Rect, pivot: Vec2, rotation: f32) -> bool {
    let local = world_point_to_local(point, pivot, rotation);
    let sample_x = local.x;
    let sample_y = local.y;
    sample_x >= bounds.x
        && sample_x <= bounds.x + bounds.w
        && sample_y >= bounds.y
        && sample_y <= bounds.y + bounds.h
}

fn component_owner_key(entity: &Table, component: &Table) -> String {
    let entity_id = entity.get::<i64>("id").unwrap_or(0);
    let name = component
        .get::<String>("__neolove_component")
        .unwrap_or_else(|_| "component".to_string());
    format!("{entity_id}:{name}")
}

fn register_popup(owner: String, bounds: Rect, pivot: Vec2, rotation: f32) {
    let popup = UiPopupRegion {
        owner,
        bounds,
        pivot,
        rotation,
    };
    if let Ok(mut state) = ui_frame_state().lock() {
        upsert_popup(&mut state.active_popups, popup.clone());
        upsert_popup(&mut state.next_popups, popup);
    }
}

fn point_blocked_by_popup(point: Vec2, owner: &str) -> bool {
    if let Ok(state) = ui_frame_state().lock() {
        state.active_popups.iter().any(|popup| {
            popup.owner != owner
                && point_in_bounds(point, popup.bounds, popup.pivot, popup.rotation)
        })
    } else {
        false
    }
}

fn queue_rect_fill(
    renderer: &mut RenderState,
    bounds: Rect,
    pivot: Vec2,
    rotation: f32,
    color: Color,
) {
    if bounds.w <= 0.0 || bounds.h <= 0.0 || color.a == 0 {
        return;
    }
    renderer.queue(DrawCommand::Rect {
        x: bounds.x,
        y: bounds.y,
        w: bounds.w,
        h: bounds.h,
        rotation,
        offset: rect_offset(bounds, pivot),
        color,
    });
}

fn queue_local_triangle(
    renderer: &mut RenderState,
    bounds: Rect,
    pivot: Vec2,
    rotation: f32,
    color: Color,
    a: (f32, f32),
    b: (f32, f32),
    c: (f32, f32),
) {
    renderer.queue(DrawCommand::Triangle {
        a: local_point_to_world(bounds, pivot, rotation, a.0, a.1),
        b: local_point_to_world(bounds, pivot, rotation, b.0, b.1),
        c: local_point_to_world(bounds, pivot, rotation, c.0, c.1),
        color,
    });
}

fn queue_corner_fan_fill(
    renderer: &mut RenderState,
    bounds: Rect,
    pivot: Vec2,
    rotation: f32,
    color: Color,
    center_x: f32,
    center_y: f32,
    radius: f32,
    start_angle: f32,
    end_angle: f32,
) {
    let segments = ((radius * 0.85).ceil() as usize).clamp(6, 24);
    let mut previous = (
        center_x + start_angle.cos() * radius,
        center_y + start_angle.sin() * radius,
    );

    for index in 1..=segments {
        let t = index as f32 / segments as f32;
        let angle = start_angle + (end_angle - start_angle) * t;
        let next = (
            center_x + angle.cos() * radius,
            center_y + angle.sin() * radius,
        );
        queue_local_triangle(
            renderer,
            bounds,
            pivot,
            rotation,
            color,
            (center_x, center_y),
            previous,
            next,
        );
        previous = next;
    }
}

fn queue_rounded_rect_fill(
    renderer: &mut RenderState,
    bounds: Rect,
    pivot: Vec2,
    rotation: f32,
    color: Color,
    radius: f32,
) {
    if bounds.w <= 0.0 || bounds.h <= 0.0 || color.a == 0 {
        return;
    }

    let radius = radius.max(0.0).min(bounds.w.min(bounds.h) * 0.5);
    if radius <= 0.5 {
        queue_rect_fill(renderer, bounds, pivot, rotation, color);
        return;
    }

    queue_rect_fill(
        renderer,
        Rect {
            x: bounds.x + radius,
            y: bounds.y,
            w: (bounds.w - radius * 2.0).max(0.0),
            h: bounds.h,
        },
        pivot,
        rotation,
        color,
    );
    queue_rect_fill(
        renderer,
        Rect {
            x: bounds.x,
            y: bounds.y + radius,
            w: radius,
            h: (bounds.h - radius * 2.0).max(0.0),
        },
        pivot,
        rotation,
        color,
    );
    queue_rect_fill(
        renderer,
        Rect {
            x: bounds.x + bounds.w - radius,
            y: bounds.y + radius,
            w: radius,
            h: (bounds.h - radius * 2.0).max(0.0),
        },
        pivot,
        rotation,
        color,
    );

    queue_corner_fan_fill(
        renderer,
        bounds,
        pivot,
        rotation,
        color,
        radius,
        radius,
        radius,
        std::f32::consts::PI,
        std::f32::consts::FRAC_PI_2 * 3.0,
    );
    queue_corner_fan_fill(
        renderer,
        bounds,
        pivot,
        rotation,
        color,
        bounds.w - radius,
        radius,
        radius,
        std::f32::consts::FRAC_PI_2 * 3.0,
        std::f32::consts::TAU,
    );
    queue_corner_fan_fill(
        renderer,
        bounds,
        pivot,
        rotation,
        color,
        radius,
        bounds.h - radius,
        radius,
        std::f32::consts::FRAC_PI_2,
        std::f32::consts::PI,
    );
    queue_corner_fan_fill(
        renderer,
        bounds,
        pivot,
        rotation,
        color,
        bounds.w - radius,
        bounds.h - radius,
        radius,
        0.0,
        std::f32::consts::FRAC_PI_2,
    );
}

fn inset_rect(bounds: Rect, inset: f32) -> Rect {
    let inset = inset.max(0.0);
    Rect {
        x: bounds.x + inset,
        y: bounds.y + inset,
        w: (bounds.w - inset * 2.0).max(0.0),
        h: (bounds.h - inset * 2.0).max(0.0),
    }
}

fn get_color_field(component: &Table, key: &str) -> Option<Color> {
    component
        .get::<Table>(key)
        .ok()
        .and_then(|table| color4_to_color(table).ok())
}

fn get_string_field(component: &Table, snake_case: &str, camel_case: &str) -> Option<String> {
    component
        .get::<String>(snake_case)
        .or_else(|_| component.get::<String>(camel_case))
        .ok()
}

fn get_number_field(component: &Table, snake_case: &str, camel_case: &str) -> Option<f32> {
    component
        .get::<f32>(snake_case)
        .or_else(|_| component.get::<f32>(camel_case))
        .ok()
        .filter(|value| value.is_finite())
}

fn get_image_field(component: &Table, key: &str) -> mlua::Result<Option<ImageHandle>> {
    let image: Option<AnyUserData> = component.get(key).unwrap_or(None);
    let Some(image) = image else {
        return Ok(None);
    };
    let image = image.borrow::<ImageHandle>()?;
    image.ensure_uploaded()?;
    Ok(Some(image.clone()))
}

fn get_image_field_any(component: &Table, keys: &[&str]) -> mlua::Result<Option<ImageHandle>> {
    for key in keys {
        if let Some(image) = get_image_field(component, key)? {
            return Ok(Some(image));
        }
    }
    Ok(None)
}

fn get_number_key(component: &Table, key: &str) -> Option<f32> {
    component
        .get::<f32>(key)
        .ok()
        .filter(|value| value.is_finite())
}

fn get_string_key(component: &Table, key: &str) -> Option<String> {
    component.get::<String>(key).ok()
}

fn get_source_rect(component: &Table, prefix: &str) -> Option<Rect> {
    let x = get_number_key(component, &format!("{prefix}_x")).unwrap_or(0.0);
    let y = get_number_key(component, &format!("{prefix}_y")).unwrap_or(0.0);
    let w = get_number_key(component, &format!("{prefix}_w"))
        .or_else(|| get_number_key(component, &format!("{prefix}_width")))?;
    let h = get_number_key(component, &format!("{prefix}_h"))
        .or_else(|| get_number_key(component, &format!("{prefix}_height")))?;
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    Some(Rect { x, y, w, h })
}

fn parse_icon_side(raw: &str) -> UiIconSide {
    match raw.trim().to_ascii_lowercase().as_str() {
        "right" | "end" => UiIconSide::Right,
        _ => UiIconSide::Left,
    }
}

fn build_inline_image(
    bounds: Rect,
    image: ImageHandle,
    tint: Color,
    source: Option<Rect>,
    side: UiIconSide,
    width: f32,
    height: f32,
    gap: f32,
) -> Option<UiInlineImage> {
    if bounds.w <= 0.0 || bounds.h <= 0.0 {
        return None;
    }

    let width = width.max(0.0).min(bounds.w);
    let height = height.max(0.0).min(bounds.h);
    if width <= 0.0 || height <= 0.0 {
        return None;
    }

    Some(UiInlineImage {
        image,
        tint,
        source,
        width,
        height,
        gap: gap.max(0.0),
        side,
    })
}

fn resolve_widget_icon(
    component: &Table,
    bounds: Rect,
    default_tint: Color,
) -> mlua::Result<Option<UiInlineImage>> {
    let Some(image) = get_image_field_any(component, &["icon_image", "content_image"])? else {
        return Ok(None);
    };

    let tint = get_color_field(component, "icon_color")
        .or_else(|| get_color_field(component, "content_image_color"))
        .unwrap_or(default_tint);
    let size = get_number_key(component, "icon_size")
        .or_else(|| get_number_key(component, "content_image_size"))
        .unwrap_or(0.0)
        .max(0.0);
    let width = get_number_key(component, "icon_width")
        .or_else(|| get_number_key(component, "content_image_width"))
        .unwrap_or(size)
        .max(0.0);
    let height = get_number_key(component, "icon_height")
        .or_else(|| get_number_key(component, "content_image_height"))
        .unwrap_or(size)
        .max(0.0);
    let width = if width > 0.0 {
        width
    } else {
        bounds.h.max(0.0)
    };
    let height = if height > 0.0 {
        height
    } else {
        bounds.h.max(0.0)
    };
    let gap = get_number_key(component, "icon_gap")
        .or_else(|| get_number_key(component, "content_image_gap"))
        .unwrap_or(8.0)
        .max(0.0);
    let side = parse_icon_side(
        &get_string_key(component, "icon_side")
            .or_else(|| get_string_key(component, "content_image_side"))
            .unwrap_or_else(|| "left".to_string()),
    );
    let source = get_source_rect(component, "icon_source")
        .or_else(|| get_source_rect(component, "content_image_source"));

    Ok(build_inline_image(
        bounds, image, tint, source, side, width, height, gap,
    ))
}

fn layout_inline_image(
    bounds: Rect,
    image: Option<UiInlineImage>,
) -> (Rect, Option<UiInlineImageLayout>) {
    let Some(image) = image else {
        return (bounds, None);
    };

    let draw_bounds = Rect {
        x: if image.side == UiIconSide::Left {
            bounds.x
        } else {
            bounds.x + bounds.w - image.width
        },
        y: bounds.y + (bounds.h - image.height) * 0.5,
        w: image.width,
        h: image.height,
    };
    let consume = (image.width + image.gap).min(bounds.w).max(0.0);
    let text_bounds = match image.side {
        UiIconSide::Left => Rect {
            x: bounds.x + consume,
            y: bounds.y,
            w: (bounds.w - consume).max(0.0),
            h: bounds.h,
        },
        UiIconSide::Right => Rect {
            x: bounds.x,
            y: bounds.y,
            w: (bounds.w - consume).max(0.0),
            h: bounds.h,
        },
    };

    (
        text_bounds,
        Some(UiInlineImageLayout {
            image: image.image,
            tint: image.tint,
            source: image.source,
            bounds: draw_bounds,
        }),
    )
}

fn queue_inline_image(
    renderer: &mut RenderState,
    draw: &EntityDrawContext,
    image: &UiInlineImageLayout,
    filter: TextureFilter,
) {
    renderer.queue(DrawCommand::Image {
        image: image.image.clone(),
        dest: image.bounds,
        source: image.source,
        rotation: draw.rotation,
        pivot: draw.pivot,
        tint: image.tint,
        filter,
    });
}

fn queue_nine_slice(
    renderer: &mut RenderState,
    image: ImageHandle,
    bounds: Rect,
    pivot: Vec2,
    rotation: f32,
    tint: Color,
    filter: TextureFilter,
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
) -> mlua::Result<()> {
    if bounds.w <= 0.0 || bounds.h <= 0.0 {
        return Ok(());
    }

    let (image_w, image_h) = image.dimensions()?;
    let image_w = image_w as f32;
    let image_h = image_h as f32;
    let left = left.max(0.0).min(image_w);
    let right = right.max(0.0).min((image_w - left).max(0.0));
    let top = top.max(0.0).min(image_h);
    let bottom = bottom.max(0.0).min((image_h - top).max(0.0));

    if left <= 0.0 && right <= 0.0 && top <= 0.0 && bottom <= 0.0 {
        renderer.queue(DrawCommand::Image {
            image,
            dest: bounds,
            source: None,
            rotation,
            pivot,
            tint,
            filter,
        });
        return Ok(());
    }

    let width_scale = if left + right > bounds.w && left + right > 0.0 {
        bounds.w / (left + right)
    } else {
        1.0
    };
    let height_scale = if top + bottom > bounds.h && top + bottom > 0.0 {
        bounds.h / (top + bottom)
    } else {
        1.0
    };
    let dest_left = left * width_scale;
    let dest_right = right * width_scale;
    let dest_top = top * height_scale;
    let dest_bottom = bottom * height_scale;
    let center_src_w = (image_w - left - right).max(0.0);
    let center_src_h = (image_h - top - bottom).max(0.0);
    let center_dest_w = (bounds.w - dest_left - dest_right).max(0.0);
    let center_dest_h = (bounds.h - dest_top - dest_bottom).max(0.0);

    let source_columns = [(0.0, left), (left, center_src_w), (image_w - right, right)];
    let source_rows = [(0.0, top), (top, center_src_h), (image_h - bottom, bottom)];
    let dest_columns = [
        (bounds.x, dest_left),
        (bounds.x + dest_left, center_dest_w),
        (bounds.x + bounds.w - dest_right, dest_right),
    ];
    let dest_rows = [
        (bounds.y, dest_top),
        (bounds.y + dest_top, center_dest_h),
        (bounds.y + bounds.h - dest_bottom, dest_bottom),
    ];

    for (row, (src_y, src_h)) in source_rows.iter().enumerate() {
        for (col, (src_x, src_w)) in source_columns.iter().enumerate() {
            let (dest_x, dest_w) = dest_columns[col];
            let (dest_y, dest_h) = dest_rows[row];
            if *src_w <= 0.0 || *src_h <= 0.0 || dest_w <= 0.0 || dest_h <= 0.0 {
                continue;
            }

            renderer.queue(DrawCommand::Image {
                image: image.clone(),
                dest: Rect {
                    x: dest_x,
                    y: dest_y,
                    w: dest_w,
                    h: dest_h,
                },
                source: Some(Rect {
                    x: *src_x,
                    y: *src_y,
                    w: *src_w,
                    h: *src_h,
                }),
                rotation,
                pivot,
                tint,
                filter,
            });
        }
    }

    Ok(())
}

fn resolve_panel_style(
    ctx: &Lua,
    component: &Table,
    background_color: Color,
    border_color: Color,
) -> mlua::Result<UiPanelStyle> {
    Ok(UiPanelStyle {
        background_color,
        border_color,
        border_width: get_number_field(component, "border_width", "borderWidth")
            .unwrap_or(0.0)
            .max(0.0),
        corner_radius: get_number_field(component, "corner_radius", "cornerRadius")
            .unwrap_or(0.0)
            .max(0.0),
        background_image: get_image_field(component, "background_image")?,
        slice_left: get_number_field(component, "slice_left", "sliceLeft").unwrap_or(0.0),
        slice_right: get_number_field(component, "slice_right", "sliceRight").unwrap_or(0.0),
        slice_top: get_number_field(component, "slice_top", "sliceTop").unwrap_or(0.0),
        slice_bottom: get_number_field(component, "slice_bottom", "sliceBottom").unwrap_or(0.0),
        filter: app_texture_filter(ctx),
    })
}

fn render_panel(
    renderer: &mut RenderState,
    bounds: Rect,
    pivot: Vec2,
    rotation: f32,
    style: &UiPanelStyle,
) -> mlua::Result<()> {
    if let Some(image) = style.background_image.clone() {
        queue_nine_slice(
            renderer,
            image,
            bounds,
            pivot,
            rotation,
            style.background_color,
            style.filter,
            style.slice_left,
            style.slice_right,
            style.slice_top,
            style.slice_bottom,
        )?;
        return Ok(());
    }

    queue_panel_fill(renderer, bounds, pivot, rotation, style);
    Ok(())
}

fn queue_panel_fill(
    renderer: &mut RenderState,
    bounds: Rect,
    pivot: Vec2,
    rotation: f32,
    style: &UiPanelStyle,
) {
    if style.border_width > 0.0 {
        queue_rounded_rect_fill(
            renderer,
            bounds,
            pivot,
            rotation,
            style.border_color,
            style.corner_radius,
        );
        let inner = inset_rect(bounds, style.border_width);
        if inner.w > 0.0 && inner.h > 0.0 {
            queue_rounded_rect_fill(
                renderer,
                inner,
                pivot,
                rotation,
                style.background_color,
                (style.corner_radius - style.border_width).max(0.0),
            );
        }
    } else {
        queue_rounded_rect_fill(
            renderer,
            bounds,
            pivot,
            rotation,
            style.background_color,
            style.corner_radius,
        );
    }
}

fn build_text_request(
    root: &Path,
    component: &Table,
    text: String,
    bounds: Rect,
    pivot: Vec2,
    rotation: f32,
    color: Color,
    default_scale: f32,
    default_align_x: TextAlignX,
    default_align_y: TextAlignY,
    default_text_scale: TextScaleMode,
    default_wrap: TextWrapMode,
    padding_x: f32,
    padding_y: f32,
) -> TextRenderRequest {
    let align_x = get_string_field(component, "align_x", "alignX")
        .map(|value| parse_align_x(&value))
        .unwrap_or(default_align_x);
    let align_y = get_string_field(component, "align_y", "alignY")
        .or_else(|| get_string_field(component, "vertical_align", "verticalAlign"))
        .map(|value| parse_align_y(&value))
        .unwrap_or(default_align_y);
    let text_scale = get_string_field(component, "text_scale", "textScale")
        .map(|value| parse_text_scale_mode(&value))
        .unwrap_or(default_text_scale);
    let wrap = match component.get::<Value>("wrap").ok() {
        Some(value @ Value::Boolean(_)) | Some(value @ Value::String(_)) => parse_wrap_mode(value),
        _ => default_wrap,
    };

    TextRenderRequest {
        text,
        bounds,
        rotation,
        pivot,
        color,
        font: parse_font_handle(root, component.get::<Value>("font").unwrap_or(Value::Nil)),
        scale: component
            .get::<f32>("scale")
            .unwrap_or(default_scale)
            .max(1.0),
        min_scale: component.get::<f32>("min_scale").unwrap_or(1.0).max(1.0),
        text_scale,
        align_x,
        align_y,
        wrap,
        padding_x: padding_x.max(0.0),
        padding_y: padding_y.max(0.0),
        line_spacing: component.get::<f32>("line_spacing").unwrap_or(1.0),
        letter_spacing: component.get::<f32>("letter_spacing").unwrap_or(0.0),
        stretch_width: 0.0,
        stretch_height: 0.0,
    }
}

fn measure_inline_text(root: &Path, component: &Table, text: &str, scale: Option<f32>) -> f32 {
    let mut request = build_text_request(
        root,
        component,
        text.to_string(),
        Rect::default(),
        Vec2::default(),
        0.0,
        Color::WHITE,
        component.get::<f32>("scale").unwrap_or(18.0).max(1.0),
        TextAlignX::Left,
        TextAlignY::Top,
        TextScaleMode::None,
        TextWrapMode::None,
        0.0,
        0.0,
    );
    if let Some(scale) = scale {
        request.scale = scale.max(1.0);
    }
    crate::renderer::measure_text(&request)
        .map(|metrics| metrics.width)
        .unwrap_or(0.0)
}

fn char_count(text: &str) -> usize {
    text.chars().count()
}

fn char_to_byte_index(text: &str, index: usize) -> usize {
    if index == 0 {
        return 0;
    }
    text.char_indices()
        .nth(index)
        .map(|(byte, _)| byte)
        .unwrap_or(text.len())
}

fn slice_chars(text: &str, start: usize, end: usize) -> String {
    if start >= end {
        return String::new();
    }
    let start_byte = char_to_byte_index(text, start);
    let end_byte = char_to_byte_index(text, end);
    text[start_byte..end_byte].to_string()
}

fn replace_char_range(text: &str, start: usize, end: usize, replacement: &str) -> String {
    let start_byte = char_to_byte_index(text, start);
    let end_byte = char_to_byte_index(text, end);
    let mut output = String::with_capacity(text.len() + replacement.len());
    output.push_str(&text[..start_byte]);
    output.push_str(replacement);
    output.push_str(&text[end_byte..]);
    output
}

fn value_to_option_string(value: Value) -> Option<String> {
    match value {
        Value::String(value) => value.to_str().ok().map(|value| value.to_string()),
        Value::Integer(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        Value::Boolean(value) => Some(value.to_string()),
        _ => None,
    }
}

fn get_table_value_string(table: &Table, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Ok(value) = table.get::<Value>(*key) {
            if let Some(value) = value_to_option_string(value) {
                return Some(value);
            }
        }
    }
    None
}

fn read_ui_list_items(table: Option<Table>) -> mlua::Result<Vec<UiListItem>> {
    let Some(table) = table else {
        return Ok(Vec::new());
    };

    let mut items = Vec::new();
    for value in table.sequence_values::<Value>() {
        let value = value?;
        let item = match value {
            Value::Table(table) => {
                let text = get_table_value_string(&table, &["text", "label", "name", "value"])
                    .unwrap_or_default();
                if text.is_empty() {
                    None
                } else {
                    let value = get_table_value_string(&table, &["value", "id"])
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| text.clone());
                    let image = if let Some(image) = get_image_field(&table, "image")? {
                        Some(image)
                    } else {
                        get_image_field(&table, "icon")?
                    };
                    let image_tint = get_color_field(&table, "image_color")
                        .or_else(|| get_color_field(&table, "icon_color"))
                        .unwrap_or(Color::WHITE);
                    let image_source = get_source_rect(&table, "image_source")
                        .or_else(|| get_source_rect(&table, "icon_source"));
                    Some(UiListItem {
                        text,
                        value,
                        image,
                        image_tint,
                        image_source,
                    })
                }
            }
            other => value_to_option_string(other).map(|text| UiListItem {
                value: text.clone(),
                text,
                image: None,
                image_tint: Color::WHITE,
                image_source: None,
            }),
        };

        if let Some(item) = item {
            items.push(item);
        }
    }

    Ok(items)
}

fn consume_wheel_steps(
    component: &Table,
    accumulator_key: &str,
    wheel_delta: f32,
    max_steps_per_frame: i32,
) -> mlua::Result<i32> {
    let mut accumulator = component.get::<f32>(accumulator_key).unwrap_or(0.0) + wheel_delta;
    let mut steps = 0i32;
    let limit = max_steps_per_frame.max(1);

    while accumulator >= 1.0 && steps < limit {
        accumulator -= 1.0;
        steps += 1;
    }
    while accumulator <= -1.0 && steps > -limit {
        accumulator += 1.0;
        steps -= 1;
    }

    component.set(accumulator_key, accumulator)?;
    Ok(steps)
}

fn call_component_callback(component: &Table, entity: &Table, name: &str) -> mlua::Result<()> {
    if let Ok(callback) = component.get::<Function>(name) {
        protect_lua_call(&format!("running component callback '{name}'"), || {
            callback.call::<()>((entity.clone(), component.clone()))
        })?;
    }
    Ok(())
}

fn call_component_string_callback(
    component: &Table,
    entity: &Table,
    name: &str,
    value: &str,
) -> mlua::Result<()> {
    if let Ok(callback) = component.get::<Function>(name) {
        let value = value.to_string();
        protect_lua_call(&format!("running component callback '{name}'"), || {
            callback.call::<()>((entity.clone(), component.clone(), value.clone()))
        })?;
    }
    Ok(())
}

fn call_component_selection_callback(
    component: &Table,
    entity: &Table,
    name: &str,
    index: usize,
    value: &str,
) -> mlua::Result<()> {
    if let Ok(callback) = component.get::<Function>(name) {
        let value = value.to_string();
        protect_lua_call(&format!("running component callback '{name}'"), || {
            callback.call::<()>((entity.clone(), component.clone(), index, value.clone()))
        })?;
    }
    Ok(())
}

fn create_basic_drawable(lua: &Lua) -> mlua::Result<Table> {
    let drawable = lua.create_table()?;
    drawable.set(
        "awake",
        lua.create_function(move |ctx, (_entity, component): (Table, Table)| {
            component.set("color", color4(ctx, 255, 255, 255, 255)?)?;
            component.set("visible", true)?;
            Ok(())
        })?,
    )?;
    drawable.set("NEOLOVE_RENDERING", true)?;
    Ok(drawable)
}

pub fn add_core_components(
    lua: &Lua,
    platform: SharedPlatformState,
    render_state: SharedRenderState,
    env_root: PathBuf,
) -> mlua::Result<()> {
    let core_components = lua.create_table()?;

    // Color4
    // not a component!? helper function to generate color4 values
    {
        lua.globals().set(
            "Color4",
            lua.create_function(move |ctx, (r, g, b, a): (f32, f32, f32, Option<f32>)| {
                let alpha: f32 = a.unwrap_or(255.0);
                color4(
                    ctx,
                    r.clamp(0.0, 255.0) as u8,
                    g.clamp(0.0, 255.0) as u8,
                    b.clamp(0.0, 255.0) as u8,
                    alpha.clamp(0.0, 255.0) as u8,
                )
            })?,
        )?;
    }

    // Rect2d
    // basic renderer
    {
        let rect2d = create_basic_drawable(lua)?;
        let render_state = render_state.clone();
        rect2d.set(
            "update",
            lua.create_function(move |_ctx, (entity, component, _dt): (Table, Table, f32)| {
                if !component.get::<bool>("visible").unwrap_or(true) {
                    return Ok(());
                }
                let (x, y, rotation) = crate::window::get_global_transform(&entity)?;
                let (w, h) = crate::window::get_global_size(&entity)?;
                let color = color4_to_color(component.get("color")?)?;
                let use_middle_pivot = crate::window::uses_middle_pivot(&entity);
                let (draw_x, draw_y, offset) = if use_middle_pivot {
                    let (px, py) = crate::window::get_global_rotation_pivot(&entity)?;
                    (px, py, Vec2 { x: 0.5, y: 0.5 })
                } else {
                    (x, y, Vec2 { x: 0.0, y: 0.0 })
                };
                let mut renderer = render_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?;
                renderer.queue(DrawCommand::Rect {
                    x: draw_x,
                    y: draw_y,
                    w,
                    h,
                    rotation,
                    offset,
                    color,
                });
                Ok(())
            })?,
        )?;

        core_components.set("Rect2D", rect2d)?;
    }

    // Shape2D
    // renderer for box, circle, and right-triangle primitives
    {
        let shape2d = create_basic_drawable(lua)?;
        let render_state = render_state.clone();
        shape2d.set(
            "awake",
            lua.create_function(move |ctx, (_entity, component): (Table, Table)| {
                component.set("color", color4(ctx, 255, 255, 255, 255)?)?;
                component.set("visible", true)?;
                component.set("shape", "box")?;
                component.set("triangle_corner", "bl")?;
                component.set("offset_x", 0.0)?;
                component.set("offset_y", 0.0)?;
                component.set("size_x", 0.0)?;
                component.set("size_y", 0.0)?;
                Ok(())
            })?,
        )?;
        shape2d.set(
            "update",
            lua.create_function(move |_ctx, (entity, component, _dt): (Table, Table, f32)| {
                if !component.get::<bool>("visible").unwrap_or(true) {
                    return Ok(());
                }
                let (origin_x, origin_y, rotation) = crate::window::get_global_transform(&entity)?;
                let (entity_w, entity_h) = crate::window::get_global_size(&entity)?;
                let entity_scale = crate::window::get_global_scale(&entity)?;
                let offset_x = component.get::<f32>("offset_x").unwrap_or(0.0);
                let offset_y = component.get::<f32>("offset_y").unwrap_or(0.0);
                let draw_w = {
                    let w = component.get::<f32>("size_x").unwrap_or(0.0);
                    if w > 0.0 { w * entity_scale } else { entity_w }
                };
                let draw_h = {
                    let h = component.get::<f32>("size_y").unwrap_or(0.0);
                    if h > 0.0 { h * entity_scale } else { entity_h }
                };
                if draw_w <= 0.0 || draw_h <= 0.0 {
                    return Ok(());
                }

                let color = color4_to_color(component.get("color")?)?;
                let shape = component
                    .get::<String>("shape")
                    .unwrap_or_else(|_| "box".to_string())
                    .to_ascii_lowercase();
                let triangle_corner = component
                    .get::<String>("triangle_corner")
                    .unwrap_or_else(|_| "bl".to_string())
                    .to_ascii_lowercase();

                let to_world = |lx: f32, ly: f32| -> Vec2 {
                    let (rx, ry) = rotate_local(lx, ly, rotation);
                    Vec2 {
                        x: origin_x + rx,
                        y: origin_y + ry,
                    }
                };

                let mut renderer = render_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?;

                match shape.as_str() {
                    "circle" => {
                        let center_local_x = offset_x + draw_w * 0.5;
                        let center_local_y = offset_y + draw_h * 0.5;
                        let center = to_world(center_local_x, center_local_y);
                        let radius = (draw_w.min(draw_h) * 0.5).max(0.0001);
                        renderer.queue(DrawCommand::Circle {
                            center,
                            radius,
                            color,
                        });
                    }
                    "triangle" | "right_triangle" | "righttriangle" | "rightangledtriangle" => {
                        let x0 = offset_x;
                        let y0 = offset_y;
                        let x1 = offset_x + draw_w;
                        let y1 = offset_y + draw_h;
                        let (a, b, c) = match triangle_corner.as_str() {
                            "br" | "bottomright" | "rightbottom" => {
                                (to_world(x1, y1), to_world(x1, y0), to_world(x0, y1))
                            }
                            "tl" | "topleft" | "lefttop" => {
                                (to_world(x0, y0), to_world(x1, y0), to_world(x0, y1))
                            }
                            "tr" | "topright" | "righttop" => {
                                (to_world(x1, y0), to_world(x0, y0), to_world(x1, y1))
                            }
                            _ => (to_world(x0, y1), to_world(x0, y0), to_world(x1, y1)),
                        };
                        renderer.queue(DrawCommand::Triangle { a, b, c, color });
                    }
                    _ => {
                        let p0 = to_world(offset_x, offset_y);
                        let p1 = to_world(offset_x + draw_w, offset_y);
                        let p2 = to_world(offset_x + draw_w, offset_y + draw_h);
                        let p3 = to_world(offset_x, offset_y + draw_h);
                        renderer.queue(DrawCommand::Triangle {
                            a: p0,
                            b: p1,
                            c: p2,
                            color,
                        });
                        renderer.queue(DrawCommand::Triangle {
                            a: p0,
                            b: p2,
                            c: p3,
                            color,
                        });
                    }
                }
                Ok(())
            })?,
        )?;

        core_components.set("Shape2D", shape2d)?;
    }

    // TextBox
    // bounded text with optional auto-fit scaling, alignment, wrapping, and font selection
    {
        let textbox = create_basic_drawable(lua)?;
        let render_state = render_state.clone();
        let text_root = env_root.clone();
        textbox.set(
            "awake",
            lua.create_function(move |ctx, (_entity, component): (Table, Table)| {
                component.set("color", color4(ctx, 255, 255, 255, 255)?)?;
                component.set("visible", true)?;
                component.set("__neolove_component", "TextBox")?;
                component.set("text", "Text Box")?;
                component.set("scale", 32.0)?;
                component.set("min_scale", 1.0)?;
                component.set("used_scale", 32.0)?;
                component.set("text_scale", "none")?;
                component.set("align_x", "left")?;
                component.set("align_y", "top")?;
                component.set("wrap", "none")?;
                component.set("size_mode", "content")?;
                component.set("padding", 0.0)?;
                component.set("padding_x", 0.0)?;
                component.set("padding_y", 0.0)?;
                component.set("line_spacing", 1.0)?;
                component.set("letter_spacing", 0.0)?;
                component.set("font", Value::Nil)?;
                component.set("scale_x", 0.0)?;
                component.set("scale_y", 0.0)?;
                component.set("dx", 0.0)?;
                component.set("dy", 0.0)?;
                component.set("line_count", 0)?;
                Ok(())
            })?,
        )?;

        textbox.set(
            "update",
            lua.create_function(move |_ctx, (entity, component, _dt): (Table, Table, f32)| {
                if !component.get::<bool>("visible").unwrap_or(true) {
                    return Ok(());
                }

                let (x, y, rotation) = crate::window::get_global_transform(&entity)?;
                let text = component
                    .get::<String>("text")
                    .unwrap_or_else(|_| String::new());
                let scale = component.get::<f32>("scale").unwrap_or(32.0).max(1.0);
                let min_scale = component.get::<f32>("min_scale").unwrap_or(1.0).max(1.0);
                let color: Color = color4_to_color(component.get("color")?)?;
                let padding = component.get::<f32>("padding").unwrap_or(0.0).max(0.0);
                let padding_x = component
                    .get::<f32>("padding_x")
                    .unwrap_or(padding)
                    .max(0.0);
                let padding_y = component
                    .get::<f32>("padding_y")
                    .unwrap_or(padding)
                    .max(0.0);
                let line_spacing = component.get::<f32>("line_spacing").unwrap_or(1.0);
                let letter_spacing = component.get::<f32>("letter_spacing").unwrap_or(0.0);
                let align_x = parse_align_x(
                    &component
                        .get::<String>("align_x")
                        .or_else(|_| component.get::<String>("align"))
                        .unwrap_or_else(|_| "left".to_string()),
                );
                let align_y = parse_align_y(
                    &component
                        .get::<String>("align_y")
                        .or_else(|_| component.get::<String>("vertical_align"))
                        .unwrap_or_else(|_| "top".to_string()),
                );
                let text_scale = parse_text_scale_mode(
                    &component
                        .get::<String>("text_scale")
                        .or_else(|_| component.get::<String>("textScale"))
                        .unwrap_or_else(|_| "none".to_string()),
                );
                let wrap = parse_wrap_mode(component.get::<Value>("wrap").unwrap_or(Value::Nil));
                let size_mode_uses_entity = uses_entity_text_bounds(&component);
                let legacy_scale_x = component.get::<f32>("scale_x").unwrap_or(0.0);
                let legacy_scale_y = component.get::<f32>("scale_y").unwrap_or(0.0);
                let use_legacy_stretch =
                    !size_mode_uses_entity && legacy_scale_x > 0.0 && legacy_scale_y > 0.0;
                let font = parse_font_handle(
                    &text_root,
                    component.get::<Value>("font").unwrap_or(Value::Nil),
                );
                let effective_scale = if use_legacy_stretch {
                    legacy_scale_y.max(1.0)
                } else {
                    scale
                };

                let (bounds, pivot) = if size_mode_uses_entity {
                    let (w, h) = crate::window::get_global_size(&entity)?;
                    if crate::window::uses_middle_pivot(&entity) {
                        let (px, py) = crate::window::get_global_rotation_pivot(&entity)?;
                        (
                            Rect {
                                x: px - w * 0.5,
                                y: py - h * 0.5,
                                w,
                                h,
                            },
                            Vec2 { x: px, y: py },
                        )
                    } else {
                        (Rect { x, y, w, h }, Vec2 { x, y })
                    }
                } else {
                    (
                        Rect {
                            x,
                            y,
                            w: 0.0,
                            h: 0.0,
                        },
                        Vec2 { x, y },
                    )
                };

                let request = TextRenderRequest {
                    text,
                    bounds,
                    rotation,
                    pivot,
                    color,
                    font,
                    scale: effective_scale,
                    min_scale,
                    text_scale,
                    align_x,
                    align_y,
                    wrap,
                    padding_x,
                    padding_y,
                    line_spacing,
                    letter_spacing,
                    stretch_width: if use_legacy_stretch {
                        legacy_scale_x
                    } else {
                        0.0
                    },
                    stretch_height: if use_legacy_stretch {
                        legacy_scale_y
                    } else {
                        0.0
                    },
                };

                let metrics = crate::renderer::measure_text(&request).unwrap_or_default();
                component.set("dx", metrics.width)?;
                component.set("dy", metrics.height)?;
                component.set("used_scale", metrics.used_scale)?;
                component.set("line_count", metrics.line_count)?;

                let mut renderer = render_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?;
                renderer.queue(DrawCommand::Text(request));

                Ok(())
            })?,
        )?;

        core_components.set("TextBox", textbox.clone())?;
        core_components.set("TextLabel", textbox.clone())?;
        core_components.set("RudimentaryTextLabel", textbox)?;
    }

    // Legacy interactive UI components are intentionally disabled.
    #[cfg(any())]
    {
    // Frame
    // customizable UI panel with borders, rounded corners, and optional 9-slice background image
    {
        let frame = create_basic_drawable(lua)?;
        frame.set(
            "awake",
            lua.create_function(move |ctx, (_entity, component): (Table, Table)| {
                component.set("color", color4(ctx, 255, 255, 255, 255)?)?;
                component.set("visible", true)?;
                component.set("__neolove_component", "Frame")?;
                component.set("background_color", color4(ctx, 32, 36, 44, 230)?)?;
                component.set("border_color", color4(ctx, 92, 106, 130, 255)?)?;
                component.set("border_width", 1.0)?;
                component.set("corner_radius", 10.0)?;
                component.set("background_image", Value::Nil)?;
                component.set("slice_left", 0.0)?;
                component.set("slice_right", 0.0)?;
                component.set("slice_top", 0.0)?;
                component.set("slice_bottom", 0.0)?;
                Ok(())
            })?,
        )?;

        let render_state = render_state.clone();
        frame.set(
            "update",
            lua.create_function(move |ctx, (entity, component, _dt): (Table, Table, f32)| {
                if !component.get::<bool>("visible").unwrap_or(true) {
                    return Ok(());
                }

                let draw = get_entity_draw_context(&entity)?;
                let background_color = get_color_field(&component, "background_color")
                    .unwrap_or(color4_to_color(component.get("color")?)?);
                let border_color =
                    get_color_field(&component, "border_color").unwrap_or(background_color);
                let style = resolve_panel_style(ctx, &component, background_color, border_color)?;

                let mut renderer = render_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?;
                render_panel(
                    &mut renderer,
                    draw.bounds,
                    draw.pivot,
                    draw.rotation,
                    &style,
                )
            })?,
        )?;

        core_components.set("Frame", frame)?;
    }

    // Button
    // interactive UI button with customizable panel states and text rendering
    {
        let button = create_basic_drawable(lua)?;
        button.set(
            "awake",
            lua.create_function(move |ctx, (_entity, component): (Table, Table)| {
                component.set("color", color4(ctx, 255, 255, 255, 255)?)?;
                component.set("visible", true)?;
                component.set("__neolove_component", "Button")?;
                component.set("text", "Button")?;
                component.set("enabled", true)?;
                component.set("hovered", false)?;
                component.set("pressed", false)?;
                component.set("scale", 18.0)?;
                component.set("min_scale", 10.0)?;
                component.set("align_x", "center")?;
                component.set("align_y", "center")?;
                component.set("text_scale", "fit")?;
                component.set("wrap", "none")?;
                component.set("padding", 8.0)?;
                component.set("padding_x", 12.0)?;
                component.set("padding_y", 8.0)?;
                component.set("line_spacing", 1.0)?;
                component.set("letter_spacing", 0.0)?;
                component.set("font", Value::Nil)?;
                component.set("background_color", color4(ctx, 52, 68, 94, 255)?)?;
                component.set("hover_background_color", color4(ctx, 67, 86, 118, 255)?)?;
                component.set("pressed_background_color", color4(ctx, 39, 51, 73, 255)?)?;
                component.set("disabled_background_color", color4(ctx, 45, 48, 52, 190)?)?;
                component.set("border_color", color4(ctx, 140, 164, 196, 255)?)?;
                component.set("hover_border_color", color4(ctx, 180, 205, 235, 255)?)?;
                component.set("pressed_border_color", color4(ctx, 110, 130, 158, 255)?)?;
                component.set("disabled_border_color", color4(ctx, 80, 84, 92, 170)?)?;
                component.set("text_color", color4(ctx, 242, 245, 250, 255)?)?;
                component.set("hover_text_color", color4(ctx, 255, 255, 255, 255)?)?;
                component.set("pressed_text_color", color4(ctx, 220, 228, 239, 255)?)?;
                component.set("disabled_text_color", color4(ctx, 170, 175, 182, 210)?)?;
                component.set("border_width", 1.0)?;
                component.set("corner_radius", 8.0)?;
                component.set("background_image", Value::Nil)?;
                component.set("icon_image", Value::Nil)?;
                component.set("icon_color", color4(ctx, 255, 255, 255, 255)?)?;
                component.set("icon_size", 0.0)?;
                component.set("icon_gap", 10.0)?;
                component.set("icon_side", "left")?;
                component.set("slice_left", 0.0)?;
                component.set("slice_right", 0.0)?;
                component.set("slice_top", 0.0)?;
                component.set("slice_bottom", 0.0)?;
                Ok(())
            })?,
        )?;

        let button_platform = platform.clone();
        let button_root = env_root.clone();
        let render_state = render_state.clone();
        button.set(
            "update",
            lua.create_function(move |ctx, (entity, component, _dt): (Table, Table, f32)| {
                if !component.get::<bool>("visible").unwrap_or(true) {
                    return Ok(());
                }

                let draw = get_entity_draw_context(&entity)?;
                let snapshot = current_input_snapshot(&button_platform)?;
                let owner_key = component_owner_key(&entity, &component);
                let enabled = component.get::<bool>("enabled").unwrap_or(true);
                let hovered = enabled
                    && point_in_bounds(snapshot.mouse, draw.bounds, draw.pivot, draw.rotation)
                    && !point_blocked_by_popup(snapshot.mouse, &owner_key);
                let was_hovered = component.get::<bool>("hovered").unwrap_or(false);
                if hovered != was_hovered {
                    component.set("hovered", hovered)?;
                    if hovered {
                        call_component_callback(&component, &entity, "onHoverEnter")?;
                    } else {
                        call_component_callback(&component, &entity, "onHoverLeave")?;
                    }
                }

                let left_pressed = snapshot.input.mouse_pressed.contains("left");
                let left_released = snapshot.input.mouse_released.contains("left");
                let was_pressed = component.get::<bool>("pressed").unwrap_or(false);
                let mut pressed = was_pressed;

                if !enabled {
                    pressed = false;
                } else {
                    if left_pressed {
                        if hovered {
                            pressed = true;
                            call_component_callback(&component, &entity, "onPress")?;
                        } else {
                            pressed = false;
                        }
                    }
                    if left_released {
                        if was_pressed {
                            call_component_callback(&component, &entity, "onRelease")?;
                            if hovered {
                                call_component_callback(&component, &entity, "onClick")?;
                            }
                        }
                        pressed = false;
                    }
                }
                component.set("pressed", pressed)?;

                let background_color = if !enabled {
                    get_color_field(&component, "disabled_background_color")
                } else if pressed {
                    get_color_field(&component, "pressed_background_color")
                } else if hovered {
                    get_color_field(&component, "hover_background_color")
                } else {
                    get_color_field(&component, "background_color")
                }
                .unwrap_or(Color::rgba(48, 56, 72, 255));
                let border_color = if !enabled {
                    get_color_field(&component, "disabled_border_color")
                } else if pressed {
                    get_color_field(&component, "pressed_border_color")
                } else if hovered {
                    get_color_field(&component, "hover_border_color")
                } else {
                    get_color_field(&component, "border_color")
                }
                .unwrap_or(background_color);
                let text_color = if !enabled {
                    get_color_field(&component, "disabled_text_color")
                } else if pressed {
                    get_color_field(&component, "pressed_text_color")
                } else if hovered {
                    get_color_field(&component, "hover_text_color")
                } else {
                    get_color_field(&component, "text_color")
                }
                .unwrap_or(Color::WHITE);

                let style = resolve_panel_style(ctx, &component, background_color, border_color)?;
                let padding = component.get::<f32>("padding").unwrap_or(8.0).max(0.0);
                let padding_x = component
                    .get::<f32>("padding_x")
                    .unwrap_or(padding)
                    .max(0.0);
                let padding_y = component
                    .get::<f32>("padding_y")
                    .unwrap_or(padding)
                    .max(0.0);
                let content_bounds = Rect {
                    x: draw.bounds.x + style.border_width + padding_x,
                    y: draw.bounds.y + style.border_width + padding_y,
                    w: (draw.bounds.w - (style.border_width + padding_x) * 2.0).max(0.0),
                    h: (draw.bounds.h - (style.border_width + padding_y) * 2.0).max(0.0),
                };
                let (text_bounds, icon) = layout_inline_image(
                    content_bounds,
                    resolve_widget_icon(&component, content_bounds, text_color)?,
                );
                let mut text_request = build_text_request(
                    &button_root,
                    &component,
                    component
                        .get::<String>("text")
                        .unwrap_or_else(|_| "Button".to_string()),
                    text_bounds,
                    draw.pivot,
                    draw.rotation,
                    text_color,
                    18.0,
                    TextAlignX::Center,
                    TextAlignY::Center,
                    TextScaleMode::Fit,
                    TextWrapMode::None,
                    0.0,
                    0.0,
                );
                text_request.bounds = text_bounds;

                let mut renderer = render_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?;
                render_panel(
                    &mut renderer,
                    draw.bounds,
                    draw.pivot,
                    draw.rotation,
                    &style,
                )?;
                if let Some(icon) = icon.as_ref() {
                    queue_inline_image(&mut renderer, &draw, icon, style.filter);
                }
                renderer.queue(DrawCommand::Text(text_request));
                Ok(())
            })?,
        )?;

        core_components.set("Button", button)?;
    }

    // TextInput
    // single-line text field with focus, caret, placeholder, and submit/change callbacks
    {
        let text_input = create_basic_drawable(lua)?;
        text_input.set(
            "awake",
            lua.create_function(move |ctx, (_entity, component): (Table, Table)| {
                component.set("color", color4(ctx, 255, 255, 255, 255)?)?;
                component.set("visible", true)?;
                component.set("__neolove_component", "TextInput")?;
                component.set("text", "")?;
                component.set("placeholder", "Type here")?;
                component.set("enabled", true)?;
                component.set("hovered", false)?;
                component.set("focused", false)?;
                component.set("password", false)?;
                component.set("max_length", 0)?;
                component.set("submit_on_enter", true)?;
                component.set("clear_on_submit", false)?;
                component.set("blur_on_submit", false)?;
                component.set("cursor_index", 0)?;
                component.set("view_start", 0)?;
                component.set("cursor_blink", 0.0)?;
                component.set("caret_width", 2.0)?;
                component.set("scale", 18.0)?;
                component.set("min_scale", 12.0)?;
                component.set("align_x", "left")?;
                component.set("align_y", "center")?;
                component.set("text_scale", "none")?;
                component.set("wrap", "none")?;
                component.set("padding", 8.0)?;
                component.set("padding_x", 10.0)?;
                component.set("padding_y", 8.0)?;
                component.set("line_spacing", 1.0)?;
                component.set("letter_spacing", 0.0)?;
                component.set("font", Value::Nil)?;
                component.set("background_color", color4(ctx, 22, 26, 33, 245)?)?;
                component.set("hover_background_color", color4(ctx, 26, 31, 40, 250)?)?;
                component.set("focus_background_color", color4(ctx, 18, 24, 34, 255)?)?;
                component.set("disabled_background_color", color4(ctx, 33, 35, 40, 200)?)?;
                component.set("border_color", color4(ctx, 86, 96, 116, 255)?)?;
                component.set("hover_border_color", color4(ctx, 124, 141, 170, 255)?)?;
                component.set("focus_border_color", color4(ctx, 166, 204, 255, 255)?)?;
                component.set("disabled_border_color", color4(ctx, 66, 72, 84, 180)?)?;
                component.set("text_color", color4(ctx, 235, 239, 244, 255)?)?;
                component.set("placeholder_color", color4(ctx, 138, 147, 162, 220)?)?;
                component.set("disabled_text_color", color4(ctx, 150, 154, 162, 210)?)?;
                component.set("caret_color", color4(ctx, 240, 244, 250, 255)?)?;
                component.set("border_width", 1.0)?;
                component.set("corner_radius", 8.0)?;
                component.set("background_image", Value::Nil)?;
                component.set("icon_image", Value::Nil)?;
                component.set("icon_color", color4(ctx, 255, 255, 255, 255)?)?;
                component.set("icon_size", 0.0)?;
                component.set("icon_gap", 8.0)?;
                component.set("icon_side", "left")?;
                component.set("slice_left", 0.0)?;
                component.set("slice_right", 0.0)?;
                component.set("slice_top", 0.0)?;
                component.set("slice_bottom", 0.0)?;
                Ok(())
            })?,
        )?;

        let input_platform = platform.clone();
        let text_root = env_root.clone();
        let render_state = render_state.clone();
        text_input.set(
            "update",
            lua.create_function(move |ctx, (entity, component, dt): (Table, Table, f32)| {
                if !component.get::<bool>("visible").unwrap_or(true) {
                    return Ok(());
                }

                let draw = get_entity_draw_context(&entity)?;
                let snapshot = current_input_snapshot(&input_platform)?;
                let owner_key = component_owner_key(&entity, &component);
                let enabled = component.get::<bool>("enabled").unwrap_or(true);
                let hovered = enabled
                    && point_in_bounds(snapshot.mouse, draw.bounds, draw.pivot, draw.rotation)
                    && !point_blocked_by_popup(snapshot.mouse, &owner_key);
                let was_focused = component.get::<bool>("focused").unwrap_or(false);
                let was_hovered = component.get::<bool>("hovered").unwrap_or(false);
                if hovered != was_hovered {
                    component.set("hovered", hovered)?;
                }

                let left_pressed = snapshot.input.mouse_pressed.contains("left");
                let mut focused = was_focused;
                if !enabled && focused {
                    focused = false;
                    call_component_callback(&component, &entity, "onBlur")?;
                } else if left_pressed {
                    if hovered {
                        if !focused {
                            focused = true;
                            call_component_callback(&component, &entity, "onFocus")?;
                        }
                    } else if focused {
                        focused = false;
                        call_component_callback(&component, &entity, "onBlur")?;
                    }
                }

                let mut text = component.get::<String>("text").unwrap_or_default();
                let mut cursor = component
                    .get::<usize>("cursor_index")
                    .unwrap_or_else(|_| char_count(&text))
                    .min(char_count(&text));
                let mut changed = false;

                if focused && enabled {
                    if let Some(key) = snapshot.input.last_key_pressed.clone() {
                        match key.as_str() {
                            "left" => cursor = cursor.saturating_sub(1),
                            "right" => cursor = (cursor + 1).min(char_count(&text)),
                            "home" => cursor = 0,
                            "end" => cursor = char_count(&text),
                            "backspace" => {
                                if cursor > 0 {
                                    text = replace_char_range(&text, cursor - 1, cursor, "");
                                    cursor -= 1;
                                    changed = true;
                                }
                            }
                            "delete" => {
                                if cursor < char_count(&text) {
                                    text = replace_char_range(&text, cursor, cursor + 1, "");
                                    changed = true;
                                }
                            }
                            "escape" => {
                                focused = false;
                                call_component_callback(&component, &entity, "onBlur")?;
                            }
                            "enter" if component.get::<bool>("submit_on_enter").unwrap_or(true) => {
                                call_component_string_callback(
                                    &component, &entity, "onSubmit", &text,
                                )?;
                                if component.get::<bool>("clear_on_submit").unwrap_or(false) {
                                    text.clear();
                                    cursor = 0;
                                    changed = true;
                                }
                                if component.get::<bool>("blur_on_submit").unwrap_or(false) {
                                    focused = false;
                                    call_component_callback(&component, &entity, "onBlur")?;
                                }
                            }
                            _ => {}
                        }
                    }

                    if let Some(ch) = snapshot.input.char_pressed.clone() {
                        let max_length = component.get::<usize>("max_length").unwrap_or(0);
                        let text_len = char_count(&text);
                        let insert_len = char_count(&ch);
                        if insert_len > 0
                            && (max_length == 0 || text_len + insert_len <= max_length)
                        {
                            text = replace_char_range(&text, cursor, cursor, &ch);
                            cursor += insert_len;
                            changed = true;
                        }
                    }
                }

                if changed {
                    component.set("text", text.clone())?;
                    call_component_string_callback(&component, &entity, "onChanged", &text)?;
                }

                component.set("focused", focused)?;
                component.set("cursor_index", cursor)?;
                let blink = if focused {
                    component.get::<f32>("cursor_blink").unwrap_or(0.0) + dt.max(0.0)
                } else {
                    0.0
                };
                component.set("cursor_blink", blink)?;

                let background_color = if !enabled {
                    get_color_field(&component, "disabled_background_color")
                } else if focused {
                    get_color_field(&component, "focus_background_color")
                } else if hovered {
                    get_color_field(&component, "hover_background_color")
                } else {
                    get_color_field(&component, "background_color")
                }
                .unwrap_or(Color::rgba(24, 28, 36, 245));
                let border_color = if !enabled {
                    get_color_field(&component, "disabled_border_color")
                } else if focused {
                    get_color_field(&component, "focus_border_color")
                } else if hovered {
                    get_color_field(&component, "hover_border_color")
                } else {
                    get_color_field(&component, "border_color")
                }
                .unwrap_or(Color::rgba(96, 110, 132, 255));
                let text_color = if !enabled {
                    get_color_field(&component, "disabled_text_color")
                } else {
                    get_color_field(&component, "text_color")
                }
                .unwrap_or(Color::WHITE);
                let placeholder_color =
                    get_color_field(&component, "placeholder_color").unwrap_or(text_color);
                let caret_color = get_color_field(&component, "caret_color").unwrap_or(text_color);
                let style = resolve_panel_style(ctx, &component, background_color, border_color)?;
                let padding = component.get::<f32>("padding").unwrap_or(8.0).max(0.0);
                let padding_x = component
                    .get::<f32>("padding_x")
                    .unwrap_or(padding)
                    .max(0.0);
                let padding_y = component
                    .get::<f32>("padding_y")
                    .unwrap_or(padding)
                    .max(0.0);
                let inner_bounds = Rect {
                    x: draw.bounds.x + style.border_width + padding_x,
                    y: draw.bounds.y + style.border_width + padding_y,
                    w: (draw.bounds.w - (style.border_width + padding_x) * 2.0).max(0.0),
                    h: (draw.bounds.h - (style.border_width + padding_y) * 2.0).max(0.0),
                };
                let (text_bounds, icon) = layout_inline_image(
                    inner_bounds,
                    resolve_widget_icon(&component, inner_bounds, text_color)?,
                );

                let display_text = if component.get::<bool>("password").unwrap_or(false) {
                    "*".repeat(char_count(&text))
                } else {
                    text.clone()
                };
                let mut view_start = component
                    .get::<usize>("view_start")
                    .unwrap_or(0)
                    .min(cursor);
                let available_width = text_bounds.w.max(0.0);
                while view_start < cursor
                    && measure_inline_text(
                        &text_root,
                        &component,
                        &slice_chars(&display_text, view_start, cursor),
                        None,
                    ) > available_width
                {
                    view_start += 1;
                }
                let display_len = char_count(&display_text);
                let mut visible_end = view_start;
                let mut visible_text = String::new();
                while visible_end < display_len {
                    let candidate = slice_chars(&display_text, view_start, visible_end + 1);
                    if visible_end == view_start
                        || measure_inline_text(&text_root, &component, &candidate, None)
                            <= available_width
                    {
                        visible_end += 1;
                        visible_text = candidate;
                    } else {
                        break;
                    }
                }
                component.set("view_start", view_start)?;

                let mut renderer = render_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?;
                render_panel(
                    &mut renderer,
                    draw.bounds,
                    draw.pivot,
                    draw.rotation,
                    &style,
                )?;
                if let Some(icon) = icon.as_ref() {
                    queue_inline_image(&mut renderer, &draw, icon, style.filter);
                }

                if text.is_empty() {
                    let placeholder = component.get::<String>("placeholder").unwrap_or_default();
                    if !placeholder.is_empty() {
                        renderer.queue(DrawCommand::Text(build_text_request(
                            &text_root,
                            &component,
                            placeholder,
                            text_bounds,
                            draw.pivot,
                            draw.rotation,
                            placeholder_color,
                            18.0,
                            TextAlignX::Left,
                            TextAlignY::Center,
                            TextScaleMode::None,
                            TextWrapMode::None,
                            0.0,
                            0.0,
                        )));
                    }
                } else {
                    renderer.queue(DrawCommand::Text(build_text_request(
                        &text_root,
                        &component,
                        visible_text.clone(),
                        text_bounds,
                        draw.pivot,
                        draw.rotation,
                        text_color,
                        18.0,
                        TextAlignX::Left,
                        TextAlignY::Center,
                        TextScaleMode::None,
                        TextWrapMode::None,
                        0.0,
                        0.0,
                    )));
                }

                if focused && ((blink * 1.6).floor() as i32 % 2 == 0) {
                    let caret_prefix = slice_chars(&display_text, view_start, cursor);
                    let caret_offset =
                        measure_inline_text(&text_root, &component, &caret_prefix, None);
                    let caret_width = component.get::<f32>("caret_width").unwrap_or(2.0).max(1.0);
                    let caret_bounds = Rect {
                        x: text_bounds.x + caret_offset,
                        y: text_bounds.y + 3.0,
                        w: caret_width,
                        h: (text_bounds.h - 6.0).max(4.0),
                    };
                    queue_rect_fill(
                        &mut renderer,
                        caret_bounds,
                        draw.pivot,
                        draw.rotation,
                        caret_color,
                    );
                }

                Ok(())
            })?,
        )?;

        core_components.set("TextInput", text_input)?;
    }

    // Dropdown
    // selectable list with customizable closed/open state styling
    {
        let dropdown = create_basic_drawable(lua)?;
        dropdown.set(
            "awake",
            lua.create_function(move |ctx, (_entity, component): (Table, Table)| {
                component.set("color", color4(ctx, 255, 255, 255, 255)?)?;
                component.set("visible", true)?;
                component.set("__neolove_component", "Dropdown")?;
                component.set("enabled", true)?;
                component.set("open", false)?;
                component.set("hovered", false)?;
                component.set("hover_index", 0)?;
                component.set("selected_index", 0)?;
                component.set("selected_text", "")?;
                component.set("selected_value", "")?;
                component.set("scroll_index", 0)?;
                component.set("wheel_scroll_accumulator", 0.0)?;
                component.set("placeholder", "Select...")?;
                component.set("options", ctx.create_table()?)?;
                component.set("item_height", 32.0)?;
                component.set("item_corner_radius", 6.0)?;
                component.set("item_icon_size", 0.0)?;
                component.set("item_icon_gap", 8.0)?;
                component.set("menu_gap", 4.0)?;
                component.set("max_visible_items", 8)?;
                component.set("open_upwards", false)?;
                component.set("scale", 18.0)?;
                component.set("min_scale", 12.0)?;
                component.set("align_x", "left")?;
                component.set("align_y", "center")?;
                component.set("text_scale", "fit_width")?;
                component.set("wrap", "none")?;
                component.set("padding", 8.0)?;
                component.set("padding_x", 10.0)?;
                component.set("padding_y", 8.0)?;
                component.set("line_spacing", 1.0)?;
                component.set("letter_spacing", 0.0)?;
                component.set("font", Value::Nil)?;
                component.set("background_color", color4(ctx, 34, 40, 52, 255)?)?;
                component.set("hover_background_color", color4(ctx, 43, 52, 67, 255)?)?;
                component.set("open_background_color", color4(ctx, 28, 36, 48, 255)?)?;
                component.set("disabled_background_color", color4(ctx, 42, 44, 48, 200)?)?;
                component.set("border_color", color4(ctx, 112, 126, 151, 255)?)?;
                component.set("hover_border_color", color4(ctx, 154, 173, 205, 255)?)?;
                component.set("open_border_color", color4(ctx, 180, 210, 255, 255)?)?;
                component.set("disabled_border_color", color4(ctx, 76, 80, 90, 180)?)?;
                component.set("text_color", color4(ctx, 240, 244, 250, 255)?)?;
                component.set("disabled_text_color", color4(ctx, 168, 172, 180, 210)?)?;
                component.set("menu_background_color", color4(ctx, 20, 24, 30, 250)?)?;
                component.set("menu_border_color", color4(ctx, 112, 126, 151, 255)?)?;
                component.set("item_background_color", color4(ctx, 20, 24, 30, 0)?)?;
                component.set(
                    "item_hover_background_color",
                    color4(ctx, 56, 74, 104, 240)?,
                )?;
                component.set(
                    "item_selected_background_color",
                    color4(ctx, 42, 58, 84, 235)?,
                )?;
                component.set("item_text_color", color4(ctx, 234, 238, 244, 255)?)?;
                component.set("item_hover_text_color", color4(ctx, 255, 255, 255, 255)?)?;
                component.set("item_selected_text_color", color4(ctx, 255, 255, 255, 255)?)?;
                component.set("border_width", 1.0)?;
                component.set("corner_radius", 8.0)?;
                component.set("background_image", Value::Nil)?;
                component.set("icon_image", Value::Nil)?;
                component.set("icon_color", color4(ctx, 255, 255, 255, 255)?)?;
                component.set("icon_size", 0.0)?;
                component.set("icon_gap", 8.0)?;
                component.set("icon_side", "left")?;
                component.set("slice_left", 0.0)?;
                component.set("slice_right", 0.0)?;
                component.set("slice_top", 0.0)?;
                component.set("slice_bottom", 0.0)?;
                Ok(())
            })?,
        )?;

        let dropdown_platform = platform.clone();
        let dropdown_root = env_root.clone();
        let render_state = render_state.clone();
        dropdown.set(
            "update",
            lua.create_function(move |ctx, (entity, component, _dt): (Table, Table, f32)| {
                if !component.get::<bool>("visible").unwrap_or(true) {
                    return Ok(());
                }

                let draw = get_entity_draw_context(&entity)?;
                let snapshot = current_input_snapshot(&dropdown_platform)?;
                let owner_key = component_owner_key(&entity, &component);
                let enabled = component.get::<bool>("enabled").unwrap_or(true);
                let items =
                    read_ui_list_items(component.get::<Option<Table>>("options").ok().flatten())?;
                let option_count = items.len();
                let mut selected_index = component.get::<usize>("selected_index").unwrap_or(0);
                if option_count == 0 {
                    selected_index = 0;
                } else {
                    selected_index = selected_index.clamp(1, option_count);
                }
                let mut open = component.get::<bool>("open").unwrap_or(false) && enabled;
                let hovered = enabled
                    && point_in_bounds(snapshot.mouse, draw.bounds, draw.pivot, draw.rotation)
                    && !point_blocked_by_popup(snapshot.mouse, &owner_key);

                let item_height = component.get::<f32>("item_height").unwrap_or(32.0).max(1.0);
                let item_corner_radius = component
                    .get::<f32>("item_corner_radius")
                    .unwrap_or(6.0)
                    .max(0.0);
                let item_icon_size = component
                    .get::<f32>("item_icon_size")
                    .unwrap_or(0.0)
                    .max(0.0);
                let item_icon_gap = component
                    .get::<f32>("item_icon_gap")
                    .unwrap_or(8.0)
                    .max(0.0);
                let menu_gap = component.get::<f32>("menu_gap").unwrap_or(4.0).max(0.0);
                let max_visible = component
                    .get::<usize>("max_visible_items")
                    .unwrap_or(option_count.max(1))
                    .max(1);
                let visible_count = option_count.min(max_visible);
                let mut scroll_index = component.get::<usize>("scroll_index").unwrap_or(0);
                if option_count > visible_count {
                    scroll_index = scroll_index.min(option_count - visible_count);
                } else {
                    scroll_index = 0;
                }

                let menu_height = item_height * visible_count as f32;
                let wants_upwards = component.get::<bool>("open_upwards").unwrap_or(false);
                let open_upwards = wants_upwards
                    || (draw.bounds.y + draw.bounds.h + menu_gap + menu_height
                        > snapshot.window.height
                        && draw.bounds.y >= menu_height + menu_gap);
                let menu_bounds = Rect {
                    x: draw.bounds.x,
                    y: if open_upwards {
                        draw.bounds.y - menu_gap - menu_height
                    } else {
                        draw.bounds.y + draw.bounds.h + menu_gap
                    },
                    w: draw.bounds.w,
                    h: menu_height,
                };
                if open && visible_count > 0 {
                    register_popup(owner_key.clone(), menu_bounds, draw.pivot, draw.rotation);
                }

                let menu_hovered = open
                    && visible_count > 0
                    && point_in_bounds(snapshot.mouse, menu_bounds, draw.pivot, draw.rotation)
                    && !point_blocked_by_popup(snapshot.mouse, &owner_key);

                let mut hovered_index = 0usize;
                if menu_hovered {
                    for visible_index in 0..visible_count {
                        let item_bounds = Rect {
                            x: menu_bounds.x,
                            y: menu_bounds.y + visible_index as f32 * item_height,
                            w: menu_bounds.w,
                            h: item_height,
                        };
                        if point_in_bounds(snapshot.mouse, item_bounds, draw.pivot, draw.rotation) {
                            hovered_index = scroll_index + visible_index + 1;
                            break;
                        }
                    }
                }

                if menu_hovered && option_count > visible_count {
                    let wheel_steps = consume_wheel_steps(
                        &component,
                        "wheel_scroll_accumulator",
                        snapshot.input.wheel_y,
                        3,
                    )?;
                    if wheel_steps > 0 {
                        scroll_index = scroll_index.saturating_sub(wheel_steps as usize);
                    } else if wheel_steps < 0 {
                        scroll_index = (scroll_index + (-wheel_steps) as usize)
                            .min(option_count - visible_count);
                    }
                }

                if snapshot.input.mouse_pressed.contains("left") {
                    if hovered {
                        open = !open;
                    } else if open && hovered_index > 0 && menu_hovered {
                        selected_index = hovered_index;
                        if let Some(item) = items.get(selected_index - 1) {
                            call_component_selection_callback(
                                &component,
                                &entity,
                                "onChanged",
                                selected_index,
                                &item.value,
                            )?;
                        }
                        open = false;
                    } else if open {
                        open = false;
                    }
                }

                let selected_item = items.get(selected_index.saturating_sub(1)).cloned();
                let selected_text = selected_item
                    .as_ref()
                    .map(|item| item.text.clone())
                    .unwrap_or_else(|| {
                        component
                            .get::<String>("placeholder")
                            .unwrap_or_else(|_| "Select...".to_string())
                    });
                let selected_value = selected_item
                    .as_ref()
                    .map(|item| item.value.clone())
                    .unwrap_or_default();
                component.set("hovered", hovered)?;
                component.set("open", open)?;
                component.set("hover_index", hovered_index)?;
                component.set("selected_index", selected_index)?;
                component.set("selected_text", selected_text.clone())?;
                component.set("selected_value", selected_value)?;
                component.set("scroll_index", scroll_index)?;

                let background_color = if !enabled {
                    get_color_field(&component, "disabled_background_color")
                } else if open {
                    get_color_field(&component, "open_background_color")
                } else if hovered {
                    get_color_field(&component, "hover_background_color")
                } else {
                    get_color_field(&component, "background_color")
                }
                .unwrap_or(Color::rgba(36, 42, 54, 255));
                let border_color = if !enabled {
                    get_color_field(&component, "disabled_border_color")
                } else if open {
                    get_color_field(&component, "open_border_color")
                } else if hovered {
                    get_color_field(&component, "hover_border_color")
                } else {
                    get_color_field(&component, "border_color")
                }
                .unwrap_or(Color::rgba(112, 126, 151, 255));
                let text_color = if !enabled {
                    get_color_field(&component, "disabled_text_color")
                } else {
                    get_color_field(&component, "text_color")
                }
                .unwrap_or(Color::WHITE);
                let style = resolve_panel_style(ctx, &component, background_color, border_color)?;
                let padding = component.get::<f32>("padding").unwrap_or(8.0).max(0.0);
                let padding_x = component
                    .get::<f32>("padding_x")
                    .unwrap_or(padding)
                    .max(0.0);
                let padding_y = component
                    .get::<f32>("padding_y")
                    .unwrap_or(padding)
                    .max(0.0);
                let arrow_width = 18.0;
                let content_bounds = Rect {
                    x: draw.bounds.x + style.border_width + padding_x,
                    y: draw.bounds.y + style.border_width + padding_y,
                    w: (draw.bounds.w - (style.border_width + padding_x) * 2.0 - arrow_width)
                        .max(0.0),
                    h: (draw.bounds.h - (style.border_width + padding_y) * 2.0).max(0.0),
                };
                let selected_item_icon = selected_item.as_ref().and_then(|item| {
                    item.image.clone().and_then(|image| {
                        let icon_extent = if item_icon_size > 0.0 {
                            item_icon_size.min(content_bounds.h)
                        } else {
                            content_bounds.h.max(0.0)
                        };
                        build_inline_image(
                            content_bounds,
                            image,
                            item.image_tint,
                            item.image_source,
                            UiIconSide::Left,
                            icon_extent,
                            icon_extent,
                            item_icon_gap,
                        )
                    })
                });
                let (text_bounds, selected_icon) = layout_inline_image(
                    content_bounds,
                    resolve_widget_icon(&component, content_bounds, text_color)?
                        .or(selected_item_icon),
                );
                let arrow_bounds = Rect {
                    x: draw.bounds.x + draw.bounds.w - style.border_width - padding_x - arrow_width,
                    y: draw.bounds.y + style.border_width + padding_y,
                    w: arrow_width,
                    h: (draw.bounds.h - (style.border_width + padding_y) * 2.0).max(0.0),
                };

                let mut renderer = render_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?;
                render_panel(
                    &mut renderer,
                    draw.bounds,
                    draw.pivot,
                    draw.rotation,
                    &style,
                )?;
                if let Some(icon) = selected_icon.as_ref() {
                    queue_inline_image(&mut renderer, &draw, icon, style.filter);
                }
                renderer.queue(DrawCommand::Text(build_text_request(
                    &dropdown_root,
                    &component,
                    selected_text,
                    text_bounds,
                    draw.pivot,
                    draw.rotation,
                    text_color,
                    18.0,
                    TextAlignX::Left,
                    TextAlignY::Center,
                    TextScaleMode::FitWidth,
                    TextWrapMode::None,
                    0.0,
                    0.0,
                )));
                renderer.queue(DrawCommand::Text(build_text_request(
                    &dropdown_root,
                    &component,
                    if open {
                        "^".to_string()
                    } else {
                        "v".to_string()
                    },
                    arrow_bounds,
                    draw.pivot,
                    draw.rotation,
                    text_color,
                    16.0,
                    TextAlignX::Center,
                    TextAlignY::Center,
                    TextScaleMode::FitWidth,
                    TextWrapMode::None,
                    0.0,
                    0.0,
                )));

                if open && visible_count > 0 {
                    let menu_background = get_color_field(&component, "menu_background_color")
                        .unwrap_or(background_color);
                    let menu_border =
                        get_color_field(&component, "menu_border_color").unwrap_or(border_color);
                    let menu_style =
                        resolve_panel_style(ctx, &component, menu_background, menu_border)?;
                    let mut overlay = RenderState::default();
                    render_panel(
                        &mut overlay,
                        menu_bounds,
                        draw.pivot,
                        draw.rotation,
                        &menu_style,
                    )?;

                    for visible_index in 0..visible_count {
                        let option_index = scroll_index + visible_index + 1;
                        let item_bounds = Rect {
                            x: menu_bounds.x + menu_style.border_width,
                            y: menu_bounds.y
                                + visible_index as f32 * item_height
                                + menu_style.border_width,
                            w: (menu_bounds.w - menu_style.border_width * 2.0).max(0.0),
                            h: (item_height - menu_style.border_width).max(0.0),
                        };
                        let item_background = if option_index == selected_index {
                            get_color_field(&component, "item_selected_background_color")
                        } else if option_index == hovered_index {
                            get_color_field(&component, "item_hover_background_color")
                        } else {
                            get_color_field(&component, "item_background_color")
                        }
                        .unwrap_or(Color::rgba(0, 0, 0, 0));
                        if item_background.a > 0 {
                            queue_rounded_rect_fill(
                                &mut overlay,
                                item_bounds,
                                draw.pivot,
                                draw.rotation,
                                item_background,
                                item_corner_radius,
                            );
                        }
                        let item_text = if option_index == selected_index {
                            get_color_field(&component, "item_selected_text_color")
                        } else if option_index == hovered_index {
                            get_color_field(&component, "item_hover_text_color")
                        } else {
                            get_color_field(&component, "item_text_color")
                        }
                        .unwrap_or(text_color);
                        if let Some(item) = items.get(option_index - 1) {
                            let item_content_bounds = Rect {
                                x: item_bounds.x + padding_x,
                                y: item_bounds.y + padding_y.min(item_height * 0.25),
                                w: (item_bounds.w - padding_x * 2.0).max(0.0),
                                h: (item_bounds.h - padding_y * 2.0).max(0.0),
                            };
                            let item_icon = item.image.clone().and_then(|image| {
                                let icon_extent = if item_icon_size > 0.0 {
                                    item_icon_size.min(item_content_bounds.h)
                                } else {
                                    item_content_bounds.h.max(0.0)
                                };
                                build_inline_image(
                                    item_content_bounds,
                                    image,
                                    item.image_tint,
                                    item.image_source,
                                    UiIconSide::Left,
                                    icon_extent,
                                    icon_extent,
                                    item_icon_gap,
                                )
                            });
                            let (item_text_bounds, item_icon) =
                                layout_inline_image(item_content_bounds, item_icon);
                            if let Some(item_icon) = item_icon.as_ref() {
                                queue_inline_image(
                                    &mut overlay,
                                    &draw,
                                    item_icon,
                                    menu_style.filter,
                                );
                            }
                            overlay.queue(DrawCommand::Text(build_text_request(
                                &dropdown_root,
                                &component,
                                item.text.clone(),
                                item_text_bounds,
                                draw.pivot,
                                draw.rotation,
                                item_text,
                                18.0,
                                TextAlignX::Left,
                                TextAlignY::Center,
                                TextScaleMode::FitWidth,
                                TextWrapMode::None,
                                0.0,
                                0.0,
                            )));
                        }
                    }

                    renderer.extend_overlay(overlay.drain());
                }

                Ok(())
            })?,
        )?;

        core_components.set("Dropdown", dropdown)?;
    }

    // ScrollList
    // scrolling list view with selection, keyboard navigation, and customizable item styling
    {
        let scroll_list = create_basic_drawable(lua)?;
        scroll_list.set(
            "awake",
            lua.create_function(move |ctx, (_entity, component): (Table, Table)| {
                component.set("color", color4(ctx, 255, 255, 255, 255)?)?;
                component.set("visible", true)?;
                component.set("__neolove_component", "ScrollList")?;
                component.set("enabled", true)?;
                component.set("hovered", false)?;
                component.set("focused", false)?;
                component.set("hover_index", 0)?;
                component.set("selected_index", 0)?;
                component.set("selected_text", "")?;
                component.set("selected_value", "")?;
                component.set("scroll_index", 0)?;
                component.set("wheel_scroll_accumulator", 0.0)?;
                component.set("options", ctx.create_table()?)?;
                component.set("empty_text", "No items")?;
                component.set("item_height", 32.0)?;
                component.set("item_spacing", 4.0)?;
                component.set("item_corner_radius", 6.0)?;
                component.set("item_icon_size", 0.0)?;
                component.set("item_icon_gap", 8.0)?;
                component.set("item_padding_x", 10.0)?;
                component.set("item_padding_y", 6.0)?;
                component.set("show_scrollbar", true)?;
                component.set("scrollbar_width", 8.0)?;
                component.set("scrollbar_dragging", false)?;
                component.set("scrollbar_drag_offset", 0.0)?;
                component.set("scale", 18.0)?;
                component.set("min_scale", 12.0)?;
                component.set("align_x", "left")?;
                component.set("align_y", "center")?;
                component.set("text_scale", "fit_width")?;
                component.set("wrap", "none")?;
                component.set("padding", 8.0)?;
                component.set("padding_x", 10.0)?;
                component.set("padding_y", 10.0)?;
                component.set("line_spacing", 1.0)?;
                component.set("letter_spacing", 0.0)?;
                component.set("font", Value::Nil)?;
                component.set("background_color", color4(ctx, 24, 29, 36, 245)?)?;
                component.set("hover_background_color", color4(ctx, 28, 34, 42, 250)?)?;
                component.set("focus_background_color", color4(ctx, 18, 24, 34, 255)?)?;
                component.set("disabled_background_color", color4(ctx, 34, 36, 40, 200)?)?;
                component.set("border_color", color4(ctx, 92, 106, 128, 255)?)?;
                component.set("hover_border_color", color4(ctx, 126, 146, 176, 255)?)?;
                component.set("focus_border_color", color4(ctx, 176, 214, 255, 255)?)?;
                component.set("disabled_border_color", color4(ctx, 74, 78, 88, 180)?)?;
                component.set("text_color", color4(ctx, 234, 239, 246, 255)?)?;
                component.set("empty_text_color", color4(ctx, 146, 156, 170, 220)?)?;
                component.set("disabled_text_color", color4(ctx, 164, 168, 176, 210)?)?;
                component.set("item_background_color", color4(ctx, 0, 0, 0, 0)?)?;
                component.set(
                    "item_hover_background_color",
                    color4(ctx, 60, 78, 107, 235)?,
                )?;
                component.set(
                    "item_selected_background_color",
                    color4(ctx, 42, 58, 84, 245)?,
                )?;
                component.set("item_text_color", color4(ctx, 234, 239, 246, 255)?)?;
                component.set("item_hover_text_color", color4(ctx, 255, 255, 255, 255)?)?;
                component.set("item_selected_text_color", color4(ctx, 255, 255, 255, 255)?)?;
                component.set("scrollbar_color", color4(ctx, 56, 64, 78, 180)?)?;
                component.set("scrollbar_thumb_color", color4(ctx, 176, 214, 255, 235)?)?;
                component.set("border_width", 1.0)?;
                component.set("corner_radius", 8.0)?;
                component.set("background_image", Value::Nil)?;
                component.set("slice_left", 0.0)?;
                component.set("slice_right", 0.0)?;
                component.set("slice_top", 0.0)?;
                component.set("slice_bottom", 0.0)?;
                Ok(())
            })?,
        )?;

        let scroll_list_platform = platform.clone();
        let scroll_list_root = env_root.clone();
        let render_state = render_state.clone();
        scroll_list.set(
            "update",
            lua.create_function(move |ctx, (entity, component, _dt): (Table, Table, f32)| {
                if !component.get::<bool>("visible").unwrap_or(true) {
                    return Ok(());
                }

                let draw = get_entity_draw_context(&entity)?;
                let snapshot = current_input_snapshot(&scroll_list_platform)?;
                let owner_key = component_owner_key(&entity, &component);
                let enabled = component.get::<bool>("enabled").unwrap_or(true);
                let hovered = enabled
                    && point_in_bounds(snapshot.mouse, draw.bounds, draw.pivot, draw.rotation)
                    && !point_blocked_by_popup(snapshot.mouse, &owner_key);
                let was_focused = component.get::<bool>("focused").unwrap_or(false);
                let mut focused = was_focused;
                if !enabled {
                    focused = false;
                } else if snapshot.input.mouse_pressed.contains("left") {
                    focused = hovered;
                }
                let focus_changed = focused != was_focused;

                let items =
                    read_ui_list_items(component.get::<Option<Table>>("options").ok().flatten())?;
                let option_count = items.len();
                let mut selected_index = component.get::<usize>("selected_index").unwrap_or(0);
                if selected_index > option_count {
                    selected_index = option_count;
                }

                let padding = component.get::<f32>("padding").unwrap_or(8.0).max(0.0);
                let padding_x = component
                    .get::<f32>("padding_x")
                    .unwrap_or(padding)
                    .max(0.0);
                let padding_y = component
                    .get::<f32>("padding_y")
                    .unwrap_or(padding)
                    .max(0.0);
                let item_height = component.get::<f32>("item_height").unwrap_or(32.0).max(1.0);
                let item_spacing = component.get::<f32>("item_spacing").unwrap_or(4.0).max(0.0);
                let item_corner_radius = component
                    .get::<f32>("item_corner_radius")
                    .unwrap_or(6.0)
                    .max(0.0);
                let item_icon_size = component
                    .get::<f32>("item_icon_size")
                    .unwrap_or(0.0)
                    .max(0.0);
                let item_icon_gap = component
                    .get::<f32>("item_icon_gap")
                    .unwrap_or(8.0)
                    .max(0.0);
                let item_padding_x = component
                    .get::<f32>("item_padding_x")
                    .unwrap_or(10.0)
                    .max(0.0);
                let item_padding_y = component
                    .get::<f32>("item_padding_y")
                    .unwrap_or(6.0)
                    .max(0.0);
                let show_scrollbar = component.get::<bool>("show_scrollbar").unwrap_or(true);
                let scrollbar_width = component
                    .get::<f32>("scrollbar_width")
                    .unwrap_or(8.0)
                    .max(0.0);
                let row_stride = item_height + item_spacing;
                let left_pressed = snapshot.input.mouse_pressed.contains("left");
                let left_down = snapshot.input.mouse_down.contains("left");

                let background_color = if !enabled {
                    get_color_field(&component, "disabled_background_color")
                } else if focused {
                    get_color_field(&component, "focus_background_color")
                } else if hovered {
                    get_color_field(&component, "hover_background_color")
                } else {
                    get_color_field(&component, "background_color")
                }
                .unwrap_or(Color::rgba(24, 29, 36, 245));
                let border_color = if !enabled {
                    get_color_field(&component, "disabled_border_color")
                } else if focused {
                    get_color_field(&component, "focus_border_color")
                } else if hovered {
                    get_color_field(&component, "hover_border_color")
                } else {
                    get_color_field(&component, "border_color")
                }
                .unwrap_or(Color::rgba(92, 106, 128, 255));
                let text_color = if !enabled {
                    get_color_field(&component, "disabled_text_color")
                } else {
                    get_color_field(&component, "text_color")
                }
                .unwrap_or(Color::WHITE);
                let empty_text_color = if !enabled {
                    get_color_field(&component, "disabled_text_color")
                } else {
                    get_color_field(&component, "empty_text_color")
                }
                .unwrap_or(text_color);
                let scrollbar_color =
                    get_color_field(&component, "scrollbar_color").unwrap_or(border_color);
                let scrollbar_thumb_color = get_color_field(&component, "scrollbar_thumb_color")
                    .unwrap_or(Color::rgba(176, 214, 255, 235));
                let style = resolve_panel_style(ctx, &component, background_color, border_color)?;
                let inner_bounds = Rect {
                    x: draw.bounds.x + style.border_width + padding_x,
                    y: draw.bounds.y + style.border_width + padding_y,
                    w: (draw.bounds.w - (style.border_width + padding_x) * 2.0).max(0.0),
                    h: (draw.bounds.h - (style.border_width + padding_y) * 2.0).max(0.0),
                };

                let visible_capacity = if inner_bounds.h <= 0.0 || row_stride <= 0.0 {
                    0
                } else {
                    (((inner_bounds.h + item_spacing) / row_stride).floor() as usize).max(1)
                };
                let visible_count = option_count.min(visible_capacity);
                let mut scroll_index = component.get::<usize>("scroll_index").unwrap_or(0);
                if option_count > visible_count && visible_count > 0 {
                    scroll_index = scroll_index.min(option_count - visible_count);
                } else {
                    scroll_index = 0;
                }

                let overflow = option_count > visible_count && visible_count > 0;
                let scrollbar_gap = if overflow && show_scrollbar && scrollbar_width > 0.0 {
                    item_spacing.max(4.0)
                } else {
                    0.0
                };
                let mut list_bounds = inner_bounds;
                if overflow && show_scrollbar && scrollbar_width > 0.0 {
                    list_bounds.w = (list_bounds.w - scrollbar_width - scrollbar_gap).max(0.0);
                }

                let max_scroll = option_count.saturating_sub(visible_count);
                let local_mouse = world_point_to_local(snapshot.mouse, draw.pivot, draw.rotation);
                let track_bounds = if overflow && show_scrollbar && scrollbar_width > 0.0 {
                    Some(Rect {
                        x: inner_bounds.x + inner_bounds.w - scrollbar_width,
                        y: inner_bounds.y,
                        w: scrollbar_width,
                        h: inner_bounds.h,
                    })
                } else {
                    None
                };
                let thumb_bounds = track_bounds.map(|track_bounds| {
                    let thumb_height = (track_bounds.h * visible_count as f32
                        / option_count as f32)
                        .max((item_height * 0.75).min(track_bounds.h))
                        .min(track_bounds.h);
                    let thumb_y = if max_scroll == 0 {
                        track_bounds.y
                    } else {
                        track_bounds.y
                            + (track_bounds.h - thumb_height)
                                * (scroll_index as f32 / max_scroll as f32)
                    };
                    Rect {
                        x: track_bounds.x,
                        y: thumb_y,
                        w: track_bounds.w,
                        h: thumb_height,
                    }
                });
                let track_hovered = track_bounds
                    .map(|track_bounds| {
                        point_in_bounds(snapshot.mouse, track_bounds, draw.pivot, draw.rotation)
                            && !point_blocked_by_popup(snapshot.mouse, &owner_key)
                    })
                    .unwrap_or(false);
                let thumb_hovered = thumb_bounds
                    .map(|thumb_bounds| {
                        point_in_bounds(snapshot.mouse, thumb_bounds, draw.pivot, draw.rotation)
                            && !point_blocked_by_popup(snapshot.mouse, &owner_key)
                    })
                    .unwrap_or(false);
                let mut scrollbar_dragging =
                    component.get::<bool>("scrollbar_dragging").unwrap_or(false) && enabled;
                let mut scrollbar_drag_offset = component
                    .get::<f32>("scrollbar_drag_offset")
                    .unwrap_or(0.0)
                    .max(0.0);
                if !left_down {
                    scrollbar_dragging = false;
                }

                if enabled && left_pressed {
                    if let Some(thumb_bounds) = thumb_bounds {
                        if thumb_hovered {
                            scrollbar_dragging = true;
                            scrollbar_drag_offset = (local_mouse.y - thumb_bounds.y)
                                .clamp(0.0, thumb_bounds.h.max(0.0));
                            focused = true;
                        } else if track_hovered {
                            scrollbar_dragging = true;
                            scrollbar_drag_offset = thumb_bounds.h * 0.5;
                            focused = true;
                        }
                    }
                }

                if overflow && scrollbar_dragging && left_down {
                    if let (Some(track_bounds), Some(thumb_bounds)) = (track_bounds, thumb_bounds) {
                        let available = (track_bounds.h - thumb_bounds.h).max(0.0);
                        let thumb_top = if available <= 0.0 {
                            track_bounds.y
                        } else {
                            (local_mouse.y - scrollbar_drag_offset)
                                .clamp(track_bounds.y, track_bounds.y + available)
                        };
                        scroll_index = if max_scroll == 0 || available <= 0.0 {
                            0
                        } else {
                            (((thumb_top - track_bounds.y) / available) * max_scroll as f32).round()
                                as usize
                        }
                        .min(max_scroll);
                    }
                } else if overflow && hovered {
                    let wheel_steps = consume_wheel_steps(
                        &component,
                        "wheel_scroll_accumulator",
                        snapshot.input.wheel_y,
                        4,
                    )?;
                    if wheel_steps > 0 {
                        scroll_index = scroll_index.saturating_sub(wheel_steps as usize);
                    } else if wheel_steps < 0 {
                        scroll_index = (scroll_index + (-wheel_steps) as usize).min(max_scroll);
                    }
                }

                let mut selection_changed = false;
                if focused && enabled && option_count > 0 {
                    if let Some(key) = snapshot.input.last_key_pressed.clone() {
                        let mut next_selected = selected_index;
                        match key.as_str() {
                            "up" => {
                                next_selected = if selected_index > 1 {
                                    selected_index - 1
                                } else {
                                    1
                                };
                            }
                            "down" => {
                                next_selected = if selected_index == 0 {
                                    1
                                } else {
                                    (selected_index + 1).min(option_count)
                                };
                            }
                            "pageup" => {
                                let step = visible_count.max(1);
                                next_selected = if selected_index == 0 {
                                    1
                                } else {
                                    selected_index.saturating_sub(step).max(1)
                                };
                            }
                            "pagedown" => {
                                let step = visible_count.max(1);
                                next_selected = if selected_index == 0 {
                                    1
                                } else {
                                    (selected_index + step).min(option_count)
                                };
                            }
                            "home" => next_selected = 1,
                            "end" => next_selected = option_count,
                            _ => {}
                        }

                        if next_selected != selected_index {
                            selected_index = next_selected;
                            selection_changed = true;
                        }
                    }
                }

                let mut hovered_index = 0usize;
                if enabled && option_count > 0 && visible_count > 0 && list_bounds.w > 0.0 {
                    for visible_index in 0..visible_count {
                        let item_y = list_bounds.y + visible_index as f32 * row_stride;
                        let item_bounds = Rect {
                            x: list_bounds.x,
                            y: item_y,
                            w: list_bounds.w,
                            h: item_height.min((list_bounds.y + list_bounds.h - item_y).max(0.0)),
                        };
                        if item_bounds.h <= 0.0 {
                            continue;
                        }
                        if point_in_bounds(snapshot.mouse, item_bounds, draw.pivot, draw.rotation)
                            && !point_blocked_by_popup(snapshot.mouse, &owner_key)
                        {
                            hovered_index = scroll_index + visible_index + 1;
                            break;
                        }
                    }
                }

                if enabled
                    && left_pressed
                    && !scrollbar_dragging
                    && hovered_index > 0
                    && hovered_index != selected_index
                {
                    selected_index = hovered_index;
                    selection_changed = true;
                }

                if selected_index > 0 && visible_count > 0 {
                    if selected_index <= scroll_index {
                        scroll_index = selected_index - 1;
                    } else if selected_index > scroll_index + visible_count {
                        scroll_index = selected_index - visible_count;
                    }
                }
                scroll_index = scroll_index.min(max_scroll);
                let thumb_bounds = track_bounds.map(|track_bounds| {
                    let thumb_height = (track_bounds.h * visible_count as f32
                        / option_count as f32)
                        .max((item_height * 0.75).min(track_bounds.h))
                        .min(track_bounds.h);
                    let thumb_y = if max_scroll == 0 {
                        track_bounds.y
                    } else {
                        track_bounds.y
                            + (track_bounds.h - thumb_height)
                                * (scroll_index as f32 / max_scroll as f32)
                    };
                    Rect {
                        x: track_bounds.x,
                        y: thumb_y,
                        w: track_bounds.w,
                        h: thumb_height,
                    }
                });

                let selected_item = items.get(selected_index.saturating_sub(1)).cloned();
                let selected_text = selected_item
                    .as_ref()
                    .map(|item| item.text.clone())
                    .unwrap_or_default();
                let selected_value = selected_item
                    .as_ref()
                    .map(|item| item.value.clone())
                    .unwrap_or_default();

                component.set("hovered", hovered)?;
                component.set("focused", focused)?;
                component.set("hover_index", hovered_index)?;
                component.set("selected_index", selected_index)?;
                component.set("selected_text", selected_text.clone())?;
                component.set("selected_value", selected_value.clone())?;
                component.set("scroll_index", scroll_index)?;
                component.set("scrollbar_dragging", scrollbar_dragging)?;
                component.set("scrollbar_drag_offset", scrollbar_drag_offset)?;

                if focus_changed {
                    if focused {
                        call_component_callback(&component, &entity, "onFocus")?;
                    } else {
                        call_component_callback(&component, &entity, "onBlur")?;
                    }
                }
                if selection_changed && selected_index > 0 {
                    call_component_selection_callback(
                        &component,
                        &entity,
                        "onChanged",
                        selected_index,
                        &selected_value,
                    )?;
                }

                let mut renderer = render_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?;
                render_panel(
                    &mut renderer,
                    draw.bounds,
                    draw.pivot,
                    draw.rotation,
                    &style,
                )?;

                if option_count == 0 {
                    let empty_text = component
                        .get::<String>("empty_text")
                        .unwrap_or_else(|_| "No items".to_string());
                    if !empty_text.is_empty() {
                        renderer.queue(DrawCommand::Text(build_text_request(
                            &scroll_list_root,
                            &component,
                            empty_text,
                            inner_bounds,
                            draw.pivot,
                            draw.rotation,
                            empty_text_color,
                            18.0,
                            TextAlignX::Left,
                            TextAlignY::Center,
                            TextScaleMode::FitWidth,
                            TextWrapMode::None,
                            0.0,
                            0.0,
                        )));
                    }
                    return Ok(());
                }

                for visible_index in 0..visible_count {
                    let option_index = scroll_index + visible_index + 1;
                    let item_y = list_bounds.y + visible_index as f32 * row_stride;
                    let item_bounds = Rect {
                        x: list_bounds.x,
                        y: item_y,
                        w: list_bounds.w,
                        h: item_height.min((list_bounds.y + list_bounds.h - item_y).max(0.0)),
                    };
                    if item_bounds.w <= 0.0 || item_bounds.h <= 0.0 {
                        continue;
                    }

                    let item_background = if option_index == selected_index {
                        get_color_field(&component, "item_selected_background_color")
                    } else if option_index == hovered_index {
                        get_color_field(&component, "item_hover_background_color")
                    } else {
                        get_color_field(&component, "item_background_color")
                    }
                    .unwrap_or(Color::rgba(0, 0, 0, 0));
                    if item_background.a > 0 {
                        queue_rounded_rect_fill(
                            &mut renderer,
                            item_bounds,
                            draw.pivot,
                            draw.rotation,
                            item_background,
                            item_corner_radius,
                        );
                    }

                    let item_text_color = if !enabled {
                        get_color_field(&component, "disabled_text_color")
                    } else if option_index == selected_index {
                        get_color_field(&component, "item_selected_text_color")
                    } else if option_index == hovered_index {
                        get_color_field(&component, "item_hover_text_color")
                    } else {
                        get_color_field(&component, "item_text_color")
                    }
                    .unwrap_or(text_color);

                    if let Some(item) = items.get(option_index - 1) {
                        let item_content_bounds = Rect {
                            x: item_bounds.x + item_padding_x,
                            y: item_bounds.y + item_padding_y,
                            w: (item_bounds.w - item_padding_x * 2.0).max(0.0),
                            h: (item_bounds.h - item_padding_y * 2.0).max(0.0),
                        };
                        let item_icon = item.image.clone().and_then(|image| {
                            let icon_extent = if item_icon_size > 0.0 {
                                item_icon_size.min(item_content_bounds.h)
                            } else {
                                item_content_bounds.h.max(0.0)
                            };
                            build_inline_image(
                                item_content_bounds,
                                image,
                                item.image_tint,
                                item.image_source,
                                UiIconSide::Left,
                                icon_extent,
                                icon_extent,
                                item_icon_gap,
                            )
                        });
                        let (item_text_bounds, item_icon) =
                            layout_inline_image(item_content_bounds, item_icon);
                        if let Some(item_icon) = item_icon.as_ref() {
                            queue_inline_image(&mut renderer, &draw, item_icon, style.filter);
                        }
                        renderer.queue(DrawCommand::Text(build_text_request(
                            &scroll_list_root,
                            &component,
                            item.text.clone(),
                            item_text_bounds,
                            draw.pivot,
                            draw.rotation,
                            item_text_color,
                            18.0,
                            TextAlignX::Left,
                            TextAlignY::Center,
                            TextScaleMode::FitWidth,
                            TextWrapMode::None,
                            0.0,
                            0.0,
                        )));
                    }
                }

                if let (Some(track_bounds), Some(thumb_bounds)) = (track_bounds, thumb_bounds) {
                    queue_rounded_rect_fill(
                        &mut renderer,
                        track_bounds,
                        draw.pivot,
                        draw.rotation,
                        scrollbar_color,
                        scrollbar_width * 0.5,
                    );
                    queue_rounded_rect_fill(
                        &mut renderer,
                        thumb_bounds,
                        draw.pivot,
                        draw.rotation,
                        scrollbar_thumb_color,
                        scrollbar_width * 0.5,
                    );
                }

                Ok(())
            })?,
        )?;

        core_components.set("ScrollList", scroll_list)?;
    }
    }

    // Image2D
    // draw an image (texture) tinted by component.color, scaled to entity size
    {
        let image2d = create_basic_drawable(lua)?;
        let render_state = render_state.clone();

        image2d.set(
            "update",
            lua.create_function(move |ctx, (entity, component, _dt): (Table, Table, f32)| {
                if !component.get::<bool>("visible").unwrap_or(true) {
                    return Ok(());
                }
                let (x, y, rotation) = crate::window::get_global_transform(&entity)?;
                let (w, h) = crate::window::get_global_size(&entity)?;
                let use_middle_pivot = crate::window::uses_middle_pivot(&entity);

                let tint: Color = color4_to_color(component.get("color")?)?;
                let image: Option<AnyUserData> = component.get("image")?;
                let Some(image) = image else {
                    return Ok(());
                };

                let image = image.borrow::<crate::assets::ImageHandle>()?;
                image.ensure_uploaded()?;
                let (draw_x, draw_y, pivot) = if use_middle_pivot {
                    let (px, py) = crate::window::get_global_rotation_pivot(&entity)?;
                    // draw_texture_ex expects the unrotated rectangle origin when pivot is provided.
                    (px - w * 0.5, py - h * 0.5, Vec2 { x: px, y: py })
                } else {
                    (x, y, Vec2 { x, y })
                };
                let mut renderer = render_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?;
                renderer.queue(DrawCommand::Image {
                    image: image.clone(),
                    dest: Rect {
                        x: draw_x,
                        y: draw_y,
                        w,
                        h,
                    },
                    source: None,
                    rotation,
                    pivot,
                    tint,
                    filter: app_texture_filter(ctx),
                });

                Ok(())
            })?,
        )?;

        core_components.set("Image2D", image2d)?;
    }

    // TileTexture2D
    // draw an image repeatedly to fill entity size, with optional tile sizing and offset
    {
        let tile_texture2d = create_basic_drawable(lua)?;
        let platform = platform.clone();
        let render_state = render_state.clone();
        tile_texture2d.set(
            "awake",
            lua.create_function(move |ctx, (_entity, component): (Table, Table)| {
                component.set("color", color4(ctx, 255, 255, 255, 255)?)?;
                component.set("visible", true)?;
                component.set("tile_width", 0.0)?;
                component.set("tile_height", 0.0)?;
                component.set("offset_x", 0.0)?;
                component.set("offset_y", 0.0)?;
                Ok(())
            })?,
        )?;

        tile_texture2d.set(
            "update",
            lua.create_function(move |ctx, (entity, component, _dt): (Table, Table, f32)| {
                if !component.get::<bool>("visible").unwrap_or(true) {
                    return Ok(());
                }
                let (x, y, rotation) = crate::window::get_global_transform(&entity)?;
                let (w, h) = crate::window::get_global_size(&entity)?;
                let entity_scale = crate::window::get_global_scale(&entity)?;
                if w <= 0.0 || h <= 0.0 {
                    return Ok(());
                }
                let use_middle_pivot = crate::window::uses_middle_pivot(&entity);

                let tint: Color = color4_to_color(component.get("color")?)?;
                let image: Option<AnyUserData> = component.get("image")?;
                let Some(image) = image else {
                    return Ok(());
                };

                let image = image.borrow::<crate::assets::ImageHandle>()?;
                image.ensure_uploaded()?;
                let (img_w, img_h) = image.dimensions()?;
                let tex_w = (img_w as f32).max(1.0);
                let tex_h = (img_h as f32).max(1.0);

                let mut tile_w = component.get::<f32>("tile_width").unwrap_or(0.0);
                let mut tile_h = component.get::<f32>("tile_height").unwrap_or(0.0);
                if tile_w <= 0.0 {
                    tile_w = tex_w;
                }
                if tile_h <= 0.0 {
                    tile_h = tex_h;
                }
                tile_w *= entity_scale;
                tile_h *= entity_scale;
                if tile_w <= 0.0 || tile_h <= 0.0 {
                    return Ok(());
                }

                let offset_x = component.get::<f32>("offset_x").unwrap_or(0.0) * entity_scale;
                let offset_y = component.get::<f32>("offset_y").unwrap_or(0.0) * entity_scale;
                let (base_x, base_y, pivot) = if use_middle_pivot {
                    let (px, py) = crate::window::get_global_rotation_pivot(&entity)?;
                    (px - w * 0.5, py - h * 0.5, Vec2 { x: px, y: py })
                } else {
                    (x, y, Vec2 { x, y })
                };
                let (phase_origin_x, phase_origin_y) =
                    if let Ok(Some(parent)) = entity.get::<Option<Table>>("parent") {
                        crate::window::get_global_position(&parent).unwrap_or((base_x, base_y))
                    } else {
                        (base_x, base_y)
                    };
                let phase_anchor_x = phase_origin_x + offset_x;
                let phase_anchor_y = phase_origin_y + offset_y;
                let tile_eps = 0.0001f32;

                // For non-rotated tile layers, cull to visible screen-space bounds first.
                // Rotated tile layers keep the full-entity iteration to preserve rendering correctness.
                let (local_left, local_top, local_right, local_bottom) = if rotation.abs() < 0.0001
                {
                    let platform = platform
                        .lock()
                        .map_err(|_| mlua::Error::external("platform lock poisoned"))?;
                    let screen_w = platform.window().width;
                    let screen_h = platform.window().height;
                    let visible_left = base_x.max(0.0);
                    let visible_top = base_y.max(0.0);
                    let visible_right = (base_x + w).min(screen_w);
                    let visible_bottom = (base_y + h).min(screen_h);

                    if visible_right <= visible_left || visible_bottom <= visible_top {
                        return Ok(());
                    }

                    (
                        visible_left - base_x,
                        visible_top - base_y,
                        visible_right - base_x,
                        visible_bottom - base_y,
                    )
                } else {
                    (0.0, 0.0, w, h)
                };

                let world_left = base_x + local_left;
                let world_top = base_y + local_top;
                let world_right = base_x + local_right;
                let world_bottom = base_y + local_bottom;

                let ix_min =
                    (((world_left - phase_anchor_x) as f64) / (tile_w as f64)).floor() as i32;
                let ix_max =
                    (((world_right - phase_anchor_x) as f64) / (tile_w as f64)).ceil() as i32;
                let iy_min =
                    (((world_top - phase_anchor_y) as f64) / (tile_h as f64)).floor() as i32;
                let iy_max =
                    (((world_bottom - phase_anchor_y) as f64) / (tile_h as f64)).ceil() as i32;

                let mut renderer = render_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?;
                for iy in iy_min..iy_max {
                    let tile_top = phase_anchor_y + iy as f32 * tile_h;
                    let visible_top = tile_top.max(world_top);
                    let visible_bottom = (tile_top + tile_h).min(world_bottom);
                    if visible_bottom - visible_top <= tile_eps {
                        continue;
                    }

                    for ix in ix_min..ix_max {
                        let tile_left = phase_anchor_x + ix as f32 * tile_w;
                        let visible_left = tile_left.max(world_left);
                        let visible_right = (tile_left + tile_w).min(world_right);
                        if visible_right - visible_left <= tile_eps {
                            continue;
                        }

                        let visible_w = visible_right - visible_left;
                        let visible_h = visible_bottom - visible_top;
                        let src_x =
                            (((visible_left - tile_left) / tile_w) * tex_w).clamp(0.0, tex_w);
                        let src_y = (((visible_top - tile_top) / tile_h) * tex_h).clamp(0.0, tex_h);
                        let src_w =
                            (((visible_w / tile_w) * tex_w).max(0.0)).min((tex_w - src_x).max(0.0));
                        let src_h =
                            (((visible_h / tile_h) * tex_h).max(0.0)).min((tex_h - src_y).max(0.0));
                        if src_w <= tile_eps || src_h <= tile_eps {
                            continue;
                        }

                        renderer.queue(DrawCommand::Image {
                            image: image.clone(),
                            dest: Rect {
                                x: visible_left,
                                y: visible_top,
                                w: visible_w,
                                h: visible_h,
                            },
                            source: Some(Rect {
                                x: src_x,
                                y: src_y,
                                w: src_w,
                                h: src_h,
                            }),
                            rotation,
                            pivot,
                            tint,
                            filter: app_texture_filter(ctx),
                        });
                    }
                }
                Ok(())
            })?,
        )?;

        core_components.set("TileTexture2D", tile_texture2d)?;
    }

    // Collider2D
    // axis-aligned collider used by Rigidbody2D collision solver
    {
        let collider2d = lua.create_table()?;

        collider2d.set(
            "awake",
            lua.create_function(move |_ctx, (_entity, component): (Table, Table)| {
                component.set("__neolove_component", "Collider2D")?;
                component.set("enabled", true)?;
                component.set("is_trigger", false)?;
                component.set("non_physics", false)?;
                component.set("offset_x", 0.0)?;
                component.set("offset_y", 0.0)?;
                component.set("size_x", 0.0)?;
                component.set("size_y", 0.0)?;
                component.set("shape", "box")?;
                component.set("triangle_corner", "bl")?;
                component.set("restitution", -1.0)?;
                component.set("friction", 0.45)?;
                component.set("touching", false)?;
                component.set("last_hit_id", 0)?;
                Ok(())
            })?,
        )?;

        collider2d.set(
            "update",
            lua.create_function(
                move |_ctx, (_entity, component, _dt): (Table, Table, f32)| {
                    component.set("touching", false)?;
                    component.set("last_hit_id", 0)?;
                    Ok(())
                },
            )?,
        )?;

        for (method_name, field_name) in [
            ("setOnCollisionEnter", "onCollisionEnter"),
            ("setOnCollisionStay", "onCollisionStay"),
            ("setOnCollisionExit", "onCollisionExit"),
            ("setOnTriggerEnter", "onTriggerEnter"),
            ("setOnTriggerStay", "onTriggerStay"),
            ("setOnTriggerExit", "onTriggerExit"),
        ] {
            collider2d.set(
                method_name,
                lua.create_function(move |_ctx, (component, callback): (Table, Value)| {
                    component.set(field_name, callback)?;
                    Ok(())
                })?,
            )?;
        }

        core_components.set("Collider2D", collider2d)?;
    }

    // Rigidbody2D
    // simple force-based body with optional window-bound collision
    {
        let rigidbody2d = lua.create_table()?;

        rigidbody2d.set(
            "awake",
            lua.create_function(move |_ctx, (_entity, component): (Table, Table)| {
                component.set("__neolove_component", "Rigidbody2D")?;
                component.set("velocity_x", 0.0)?;
                component.set("velocity_y", 0.0)?;
                component.set("force_x", 0.0)?;
                component.set("force_y", 0.0)?;
                component.set("acceleration_x", 0.0)?;
                component.set("acceleration_y", 0.0)?;
                component.set("gravity_x", 0.0)?;
                component.set("gravity_y", 980.0)?;
                component.set("gravity_scale", 1.0)?;
                component.set("mass", 1.0)?;
                component.set("inertia", 0.0)?;
                component.set("linear_damping", 0.0)?;
                component.set("angular_damping", 0.5)?;
                component.set("restitution", 0.25)?;
                component.set("friction", 0.45)?;
                component.set("sleep_epsilon", 1.0)?;
                component.set("bounds_mode", "none")?;
                component.set("freeze_x", false)?;
                component.set("freeze_y", false)?;
                component.set("freeze_rotation", false)?;
                component.set("is_static", false)?;
                component.set("collision_enabled", true)?;
                component.set("grounded", false)?;
                component.set("max_speed", 0.0)?;
                component.set("max_angular_speed", 0.0)?;
                component.set("angular_velocity", 0.0)?;
                component.set("torque", 0.0)?;
                Ok(())
            })?,
        )?;

        rigidbody2d.set(
            "addForce",
            lua.create_function(move |_ctx, (component, fx, fy): (Table, f32, f32)| {
                let current_fx: f32 = component.get::<f32>("force_x").unwrap_or(0.0);
                let current_fy: f32 = component.get::<f32>("force_y").unwrap_or(0.0);
                component.set("force_x", current_fx + fx)?;
                component.set("force_y", current_fy + fy)?;
                Ok(())
            })?,
        )?;

        rigidbody2d.set(
            "addImpulse",
            lua.create_function(move |_ctx, (component, ix, iy): (Table, f32, f32)| {
                let mass = component.get::<f32>("mass").unwrap_or(1.0).max(0.0001);
                let mut vx: f32 = component.get::<f32>("velocity_x").unwrap_or(0.0);
                let mut vy: f32 = component.get::<f32>("velocity_y").unwrap_or(0.0);
                vx += ix / mass;
                vy += iy / mass;
                component.set("velocity_x", vx)?;
                component.set("velocity_y", vy)?;
                Ok(())
            })?,
        )?;

        rigidbody2d.set(
            "addTorque",
            lua.create_function(move |_ctx, (component, torque): (Table, f32)| {
                let current_torque: f32 = component.get::<f32>("torque").unwrap_or(0.0);
                component.set("torque", current_torque + torque)?;
                Ok(())
            })?,
        )?;

        rigidbody2d.set(
            "addAngularImpulse",
            lua.create_function(move |_ctx, (component, impulse): (Table, f32)| {
                let mut inertia = component.get::<f32>("inertia").unwrap_or(0.0);
                if inertia <= 0.0 {
                    let mass = component.get::<f32>("mass").unwrap_or(1.0).max(0.0001);
                    inertia = mass;
                }
                let mut omega: f32 = component.get::<f32>("angular_velocity").unwrap_or(0.0);
                omega += impulse / inertia.max(0.0001);
                component.set("angular_velocity", omega)?;
                Ok(())
            })?,
        )?;

        rigidbody2d.set(
            "setVelocity",
            lua.create_function(move |_ctx, (component, vx, vy): (Table, f32, f32)| {
                component.set("velocity_x", vx)?;
                component.set("velocity_y", vy)?;
                Ok(())
            })?,
        )?;

        rigidbody2d.set(
            "getVelocity",
            lua.create_function(move |_ctx, component: Table| {
                let vx: f32 = component.get::<f32>("velocity_x").unwrap_or(0.0);
                let vy: f32 = component.get::<f32>("velocity_y").unwrap_or(0.0);
                Ok((vx, vy))
            })?,
        )?;

        rigidbody2d.set(
            "setAngularVelocity",
            lua.create_function(move |_ctx, (component, omega): (Table, f32)| {
                component.set("angular_velocity", omega)?;
                Ok(())
            })?,
        )?;

        rigidbody2d.set(
            "getAngularVelocity",
            lua.create_function(move |_ctx, component: Table| {
                let omega: f32 = component.get::<f32>("angular_velocity").unwrap_or(0.0);
                Ok(omega)
            })?,
        )?;

        rigidbody2d.set(
            "setGravity",
            lua.create_function(move |_ctx, (component, gx, gy): (Table, f32, f32)| {
                component.set("gravity_x", gx)?;
                component.set("gravity_y", gy)?;
                Ok(())
            })?,
        )?;

        rigidbody2d.set(
            "update",
            lua.create_function(move |ctx, (entity, component, dt): (Table, Table, f32)| {
                let _ = ctx;
                let _ = entity;
                let _ = dt;

                component.set("grounded", false)?;
                if component.get::<bool>("is_static").unwrap_or(false) {
                    component.set("velocity_x", 0.0)?;
                    component.set("velocity_y", 0.0)?;
                    component.set("angular_velocity", 0.0)?;
                }
                Ok(())
            })?,
        )?;

        core_components.set("Rigidbody2D", rigidbody2d)?;
    }

    // Rope2D / String2D
    // distance constraint between two entities, solved globally each frame
    {
        let rope2d = lua.create_table()?;

        rope2d.set(
            "awake",
            lua.create_function(move |_ctx, (_entity, component): (Table, Table)| {
                component.set("__neolove_component", "Rope2D")?;
                component.set("enabled", true)?;
                component.set("entity_a", Value::Nil)?;
                component.set("entity_b", Value::Nil)?;
                component.set("min_length", 0.0)?;
                component.set("max_length", 160.0)?;
                component.set("stiffness", 0.82)?;
                component.set("damping", 0.08)?;
                component.set("break_force", 0.0)?;
                component.set("current_length", 0.0)?;
                component.set("tension", 0.0)?;
                component.set("snapped", false)?;
                Ok(())
            })?,
        )?;

        rope2d.set(
            "link",
            lua.create_function(
                move |_ctx, (component, entity_a, entity_b): (Table, Table, Table)| {
                    component.set("entity_a", entity_a)?;
                    component.set("entity_b", entity_b)?;
                    component.set("snapped", false)?;
                    Ok(())
                },
            )?,
        )?;

        rope2d.set(
            "update",
            lua.create_function(
                move |_ctx, (_entity, _component, _dt): (Table, Table, f32)| Ok(()),
            )?,
        )?;

        core_components.set("Rope2D", rope2d.clone())?;
        core_components.set("String2D", rope2d)?;
    }

    lua.globals().set("core", core_components)?;
    Ok(())
}
