#![allow(dead_code)]

use crate::assets::ImageHandle;
use crate::lua_error::protect_lua_call;
use crate::platform::{Color, InputState, SharedPlatformState, WindowState, lock_platform_state};
use crate::renderer::{
    DrawCommand, FontHandle, Rect, RenderState, SharedRenderState, TextAlignX, TextAlignY,
    TextAntialiasing, TextRenderRequest, TextScaleMode, TextStyleRange, TextWrapMode,
    TextureFilter, Vec2,
};
use mlua::{AnyUserData, Function, Lua, Table, UserData, Value};
use std::path::{Component, Path, PathBuf};

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

fn rich_text_ranges_from_component(
    root: &Path,
    component: &Table,
) -> mlua::Result<Vec<TextStyleRange>> {
    let Ok(ranges) = component.get::<Table>("__rich_text_ranges") else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for pair in ranges.sequence_values::<Table>() {
        let range = pair?;
        let start = range.get::<usize>("start").unwrap_or(0);
        let end = range.get::<usize>("end").unwrap_or(start);
        if end <= start {
            continue;
        }
        let color = match range.get::<Table>("color") {
            Ok(c) => Some([
                c.get::<f32>("r")?.clamp(0.0, 255.0) as u8,
                c.get::<f32>("g")?.clamp(0.0, 255.0) as u8,
                c.get::<f32>("b")?.clamp(0.0, 255.0) as u8,
                c.get::<f32>("a").unwrap_or(255.0).clamp(0.0, 255.0) as u8,
            ]),
            Err(_) => None,
        };
        let font = match range.get::<String>("font") {
            Ok(path) => resolve_font_path(root, &path)
                .map(FontHandle::Path)
                .or(Some(FontHandle::Default))
                .filter(|_| !path.trim().is_empty()),
            Err(_) => None,
        };
        out.push(TextStyleRange {
            start,
            end,
            bold: range.get::<bool>("bold").unwrap_or(false),
            italic: range.get::<bool>("italic").unwrap_or(false),
            underline: range.get::<bool>("underline").unwrap_or(false),
            color,
            size: range.get::<f32>("size").ok().map(f32::to_bits),
            font,
            offset_x: range
                .get::<f32>("offset_x")
                .ok()
                .filter(|value| value.is_finite())
                .map(f32::to_bits),
            offset_y: range
                .get::<f32>("offset_y")
                .ok()
                .filter(|value| value.is_finite())
                .map(f32::to_bits),
        });
    }
    Ok(out)
}

fn shader_from_component(component: &Table) -> mlua::Result<Option<crate::shader::ShaderHandle>> {
    let shader_ud: Option<AnyUserData> = component.get("shader")?;
    let Some(shader_ud) = shader_ud else {
        return Ok(None);
    };
    let shader = shader_ud.borrow::<crate::shader::ShaderHandle>()?;
    Ok(Some(shader.clone()))
}

fn rotate_local(x: f32, y: f32, rotation: f32) -> (f32, f32) {
    let cos_r = rotation.cos();
    let sin_r = rotation.sin();
    (x * cos_r - y * sin_r, x * sin_r + y * cos_r)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VisibleTileCells {
    column_start: usize,
    column_end: usize,
    row_start: usize,
    row_end: usize,
}

/// Conservative tile-cell culling in the layer's unrotated coordinate space.
/// Rotating the viewport corners back into the map makes this work for both
/// ordinary and rotated tilemaps while keeping large maps from queueing every
/// off-screen tile each frame.
#[allow(clippy::too_many_arguments)]
fn visible_tile_cells(
    base_x: f32,
    base_y: f32,
    width: f32,
    height: f32,
    pivot: Vec2,
    rotation: f32,
    viewport_width: f32,
    viewport_height: f32,
    columns: usize,
    rows: usize,
) -> Option<VisibleTileCells> {
    if width <= 0.0
        || height <= 0.0
        || viewport_width <= 0.0
        || viewport_height <= 0.0
        || columns == 0
        || rows == 0
    {
        return None;
    }

    let mut left = f32::INFINITY;
    let mut top = f32::INFINITY;
    let mut right = f32::NEG_INFINITY;
    let mut bottom = f32::NEG_INFINITY;
    for (screen_x, screen_y) in [
        (0.0, 0.0),
        (viewport_width, 0.0),
        (viewport_width, viewport_height),
        (0.0, viewport_height),
    ] {
        let (local_x, local_y) = rotate_local(screen_x - pivot.x, screen_y - pivot.y, -rotation);
        let x = pivot.x + local_x;
        let y = pivot.y + local_y;
        left = left.min(x);
        top = top.min(y);
        right = right.max(x);
        bottom = bottom.max(y);
    }

    left = left.max(base_x);
    top = top.max(base_y);
    right = right.min(base_x + width);
    bottom = bottom.min(base_y + height);
    if right <= left || bottom <= top {
        return None;
    }

    let cell_width = width / columns as f32;
    let cell_height = height / rows as f32;
    let column_start =
        (((left - base_x) / cell_width).floor() as isize).clamp(0, columns as isize) as usize;
    let column_end =
        (((right - base_x) / cell_width).ceil() as isize).clamp(0, columns as isize) as usize;
    let row_start =
        (((top - base_y) / cell_height).floor() as isize).clamp(0, rows as isize) as usize;
    let row_end =
        (((bottom - base_y) / cell_height).ceil() as isize).clamp(0, rows as isize) as usize;

    (column_start < column_end && row_start < row_end).then_some(VisibleTileCells {
        column_start,
        column_end,
        row_start,
        row_end,
    })
}

fn particle_random(seed: &mut u32) -> f32 {
    *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    (*seed as f32) / (u32::MAX as f32)
}

fn lerp_particle_color(start: Color, end: Color, t: f32) -> Color {
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)).round() as u8;
    Color::rgba(
        mix(start.r, end.r),
        mix(start.g, end.g),
        mix(start.b, end.b),
        mix(start.a, end.a),
    )
}

fn read_particle_color_sequence(
    component: &Table,
    start: Color,
    end: Color,
) -> mlua::Result<Vec<(f32, Color)>> {
    let mut keypoints = Vec::new();
    if let Ok(sequence) = component.get::<Table>("color_sequence") {
        for entry in sequence.sequence_values::<Table>() {
            let entry = entry?;
            if let Ok(color) = entry.get::<Table>("color") {
                keypoints.push((
                    entry.get::<f32>("time").unwrap_or(0.0).clamp(0.0, 1.0),
                    color4_to_color(color)?,
                ));
            }
        }
    }
    if keypoints.len() < 2 {
        keypoints = vec![(0.0, start), (1.0, end)];
    }
    keypoints.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(keypoints)
}

fn read_particle_number_sequence(
    component: &Table,
    start: f32,
    end: f32,
) -> mlua::Result<Vec<(f32, f32)>> {
    let mut keypoints = Vec::new();
    if let Ok(sequence) = component.get::<Table>("transparency_sequence") {
        for entry in sequence.sequence_values::<Table>() {
            let entry = entry?;
            keypoints.push((
                entry.get::<f32>("time").unwrap_or(0.0).clamp(0.0, 1.0),
                entry.get::<f32>("value").unwrap_or(0.0).clamp(0.0, 1.0),
            ));
        }
    }
    if keypoints.len() < 2 {
        keypoints = vec![(0.0, start), (1.0, end)];
    }
    keypoints.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(keypoints)
}

fn sample_particle_color(keypoints: &[(f32, Color)], time: f32) -> Color {
    let Some(first) = keypoints.first() else {
        return Color::WHITE;
    };
    let time = time.clamp(0.0, 1.0);
    if time <= first.0 {
        return first.1;
    }
    for pair in keypoints.windows(2) {
        if time <= pair[1].0 {
            let amount = (time - pair[0].0) / (pair[1].0 - pair[0].0).max(f32::EPSILON);
            return lerp_particle_color(pair[0].1, pair[1].1, amount);
        }
    }
    keypoints
        .last()
        .map(|keypoint| keypoint.1)
        .unwrap_or(first.1)
}

fn sample_particle_number(keypoints: &[(f32, f32)], time: f32) -> f32 {
    let Some(first) = keypoints.first() else {
        return 0.0;
    };
    let time = time.clamp(0.0, 1.0);
    if time <= first.0 {
        return first.1;
    }
    for pair in keypoints.windows(2) {
        if time <= pair[1].0 {
            let amount = (time - pair[0].0) / (pair[1].0 - pair[0].0).max(f32::EPSILON);
            return pair[0].1 + (pair[1].1 - pair[0].1) * amount;
        }
    }
    keypoints
        .last()
        .map(|keypoint| keypoint.1)
        .unwrap_or(first.1)
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
                && let Some(path) = resolve_font_path(root, &path)
            {
                return FontHandle::Path(path);
            }

            if let Ok(builtin) = table
                .get::<String>("builtin")
                .or_else(|_| table.get::<String>("name"))
                && builtin.trim().eq_ignore_ascii_case("default")
            {
                return FontHandle::Default;
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

fn parse_text_antialiasing(raw: &str) -> TextAntialiasing {
    match raw.trim().to_ascii_lowercase().as_str() {
        "off" | "none" | "disabled" | "pixel" => TextAntialiasing::Off,
        "standard" | "fast" | "normal" | "on" => TextAntialiasing::Standard,
        _ => TextAntialiasing::High,
    }
}

fn component_text_antialiasing(lua: &Lua, component: &Table) -> TextAntialiasing {
    let component_mode = component
        .get::<String>("antialiasing")
        .unwrap_or_else(|_| "inherit".to_string());
    if !component_mode.eq_ignore_ascii_case("inherit") {
        return parse_text_antialiasing(&component_mode);
    }
    let app_mode = lua
        .globals()
        .get::<Table>("app")
        .ok()
        .and_then(|app| app.get::<String>("antiAliasing").ok())
        .unwrap_or_else(|| "high".to_string());
    parse_text_antialiasing(&app_mode)
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

pub(crate) fn begin_ui_frame(render_state: &SharedRenderState) {
    crate::widget_interaction::begin_frame(interaction_surface(render_state));
}

fn current_input_snapshot(
    platform: &SharedPlatformState,
    render_state: &SharedRenderState,
) -> mlua::Result<UiInputSnapshot> {
    let mut snapshot = {
        let platform = lock_platform_state(platform);
        let mouse = platform.mouse();
        UiInputSnapshot {
            mouse: Vec2 {
                x: mouse.x,
                y: mouse.y,
            },
            input: platform.input().clone(),
            window: platform.window(),
        }
    };
    let camera = render_state
        .lock()
        .map_err(|_| mlua::Error::external("render state lock poisoned"))?
        .camera_offset();
    snapshot.mouse.x -= camera.x;
    snapshot.mouse.y -= camera.y;
    Ok(snapshot)
}

fn camera_offset_for_entity(entity: &Table, platform: &SharedPlatformState) -> mlua::Result<Vec2> {
    let (camera_x, camera_y) = crate::window::get_global_position(entity)?;
    let window = lock_platform_state(platform).window();
    Ok(Vec2 {
        x: window.width * 0.5 - camera_x,
        y: window.height * 0.5 - camera_y,
    })
}

fn camera_3d_for_entity(
    entity: &Table,
    component: &Table,
) -> mlua::Result<crate::render3d::Camera3D> {
    let transform = crate::window::get_global_transform_3d(entity)?;
    let projection = component
        .get::<String>("projection")
        .unwrap_or_else(|_| "perspective".to_string());
    Ok(crate::render3d::Camera3D {
        position: transform.position,
        euler: transform.euler,
        projection: if projection.eq_ignore_ascii_case("orthographic") {
            crate::render3d::Projection3D::Orthographic
        } else {
            crate::render3d::Projection3D::Perspective
        },
        fov: component
            .get::<f32>("fov")
            .unwrap_or(60.0)
            .clamp(1.0, 179.0),
        orthographic_size: component
            .get::<f32>("orthographic_size")
            .unwrap_or(10.0)
            .max(0.0001),
        near_clip: component.get::<f32>("near_clip").unwrap_or(0.1).max(0.0001),
        far_clip: component
            .get::<f32>("far_clip")
            .unwrap_or(1000.0)
            .max(0.0002),
    })
}

fn forward_from_euler(euler: crate::render3d::Vec3) -> crate::render3d::Vec3 {
    crate::render3d::Mat4::rotation_euler_degrees(euler)
        .transform_direction(crate::render3d::Vec3::new(0.0, 0.0, -1.0))
}

fn environment_3d_from_component(
    lua: &Lua,
    component: &Table,
) -> mlua::Result<crate::environment3d::Environment3D> {
    use crate::environment3d::{Environment3D, EnvironmentMode3D};

    let mut environment = Environment3D::default();
    environment.enabled = component.get::<bool>("enabled").unwrap_or(true);
    let mode = component
        .get::<String>("mode")
        .unwrap_or_else(|_| "gradient".to_string())
        .to_ascii_lowercase();
    environment.mode = match mode.as_str() {
        "solid" | "color" | "colour" => EnvironmentMode3D::Solid,
        "equirectangular" | "panorama" | "skybox" | "texture" => EnvironmentMode3D::Equirectangular,
        _ => EnvironmentMode3D::Gradient,
    };
    if let Ok(color) = component.get::<Table>("color") {
        environment.solid = color4_to_color(color)?;
    }
    if let Ok(color) = component.get::<Table>("top_color") {
        environment.top = color4_to_color(color)?;
    }
    if let Ok(color) = component.get::<Table>("bottom_color") {
        environment.bottom = color4_to_color(color)?;
    }
    environment.intensity = component.get::<f32>("intensity").unwrap_or(1.0).max(0.0);
    environment.rotation_degrees = component.get::<f32>("rotation").unwrap_or(0.0);
    environment.equirectangular = get_image_field(component, "texture")?;
    if environment.equirectangular.is_none() {
        let path = component.get::<String>("texture_path").unwrap_or_default();
        if !path.trim().is_empty() {
            let assets: Table = lua.globals().get("assets")?;
            let image: AnyUserData = assets.get::<Function>("loadImage")?.call(path)?;
            environment.equirectangular = Some(image.borrow::<ImageHandle>()?.clone());
            component.set("texture", image)?;
        }
    }
    Ok(environment)
}

fn particle_config_3d_from_component(
    component: &Table,
) -> mlua::Result<crate::particles3d::ParticleConfig3D> {
    use crate::particles3d::{EmissionShape3D, ParticleConfig3D};
    use crate::render3d::Vec3;

    let defaults = ParticleConfig3D::default();
    let shape = component
        .get::<String>("shape")
        .unwrap_or_else(|_| "point".to_string())
        .to_ascii_lowercase();
    let lifetime = component.get::<f32>("lifetime").unwrap_or(1.5);
    let speed = component.get::<f32>("speed").unwrap_or(2.0);
    let color = |name: &str, fallback: Color| -> mlua::Result<Color> {
        match component.get::<Table>(name) {
            Ok(value) => color4_to_color(value),
            Err(_) => Ok(fallback),
        }
    };
    Ok(ParticleConfig3D {
        max_particles: component
            .get::<i32>("max_particles")
            .unwrap_or(defaults.max_particles as i32)
            .clamp(1, crate::particles3d::MAX_PARTICLES_PER_EMITTER as i32)
            as usize,
        playing: component.get::<bool>("playing").unwrap_or(true),
        looping: component.get::<bool>("looping").unwrap_or(true),
        duration: component.get::<f32>("duration").unwrap_or(5.0).max(0.0),
        emission_rate: component
            .get::<f32>("emission_rate")
            .unwrap_or(defaults.emission_rate)
            .max(0.0),
        shape: match shape.as_str() {
            "box" | "cube" => EmissionShape3D::Box,
            "sphere" | "ball" => EmissionShape3D::Sphere,
            "cone" => EmissionShape3D::Cone,
            _ => EmissionShape3D::Point,
        },
        box_extents: Vec3::new(
            component.get::<f32>("box_size_x").unwrap_or(2.0).abs() * 0.5,
            component.get::<f32>("box_size_y").unwrap_or(2.0).abs() * 0.5,
            component.get::<f32>("box_size_z").unwrap_or(2.0).abs() * 0.5,
        ),
        sphere_radius: component
            .get::<f32>("sphere_radius")
            .or_else(|_| component.get::<f32>("radius"))
            .unwrap_or(1.0)
            .max(0.0),
        cone_angle_degrees: component
            .get::<f32>("cone_angle")
            .unwrap_or(defaults.cone_angle_degrees)
            .abs(),
        cone_length: component
            .get::<f32>("cone_length")
            .unwrap_or(defaults.cone_length)
            .max(0.0),
        direction: Vec3::new(
            component.get::<f32>("direction_x").unwrap_or(0.0),
            component.get::<f32>("direction_y").unwrap_or(1.0),
            component.get::<f32>("direction_z").unwrap_or(0.0),
        ),
        spread_degrees: component
            .get::<f32>("spread")
            .unwrap_or(defaults.spread_degrees)
            .abs(),
        lifetime_min: component
            .get::<f32>("lifetime_min")
            .unwrap_or(lifetime)
            .max(0.001),
        lifetime_max: component
            .get::<f32>("lifetime_max")
            .unwrap_or(lifetime)
            .max(0.001),
        speed_min: component.get::<f32>("speed_min").unwrap_or(speed).max(0.0),
        speed_max: component.get::<f32>("speed_max").unwrap_or(speed).max(0.0),
        gravity: Vec3::new(
            component.get::<f32>("gravity_x").unwrap_or(0.0),
            component.get::<f32>("gravity_y").unwrap_or(-9.81),
            component.get::<f32>("gravity_z").unwrap_or(0.0),
        ),
        drag: component.get::<f32>("drag").unwrap_or(0.0).max(0.0),
        start_size: component
            .get::<f32>("start_size")
            .unwrap_or(defaults.start_size)
            .max(0.0),
        end_size: component
            .get::<f32>("end_size")
            .unwrap_or(defaults.end_size)
            .max(0.0),
        start_color: color("start_color", defaults.start_color)?,
        end_color: color("end_color", defaults.end_color)?,
        start_rotation_degrees: component.get::<f32>("start_rotation").unwrap_or(0.0),
        angular_velocity_degrees: component.get::<f32>("angular_velocity").unwrap_or(0.0),
    })
}

fn particle_emitter_3d_from_component(
    component: &Table,
) -> mlua::Result<crate::particles3d::ParticleEmitterHandle> {
    let state = component.get::<AnyUserData>("__particle_state")?;
    let emitter = state
        .borrow::<crate::particles3d::ParticleEmitterHandle>()?
        .clone();
    Ok(emitter)
}

fn post_process_number(options: &Option<Table>, name: &str, default: f32) -> f32 {
    options
        .as_ref()
        .and_then(|table| table.get::<f32>(name).ok())
        .filter(|value| value.is_finite())
        .unwrap_or(default)
}

fn post_process_bool(options: &Option<Table>, name: &str, default: bool) -> bool {
    options
        .as_ref()
        .and_then(|table| table.get::<bool>(name).ok())
        .unwrap_or(default)
}

fn post_process_effect(
    kind: &str,
    options: Option<Table>,
) -> mlua::Result<crate::post_process::Effect> {
    use crate::post_process::*;

    let normalized = kind.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    match normalized.as_str() {
        "bloom" => {
            let defaults = BloomConfig::default();
            Ok(Effect::Bloom(BloomConfig {
                threshold: post_process_number(&options, "threshold", defaults.threshold),
                intensity: post_process_number(&options, "intensity", defaults.intensity),
                radius: post_process_number(&options, "radius", defaults.radius as f32)
                    .round()
                    .clamp(0.0, 64.0) as u32,
            }))
        }
        "pixelate" | "pixelated" | "pixelation" => {
            let defaults = PixelateConfig::default();
            Ok(Effect::Pixelate(PixelateConfig {
                block_size: post_process_number(&options, "block_size", defaults.block_size as f32)
                    .round()
                    .clamp(1.0, 4096.0) as u32,
            }))
        }
        "chromatic_aberration" | "chromatic_abberation" | "chromatic" => {
            let defaults = ChromaticAberrationConfig::default();
            Ok(Effect::ChromaticAberration(ChromaticAberrationConfig {
                offset_pixels: post_process_number(
                    &options,
                    "offset_pixels",
                    defaults.offset_pixels,
                ),
                angle_degrees: post_process_number(
                    &options,
                    "angle_degrees",
                    defaults.angle_degrees,
                ),
            }))
        }
        "motion_blur" | "motionblur" => {
            let defaults = MotionBlurConfig::default();
            Ok(Effect::MotionBlur(MotionBlurConfig {
                strength: post_process_number(&options, "strength", defaults.strength),
            }))
        }
        "quantization" | "quantize" | "posterize" => {
            let defaults = QuantizationConfig::default();
            Ok(Effect::Quantization(QuantizationConfig {
                levels: post_process_number(&options, "levels", defaults.levels as f32)
                    .round()
                    .clamp(2.0, 255.0) as u8,
                dither_strength: post_process_number(
                    &options,
                    "dither_strength",
                    defaults.dither_strength,
                ),
            }))
        }
        "vignette" => {
            let defaults = VignetteConfig::default();
            Ok(Effect::Vignette(VignetteConfig {
                strength: post_process_number(&options, "strength", defaults.strength),
                radius: post_process_number(&options, "radius", defaults.radius),
                softness: post_process_number(&options, "softness", defaults.softness),
            }))
        }
        "grayscale" | "greyscale" => {
            let defaults = GrayscaleConfig::default();
            Ok(Effect::Grayscale(GrayscaleConfig {
                amount: post_process_number(&options, "amount", defaults.amount),
            }))
        }
        "invert" | "negative" => {
            let defaults = InvertConfig::default();
            Ok(Effect::Invert(InvertConfig {
                amount: post_process_number(&options, "amount", defaults.amount),
            }))
        }
        "color_adjust" | "brightness_contrast_saturation" | "bcs" => {
            let defaults = BrightnessContrastSaturationConfig::default();
            Ok(Effect::BrightnessContrastSaturation(
                BrightnessContrastSaturationConfig {
                    brightness: post_process_number(&options, "brightness", defaults.brightness),
                    contrast: post_process_number(&options, "contrast", defaults.contrast),
                    saturation: post_process_number(&options, "saturation", defaults.saturation),
                },
            ))
        }
        "exposure" | "tonemap" | "exposure_tonemap" => {
            let defaults = ExposureTonemapConfig::default();
            let operator = options
                .as_ref()
                .and_then(|table| table.get::<String>("operator").ok())
                .unwrap_or_else(|| "reinhard".to_string());
            Ok(Effect::ExposureTonemap(ExposureTonemapConfig {
                exposure: post_process_number(&options, "exposure", defaults.exposure),
                operator: match operator.to_ascii_lowercase().as_str() {
                    "none" | "off" => TonemapOperator::None,
                    "aces" | "aces_filmic" => TonemapOperator::Aces,
                    _ => TonemapOperator::Reinhard,
                },
                gamma: post_process_number(&options, "gamma", defaults.gamma).max(0.01),
            }))
        }
        _ => Err(mlua::Error::external(format!(
            "unknown post-process effect '{kind}'"
        ))),
    }
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

fn component_owner_key(component: &Table) -> String {
    let name = component
        .get::<String>("__neolove_component")
        .unwrap_or_else(|_| "component".to_string());
    // Component type alone is not unique: an entity can legally contain two
    // widgets of the same type. The Lua table identity remains stable for the
    // life of the component and also lets methods such as TextInput:focus()
    // participate without needing its entity passed separately.
    format!("{name}:{:p}", component.to_pointer())
}

fn interaction_surface(render_state: &SharedRenderState) -> usize {
    crate::renderer::interaction_surface_id(render_state)
}

fn widget_body_region_id(owner: &str) -> String {
    format!("{owner}/body")
}

fn widget_popup_region_id(owner: &str) -> String {
    format!("{owner}/popup")
}

fn interaction_region(
    id: String,
    owner: String,
    bounds: Rect,
    pivot: Vec2,
    rotation: f32,
    layer: crate::widget_interaction::Layer,
    focusable: bool,
    enabled: bool,
) -> crate::widget_interaction::Region {
    crate::widget_interaction::Region {
        id,
        owner,
        x: bounds.x,
        y: bounds.y,
        width: bounds.w,
        height: bounds.h,
        pivot_x: pivot.x,
        pivot_y: pivot.y,
        rotation_degrees: rotation,
        layer,
        focusable,
        enabled,
    }
}

fn register_widget_region(
    surface: usize,
    owner: &str,
    draw: EntityDrawContext,
    enabled: bool,
) -> String {
    let id = widget_body_region_id(owner);
    crate::widget_interaction::register(
        surface,
        interaction_region(
            id.clone(),
            owner.to_string(),
            draw.bounds,
            draw.pivot,
            draw.rotation,
            crate::widget_interaction::Layer::Content,
            true,
            enabled,
        ),
    );
    id
}

fn register_popup(surface: usize, owner: &str, bounds: Rect, pivot: Vec2, rotation: f32) -> String {
    let id = widget_popup_region_id(owner);
    crate::widget_interaction::register(
        surface,
        interaction_region(
            id.clone(),
            owner.to_string(),
            bounds,
            pivot,
            rotation,
            crate::widget_interaction::Layer::Popup,
            false,
            true,
        ),
    );
    id
}

fn unregister_widget(render_state: &SharedRenderState, component: &Table) {
    let surface = interaction_surface(render_state);
    let owner = component_owner_key(component);
    crate::widget_interaction::unregister_owner(surface, &owner);
}

fn tab_navigation(snapshot: &UiInputSnapshot) -> (bool, bool) {
    let tab = snapshot.input.keys_pressed.contains("tab")
        || snapshot.input.last_key_pressed.as_deref() == Some("tab");
    let reverse = snapshot.input.keys_down.contains("shift")
        || snapshot.input.keys_down.contains("leftshift")
        || snapshot.input.keys_down.contains("rightshift");
    (tab, reverse)
}

fn install_widget_focus_methods(
    lua: &Lua,
    prototype: &Table,
    render_state: &SharedRenderState,
) -> mlua::Result<()> {
    let focus_state = render_state.clone();
    let focus = lua.create_function(move |_lua, component: Table| {
        let enabled = component.get::<bool>("enabled").unwrap_or(true);
        let surface = interaction_surface(&focus_state);
        let owner = component_owner_key(&component);
        if enabled {
            crate::widget_interaction::request_focus(surface, &owner);
        } else {
            crate::widget_interaction::clear_focus(surface, &owner);
        }
        component.set("focused", enabled)
    })?;
    prototype.set("focus", focus.clone())?;
    prototype.set("Focus", focus)?;

    let blur_state = render_state.clone();
    let blur = lua.create_function(move |_lua, component: Table| {
        let surface = interaction_surface(&blur_state);
        let owner = component_owner_key(&component);
        crate::widget_interaction::clear_focus(surface, &owner);
        component.set("focused", false)
    })?;
    prototype.set("blur", blur.clone())?;
    prototype.set("Blur", blur)?;
    Ok(())
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
        shader: None,
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
        shader: None,
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
    let x = get_number_key(component, &format!("{prefix}_x"))
        .or_else(|| get_number_key(component, &format!("{prefix}X")))
        .unwrap_or(0.0);
    let y = get_number_key(component, &format!("{prefix}_y"))
        .or_else(|| get_number_key(component, &format!("{prefix}Y")))
        .unwrap_or(0.0);
    let w = get_number_key(component, &format!("{prefix}_w"))
        .or_else(|| get_number_key(component, &format!("{prefix}W")))
        .or_else(|| get_number_key(component, &format!("{prefix}_width")))
        .or_else(|| get_number_key(component, &format!("{prefix}Width")))?;
    let h = get_number_key(component, &format!("{prefix}_h"))
        .or_else(|| get_number_key(component, &format!("{prefix}H")))
        .or_else(|| get_number_key(component, &format!("{prefix}_height")))
        .or_else(|| get_number_key(component, &format!("{prefix}Height")))?;
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    Some(Rect { x, y, w, h })
}

fn clamp_rect_to_bounds(rect: Rect, bounds: Rect) -> Rect {
    let left = rect.x.max(bounds.x);
    let top = rect.y.max(bounds.y);
    let right = (rect.x + rect.w).min(bounds.x + bounds.w);
    let bottom = (rect.y + rect.h).min(bounds.y + bounds.h);
    Rect {
        x: left,
        y: top,
        w: (right - left).max(0.0),
        h: (bottom - top).max(0.0),
    }
}

#[derive(Clone, Copy, Debug)]
struct ActiveSpriteboxRect {
    x0: u32,
    x1: u32,
    y0: u32,
    y1: u32,
}

#[derive(Clone, Debug, Default)]
struct SpriteboxShape {
    rects: Vec<Rect>,
    bounds: Rect,
}

#[derive(Clone, Debug)]
struct SpriteboxShapeHandle {
    shape: SpriteboxShape,
}

impl UserData for SpriteboxShapeHandle {}

#[derive(Clone, Debug)]
struct SpriteboxSource {
    image: ImageHandle,
    source: Rect,
}

#[derive(Clone, Copy, Debug)]
struct SpriteboxWorldRect {
    corners: [Vec2; 4],
    bounds: Rect,
}

#[derive(Clone, Debug, Default)]
struct SpriteboxWorldShape {
    rects: Vec<SpriteboxWorldRect>,
    bounds: Rect,
}

#[derive(Clone, Debug)]
struct SpriteboxWorldShapeHandle {
    shape: SpriteboxWorldShape,
}

impl UserData for SpriteboxWorldShapeHandle {}

fn is_sprite_component_name(name: &str) -> bool {
    matches!(
        name,
        "Sprite2D" | "Image2D" | "NineSliceSprite2D" | "9SliceSprite2D"
    )
}

fn is_spritebox_component(component: &Table) -> bool {
    component
        .get::<String>("__neolove_component")
        .map(|name| name == "Spritebox2D")
        .unwrap_or(false)
}

fn sprite_source_rect(component: &Table, image: &ImageHandle) -> mlua::Result<Rect> {
    let (image_w, image_h) = image.dimensions()?;
    let image_bounds = Rect {
        x: 0.0,
        y: 0.0,
        w: image_w as f32,
        h: image_h as f32,
    };
    Ok(get_source_rect(component, "source")
        .map(|source| clamp_rect_to_bounds(source, image_bounds))
        .filter(|source| source.w > 0.0 && source.h > 0.0)
        .unwrap_or(image_bounds))
}

fn find_spritebox_source(
    entity: &Table,
    spritebox: &Table,
) -> mlua::Result<Option<SpriteboxSource>> {
    let components: Table = entity.get("components")?;
    for component in components.sequence_values::<Table>() {
        let component = component?;
        if component.to_pointer() == spritebox.to_pointer() {
            continue;
        }

        let Ok(component_name) = component.get::<String>("__neolove_component") else {
            continue;
        };
        if !is_sprite_component_name(&component_name) {
            continue;
        }

        let Some(image) = get_image_field(&component, "image")? else {
            continue;
        };
        let source = sprite_source_rect(&component, &image)?;
        return Ok(Some(SpriteboxSource { image, source }));
    }

    Ok(None)
}

fn push_spritebox_pixel_rect(
    out: &mut Vec<Rect>,
    rect: ActiveSpriteboxRect,
    source_x: u32,
    source_y: u32,
    source_w: f32,
    source_h: f32,
) {
    if rect.x1 <= rect.x0 || rect.y1 <= rect.y0 || source_w <= 0.0 || source_h <= 0.0 {
        return;
    }

    out.push(Rect {
        x: (rect.x0.saturating_sub(source_x)) as f32 / source_w,
        y: (rect.y0.saturating_sub(source_y)) as f32 / source_h,
        w: (rect.x1 - rect.x0) as f32 / source_w,
        h: (rect.y1 - rect.y0) as f32 / source_h,
    });
}

fn spritebox_bounds(rects: &[Rect]) -> Rect {
    let Some(first) = rects.first().copied() else {
        return Rect::default();
    };

    let mut min_x = first.x;
    let mut min_y = first.y;
    let mut max_x = first.x + first.w;
    let mut max_y = first.y + first.h;
    for rect in rects.iter().skip(1) {
        min_x = min_x.min(rect.x);
        min_y = min_y.min(rect.y);
        max_x = max_x.max(rect.x + rect.w);
        max_y = max_y.max(rect.y + rect.h);
    }

    Rect {
        x: min_x,
        y: min_y,
        w: (max_x - min_x).max(0.0),
        h: (max_y - min_y).max(0.0),
    }
}

fn build_spritebox_shape(
    image: &ImageHandle,
    source: Rect,
    alpha_threshold: u8,
) -> mlua::Result<SpriteboxShape> {
    image.with_image(|image| {
        let sx0 = source.x.floor().clamp(0.0, image.width() as f32) as u32;
        let sy0 = source.y.floor().clamp(0.0, image.height() as f32) as u32;
        let sx1 = (source.x + source.w)
            .ceil()
            .clamp(0.0, image.width() as f32) as u32;
        let sy1 = (source.y + source.h)
            .ceil()
            .clamp(0.0, image.height() as f32) as u32;
        if sx1 <= sx0 || sy1 <= sy0 {
            return SpriteboxShape::default();
        }

        let source_w = (sx1 - sx0) as f32;
        let source_h = (sy1 - sy0) as f32;
        let mut rects = Vec::<Rect>::new();
        let mut active = Vec::<ActiveSpriteboxRect>::new();

        for y in sy0..sy1 {
            let mut spans = Vec::<(u32, u32)>::new();
            let mut x = sx0;
            while x < sx1 {
                while x < sx1 && image.get_pixel(x, y).0[3] <= alpha_threshold {
                    x += 1;
                }
                if x >= sx1 {
                    break;
                }
                let start = x;
                while x < sx1 && image.get_pixel(x, y).0[3] > alpha_threshold {
                    x += 1;
                }
                spans.push((start, x));
            }

            let mut next_active = Vec::with_capacity(spans.len());
            for (x0, x1) in spans {
                if let Some(index) = active
                    .iter()
                    .position(|rect| rect.x0 == x0 && rect.x1 == x1)
                {
                    let mut rect = active.swap_remove(index);
                    rect.y1 = y + 1;
                    next_active.push(rect);
                } else {
                    next_active.push(ActiveSpriteboxRect {
                        x0,
                        x1,
                        y0: y,
                        y1: y + 1,
                    });
                }
            }

            for rect in active.drain(..) {
                push_spritebox_pixel_rect(&mut rects, rect, sx0, sy0, source_w, source_h);
            }
            active = next_active;
        }

        for rect in active {
            push_spritebox_pixel_rect(&mut rects, rect, sx0, sy0, source_w, source_h);
        }

        let bounds = spritebox_bounds(&rects);
        SpriteboxShape { rects, bounds }
    })
}

fn write_spritebox_shape(lua: &Lua, component: &Table, shape: &SpriteboxShape) -> mlua::Result<()> {
    let revision = component
        .raw_get::<i64>("__spritebox_revision")
        .unwrap_or(0)
        .saturating_add(1);
    component.raw_set("__spritebox_revision", revision)?;
    component.set(
        "__spritebox_shape",
        lua.create_userdata(SpriteboxShapeHandle {
            shape: shape.clone(),
        })?,
    )?;
    component.set("__spritebox_world_shape", Value::Nil)?;
    component.set("__spritebox_rects", Value::Nil)?;
    component.set("computed", true)?;
    component.set("rect_count", shape.rects.len())?;
    component.set("bounds_x", shape.bounds.x)?;
    component.set("bounds_y", shape.bounds.y)?;
    component.set("bounds_w", shape.bounds.w)?;
    component.set("bounds_h", shape.bounds.h)?;
    Ok(())
}

fn read_spritebox_shape(component: &Table) -> mlua::Result<Option<SpriteboxShape>> {
    if !component.get::<bool>("computed").unwrap_or(false) {
        return Ok(None);
    }

    if let Some(shape) = component
        .get::<Option<AnyUserData>>("__spritebox_shape")
        .unwrap_or(None)
    {
        let shape = shape.borrow::<SpriteboxShapeHandle>()?;
        return Ok(Some(shape.shape.clone()));
    }

    let Some(rect_table) = component
        .get::<Option<Table>>("__spritebox_rects")
        .unwrap_or(None)
    else {
        return Ok(None);
    };

    let rect_count = component.get::<usize>("rect_count").unwrap_or(0);
    let mut rects = Vec::with_capacity(rect_count);
    for index in 0..rect_count {
        let base = index * 4 + 1;
        let rect = Rect {
            x: rect_table.raw_get::<f32>(base).unwrap_or(0.0),
            y: rect_table.raw_get::<f32>(base + 1).unwrap_or(0.0),
            w: rect_table.raw_get::<f32>(base + 2).unwrap_or(0.0),
            h: rect_table.raw_get::<f32>(base + 3).unwrap_or(0.0),
        };
        if rect.w > 0.0 && rect.h > 0.0 {
            rects.push(rect);
        }
    }

    let bounds = if rects.is_empty() {
        Rect::default()
    } else {
        Rect {
            x: component.get::<f32>("bounds_x").unwrap_or(0.0),
            y: component.get::<f32>("bounds_y").unwrap_or(0.0),
            w: component.get::<f32>("bounds_w").unwrap_or(0.0),
            h: component.get::<f32>("bounds_h").unwrap_or(0.0),
        }
    };
    Ok(Some(SpriteboxShape { rects, bounds }))
}

fn spritebox_entity(component: &Table) -> mlua::Result<Table> {
    component
        .get::<Option<Table>>("entity")?
        .ok_or_else(|| mlua::Error::external("Spritebox2D is not attached to an entity"))
}

fn resolve_spritebox_component(value: Value) -> mlua::Result<Option<Table>> {
    let Value::Table(table) = value else {
        return Ok(None);
    };

    if is_spritebox_component(&table) {
        return Ok(Some(table));
    }

    if let Ok(components) = table.get::<Table>("components") {
        for component in components.sequence_values::<Table>() {
            let component = component?;
            if is_spritebox_component(&component) {
                return Ok(Some(component));
            }
        }
    }

    Ok(None)
}

fn transform_local_point(origin: Vec2, rotation: f32, x: f32, y: f32) -> Vec2 {
    let (rx, ry) = rotate_local(x, y, rotation);
    Vec2 {
        x: origin.x + rx,
        y: origin.y + ry,
    }
}

fn bounds_from_points(points: &[Vec2]) -> Rect {
    let Some(first) = points.first().copied() else {
        return Rect::default();
    };

    let mut min_x = first.x;
    let mut min_y = first.y;
    let mut max_x = first.x;
    let mut max_y = first.y;
    for point in points.iter().skip(1) {
        min_x = min_x.min(point.x);
        min_y = min_y.min(point.y);
        max_x = max_x.max(point.x);
        max_y = max_y.max(point.y);
    }

    Rect {
        x: min_x,
        y: min_y,
        w: (max_x - min_x).max(0.0),
        h: (max_y - min_y).max(0.0),
    }
}

fn rect_aabb_intersects(a: Rect, b: Rect) -> bool {
    a.w >= 0.0
        && a.h >= 0.0
        && b.w >= 0.0
        && b.h >= 0.0
        && a.x <= b.x + b.w
        && a.x + a.w >= b.x
        && a.y <= b.y + b.h
        && a.y + a.h >= b.y
}

fn spritebox_world_rect(origin: Vec2, rotation: f32, size: Vec2, rect: Rect) -> SpriteboxWorldRect {
    let x0 = rect.x * size.x;
    let y0 = rect.y * size.y;
    let x1 = (rect.x + rect.w) * size.x;
    let y1 = (rect.y + rect.h) * size.y;
    let corners = [
        transform_local_point(origin, rotation, x0, y0),
        transform_local_point(origin, rotation, x1, y0),
        transform_local_point(origin, rotation, x1, y1),
        transform_local_point(origin, rotation, x0, y1),
    ];
    SpriteboxWorldRect {
        corners,
        bounds: bounds_from_points(&corners),
    }
}

fn spritebox_world_cache_matches(
    component: &Table,
    origin_x: f32,
    origin_y: f32,
    rotation: f32,
    width: f32,
    height: f32,
    revision: i64,
) -> bool {
    component
        .raw_get::<i64>("__spritebox_world_revision")
        .unwrap_or(-1)
        == revision
        && component
            .raw_get::<f32>("__spritebox_world_x")
            .unwrap_or(f32::NAN)
            == origin_x
        && component
            .raw_get::<f32>("__spritebox_world_y")
            .unwrap_or(f32::NAN)
            == origin_y
        && component
            .raw_get::<f32>("__spritebox_world_rotation")
            .unwrap_or(f32::NAN)
            == rotation
        && component
            .raw_get::<f32>("__spritebox_world_w")
            .unwrap_or(f32::NAN)
            == width
        && component
            .raw_get::<f32>("__spritebox_world_h")
            .unwrap_or(f32::NAN)
            == height
}

fn write_spritebox_world_cache(
    lua: &Lua,
    component: &Table,
    shape: &SpriteboxWorldShape,
    origin_x: f32,
    origin_y: f32,
    rotation: f32,
    width: f32,
    height: f32,
    revision: i64,
) -> mlua::Result<()> {
    component.raw_set("__spritebox_world_revision", revision)?;
    component.raw_set("__spritebox_world_x", origin_x)?;
    component.raw_set("__spritebox_world_y", origin_y)?;
    component.raw_set("__spritebox_world_rotation", rotation)?;
    component.raw_set("__spritebox_world_w", width)?;
    component.raw_set("__spritebox_world_h", height)?;
    component.set(
        "__spritebox_world_shape",
        lua.create_userdata(SpriteboxWorldShapeHandle {
            shape: shape.clone(),
        })?,
    )?;
    Ok(())
}

fn build_world_spritebox_shape(
    lua: &Lua,
    component: &Table,
) -> mlua::Result<Option<SpriteboxWorldShape>> {
    let Some(shape) = read_spritebox_shape(component)? else {
        return Ok(None);
    };
    if shape.rects.is_empty() {
        return Ok(Some(SpriteboxWorldShape::default()));
    }

    let entity = spritebox_entity(component)?;
    let (origin_x, origin_y, rotation) = crate::window::get_global_transform(&entity)?;
    let (width, height) = crate::window::get_global_size(&entity)?;
    if width <= 0.0 || height <= 0.0 {
        return Ok(Some(SpriteboxWorldShape::default()));
    }
    let revision = component
        .raw_get::<i64>("__spritebox_revision")
        .unwrap_or(0);

    if spritebox_world_cache_matches(
        component, origin_x, origin_y, rotation, width, height, revision,
    ) {
        if let Some(cached) = component
            .get::<Option<AnyUserData>>("__spritebox_world_shape")
            .unwrap_or(None)
        {
            let cached = cached.borrow::<SpriteboxWorldShapeHandle>()?;
            return Ok(Some(cached.shape.clone()));
        }
    }

    let origin = Vec2 {
        x: origin_x,
        y: origin_y,
    };
    let size = Vec2 {
        x: width,
        y: height,
    };
    let rects = shape
        .rects
        .iter()
        .map(|rect| spritebox_world_rect(origin, rotation, size, *rect))
        .collect::<Vec<_>>();
    let bounds = spritebox_world_rect(origin, rotation, size, shape.bounds).bounds;
    let world_shape = SpriteboxWorldShape { rects, bounds };
    write_spritebox_world_cache(
        lua,
        component,
        &world_shape,
        origin_x,
        origin_y,
        rotation,
        width,
        height,
        revision,
    )?;
    Ok(Some(world_shape))
}

fn projection_on_axis(corners: &[Vec2; 4], axis: Vec2) -> (f32, f32) {
    let mut min = corners[0].x * axis.x + corners[0].y * axis.y;
    let mut max = min;
    for corner in corners.iter().skip(1) {
        let projected = corner.x * axis.x + corner.y * axis.y;
        min = min.min(projected);
        max = max.max(projected);
    }
    (min, max)
}

fn has_separating_axis(a: &[Vec2; 4], b: &[Vec2; 4], axis: Vec2) -> bool {
    let len_sq = axis.x * axis.x + axis.y * axis.y;
    if len_sq <= f32::EPSILON {
        return false;
    }

    let (a_min, a_max) = projection_on_axis(a, axis);
    let (b_min, b_max) = projection_on_axis(b, axis);
    a_max < b_min || b_max < a_min
}

fn spritebox_rects_intersect(a: &SpriteboxWorldRect, b: &SpriteboxWorldRect) -> bool {
    if !rect_aabb_intersects(a.bounds, b.bounds) {
        return false;
    }

    for corners in [&a.corners, &b.corners] {
        for edge in 0..2 {
            let next = edge + 1;
            let dx = corners[next].x - corners[edge].x;
            let dy = corners[next].y - corners[edge].y;
            let axis = Vec2 { x: -dy, y: dx };
            if has_separating_axis(&a.corners, &b.corners, axis) {
                return false;
            }
        }
    }

    true
}

fn point_in_spritebox_shape(component: &Table, point: Vec2) -> mlua::Result<bool> {
    let Some(shape) = read_spritebox_shape(component)? else {
        return Ok(false);
    };
    if shape.rects.is_empty() {
        return Ok(false);
    }

    let entity = spritebox_entity(component)?;
    let (origin_x, origin_y, rotation) = crate::window::get_global_transform(&entity)?;
    let (width, height) = crate::window::get_global_size(&entity)?;
    if width <= 0.0 || height <= 0.0 {
        return Ok(false);
    }

    let (local_x, local_y) = rotate_local(point.x - origin_x, point.y - origin_y, -rotation);
    let nx = local_x / width;
    let ny = local_y / height;
    if nx < shape.bounds.x
        || nx > shape.bounds.x + shape.bounds.w
        || ny < shape.bounds.y
        || ny > shape.bounds.y + shape.bounds.h
    {
        return Ok(false);
    }

    Ok(shape
        .rects
        .iter()
        .any(|rect| nx >= rect.x && nx <= rect.x + rect.w && ny >= rect.y && ny <= rect.y + rect.h))
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
        shader: None,
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
    shader: Option<crate::shader::ShaderHandle>,
    source: Option<Rect>,
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
) -> mlua::Result<()> {
    if bounds.w <= 0.0 || bounds.h <= 0.0 {
        return Ok(());
    }

    let (image_w, image_h) = image.dimensions()?;
    let image_bounds = Rect {
        x: 0.0,
        y: 0.0,
        w: image_w as f32,
        h: image_h as f32,
    };
    let source = source
        .map(|source| clamp_rect_to_bounds(source, image_bounds))
        .filter(|source| source.w > 0.0 && source.h > 0.0)
        .unwrap_or(image_bounds);
    let image_w = source.w;
    let image_h = source.h;
    let left = left.max(0.0).min(image_w);
    let right = right.max(0.0).min((image_w - left).max(0.0));
    let top = top.max(0.0).min(image_h);
    let bottom = bottom.max(0.0).min((image_h - top).max(0.0));

    if left <= 0.0 && right <= 0.0 && top <= 0.0 && bottom <= 0.0 {
        renderer.queue(DrawCommand::Image {
            image,
            dest: bounds,
            source: Some(source),
            rotation,
            pivot,
            tint,
            filter,
            shader,
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
                    x: source.x + *src_x,
                    y: source.y + *src_y,
                    w: *src_w,
                    h: *src_h,
                }),
                rotation,
                pivot,
                tint,
                filter,
                shader: shader.clone(),
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
            None,
            None,
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
    let tab_size = component
        .get::<f32>("tab_size")
        .or_else(|_| component.get::<f32>("tab_width"))
        .unwrap_or(4.0);

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
        tab_size,
        stretch_width: 0.0,
        stretch_height: 0.0,
        rich_text: rich_text_ranges_from_component(root, component).unwrap_or_default(),
        antialiasing: component
            .get::<String>("antialiasing")
            .ok()
            .map(|mode| parse_text_antialiasing(&mode))
            .unwrap_or_default(),
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

fn build_textbox_render_request(
    root: &Path,
    entity: &Table,
    component: &Table,
    antialiasing: TextAntialiasing,
) -> mlua::Result<TextRenderRequest> {
    let (x, y, rotation) = crate::window::get_global_transform(entity)?;
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
    let tab_size = component
        .get::<f32>("tab_size")
        .or_else(|_| component.get::<f32>("tab_width"))
        .unwrap_or(4.0);
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
    let size_mode_uses_entity = uses_entity_text_bounds(component);
    let legacy_scale_x = component.get::<f32>("scale_x").unwrap_or(0.0);
    let legacy_scale_y = component.get::<f32>("scale_y").unwrap_or(0.0);
    let use_legacy_stretch = !size_mode_uses_entity && legacy_scale_x > 0.0 && legacy_scale_y > 0.0;
    let font = parse_font_handle(root, component.get::<Value>("font").unwrap_or(Value::Nil));
    let effective_scale = if use_legacy_stretch {
        legacy_scale_y.max(1.0)
    } else {
        scale
    };

    let (bounds, pivot) = if size_mode_uses_entity {
        let (w, h) = crate::window::get_global_size(entity)?;
        if crate::window::uses_middle_pivot(entity) {
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

    Ok(TextRenderRequest {
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
        tab_size,
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
        rich_text: rich_text_ranges_from_component(root, component)?,
        antialiasing,
    })
}

fn write_textbox_letter_bounds(
    lua: &Lua,
    component: &Table,
    request: &TextRenderRequest,
    letter_bounds: &[Rect],
    empty_height: f32,
) -> mlua::Result<()> {
    let bounds_table = lua.create_table()?;
    for (i, rect) in letter_bounds.iter().enumerate() {
        let entry = lua.create_table()?;
        entry.set("x", rect.x)?;
        entry.set("y", rect.y)?;
        entry.set("w", rect.w)?;
        entry.set("h", rect.h)?;
        bounds_table.set(i + 1, entry)?;
    }
    component.set("__letter_bounds", bounds_table)?;

    let (start, end) = match (letter_bounds.first(), letter_bounds.last()) {
        (Some(first), Some(last)) => (
            Rect {
                x: first.x,
                y: first.y,
                w: 0.0,
                h: first.h,
            },
            Rect {
                x: last.x + last.w,
                y: last.y,
                w: 0.0,
                h: last.h,
            },
        ),
        _ => {
            let rect = Rect {
                x: request.bounds.x + request.padding_x.max(0.0),
                y: request.bounds.y + request.padding_y.max(0.0),
                w: 0.0,
                h: empty_height.max(request.scale.max(1.0)),
            };
            (rect, rect)
        }
    };

    let start_table = lua.create_table()?;
    start_table.set("x", start.x)?;
    start_table.set("y", start.y)?;
    start_table.set("w", start.w)?;
    start_table.set("h", start.h)?;
    component.set("__letter_caret_start", start_table)?;

    let end_table = lua.create_table()?;
    end_table.set("x", end.x)?;
    end_table.set("y", end.y)?;
    end_table.set("w", end.w)?;
    end_table.set("h", end.h)?;
    component.set("__letter_caret_end", end_table)
}

fn refresh_textbox_layout_cache(
    lua: &Lua,
    root: &Path,
    entity: &Table,
    component: &Table,
) -> mlua::Result<TextRenderRequest> {
    let request = build_textbox_render_request(
        root,
        entity,
        component,
        component_text_antialiasing(lua, component),
    )?;
    let cache_id = crate::renderer::text_render_request_cache_id(&request).to_string();
    let has_cached_bounds = component.get::<Table>("__letter_bounds").is_ok()
        && component.get::<Table>("__letter_caret_start").is_ok()
        && component.get::<Table>("__letter_caret_end").is_ok();
    if has_cached_bounds
        && component
            .get::<String>("__layout_cache_id")
            .ok()
            .is_some_and(|previous| previous == cache_id)
    {
        return Ok(request);
    }

    let metrics = crate::renderer::measure_text(&request).unwrap_or_default();
    let letter_bounds = metrics.letter_bounds.clone();
    component.set("dx", metrics.width)?;
    component.set("dy", metrics.height)?;
    component.set("used_scale", metrics.used_scale)?;
    component.set("line_count", metrics.line_count)?;
    let empty_height = metrics.height.max(metrics.used_scale);
    write_textbox_letter_bounds(lua, component, &request, &letter_bounds, empty_height)?;
    component.set("__layout_cache_id", cache_id)?;
    Ok(request)
}

fn letter_bounds_table_key(index: Value) -> Option<i64> {
    match index {
        Value::Integer(index) => {
            if index < -1 {
                return None;
            }
            (index as i64).checked_add(1)
        }
        Value::Number(index) => {
            if !index.is_finite() || index < -1.0 || index.fract() != 0.0 {
                return None;
            }
            let number = index;
            let index = number as i64;
            if index < -1 || index as f64 != number {
                return None;
            }
            index.checked_add(1)
        }
        _ => None,
    }
}

fn missing_letter_bounds() -> (Value, Value, Value, Value) {
    (Value::Nil, Value::Nil, Value::Nil, Value::Nil)
}

fn missing_letter_position() -> (Value, Value) {
    (Value::Nil, Value::Nil)
}

fn missing_letter_index() -> Value {
    Value::Nil
}

fn letter_bounds_call_target(
    bound_component: Option<&Table>,
    args: mlua::Variadic<Value>,
) -> Option<(Table, Value)> {
    match args.get(0) {
        Some(Value::Table(component)) => {
            let index = args.get(1).cloned().unwrap_or(Value::Nil);
            Some((component.clone(), index))
        }
        Some(index) => bound_component.map(|component| (component.clone(), index.clone())),
        None => None,
    }
}

fn parse_finite_lua_number(value: Option<&Value>) -> Option<f64> {
    let value = match value? {
        Value::Integer(value) => *value as f64,
        Value::Number(value) => *value,
        _ => return None,
    };
    value.is_finite().then_some(value)
}

fn closest_letter_call_target(
    bound_component: Option<&Table>,
    args: mlua::Variadic<Value>,
) -> Option<(Table, f64, f64)> {
    match args.get(0) {
        Some(Value::Table(component)) => {
            let x = parse_finite_lua_number(args.get(1))?;
            let y = parse_finite_lua_number(args.get(2))?;
            Some((component.clone(), x, y))
        }
        Some(_) => {
            let component = bound_component?.clone();
            let x = parse_finite_lua_number(args.get(0))?;
            let y = parse_finite_lua_number(args.get(1))?;
            Some((component, x, y))
        }
        None => None,
    }
}

fn cached_letter_bounds_entry(component: &Table, key: i64) -> Option<Table> {
    let bounds = component.get::<Table>("__letter_bounds").ok()?;
    bounds.get::<Table>(key).ok()
}

fn caret_letter_bounds_entry(component: &Table, key: i64) -> Option<Table> {
    let bounds = component.get::<Table>("__letter_bounds").ok()?;
    let len = bounds.raw_len() as i64;
    if key <= 0 {
        component.get::<Table>("__letter_caret_start").ok()
    } else if key == len + 1 {
        component.get::<Table>("__letter_caret_end").ok()
    } else {
        None
    }
}

fn refresh_letter_bounds_if_available(
    lua: &Lua,
    root: Option<&Path>,
    component: &Table,
) -> mlua::Result<()> {
    let Some(root) = root else {
        return Ok(());
    };
    let Some(entity) = component.get::<Option<Table>>("entity")? else {
        return Ok(());
    };
    refresh_textbox_layout_cache(lua, root, &entity, component)?;
    Ok(())
}

fn get_letter_bounds_values(
    lua: &Lua,
    root: Option<&Path>,
    component: Table,
    index: Value,
) -> mlua::Result<(Value, Value, Value, Value)> {
    let Some(key) = letter_bounds_table_key(index) else {
        return Ok(missing_letter_bounds());
    };
    refresh_letter_bounds_if_available(lua, root, &component)?;
    let entry = cached_letter_bounds_entry(&component, key);
    let entry = entry.or_else(|| caret_letter_bounds_entry(&component, key));
    let Some(entry) = entry else {
        return Ok(missing_letter_bounds());
    };
    Ok((
        Value::Number(entry.get::<f64>("x")?),
        Value::Number(entry.get::<f64>("y")?),
        Value::Number(entry.get::<f64>("w")?),
        Value::Number(entry.get::<f64>("h")?),
    ))
}

fn get_letter_position_values(
    lua: &Lua,
    root: Option<&Path>,
    component: Table,
    index: Value,
) -> mlua::Result<(Value, Value)> {
    let Some(key) = letter_bounds_table_key(index) else {
        return Ok(missing_letter_position());
    };
    refresh_letter_bounds_if_available(lua, root, &component)?;
    let entry = cached_letter_bounds_entry(&component, key);
    let entry = entry.or_else(|| caret_letter_bounds_entry(&component, key));
    let Some(entry) = entry else {
        return Ok(missing_letter_position());
    };
    Ok((
        Value::Number(entry.get::<f64>("x")?),
        Value::Number(entry.get::<f64>("y")?),
    ))
}

fn rect_from_table(table: &Table) -> mlua::Result<Rect> {
    Ok(Rect {
        x: table.get::<f32>("x")?,
        y: table.get::<f32>("y")?,
        w: table.get::<f32>("w")?,
        h: table.get::<f32>("h")?,
    })
}

fn caret_distance_sq(x: f64, y: f64, caret_x: f64, top: f64, bottom: f64) -> f64 {
    let dx = x - caret_x;
    let dy = if y < top {
        top - y
    } else if y > bottom {
        y - bottom
    } else {
        0.0
    };
    dx * dx + dy * dy
}

fn consider_caret_candidate(
    best: &mut Option<(usize, f64)>,
    index: usize,
    x: f64,
    y: f64,
    caret_x: f64,
    top: f64,
    bottom: f64,
) {
    let distance = caret_distance_sq(x, y, caret_x, top, bottom);
    if best.is_none_or(|(_, best_distance)| distance < best_distance) {
        *best = Some((index, distance));
    }
}

fn get_closest_letter_index_value(
    lua: &Lua,
    root: Option<&Path>,
    component: Table,
    x: f64,
    y: f64,
) -> mlua::Result<Value> {
    refresh_letter_bounds_if_available(lua, root, &component)?;
    let Some(bounds) = component.get::<Table>("__letter_bounds").ok() else {
        return Ok(Value::Integer(0));
    };

    let len = bounds.raw_len();
    if len == 0 {
        return Ok(Value::Integer(0));
    }

    let mut best = None;
    for index in 1..=len {
        let Ok(entry) = bounds.get::<Table>(index) else {
            continue;
        };
        let rect = rect_from_table(&entry)?;
        let top = rect.y as f64;
        let bottom = (rect.y + rect.h) as f64;
        consider_caret_candidate(&mut best, index - 1, x, y, rect.x as f64, top, bottom);
        consider_caret_candidate(
            &mut best,
            index,
            x,
            y,
            (rect.x + rect.w) as f64,
            top,
            bottom,
        );
    }

    Ok(Value::Integer(
        best.map(|(index, _)| index as mlua::Integer).unwrap_or(0),
    ))
}

fn get_closest_letter_index_from_args(
    lua: &Lua,
    root: Option<&Path>,
    bound_component: Option<&Table>,
    args: mlua::Variadic<Value>,
) -> mlua::Result<Value> {
    let Some((component, x, y)) = closest_letter_call_target(bound_component, args) else {
        return Ok(missing_letter_index());
    };
    get_closest_letter_index_value(lua, root, component, x, y)
}

fn get_letter_bounds_from_args(
    lua: &Lua,
    root: Option<&Path>,
    bound_component: Option<&Table>,
    args: mlua::Variadic<Value>,
) -> mlua::Result<(Value, Value, Value, Value)> {
    let Some((component, index)) = letter_bounds_call_target(bound_component, args) else {
        return Ok(missing_letter_bounds());
    };
    get_letter_bounds_values(lua, root, component, index)
}

fn get_letter_position_from_args(
    lua: &Lua,
    root: Option<&Path>,
    bound_component: Option<&Table>,
    args: mlua::Variadic<Value>,
) -> mlua::Result<(Value, Value)> {
    let Some((component, index)) = letter_bounds_call_target(bound_component, args) else {
        return Ok(missing_letter_position());
    };
    get_letter_position_values(lua, root, component, index)
}

fn install_unbound_textbox_letter_lookup_methods(
    lua: &Lua,
    table: &Table,
    root: PathBuf,
) -> mlua::Result<()> {
    let bounds_root = root.clone();
    table.set(
        "getLetterBounds",
        lua.create_function(move |ctx, args: mlua::Variadic<Value>| {
            get_letter_bounds_from_args(ctx, Some(&bounds_root), None, args)
        })?,
    )?;
    let closest_root = root.clone();
    let position_root = root;
    table.set(
        "getLetterPosition",
        lua.create_function(move |ctx, args: mlua::Variadic<Value>| {
            get_letter_position_from_args(ctx, Some(&position_root), None, args)
        })?,
    )?;
    let closest = lua.create_function(move |ctx, args: mlua::Variadic<Value>| {
        get_closest_letter_index_from_args(ctx, Some(&closest_root), None, args)
    })?;
    table.set("getClosestLetterIndex", closest.clone())?;
    table.set("getClosestCharacterIndex", closest)?;
    Ok(())
}

fn bind_textbox_letter_lookup_methods(lua: &Lua, component: &Table) -> mlua::Result<()> {
    let bind: Function = lua
        .load(
            r#"
            return function(component, raw)
                return function(first, ...)
                    if type(first) == "table" then
                        return raw(first, ...)
                    end
                    return raw(component, first, ...)
                end
            end
            "#,
        )
        .eval()?;

    let raw_bounds: Function = component.get("getLetterBounds")?;
    let bound_bounds: Function = bind.call((component.clone(), raw_bounds))?;
    component.set("getLetterBounds", bound_bounds)?;

    let raw_position: Function = component.get("getLetterPosition")?;
    let bound_position: Function = bind.call((component.clone(), raw_position))?;
    component.set("getLetterPosition", bound_position)?;

    let raw_closest: Function = component.get("getClosestLetterIndex")?;
    let bound_closest: Function = bind.call((component.clone(), raw_closest))?;
    component.set("getClosestLetterIndex", bound_closest.clone())?;
    component.set("getClosestCharacterIndex", bound_closest)?;

    Ok(())
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

fn call_component_number_callback(
    component: &Table,
    entity: &Table,
    name: &str,
    value: f32,
) -> mlua::Result<()> {
    if let Ok(callback) = component.get::<Function>(name) {
        protect_lua_call(&format!("running component callback '{name}'"), || {
            callback.call::<()>((entity.clone(), component.clone(), value))
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
            component.set("shader", Value::Nil)?;
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

    // Persistent scene environment controls. The Environment3D component below
    // writes through the same state, while scripts that do not use an ECS scene
    // can configure the sky directly through `environment3d` / `skybox`.
    {
        let environment = lua.create_table()?;
        let solid_state = render_state.clone();
        environment.set(
            "setSolid",
            lua.create_function(move |_lua, color: Table| {
                let mut value = solid_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                    .environment_3d();
                value.enabled = true;
                value.mode = crate::environment3d::EnvironmentMode3D::Solid;
                value.solid = color4_to_color(color)?;
                solid_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                    .set_environment_3d(None, value);
                Ok(())
            })?,
        )?;
        let gradient_state = render_state.clone();
        environment.set(
            "setGradient",
            lua.create_function(move |_lua, (top, bottom): (Table, Table)| {
                let mut value = gradient_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                    .environment_3d();
                value.enabled = true;
                value.mode = crate::environment3d::EnvironmentMode3D::Gradient;
                value.top = color4_to_color(top)?;
                value.bottom = color4_to_color(bottom)?;
                gradient_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                    .set_environment_3d(None, value);
                Ok(())
            })?,
        )?;
        let panorama_state = render_state.clone();
        environment.set(
            "setEquirectangular",
            lua.create_function(move |_lua, (image, rotation): (AnyUserData, Option<f32>)| {
                let image = image.borrow::<ImageHandle>()?.clone();
                let mut value = panorama_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                    .environment_3d();
                value.enabled = true;
                value.mode = crate::environment3d::EnvironmentMode3D::Equirectangular;
                value.equirectangular = Some(image);
                value.rotation_degrees = rotation.unwrap_or(0.0);
                panorama_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                    .set_environment_3d(None, value);
                Ok(())
            })?,
        )?;
        let enabled_state = render_state.clone();
        environment.set(
            "setEnabled",
            lua.create_function(move |_lua, enabled: bool| {
                let mut value = enabled_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                    .environment_3d();
                value.enabled = enabled;
                enabled_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                    .set_environment_3d(None, value);
                Ok(())
            })?,
        )?;
        let intensity_state = render_state.clone();
        environment.set(
            "setIntensity",
            lua.create_function(move |_lua, intensity: f32| {
                let mut value = intensity_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                    .environment_3d();
                value.intensity = if intensity.is_finite() {
                    intensity.max(0.0)
                } else {
                    1.0
                };
                intensity_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                    .set_environment_3d(None, value);
                Ok(())
            })?,
        )?;
        let clear_state = render_state.clone();
        environment.set(
            "clear",
            lua.create_function(move |_lua, ()| {
                clear_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                    .clear_environment_3d(None);
                Ok(())
            })?,
        )?;
        lua.globals().set("environment3d", environment.clone())?;
        lua.globals().set("environment3D", environment.clone())?;
        lua.globals().set("skybox", environment)?;
    }

    // EntityScaler
    // percentage-plus-pixel transform helper for responsive parent-relative layout
    {
        let entity_scaler = lua.create_table()?;
        entity_scaler.set(
            "awake",
            lua.create_function(move |_ctx, (_entity, component): (Table, Table)| {
                component.set("__neolove_component", "EntityScaler")?;
                component.set("enabled", true)?;
                component.set("edit_with_percent", true)?;
                component.set("x_percent", 0.0)?;
                component.set("y_percent", 0.0)?;
                component.set("size_x_percent", 0.0)?;
                component.set("size_y_percent", 0.0)?;
                component.set("offset_x", 0.0)?;
                component.set("offset_y", 0.0)?;
                component.set("pivot_x", 0.0)?;
                component.set("pivot_y", 0.0)?;
                Ok(())
            })?,
        )?;
        entity_scaler.set(
            "update",
            lua.create_function(move |_ctx, (entity, component, _dt): (Table, Table, f32)| {
                if !component.get::<bool>("enabled").unwrap_or(true) {
                    return Ok(());
                }

                let x_percent = get_number_field(&component, "x_percent", "xPercent")
                    .or_else(|| get_number_field(&component, "percent_x", "percentX"))
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0);
                let y_percent = get_number_field(&component, "y_percent", "yPercent")
                    .or_else(|| get_number_field(&component, "percent_y", "percentY"))
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0);
                let size_x_percent = get_number_field(&component, "size_x_percent", "sizeXPercent")
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0);
                let size_y_percent = get_number_field(&component, "size_y_percent", "sizeYPercent")
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0);
                let offset_x = get_number_field(&component, "offset_x", "offsetX").unwrap_or(0.0);
                let offset_y = get_number_field(&component, "offset_y", "offsetY").unwrap_or(0.0);
                let pivot_x = get_number_field(&component, "pivot_x", "pivotX")
                    .or_else(|| get_number_field(&component, "anchor_x", "anchorX"))
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0);
                let pivot_y = get_number_field(&component, "pivot_y", "pivotY")
                    .or_else(|| get_number_field(&component, "anchor_y", "anchorY"))
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0);

                entity.set("anchor_x", x_percent)?;
                entity.set("anchor_y", y_percent)?;
                entity.set("x", offset_x)?;
                entity.set("y", offset_y)?;
                entity.set("pivot_x", pivot_x)?;
                entity.set("pivot_y", pivot_y)?;
                if let Some(parent) = entity.get::<Option<Table>>("parent")? {
                    if size_x_percent > 0.0 {
                        entity.set("size_x", parent.get::<f32>("size_x")? * size_x_percent)?;
                    }
                    if size_y_percent > 0.0 {
                        entity.set("size_y", parent.get::<f32>("size_y")? * size_y_percent)?;
                    }
                }
                Ok(())
            })?,
        )?;
        core_components.set("EntityScaler", entity_scaler)?;
    }

    // Camera
    // Centers the scene viewport on the owning entity. Camera components run
    // in a dedicated pre-pass after physics and before drawable components so
    // every command and light in a frame observes one atomic camera transform.
    {
        let camera = lua.create_table()?;
        camera.set("__neolove_component", "Camera")?;
        camera.set("NEOLOVE_RENDERING", true)?;
        camera.set("NEOLOVE_CAMERA", true)?;
        camera.set(
            "awake",
            lua.create_function(|_lua, (_entity, component): (Table, Table)| {
                component.set("__neolove_component", "Camera")?;
                component.set("enabled", true)
            })?,
        )?;

        let active_platform = platform.clone();
        let active_render_state = render_state.clone();
        let set_active = lua.create_function(move |_lua, component: Table| {
            if !component.get::<bool>("enabled").unwrap_or(true) {
                return Ok(false);
            }
            let Some(entity) = component.get::<Option<Table>>("entity")? else {
                return Ok(false);
            };
            let offset = camera_offset_for_entity(&entity, &active_platform)?;
            active_render_state
                .lock()
                .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                .activate_camera(component.to_pointer() as usize, offset);
            Ok(true)
        })?;
        camera.set("SetActive", set_active.clone())?;
        camera.set("setActive", set_active)?;

        let query_render_state = render_state.clone();
        let is_active = lua.create_function(move |_lua, component: Table| {
            Ok(query_render_state
                .lock()
                .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                .is_camera_active(component.to_pointer() as usize))
        })?;
        camera.set("IsActive", is_active.clone())?;
        camera.set("isActive", is_active)?;

        let update_platform = platform.clone();
        let update_render_state = render_state.clone();
        camera.set(
            "update",
            lua.create_function(move |_lua, (entity, component, _dt): (Table, Table, f32)| {
                if !component.get::<bool>("enabled").unwrap_or(true) {
                    return Ok(());
                }
                let offset = camera_offset_for_entity(&entity, &update_platform)?;
                update_render_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                    .submit_camera(component.to_pointer() as usize, offset);
                Ok(())
            })?,
        )?;

        let destroy_render_state = render_state.clone();
        camera.set(
            "destroy",
            lua.create_function(move |_lua, (_entity, component): (Table, Table)| {
                destroy_render_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                    .clear_camera(component.to_pointer() as usize);
                Ok(())
            })?,
        )?;

        core_components.set("Camera", camera)?;
    }

    // Camera3D
    // Resolves an Euler-authored view/projection in the shared camera pre-pass.
    {
        let camera = lua.create_table()?;
        camera.set("__neolove_component", "Camera3D")?;
        camera.set("NEOLOVE_RENDERING", true)?;
        camera.set("NEOLOVE_CAMERA", true)?;
        camera.set(
            "awake",
            lua.create_function(|_lua, (_entity, component): (Table, Table)| {
                component.set("__neolove_component", "Camera3D")?;
                component.set("enabled", true)?;
                component.set("projection", "perspective")?;
                component.set("fov", 60.0)?;
                component.set("orthographic_size", 10.0)?;
                component.set("near_clip", 0.1)?;
                component.set("far_clip", 1000.0)?;
                Ok(())
            })?,
        )?;

        let active_render_state = render_state.clone();
        let set_active = lua.create_function(move |_lua, component: Table| {
            if !component.get::<bool>("enabled").unwrap_or(true) {
                return Ok(false);
            }
            let Some(entity) = component.get::<Option<Table>>("entity")? else {
                return Ok(false);
            };
            let camera = camera_3d_for_entity(&entity, &component)?;
            active_render_state
                .lock()
                .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                .activate_camera_3d(component.to_pointer() as usize, camera);
            Ok(true)
        })?;
        camera.set("SetActive", set_active.clone())?;
        camera.set("setActive", set_active)?;

        let query_render_state = render_state.clone();
        let is_active = lua.create_function(move |_lua, component: Table| {
            Ok(query_render_state
                .lock()
                .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                .is_camera_active(component.to_pointer() as usize))
        })?;
        camera.set("IsActive", is_active.clone())?;
        camera.set("isActive", is_active)?;

        let update_render_state = render_state.clone();
        camera.set(
            "update",
            lua.create_function(move |_lua, (entity, component, _dt): (Table, Table, f32)| {
                if !component.get::<bool>("enabled").unwrap_or(true) {
                    return Ok(());
                }
                let camera = camera_3d_for_entity(&entity, &component)?;
                update_render_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                    .submit_camera_3d(component.to_pointer() as usize, camera);
                Ok(())
            })?,
        )?;

        let destroy_render_state = render_state.clone();
        camera.set(
            "destroy",
            lua.create_function(move |_lua, (_entity, component): (Table, Table)| {
                destroy_render_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                    .clear_camera(component.to_pointer() as usize);
                Ok(())
            })?,
        )?;
        core_components.set("Camera3D", camera)?;
    }

    // Environment3D / Skybox3D
    // Fills the framebuffer before depth-tested geometry. Texture mode accepts
    // an equirectangular ImageHandle and follows camera rotation without camera
    // translation.
    {
        let environment = lua.create_table()?;
        environment.set("__neolove_component", "Environment3D")?;
        environment.set("NEOLOVE_RENDERING", true)?;
        environment.set(
            "awake",
            lua.create_function(|lua, (_entity, component): (Table, Table)| {
                component.set("__neolove_component", "Environment3D")?;
                component.set("enabled", true)?;
                component.set("mode", "gradient")?;
                component.set("color", color4(lua, 20, 24, 32, 255)?)?;
                component.set("top_color", color4(lua, 30, 47, 78, 255)?)?;
                component.set("bottom_color", color4(lua, 8, 10, 16, 255)?)?;
                component.set("texture", Value::Nil)?;
                component.set("texture_path", "")?;
                component.set("rotation", 0.0)?;
                component.set("intensity", 1.0)?;
                Ok(())
            })?,
        )?;
        let environment_state = render_state.clone();
        environment.set(
            "update",
            lua.create_function(move |lua, (_entity, component, _dt): (Table, Table, f32)| {
                let value = environment_3d_from_component(lua, &component)?;
                environment_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                    .set_environment_3d(Some(component.to_pointer() as usize), value);
                Ok(())
            })?,
        )?;
        let destroy_state = render_state.clone();
        environment.set(
            "destroy",
            lua.create_function(move |_lua, (_entity, component): (Table, Table)| {
                destroy_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                    .clear_environment_3d(Some(component.to_pointer() as usize));
                Ok(())
            })?,
        )?;
        core_components.set("Environment3D", environment.clone())?;
        core_components.set("Skybox3D", environment)?;
    }

    // MeshRenderer3D
    // Mesh handles are revisioned: scripts can edit vertices/material metadata
    // live and every backend observes the new immutable snapshot next frame.
    {
        let mesh_renderer = lua.create_table()?;
        mesh_renderer.set("__neolove_component", "MeshRenderer3D")?;
        mesh_renderer.set("NEOLOVE_RENDERING", true)?;
        mesh_renderer.set(
            "awake",
            lua.create_function(|lua, (_entity, component): (Table, Table)| {
                component.set("__neolove_component", "MeshRenderer3D")?;
                component.set("mesh", Value::Nil)?;
                component.set("mesh_path", "")?;
                // Keep script-created components backward compatible: assigning
                // `mesh` must work without also opting out of an implicit cube.
                // The visual editor schema selects "cube" for newly authored
                // MeshRenderer3D components.
                component.set("primitive", "none")?;
                component.set("primitive_size_x", 1.0)?;
                component.set("primitive_size_y", 1.0)?;
                component.set("primitive_size_z", 1.0)?;
                component.set("primitive_radius", 0.5)?;
                component.set("primitive_height", 1.0)?;
                component.set("primitive_segments", 24)?;
                component.set("primitive_rings", 12)?;
                component.set("texture", Value::Nil)?;
                component.set("normal_texture", Value::Nil)?;
                component.set("color", color4(lua, 255, 255, 255, 255)?)?;
                component.set("metallic", 0.0)?;
                component.set("roughness", 1.0)?;
                component.set("visible", true)?;
                component.set("casts_shadows", true)?;
                component.set("receives_shadows", true)?;
                component.set("double_sided", false)?;
                component.set("shader", Value::Nil)?;
                component.set("animation", "")?;
                component.set("animation_autoplay", true)?;
                component.set("animation_looping", true)?;
                component.set("animation_playing", true)?;
                component.set("animation_speed", 1.0)?;
                component.raw_set("__mesh_animation_initialized", false)?;
                component.raw_set("__mesh_animation_play_requested", false)?;
                component.raw_set("__mesh_animation_time", 0.0)?;
                Ok(())
            })?,
        )?;

        let play_animation =
            lua.create_function(|_lua, (component, clip): (Table, Option<String>)| {
                if let Some(clip) = clip {
                    component.set("animation", clip)?;
                }
                component.set("animation_playing", true)?;
                component.raw_set("__active_mesh_animation", Value::Nil)?;
                component.raw_set("__mesh_animation_paused", false)?;
                component.raw_set("__mesh_animation_play_requested", true)?;
                component.raw_set("__mesh_animation_time", 0.0)?;
                Ok(())
            })?;
        mesh_renderer.set("playAnimation", play_animation.clone())?;
        mesh_renderer.set("PlayAnimation", play_animation)?;
        let pause_animation = lua.create_function(|_lua, component: Table| {
            component.set("animation_playing", false)?;
            component.raw_set("__mesh_animation_play_requested", false)?;
            component.raw_set("__mesh_animation_initialized", true)?;
            let animation = component.get::<String>("animation").unwrap_or_default();
            if !animation.trim().is_empty() {
                component.raw_set("__mesh_animation_clip", animation.trim())?;
            }
            if component
                .raw_get::<Option<String>>("__active_mesh_animation")
                .unwrap_or(None)
                .is_some()
                && let Value::UserData(userdata) = component.get::<Value>("mesh")?
            {
                userdata
                    .borrow::<crate::mesh::MeshHandle>()?
                    .set_animation_paused(true)
                    .map_err(mlua::Error::external)?;
                component.raw_set("__mesh_animation_paused", true)?;
            }
            Ok(())
        })?;
        mesh_renderer.set("pauseAnimation", pause_animation.clone())?;
        mesh_renderer.set("PauseAnimation", pause_animation)?;
        let stop_animation = lua.create_function(|_lua, component: Table| {
            component.set("animation_playing", false)?;
            component.raw_set("__active_mesh_animation", Value::Nil)?;
            component.raw_set("__mesh_animation_paused", false)?;
            component.raw_set("__mesh_animation_play_requested", false)?;
            component.raw_set("__mesh_animation_initialized", true)?;
            component.raw_set("__mesh_animation_time", 0.0)?;
            let animation = component.get::<String>("animation").unwrap_or_default();
            if !animation.trim().is_empty() {
                component.raw_set("__mesh_animation_clip", animation.trim())?;
            }
            if let Value::UserData(userdata) = component.get::<Value>("mesh")? {
                userdata
                    .borrow::<crate::mesh::MeshHandle>()?
                    .stop_animation()
                    .map_err(mlua::Error::external)?;
            }
            Ok(())
        })?;
        mesh_renderer.set("stopAnimation", stop_animation.clone())?;
        mesh_renderer.set("StopAnimation", stop_animation)?;

        let mesh_render_state = render_state.clone();
        let mesh_platform = platform.clone();
        mesh_renderer.set(
            "update",
            lua.create_function(move |lua, (entity, component, dt): (Table, Table, f32)| {
                if !component.get::<bool>("visible").unwrap_or(true) {
                    return Ok(());
                }

                let path = component
                    .get::<Option<String>>("mesh_path")?
                    .unwrap_or_default();
                let primitive = component
                    .get::<Option<String>>("primitive")?
                    .unwrap_or_else(|| "cube".to_string());
                let primitive_enabled = path.trim().is_empty()
                    && !matches!(
                        primitive.trim().to_ascii_lowercase().as_str(),
                        "" | "none" | "off"
                    );
                // Preserve validation errors from active primitive values instead
                // of silently replacing an invalid value with a default. Irrelevant
                // primitive fields do not interfere with path/manual mesh modes.
                let primitive_options = if primitive_enabled {
                    Some(crate::mesh::PrimitiveOptions {
                        size: [
                            component
                                .get::<Option<f32>>("primitive_size_x")?
                                .unwrap_or(1.0),
                            component
                                .get::<Option<f32>>("primitive_size_y")?
                                .unwrap_or(1.0),
                            component
                                .get::<Option<f32>>("primitive_size_z")?
                                .unwrap_or(1.0),
                        ],
                        radius: component
                            .get::<Option<f32>>("primitive_radius")?
                            .unwrap_or(0.5),
                        height: component
                            .get::<Option<f32>>("primitive_height")?
                            .unwrap_or(1.0),
                        segments: component
                            .get::<Option<u32>>("primitive_segments")?
                            .unwrap_or(24),
                        rings: component
                            .get::<Option<u32>>("primitive_rings")?
                            .unwrap_or(12),
                    })
                } else {
                    None
                };
                let managed_source = if !path.trim().is_empty() {
                    Some(format!("path:{}", path.trim()))
                } else {
                    primitive_options.as_ref().map(|options| {
                        format!(
                        "primitive:{}:{:08x}:{:08x}:{:08x}:{:08x}:{:08x}:{}:{}",
                        primitive.trim().to_ascii_lowercase(),
                        options.size[0].to_bits(),
                        options.size[1].to_bits(),
                        options.size[2].to_bits(),
                        options.radius.to_bits(),
                        options.height.to_bits(),
                        options.segments,
                        options.rings,
                    )
                    })
                };
                let current_source = component
                    .raw_get::<Option<String>>("__mesh_source")
                    .unwrap_or(None);
                let mesh_before = component.get::<Value>("mesh")?;
                let mesh_identity_before = match &mesh_before {
                    Value::UserData(userdata) => {
                        Some(userdata.borrow::<crate::mesh::MeshHandle>()?.identity() as i64)
                    }
                    _ => None,
                };
                let recorded_identity = component
                    .raw_get::<Option<i64>>("__mesh_identity")
                    .unwrap_or(None);

                // A configured path/primitive owns the mesh slot. Scripts that
                // assign a handle directly opt out with primitive = "none".
                // When both are changed in one frame, preserve the newly assigned
                // manual handle rather than clearing it with the old source.
                if managed_source.is_none() && current_source.is_some() {
                    let replaced_with_manual = mesh_identity_before.is_some()
                        && recorded_identity.is_some()
                        && mesh_identity_before != recorded_identity;
                    component.raw_set("__mesh_source", Value::Nil)?;
                    if !replaced_with_manual {
                        component.set("mesh", Value::Nil)?;
                        component.raw_set("__mesh_identity", Value::Nil)?;
                        component.raw_set("__mesh_animation_detached", false)?;
                        component.raw_set("__active_mesh_animation", Value::Nil)?;
                        component.raw_set("__mesh_animation_paused", false)?;
                        return Ok(());
                    }
                }

                let source_changed = current_source.as_ref() != managed_source.as_ref();
                let managed_identity_changed = current_source.is_some()
                    && !source_changed
                    && recorded_identity.is_some()
                    && mesh_identity_before != recorded_identity;
                let needs_managed_mesh = managed_source.is_some()
                    && (mesh_identity_before.is_none()
                        || source_changed
                        || managed_identity_changed);
                if needs_managed_mesh {
                    let source = managed_source.as_ref().ok_or_else(|| {
                        mlua::Error::external("managed mesh source disappeared during update")
                    })?;
                    let userdata: AnyUserData = if source.starts_with("path:") {
                        let assets: Table = lua.globals().get("assets")?;
                        assets.get::<Function>("loadMesh")?.call(path.clone())?
                    } else {
                        let options = primitive_options.ok_or_else(|| {
                            mlua::Error::external("primitive mesh options disappeared during update")
                        })?;
                        let handle = crate::mesh::primitive_mesh(&primitive, options)
                            .map_err(mlua::Error::external)?;
                        lua.create_userdata(handle)?
                    };
                    component.set("mesh", userdata)?;
                    component.raw_set("__mesh_source", source.clone())?;
                    component.raw_set("__mesh_animation_detached", false)?;
                    component.raw_set("__active_mesh_animation", Value::Nil)?;
                    component.raw_set("__mesh_animation_paused", false)?;
                    component.raw_set("__mesh_animation_time", 0.0)?;
                } else if managed_source.is_none() {
                    component.raw_set("__mesh_source", Value::Nil)?;
                }

                let mut mesh_userdata = match component.get::<Value>("mesh")? {
                    Value::UserData(userdata) => userdata,
                    _ => return Ok(()),
                };
                let mut mesh = mesh_userdata.borrow::<crate::mesh::MeshHandle>()?.clone();
                let identity = mesh.identity() as i64;
                if component
                    .raw_get::<Option<i64>>("__mesh_identity")
                    .unwrap_or(None)
                    != Some(identity)
                {
                    component.raw_set("__mesh_identity", identity)?;
                    component.raw_set("__mesh_animation_detached", false)?;
                    component.raw_set("__active_mesh_animation", Value::Nil)?;
                    component.raw_set("__mesh_animation_paused", false)?;
                    component.raw_set("__mesh_animation_time", 0.0)?;
                }

                let animation = component
                    .get::<Option<String>>("animation")?
                    .unwrap_or_default();
                let animation = animation.trim();
                let configured_clip = component
                    .raw_get::<Option<String>>("__mesh_animation_clip")
                    .unwrap_or(None);
                if animation.is_empty() {
                    component.set("animation_playing", false)?;
                    let active = component
                        .raw_get::<Option<String>>("__active_mesh_animation")
                        .unwrap_or(None)
                        .is_some();
                    if active {
                        mesh.stop_animation().map_err(mlua::Error::external)?;
                    }
                    component.raw_set("__mesh_animation_clip", Value::Nil)?;
                    component.raw_set("__mesh_animation_initialized", false)?;
                    component.raw_set("__mesh_animation_play_requested", false)?;
                    component.raw_set("__active_mesh_animation", Value::Nil)?;
                    component.raw_set("__mesh_animation_paused", false)?;
                    component.raw_set("__mesh_animation_time", 0.0)?;
                } else {
                    if configured_clip.as_deref() != Some(animation) {
                        if component
                            .raw_get::<Option<String>>("__active_mesh_animation")
                            .unwrap_or(None)
                            .is_some()
                        {
                            mesh.stop_animation().map_err(mlua::Error::external)?;
                        }
                        component.raw_set("__mesh_animation_clip", animation)?;
                        component.raw_set("__mesh_animation_initialized", false)?;
                        component.raw_set("__active_mesh_animation", Value::Nil)?;
                        component.raw_set("__mesh_animation_paused", false)?;
                        component.raw_set("__mesh_animation_time", 0.0)?;
                    }

                    let play_requested = component
                        .raw_get::<bool>("__mesh_animation_play_requested")
                        .unwrap_or(false);
                    let initialized = component
                        .raw_get::<bool>("__mesh_animation_initialized")
                        .unwrap_or(false);
                    if play_requested {
                        component.set("animation_playing", true)?;
                        component.raw_set("__mesh_animation_play_requested", false)?;
                        component.raw_set("__mesh_animation_initialized", true)?;
                    } else if !initialized {
                        let autoplay = component
                            .get::<Option<bool>>("animation_autoplay")?
                            .unwrap_or(true);
                        component.set("animation_playing", autoplay)?;
                        component.raw_set("__mesh_animation_initialized", true)?;
                    }

                    let looped = component
                        .get::<Option<bool>>("animation_looping")?
                        .unwrap_or(true);
                    let speed = component
                        .get::<Option<f32>>("animation_speed")?
                        .unwrap_or(1.0);
                    if !speed.is_finite() || speed < 0.0 {
                        return Err(mlua::Error::external(
                            "MeshRenderer3D animation_speed must be a finite non-negative number; reverse component playback is not supported",
                        ));
                    }
                    let animation_signature =
                        format!("{}:{}:{:08x}", animation, looped, speed.to_bits());
                    let mut active_signature = component
                        .raw_get::<Option<String>>("__active_mesh_animation")
                        .unwrap_or(None);
                    let playing = component
                        .get::<Option<bool>>("animation_playing")?
                        .unwrap_or(false);
                    if playing {
                        if !component
                            .raw_get::<bool>("__mesh_animation_detached")
                            .unwrap_or(false)
                        {
                            mesh = mesh.detached_clone().map_err(mlua::Error::external)?;
                            mesh_userdata = lua.create_userdata(mesh.clone())?;
                            component.set("mesh", mesh_userdata.clone())?;
                            component.raw_set("__mesh_identity", mesh.identity() as i64)?;
                            component.raw_set("__mesh_animation_detached", true)?;
                            component.raw_set("__active_mesh_animation", Value::Nil)?;
                            component.raw_set("__mesh_animation_paused", false)?;
                            active_signature = None;
                        }
                        if active_signature.as_deref() != Some(animation_signature.as_str()) {
                            mesh.play_animation(animation, looped, speed)
                                .map_err(mlua::Error::external)?;
                            component.raw_set("__active_mesh_animation", animation_signature)?;
                            component.raw_set("__mesh_animation_paused", false)?;
                            component.raw_set("__mesh_animation_time", 0.0)?;
                        }
                        if component
                            .raw_get::<bool>("__mesh_animation_paused")
                            .unwrap_or(false)
                        {
                            mesh.set_animation_paused(false)
                                .map_err(mlua::Error::external)?;
                            component.raw_set("__mesh_animation_paused", false)?;
                        }
                        let delta = if dt.is_finite() {
                            dt.max(0.0)
                        } else {
                            return Err(mlua::Error::external(
                                "MeshRenderer3D update delta must be finite",
                            ));
                        };
                        let advanced = mesh
                            .advance_animation(delta)
                            .map_err(mlua::Error::external)?;
                        let previous_time = component
                            .raw_get::<f32>("__mesh_animation_time")
                            .unwrap_or(0.0);
                        let animation_time = previous_time + delta * speed;
                        component.raw_set("__mesh_animation_time", animation_time)?;
                        let duration = mesh
                            .animation_duration(animation)
                            .map_err(mlua::Error::external)?
                            .ok_or_else(|| {
                                mlua::Error::external(format!(
                                    "animation '{animation}' does not exist"
                                ))
                            })?;
                        let completed =
                            !looped && (duration <= 0.0 || animation_time >= duration);
                        if !advanced || completed {
                            component.set("animation_playing", false)?;
                            if advanced && completed {
                                mesh.set_animation_paused(true)
                                    .map_err(mlua::Error::external)?;
                            }
                            component.raw_set("__mesh_animation_paused", true)?;
                        }
                    } else if active_signature.is_some()
                        && !component
                            .raw_get::<bool>("__mesh_animation_paused")
                            .unwrap_or(false)
                    {
                        mesh.set_animation_paused(true)
                            .map_err(mlua::Error::external)?;
                        component.raw_set("__mesh_animation_paused", true)?;
                    }
                }
                let texture = match component.get::<Value>("texture")? {
                    Value::UserData(userdata) => Some(userdata.borrow::<ImageHandle>()?.clone()),
                    _ => None,
                };
                let color = match component.get::<Table>("color") {
                    Ok(color) => color4_to_color(color)?,
                    Err(_) => Color::WHITE,
                };
                let shader = shader_from_component(&component)?;
                let transform = crate::window::get_global_transform_3d(&entity)?;
                let (camera, window) = {
                    let state = mesh_render_state
                        .lock()
                        .map_err(|_| mlua::Error::external("render state lock poisoned"))?;
                    (
                        state.camera_3d(),
                        lock_platform_state(&mesh_platform).window(),
                    )
                };
                let aspect = window.width.max(1.0) / window.height.max(1.0);
                mesh_render_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                    .queue(DrawCommand::Mesh3D(crate::render3d::Mesh3DCommand {
                        mesh,
                        model: transform.model,
                        view_projection: camera.view_projection(aspect),
                        tint: color,
                        texture,
                        shader,
                        double_sided: component.get::<bool>("double_sided").unwrap_or(false),
                    }));
                Ok(())
            })?,
        )?;
        core_components.set("MeshRenderer3D", mesh_renderer)?;
    }

    // Light3D
    {
        let light = lua.create_table()?;
        light.set("__neolove_component", "Light3D")?;
        light.set("NEOLOVE_RENDERING", true)?;
        light.set(
            "awake",
            lua.create_function(|lua, (_entity, component): (Table, Table)| {
                component.set("__neolove_component", "Light3D")?;
                component.set("kind", "point")?;
                component.set("color", color4(lua, 255, 255, 255, 255)?)?;
                component.set("intensity", 1.0)?;
                component.set("range", 10.0)?;
                component.set("spot_angle", 45.0)?;
                component.set("spot_softness", 0.15)?;
                component.set("casts_shadows", true)?;
                component.set("shadow_bias", 0.005)?;
                component.set("visible", true)?;
                Ok(())
            })?,
        )?;
        let light_render_state = render_state.clone();
        light.set(
            "update",
            lua.create_function(move |_lua, (entity, component, _dt): (Table, Table, f32)| {
                if !component.get::<bool>("visible").unwrap_or(true) {
                    return Ok(());
                }
                let transform = crate::window::get_global_transform_3d(&entity)?;
                let kind = match component
                    .get::<String>("kind")
                    .unwrap_or_else(|_| "point".to_string())
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "directional" | "sun" => crate::render3d::LightKind3D::Directional,
                    "spot" | "spotlight" => crate::render3d::LightKind3D::Spot,
                    _ => crate::render3d::LightKind3D::Point,
                };
                let color = match component.get::<Table>("color") {
                    Ok(color) => color4_to_color(color)?,
                    Err(_) => Color::WHITE,
                };
                light_render_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                    .queue_light_3d(crate::render3d::Light3D {
                        kind,
                        position: transform.position,
                        direction: forward_from_euler(transform.euler),
                        color,
                        intensity: component.get::<f32>("intensity").unwrap_or(1.0).max(0.0),
                        range: component.get::<f32>("range").unwrap_or(10.0).max(0.0001),
                        spot_angle_radians: component
                            .get::<f32>("spot_angle")
                            .unwrap_or(45.0)
                            .clamp(0.1, 179.0)
                            .to_radians(),
                        spot_softness: component
                            .get::<f32>("spot_softness")
                            .unwrap_or(0.15)
                            .clamp(0.0, 1.0),
                        casts_shadows: component.get::<bool>("casts_shadows").unwrap_or(true),
                    });
                Ok(())
            })?,
        )?;
        core_components.set("Light3D", light)?;
    }

    // ParticleSystem3D
    // Simulation lives in a fixed-capacity native pool. Rendering expands the
    // entire emitter into a single camera-facing billboard command.
    {
        let particles = lua.create_table()?;
        particles.set("__neolove_component", "ParticleSystem3D")?;
        particles.set("NEOLOVE_RENDERING", true)?;
        particles.set(
            "awake",
            lua.create_function(|lua, (_entity, component): (Table, Table)| {
                component.set("__neolove_component", "ParticleSystem3D")?;
                component.set("enabled", true)?;
                component.set("visible", true)?;
                component.set("playing", true)?;
                component.set("looping", true)?;
                component.set("duration", 5.0)?;
                component.set("emission_rate", 24.0)?;
                component.set("max_particles", 1024)?;
                component.set("shape", "point")?;
                component.set("box_size_x", 2.0)?;
                component.set("box_size_y", 2.0)?;
                component.set("box_size_z", 2.0)?;
                component.set("sphere_radius", 1.0)?;
                component.set("cone_angle", 30.0)?;
                component.set("cone_length", 1.0)?;
                component.set("direction_x", 0.0)?;
                component.set("direction_y", 1.0)?;
                component.set("direction_z", 0.0)?;
                component.set("spread", 12.0)?;
                component.set("lifetime", 1.5)?;
                component.set("lifetime_min", 1.0)?;
                component.set("lifetime_max", 2.0)?;
                component.set("speed", 2.0)?;
                component.set("speed_min", 1.0)?;
                component.set("speed_max", 3.0)?;
                component.set("gravity_x", 0.0)?;
                component.set("gravity_y", -9.81)?;
                component.set("gravity_z", 0.0)?;
                component.set("drag", 0.0)?;
                component.set("start_size", 0.25)?;
                component.set("end_size", 0.0)?;
                component.set("start_color", color4(lua, 255, 190, 80, 255)?)?;
                component.set("end_color", color4(lua, 255, 70, 20, 0)?)?;
                component.set("start_rotation", 0.0)?;
                component.set("angular_velocity", 0.0)?;
                component.set("texture", Value::Nil)?;
                component.set("particle_count", 0)?;
                component.set("seed", 0x6d2b_79f5_i64)?;
                component.set(
                    "__particle_state",
                    crate::particles3d::ParticleEmitterHandle::new(1024, 0x6d2b_79f5),
                )?;
                Ok(())
            })?,
        )?;

        let play = lua.create_function(|_lua, component: Table| {
            component.set("playing", true)?;
            particle_emitter_3d_from_component(&component)?
                .play()
                .map_err(mlua::Error::external)
        })?;
        particles.set("play", play.clone())?;
        particles.set("Play", play)?;
        let pause = lua.create_function(|_lua, component: Table| {
            component.set("playing", false)?;
            particle_emitter_3d_from_component(&component)?
                .pause()
                .map_err(mlua::Error::external)
        })?;
        particles.set("pause", pause.clone())?;
        particles.set("Pause", pause)?;
        let stop = lua.create_function(|_lua, component: Table| {
            component.set("playing", false)?;
            component.set("particle_count", 0)?;
            particle_emitter_3d_from_component(&component)?
                .stop()
                .map_err(mlua::Error::external)
        })?;
        particles.set("stop", stop.clone())?;
        particles.set("Stop", stop)?;
        let emit = lua.create_function(|_lua, (component, count): (Table, Option<i32>)| {
            particle_emitter_3d_from_component(&component)?
                .emit(count.unwrap_or(1).max(0) as usize)
                .map_err(mlua::Error::external)
        })?;
        particles.set("emit", emit.clone())?;
        particles.set("Emit", emit)?;

        let particle_render_state = render_state.clone();
        let particle_platform = platform.clone();
        particles.set(
            "update",
            lua.create_function(move |lua, (entity, component, dt): (Table, Table, f32)| {
                if !component.get::<bool>("enabled").unwrap_or(true) {
                    return Ok(());
                }
                let emitter = particle_emitter_3d_from_component(&component)?;
                let mut config = particle_config_3d_from_component(&component)?;
                let transform = crate::window::get_global_transform_3d(&entity)?;
                let scale = crate::render3d::Vec3::new(
                    transform.scale.x.abs(),
                    transform.scale.y.abs(),
                    transform.scale.z.abs(),
                );
                config.box_extents.x *= scale.x;
                config.box_extents.y *= scale.y;
                config.box_extents.z *= scale.z;
                let average_scale = ((scale.x + scale.y + scale.z) / 3.0).max(0.0);
                config.sphere_radius *= average_scale;
                config.cone_length *= average_scale;
                config.start_size *= average_scale;
                config.end_size *= average_scale;
                emitter
                    .step(dt, transform.position, transform.euler, &config)
                    .map_err(mlua::Error::external)?;
                let count = emitter.particle_count().map_err(mlua::Error::external)?;
                component.set("particle_count", count)?;
                component.set(
                    "playing",
                    emitter.is_playing().map_err(mlua::Error::external)?,
                )?;
                if count == 0 || !component.get::<bool>("visible").unwrap_or(true) {
                    return Ok(());
                }
                let texture = get_image_field(&component, "texture")?;
                let filter = app_texture_filter(lua);
                let (camera, window) = {
                    let state = particle_render_state
                        .lock()
                        .map_err(|_| mlua::Error::external("render state lock poisoned"))?;
                    (
                        state.camera_3d(),
                        lock_platform_state(&particle_platform).window(),
                    )
                };
                let aspect = window.width.max(1.0) / window.height.max(1.0);
                particle_render_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                    .queue(DrawCommand::Particles3D(
                        crate::particles3d::ParticleSystem3DCommand {
                            emitter,
                            view_projection: camera.view_projection(aspect),
                            camera_euler: camera.euler,
                            texture,
                            filter,
                        },
                    ));
                Ok(())
            })?,
        )?;
        particles.set(
            "destroy",
            lua.create_function(|_lua, (_entity, component): (Table, Table)| {
                particle_emitter_3d_from_component(&component)?
                    .stop()
                    .map_err(mlua::Error::external)
            })?,
        )?;
        core_components.set("ParticleSystem3D", particles)?;
    }

    // Physics3D collider/query registry plus lightweight rigid-body integration.
    // Triangle meshes retain their BVH across frames and refresh atomically
    // when a script edits the source MeshHandle.
    {
        use crate::physics3d::{
            Collider3D as NativeCollider3D, ColliderShape3D, CollisionFilter3D, QueryFilter3D,
            Ray3D, Transform3D,
        };

        let registry = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::<
            usize,
            NativeCollider3D,
        >::new()));

        let rigidbody = lua.create_table()?;
        rigidbody.set("__neolove_component", "Rigidbody3D")?;
        rigidbody.set(
            "awake",
            lua.create_function(|_lua, (entity, component): (Table, Table)| {
                component.set("__neolove_component", "Rigidbody3D")?;
                component.set("enabled", true)?;
                component.set("is_static", false)?;
                component.set("mass", 1.0)?;
                component.set("gravity_scale", 1.0)?;
                component.set("linear_damping", 0.0)?;
                component.set("angular_damping", 0.0)?;
                component.set("velocity_x", 0.0)?;
                component.set("velocity_y", 0.0)?;
                component.set("velocity_z", 0.0)?;
                component.set("angular_velocity_x", 0.0)?;
                component.set("angular_velocity_y", 0.0)?;
                component.set("angular_velocity_z", 0.0)?;
                component.set("freeze_x", false)?;
                component.set("freeze_y", false)?;
                component.set("freeze_z", false)?;
                component.set("freeze_rotation_x", false)?;
                component.set("freeze_rotation_y", false)?;
                component.set("freeze_rotation_z", false)?;
                component.set("continuous_collision", false)?;
                component.set("auto_resolve", true)?;
                component.set("contact_slop", 0.001)?;
                component.set("onContact", Value::Nil)?;
                component.set("onTrigger", Value::Nil)?;
                let transform = crate::window::get_global_transform_3d(&entity)?;
                component.set("__last_world_x", transform.position.x)?;
                component.set("__last_world_y", transform.position.y)?;
                component.set("__last_world_z", transform.position.z)?;
                Ok(())
            })?,
        )?;
        rigidbody.set(
            "update",
            lua.create_function(|_lua, (entity, component, dt): (Table, Table, f32)| {
                if !component.get::<bool>("enabled").unwrap_or(true)
                    || component.get::<bool>("is_static").unwrap_or(false)
                {
                    return Ok(());
                }
                let dt = dt.clamp(0.0, 0.1);
                let linear_damping = component
                    .get::<f32>("linear_damping")
                    .unwrap_or(0.0)
                    .max(0.0);
                let angular_damping = component
                    .get::<f32>("angular_damping")
                    .unwrap_or(0.0)
                    .max(0.0);
                let linear_decay = (1.0 - linear_damping * dt).max(0.0);
                let angular_decay = (1.0 - angular_damping * dt).max(0.0);
                let mut velocity = [
                    component.get::<f32>("velocity_x").unwrap_or(0.0),
                    component.get::<f32>("velocity_y").unwrap_or(0.0),
                    component.get::<f32>("velocity_z").unwrap_or(0.0),
                ];
                velocity[1] -= 9.81 * component.get::<f32>("gravity_scale").unwrap_or(1.0) * dt;
                for axis in &mut velocity {
                    *axis *= linear_decay;
                }
                for (axis, field) in ["x", "y", "position_z"].into_iter().enumerate() {
                    let frozen = component
                        .get::<bool>(["freeze_x", "freeze_y", "freeze_z"][axis])
                        .unwrap_or(false);
                    if frozen {
                        velocity[axis] = 0.0;
                    } else {
                        let value = entity.get::<f32>(field).unwrap_or(0.0) + velocity[axis] * dt;
                        entity.set(field, value)?;
                    }
                    component.set(
                        ["velocity_x", "velocity_y", "velocity_z"][axis],
                        velocity[axis],
                    )?;
                }

                for axis in 0..3 {
                    let velocity_field = [
                        "angular_velocity_x",
                        "angular_velocity_y",
                        "angular_velocity_z",
                    ][axis];
                    let rotation_field = ["rotation_x", "rotation_y", "rotation_z"][axis];
                    let freeze_field = [
                        "freeze_rotation_x",
                        "freeze_rotation_y",
                        "freeze_rotation_z",
                    ][axis];
                    let mut angular_velocity =
                        component.get::<f32>(velocity_field).unwrap_or(0.0) * angular_decay;
                    if component.get::<bool>(freeze_field).unwrap_or(false) {
                        angular_velocity = 0.0;
                    } else {
                        let rotation = entity.get::<f32>(rotation_field).unwrap_or(0.0)
                            + angular_velocity * dt;
                        entity.set(rotation_field, rotation)?;
                    }
                    component.set(velocity_field, angular_velocity)?;
                }
                if component.get::<bool>("auto_resolve").unwrap_or(true) {
                    let physics = _lua.globals().get::<Table>("physics3d")?;
                    physics.get::<Function>("resolveBody")?.call::<()>((
                        entity,
                        component,
                        None::<Table>,
                    ))?;
                }
                Ok(())
            })?,
        )?;
        for (method_name, field_name) in
            [("setOnContact", "onContact"), ("setOnTrigger", "onTrigger")]
        {
            rigidbody.set(
                method_name,
                lua.create_function(move |_lua, (component, callback): (Table, Value)| {
                    component.set(field_name, callback)?;
                    Ok(())
                })?,
            )?;
        }
        core_components.set("Rigidbody3D", rigidbody)?;

        let collider = lua.create_table()?;
        collider.set("__neolove_component", "Collider3D")?;
        collider.set(
            "awake",
            lua.create_function(|_lua, (_entity, component): (Table, Table)| {
                component.set("__neolove_component", "Collider3D")?;
                component.set("enabled", true)?;
                component.set("is_trigger", false)?;
                component.set("shape", "box")?;
                component.set("mesh", Value::Nil)?;
                component.set("mesh_path", "")?;
                component.set("convex", false)?;
                component.set("size_x", 1.0)?;
                component.set("size_y", 1.0)?;
                component.set("size_z", 1.0)?;
                component.set("radius", 0.5)?;
                component.set("height", 1.0)?;
                component.set("offset_x", 0.0)?;
                component.set("offset_y", 0.0)?;
                component.set("offset_z", 0.0)?;
                component.set("restitution", 0.0)?;
                component.set("friction", 0.5)?;
                component.set("non_physics", false)?;
                component.set("layer", 1u32)?;
                component.set("mask", u32::MAX)?;
                Ok(())
            })?,
        )?;
        let update_registry = registry.clone();
        collider.set(
            "update",
            lua.create_function(move |lua, (entity, component, _dt): (Table, Table, f32)| {
                let key = component.to_pointer() as usize;
                if !component.get::<bool>("enabled").unwrap_or(true) {
                    update_registry
                        .lock()
                        .map_err(|_| mlua::Error::external("3D collider registry lock poisoned"))?
                        .remove(&key);
                    return Ok(());
                }
                let transform = crate::window::get_global_transform_3d(&entity)?;
                let offset = crate::render3d::Vec3::new(
                    component.get::<f32>("offset_x").unwrap_or(0.0),
                    component.get::<f32>("offset_y").unwrap_or(0.0),
                    component.get::<f32>("offset_z").unwrap_or(0.0),
                );
                let offset = transform.model.transform_direction(offset);
                let position = crate::render3d::Vec3::new(
                    transform.position.x + offset.x,
                    transform.position.y + offset.y,
                    transform.position.z + offset.z,
                );
                let shape_name = component
                    .get::<String>("shape")
                    .unwrap_or_else(|_| "box".to_string())
                    .to_ascii_lowercase();
                let entity_id = entity.get::<u64>("id").unwrap_or(key as u64);

                let mut registry = update_registry
                    .lock()
                    .map_err(|_| mlua::Error::external("3D collider registry lock poisoned"))?;
                let mesh = if shape_name == "mesh" {
                    let userdata = match component.get::<Value>("mesh")? {
                        Value::UserData(userdata) => userdata,
                        _ => {
                            let path = component.get::<String>("mesh_path").unwrap_or_default();
                            if path.trim().is_empty() {
                                registry.remove(&key);
                                return Ok(());
                            }
                            let assets: Table = lua.globals().get("assets")?;
                            let userdata: AnyUserData =
                                assets.get::<Function>("loadMesh")?.call(path)?;
                            component.set("mesh", userdata.clone())?;
                            userdata
                        }
                    };
                    Some(userdata.borrow::<crate::mesh::MeshHandle>()?.clone())
                } else {
                    None
                };

                let mesh_identity = mesh.as_ref().map(|mesh| mesh.identity() as u64);
                let reuse_mesh = mesh_identity.is_some()
                    && component.get::<Option<u64>>("__mesh_identity")? == mesh_identity
                    && matches!(
                        registry.get(&key).map(|collider| &collider.shape),
                        Some(ColliderShape3D::TriangleMesh(_))
                    );
                if reuse_mesh {
                    let collider = registry
                        .get_mut(&key)
                        .ok_or_else(|| mlua::Error::external("3D mesh collider disappeared"))?;
                    collider
                        .refresh_mesh_if_changed()
                        .map_err(mlua::Error::external)?;
                    collider.id = entity_id;
                    collider.transform = Transform3D {
                        position,
                        euler: transform.euler,
                        scale: transform.scale,
                    };
                    collider.enabled = true;
                    collider.is_trigger = component.get::<bool>("is_trigger").unwrap_or(false);
                    collider.filter = CollisionFilter3D::new(
                        component.get::<u32>("layer").unwrap_or(1),
                        component.get::<u32>("mask").unwrap_or(u32::MAX),
                    );
                    collider.restitution = component
                        .get::<f32>("restitution")
                        .unwrap_or(0.0)
                        .clamp(0.0, 1.0);
                    collider.friction = component
                        .get::<f32>("friction")
                        .unwrap_or(0.5)
                        .clamp(0.0, 1.0);
                    collider.response_enabled =
                        !component.get::<bool>("non_physics").unwrap_or(false);
                    collider.body_is_static = entity
                        .get::<Table>("components")
                        .ok()
                        .and_then(|components| {
                            components.sequence_values::<Table>().find_map(|value| {
                                let body = value.ok()?;
                                (body.get::<String>("__neolove_component").ok()?.as_str()
                                    == "Rigidbody3D")
                                    .then(|| body.get::<bool>("is_static").unwrap_or(false))
                            })
                        })
                        .unwrap_or(true);
                    return Ok(());
                }

                let shape = match shape_name.as_str() {
                    "sphere" => ColliderShape3D::Sphere {
                        radius: component.get::<f32>("radius").unwrap_or(0.5).max(0.0001),
                    },
                    "capsule" => ColliderShape3D::Capsule {
                        radius: component.get::<f32>("radius").unwrap_or(0.5).max(0.0001),
                        half_height: component.get::<f32>("height").unwrap_or(1.0).max(0.0) * 0.5,
                    },
                    "mesh" => ColliderShape3D::triangle_mesh(mesh.ok_or_else(|| {
                        mlua::Error::external("mesh collider requires a mesh or mesh_path")
                    })?)
                    .map_err(mlua::Error::external)?,
                    _ => ColliderShape3D::Box {
                        half_extents: crate::render3d::Vec3::new(
                            component.get::<f32>("size_x").unwrap_or(1.0).max(0.0001) * 0.5,
                            component.get::<f32>("size_y").unwrap_or(1.0).max(0.0001) * 0.5,
                            component.get::<f32>("size_z").unwrap_or(1.0).max(0.0001) * 0.5,
                        ),
                    },
                };
                let mut native = NativeCollider3D::new(entity_id, shape);
                native.transform = Transform3D {
                    position,
                    euler: transform.euler,
                    scale: transform.scale,
                };
                native.is_trigger = component.get::<bool>("is_trigger").unwrap_or(false);
                native.filter = CollisionFilter3D::new(
                    component.get::<u32>("layer").unwrap_or(1),
                    component.get::<u32>("mask").unwrap_or(u32::MAX),
                );
                native.restitution = component
                    .get::<f32>("restitution")
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0);
                native.friction = component
                    .get::<f32>("friction")
                    .unwrap_or(0.5)
                    .clamp(0.0, 1.0);
                native.response_enabled = !component.get::<bool>("non_physics").unwrap_or(false);
                native.body_is_static = entity
                    .get::<Table>("components")
                    .ok()
                    .and_then(|components| {
                        components.sequence_values::<Table>().find_map(|value| {
                            let body = value.ok()?;
                            (body.get::<String>("__neolove_component").ok()?.as_str()
                                == "Rigidbody3D")
                                .then(|| body.get::<bool>("is_static").unwrap_or(false))
                        })
                    })
                    .unwrap_or(true);
                component.set("__mesh_identity", mesh_identity)?;
                registry.insert(key, native);
                Ok(())
            })?,
        )?;
        let destroy_registry = registry.clone();
        collider.set(
            "destroy",
            lua.create_function(move |_lua, (_entity, component): (Table, Table)| {
                destroy_registry
                    .lock()
                    .map_err(|_| mlua::Error::external("3D collider registry lock poisoned"))?
                    .remove(&(component.to_pointer() as usize));
                Ok(())
            })?,
        )?;
        core_components.set("Collider3D", collider)?;

        let physics = lua.create_table()?;
        let raycast_registry = registry.clone();
        physics.set(
            "raycast",
            lua.create_function(
                move |lua,
                      (ox, oy, oz, dx, dy, dz, max_distance, options): (
                    f32,
                    f32,
                    f32,
                    f32,
                    f32,
                    f32,
                    Option<f32>,
                    Option<Table>,
                )| {
                    let ray = Ray3D::new(
                        crate::render3d::Vec3::new(ox, oy, oz),
                        crate::render3d::Vec3::new(dx, dy, dz),
                    )
                    .map_err(mlua::Error::external)?;
                    let query = QueryFilter3D {
                        collision: CollisionFilter3D::new(
                            options
                                .as_ref()
                                .and_then(|table| table.get::<u32>("layer").ok())
                                .unwrap_or(1),
                            options
                                .as_ref()
                                .and_then(|table| table.get::<u32>("mask").ok())
                                .unwrap_or(u32::MAX),
                        ),
                        include_triggers: options
                            .as_ref()
                            .and_then(|table| table.get::<bool>("include_triggers").ok())
                            .unwrap_or(true),
                    };
                    let mut registry = raycast_registry
                        .lock()
                        .map_err(|_| mlua::Error::external("3D collider registry lock poisoned"))?;
                    for collider in registry.values_mut() {
                        collider
                            .refresh_mesh_if_changed()
                            .map_err(mlua::Error::external)?;
                    }
                    let colliders = registry.values().cloned().collect::<Vec<_>>();
                    let Some(hit) = crate::physics3d::raycast_nearest(
                        &colliders,
                        ray,
                        max_distance.unwrap_or(1_000_000.0).max(0.0),
                        query,
                    )
                    .map_err(mlua::Error::external)?
                    else {
                        return Ok(None::<Table>);
                    };
                    let result = lua.create_table()?;
                    result.set("entity_id", hit.collider_id)?;
                    result.set("distance", hit.distance)?;
                    result.set("x", hit.position.x)?;
                    result.set("y", hit.position.y)?;
                    result.set("z", hit.position.z)?;
                    result.set("normal_x", hit.normal.x)?;
                    result.set("normal_y", hit.normal.y)?;
                    result.set("normal_z", hit.normal.z)?;
                    result.set("front_face", hit.front_face)?;
                    result.set("is_trigger", hit.is_trigger)?;
                    result.set(
                        "triangle_index",
                        hit.triangle_index.map(|index| index as u64 + 1),
                    )?;
                    if let Some(barycentric) = hit.barycentric {
                        let values = lua.create_table_from([
                            (1, barycentric[0]),
                            (2, barycentric[1]),
                            (3, barycentric[2]),
                        ])?;
                        result.set("barycentric", values)?;
                    }
                    Ok(Some(result))
                },
            )?,
        )?;

        let contacts_registry = registry.clone();
        physics.set(
            "contacts",
            lua.create_function(move |lua, options: Option<Table>| {
                let include_triggers = options
                    .as_ref()
                    .and_then(|options| options.get::<bool>("include_triggers").ok())
                    .unwrap_or(true);
                let include_bounds = options
                    .as_ref()
                    .and_then(|options| options.get::<bool>("include_bounds").ok())
                    .unwrap_or(true);
                let entity_id = options
                    .as_ref()
                    .and_then(|options| options.get::<u64>("entity_id").ok());
                let mut registry = contacts_registry
                    .lock()
                    .map_err(|_| mlua::Error::external("3D collider registry lock poisoned"))?;
                for collider in registry.values_mut() {
                    collider
                        .refresh_mesh_if_changed()
                        .map_err(mlua::Error::external)?;
                }
                let mut colliders = registry
                    .iter()
                    .map(|(key, collider)| (*key, collider.clone()))
                    .collect::<Vec<_>>();
                drop(registry);
                colliders.sort_unstable_by_key(|(key, collider)| (collider.id, *key));
                let colliders = colliders
                    .into_iter()
                    .map(|(_, collider)| collider)
                    .collect::<Vec<_>>();
                let contacts =
                    crate::physics3d::contacts(&colliders).map_err(mlua::Error::external)?;
                let result = lua.create_table()?;
                let mut output_index = 1usize;
                for contact in contacts {
                    if (!include_triggers && contact.has_trigger)
                        || (!include_bounds
                            && contact.quality == crate::physics3d::ContactQuality3D::Bounds)
                        || entity_id.is_some_and(|entity_id| {
                            contact.first_id != entity_id && contact.second_id != entity_id
                        })
                    {
                        continue;
                    }
                    let value = lua.create_table()?;
                    value.set("first_id", contact.first_id)?;
                    value.set("second_id", contact.second_id)?;
                    value.set("normal_x", contact.normal.x)?;
                    value.set("normal_y", contact.normal.y)?;
                    value.set("normal_z", contact.normal.z)?;
                    value.set("penetration", contact.penetration)?;
                    value.set("x", contact.point.x)?;
                    value.set("y", contact.point.y)?;
                    value.set("z", contact.point.z)?;
                    value.set("is_trigger", contact.has_trigger)?;
                    value.set("quality", contact.quality.as_str())?;
                    value.set(
                        "exact",
                        contact.quality == crate::physics3d::ContactQuality3D::Exact,
                    )?;
                    value.set(
                        "can_resolve",
                        contact.quality == crate::physics3d::ContactQuality3D::Exact
                            && !contact.has_trigger
                            && colliders[contact.first_index].response_enabled
                            && colliders[contact.second_index].response_enabled,
                    )?;
                    result.raw_set(output_index, value)?;
                    output_index += 1;
                }
                Ok(result)
            })?,
        )?;

        let resolve_registry = registry.clone();
        physics.set(
            "resolveBody",
            lua.create_function(
                move |lua, (entity, body, options): (Table, Table, Option<Table>)| {
                    let entity_id = entity.get::<u64>("id").map_err(|_| {
                        mlua::Error::external("3D body entity requires a numeric id")
                    })?;
                    let transform = crate::window::get_global_transform_3d(&entity)?;
                    let previous = crate::render3d::Vec3::new(
                        body.get::<f32>("__last_world_x")
                            .unwrap_or(transform.position.x),
                        body.get::<f32>("__last_world_y")
                            .unwrap_or(transform.position.y),
                        body.get::<f32>("__last_world_z")
                            .unwrap_or(transform.position.z),
                    );
                    let movement = crate::render3d::Vec3::new(
                        transform.position.x - previous.x,
                        transform.position.y - previous.y,
                        transform.position.z - previous.z,
                    );
                    let correction_percent = options
                        .as_ref()
                        .and_then(|options| options.get::<f32>("correction_percent").ok())
                        .filter(|value| value.is_finite())
                        .unwrap_or(1.0)
                        .clamp(0.0, 1.0);
                    let slop = options
                        .as_ref()
                        .and_then(|options| options.get::<f32>("slop").ok())
                        .or_else(|| body.get::<f32>("contact_slop").ok())
                        .filter(|value| value.is_finite())
                        .unwrap_or(0.001)
                        .max(0.0);
                    let response_enabled = options
                        .as_ref()
                        .and_then(|options| options.get::<bool>("response").ok())
                        .unwrap_or(true)
                        && body.get::<bool>("enabled").unwrap_or(true)
                        && !body.get::<bool>("is_static").unwrap_or(false);

                    let mut registry = resolve_registry
                        .lock()
                        .map_err(|_| mlua::Error::external("3D collider registry lock poisoned"))?;
                    for collider in registry.values_mut() {
                        collider
                            .refresh_mesh_if_changed()
                            .map_err(mlua::Error::external)?;
                    }
                    let mut colliders = registry
                        .iter()
                        .map(|(key, collider)| (*key, collider.clone()))
                        .collect::<Vec<_>>();
                    drop(registry);
                    colliders.sort_unstable_by_key(|(key, collider)| (collider.id, *key));
                    let mut colliders = colliders
                        .into_iter()
                        .map(|(_, collider)| collider)
                        .collect::<Vec<_>>();
                    for collider in colliders
                        .iter_mut()
                        .filter(|collider| collider.id == entity_id)
                    {
                        collider.transform.position.x += movement.x;
                        collider.transform.position.y += movement.y;
                        collider.transform.position.z += movement.z;
                        collider.transform.euler = transform.euler;
                        collider.transform.scale = transform.scale;
                        collider.body_is_static = body.get::<bool>("is_static").unwrap_or(false);
                    }

                    let generated =
                        crate::physics3d::contacts(&colliders).map_err(mlua::Error::external)?;
                    let contact_values = lua.create_table()?;
                    let mut contact_count = 0usize;
                    let mut trigger_count = 0usize;
                    let mut exact_count = 0usize;
                    let mut bounds_count = 0usize;
                    let mut resolved_count = 0usize;
                    let mut grounded = false;
                    let mut correction = crate::render3d::Vec3::ZERO;
                    let mut velocity = crate::render3d::Vec3::new(
                        body.get::<f32>("velocity_x").unwrap_or(0.0),
                        body.get::<f32>("velocity_y").unwrap_or(0.0),
                        body.get::<f32>("velocity_z").unwrap_or(0.0),
                    );

                    for contact in generated {
                        if contact.first_id == entity_id && contact.second_id == entity_id {
                            continue;
                        }
                        let (own_index, other_index, normal) = if contact.first_id == entity_id {
                            (contact.first_index, contact.second_index, contact.normal)
                        } else if contact.second_id == entity_id {
                            (
                                contact.second_index,
                                contact.first_index,
                                crate::render3d::Vec3::new(
                                    -contact.normal.x,
                                    -contact.normal.y,
                                    -contact.normal.z,
                                ),
                            )
                        } else {
                            continue;
                        };
                        let own = &colliders[own_index];
                        let other = &colliders[other_index];
                        let exact = contact.quality == crate::physics3d::ContactQuality3D::Exact;
                        let can_resolve = response_enabled
                            && exact
                            && !contact.has_trigger
                            && own.response_enabled
                            && other.response_enabled;

                        contact_count += 1;
                        if contact.has_trigger {
                            trigger_count += 1;
                        }
                        if exact {
                            exact_count += 1;
                        } else {
                            bounds_count += 1;
                        }
                        let value = lua.create_table()?;
                        value.set("self_id", entity_id)?;
                        value.set("other_id", other.id)?;
                        value.set("normal_x", normal.x)?;
                        value.set("normal_y", normal.y)?;
                        value.set("normal_z", normal.z)?;
                        value.set("penetration", contact.penetration)?;
                        value.set("x", contact.point.x)?;
                        value.set("y", contact.point.y)?;
                        value.set("z", contact.point.z)?;
                        value.set("is_trigger", contact.has_trigger)?;
                        value.set("quality", contact.quality.as_str())?;
                        value.set("exact", exact)?;
                        value.set("can_resolve", can_resolve)?;
                        contact_values.raw_set(contact_count, value.clone())?;

                        let callback_name = if contact.has_trigger {
                            "onTrigger"
                        } else {
                            "onContact"
                        };
                        if let Ok(Value::Function(callback)) = body.get::<Value>(callback_name) {
                            callback.call::<()>(value)?;
                        }

                        if !can_resolve {
                            continue;
                        }
                        let correction_share = if other.body_is_static { 1.0 } else { 0.5 };
                        let correction_distance = (contact.penetration - slop).max(0.0)
                            * correction_percent
                            * correction_share;
                        let mut contact_correction = crate::render3d::Vec3::new(
                            -normal.x * correction_distance,
                            -normal.y * correction_distance,
                            -normal.z * correction_distance,
                        );
                        for (axis, freeze_field) in
                            ["freeze_x", "freeze_y", "freeze_z"].into_iter().enumerate()
                        {
                            if body.get::<bool>(freeze_field).unwrap_or(false) {
                                match axis {
                                    0 => contact_correction.x = 0.0,
                                    1 => contact_correction.y = 0.0,
                                    _ => contact_correction.z = 0.0,
                                }
                            }
                        }
                        correction.x += contact_correction.x;
                        correction.y += contact_correction.y;
                        correction.z += contact_correction.z;
                        let restitution = own.restitution.max(other.restitution);
                        let friction = (own.friction * other.friction).sqrt();
                        velocity = crate::physics3d::resolve_contact_velocity(
                            velocity,
                            normal,
                            restitution,
                            friction,
                        );
                        grounded |= normal.y < -0.5;
                        resolved_count += 1;
                    }

                    entity.set("x", entity.get::<f32>("x").unwrap_or(0.0) + correction.x)?;
                    entity.set("y", entity.get::<f32>("y").unwrap_or(0.0) + correction.y)?;
                    entity.set(
                        "position_z",
                        entity.get::<f32>("position_z").unwrap_or(0.0) + correction.z,
                    )?;
                    body.set("velocity_x", velocity.x)?;
                    body.set("velocity_y", velocity.y)?;
                    body.set("velocity_z", velocity.z)?;
                    body.set("__last_world_x", transform.position.x + correction.x)?;
                    body.set("__last_world_y", transform.position.y + correction.y)?;
                    body.set("__last_world_z", transform.position.z + correction.z)?;

                    // Keep the shared snapshot coherent for bodies updated
                    // later in the same frame. Collider3D.update will replace
                    // this translated snapshot with its exact offset-derived
                    // transform when it runs, regardless of component order.
                    let mut registry = resolve_registry
                        .lock()
                        .map_err(|_| mlua::Error::external("3D collider registry lock poisoned"))?;
                    for collider in registry
                        .values_mut()
                        .filter(|collider| collider.id == entity_id)
                    {
                        collider.transform.position.x += movement.x + correction.x;
                        collider.transform.position.y += movement.y + correction.y;
                        collider.transform.position.z += movement.z + correction.z;
                        collider.transform.euler = transform.euler;
                        collider.transform.scale = transform.scale;
                        collider.body_is_static = body.get::<bool>("is_static").unwrap_or(false);
                    }
                    drop(registry);

                    let result = lua.create_table()?;
                    result.set("contacts", contact_values)?;
                    result.set("contact_count", contact_count)?;
                    result.set("trigger_count", trigger_count)?;
                    result.set("exact_count", exact_count)?;
                    result.set("bounds_count", bounds_count)?;
                    result.set("resolved_count", resolved_count)?;
                    result.set("grounded", grounded)?;
                    result.set("correction_x", correction.x)?;
                    result.set("correction_y", correction.y)?;
                    result.set("correction_z", correction.z)?;
                    Ok(result)
                },
            )?,
        )?;

        let broadphase_registry = registry.clone();
        physics.set(
            "broadphasePairs",
            lua.create_function(move |lua, ()| {
                let mut registry = broadphase_registry
                    .lock()
                    .map_err(|_| mlua::Error::external("3D collider registry lock poisoned"))?;
                for collider in registry.values_mut() {
                    collider
                        .refresh_mesh_if_changed()
                        .map_err(mlua::Error::external)?;
                }
                let mut colliders = registry
                    .iter()
                    .map(|(key, collider)| (*key, collider.clone()))
                    .collect::<Vec<_>>();
                drop(registry);
                colliders.sort_unstable_by_key(|(key, collider)| (collider.id, *key));
                let colliders = colliders
                    .into_iter()
                    .map(|(_, collider)| collider)
                    .collect::<Vec<_>>();
                let pairs = crate::physics3d::broadphase_pairs(&colliders)
                    .map_err(mlua::Error::external)?;
                let result = lua.create_table_with_capacity(pairs.len(), 0)?;
                for (index, pair) in pairs.into_iter().enumerate() {
                    let value = lua.create_table()?;
                    value.set("first_id", pair.first_id)?;
                    value.set("second_id", pair.second_id)?;
                    value.set("has_trigger", pair.has_trigger)?;
                    result.raw_set(index + 1, value)?;
                }
                Ok(result)
            })?,
        )?;
        lua.globals().set("physics3d", physics.clone())?;
        lua.globals().set("physics3D", physics)?;
    }

    // SpatialSound2D
    // Plays a sound at the owning entity's world position and keeps the
    // emitter position synchronized while it is active.
    {
        let spatial_sound = lua.create_table()?;
        spatial_sound.set("__neolove_component", "SpatialSound2D")?;
        spatial_sound.set(
            "awake",
            lua.create_function(|_lua, (_entity, component): (Table, Table)| {
                component.set("__neolove_component", "SpatialSound2D")?;
                component.set("sound", Value::Nil)?;
                component.set("volume", 1.0)?;
                component.set("looping", false)?;
                component.set("autoplay", false)?;
                component.set("__autoplay_started", false)?;
                component.set("__playing", false)?;
                Ok(())
            })?,
        )?;

        let play = lua.create_function(|lua, component: Table| {
            let sound = component.get::<Value>("sound").unwrap_or(Value::Nil);
            if !matches!(&sound, Value::UserData(_)) {
                return Ok(false);
            }
            let entity: Table = component.get("entity")?;
            let (x, y, _) = crate::window::get_global_transform(&entity)?;
            let looping = component.get::<bool>("looping").unwrap_or(false);
            let volume = component
                .get::<f32>("volume")
                .unwrap_or(1.0)
                .clamp(0.0, 1.0);
            let audio: Table = lua.globals().get("audio")?;
            audio
                .get::<Function>("playSpatial")?
                .call::<()>((sound, x, y, looping, volume))?;
            component.set("__autoplay_started", true)?;
            component.set("__playing", true)?;
            Ok(true)
        })?;
        spatial_sound.set("play", play.clone())?;
        spatial_sound.set("Play", play.clone())?;

        let stop = lua.create_function(|lua, component: Table| {
            let sound = component.get::<Value>("sound").unwrap_or(Value::Nil);
            if matches!(&sound, Value::UserData(_)) {
                let audio: Table = lua.globals().get("audio")?;
                audio.get::<Function>("stop")?.call::<()>(sound)?;
            }
            component.set("__playing", false)
        })?;
        spatial_sound.set("stop", stop.clone())?;
        spatial_sound.set("Stop", stop.clone())?;

        let play_for_update = play.clone();
        spatial_sound.set(
            "update",
            lua.create_function(move |lua, (entity, component, _dt): (Table, Table, f32)| {
                let autoplay = component.get::<bool>("autoplay").unwrap_or(false);
                let started = component.get::<bool>("__autoplay_started").unwrap_or(false);
                if autoplay && !started {
                    let _ = play_for_update.call::<bool>(component.clone())?;
                }
                if component.get::<bool>("__playing").unwrap_or(false) {
                    let sound = component.get::<Value>("sound").unwrap_or(Value::Nil);
                    if matches!(&sound, Value::UserData(_)) {
                        let (x, y, _) = crate::window::get_global_transform(&entity)?;
                        let audio: Table = lua.globals().get("audio")?;
                        let _ = audio
                            .get::<Function>("setPosition")?
                            .call::<bool>((sound, x, y))?;
                    }
                }
                Ok(())
            })?,
        )?;

        spatial_sound.set(
            "destroy",
            lua.create_function(move |lua, (_entity, component): (Table, Table)| {
                let sound = component.get::<Value>("sound").unwrap_or(Value::Nil);
                if matches!(&sound, Value::UserData(_)) {
                    let audio: Table = lua.globals().get("audio")?;
                    audio.get::<Function>("stop")?.call::<()>(sound)?;
                }
                Ok(())
            })?,
        )?;
        core_components.set("SpatialSound2D", spatial_sound)?;
    }

    // Rect2d
    // basic renderer
    {
        let rect2d = create_basic_drawable(lua)?;
        rect2d.set("__neolove_component", "Rect2D")?;
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
                let shader = shader_from_component(&component)?;
                let use_middle_pivot = crate::window::uses_middle_pivot(&entity);
                let (draw_x, draw_y, offset) = if use_middle_pivot {
                    let (px, py) = crate::window::get_global_rotation_pivot(&entity)?;
                    // `DrawCommand::Rect` treats (x, y) as the unrotated top-left
                    // corner and `offset` as the pivot fraction within the rect,
                    // so the top-left must be the pivot minus half the size for
                    // the 0.5/0.5 offset to land exactly on the center pivot.
                    (px - w * 0.5, py - h * 0.5, Vec2 { x: 0.5, y: 0.5 })
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
                    shader,
                });
                Ok(())
            })?,
        )?;

        core_components.set("Rect2D", rect2d)?;
    }

    // lighting
    // global module controlling the 2D lighting compositor
    {
        use crate::lighting::{LightConfig, LightQuality};

        let lighting = lua.create_table()?;

        macro_rules! edit_config {
            ($ty:ty, |$cfg:ident, $arg:pat_param| $body:expr) => {{
                let render_state = render_state.clone();
                lua.create_function(move |_ctx, $arg: $ty| -> mlua::Result<()> {
                    let mut state = render_state
                        .lock()
                        .map_err(|_| mlua::Error::external("render state lock poisoned"))?;
                    #[allow(unused_mut)]
                    let mut $cfg = state.lighting_config();
                    $body;
                    state.set_lighting_config($cfg);
                    Ok(())
                })?
            }};
        }

        lighting.set(
            "setEnabled",
            edit_config!(Option<bool>, |config, enabled| {
                config.enabled = enabled.unwrap_or(true);
            }),
        )?;
        lighting.set(
            "enable",
            edit_config!((), |config, _unused| config.enabled = true),
        )?;
        lighting.set(
            "disable",
            edit_config!((), |config, _unused| config.enabled = false),
        )?;

        {
            let render_state = render_state.clone();
            lighting.set(
                "isEnabled",
                lua.create_function(move |_ctx, ()| {
                    let state = render_state
                        .lock()
                        .map_err(|_| mlua::Error::external("render state lock poisoned"))?;
                    Ok(state.lighting_config().enabled)
                })?,
            )?;
        }

        lighting.set(
            "setAmbient",
            edit_config!((Table, Option<f32>), |config, (color, intensity)| {
                if let Ok(c) = color4_to_color(color) {
                    config.ambient = c;
                }
                if let Some(intensity) = intensity {
                    config.ambient_intensity = intensity.max(0.0);
                }
            }),
        )?;
        lighting.set(
            "setAmbientIntensity",
            edit_config!(f32, |config, intensity| {
                config.ambient_intensity = intensity.max(0.0);
            }),
        )?;

        {
            let render_state = render_state.clone();
            lighting.set(
                "getAmbient",
                lua.create_function(move |ctx, ()| {
                    let config = render_state
                        .lock()
                        .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                        .lighting_config();
                    let color = color4(
                        ctx,
                        config.ambient.r,
                        config.ambient.g,
                        config.ambient.b,
                        config.ambient.a,
                    )?;
                    Ok((color, config.ambient_intensity))
                })?,
            )?;
        }

        lighting.set(
            "setAmbientOcclusion",
            edit_config!(
                (Option<bool>, Option<f32>, Option<f32>, Option<u32>),
                |config, (enabled, radius, intensity, samples)| {
                    config.ao_enabled = enabled.unwrap_or(true);
                    if let Some(radius) = radius {
                        config.ao_radius = radius.max(0.0);
                    }
                    if let Some(intensity) = intensity {
                        config.ao_intensity = intensity.clamp(0.0, 1.0);
                    }
                    if let Some(samples) = samples {
                        config.ao_samples = samples.clamp(1, 64);
                    }
                }
            ),
        )?;
        lighting.set(
            "setShadows",
            edit_config!(
                (Option<bool>, Option<f32>),
                |config, (enabled, softness)| {
                    config.shadows_enabled = enabled.unwrap_or(true);
                    if let Some(softness) = softness {
                        config.soft_shadows = softness.max(0.0);
                    }
                }
            ),
        )?;
        lighting.set(
            "setBloom",
            edit_config!(f32, |config, amount| config.bloom = amount.max(0.0)),
        )?;
        lighting.set(
            "setExposure",
            edit_config!(f32, |config, value| config.exposure = value.max(0.0)),
        )?;
        lighting.set(
            "setQuality",
            edit_config!(String, |config, quality| {
                config.quality = LightQuality::parse(&quality);
            }),
        )?;

        {
            let render_state = render_state.clone();
            lighting.set(
                "getQuality",
                lua.create_function(move |_ctx, ()| {
                    let config = render_state
                        .lock()
                        .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                        .lighting_config();
                    Ok(config.quality.as_str().to_string())
                })?,
            )?;
        }

        {
            let render_state = render_state.clone();
            lighting.set(
                "reset",
                lua.create_function(move |_ctx, ()| {
                    render_state
                        .lock()
                        .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                        .set_lighting_config(LightConfig::default());
                    Ok(())
                })?,
            )?;
        }

        // sample(x, y) -> Color4?  The light reaching a world/screen position,
        // as a color, or nil when the point is off-screen. Uses the last
        // completed frame's lights/occluders so it is safe to call from update
        // (this frame's lights are still being queued). Returns opaque white
        // when lighting is disabled (everything is effectively fully lit).
        {
            let render_state = render_state.clone();
            let sample = lua.create_function(move |lua, (x, y): (f32, f32)| {
                // Reject points outside the logical window when its size is known.
                if let Ok(Some(window)) = lua.globals().get::<Option<Table>>("window") {
                    let w: f32 = window.get("x").unwrap_or(0.0);
                    let h: f32 = window.get("y").unwrap_or(0.0);
                    if w > 0.0 && h > 0.0 && !(x >= 0.0 && y >= 0.0 && x <= w && y <= h) {
                        return Ok(Value::Nil);
                    }
                }
                let (config, lights, occluders) = render_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                    .last_frame_lighting();
                if !config.enabled {
                    return Ok(Value::Table(color4(lua, 255, 255, 255, 255)?));
                }
                let (r, g, b) =
                    crate::lighting::sample_light_at(x, y, &config, &lights, &occluders);
                let to_byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                let color = color4(lua, to_byte(r), to_byte(g), to_byte(b), 255)?;
                Ok(Value::Table(color))
            })?;
            lighting.set("sample", sample.clone())?;
            lighting.set("getAt", sample.clone())?;
            lighting.set("sampleAt", sample)?;
        }

        lua.globals().set("lighting", lighting)?;
    }

    // Post-process stack
    // Effects are ordered and mutable at runtime. The software path is also the
    // deterministic reference used to validate backend shader equivalents.
    {
        let postprocess = lua.create_table()?;

        {
            let render_state = render_state.clone();
            postprocess.set(
                "setEnabled",
                lua.create_function(move |_lua, enabled: Option<bool>| {
                    render_state
                        .lock()
                        .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                        .edit_post_process(|stack| stack.enabled = enabled.unwrap_or(true));
                    Ok(())
                })?,
            )?;
        }
        {
            let render_state = render_state.clone();
            postprocess.set(
                "isEnabled",
                lua.create_function(move |_lua, ()| {
                    Ok(render_state
                        .lock()
                        .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                        .post_process()
                        .enabled)
                })?,
            )?;
        }
        {
            let render_state = render_state.clone();
            postprocess.set(
                "add",
                lua.create_function(move |_lua, (kind, options): (String, Option<Table>)| {
                    let enabled = post_process_bool(&options, "enabled", true);
                    let effect = post_process_effect(&kind, options)?;
                    let mut index = 0usize;
                    render_state
                        .lock()
                        .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                        .edit_post_process(|stack| {
                            index = stack.push(effect);
                            if !enabled {
                                stack.effects[index].enabled = false;
                            }
                        });
                    Ok(index as u64 + 1)
                })?,
            )?;
        }
        {
            let render_state = render_state.clone();
            postprocess.set(
                "remove",
                lua.create_function(move |_lua, index: i64| {
                    if index <= 0 {
                        return Ok(false);
                    }
                    let mut removed = false;
                    render_state
                        .lock()
                        .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                        .edit_post_process(|stack| {
                            removed = stack.remove(index as usize - 1).is_some()
                        });
                    Ok(removed)
                })?,
            )?;
        }
        {
            let render_state = render_state.clone();
            postprocess.set(
                "move",
                lua.create_function(move |_lua, (from, to): (i64, i64)| {
                    if from <= 0 || to <= 0 {
                        return Ok(false);
                    }
                    let mut moved = false;
                    render_state
                        .lock()
                        .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                        .edit_post_process(|stack| {
                            moved = stack.move_effect(from as usize - 1, to as usize - 1)
                        });
                    Ok(moved)
                })?,
            )?;
        }
        {
            let render_state = render_state.clone();
            postprocess.set(
                "setPassEnabled",
                lua.create_function(move |_lua, (index, enabled): (i64, bool)| {
                    if index <= 0 {
                        return Ok(false);
                    }
                    let mut changed = false;
                    render_state
                        .lock()
                        .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                        .edit_post_process(|stack| {
                            if let Some(pass) = stack.effects.get_mut(index as usize - 1) {
                                pass.enabled = enabled;
                                changed = true;
                            }
                        });
                    Ok(changed)
                })?,
            )?;
        }
        {
            let render_state = render_state.clone();
            postprocess.set(
                "clear",
                lua.create_function(move |_lua, ()| {
                    render_state
                        .lock()
                        .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                        .edit_post_process(|stack| {
                            stack.effects.clear();
                            stack.clear_history();
                        });
                    Ok(())
                })?,
            )?;
        }
        {
            let render_state = render_state.clone();
            postprocess.set(
                "resetHistory",
                lua.create_function(move |_lua, ()| {
                    render_state
                        .lock()
                        .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                        .edit_post_process(|stack| stack.clear_history());
                    Ok(())
                })?,
            )?;
        }
        {
            let render_state = render_state.clone();
            postprocess.set(
                "count",
                lua.create_function(move |_lua, ()| {
                    Ok(render_state
                        .lock()
                        .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                        .post_process()
                        .effects
                        .len() as u64)
                })?,
            )?;
        }

        lua.globals().set("postprocess", postprocess.clone())?;
        lua.globals().set("postProcess", postprocess)?;
    }

    // Rng
    // seedable random-number generators as first-class objects
    {
        lua.globals().set("Rng", crate::rng::create_module(lua)?)?;
    }

    // Light2D
    // emits a point, spot, or directional light into the lighting compositor
    {
        use crate::lighting::{Light, LightKind};

        let light2d = lua.create_table()?;
        light2d.set("__neolove_component", "Light2D")?;
        light2d.set("NEOLOVE_RENDERING", true)?;
        light2d.set(
            "awake",
            lua.create_function(move |ctx, (_entity, component): (Table, Table)| {
                component.set("kind", "point")?;
                component.set("color", color4(ctx, 255, 255, 255, 255)?)?;
                component.set("intensity", 1.0)?;
                component.set("radius", 256.0)?;
                component.set("falloff", 2.0)?;
                component.set("angleOffset", 0.0)?;
                component.set("coneAngle", 60.0)?;
                component.set("coneSoftness", 0.35)?;
                component.set("castsShadows", true)?;
                // Negative means "use the global lighting.setShadows softness".
                component.set("shadowSoftness", -1.0)?;
                component.set("visible", true)?;
                Ok(())
            })?,
        )?;

        let render_state = render_state.clone();
        light2d.set(
            "update",
            lua.create_function(move |_ctx, (entity, component, _dt): (Table, Table, f32)| {
                if !component.get::<bool>("visible").unwrap_or(true) {
                    return Ok(());
                }
                let (x, y, rotation) = crate::window::get_global_transform(&entity)?;
                let kind = LightKind::parse(
                    &component
                        .get::<String>("kind")
                        .unwrap_or_else(|_| "point".to_string()),
                );
                let color = match component.get::<Table>("color") {
                    Ok(table) => color4_to_color(table)?,
                    Err(_) => Color::WHITE,
                };
                let angle_offset = component
                    .get::<f32>("angleOffset")
                    .unwrap_or(0.0)
                    .to_radians();
                let cone_deg = component.get::<f32>("coneAngle").unwrap_or(60.0).max(0.0);
                let light = Light {
                    kind,
                    x,
                    y,
                    radius: component.get::<f32>("radius").unwrap_or(256.0).max(0.0),
                    color,
                    intensity: component.get::<f32>("intensity").unwrap_or(1.0).max(0.0),
                    falloff: component.get::<f32>("falloff").unwrap_or(2.0).max(0.1),
                    angle: rotation + angle_offset,
                    cone: (cone_deg * 0.5).to_radians(),
                    cone_softness: component.get::<f32>("coneSoftness").unwrap_or(0.35),
                    casts_shadows: component.get::<bool>("castsShadows").unwrap_or(true),
                    shadow_softness: component.get::<f32>("shadowSoftness").unwrap_or(-1.0),
                };
                render_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                    .queue_light(light);
                Ok(())
            })?,
        )?;
        core_components.set("Light2D", light2d)?;
    }

    // LightOccluder2D
    // blocks light and contributes ambient occlusion using its bounds
    {
        use crate::lighting::Occluder;

        let occluder2d = lua.create_table()?;
        occluder2d.set("__neolove_component", "LightOccluder2D")?;
        occluder2d.set("NEOLOVE_RENDERING", true)?;
        occluder2d.set(
            "awake",
            lua.create_function(move |_ctx, (_entity, component): (Table, Table)| {
                component.set("visible", true)?;
                component.set("shape", "box")?;
                Ok(())
            })?,
        )?;

        let render_state = render_state.clone();
        occluder2d.set(
            "update",
            lua.create_function(move |_ctx, (entity, component, _dt): (Table, Table, f32)| {
                if !component.get::<bool>("visible").unwrap_or(true) {
                    return Ok(());
                }
                let (x, y, rotation) = crate::window::get_global_transform(&entity)?;
                let (w, h) = crate::window::get_global_size(&entity)?;
                if w <= 0.0 || h <= 0.0 {
                    return Ok(());
                }
                // The entity position is its top-left; the occluder center is the
                // middle of the (possibly rotated) bounds, matching the drawn rect.
                let (offset_x, offset_y) = rotate_local(w * 0.5, h * 0.5, rotation);
                render_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                    .queue_occluder(Occluder {
                        cx: x + offset_x,
                        cy: y + offset_y,
                        half_w: w * 0.5,
                        half_h: h * 0.5,
                        rotation,
                        shape: crate::lighting::OccluderShape::parse(
                            &component
                                .get::<String>("shape")
                                .unwrap_or_else(|_| "box".to_string()),
                        ),
                    });
                Ok(())
            })?,
        )?;
        core_components.set("LightOccluder2D", occluder2d)?;
    }

    // Shape2D
    // renderer for box, circle, and right-triangle primitives
    {
        let shape2d = create_basic_drawable(lua)?;
        shape2d.set("__neolove_component", "Shape2D")?;
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
                let shader = shader_from_component(&component)?;
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
                            shader: shader.clone(),
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
                        renderer.queue(DrawCommand::Triangle {
                            a,
                            b,
                            c,
                            color,
                            shader: shader.clone(),
                        });
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
                            shader: shader.clone(),
                        });
                        renderer.queue(DrawCommand::Triangle {
                            a: p0,
                            b: p2,
                            c: p3,
                            color,
                            shader,
                        });
                    }
                }
                Ok(())
            })?,
        )?;

        core_components.set("Shape2D", shape2d)?;
    }

    // ParticleSystem2D
    // A deterministic, allocation-bounded circle particle emitter.
    {
        let particles = create_basic_drawable(lua)?;
        particles.set("__neolove_component", "ParticleSystem2D")?;
        particles.set(
            "awake",
            lua.create_function(move |ctx, (_entity, component): (Table, Table)| {
                component.set("__neolove_component", "ParticleSystem2D")?;
                component.set("visible", true)?;
                component.set("image", Value::Nil)?;
                component.set("shader", Value::Nil)?;
                component.set("playing", true)?;
                component.set("looping", true)?;
                component.set("duration", 5.0)?;
                component.set("emission_rate", 12.0)?;
                component.set("max_particles", 256)?;
                component.set("lifetime", 1.5)?;
                component.set("speed", 80.0)?;
                component.set("direction", -90.0)?;
                component.set("spread", 30.0)?;
                component.set("start_size", 8.0)?;
                component.set("end_size", 2.0)?;
                component.set("start_color", color4(ctx, 255, 184, 76, 255)?)?;
                component.set("end_color", color4(ctx, 255, 92, 40, 0)?)?;
                let color_sequence = ctx.create_table()?;
                let color_start = ctx.create_table()?;
                color_start.set("time", 0.0)?;
                color_start.set("color", color4(ctx, 255, 184, 76, 255)?)?;
                color_sequence.set(1, color_start)?;
                let color_end = ctx.create_table()?;
                color_end.set("time", 1.0)?;
                color_end.set("color", color4(ctx, 255, 92, 40, 255)?)?;
                color_sequence.set(2, color_end)?;
                component.set("color_sequence", color_sequence)?;
                let transparency_sequence = ctx.create_table()?;
                let transparency_start = ctx.create_table()?;
                transparency_start.set("time", 0.0)?;
                transparency_start.set("value", 0.0)?;
                transparency_sequence.set(1, transparency_start)?;
                let transparency_end = ctx.create_table()?;
                transparency_end.set("time", 1.0)?;
                transparency_end.set("value", 1.0)?;
                transparency_sequence.set(2, transparency_end)?;
                component.set("transparency_sequence", transparency_sequence)?;
                component.set("shape", "point")?;
                component.set("radius", 32.0)?;
                component.set("gravity_x", 0.0)?;
                component.set("gravity_y", 60.0)?;
                component.set("particle_count", 0)?;
                component.set("__particles", ctx.create_table()?)?;
                component.set("__emit_accumulator", 0.0)?;
                component.set("__elapsed", 0.0)?;
                component.set("__manual_emit", 0)?;
                component.set("__rng", 0x6d2b_79f5_u32 as i64)?;
                Ok(())
            })?,
        )?;

        let play = lua.create_function(|_ctx, component: Table| component.set("playing", true))?;
        particles.set("play", play.clone())?;
        particles.set("Play", play)?;
        let pause =
            lua.create_function(|_ctx, component: Table| component.set("playing", false))?;
        particles.set("pause", pause.clone())?;
        particles.set("Pause", pause)?;
        let stop = lua.create_function(|ctx, component: Table| {
            component.set("playing", false)?;
            component.set("__particles", ctx.create_table()?)?;
            component.set("__emit_accumulator", 0.0)?;
            component.set("__elapsed", 0.0)?;
            component.set("particle_count", 0)?;
            Ok(())
        })?;
        particles.set("stop", stop.clone())?;
        particles.set("Stop", stop)?;
        let emit = lua.create_function(|_ctx, (component, count): (Table, Option<i32>)| {
            let pending = component.get::<i32>("__manual_emit").unwrap_or(0);
            component.set(
                "__manual_emit",
                pending.saturating_add(count.unwrap_or(1).max(0)),
            )
        })?;
        particles.set("emit", emit.clone())?;
        particles.set("Emit", emit)?;

        let render_state = render_state.clone();
        particles.set(
            "update",
            lua.create_function(move |ctx, (entity, component, dt): (Table, Table, f32)| {
                let dt = dt.clamp(0.0, 0.25);
                let visible = component.get::<bool>("visible").unwrap_or(true);
                let mut playing = component.get::<bool>("playing").unwrap_or(true);
                let looping = component.get::<bool>("looping").unwrap_or(true);
                let duration = component.get::<f32>("duration").unwrap_or(5.0).max(0.0);
                let mut elapsed = component.get::<f32>("__elapsed").unwrap_or(0.0);
                if playing {
                    elapsed += dt;
                    if duration > 0.0 && elapsed >= duration {
                        if looping {
                            elapsed %= duration;
                        } else {
                            playing = false;
                            component.set("playing", false)?;
                        }
                    }
                    component.set("__elapsed", elapsed)?;
                }

                let lifetime = component.get::<f32>("lifetime").unwrap_or(1.5).max(0.001);
                let max_particles = component
                    .get::<i32>("max_particles")
                    .unwrap_or(256)
                    .clamp(1, 10_000) as usize;
                let entity_scale = crate::window::get_global_scale(&entity)?.abs();
                let gravity_x = component.get::<f32>("gravity_x").unwrap_or(0.0);
                let gravity_y = component.get::<f32>("gravity_y").unwrap_or(60.0);
                let start_size =
                    component.get::<f32>("start_size").unwrap_or(8.0).max(0.0) * entity_scale;
                let end_size =
                    component.get::<f32>("end_size").unwrap_or(2.0).max(0.0) * entity_scale;
                let start_color = color4_to_color(component.get("start_color")?)?;
                let end_color = color4_to_color(component.get("end_color")?)?;
                let color_sequence =
                    read_particle_color_sequence(&component, start_color, end_color)?;
                let transparency_sequence = read_particle_number_sequence(
                    &component,
                    1.0 - start_color.a as f32 / 255.0,
                    1.0 - end_color.a as f32 / 255.0,
                )?;
                let shader = shader_from_component(&component)?;
                let image = get_image_field(&component, "image")?;
                let filter = app_texture_filter(ctx);

                let current = component
                    .get::<Option<Table>>("__particles")?
                    .unwrap_or(ctx.create_table()?);
                let next = ctx.create_table()?;
                let mut count = 0usize;
                let mut draw = Vec::new();
                for value in current.sequence_values::<Table>() {
                    let particle = value?;
                    let age = particle.get::<f32>("age").unwrap_or(0.0) + dt;
                    let particle_lifetime = particle
                        .get::<f32>("lifetime")
                        .unwrap_or(lifetime)
                        .max(0.001);
                    if age >= particle_lifetime || count >= max_particles {
                        continue;
                    }
                    let mut vx = particle.get::<f32>("vx").unwrap_or(0.0);
                    let mut vy = particle.get::<f32>("vy").unwrap_or(0.0);
                    let mut x = particle.get::<f32>("x").unwrap_or(0.0);
                    let mut y = particle.get::<f32>("y").unwrap_or(0.0);
                    vx += gravity_x * dt;
                    vy += gravity_y * dt;
                    x += vx * dt;
                    y += vy * dt;
                    particle.set("age", age)?;
                    particle.set("x", x)?;
                    particle.set("y", y)?;
                    particle.set("vx", vx)?;
                    particle.set("vy", vy)?;
                    count += 1;
                    next.raw_set(count, particle)?;
                    if visible {
                        let t = age / particle_lifetime;
                        let size = start_size + (end_size - start_size) * t;
                        let mut color = sample_particle_color(&color_sequence, t);
                        color.a = ((1.0
                            - sample_particle_number(&transparency_sequence, t).clamp(0.0, 1.0))
                            * 255.0)
                            .round() as u8;
                        draw.push((x, y, size, color));
                    }
                }

                let rate = component
                    .get::<f32>("emission_rate")
                    .unwrap_or(12.0)
                    .max(0.0);
                let mut accumulator = component.get::<f32>("__emit_accumulator").unwrap_or(0.0);
                if playing {
                    accumulator += rate * dt;
                }
                let automatic = accumulator.floor().max(0.0) as usize;
                accumulator -= automatic as f32;
                let manual = component.get::<i32>("__manual_emit").unwrap_or(0).max(0) as usize;
                component.set("__manual_emit", 0)?;
                component.set("__emit_accumulator", accumulator)?;
                let spawn_count = automatic
                    .saturating_add(manual)
                    .min(max_particles.saturating_sub(count));

                let (origin_x, origin_y, rotation) = crate::window::get_global_transform(&entity)?;
                let (entity_w, entity_h) = crate::window::get_global_size(&entity)?;
                let speed = component.get::<f32>("speed").unwrap_or(80.0) * entity_scale;
                let direction = component
                    .get::<f32>("direction")
                    .unwrap_or(-90.0)
                    .to_radians();
                let spread = component
                    .get::<f32>("spread")
                    .unwrap_or(30.0)
                    .abs()
                    .to_radians();
                let shape = component
                    .get::<String>("shape")
                    .unwrap_or_else(|_| "point".to_string())
                    .to_ascii_lowercase();
                let radius = component.get::<f32>("radius").unwrap_or(32.0).max(0.0) * entity_scale;
                let mut seed = component.get::<i64>("__rng").unwrap_or(0x6d2b_79f5) as u32;

                for _ in 0..spawn_count {
                    let (local_x, local_y) = match shape.as_str() {
                        "box" => (
                            particle_random(&mut seed) * entity_w,
                            particle_random(&mut seed) * entity_h,
                        ),
                        "circle" => {
                            let angle = particle_random(&mut seed) * std::f32::consts::TAU;
                            let distance = particle_random(&mut seed).sqrt() * radius;
                            (angle.cos() * distance, angle.sin() * distance)
                        }
                        _ => (0.0, 0.0),
                    };
                    let (offset_x, offset_y) = rotate_local(local_x, local_y, rotation);
                    let angle = direction + rotation + (particle_random(&mut seed) - 0.5) * spread;
                    let particle = ctx.create_table()?;
                    particle.set("x", origin_x + offset_x)?;
                    particle.set("y", origin_y + offset_y)?;
                    particle.set("vx", angle.cos() * speed)?;
                    particle.set("vy", angle.sin() * speed)?;
                    particle.set("age", 0.0)?;
                    particle.set("lifetime", lifetime)?;
                    count += 1;
                    next.raw_set(count, particle)?;
                    if visible {
                        let mut color = sample_particle_color(&color_sequence, 0.0);
                        color.a = ((1.0
                            - sample_particle_number(&transparency_sequence, 0.0).clamp(0.0, 1.0))
                            * 255.0)
                            .round() as u8;
                        draw.push((origin_x + offset_x, origin_y + offset_y, start_size, color));
                    }
                }
                component.set("__rng", seed as i64)?;
                component.set("__particles", next)?;
                component.set("particle_count", count)?;

                if visible && !draw.is_empty() {
                    let mut renderer = render_state
                        .lock()
                        .map_err(|_| mlua::Error::external("render state lock poisoned"))?;
                    for (x, y, size, color) in draw {
                        if size > 0.0 && color.a > 0 {
                            if let Some(image) = &image {
                                renderer.queue(DrawCommand::Image {
                                    image: image.clone(),
                                    dest: Rect {
                                        x: x - size * 0.5,
                                        y: y - size * 0.5,
                                        w: size,
                                        h: size,
                                    },
                                    source: None,
                                    rotation: 0.0,
                                    pivot: Vec2 { x, y },
                                    tint: color,
                                    filter,
                                    shader: shader.clone(),
                                });
                            } else {
                                renderer.queue(DrawCommand::Circle {
                                    center: Vec2 { x, y },
                                    radius: size * 0.5,
                                    color,
                                    shader: shader.clone(),
                                });
                            }
                        }
                    }
                }
                Ok(())
            })?,
        )?;

        core_components.set("ParticleSystem2D", particles)?;
    }

    // AnimationController: owns a single clip/table and plays it against the
    // entity transform/properties through the animation module.
    {
        let controller = lua.create_table()?;
        controller.set("__neolove_component", "AnimationController")?;
        controller.set(
            "awake",
            lua.create_function(move |_ctx, (_entity, component): (Table, Table)| {
                component.set("__neolove_component", "AnimationController")?;
                component.set("animation", Value::Nil)?;
                component.set("autoplay", true)?;
                component.set("looping", true)?;
                component.set("playing", false)?;
                component.set("speed", 1.0)?;
                component.set("__player", Value::Nil)?;
                Ok(())
            })?,
        )?;

        let play = lua.create_function(|_ctx, component: Table| {
            component.set("playing", true)?;
            if let Ok(player) = component.get::<Table>("__player") {
                let play: Function = player.get("play")?;
                play.call::<()>(player)?;
            }
            Ok(())
        })?;
        controller.set("play", play.clone())?;
        controller.set("Play", play)?;

        let pause = lua.create_function(|_ctx, component: Table| {
            component.set("playing", false)?;
            if let Ok(player) = component.get::<Table>("__player") {
                let pause: Function = player.get("pause")?;
                pause.call::<()>(player)?;
            }
            Ok(())
        })?;
        controller.set("pause", pause.clone())?;
        controller.set("Pause", pause)?;

        let stop = lua.create_function(|_ctx, component: Table| {
            component.set("playing", false)?;
            if let Ok(player) = component.get::<Table>("__player") {
                let stop: Function = player.get("stop")?;
                stop.call::<()>(player)?;
            }
            Ok(())
        })?;
        controller.set("stop", stop.clone())?;
        controller.set("Stop", stop)?;

        controller.set(
            "update",
            lua.create_function(move |ctx, (entity, component, _dt): (Table, Table, f32)| {
                let Some(clip) = component.get::<Option<Table>>("animation")? else {
                    component.set("__player", Value::Nil)?;
                    return Ok(());
                };
                clip.set("looping", component.get::<bool>("looping").unwrap_or(true))?;

                let player = match component.get::<Table>("__player") {
                    Ok(player) => player,
                    Err(_) => {
                        let animations: Table = ctx.globals().get("animation")?;
                        let create: Function = animations.get("create")?;
                        let player: Table = create.call((entity.clone(), clip))?;
                        component.set("__player", player.clone())?;
                        if component.get::<bool>("autoplay").unwrap_or(true) {
                            component.set("playing", true)?;
                        }
                        player
                    }
                };

                let speed = component.get::<f32>("speed").unwrap_or(1.0).max(0.0);
                let set_speed: Function = player.get("setSpeed")?;
                set_speed.call::<()>((player.clone(), speed as f64))?;
                let should_play = component.get::<bool>("playing").unwrap_or(false);
                let command: Function = if should_play {
                    player.get("play")?
                } else {
                    player.get("pause")?
                };
                command.call::<()>(player)?;
                Ok(())
            })?,
        )?;

        core_components.set("AnimationController", controller)?;
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
                component.set("tab_size", 4.0)?;
                component.set("antialiasing", "inherit")?;
                component.set("font", Value::Nil)?;
                component.set("scale_x", 0.0)?;
                component.set("scale_y", 0.0)?;
                component.set("dx", 0.0)?;
                component.set("dy", 0.0)?;
                component.set("line_count", 0)?;
                component.set("__rich_text_ranges", ctx.create_table()?)?;
                component.set("__letter_bounds", ctx.create_table()?)?;
                component.set("__layout_cache_id", "")?;
                bind_textbox_letter_lookup_methods(ctx, &component)?;
                Ok(())
            })?,
        )?;

        let add_rich_method =
            |name: &'static str, key: &'static str, has_value: bool| -> mlua::Result<()> {
                let text_root = text_root.clone();
                textbox.set(
                    name,
                    lua.create_function(move |ctx, args: mlua::Variadic<Value>| {
                        let component = match args.get(0) {
                            Some(Value::Table(t)) => t.clone(),
                            _ => return Ok(()),
                        };
                        let start = match args.get(1) {
                            Some(Value::Integer(v)) => (*v).max(0) as usize,
                            Some(Value::Number(v)) => v.max(0.0) as usize,
                            _ => 0,
                        };
                        let end = match args.get(2) {
                            Some(Value::Integer(v)) => (*v).max(0) as usize,
                            Some(Value::Number(v)) => v.max(0.0) as usize,
                            _ => start,
                        };
                        let ranges =
                            component.get::<Table>("__rich_text_ranges").or_else(|_| {
                                let t = ctx.create_table()?;
                                component.set("__rich_text_ranges", t.clone())?;
                                Ok::<_, mlua::Error>(t)
                            })?;
                        let r = ctx.create_table()?;
                        r.set("start", start)?;
                        r.set("end", end)?;
                        if has_value {
                            if let Some(v) = args.get(3) {
                                r.set(key, v.clone())?;
                            }
                        } else {
                            r.set(key, true)?;
                        }
                        ranges.set(ranges.raw_len() + 1, r)?;
                        let _ = &text_root;
                        Ok(())
                    })?,
                )
            };
        add_rich_method("setBold", "bold", false)?;
        add_rich_method("setItalic", "italic", false)?;
        add_rich_method("setUnderline", "underline", false)?;
        add_rich_method("setColor", "color", true)?;
        add_rich_method("setSize", "size", true)?;
        add_rich_method("setFont", "font", true)?;
        let set_offset = lua.create_function(|ctx, args: mlua::Variadic<Value>| {
            let component = match args.first() {
                Some(Value::Table(table)) => table.clone(),
                _ => return Ok(()),
            };
            let number = |value: Option<&Value>, default: f32| match value {
                Some(Value::Integer(value)) => *value as f32,
                Some(Value::Number(value)) if value.is_finite() => *value as f32,
                _ => default,
            };
            let start = number(args.get(1), 0.0).max(0.0) as usize;
            let end = number(args.get(2), start as f32).max(start as f32) as usize;
            let offset_x = number(args.get(3), 0.0);
            let offset_y = number(args.get(4), 0.0);
            let ranges = component.get::<Table>("__rich_text_ranges").or_else(|_| {
                let ranges = ctx.create_table()?;
                component.set("__rich_text_ranges", ranges.clone())?;
                Ok::<_, mlua::Error>(ranges)
            })?;
            let range = ctx.create_table()?;
            range.set("start", start)?;
            range.set("end", end)?;
            range.set("offset_x", offset_x)?;
            range.set("offset_y", offset_y)?;
            ranges.set(ranges.raw_len() + 1, range)
        })?;
        textbox.set("setOffset", set_offset.clone())?;
        textbox.set("setPixelOffset", set_offset)?;

        let set_character_offset = lua.create_function(|ctx, args: mlua::Variadic<Value>| {
            let component = match args.first() {
                Some(Value::Table(table)) => table.clone(),
                _ => return Ok(()),
            };
            let number = |value: Option<&Value>, default: f32| match value {
                Some(Value::Integer(value)) => *value as f32,
                Some(Value::Number(value)) if value.is_finite() => *value as f32,
                _ => default,
            };
            let index = number(args.get(1), 0.0).max(0.0) as usize;
            let ranges = component.get::<Table>("__rich_text_ranges").or_else(|_| {
                let ranges = ctx.create_table()?;
                component.set("__rich_text_ranges", ranges.clone())?;
                Ok::<_, mlua::Error>(ranges)
            })?;
            let range = ctx.create_table()?;
            range.set("start", index)?;
            range.set("end", index + 1)?;
            range.set("offset_x", number(args.get(2), 0.0))?;
            range.set("offset_y", number(args.get(3), 0.0))?;
            ranges.set(ranges.raw_len() + 1, range)
        })?;
        textbox.set("setCharacterOffset", set_character_offset)?;
        textbox.set(
            "clearAllFormatting",
            lua.create_function(|ctx, component: Table| {
                component.set("__rich_text_ranges", ctx.create_table()?)
            })?,
        )?;

        textbox.set(
            "getLetterCount",
            lua.create_function(|_ctx, component: Table| {
                Ok(component
                    .get::<String>("text")
                    .unwrap_or_default()
                    .chars()
                    .count())
            })?,
        )?;
        install_unbound_textbox_letter_lookup_methods(lua, &textbox, text_root.clone())?;

        textbox.set(
            "clearFormatting",
            lua.create_function(|ctx, args: mlua::Variadic<Value>| {
                let component = match args.get(0) {
                    Some(Value::Table(t)) => t.clone(),
                    _ => return Ok(()),
                };
                if args.len() < 3 {
                    component.set("__rich_text_ranges", ctx.create_table()?)?;
                    return Ok(());
                }
                let start = match args.get(1) {
                    Some(Value::Integer(v)) => (*v).max(0) as usize,
                    Some(Value::Number(v)) => v.max(0.0) as usize,
                    _ => 0,
                };
                let end = match args.get(2) {
                    Some(Value::Integer(v)) => (*v).max(0) as usize,
                    Some(Value::Number(v)) => v.max(0.0) as usize,
                    _ => start,
                };
                let old = component
                    .get::<Table>("__rich_text_ranges")
                    .or_else(|_| ctx.create_table())?;
                let new_ranges = ctx.create_table()?;
                let mut idx = 1;
                for r in old.sequence_values::<Table>() {
                    let r = r?;
                    let rs = r.get::<usize>("start").unwrap_or(0);
                    let re = r.get::<usize>("end").unwrap_or(rs);
                    if re <= start || rs >= end {
                        new_ranges.set(idx, r)?;
                        idx += 1;
                        continue;
                    }

                    if rs < start {
                        let left = ctx.create_table()?;
                        for pair in r.clone().pairs::<Value, Value>() {
                            let (key, value) = pair?;
                            left.set(key, value)?;
                        }
                        left.set("start", rs)?;
                        left.set("end", start)?;
                        new_ranges.set(idx, left)?;
                        idx += 1;
                    }
                    if re > end {
                        let right = ctx.create_table()?;
                        for pair in r.pairs::<Value, Value>() {
                            let (key, value) = pair?;
                            right.set(key, value)?;
                        }
                        right.set("start", end)?;
                        right.set("end", re)?;
                        new_ranges.set(idx, right)?;
                        idx += 1;
                    }
                }
                component.set("__rich_text_ranges", new_ranges)
            })?,
        )?;

        textbox.set(
            "update",
            lua.create_function(move |ctx, (entity, component, _dt): (Table, Table, f32)| {
                if !component.get::<bool>("visible").unwrap_or(true) {
                    return Ok(());
                }

                let request = refresh_textbox_layout_cache(ctx, &text_root, &entity, &component)?;

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

    // Interactive UI components. These share the ordinary component lifecycle,
    // so they can be composed with transforms, prefabs, and scene export.
    {
        // Panel
        // customizable UI container with borders, rounded corners, and optional 9-slice background image.
        // Defaults follow Visual Studio Code's Dark+ theme (editor sidebar/panel colours).
        {
            let frame = create_basic_drawable(lua)?;
            frame.set(
                "awake",
                lua.create_function(move |ctx, (_entity, component): (Table, Table)| {
                    component.set("color", color4(ctx, 255, 255, 255, 255)?)?;
                    component.set("visible", true)?;
                    component.set("__neolove_component", "Panel")?;
                    // VS Code Dark+: sideBar.background #252526, widget border #454545.
                    component.set("background_color", color4(ctx, 37, 37, 38, 255)?)?;
                    component.set("border_color", color4(ctx, 69, 69, 69, 255)?)?;
                    component.set("border_width", 1.0)?;
                    component.set("corner_radius", 4.0)?;
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
                    let style =
                        resolve_panel_style(ctx, &component, background_color, border_color)?;

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

            core_components.set("Panel", frame.clone())?;
            core_components.set("Frame", frame)?;
        }

        // Button
        // interactive UI button with customizable panel states and text rendering.
        // Defaults follow Visual Studio Code's Dark+ theme (button.* colours).
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
                    component.set("focused", false)?;
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
                    // VS Code Dark+: button.background #0e639c, hoverBackground #1177bb,
                    // foreground #ffffff. Buttons are borderless; keep border colours in
                    // sync with the fill so users can opt into a border by widening it.
                    component.set("background_color", color4(ctx, 14, 99, 156, 255)?)?;
                    component.set("hover_background_color", color4(ctx, 17, 119, 187, 255)?)?;
                    component.set("pressed_background_color", color4(ctx, 10, 76, 121, 255)?)?;
                    component.set("disabled_background_color", color4(ctx, 37, 37, 38, 190)?)?;
                    component.set("border_color", color4(ctx, 14, 99, 156, 255)?)?;
                    component.set("hover_border_color", color4(ctx, 17, 119, 187, 255)?)?;
                    component.set("pressed_border_color", color4(ctx, 10, 76, 121, 255)?)?;
                    component.set("disabled_border_color", color4(ctx, 37, 37, 38, 190)?)?;
                    component.set("text_color", color4(ctx, 255, 255, 255, 255)?)?;
                    component.set("hover_text_color", color4(ctx, 255, 255, 255, 255)?)?;
                    component.set("pressed_text_color", color4(ctx, 255, 255, 255, 255)?)?;
                    component.set("disabled_text_color", color4(ctx, 204, 204, 204, 120)?)?;
                    component.set("border_width", 0.0)?;
                    component.set("corner_radius", 2.0)?;
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

            install_widget_focus_methods(lua, &button, &render_state)?;
            let button_platform = platform.clone();
            let button_root = env_root.clone();
            let render_state = render_state.clone();
            button.set(
                "update",
                lua.create_function(move |ctx, (entity, component, _dt): (Table, Table, f32)| {
                    if !component.get::<bool>("visible").unwrap_or(true) {
                        unregister_widget(&render_state, &component);
                        return Ok(());
                    }

                    let draw = get_entity_draw_context(&entity)?;
                    let snapshot = current_input_snapshot(&button_platform, &render_state)?;
                    let surface = interaction_surface(&render_state);
                    let owner_key = component_owner_key(&component);
                    let enabled = component.get::<bool>("enabled").unwrap_or(true);
                    let body_region = register_widget_region(surface, &owner_key, draw, enabled);
                    let (tab, reverse_tab) = tab_navigation(&snapshot);
                    crate::widget_interaction::advance_focus(surface, tab, reverse_tab);
                    if component.get::<bool>("focused").unwrap_or(false) {
                        crate::widget_interaction::adopt_focus_if_unowned(surface, &owner_key);
                    }
                    if !enabled {
                        crate::widget_interaction::clear_focus(surface, &owner_key);
                    }
                    let hovered = enabled
                        && crate::widget_interaction::region_hovered(
                            surface,
                            &body_region,
                            [snapshot.mouse.x, snapshot.mouse.y],
                        );
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
                    let left_down = snapshot.input.mouse_down.contains("left");
                    let left_released = snapshot.input.mouse_released.contains("left");
                    let press_started = crate::widget_interaction::press_region(
                        surface,
                        &body_region,
                        [snapshot.mouse.x, snapshot.mouse.y],
                        left_pressed,
                        left_down,
                    );
                    if press_started {
                        crate::widget_interaction::request_focus(surface, &owner_key);
                    } else if left_pressed
                        && crate::widget_interaction::is_focused(surface, &owner_key)
                        && !hovered
                    {
                        crate::widget_interaction::clear_focus(surface, &owner_key);
                    }
                    let was_focused = component.get::<bool>("focused").unwrap_or(false);
                    let focused = crate::widget_interaction::is_focused(surface, &owner_key);
                    if focused != was_focused {
                        call_component_callback(
                            &component,
                            &entity,
                            if focused { "onFocus" } else { "onBlur" },
                        )?;
                    }
                    component.set("focused", focused)?;

                    let was_pressed = component.get::<bool>("pressed").unwrap_or(false);
                    let mut pressed = was_pressed;

                    if !enabled {
                        pressed = false;
                    } else {
                        if press_started {
                            pressed = true;
                            call_component_callback(&component, &entity, "onPress")?;
                        }
                        if left_released {
                            let captured = crate::widget_interaction::release_pointer(
                                surface, &owner_key, true,
                            );
                            if was_pressed || captured {
                                call_component_callback(&component, &entity, "onRelease")?;
                                if hovered {
                                    call_component_callback(&component, &entity, "onClick")?;
                                }
                            }
                            pressed = false;
                        }

                        let keyboard_activate = focused
                            && (snapshot.input.keys_pressed.contains("enter")
                                || snapshot.input.keys_pressed.contains("space")
                                || matches!(
                                    snapshot.input.last_key_pressed.as_deref(),
                                    Some("enter" | "space")
                                ));
                        if keyboard_activate {
                            call_component_callback(&component, &entity, "onPress")?;
                            call_component_callback(&component, &entity, "onRelease")?;
                            call_component_callback(&component, &entity, "onClick")?;
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

                    let style =
                        resolve_panel_style(ctx, &component, background_color, border_color)?;
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
                    component.set("locked", false)?;
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
                    component.set("antialiasing", "inherit")?;
                    component.set("__rich_text_ranges", ctx.create_table()?)?;
                    // VS Code Dark+: input.background #3c3c3c, foreground #cccccc,
                    // placeholderForeground #a6a6a6, focusBorder #007fd4, cursor #aeafad.
                    component.set("background_color", color4(ctx, 60, 60, 60, 255)?)?;
                    component.set("hover_background_color", color4(ctx, 66, 66, 66, 255)?)?;
                    component.set("focus_background_color", color4(ctx, 60, 60, 60, 255)?)?;
                    component.set("disabled_background_color", color4(ctx, 60, 60, 60, 120)?)?;
                    component.set("border_color", color4(ctx, 60, 60, 60, 255)?)?;
                    component.set("hover_border_color", color4(ctx, 98, 98, 98, 255)?)?;
                    component.set("focus_border_color", color4(ctx, 0, 127, 212, 255)?)?;
                    component.set("disabled_border_color", color4(ctx, 60, 60, 60, 120)?)?;
                    component.set("text_color", color4(ctx, 204, 204, 204, 255)?)?;
                    component.set("placeholder_color", color4(ctx, 166, 166, 166, 255)?)?;
                    component.set("disabled_text_color", color4(ctx, 204, 204, 204, 120)?)?;
                    component.set("caret_color", color4(ctx, 174, 175, 173, 255)?)?;
                    component.set("border_width", 1.0)?;
                    component.set("corner_radius", 2.0)?;
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

            // TextInput uses the same rich-text editing surface as TextBox.
            // Reusing the functions keeps formatting behavior and indexing
            // identical across display and editable text.
            let textbox: Table = core_components.get("TextBox")?;
            for method in [
                "setBold",
                "setItalic",
                "setUnderline",
                "setColor",
                "setSize",
                "setFont",
                "setOffset",
                "setPixelOffset",
                "setCharacterOffset",
                "clearFormatting",
                "clearAllFormatting",
            ] {
                text_input.set(method, textbox.get::<Value>(method)?)?;
            }
            let input_focus_render_state = render_state.clone();
            let focus_input = lua.create_function(move |_ctx, component: Table| {
                let enabled = component.get::<bool>("enabled").unwrap_or(true)
                    && !component.get::<bool>("locked").unwrap_or(false);
                let surface = interaction_surface(&input_focus_render_state);
                let owner = component_owner_key(&component);
                if enabled {
                    crate::widget_interaction::request_focus(surface, &owner);
                } else {
                    crate::widget_interaction::clear_focus(surface, &owner);
                }
                component.set("focused", enabled)
            })?;
            text_input.set("focus", focus_input.clone())?;
            text_input.set("Focus", focus_input)?;
            let input_blur_render_state = render_state.clone();
            let blur_input = lua.create_function(move |_ctx, component: Table| {
                let surface = interaction_surface(&input_blur_render_state);
                let owner = component_owner_key(&component);
                crate::widget_interaction::clear_focus(surface, &owner);
                component.set("focused", false)
            })?;
            text_input.set("blur", blur_input.clone())?;
            text_input.set("Blur", blur_input)?;

            let input_platform = platform.clone();
            let text_root = env_root.clone();
            let render_state = render_state.clone();
            text_input.set(
                "update",
                lua.create_function(move |ctx, (entity, component, dt): (Table, Table, f32)| {
                    if !component.get::<bool>("visible").unwrap_or(true) {
                        unregister_widget(&render_state, &component);
                        return Ok(());
                    }

                    let draw = get_entity_draw_context(&entity)?;
                    let snapshot = current_input_snapshot(&input_platform, &render_state)?;
                    let surface = interaction_surface(&render_state);
                    let owner_key = component_owner_key(&component);
                    let enabled = component.get::<bool>("enabled").unwrap_or(true)
                        && !component.get::<bool>("locked").unwrap_or(false);
                    let was_focused = component.get::<bool>("focused").unwrap_or(false);
                    let body_region = register_widget_region(surface, &owner_key, draw, enabled);
                    if was_focused {
                        crate::widget_interaction::adopt_focus_if_unowned(surface, &owner_key);
                    }
                    if !enabled {
                        crate::widget_interaction::clear_focus(surface, &owner_key);
                    }
                    let (tab, reverse_tab) = tab_navigation(&snapshot);
                    crate::widget_interaction::advance_focus(surface, tab, reverse_tab);
                    let hovered = enabled
                        && crate::widget_interaction::region_hovered(
                            surface,
                            &body_region,
                            [snapshot.mouse.x, snapshot.mouse.y],
                        );
                    let was_hovered = component.get::<bool>("hovered").unwrap_or(false);
                    if hovered != was_hovered {
                        component.set("hovered", hovered)?;
                    }

                    let left_pressed = snapshot.input.mouse_pressed.contains("left");
                    let left_down = snapshot.input.mouse_down.contains("left");
                    let left_released = snapshot.input.mouse_released.contains("left");
                    let press_started = crate::widget_interaction::press_region(
                        surface,
                        &body_region,
                        [snapshot.mouse.x, snapshot.mouse.y],
                        left_pressed,
                        left_down,
                    );
                    if press_started {
                        crate::widget_interaction::request_focus(surface, &owner_key);
                    } else if left_pressed
                        && crate::widget_interaction::is_focused(surface, &owner_key)
                    {
                        crate::widget_interaction::clear_focus(surface, &owner_key);
                    }
                    if left_released {
                        crate::widget_interaction::release_pointer(surface, &owner_key, true);
                    }
                    let mut focused = crate::widget_interaction::is_focused(surface, &owner_key);
                    if focused != was_focused {
                        call_component_callback(
                            &component,
                            &entity,
                            if focused { "onFocus" } else { "onBlur" },
                        )?;
                    }

                    let mut text = component.get::<String>("text").unwrap_or_default();
                    let mut cursor = component
                        .get::<usize>("cursor_index")
                        .unwrap_or_else(|_| char_count(&text))
                        .min(char_count(&text));
                    let mut changed = false;

                    // Place the caret at the closest character when focus was
                    // acquired by clicking, including on rotated inputs.
                    if left_pressed && hovered {
                        let local = world_point_to_local(snapshot.mouse, draw.pivot, draw.rotation);
                        let border = component.get::<f32>("border_width").unwrap_or(1.0).max(0.0);
                        let padding = component.get::<f32>("padding").unwrap_or(8.0).max(0.0);
                        let padding_x = component
                            .get::<f32>("padding_x")
                            .unwrap_or(padding)
                            .max(0.0);
                        let target_x = (local.x - draw.bounds.x - border - padding_x).max(0.0);
                        let display = if component.get::<bool>("password").unwrap_or(false) {
                            "*".repeat(char_count(&text))
                        } else {
                            text.clone()
                        };
                        let view_start = component.get::<usize>("view_start").unwrap_or(0);
                        cursor = view_start.min(char_count(&display));
                        for index in (cursor + 1)..=char_count(&display) {
                            let previous = measure_inline_text(
                                &text_root,
                                &component,
                                &slice_chars(&display, view_start, index - 1),
                                None,
                            );
                            let current = measure_inline_text(
                                &text_root,
                                &component,
                                &slice_chars(&display, view_start, index),
                                None,
                            );
                            if target_x < (previous + current) * 0.5 {
                                break;
                            }
                            cursor = index;
                        }
                    }

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
                                    crate::widget_interaction::clear_focus(surface, &owner_key);
                                    call_component_callback(&component, &entity, "onBlur")?;
                                }
                                "enter"
                                    if component.get::<bool>("submit_on_enter").unwrap_or(true) =>
                                {
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
                                        crate::widget_interaction::clear_focus(surface, &owner_key);
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
                    let caret_color =
                        get_color_field(&component, "caret_color").unwrap_or(text_color);
                    let style =
                        resolve_panel_style(ctx, &component, background_color, border_color)?;
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
                        let placeholder =
                            component.get::<String>("placeholder").unwrap_or_default();
                        if !placeholder.is_empty() {
                            let mut request = build_text_request(
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
                            );
                            request.rich_text.clear();
                            renderer.queue(DrawCommand::Text(request));
                        }
                    } else {
                        let mut request = build_text_request(
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
                        );
                        request.rich_text = request
                            .rich_text
                            .into_iter()
                            .filter_map(|mut range| {
                                let start = range.start.max(view_start);
                                let end = range.end.min(visible_end);
                                if end <= start {
                                    return None;
                                }
                                range.start = start - view_start;
                                range.end = end - view_start;
                                Some(range)
                            })
                            .collect();
                        renderer.queue(DrawCommand::Text(request));
                    }

                    if focused && ((blink * 1.6).floor() as i32 % 2 == 0) {
                        let caret_prefix = slice_chars(&display_text, view_start, cursor);
                        let caret_offset =
                            measure_inline_text(&text_root, &component, &caret_prefix, None);
                        let visible_width =
                            measure_inline_text(&text_root, &component, &visible_text, None);
                        let caret_origin = match get_string_field(&component, "align_x", "alignX")
                            .as_deref()
                            .map(parse_align_x)
                            .unwrap_or(TextAlignX::Left)
                        {
                            TextAlignX::Center => {
                                text_bounds.x + (text_bounds.w - visible_width).max(0.0) * 0.5
                            }
                            TextAlignX::Right => {
                                text_bounds.x + (text_bounds.w - visible_width).max(0.0)
                            }
                            TextAlignX::Left => text_bounds.x,
                        };
                        let caret_width =
                            component.get::<f32>("caret_width").unwrap_or(2.0).max(1.0);
                        let caret_bounds = Rect {
                            x: caret_origin + caret_offset,
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
        // selectable list with customizable closed/open state styling.
        // Defaults follow Visual Studio Code's Dark+ theme (dropdown.* / list.* colours).
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
                    component.set("focused", false)?;
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
                    // VS Code Dark+: dropdown.background #3c3c3c, foreground #f0f0f0,
                    // border #454545, focusBorder #007fd4. Menu uses editorWidget.background
                    // #252526; items use list.hoverBackground #2a2d2e and
                    // list.activeSelectionBackground #094771.
                    component.set("background_color", color4(ctx, 60, 60, 60, 255)?)?;
                    component.set("hover_background_color", color4(ctx, 74, 74, 74, 255)?)?;
                    component.set("open_background_color", color4(ctx, 60, 60, 60, 255)?)?;
                    component.set("disabled_background_color", color4(ctx, 60, 60, 60, 120)?)?;
                    component.set("border_color", color4(ctx, 69, 69, 69, 255)?)?;
                    component.set("hover_border_color", color4(ctx, 98, 98, 98, 255)?)?;
                    component.set("open_border_color", color4(ctx, 0, 127, 212, 255)?)?;
                    component.set("disabled_border_color", color4(ctx, 69, 69, 69, 120)?)?;
                    component.set("text_color", color4(ctx, 240, 240, 240, 255)?)?;
                    component.set("disabled_text_color", color4(ctx, 204, 204, 204, 120)?)?;
                    component.set("menu_background_color", color4(ctx, 37, 37, 38, 255)?)?;
                    component.set("menu_border_color", color4(ctx, 69, 69, 69, 255)?)?;
                    component.set("item_background_color", color4(ctx, 37, 37, 38, 0)?)?;
                    component.set("item_hover_background_color", color4(ctx, 42, 45, 46, 255)?)?;
                    component.set(
                        "item_selected_background_color",
                        color4(ctx, 9, 71, 113, 255)?,
                    )?;
                    component.set("item_text_color", color4(ctx, 204, 204, 204, 255)?)?;
                    component.set("item_hover_text_color", color4(ctx, 255, 255, 255, 255)?)?;
                    component.set("item_selected_text_color", color4(ctx, 255, 255, 255, 255)?)?;
                    component.set("border_width", 1.0)?;
                    component.set("corner_radius", 2.0)?;
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

            install_widget_focus_methods(lua, &dropdown, &render_state)?;
            let dropdown_platform = platform.clone();
            let dropdown_root = env_root.clone();
            let render_state = render_state.clone();
            dropdown.set(
                "update",
                lua.create_function(move |ctx, (entity, component, _dt): (Table, Table, f32)| {
                    if !component.get::<bool>("visible").unwrap_or(true) {
                        unregister_widget(&render_state, &component);
                        return Ok(());
                    }

                    let draw = get_entity_draw_context(&entity)?;
                    let snapshot = current_input_snapshot(&dropdown_platform, &render_state)?;
                    let surface = interaction_surface(&render_state);
                    let owner_key = component_owner_key(&component);
                    let enabled = component.get::<bool>("enabled").unwrap_or(true);
                    let was_focused = component.get::<bool>("focused").unwrap_or(false);
                    let body_region = register_widget_region(surface, &owner_key, draw, enabled);
                    if was_focused {
                        crate::widget_interaction::adopt_focus_if_unowned(surface, &owner_key);
                    }
                    if !enabled {
                        crate::widget_interaction::clear_focus(surface, &owner_key);
                    }
                    let (tab, reverse_tab) = tab_navigation(&snapshot);
                    crate::widget_interaction::advance_focus(surface, tab, reverse_tab);
                    let items = read_ui_list_items(
                        component.get::<Option<Table>>("options").ok().flatten(),
                    )?;
                    let option_count = items.len();
                    let mut selected_index = component.get::<usize>("selected_index").unwrap_or(0);
                    if option_count == 0 {
                        selected_index = 0;
                    } else {
                        selected_index = selected_index.clamp(1, option_count);
                    }
                    let mut open = component.get::<bool>("open").unwrap_or(false) && enabled;
                    let hovered = enabled
                        && crate::widget_interaction::region_hovered(
                            surface,
                            &body_region,
                            [snapshot.mouse.x, snapshot.mouse.y],
                        );

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
                    let popup_region = widget_popup_region_id(&owner_key);
                    if open && visible_count > 0 {
                        register_popup(surface, &owner_key, menu_bounds, draw.pivot, draw.rotation);
                    } else {
                        crate::widget_interaction::unregister_region(surface, &popup_region);
                    }

                    let menu_hovered = open
                        && visible_count > 0
                        && crate::widget_interaction::region_hovered(
                            surface,
                            &popup_region,
                            [snapshot.mouse.x, snapshot.mouse.y],
                        );

                    let mut hovered_index = 0usize;
                    if menu_hovered {
                        for visible_index in 0..visible_count {
                            let item_bounds = Rect {
                                x: menu_bounds.x,
                                y: menu_bounds.y + visible_index as f32 * item_height,
                                w: menu_bounds.w,
                                h: item_height,
                            };
                            if point_in_bounds(
                                snapshot.mouse,
                                item_bounds,
                                draw.pivot,
                                draw.rotation,
                            ) {
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

                    let left_pressed = snapshot.input.mouse_pressed.contains("left");
                    let left_down = snapshot.input.mouse_down.contains("left");
                    let left_released = snapshot.input.mouse_released.contains("left");
                    let body_pressed = crate::widget_interaction::press_region(
                        surface,
                        &body_region,
                        [snapshot.mouse.x, snapshot.mouse.y],
                        left_pressed,
                        left_down,
                    );
                    let menu_pressed = crate::widget_interaction::press_region(
                        surface,
                        &popup_region,
                        [snapshot.mouse.x, snapshot.mouse.y],
                        left_pressed,
                        left_down,
                    );
                    if left_pressed {
                        if body_pressed {
                            crate::widget_interaction::request_focus(surface, &owner_key);
                            open = !open;
                        } else if open && hovered_index > 0 && menu_pressed {
                            crate::widget_interaction::request_focus(surface, &owner_key);
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
                        } else if crate::widget_interaction::is_focused(surface, &owner_key) {
                            crate::widget_interaction::clear_focus(surface, &owner_key);
                        }
                    }

                    if left_released {
                        crate::widget_interaction::release_pointer(surface, &owner_key, true);
                    }

                    let focused = crate::widget_interaction::is_focused(surface, &owner_key);
                    if focused && enabled {
                        let key = snapshot.input.last_key_pressed.as_deref();
                        match key {
                            Some("escape") => open = false,
                            Some("enter" | "space") => open = !open,
                            Some("up" | "left") if option_count > 0 => {
                                selected_index = if selected_index <= 1 {
                                    option_count
                                } else {
                                    selected_index - 1
                                };
                                if let Some(item) = items.get(selected_index - 1) {
                                    call_component_selection_callback(
                                        &component,
                                        &entity,
                                        "onChanged",
                                        selected_index,
                                        &item.value,
                                    )?;
                                }
                            }
                            Some("down" | "right") if option_count > 0 => {
                                selected_index =
                                    if selected_index == 0 || selected_index >= option_count {
                                        1
                                    } else {
                                        selected_index + 1
                                    };
                                if let Some(item) = items.get(selected_index - 1) {
                                    call_component_selection_callback(
                                        &component,
                                        &entity,
                                        "onChanged",
                                        selected_index,
                                        &item.value,
                                    )?;
                                }
                            }
                            Some("home") if option_count > 0 => selected_index = 1,
                            Some("end") if option_count > 0 => selected_index = option_count,
                            _ => {}
                        }
                    }
                    if !open {
                        crate::widget_interaction::unregister_region(surface, &popup_region);
                    }
                    if focused != was_focused {
                        call_component_callback(
                            &component,
                            &entity,
                            if focused { "onFocus" } else { "onBlur" },
                        )?;
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
                    component.set("focused", focused)?;
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
                    let style =
                        resolve_panel_style(ctx, &component, background_color, border_color)?;
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
                        x: draw.bounds.x + draw.bounds.w
                            - style.border_width
                            - padding_x
                            - arrow_width,
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
                        let menu_border = get_color_field(&component, "menu_border_color")
                            .unwrap_or(border_color);
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

        // Slider
        // draggable value slider with a track, filled range, and thumb.
        // Defaults follow Visual Studio Code's Dark+ theme (input background track,
        // #007acc filled range, #cccccc thumb). Hover colours are fully configurable.
        {
            let slider = create_basic_drawable(lua)?;
            slider.set(
                "awake",
                lua.create_function(move |ctx, (_entity, component): (Table, Table)| {
                    component.set("color", color4(ctx, 255, 255, 255, 255)?)?;
                    component.set("visible", true)?;
                    component.set("__neolove_component", "Slider")?;
                    component.set("enabled", true)?;
                    component.set("hovered", false)?;
                    component.set("focused", false)?;
                    component.set("dragging", false)?;
                    component.set("min", 0.0)?;
                    component.set("max", 100.0)?;
                    component.set("value", 0.0)?;
                    component.set("fraction", 0.0)?;
                    // 0 = continuous; otherwise the value snaps to multiples of `step`.
                    component.set("step", 0.0)?;
                    component.set("orientation", "horizontal")?;
                    component.set("track_thickness", 6.0)?;
                    component.set("thumb_size", 16.0)?;
                    component.set("thumb_corner_radius", 8.0)?;
                    component.set("corner_radius", 3.0)?;
                    component.set("border_width", 0.0)?;
                    // VS Code Dark+ derived palette.
                    component.set("background_color", color4(ctx, 60, 60, 60, 255)?)?;
                    component.set("hover_background_color", color4(ctx, 66, 66, 66, 255)?)?;
                    component.set("disabled_background_color", color4(ctx, 60, 60, 60, 120)?)?;
                    component.set("border_color", color4(ctx, 60, 60, 60, 255)?)?;
                    component.set("hover_border_color", color4(ctx, 98, 98, 98, 255)?)?;
                    component.set("disabled_border_color", color4(ctx, 60, 60, 60, 120)?)?;
                    component.set("fill_color", color4(ctx, 0, 122, 204, 255)?)?;
                    component.set("hover_fill_color", color4(ctx, 17, 119, 187, 255)?)?;
                    component.set("disabled_fill_color", color4(ctx, 60, 60, 60, 180)?)?;
                    component.set("thumb_color", color4(ctx, 204, 204, 204, 255)?)?;
                    component.set("hover_thumb_color", color4(ctx, 255, 255, 255, 255)?)?;
                    component.set("disabled_thumb_color", color4(ctx, 128, 128, 128, 255)?)?;
                    component.set("background_image", Value::Nil)?;
                    component.set("slice_left", 0.0)?;
                    component.set("slice_right", 0.0)?;
                    component.set("slice_top", 0.0)?;
                    component.set("slice_bottom", 0.0)?;
                    Ok(())
                })?,
            )?;

            install_widget_focus_methods(lua, &slider, &render_state)?;
            let set_value = lua.create_function(|_ctx, (component, value): (Table, f32)| {
                let min = component.get::<f32>("min").unwrap_or(0.0);
                let max = component.get::<f32>("max").unwrap_or(100.0);
                let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
                let clamped = value.clamp(lo, hi);
                component.set("value", clamped)?;
                let range = hi - lo;
                let fraction = if range.abs() > f32::EPSILON {
                    ((clamped - min) / (max - min)).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                component.set("fraction", fraction)?;
                Ok(())
            })?;
            slider.set("setValue", set_value.clone())?;
            slider.set("SetValue", set_value)?;

            let slider_platform = platform.clone();
            let render_state = render_state.clone();
            slider.set(
                "update",
                lua.create_function(move |ctx, (entity, component, _dt): (Table, Table, f32)| {
                    if !component.get::<bool>("visible").unwrap_or(true) {
                        unregister_widget(&render_state, &component);
                        return Ok(());
                    }

                    let draw = get_entity_draw_context(&entity)?;
                    let snapshot = current_input_snapshot(&slider_platform, &render_state)?;
                    let surface = interaction_surface(&render_state);
                    let owner_key = component_owner_key(&component);
                    let enabled = component.get::<bool>("enabled").unwrap_or(true);
                    let was_focused = component.get::<bool>("focused").unwrap_or(false);
                    let body_region = register_widget_region(surface, &owner_key, draw, enabled);
                    if was_focused {
                        crate::widget_interaction::adopt_focus_if_unowned(surface, &owner_key);
                    }
                    if !enabled {
                        crate::widget_interaction::clear_focus(surface, &owner_key);
                    }
                    let (tab, reverse_tab) = tab_navigation(&snapshot);
                    crate::widget_interaction::advance_focus(surface, tab, reverse_tab);

                    let min = component.get::<f32>("min").unwrap_or(0.0);
                    let max = component.get::<f32>("max").unwrap_or(100.0);
                    let range = max - min;
                    let step = component.get::<f32>("step").unwrap_or(0.0).max(0.0);
                    let mut value = component.get::<f32>("value").unwrap_or(min);

                    let vertical = get_string_field(&component, "orientation", "orientation")
                        .map(|value| value.eq_ignore_ascii_case("vertical"))
                        .unwrap_or(false);
                    let thumb_size = component.get::<f32>("thumb_size").unwrap_or(16.0).max(0.0);
                    let half_thumb = thumb_size * 0.5;

                    let hovered = enabled
                        && crate::widget_interaction::region_hovered(
                            surface,
                            &body_region,
                            [snapshot.mouse.x, snapshot.mouse.y],
                        );

                    let left_pressed = snapshot.input.mouse_pressed.contains("left");
                    let left_down = snapshot.input.mouse_down.contains("left");
                    let left_released = snapshot.input.mouse_released.contains("left");
                    let press_started = crate::widget_interaction::press_region(
                        surface,
                        &body_region,
                        [snapshot.mouse.x, snapshot.mouse.y],
                        left_pressed,
                        left_down,
                    );
                    if press_started {
                        crate::widget_interaction::request_focus(surface, &owner_key);
                    } else if left_pressed
                        && crate::widget_interaction::is_focused(surface, &owner_key)
                        && !hovered
                    {
                        crate::widget_interaction::clear_focus(surface, &owner_key);
                    }
                    let mut dragging = enabled
                        && left_down
                        && crate::widget_interaction::pointer_owned(surface, &owner_key);

                    // Usable travel distance for the thumb centre.
                    let track_len = ((if vertical {
                        draw.bounds.h
                    } else {
                        draw.bounds.w
                    }) - thumb_size)
                        .max(0.0);

                    let mut changed = false;
                    if dragging && enabled {
                        let local = world_point_to_local(snapshot.mouse, draw.pivot, draw.rotation);
                        let fraction = if track_len <= 0.0 {
                            0.0
                        } else if vertical {
                            // Top of the widget is `max`, bottom is `min`.
                            let pos = (draw.bounds.y + draw.bounds.h - half_thumb) - local.y;
                            (pos / track_len).clamp(0.0, 1.0)
                        } else {
                            let pos = local.x - draw.bounds.x - half_thumb;
                            (pos / track_len).clamp(0.0, 1.0)
                        };
                        let mut new_value = min + fraction * range;
                        if step > 0.0 {
                            new_value = min + (((new_value - min) / step).round()) * step;
                        }
                        let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
                        new_value = new_value.clamp(lo, hi);
                        if (new_value - value).abs() > f32::EPSILON {
                            value = new_value;
                            changed = true;
                        }
                    }

                    let focused = crate::widget_interaction::is_focused(surface, &owner_key);
                    if focused && enabled && !dragging {
                        let keyboard_step = if step > 0.0 { step } else { range.abs() * 0.01 };
                        let keyboard_step = if keyboard_step > f32::EPSILON {
                            keyboard_step
                        } else {
                            1.0
                        };
                        let keyboard_value = match snapshot.input.last_key_pressed.as_deref() {
                            Some("home") => Some(min),
                            Some("end") => Some(max),
                            Some("left") if !vertical => Some(value - keyboard_step),
                            Some("right") if !vertical => Some(value + keyboard_step),
                            Some("down") if vertical => Some(value - keyboard_step),
                            Some("up") if vertical => Some(value + keyboard_step),
                            Some("pagedown") => Some(value - keyboard_step * 10.0),
                            Some("pageup") => Some(value + keyboard_step * 10.0),
                            _ => None,
                        };
                        if let Some(mut new_value) = keyboard_value {
                            if step > 0.0 {
                                new_value = min + (((new_value - min) / step).round()) * step;
                            }
                            let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
                            new_value = new_value.clamp(lo, hi);
                            if (new_value - value).abs() > f32::EPSILON {
                                value = new_value;
                                changed = true;
                            }
                        }
                    }

                    if left_released {
                        crate::widget_interaction::release_pointer(surface, &owner_key, true);
                        dragging = false;
                    }
                    if focused != was_focused {
                        call_component_callback(
                            &component,
                            &entity,
                            if focused { "onFocus" } else { "onBlur" },
                        )?;
                    }

                    let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
                    value = value.clamp(lo, hi);
                    let fraction = if range.abs() > f32::EPSILON {
                        ((value - min) / range).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };

                    component.set("hovered", hovered)?;
                    component.set("focused", focused)?;
                    component.set("dragging", dragging)?;
                    component.set("value", value)?;
                    component.set("fraction", fraction)?;
                    if changed {
                        call_component_number_callback(&component, &entity, "onChanged", value)?;
                    }

                    let active = hovered || dragging;
                    let track_color = if !enabled {
                        get_color_field(&component, "disabled_background_color")
                    } else if active {
                        get_color_field(&component, "hover_background_color")
                    } else {
                        get_color_field(&component, "background_color")
                    }
                    .unwrap_or(Color::rgba(60, 60, 60, 255));
                    let border_color = if !enabled {
                        get_color_field(&component, "disabled_border_color")
                    } else if active {
                        get_color_field(&component, "hover_border_color")
                    } else {
                        get_color_field(&component, "border_color")
                    }
                    .unwrap_or(track_color);
                    let fill_color = if !enabled {
                        get_color_field(&component, "disabled_fill_color")
                    } else if active {
                        get_color_field(&component, "hover_fill_color")
                    } else {
                        get_color_field(&component, "fill_color")
                    }
                    .unwrap_or(Color::rgba(0, 122, 204, 255));
                    let thumb_color = if !enabled {
                        get_color_field(&component, "disabled_thumb_color")
                    } else if active {
                        get_color_field(&component, "hover_thumb_color")
                    } else {
                        get_color_field(&component, "thumb_color")
                    }
                    .unwrap_or(Color::rgba(204, 204, 204, 255));

                    let track_thickness = component
                        .get::<f32>("track_thickness")
                        .unwrap_or(6.0)
                        .max(0.0);
                    let corner_radius = component
                        .get::<f32>("corner_radius")
                        .unwrap_or(3.0)
                        .max(0.0);
                    let thumb_corner_radius = component
                        .get::<f32>("thumb_corner_radius")
                        .unwrap_or(thumb_size * 0.5)
                        .max(0.0);

                    // Track rectangle: a thin bar centred along the cross axis.
                    let track_bounds = if vertical {
                        let thickness = if track_thickness > 0.0 {
                            track_thickness.min(draw.bounds.w)
                        } else {
                            draw.bounds.w
                        };
                        Rect {
                            x: draw.bounds.x + (draw.bounds.w - thickness) * 0.5,
                            y: draw.bounds.y,
                            w: thickness,
                            h: draw.bounds.h,
                        }
                    } else {
                        let thickness = if track_thickness > 0.0 {
                            track_thickness.min(draw.bounds.h)
                        } else {
                            draw.bounds.h
                        };
                        Rect {
                            x: draw.bounds.x,
                            y: draw.bounds.y + (draw.bounds.h - thickness) * 0.5,
                            w: draw.bounds.w,
                            h: thickness,
                        }
                    };

                    let style = resolve_panel_style(ctx, &component, track_color, border_color)?;

                    // Thumb centre in local (unrotated) coordinates.
                    let thumb_center_x;
                    let thumb_center_y;
                    let fill_bounds;
                    if vertical {
                        thumb_center_x = track_bounds.x + track_bounds.w * 0.5;
                        thumb_center_y =
                            draw.bounds.y + draw.bounds.h - half_thumb - fraction * track_len;
                        let fill_top = thumb_center_y;
                        fill_bounds = Rect {
                            x: track_bounds.x,
                            y: fill_top,
                            w: track_bounds.w,
                            h: (track_bounds.y + track_bounds.h - fill_top).max(0.0),
                        };
                    } else {
                        thumb_center_y = track_bounds.y + track_bounds.h * 0.5;
                        thumb_center_x = draw.bounds.x + half_thumb + fraction * track_len;
                        fill_bounds = Rect {
                            x: track_bounds.x,
                            y: track_bounds.y,
                            w: (thumb_center_x - track_bounds.x).max(0.0),
                            h: track_bounds.h,
                        };
                    }
                    let thumb_bounds = Rect {
                        x: thumb_center_x - half_thumb,
                        y: thumb_center_y - half_thumb,
                        w: thumb_size,
                        h: thumb_size,
                    };

                    let mut renderer = render_state
                        .lock()
                        .map_err(|_| mlua::Error::external("render state lock poisoned"))?;
                    render_panel(
                        &mut renderer,
                        track_bounds,
                        draw.pivot,
                        draw.rotation,
                        &style,
                    )?;
                    if fill_bounds.w > 0.0 && fill_bounds.h > 0.0 && fill_color.a > 0 {
                        queue_rounded_rect_fill(
                            &mut renderer,
                            fill_bounds,
                            draw.pivot,
                            draw.rotation,
                            fill_color,
                            corner_radius,
                        );
                    }
                    if thumb_size > 0.0 && thumb_color.a > 0 {
                        queue_rounded_rect_fill(
                            &mut renderer,
                            thumb_bounds,
                            draw.pivot,
                            draw.rotation,
                            thumb_color,
                            thumb_corner_radius,
                        );
                    }
                    Ok(())
                })?,
            )?;

            core_components.set("Slider", slider)?;
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

            install_widget_focus_methods(lua, &scroll_list, &render_state)?;
            let scroll_list_platform = platform.clone();
            let scroll_list_root = env_root.clone();
            let render_state = render_state.clone();
            scroll_list.set(
                "update",
                lua.create_function(move |ctx, (entity, component, _dt): (Table, Table, f32)| {
                    if !component.get::<bool>("visible").unwrap_or(true) {
                        unregister_widget(&render_state, &component);
                        return Ok(());
                    }

                    let draw = get_entity_draw_context(&entity)?;
                    let snapshot = current_input_snapshot(&scroll_list_platform, &render_state)?;
                    let surface = interaction_surface(&render_state);
                    let owner_key = component_owner_key(&component);
                    let enabled = component.get::<bool>("enabled").unwrap_or(true);
                    let was_focused = component.get::<bool>("focused").unwrap_or(false);
                    let body_region = register_widget_region(surface, &owner_key, draw, enabled);
                    if was_focused {
                        crate::widget_interaction::adopt_focus_if_unowned(surface, &owner_key);
                    }
                    if !enabled {
                        crate::widget_interaction::clear_focus(surface, &owner_key);
                    }
                    let (tab, reverse_tab) = tab_navigation(&snapshot);
                    crate::widget_interaction::advance_focus(surface, tab, reverse_tab);
                    let hovered = enabled
                        && crate::widget_interaction::region_hovered(
                            surface,
                            &body_region,
                            [snapshot.mouse.x, snapshot.mouse.y],
                        );
                    let left_pressed = snapshot.input.mouse_pressed.contains("left");
                    let left_down = snapshot.input.mouse_down.contains("left");
                    let left_released = snapshot.input.mouse_released.contains("left");
                    let press_started = crate::widget_interaction::press_region(
                        surface,
                        &body_region,
                        [snapshot.mouse.x, snapshot.mouse.y],
                        left_pressed,
                        left_down,
                    );
                    if press_started {
                        crate::widget_interaction::request_focus(surface, &owner_key);
                    } else if left_pressed
                        && crate::widget_interaction::is_focused(surface, &owner_key)
                        && !hovered
                    {
                        crate::widget_interaction::clear_focus(surface, &owner_key);
                    }
                    let focused = crate::widget_interaction::is_focused(surface, &owner_key);
                    let focus_changed = focused != was_focused;

                    let items = read_ui_list_items(
                        component.get::<Option<Table>>("options").ok().flatten(),
                    )?;
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
                    let scrollbar_thumb_color =
                        get_color_field(&component, "scrollbar_thumb_color")
                            .unwrap_or(Color::rgba(176, 214, 255, 235));
                    let style =
                        resolve_panel_style(ctx, &component, background_color, border_color)?;
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
                    let local_mouse =
                        world_point_to_local(snapshot.mouse, draw.pivot, draw.rotation);
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
                            hovered
                                && point_in_bounds(
                                    snapshot.mouse,
                                    track_bounds,
                                    draw.pivot,
                                    draw.rotation,
                                )
                        })
                        .unwrap_or(false);
                    let thumb_hovered = thumb_bounds
                        .map(|thumb_bounds| {
                            hovered
                                && point_in_bounds(
                                    snapshot.mouse,
                                    thumb_bounds,
                                    draw.pivot,
                                    draw.rotation,
                                )
                        })
                        .unwrap_or(false);
                    let mut scrollbar_dragging =
                        component.get::<bool>("scrollbar_dragging").unwrap_or(false)
                            && enabled
                            && crate::widget_interaction::pointer_owned(surface, &owner_key);
                    let mut scrollbar_drag_offset = component
                        .get::<f32>("scrollbar_drag_offset")
                        .unwrap_or(0.0)
                        .max(0.0);
                    if !left_down {
                        scrollbar_dragging = false;
                    }

                    if enabled && press_started {
                        if let Some(thumb_bounds) = thumb_bounds {
                            if thumb_hovered {
                                scrollbar_dragging = true;
                                scrollbar_drag_offset = (local_mouse.y - thumb_bounds.y)
                                    .clamp(0.0, thumb_bounds.h.max(0.0));
                            } else if track_hovered {
                                scrollbar_dragging = true;
                                scrollbar_drag_offset = thumb_bounds.h * 0.5;
                            }
                        }
                    }

                    if overflow && scrollbar_dragging && left_down {
                        if let (Some(track_bounds), Some(thumb_bounds)) =
                            (track_bounds, thumb_bounds)
                        {
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
                                (((thumb_top - track_bounds.y) / available) * max_scroll as f32)
                                    .round() as usize
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
                                h: item_height
                                    .min((list_bounds.y + list_bounds.h - item_y).max(0.0)),
                            };
                            if item_bounds.h <= 0.0 {
                                continue;
                            }
                            if hovered
                                && point_in_bounds(
                                    snapshot.mouse,
                                    item_bounds,
                                    draw.pivot,
                                    draw.rotation,
                                )
                            {
                                hovered_index = scroll_index + visible_index + 1;
                                break;
                            }
                        }
                    }

                    if enabled
                        && press_started
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

                    if left_released {
                        crate::widget_interaction::release_pointer(surface, &owner_key, true);
                        component.set("scrollbar_dragging", false)?;
                    }

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
        image2d.set(
            "awake",
            lua.create_function(move |ctx, (_entity, component): (Table, Table)| {
                component.set("__neolove_component", "Image2D")?;
                component.set("color", color4(ctx, 255, 255, 255, 255)?)?;
                component.set("visible", true)?;
                component.set("shader", Value::Nil)?;
                component.set("image", Value::Nil)?;
                Ok(())
            })?,
        )?;
        let sprite2d = create_basic_drawable(lua)?;
        sprite2d.set(
            "awake",
            lua.create_function(move |ctx, (_entity, component): (Table, Table)| {
                component.set("__neolove_component", "Sprite2D")?;
                component.set("color", color4(ctx, 255, 255, 255, 255)?)?;
                component.set("visible", true)?;
                component.set("shader", Value::Nil)?;
                component.set("image", Value::Nil)?;
                Ok(())
            })?,
        )?;
        let render_state = render_state.clone();

        let image_update =
            lua.create_function(move |ctx, (entity, component, _dt): (Table, Table, f32)| {
                if !component.get::<bool>("visible").unwrap_or(true) {
                    return Ok(());
                }
                let (x, y, rotation) = crate::window::get_global_transform(&entity)?;
                let (w, h) = crate::window::get_global_size(&entity)?;
                let use_middle_pivot = crate::window::uses_middle_pivot(&entity);

                let tint: Color = color4_to_color(component.get("color")?)?;
                let shader = shader_from_component(&component)?;
                let image: Option<AnyUserData> = component.get("image")?;
                let Some(image) = image else {
                    return Ok(());
                };

                let image = image.borrow::<crate::assets::ImageHandle>()?;
                image.ensure_uploaded()?;
                let (image_w, image_h) = image.dimensions()?;
                let image_bounds = Rect {
                    x: 0.0,
                    y: 0.0,
                    w: image_w as f32,
                    h: image_h as f32,
                };
                let source = get_source_rect(&component, "source")
                    .map(|source| clamp_rect_to_bounds(source, image_bounds))
                    .filter(|source| source.w > 0.0 && source.h > 0.0);
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
                    source,
                    rotation,
                    pivot,
                    tint,
                    filter: app_texture_filter(ctx),
                    shader,
                });

                Ok(())
            })?;

        image2d.set("update", image_update.clone())?;
        sprite2d.set("update", image_update)?;

        core_components.set("Image2D", image2d)?;
        core_components.set("Sprite2D", sprite2d)?;
    }

    // SpriteSheet2D: frame-based atlas animation without requiring gameplay
    // code to calculate source rectangles every frame.
    {
        let sprite_sheet = create_basic_drawable(lua)?;
        sprite_sheet.set(
            "awake",
            lua.create_function(move |ctx, (_entity, component): (Table, Table)| {
                component.set("__neolove_component", "SpriteSheet2D")?;
                component.set("color", color4(ctx, 255, 255, 255, 255)?)?;
                component.set("visible", true)?;
                component.set("shader", Value::Nil)?;
                component.set("image", Value::Nil)?;
                component.set("frame_width", 32.0)?;
                component.set("frame_height", 32.0)?;
                component.set("columns", 0)?;
                component.set("frame_count", 0)?;
                component.set("spacing", 0.0)?;
                component.set("margin", 0.0)?;
                component.set("frame", 0)?;
                component.set("fps", 12.0)?;
                component.set("playing", true)?;
                component.set("looping", true)?;
                component.set("__frame_time", 0.0)?;
                Ok(())
            })?,
        )?;

        let play = lua.create_function(|_ctx, component: Table| component.set("playing", true))?;
        sprite_sheet.set("play", play.clone())?;
        sprite_sheet.set("Play", play)?;
        let pause =
            lua.create_function(|_ctx, component: Table| component.set("playing", false))?;
        sprite_sheet.set("pause", pause.clone())?;
        sprite_sheet.set("Pause", pause)?;
        let stop = lua.create_function(|_ctx, component: Table| {
            component.set("playing", false)?;
            component.set("frame", 0)?;
            component.set("__frame_time", 0.0)
        })?;
        sprite_sheet.set("stop", stop.clone())?;
        sprite_sheet.set("Stop", stop)?;
        let set_frame = lua.create_function(|_ctx, (component, frame): (Table, i64)| {
            component.set("frame", frame.max(0))?;
            component.set("__frame_time", 0.0)
        })?;
        sprite_sheet.set("setFrame", set_frame.clone())?;
        sprite_sheet.set("set_frame", set_frame)?;

        let sprite_sheet_render_state = render_state.clone();
        sprite_sheet.set(
            "update",
            lua.create_function(move |ctx, (entity, component, dt): (Table, Table, f32)| {
                if !component.get::<bool>("visible").unwrap_or(true) {
                    return Ok(());
                }
                let Some(image) = component.get::<Option<AnyUserData>>("image")? else {
                    return Ok(());
                };
                let image = image.borrow::<crate::assets::ImageHandle>()?;
                image.ensure_uploaded()?;
                let (image_w, image_h) = image.dimensions()?;
                let frame_w = component
                    .get::<f32>("frame_width")
                    .unwrap_or(32.0)
                    .max(1.0)
                    .min(image_w as f32);
                let frame_h = component
                    .get::<f32>("frame_height")
                    .unwrap_or(32.0)
                    .max(1.0)
                    .min(image_h as f32);
                let spacing = component.get::<f32>("spacing").unwrap_or(0.0).max(0.0);
                let margin = component
                    .get::<f32>("margin")
                    .unwrap_or(0.0)
                    .max(0.0)
                    .min(((image_w as f32 - frame_w) * 0.5).max(0.0))
                    .min(((image_h as f32 - frame_h) * 0.5).max(0.0));
                let usable_w = (image_w as f32 - margin * 2.0).max(frame_w);
                let usable_h = (image_h as f32 - margin * 2.0).max(frame_h);
                let available_columns =
                    (((usable_w + spacing) / (frame_w + spacing)).floor() as i64).max(1);
                let columns = component.get::<i64>("columns").unwrap_or(0).max(0);
                let columns = if columns == 0 {
                    available_columns
                } else {
                    columns.min(available_columns).max(1)
                };
                let available_rows =
                    (((usable_h + spacing) / (frame_h + spacing)).floor() as i64).max(1);
                let available_frames = columns.saturating_mul(available_rows).max(1);
                let configured_count = component.get::<i64>("frame_count").unwrap_or(0);
                let frame_count = if configured_count <= 0 {
                    available_frames
                } else {
                    configured_count.min(available_frames).max(1)
                };
                let mut frame = component.get::<i64>("frame").unwrap_or(0).max(0);

                let fps = component.get::<f32>("fps").unwrap_or(12.0);
                if component.get::<bool>("playing").unwrap_or(true) && fps.is_finite() && fps > 0.0
                {
                    let frame_duration = 1.0 / fps;
                    let mut elapsed =
                        component.get::<f32>("__frame_time").unwrap_or(0.0) + dt.max(0.0);
                    let steps = (elapsed / frame_duration).floor() as i64;
                    if steps > 0 {
                        elapsed -= steps as f32 * frame_duration;
                        frame = frame.saturating_add(steps);
                        if component.get::<bool>("looping").unwrap_or(true) {
                            frame %= frame_count;
                        } else if frame >= frame_count {
                            frame = frame_count - 1;
                            component.set("playing", false)?;
                        }
                        component.set("frame", frame)?;
                    }
                    component.set("__frame_time", elapsed)?;
                }
                frame = frame.min(frame_count - 1);

                let source = Rect {
                    x: margin + (frame % columns) as f32 * (frame_w + spacing),
                    y: margin + (frame / columns) as f32 * (frame_h + spacing),
                    w: frame_w,
                    h: frame_h,
                };
                let (x, y, rotation) = crate::window::get_global_transform(&entity)?;
                let (w, h) = crate::window::get_global_size(&entity)?;
                let use_middle_pivot = crate::window::uses_middle_pivot(&entity);
                let (draw_x, draw_y, pivot) = if use_middle_pivot {
                    let (px, py) = crate::window::get_global_rotation_pivot(&entity)?;
                    (px - w * 0.5, py - h * 0.5, Vec2 { x: px, y: py })
                } else {
                    (x, y, Vec2 { x, y })
                };
                let tint: Color = color4_to_color(component.get("color")?)?;
                let shader = shader_from_component(&component)?;
                let mut renderer = sprite_sheet_render_state
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
                    source: Some(source),
                    rotation,
                    pivot,
                    tint,
                    filter: app_texture_filter(ctx),
                    shader,
                });
                Ok(())
            })?,
        )?;
        core_components.set("SpriteSheet2D", sprite_sheet)?;
    }

    // NineSliceSprite2D / 9SliceSprite2D
    // draw a sprite with fixed-size edges and stretched center.
    {
        let nine_slice = create_basic_drawable(lua)?;
        let render_state = render_state.clone();

        nine_slice.set(
            "awake",
            lua.create_function(move |ctx, (_entity, component): (Table, Table)| {
                component.set("__neolove_component", "NineSliceSprite2D")?;
                component.set("color", color4(ctx, 255, 255, 255, 255)?)?;
                component.set("visible", true)?;
                component.set("shader", Value::Nil)?;
                component.set("image", Value::Nil)?;
                component.set("slice_left", 0.0)?;
                component.set("slice_right", 0.0)?;
                component.set("slice_top", 0.0)?;
                component.set("slice_bottom", 0.0)?;
                Ok(())
            })?,
        )?;

        nine_slice.set(
            "update",
            lua.create_function(move |ctx, (entity, component, _dt): (Table, Table, f32)| {
                if !component.get::<bool>("visible").unwrap_or(true) {
                    return Ok(());
                }
                let (x, y, rotation) = crate::window::get_global_transform(&entity)?;
                let (w, h) = crate::window::get_global_size(&entity)?;
                if w <= 0.0 || h <= 0.0 {
                    return Ok(());
                }
                let image = get_image_field(&component, "image")?;
                let Some(image) = image else {
                    return Ok(());
                };
                let tint = color4_to_color(component.get("color")?)?;
                let shader = shader_from_component(&component)?;
                let source = get_source_rect(&component, "source");
                let use_middle_pivot = crate::window::uses_middle_pivot(&entity);
                let (draw_x, draw_y, pivot) = if use_middle_pivot {
                    let (px, py) = crate::window::get_global_rotation_pivot(&entity)?;
                    (px - w * 0.5, py - h * 0.5, Vec2 { x: px, y: py })
                } else {
                    (x, y, Vec2 { x, y })
                };

                let mut renderer = render_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?;
                queue_nine_slice(
                    &mut renderer,
                    image,
                    Rect {
                        x: draw_x,
                        y: draw_y,
                        w,
                        h,
                    },
                    pivot,
                    rotation,
                    tint,
                    app_texture_filter(ctx),
                    shader,
                    source,
                    get_number_field(&component, "slice_left", "sliceLeft").unwrap_or(0.0),
                    get_number_field(&component, "slice_right", "sliceRight").unwrap_or(0.0),
                    get_number_field(&component, "slice_top", "sliceTop").unwrap_or(0.0),
                    get_number_field(&component, "slice_bottom", "sliceBottom").unwrap_or(0.0),
                )?;
                Ok(())
            })?,
        )?;

        core_components.set("NineSliceSprite2D", nine_slice.clone())?;
        core_components.set("9SliceSprite2D", nine_slice)?;
    }

    // Spritebox2D
    // cached geometric hit shape derived from the opaque pixels of Sprite2D/Image2D sprites.
    {
        let spritebox2d = lua.create_table()?;

        spritebox2d.set(
            "awake",
            lua.create_function(move |_ctx, (_entity, component): (Table, Table)| {
                component.set("__neolove_component", "Spritebox2D")?;
                component.set("computed", false)?;
                component.set("alpha_threshold", 0.0)?;
                component.set("rect_count", 0)?;
                component.set("bounds_x", 0.0)?;
                component.set("bounds_y", 0.0)?;
                component.set("bounds_w", 0.0)?;
                component.set("bounds_h", 0.0)?;
                component.raw_set("__spritebox_revision", 0)?;
                Ok(())
            })?,
        )?;

        let compute_spritebox = lua.create_function(move |lua, component: Table| {
            let entity = spritebox_entity(&component)?;
            let source = find_spritebox_source(&entity, &component)?.ok_or_else(|| {
                mlua::Error::external(
                    "Spritebox2D requires a Sprite2D, Image2D, or NineSliceSprite2D on the same entity",
                )
            })?;
            let alpha_threshold = component
                .get::<f32>("alpha_threshold")
                .unwrap_or(0.0)
                .clamp(0.0, 255.0) as u8;
            let shape = build_spritebox_shape(&source.image, source.source, alpha_threshold)?;
            write_spritebox_shape(lua, &component, &shape)?;
            Ok(true)
        })?;
        spritebox2d.set("ComputeSpritebox", compute_spritebox.clone())?;
        spritebox2d.set("computeSpritebox", compute_spritebox)?;

        let is_inside =
            lua.create_function(move |_ctx, (component, x, y): (Table, f32, f32)| {
                point_in_spritebox_shape(&component, Vec2 { x, y })
            })?;
        spritebox2d.set("IsInside", is_inside.clone())?;
        spritebox2d.set("isInside", is_inside)?;

        let is_intersecting =
            lua.create_function(move |lua, (component, other): (Table, Value)| {
                let Some(other) = resolve_spritebox_component(other)? else {
                    return Ok(false);
                };

                let Some(a) = build_world_spritebox_shape(lua, &component)? else {
                    return Ok(false);
                };
                let Some(b) = build_world_spritebox_shape(lua, &other)? else {
                    return Ok(false);
                };
                if a.rects.is_empty()
                    || b.rects.is_empty()
                    || !rect_aabb_intersects(a.bounds, b.bounds)
                {
                    return Ok(false);
                }

                for a_rect in &a.rects {
                    if !rect_aabb_intersects(a_rect.bounds, b.bounds) {
                        continue;
                    }
                    for b_rect in &b.rects {
                        if spritebox_rects_intersect(a_rect, b_rect) {
                            return Ok(true);
                        }
                    }
                }

                Ok(false)
            })?;
        spritebox2d.set("IsIntersecting", is_intersecting.clone())?;
        spritebox2d.set("isIntersecting", is_intersecting)?;

        spritebox2d.set(
            "update",
            lua.create_function(
                move |_ctx, (_entity, _component, _dt): (Table, Table, f32)| Ok(()),
            )?,
        )?;

        core_components.set("Spritebox2D", spritebox2d)?;
    }

    // TileTexture2D
    // draw an image repeatedly to fill entity size, with optional tile sizing and offset
    {
        let tile_texture2d = create_basic_drawable(lua)?;
        tile_texture2d.set("__neolove_component", "TileTexture2D")?;
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
                let shader = shader_from_component(&component)?;
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

                // Transform the viewport into the tile layer's unrotated coordinate space.
                // This keeps iteration bounded to potentially visible tiles even for rotated layers.
                let (screen_w, screen_h) = {
                    let platform = lock_platform_state(&platform);
                    let window = platform.window();
                    (window.width, window.height)
                };
                let viewport_corners = [
                    (0.0, 0.0),
                    (screen_w, 0.0),
                    (screen_w, screen_h),
                    (0.0, screen_h),
                ];
                let mut visible_left = f32::INFINITY;
                let mut visible_top = f32::INFINITY;
                let mut visible_right = f32::NEG_INFINITY;
                let mut visible_bottom = f32::NEG_INFINITY;
                for (screen_x, screen_y) in viewport_corners {
                    let (local_x, local_y) =
                        rotate_local(screen_x - pivot.x, screen_y - pivot.y, -rotation);
                    let unrotated_x = pivot.x + local_x;
                    let unrotated_y = pivot.y + local_y;
                    visible_left = visible_left.min(unrotated_x);
                    visible_top = visible_top.min(unrotated_y);
                    visible_right = visible_right.max(unrotated_x);
                    visible_bottom = visible_bottom.max(unrotated_y);
                }
                visible_left = visible_left.max(base_x);
                visible_top = visible_top.max(base_y);
                visible_right = visible_right.min(base_x + w);
                visible_bottom = visible_bottom.min(base_y + h);
                if visible_right <= visible_left || visible_bottom <= visible_top {
                    return Ok(());
                }
                let (local_left, local_top, local_right, local_bottom) = (
                    visible_left - base_x,
                    visible_top - base_y,
                    visible_right - base_x,
                    visible_bottom - base_y,
                );

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
                let filter = app_texture_filter(ctx);

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
                            filter,
                            shader: shader.clone(),
                        });
                    }
                }
                Ok(())
            })?,
        )?;

        core_components.set("TileTexture2D", tile_texture2d)?;
    }

    // Tilemap2D: render a finite grid of atlas tile ids. Tile id 0 addresses
    // the first atlas cell and -1 is empty. `tiles` accepts either a flat Lua
    // array or a comma/whitespace-separated string, which keeps editor scene
    // JSON compact and hand-editable.
    {
        let tilemap = create_basic_drawable(lua)?;
        tilemap.set("__neolove_component", "Tilemap2D")?;
        tilemap.set(
            "awake",
            lua.create_function(move |ctx, (_entity, component): (Table, Table)| {
                component.set("color", color4(ctx, 255, 255, 255, 255)?)?;
                component.set("visible", true)?;
                component.set("image", Value::Nil)?;
                component.set("map_width", 1)?;
                component.set("map_height", 1)?;
                component.set("tile_width", 32.0)?;
                component.set("tile_height", 32.0)?;
                component.set("spacing", 0.0)?;
                component.set("margin", 0.0)?;
                component.set("tiles", "0")?;
                Ok(())
            })?,
        )?;
        let tilemap_platform = platform.clone();
        let render_state = render_state.clone();
        tilemap.set(
            "update",
            lua.create_function(move |ctx, (entity, component, _dt): (Table, Table, f32)| {
                if !component.get::<bool>("visible").unwrap_or(true) {
                    return Ok(());
                }
                let map_width = component.get::<i32>("map_width").unwrap_or(1).max(1) as usize;
                let map_height = component.get::<i32>("map_height").unwrap_or(1).max(1) as usize;
                let tile_width = component.get::<f32>("tile_width").unwrap_or(32.0).max(1.0);
                let tile_height = component.get::<f32>("tile_height").unwrap_or(32.0).max(1.0);
                let spacing = component.get::<f32>("spacing").unwrap_or(0.0).max(0.0);
                let margin = component.get::<f32>("margin").unwrap_or(0.0).max(0.0);
                let image: Option<AnyUserData> = component.get("image")?;
                let Some(image) = image else {
                    return Ok(());
                };
                let image = image.borrow::<crate::assets::ImageHandle>()?;
                image.ensure_uploaded()?;
                let (image_width, image_height) = image.dimensions()?;
                let atlas_columns = (((image_width as f32 - margin * 2.0 + spacing)
                    / (tile_width + spacing))
                    .floor() as i32)
                    .max(1) as usize;
                let atlas_rows = (((image_height as f32 - margin * 2.0 + spacing)
                    / (tile_height + spacing))
                    .floor() as i32)
                    .max(1) as usize;
                let atlas_len = atlas_columns.saturating_mul(atlas_rows);

                let tiles = match component.get::<Value>("tiles")? {
                    Value::Table(values) => values
                        .sequence_values::<i32>()
                        .filter_map(Result::ok)
                        .collect::<Vec<_>>(),
                    Value::String(value) => value
                        .to_string_lossy()
                        .split(|character: char| character == ',' || character.is_whitespace())
                        .filter(|part| !part.is_empty())
                        .filter_map(|part| part.parse::<i32>().ok())
                        .collect::<Vec<_>>(),
                    _ => Vec::new(),
                };

                let (x, y, rotation) = crate::window::get_global_transform(&entity)?;
                let (width, height) = crate::window::get_global_size(&entity)?;
                if width <= 0.0 || height <= 0.0 {
                    return Ok(());
                }
                let use_middle_pivot = crate::window::uses_middle_pivot(&entity);
                let (base_x, base_y, pivot) = if use_middle_pivot {
                    let (px, py) = crate::window::get_global_rotation_pivot(&entity)?;
                    (px - width * 0.5, py - height * 0.5, Vec2 { x: px, y: py })
                } else {
                    (x, y, Vec2 { x, y })
                };
                let cell_width = width / map_width as f32;
                let cell_height = height / map_height as f32;
                let (viewport_width, viewport_height) = {
                    let platform = lock_platform_state(&tilemap_platform);
                    let window = platform.window();
                    (window.width, window.height)
                };
                let camera = render_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?
                    .camera_offset();
                let Some(visible) = visible_tile_cells(
                    base_x + camera.x,
                    base_y + camera.y,
                    width,
                    height,
                    Vec2 {
                        x: pivot.x + camera.x,
                        y: pivot.y + camera.y,
                    },
                    rotation,
                    viewport_width,
                    viewport_height,
                    map_width,
                    map_height,
                ) else {
                    return Ok(());
                };
                let tint = color4_to_color(component.get("color")?)?;
                let filter = app_texture_filter(ctx);
                let shader = shader_from_component(&component)?;
                let mut renderer = render_state
                    .lock()
                    .map_err(|_| mlua::Error::external("render state lock poisoned"))?;
                for row in visible.row_start..visible.row_end {
                    for column in visible.column_start..visible.column_end {
                        let index = row * map_width + column;
                        let tile = tiles.get(index).copied().unwrap_or(-1);
                        if tile < 0 || tile as usize >= atlas_len {
                            continue;
                        }
                        let tile = tile as usize;
                        let atlas_x = tile % atlas_columns;
                        let atlas_y = tile / atlas_columns;
                        renderer.queue(DrawCommand::Image {
                            image: image.clone(),
                            dest: Rect {
                                x: base_x + column as f32 * cell_width,
                                y: base_y + row as f32 * cell_height,
                                w: cell_width,
                                h: cell_height,
                            },
                            source: Some(Rect {
                                x: margin + atlas_x as f32 * (tile_width + spacing),
                                y: margin + atlas_y as f32 * (tile_height + spacing),
                                w: tile_width,
                                h: tile_height,
                            }),
                            rotation,
                            pivot,
                            tint,
                            filter,
                            shader: shader.clone(),
                        });
                    }
                }
                Ok(())
            })?,
        )?;
        core_components.set("Tilemap2D", tilemap)?;
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

    // Bolt2D / LegacyBolt2D
    // pins this entity's pivot to another entity at a local offset from that entity's pivot
    {
        for component_name in ["Bolt2D", "LegacyBolt2D"] {
            let bolt2d = lua.create_table()?;

            bolt2d.set(
                "awake",
                lua.create_function(move |_ctx, (_entity, component): (Table, Table)| {
                    component.set("__neolove_component", component_name)?;
                    component.set("enabled", true)?;
                    component.set("target_entity", Value::Nil)?;
                    component.set("target", Value::Nil)?;
                    component.set("x", 0.0)?;
                    component.set("y", 0.0)?;
                    component.set("offset_x", 0.0)?;
                    component.set("offset_y", 0.0)?;
                    component.set("strength", 1.0)?;
                    component.set("contacts_enabled", true)?;
                    component.set("current_force", 0.0)?;
                    component.set("force", 0.0)?;
                    Ok(())
                })?,
            )?;

            bolt2d.set(
                "attach",
                lua.create_function(move |_ctx, (component, target_entity): (Table, Table)| {
                    component.set("target_entity", target_entity.clone())?;
                    component.set("target", target_entity)?;
                    Ok(())
                })?,
            )?;

            bolt2d.set(
                "link",
                lua.create_function(move |_ctx, (component, target_entity): (Table, Table)| {
                    component.set("target_entity", target_entity.clone())?;
                    component.set("target", target_entity)?;
                    Ok(())
                })?,
            )?;

            bolt2d.set(
                "update",
                lua.create_function(
                    move |_ctx, (_entity, _component, _dt): (Table, Table, f32)| Ok(()),
                )?,
            )?;

            core_components.set(component_name, bolt2d)?;
        }
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

    for pair in core_components.pairs::<Value, Value>() {
        if let Ok((_, Value::Table(component))) = pair {
            component.raw_set("__neolove_core_component", true)?;
        }
    }
    lua.globals().set("core", core_components)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilemap_visibility_culls_large_maps_and_supports_rotation() {
        let visible = visible_tile_cells(
            -320.0,
            -320.0,
            1280.0,
            1280.0,
            Vec2 {
                x: -320.0,
                y: -320.0,
            },
            0.0,
            320.0,
            240.0,
            40,
            40,
        )
        .expect("map overlaps viewport");
        assert_eq!(visible.column_start, 10);
        assert_eq!(visible.column_end, 20);
        assert_eq!(visible.row_start, 10);
        assert_eq!(visible.row_end, 18);

        let rotated = visible_tile_cells(
            200.0,
            -40.0,
            320.0,
            320.0,
            Vec2 { x: 200.0, y: -40.0 },
            std::f32::consts::FRAC_PI_2,
            320.0,
            240.0,
            10,
            10,
        )
        .expect("rotated map overlaps viewport");
        assert!(rotated.column_start < rotated.column_end);
        assert!(rotated.row_start < rotated.row_end);

        assert!(
            visible_tile_cells(
                1000.0,
                1000.0,
                320.0,
                320.0,
                Vec2 {
                    x: 1000.0,
                    y: 1000.0
                },
                0.0,
                320.0,
                240.0,
                10,
                10,
            )
            .is_none()
        );
    }

    fn lua_with_core_components() -> (Lua, SharedRenderState) {
        let lua = Lua::new();
        let platform = crate::platform::new_shared_platform_state();
        let render_state = crate::renderer::new_shared_render_state();
        add_core_components(&lua, platform, render_state.clone(), std::env::temp_dir())
            .expect("core components install");
        (lua, render_state)
    }

    fn animated_mesh_renderer_test_handle() -> crate::mesh::MeshHandle {
        crate::mesh::import_from_bytes(
            br#"
                ; FBX 7.4.0 project file
                Objects: {
                    Geometry: 1, "Geometry::Triangle", "Mesh" {
                        Vertices: *9 { a: 0,0,0, 1,0,0, 0,1,0 }
                        PolygonVertexIndex: *3 { a: 0,1,-3 }
                    }
                    Model: 30, "Model::Bone", "LimbNode" {
                        Properties70: {
                            P: "Lcl Translation", "Lcl Translation", "", "A",0,0,0
                            P: "Lcl Rotation", "Lcl Rotation", "", "A",0,0,0
                            P: "Lcl Scaling", "Lcl Scaling", "", "A",1,1,1
                        }
                    }
                    Deformer: 20, "Deformer::Skin", "Skin" { }
                    Deformer: 21, "SubDeformer::Cluster", "Cluster" {
                        Indexes: *3 { a: 0,1,2 }
                        Weights: *3 { a: 1,1,1 }
                        TransformLink: *16 { a:
                            1,0,0,0,
                            0,1,0,0,
                            0,0,1,0,
                            0,0,0,1
                        }
                    }
                    AnimationStack: 40, "AnimStack::Move", "" { }
                    AnimationCurveNode: 50, "AnimCurveNode::Translate", "" { }
                    AnimationCurve: 51, "AnimCurve::X", "" {
                        KeyTime: *2 { a: 0,46186158000 }
                        KeyValueFloat: *2 { a: 0,2 }
                    }
                }
                Connections: {
                    C: "OO",20,1
                    C: "OO",21,20
                    C: "OO",30,21
                    C: "OP",51,50,"d|X"
                    C: "OP",50,30,"Lcl Translation"
                }
            "#,
            crate::mesh::MeshFormat::Fbx,
        )
        .expect("animated ASCII FBX fixture imports")
    }

    fn mesh_renderer_test_component(
        lua: &Lua,
        entity: &Table,
        mesh: crate::mesh::MeshHandle,
    ) -> Table {
        let component = lua.create_table().expect("component table");
        let prototype: Table = lua
            .globals()
            .get::<Table>("core")
            .and_then(|core| core.get("MeshRenderer3D"))
            .expect("MeshRenderer3D prototype");
        prototype
            .get::<Function>("awake")
            .and_then(|awake| awake.call::<()>((entity.clone(), component.clone())))
            .expect("MeshRenderer3D awake");
        assert_eq!(
            component
                .get::<String>("primitive")
                .expect("primitive mode"),
            "none",
            "runtime components must preserve the legacy direct-mesh workflow"
        );
        component
            .set("mesh", lua.create_userdata(mesh).expect("mesh userdata"))
            .expect("assign mesh");
        component
    }

    #[test]
    fn lighting_global_is_installed_and_updates_config() {
        let (lua, render_state) = lua_with_core_components();
        lua.load(
            r#"
                assert(type(lighting) == "table", "lighting global missing")
                assert(lighting.isEnabled() == false, "lighting should start disabled")
                lighting.setEnabled(true)
                lighting.setAmbient(Color4(10, 20, 30), 0.5)
                lighting.setAmbientOcclusion(true, 24, 0.7, 8)
                lighting.setShadows(true, 4)
                lighting.setBloom(0.25)
                lighting.setExposure(1.5)
                lighting.setQuality("high")
                assert(lighting.isEnabled() == true, "enable failed")
                assert(lighting.getQuality() == "high", "quality not applied")
                local color, intensity = lighting.getAmbient()
                assert(color.r == 10 and color.g == 20 and color.b == 30, "ambient color wrong")
                assert(math.abs(intensity - 0.5) < 1e-4, "ambient intensity wrong")
            "#,
        )
        .exec()
        .expect("lighting api script runs");

        let config = render_state
            .lock()
            .expect("render state lock should remain available")
            .lighting_config();
        assert!(config.enabled);
        assert_eq!(config.ambient, Color::rgba(10, 20, 30, 255));
        assert!(config.ao_enabled);
        assert_eq!(config.quality, crate::lighting::LightQuality::High);
        assert!((config.exposure - 1.5).abs() < 1e-4);
    }

    #[test]
    fn environment_global_and_component_update_shared_render_state() {
        let (lua, render_state) = lua_with_core_components();
        lua.load(
            r#"
                assert(type(environment3d) == "table")
                assert(environment3d == skybox)
                environment3d.setGradient(Color4(10, 20, 30), Color4(40, 50, 60))
                environment3d.setIntensity(1.5)

                local component = {}
                core.Environment3D.awake({}, component)
                component.mode = "solid"
                component.color = Color4(70, 80, 90)
                core.Environment3D.update({}, component, 0)
            "#,
        )
        .exec()
        .expect("environment script runs");

        let environment = render_state.lock().expect("render state").environment_3d();
        assert!(environment.enabled);
        assert_eq!(
            environment.mode,
            crate::environment3d::EnvironmentMode3D::Solid
        );
        assert_eq!(environment.solid, Color::rgba(70, 80, 90, 255));
    }

    #[test]
    fn particle_system_3d_controls_bounded_native_pool_and_queues_one_batch() {
        let (lua, render_state) = lua_with_core_components();
        lua.load(
            r#"
                local component = {}
                local entity = {}
                core.ParticleSystem3D.awake(entity, component)
                component.playing = false
                component.max_particles = 4
                component.lifetime_min = 10
                component.lifetime_max = 10
                core.ParticleSystem3D.Emit(component, 100)
                core.ParticleSystem3D.update(entity, component, 0)
                assert(component.particle_count == 4, "emission must clamp to capacity")
                core.ParticleSystem3D.Pause(component)
                assert(component.playing == false)
            "#,
        )
        .exec()
        .expect("particle component script runs");

        let commands = render_state.lock().expect("render state").drain();
        assert_eq!(
            commands
                .iter()
                .filter(|command| matches!(command, DrawCommand::Particles3D(_)))
                .count(),
            1,
            "one emitter should submit one renderer batch"
        );
    }

    #[test]
    fn mesh_renderer_3d_resolves_editor_primitives_and_queues_geometry() {
        let (lua, render_state) = lua_with_core_components();
        let entity =
            crate::window::create_entity_table(&lua, "primitive", 0.0, 0.0, None).expect("entity");
        lua.globals()
            .set("primitiveEntity", entity)
            .expect("global");
        lua.load(
            r#"
                local renderer = {}
                core.MeshRenderer3D.awake(primitiveEntity, renderer)
                renderer.primitive = "cube"
                renderer.primitive_size_y = 3
                renderer.primitive_segments = 12
                renderer.primitive_rings = 6
                core.MeshRenderer3D.update(primitiveEntity, renderer, 1 / 60)
                assert(renderer.mesh ~= nil, "primitive should resolve to a MeshHandle")
                assert(renderer.mesh:triangleCount() > 0)
                local bounds = renderer.mesh:bounds()
                assert(math.abs((bounds.max_y - bounds.min_y) - 3) < 0.0001,
                    "primitive_size_y must not be shadowed by primitive_height")
                assert(type(core.MeshRenderer3D.PlayAnimation) == "function")
                assert(type(core.MeshRenderer3D.StopAnimation) == "function")

                renderer.primitive = "sphere"
                renderer.primitive_segments = 2
                local ok, message = pcall(function()
                    core.MeshRenderer3D.update(primitiveEntity, renderer, 1 / 60)
                end)
                assert(not ok and string.find(tostring(message), "segments must be 3..1024"),
                    "invalid primitive tessellation must return a validation error")
            "#,
        )
        .exec()
        .expect("primitive renderer script runs");

        let commands = render_state.lock().expect("render state").drain();
        assert_eq!(
            commands
                .iter()
                .filter(|command| matches!(command, DrawCommand::Mesh3D(_)))
                .count(),
            1
        );
    }

    #[test]
    fn mesh_renderer_3d_manual_mode_preserves_handles_and_managed_sources_replace_them() {
        let (lua, render_state) = lua_with_core_components();
        let entity =
            crate::window::create_entity_table(&lua, "manual", 0.0, 0.0, None).expect("entity");
        let source = animated_mesh_renderer_test_handle();
        let component = mesh_renderer_test_component(&lua, &entity, source.clone());
        let prototype: Table = lua
            .globals()
            .get::<Table>("core")
            .and_then(|core| core.get("MeshRenderer3D"))
            .expect("MeshRenderer3D prototype");
        let update: Function = prototype.get("update").expect("update callback");

        component
            .set("primitive_segments", "unused in manual mode")
            .expect("irrelevant primitive field");
        update
            .call::<()>((entity.clone(), component.clone(), 0.0f32))
            .expect("manual mesh update");
        let manual = component
            .get::<AnyUserData>("mesh")
            .expect("manual mesh userdata")
            .borrow::<crate::mesh::MeshHandle>()
            .expect("manual MeshHandle")
            .clone();
        assert_eq!(manual.identity(), source.identity());

        component.set("primitive", "cube").expect("managed cube");
        component
            .set("primitive_segments", 24)
            .expect("cube segments");
        update
            .call::<()>((entity.clone(), component.clone(), 0.0f32))
            .expect("managed primitive update");
        let managed = component
            .get::<AnyUserData>("mesh")
            .expect("managed mesh userdata")
            .borrow::<crate::mesh::MeshHandle>()
            .expect("managed MeshHandle")
            .clone();
        assert_ne!(managed.identity(), source.identity());

        // Switching modes and assigning in one script frame must not clear the
        // newly supplied handle along with the previous managed source.
        component.set("primitive", "none").expect("manual mode");
        component
            .set(
                "mesh",
                lua.create_userdata(source.clone())
                    .expect("source userdata"),
            )
            .expect("replace with manual mesh");
        update
            .call::<()>((entity, component.clone(), 0.0f32))
            .expect("manual replacement update");
        let restored = component
            .get::<AnyUserData>("mesh")
            .expect("restored mesh userdata")
            .borrow::<crate::mesh::MeshHandle>()
            .expect("restored MeshHandle")
            .clone();
        assert_eq!(restored.identity(), source.identity());

        assert_eq!(
            render_state
                .lock()
                .expect("render state")
                .drain()
                .into_iter()
                .filter(|command| matches!(command, DrawCommand::Mesh3D(_)))
                .count(),
            3
        );
    }

    #[test]
    fn mesh_renderer_3d_obeys_autoplay_pause_stop_and_completion() {
        let (lua, _render_state) = lua_with_core_components();
        let entity =
            crate::window::create_entity_table(&lua, "animated", 0.0, 0.0, None).expect("entity");
        let source = animated_mesh_renderer_test_handle();
        let component = mesh_renderer_test_component(&lua, &entity, source.clone());
        component.set("animation", "Move").expect("animation clip");
        component
            .set("animation_autoplay", false)
            .expect("disable autoplay");

        let prototype: Table = lua
            .globals()
            .get::<Table>("core")
            .and_then(|core| core.get("MeshRenderer3D"))
            .expect("MeshRenderer3D prototype");
        let update: Function = prototype.get("update").expect("update callback");
        let play: Function = prototype.get("PlayAnimation").expect("play callback");
        let pause: Function = prototype.get("PauseAnimation").expect("pause callback");
        let stop: Function = prototype.get("StopAnimation").expect("stop callback");

        update
            .call::<()>((entity.clone(), component.clone(), 0.25f32))
            .expect("autoplay-disabled update");
        assert!(!component.get::<bool>("animation_playing").expect("playing"));
        let unplayed = component
            .get::<AnyUserData>("mesh")
            .expect("unplayed userdata")
            .borrow::<crate::mesh::MeshHandle>()
            .expect("unplayed MeshHandle")
            .clone();
        assert_eq!(unplayed.identity(), source.identity());
        assert_eq!(source.revision().expect("source revision"), 0);

        play.call::<()>((component.clone(), Option::<String>::None))
            .expect("play animation");
        update
            .call::<()>((entity.clone(), component.clone(), 0.25f32))
            .expect("playing update");
        let posed = component
            .get::<AnyUserData>("mesh")
            .expect("posed userdata")
            .borrow::<crate::mesh::MeshHandle>()
            .expect("posed MeshHandle")
            .clone();
        assert_ne!(posed.identity(), source.identity());
        assert_eq!(source.revision().expect("shared source revision"), 0);
        assert!(posed.revision().expect("pose revision") > 0);

        pause
            .call::<()>(component.clone())
            .expect("pause animation");
        let paused_revision = posed.revision().expect("paused revision");
        update
            .call::<()>((entity.clone(), component.clone(), 0.25f32))
            .expect("paused update");
        assert_eq!(posed.revision().expect("still paused"), paused_revision);

        component
            .set("animation_looping", false)
            .expect("non-looping playback");
        play.call::<()>((component.clone(), Option::<String>::None))
            .expect("restart animation");
        update
            .call::<()>((entity.clone(), component.clone(), 2.0f32))
            .expect("finishing update");
        assert!(
            !component.get::<bool>("animation_playing").expect("playing"),
            "non-looping playback must report completion on the finishing frame"
        );
        let completed_revision = posed.revision().expect("completed revision");
        update
            .call::<()>((entity.clone(), component.clone(), 0.25f32))
            .expect("completed update");
        assert_eq!(
            posed.revision().expect("stable completed pose"),
            completed_revision,
            "a completed clip must not keep deforming the mesh"
        );

        stop.call::<()>(component.clone()).expect("stop animation");
        let stopped_revision = posed.revision().expect("stopped revision");
        update
            .call::<()>((entity.clone(), component.clone(), 0.25f32))
            .expect("stopped update");
        assert_eq!(
            posed.revision().expect("stable bind pose"),
            stopped_revision
        );
        assert!(
            posed.snapshot().expect("bind-pose snapshot").mesh.vertices[0].position[0].abs()
                < 0.0001
        );

        component
            .set("animation_speed", -1.0)
            .expect("negative speed");
        play.call::<()>((component.clone(), Option::<String>::None))
            .expect("request reverse playback");
        let error = update
            .call::<()>((entity, component, 0.1f32))
            .expect_err("reverse component playback should be rejected");
        assert!(
            error
                .to_string()
                .contains("reverse component playback is not supported")
        );
    }

    #[test]
    fn mesh_renderer_3d_animated_entities_do_not_share_pose_state() {
        let (lua, _render_state) = lua_with_core_components();
        let source = animated_mesh_renderer_test_handle();
        let first_entity =
            crate::window::create_entity_table(&lua, "first", 0.0, 0.0, None).expect("entity");
        let second_entity =
            crate::window::create_entity_table(&lua, "second", 0.0, 0.0, None).expect("entity");
        let first = mesh_renderer_test_component(&lua, &first_entity, source.clone());
        let second = mesh_renderer_test_component(&lua, &second_entity, source.clone());
        first.set("animation", "Move").expect("first animation");
        second.set("animation", "Move").expect("second animation");

        let update: Function = lua
            .globals()
            .get::<Table>("core")
            .and_then(|core| core.get::<Table>("MeshRenderer3D"))
            .and_then(|prototype| prototype.get("update"))
            .expect("update callback");
        update
            .call::<()>((first_entity, first.clone(), 0.25f32))
            .expect("first update");
        update
            .call::<()>((second_entity, second.clone(), 0.75f32))
            .expect("second update");

        let first_mesh = first
            .get::<AnyUserData>("mesh")
            .expect("first mesh")
            .borrow::<crate::mesh::MeshHandle>()
            .expect("first MeshHandle")
            .clone();
        let second_mesh = second
            .get::<AnyUserData>("mesh")
            .expect("second mesh")
            .borrow::<crate::mesh::MeshHandle>()
            .expect("second MeshHandle")
            .clone();
        assert_ne!(first_mesh.identity(), source.identity());
        assert_ne!(second_mesh.identity(), source.identity());
        assert_ne!(first_mesh.identity(), second_mesh.identity());
        assert_eq!(source.revision().expect("shared source revision"), 0);
        assert!(
            first_mesh.snapshot().expect("first pose").mesh.vertices[0].position[0]
                < second_mesh.snapshot().expect("second pose").mesh.vertices[0].position[0]
        );
    }

    #[test]
    fn lighting_sample_reports_light_at_a_position() {
        let (lua, _render_state) = lua_with_core_components();
        lua.load(
            r#"
                window = { x = 800, y = 600 }
                -- Disabled lighting reports fully lit (white).
                local off = lighting.sample(400, 300)
                assert(off ~= nil and off.r == 255 and off.g == 255 and off.b == 255,
                    "disabled lighting should read as white")
                -- Off-screen returns nil.
                assert(lighting.sample(-10, 300) == nil, "off-screen should be nil")
                assert(lighting.sample(400, 9000) == nil, "off-screen should be nil")
                -- Enabled with dark ambient reads dark (no lights queued yet).
                lighting.setEnabled(true)
                lighting.setAmbient(Color4(0, 0, 0), 0.0)
                local dark = lighting.sample(400, 300)
                assert(dark ~= nil and dark.r == 0 and dark.g == 0 and dark.b == 0,
                    "dark ambient with no lights should read black")
                assert(type(lighting.getAt) == "function", "getAt alias missing")
            "#,
        )
        .exec()
        .expect("lighting.sample script runs");
    }

    #[test]
    fn rng_global_is_seedable_and_reproducible() {
        let (lua, _render_state) = lua_with_core_components();
        lua.load(
            r#"
                assert(type(Rng) == "table", "Rng global missing")
                local a = Rng.new(1234)
                local b = Rng.new(1234)
                for i = 1, 50 do
                    assert(a:number() == b:number(), "same seed must reproduce")
                end
                local r = Rng.new(7)
                for i = 1, 200 do
                    local n = r:integer(1, 6)
                    assert(n >= 1 and n <= 6, "integer out of range: " .. tostring(n))
                end
                assert(type(Rng(99)) == "userdata", "callable Rng() form failed")
                local named = Rng.fromString("world-seed")
                assert(type(named:number()) == "number", "fromString failed")
                local deck = { 1, 2, 3, 4, 5 }
                r:shuffle(deck)
                assert(#deck == 5, "shuffle must preserve length")
                assert(r:pick(deck) ~= nil, "pick must return an element")
            "#,
        )
        .exec()
        .expect("rng api script runs");
    }

    #[test]
    fn light_components_are_registered() {
        let (lua, _render_state) = lua_with_core_components();
        lua.load(
            r#"
                assert(type(core.Light2D) == "table", "Light2D missing")
                assert(type(core.Light2D.update) == "function", "Light2D.update missing")
                assert(type(core.Light2D.awake) == "function", "Light2D.awake missing")
                assert(type(core.LightOccluder2D) == "table", "LightOccluder2D missing")
                assert(type(core.LightOccluder2D.update) == "function", "LightOccluder2D.update missing")
            "#,
        )
        .exec()
        .expect("component registration script runs");
    }

    #[test]
    fn camera_component_is_registered_with_activation_methods() {
        let (lua, _render_state) = lua_with_core_components();
        lua.load(
            r#"
                assert(type(core.Camera) == "table", "Camera missing")
                assert(core.Camera.__neolove_component == "Camera")
                assert(core.Camera.NEOLOVE_CAMERA == true)
                assert(type(core.Camera.SetActive) == "function")
                assert(type(core.Camera.setActive) == "function")
                assert(type(core.Camera.IsActive) == "function")
                assert(type(core.Camera.isActive) == "function")
            "#,
        )
        .exec()
        .expect("camera component registration script runs");
    }

    #[test]
    fn three_d_components_and_runtime_modules_are_registered() {
        let (lua, _render_state) = lua_with_core_components();
        lua.load(
            r#"
                for _, name in ipairs({
                    "Camera3D", "MeshRenderer3D", "Light3D", "Rigidbody3D", "Collider3D"
                }) do
                    local component = core[name]
                    assert(type(component) == "table", name .. " missing")
                    assert(component.__neolove_component == name, name .. " has wrong metadata")
                    assert(type(component.update) == "function", name .. ".update missing")
                end
                assert(type(physics3d) == "table", "physics3d global missing")
                assert(type(physics3d.raycast) == "function", "physics3d.raycast missing")
                assert(type(physics3d.contacts) == "function", "physics3d contacts missing")
                assert(type(physics3d.resolveBody) == "function", "physics3d response missing")
                assert(type(physics3d.broadphasePairs) == "function", "physics3d broadphase missing")
                assert(physics3D == physics3d, "physics3D alias must share the module")
                assert(type(postprocess) == "table", "postprocess global missing")
                assert(postProcess == postprocess, "postProcess alias must share the module")
            "#,
        )
        .exec()
        .expect("3D runtime APIs are registered");
    }

    #[test]
    fn postprocess_lua_stack_is_ordered_and_configurable() {
        let (lua, render_state) = lua_with_core_components();
        lua.load(
            r#"
                assert(postprocess.count() == 0)
                local pixel = postprocess.add("pixelated", { block_size = 3 })
                local chromatic = postprocess.add("chromatic_abberation", {
                    offset_pixels = 1.5,
                    angle_degrees = 90,
                    enabled = false,
                })
                assert(pixel == 1 and chromatic == 2)
                assert(postprocess.move(2, 1))
                assert(postprocess.setPassEnabled(1, true))
                postprocess.setEnabled(false)
                assert(postprocess.isEnabled() == false)
                assert(postprocess.remove(2))
                assert(postprocess.count() == 1)
            "#,
        )
        .exec()
        .expect("post-process Lua API runs");

        let state = render_state.lock().expect("render state lock");
        let stack = state.post_process();
        assert!(!stack.enabled);
        assert_eq!(stack.effects.len(), 1);
        assert!(matches!(
            stack.effects[0].effect,
            crate::post_process::Effect::ChromaticAberration(_)
        ));
        assert!(stack.effects[0].enabled);
    }

    #[test]
    fn rigidbody_and_box_collider_work_through_lua() {
        let (lua, _render_state) = lua_with_core_components();
        let entity = crate::window::create_entity_table(&lua, "physics", 0.0, 0.0, None)
            .expect("entity table");
        entity.set("id", 42u64).expect("entity id");
        lua.globals().set("physics_entity", entity).expect("global");

        lua.load(
            r#"
                local body = {}
                core.Rigidbody3D.awake(physics_entity, body)
                body.gravity_scale = 0
                body.velocity_z = -10
                core.Rigidbody3D.update(physics_entity, body, 0.05)
                assert(math.abs(physics_entity.position_z + 0.5) < 0.0001,
                    "Rigidbody3D should integrate position_z")

                local collider = {}
                core.Collider3D.awake(physics_entity, collider)
                collider.size_x = 2
                collider.size_y = 2
                collider.size_z = 2
                core.Collider3D.update(physics_entity, collider, 0)
                local hit = physics3d.raycast(0, 0, 5, 0, 0, -1, 20)
                assert(hit ~= nil, "box collider should be raycastable")
                assert(hit.entity_id == 42, "raycast returned the wrong collider")
                assert(hit.distance > 4 and hit.distance < 5, "unexpected hit distance")
                assert(hit.normal_z > 0.9, "front face should point toward the ray origin")

                core.Collider3D.destroy(physics_entity, collider)
                assert(physics3d.raycast(0, 0, 5, 0, 0, -1, 20) == nil,
                    "destroy must unregister the collider")
            "#,
        )
        .exec()
        .expect("3D body and collider APIs run");
    }

    #[test]
    fn physics3d_contacts_auto_response_and_trigger_callbacks_work_through_lua() {
        let (lua, _render_state) = lua_with_core_components();
        let dynamic = crate::window::create_entity_table(&lua, "dynamic", 0.0, 0.0, None)
            .expect("dynamic entity");
        dynamic.set("id", 100u64).expect("dynamic id");
        let obstacle = crate::window::create_entity_table(&lua, "obstacle", 1.5, 0.0, None)
            .expect("obstacle entity");
        obstacle.set("id", 101u64).expect("obstacle id");
        lua.globals().set("dynamic3d", dynamic).expect("global");
        lua.globals().set("obstacle3d", obstacle).expect("global");

        lua.load(
            r#"
                local body = {}
                local dynamicCollider = {}
                local obstacleCollider = {}
                core.Rigidbody3D.awake(dynamic3d, body)
                core.Collider3D.awake(dynamic3d, dynamicCollider)
                core.Collider3D.awake(obstacle3d, obstacleCollider)
                dynamicCollider.shape = "sphere"
                dynamicCollider.radius = 1
                obstacleCollider.shape = "sphere"
                obstacleCollider.radius = 1
                table.insert(dynamic3d.components, body)
                table.insert(dynamic3d.components, dynamicCollider)
                table.insert(obstacle3d.components, obstacleCollider)
                core.Collider3D.update(dynamic3d, dynamicCollider, 0)
                core.Collider3D.update(obstacle3d, obstacleCollider, 0)

                local initial = physics3d.contacts({ entity_id = 100 })
                assert(#initial == 1 and initial[1].quality == "exact")
                assert(initial[1].can_resolve == true)
                assert(math.abs(initial[1].penetration - 0.5) < 0.0001)

                local contactCallbacks = 0
                local triggerCallbacks = 0
                core.Rigidbody3D.setOnContact(body, function(contact)
                    contactCallbacks += 1
                    assert(contact.self_id == 100 and contact.other_id == 101)
                    assert(contact.exact and contact.can_resolve)
                end)
                core.Rigidbody3D.setOnTrigger(body, function(contact)
                    triggerCallbacks += 1
                    assert(contact.is_trigger and not contact.can_resolve)
                end)
                body.gravity_scale = 0
                body.velocity_x = 4
                core.Rigidbody3D.update(dynamic3d, body, 0.05)
                assert(dynamic3d.x < -0.49, "exact contact should remove penetration")
                assert(math.abs(body.velocity_x) < 0.0001,
                    "zero restitution should stop inward velocity")
                assert(contactCallbacks == 1, "onContact should run once for this step")

                -- A trigger reports the same exact shape contact without moving
                -- the body or changing velocity.
                dynamic3d.x = 0
                body.__last_world_x = 0
                body.velocity_x = 4
                core.Collider3D.update(dynamic3d, dynamicCollider, 0)
                obstacleCollider.is_trigger = true
                core.Collider3D.update(obstacle3d, obstacleCollider, 0)
                local triggerResult = physics3d.resolveBody(dynamic3d, body)
                assert(triggerResult.trigger_count == 1)
                assert(triggerResult.resolved_count == 0)
                assert(math.abs(dynamic3d.x) < 0.0001)
                assert(math.abs(body.velocity_x - 4) < 0.0001)
                assert(triggerCallbacks == 1, "onTrigger should run once")

                core.Collider3D.destroy(dynamic3d, dynamicCollider)
                core.Collider3D.destroy(obstacle3d, obstacleCollider)
            "#,
        )
        .exec()
        .expect("3D contact/response Lua APIs run");
    }

    fn component_with_letter_bounds(lua: &Lua) -> mlua::Result<Table> {
        let component = lua.create_table()?;
        let bounds = lua.create_table()?;
        let entry = lua.create_table()?;
        entry.set("x", 10.0)?;
        entry.set("y", 20.0)?;
        entry.set("w", 30.0)?;
        entry.set("h", 40.0)?;
        bounds.set(1, entry)?;
        component.set("__letter_bounds", bounds)?;
        Ok(component)
    }

    fn assert_nil4(values: (Value, Value, Value, Value)) {
        assert!(matches!(values.0, Value::Nil));
        assert!(matches!(values.1, Value::Nil));
        assert!(matches!(values.2, Value::Nil));
        assert!(matches!(values.3, Value::Nil));
    }

    fn assert_nil2(values: (Value, Value)) {
        assert!(matches!(values.0, Value::Nil));
        assert!(matches!(values.1, Value::Nil));
    }

    fn assert_lua_number(value: &Value, expected: f64) {
        match value {
            Value::Integer(value) => assert_eq!(*value as f64, expected),
            Value::Number(value) => assert_eq!(*value, expected),
            other => panic!("expected numeric Lua value, got {}", other.type_name()),
        }
    }

    #[test]
    fn letter_bounds_lookup_uses_zero_based_index() -> mlua::Result<()> {
        let lua = Lua::new();
        let component = component_with_letter_bounds(&lua)?;

        let bounds = get_letter_bounds_values(&lua, None, component.clone(), Value::Integer(0))?;
        assert_lua_number(&bounds.0, 10.0);
        assert_lua_number(&bounds.1, 20.0);
        assert_lua_number(&bounds.2, 30.0);
        assert_lua_number(&bounds.3, 40.0);

        let position = get_letter_position_values(&lua, None, component, Value::Integer(0))?;
        assert_lua_number(&position.0, 10.0);
        assert_lua_number(&position.1, 20.0);
        Ok(())
    }

    #[test]
    fn letter_bounds_lookup_rejects_invalid_indexes() -> mlua::Result<()> {
        let lua = Lua::new();
        let component = component_with_letter_bounds(&lua)?;
        let invalid = vec![
            Value::Integer(-1),
            Value::Integer(i64::MAX),
            Value::Number(-1.0),
            Value::Number(1.5),
            Value::Number(f64::NAN),
            Value::String(lua.create_string("0")?),
        ];

        for index in invalid {
            assert_nil4(get_letter_bounds_values(
                &lua,
                None,
                component.clone(),
                index.clone(),
            )?);
            assert_nil2(get_letter_position_values(
                &lua,
                None,
                component.clone(),
                index,
            )?);
        }

        Ok(())
    }

    #[test]
    fn unbound_letter_bounds_callback_rejects_dot_call_without_panicking() -> mlua::Result<()> {
        let lua = Lua::new();
        let component = component_with_letter_bounds(&lua)?;
        let get_letter_bounds = lua.create_function(|ctx, args: mlua::Variadic<Value>| {
            get_letter_bounds_from_args(ctx, None, None, args)
        })?;
        component.set("getLetterBounds", get_letter_bounds)?;

        let script = r#"
            local x, y, w, h = component.getLetterBounds(0)
            return x, y, w, h
        "#;
        lua.globals().set("component", component)?;
        let values: (Value, Value, Value, Value) = lua.load(script).eval()?;
        assert_nil4(values);
        Ok(())
    }

    #[test]
    fn bound_letter_bounds_callback_accepts_dot_call() -> mlua::Result<()> {
        let lua = Lua::new();
        let component = component_with_letter_bounds(&lua)?;
        install_unbound_textbox_letter_lookup_methods(&lua, &component, PathBuf::new())?;
        bind_textbox_letter_lookup_methods(&lua, &component)?;

        lua.globals().set("component", component.clone())?;
        let bounds: (Value, Value, Value, Value) = lua
            .load(
                r#"
                return component.getLetterBounds(0)
                "#,
            )
            .eval()?;
        assert_lua_number(&bounds.0, 10.0);
        assert_lua_number(&bounds.1, 20.0);
        assert_lua_number(&bounds.2, 30.0);
        assert_lua_number(&bounds.3, 40.0);

        let position: (Value, Value) = lua
            .load(
                r#"
                return component:getLetterPosition(0)
                "#,
            )
            .eval()?;
        assert_lua_number(&position.0, 10.0);
        assert_lua_number(&position.1, 20.0);
        Ok(())
    }

    fn runtime_widget_fixture() -> (Lua, SharedPlatformState, SharedRenderState) {
        let lua = Lua::new();
        let platform = crate::platform::new_shared_platform_state();
        let render_state = crate::renderer::new_shared_render_state();
        add_core_components(
            &lua,
            platform.clone(),
            render_state.clone(),
            std::env::temp_dir(),
        )
        .expect("core components install");
        (lua, platform, render_state)
    }

    fn make_runtime_widget(
        lua: &Lua,
        kind: &str,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> mlua::Result<(Table, Table, Function)> {
        let entity = crate::window::create_entity_table(lua, kind, x, y, None)?;
        entity.set("size_x", width)?;
        entity.set("size_y", height)?;
        let core: Table = lua.globals().get("core")?;
        let prototype: Table = core.get(kind)?;
        let component = lua.create_table()?;
        prototype
            .get::<Function>("awake")?
            .call::<()>((entity.clone(), component.clone()))?;
        let update = prototype.get::<Function>("update")?;
        Ok((entity, component, update))
    }

    fn update_runtime_widget(
        entity: &Table,
        component: &Table,
        update: &Function,
    ) -> mlua::Result<()> {
        update.call::<()>((entity.clone(), component.clone(), 1.0 / 60.0))
    }

    #[test]
    fn overlapping_runtime_widgets_route_press_only_to_topmost() -> mlua::Result<()> {
        let (lua, platform, _render_state) = runtime_widget_fixture();
        let (back_entity, back, back_update) =
            make_runtime_widget(&lua, "Button", 20.0, 20.0, 100.0, 40.0)?;
        let (front_entity, front, front_update) =
            make_runtime_widget(&lua, "Button", 20.0, 20.0, 100.0, 40.0)?;

        // Prime retained draw order with the same order used by the renderer.
        begin_ui_frame(&_render_state);
        update_runtime_widget(&back_entity, &back, &back_update)?;
        update_runtime_widget(&front_entity, &front, &front_update)?;

        {
            let mut platform = lock_platform_state(&platform);
            platform.set_mouse_position(50.0, 40.0);
            platform.input_mut().mouse_pressed.insert("left".into());
            platform.input_mut().mouse_down.insert("left".into());
        }
        begin_ui_frame(&_render_state);
        update_runtime_widget(&back_entity, &back, &back_update)?;
        update_runtime_widget(&front_entity, &front, &front_update)?;

        assert!(!back.get::<bool>("pressed")?);
        assert!(!back.get::<bool>("hovered")?);
        assert!(front.get::<bool>("pressed")?);
        assert!(front.get::<bool>("hovered")?);
        Ok(())
    }

    #[test]
    fn runtime_slider_capture_and_button_keyboard_focus_are_integrated() -> mlua::Result<()> {
        let (lua, platform, _render_state) = runtime_widget_fixture();
        let (slider_entity, slider, slider_update) =
            make_runtime_widget(&lua, "Slider", 20.0, 20.0, 100.0, 24.0)?;
        begin_ui_frame(&_render_state);
        update_runtime_widget(&slider_entity, &slider, &slider_update)?;

        {
            let mut platform = lock_platform_state(&platform);
            platform.set_mouse_position(40.0, 32.0);
            platform.input_mut().mouse_pressed.insert("left".into());
            platform.input_mut().mouse_down.insert("left".into());
        }
        begin_ui_frame(&_render_state);
        update_runtime_widget(&slider_entity, &slider, &slider_update)?;
        assert!(slider.get::<bool>("dragging")?);

        // Capture keeps updating after the pointer leaves the slider bounds.
        {
            let mut platform = lock_platform_state(&platform);
            platform.begin_frame();
            platform.set_mouse_position(400.0, 200.0);
        }
        begin_ui_frame(&_render_state);
        update_runtime_widget(&slider_entity, &slider, &slider_update)?;
        assert!(slider.get::<bool>("dragging")?);
        assert_eq!(slider.get::<f32>("value")?, 100.0);

        // Release the slider, then verify Tab focus and Enter activation across
        // two independently drawn buttons.
        {
            let mut platform = lock_platform_state(&platform);
            platform.begin_frame();
            platform.input_mut().mouse_down.remove("left");
            platform.input_mut().mouse_released.insert("left".into());
        }
        begin_ui_frame(&_render_state);
        update_runtime_widget(&slider_entity, &slider, &slider_update)?;

        let (first_entity, first, first_update) =
            make_runtime_widget(&lua, "Button", 20.0, 80.0, 100.0, 32.0)?;
        let (second_entity, second, second_update) =
            make_runtime_widget(&lua, "Button", 140.0, 80.0, 100.0, 32.0)?;
        let clicks = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let callback_clicks = clicks.clone();
        second.set(
            "onClick",
            lua.create_function(move |_lua, (_entity, _component): (Table, Table)| {
                callback_clicks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(())
            })?,
        )?;

        {
            let mut platform = lock_platform_state(&platform);
            platform.begin_frame();
        }
        begin_ui_frame(&_render_state);
        update_runtime_widget(&first_entity, &first, &first_update)?;
        update_runtime_widget(&second_entity, &second, &second_update)?;

        // Programmatic focus uses the same exclusive owner as mouse and Tab.
        crate::widget_interaction::request_focus(
            interaction_surface(&_render_state),
            &component_owner_key(&first),
        );
        {
            let mut platform = lock_platform_state(&platform);
            platform.begin_frame();
            platform.input_mut().keys_pressed.insert("tab".into());
            platform.input_mut().last_key_pressed = Some("tab".into());
        }
        begin_ui_frame(&_render_state);
        update_runtime_widget(&first_entity, &first, &first_update)?;
        update_runtime_widget(&second_entity, &second, &second_update)?;
        assert!(!first.get::<bool>("focused")?);
        assert!(second.get::<bool>("focused")?);

        {
            let mut platform = lock_platform_state(&platform);
            platform.begin_frame();
            platform.input_mut().keys_pressed.insert("enter".into());
            platform.input_mut().last_key_pressed = Some("enter".into());
        }
        begin_ui_frame(&_render_state);
        update_runtime_widget(&first_entity, &first, &first_update)?;
        update_runtime_widget(&second_entity, &second, &second_update)?;
        assert_eq!(clicks.load(std::sync::atomic::Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn runtime_scroll_list_supports_pointer_selection_and_keyboard_navigation() -> mlua::Result<()>
    {
        let (lua, platform, _render_state) = runtime_widget_fixture();
        let (entity, list, update) =
            make_runtime_widget(&lua, "ScrollList", 20.0, 20.0, 160.0, 80.0)?;
        list.set("padding", 0.0)?;
        list.set("padding_x", 0.0)?;
        list.set("padding_y", 0.0)?;
        list.set("border_width", 0.0)?;
        list.set("item_height", 20.0)?;
        list.set("item_spacing", 0.0)?;
        let options = lua.create_sequence_from(["one", "two", "three", "four", "five"])?;
        list.set("options", options)?;

        begin_ui_frame(&_render_state);
        update_runtime_widget(&entity, &list, &update)?;
        {
            let mut platform = lock_platform_state(&platform);
            platform.set_mouse_position(40.0, 50.0);
            platform.input_mut().mouse_pressed.insert("left".into());
            platform.input_mut().mouse_down.insert("left".into());
        }
        begin_ui_frame(&_render_state);
        update_runtime_widget(&entity, &list, &update)?;
        assert_eq!(list.get::<usize>("selected_index")?, 2);
        assert_eq!(list.get::<String>("selected_value")?, "two");
        assert!(list.get::<bool>("focused")?);

        {
            let mut platform = lock_platform_state(&platform);
            platform.begin_frame();
            platform.input_mut().mouse_down.remove("left");
            platform.input_mut().mouse_released.insert("left".into());
        }
        begin_ui_frame(&_render_state);
        update_runtime_widget(&entity, &list, &update)?;

        {
            let mut platform = lock_platform_state(&platform);
            platform.begin_frame();
            platform.input_mut().keys_pressed.insert("end".into());
            platform.input_mut().last_key_pressed = Some("end".into());
        }
        begin_ui_frame(&_render_state);
        update_runtime_widget(&entity, &list, &update)?;
        assert_eq!(list.get::<usize>("selected_index")?, 5);
        assert_eq!(list.get::<String>("selected_text")?, "five");
        assert_eq!(list.get::<usize>("scroll_index")?, 1);
        Ok(())
    }
}
