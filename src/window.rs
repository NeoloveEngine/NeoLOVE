use mlua::{Compiler, Function, Lua, RegistryKey, Table, TextRequirer, Value};
use rapier2d::prelude::{
    nalgebra, point, vector, CCDSolver, ColliderBuilder, ColliderHandle, ColliderSet,
    DefaultBroadPhase, ImpulseJointHandle, ImpulseJointSet, IntegrationParameters, IslandManager,
    MultibodyJointSet, NarrowPhase, PhysicsPipeline, RigidBodyBuilder, RigidBodyHandle,
    RigidBodySet, RopeJointBuilder,
};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use crate::hierarchy;
use crate::lua_error::{describe_lua_error, protect_lua_call};
use crate::platform::{
    new_shared_platform_state, Color as PlatformColor, SharedPlatformState, WindowState,
};
use crate::renderer::{new_shared_render_state, SharedRenderState};

pub struct Runtime {
    entities: Rc<RefCell<HashMap<hierarchy::EntityId, hierarchy::Entity>>>,
    entity_listeners: Rc<RefCell<HashMap<u64, EntityListener>>>,
    next_entity_listener_id: Rc<RefCell<u64>>,
    systems: Rc<RefCell<Vec<RegistryKey>>>,
    environment: PathBuf,
    lua: Lua,
    entity_max: usize,
    max_fps: Rc<RefCell<Option<f32>>>,
    show_fps: Rc<RefCell<bool>>,
    exit_requested: Rc<RefCell<bool>>,
    physics_world: Option<PhysicsWorld>,
    physics_signature: u64,
    platform: SharedPlatformState,
    render_state: SharedRenderState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum EntityListenEvent {
    LeftClick,
    RightClick,
    MiddleClick,
    ScrollUp,
    ScrollDown,
}

impl EntityListenEvent {
    fn from_name(raw: &str) -> Option<Self> {
        match raw
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .map(|ch| ch.to_ascii_lowercase())
            .collect::<String>()
            .as_str()
        {
            "leftclick" | "leftmouse" | "leftbutton" | "left" | "lmb" => Some(Self::LeftClick),
            "rightclick" | "rightmouse" | "rightbutton" | "right" | "rmb" => Some(Self::RightClick),
            "middleclick" | "middlemouse" | "middlebutton" | "middle" | "mmb" | "wheelclick" => {
                Some(Self::MiddleClick)
            }
            "scrollup" | "wheelup" => Some(Self::ScrollUp),
            "scrolldown" | "wheeldown" => Some(Self::ScrollDown),
            _ => None,
        }
    }

    fn kind(self) -> &'static str {
        match self {
            Self::LeftClick => "leftClick",
            Self::RightClick => "rightClick",
            Self::MiddleClick => "middleClick",
            Self::ScrollUp => "scrollUp",
            Self::ScrollDown => "scrollDown",
        }
    }

    fn button(self) -> Option<&'static str> {
        match self {
            Self::LeftClick => Some("left"),
            Self::RightClick => Some("right"),
            Self::MiddleClick => Some("middle"),
            Self::ScrollUp | Self::ScrollDown => None,
        }
    }
}

struct EntityListener {
    entity_id: usize,
    event: EntityListenEvent,
    callback: RegistryKey,
    connected: Rc<Cell<bool>>,
}

fn color4_table(lua: &Lua, r: u8, g: u8, b: u8, a: u8) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("r", r)?;
    t.set("g", g)?;
    t.set("b", b)?;
    t.set("a", a)?;
    Ok(t)
}

fn deep_copy_table(lua: &Lua, table: &Table) -> mlua::Result<Table> {
    let copy = lua.create_table()?;
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let copied_value = match value {
            Value::Table(t) => Value::Table(deep_copy_table(lua, &t)?),
            other => other,
        };
        copy.set(key, copied_value)?;
    }
    Ok(copy)
}

fn disconnect_entity_listener(
    lua: &Lua,
    listeners: &Rc<RefCell<HashMap<u64, EntityListener>>>,
    listener_id: u64,
) -> mlua::Result<bool> {
    let removed = listeners.borrow_mut().remove(&listener_id);
    let Some(listener) = removed else {
        return Ok(false);
    };
    listener.connected.set(false);
    lua.remove_registry_value(listener.callback)?;
    Ok(true)
}

fn disconnect_entity_listeners_for_entities(
    lua: &Lua,
    listeners: &Rc<RefCell<HashMap<u64, EntityListener>>>,
    entity_ids: &[usize],
) -> mlua::Result<()> {
    if entity_ids.is_empty() {
        return Ok(());
    }

    let entity_ids: HashSet<usize> = entity_ids.iter().copied().collect();
    let listener_ids: Vec<u64> = {
        let listeners = listeners.borrow();
        listeners
            .iter()
            .filter_map(|(listener_id, listener)| {
                entity_ids
                    .contains(&listener.entity_id)
                    .then_some(*listener_id)
            })
            .collect()
    };

    for listener_id in listener_ids {
        let _ = disconnect_entity_listener(lua, listeners, listener_id)?;
    }

    Ok(())
}

fn create_entity_listener_connection(
    lua: &Lua,
    listeners: Rc<RefCell<HashMap<u64, EntityListener>>>,
    listener_id: u64,
    connected: Rc<Cell<bool>>,
) -> mlua::Result<Table> {
    let connection = lua.create_table()?;

    let disconnect_listeners = listeners.clone();
    let disconnect_connected = connected.clone();
    let disconnect = lua.create_function(move |lua, _self: Table| {
        let removed = disconnect_entity_listener(lua, &disconnect_listeners, listener_id)?;
        if removed {
            disconnect_connected.set(false);
        }
        Ok(removed)
    })?;
    connection.set("Disconnect", disconnect.clone())?;
    connection.set("disconnect", disconnect)?;

    let connected_reader = connected;
    let is_connected = lua.create_function(move |_lua, _self: Table| Ok(connected_reader.get()))?;
    connection.set("IsConnected", is_connected.clone())?;
    connection.set("isConnected", is_connected)?;

    Ok(connection)
}

fn create_entity_listener_event(
    lua: &Lua,
    event: EntityListenEvent,
    mouse_x: f32,
    mouse_y: f32,
    wheel_x: f32,
    wheel_y: f32,
) -> mlua::Result<Table> {
    let payload = lua.create_table()?;
    payload.set("kind", event.kind())?;
    payload.set("type", event.kind())?;
    payload.set("x", mouse_x)?;
    payload.set("y", mouse_y)?;
    payload.set("mouseX", mouse_x)?;
    payload.set("mouseY", mouse_y)?;
    payload.set("wheelX", wheel_x)?;
    payload.set("wheelY", wheel_y)?;

    match event.button() {
        Some(button) => payload.set("button", button)?,
        None => payload.set("button", Value::Nil)?,
    }

    let amount = match event {
        EntityListenEvent::ScrollUp => wheel_y.max(0.0),
        EntityListenEvent::ScrollDown => (-wheel_y).max(0.0),
        EntityListenEvent::LeftClick
        | EntityListenEvent::RightClick
        | EntityListenEvent::MiddleClick => 0.0,
    };
    payload.set("amount", amount)?;

    Ok(payload)
}

fn describe_component_name(component: &Table, entity: Option<&Table>) -> String {
    if let Ok(name) = component.get::<String>("__neolove_component") {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    if let Ok(name) = component.get::<String>("name") {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            return format!("component '{trimmed}'");
        }
    }

    if let Some(entity) = entity {
        if let Ok(entity_name) = entity.get::<String>("name") {
            let trimmed = entity_name.trim();
            if !trimmed.is_empty() {
                return format!("anonymous component on entity '{trimmed}'");
            }
        }
    }

    "anonymous component".to_string()
}

pub(crate) fn attach_entity_methods(lua: &Lua, entity: &Table) -> mlua::Result<()> {
    let listen = lua.create_function(
        move |lua, (entity, event_name, callback): (Table, String, Function)| {
            let listen_impl: Function = lua
                .globals()
                .get("__neolove_entity_listen_impl")
                .map_err(|_| mlua::Error::external("entity listeners are unavailable"))?;
            listen_impl.call::<Table>((entity, event_name, callback))
        },
    )?;
    entity.set("listen", listen.clone())?;
    entity.set("Listen", listen)?;

    let delete = lua.create_function(move |lua, entity: Table| {
        let ecs: Table = lua.globals().get("ecs")?;
        let delete_entity: Function = ecs.get("deleteEntity")?;
        delete_entity.call::<()>(entity)
    })?;
    entity.set("delete", delete.clone())?;
    entity.set("Delete", delete)?;

    let add_component = lua.create_function(move |lua, (entity, component): (Table, Value)| {
        let ecs: Table = lua.globals().get("ecs")?;
        let add_component: Function = ecs.get("addComponent")?;
        add_component.call::<Value>((entity, component))
    })?;
    entity.set("addComponent", add_component.clone())?;
    entity.set("AddComponent", add_component)?;

    let remove_component = lua.create_function(move |lua, (entity, target): (Table, Value)| {
        let ecs: Table = lua.globals().get("ecs")?;
        let remove_component: Function = ecs.get("removeComponent")?;
        remove_component.call::<bool>((entity, target))
    })?;
    entity.set("removeComponent", remove_component.clone())?;
    entity.set("RemoveComponent", remove_component)?;

    let duplicate = lua.create_function(move |lua, (entity, parent): (Table, Option<Table>)| {
        let ecs: Table = lua.globals().get("ecs")?;
        let duplicate_entity: Function = ecs.get("duplicateEntity")?;
        let parent = match parent {
            Some(parent) => parent,
            None => entity
                .get::<Option<Table>>("parent")?
                .unwrap_or(ecs.get::<Table>("root")?),
        };
        duplicate_entity.call::<Table>((entity, parent))
    })?;
    entity.set("duplicate", duplicate.clone())?;
    entity.set("Duplicate", duplicate)?;

    let find_first_child = lua.create_function(move |lua, (entity, name): (Table, String)| {
        let ecs: Table = lua.globals().get("ecs")?;
        let find_first_child: Function = ecs.get("findFirstChild")?;
        find_first_child.call::<Option<Table>>((entity, name))
    })?;
    entity.set("findFirstChild", find_first_child.clone())?;
    entity.set("FindFirstChild", find_first_child)?;

    let get_world_position = lua.create_function(move |lua, entity: Table| {
        let transform: Table = lua.globals().get("transform")?;
        let get_world_position: Function = transform.get("getWorldPosition")?;
        get_world_position.call::<(f32, f32)>(entity)
    })?;
    entity.set("getWorldPosition", get_world_position.clone())?;
    entity.set("GetWorldPosition", get_world_position)?;

    let get_world_rotation = lua.create_function(move |lua, entity: Table| {
        let transform: Table = lua.globals().get("transform")?;
        let get_world_rotation: Function = transform.get("getWorldRotation")?;
        get_world_rotation.call::<f32>(entity)
    })?;
    entity.set("getWorldRotation", get_world_rotation.clone())?;
    entity.set("GetWorldRotation", get_world_rotation)?;

    Ok(())
}

pub(crate) fn attach_component_methods(lua: &Lua, component: &Table) -> mlua::Result<()> {
    let remove = lua.create_function(move |lua, component: Table| {
        let Some(entity) = component.get::<Option<Table>>("entity")? else {
            return Ok(false);
        };
        let ecs: Table = lua.globals().get("ecs")?;
        let remove_component: Function = ecs.get("removeComponent")?;
        remove_component.call::<bool>((entity, component))
    })?;
    component.set("remove", remove.clone())?;
    component.set("Remove", remove)?;

    let get_entity = lua
        .create_function(move |_lua, component: Table| component.get::<Option<Table>>("entity"))?;
    component.set("getEntity", get_entity.clone())?;
    component.set("GetEntity", get_entity)?;

    Ok(())
}

pub(crate) fn create_entity_table(
    lua: &Lua,
    name: &str,
    x: f64,
    y: f64,
    parent: Option<Table>,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("name", name)?;
    table.set("x", x)?;
    table.set("y", y)?;
    table.set("rotation", 0.0)?;
    table.set("rotation_pivot", "topleft")?;
    table.set("rotation_pivot_x", Value::Nil)?;
    table.set("rotation_pivot_y", Value::Nil)?;
    table.set("z", 0.0)?;
    table.set("size_x", 32.0)?;
    table.set("size_y", 32.0)?;
    table.set("scale", 1.0)?;
    table.set("anchor_x", 0.0)?;
    table.set("anchor_y", 0.0)?;
    table.set("pivot_x", Value::Nil)?;
    table.set("pivot_y", Value::Nil)?;
    table.set("components", lua.create_table()?)?;
    if let Some(par) = parent {
        table.set("parent", &par)?;
        let children: Table = par.get("children")?;
        children.push(&table)?;
    }
    table.set("children", lua.create_table()?)?;
    attach_entity_methods(lua, &table)?;
    Ok(table)
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

fn resolve_existing_softrequire_path(root: &Path, input: &str) -> Result<Option<PathBuf>, String> {
    let path = PathBuf::from(input);
    let candidate = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    let mut resolved = normalize_path(&candidate);
    if resolved.extension().is_none() && !resolved.exists() {
        let with_luau = resolved.with_extension("luau");
        if with_luau.exists() {
            resolved = with_luau;
        } else {
            let with_lua = resolved.with_extension("lua");
            if with_lua.exists() {
                resolved = with_lua;
            }
        }
    }
    if resolved.is_dir() {
        resolved = resolved.join("init.luau");
    }
    if !resolved.exists() {
        return Ok(None);
    }
    let canonical = fs::canonicalize(&resolved)
        .map_err(|e| format!("failed to resolve softrequire path '{}': {e}", input))?;
    if !canonical.starts_with(root) {
        return Err(format!("softrequire path escapes project root: {}", input));
    }
    Ok(Some(canonical))
}

fn softrequire_source_cache_key(source: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    format!("softrequire:text:{}:{}", source.len(), hasher.finish())
}

fn create_softrequire_sandbox(lua: &Lua, allowed: Option<Table>) -> mlua::Result<Table> {
    let globals = lua.globals();
    let sandbox = lua.create_table()?;
    sandbox.set("_G", sandbox.clone())?;

    for name in [
        "assert",
        "error",
        "getmetatable",
        "ipairs",
        "next",
        "pairs",
        "pcall",
        "rawequal",
        "rawget",
        "rawlen",
        "rawset",
        "select",
        "setmetatable",
        "tonumber",
        "tostring",
        "type",
        "unpack",
        "xpcall",
    ] {
        if let Ok(value) = globals.get::<Value>(name) {
            sandbox.set(name, value)?;
        }
    }

    for lib in ["math", "string", "table", "utf8"] {
        if let Ok(value) = globals.get::<Value>(lib) {
            sandbox.set(lib, value)?;
        }
    }

    if let Some(allowed) = allowed {
        for pair in allowed.pairs::<Value, Value>() {
            let (key, value) = pair?;
            match (key, value) {
                (Value::Integer(_), Value::String(name)) => {
                    if let Ok(name) = name.to_str() {
                        let name = name.to_string();
                        if let Ok(global_value) = globals.get::<Value>(name.as_str()) {
                            if !matches!(global_value, Value::Nil) {
                                sandbox.set(name, global_value)?;
                            }
                        }
                    }
                }
                (Value::String(name), value) => {
                    if let Ok(name) = name.to_str() {
                        sandbox.set(name, value)?;
                    }
                }
                _ => {}
            }
        }
    }

    Ok(sandbox)
}

fn load_softrequire_chunk(
    lua: &Lua,
    source: &str,
    chunk_name: &str,
    allowed: Option<Table>,
) -> mlua::Result<Function> {
    let sandbox = create_softrequire_sandbox(lua, allowed)?;
    lua.load(source)
        .set_name(chunk_name.to_string())
        .set_environment(sandbox)
        .into_function()
}

fn rotate_point(x: f32, y: f32, rotation: f32) -> (f32, f32) {
    let cos_r = rotation.cos();
    let sin_r = rotation.sin();
    (x * cos_r - y * sin_r, x * sin_r + y * cos_r)
}

fn collect_ignored_ids(value: Value, ignored_ids: &mut HashSet<usize>) -> mlua::Result<()> {
    match value {
        Value::Table(table) => {
            if let Ok(id) = table.get::<usize>("id") {
                ignored_ids.insert(id);
                return Ok(());
            }

            for item in table.sequence_values::<Value>() {
                collect_ignored_ids(item?, ignored_ids)?;
            }
        }
        Value::Nil => {}
        _ => {}
    }

    Ok(())
}

fn raycast_aabb(
    origin_x: f32,
    origin_y: f32,
    dir_x: f32,
    dir_y: f32,
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
    max_distance: f32,
) -> Option<(f32, f32, f32, f32, f32)> {
    let mut t_min = 0.0f32;
    let mut t_max = max_distance;

    if dir_x.abs() < f32::EPSILON {
        if origin_x < min_x || origin_x > max_x {
            return None;
        }
    } else {
        let inv_x = 1.0 / dir_x;
        let mut tx1 = (min_x - origin_x) * inv_x;
        let mut tx2 = (max_x - origin_x) * inv_x;
        if tx1 > tx2 {
            std::mem::swap(&mut tx1, &mut tx2);
        }
        t_min = t_min.max(tx1);
        t_max = t_max.min(tx2);
        if t_max < t_min {
            return None;
        }
    }

    if dir_y.abs() < f32::EPSILON {
        if origin_y < min_y || origin_y > max_y {
            return None;
        }
    } else {
        let inv_y = 1.0 / dir_y;
        let mut ty1 = (min_y - origin_y) * inv_y;
        let mut ty2 = (max_y - origin_y) * inv_y;
        if ty1 > ty2 {
            std::mem::swap(&mut ty1, &mut ty2);
        }
        t_min = t_min.max(ty1);
        t_max = t_max.min(ty2);
        if t_max < t_min {
            return None;
        }
    }

    let distance = t_min;
    if !distance.is_finite() || distance < 0.0 || distance > max_distance {
        return None;
    }

    let hit_x = origin_x + dir_x * distance;
    let hit_y = origin_y + dir_y * distance;
    let eps = 0.01f32;
    let (mut normal_x, mut normal_y) = (0.0f32, 0.0f32);

    if (hit_x - min_x).abs() <= eps {
        normal_x = -1.0;
    } else if (hit_x - max_x).abs() <= eps {
        normal_x = 1.0;
    } else if (hit_y - min_y).abs() <= eps {
        normal_y = -1.0;
    } else if (hit_y - max_y).abs() <= eps {
        normal_y = 1.0;
    }

    Some((distance, hit_x, hit_y, normal_x, normal_y))
}

fn uses_middle_rotation_pivot(entity: &Table) -> bool {
    if let Ok(pivot) = entity.get::<String>("rotation_pivot") {
        let pivot = pivot.to_ascii_lowercase();
        return pivot == "middle" || pivot == "center";
    }

    if let Ok(pivot) = entity.get::<String>("rotationPivot") {
        let pivot = pivot.to_ascii_lowercase();
        return pivot == "middle" || pivot == "center";
    }

    entity.get::<bool>("rotation_pivot_middle").unwrap_or(false)
}

fn read_entity_scale(entity: &Table) -> f32 {
    let scale = entity.get::<f32>("scale").unwrap_or(1.0);
    if scale.is_finite() {
        scale
    } else {
        1.0
    }
}

fn read_optional_f32(entity: &Table, snake_case: &str, camel_case: &str) -> Option<f32> {
    entity
        .get::<f32>(snake_case)
        .or_else(|_| entity.get::<f32>(camel_case))
        .ok()
        .filter(|value| value.is_finite())
}

fn get_local_anchor_offset(entity: &Table) -> mlua::Result<(f32, f32)> {
    let anchor_x = read_optional_f32(entity, "anchor_x", "anchorX").unwrap_or(0.0);
    let anchor_y = read_optional_f32(entity, "anchor_y", "anchorY").unwrap_or(0.0);
    if anchor_x == 0.0 && anchor_y == 0.0 {
        return Ok((0.0, 0.0));
    }

    let Some(parent) = entity.get::<Option<Table>>("parent")? else {
        return Ok((0.0, 0.0));
    };

    let parent_w: f32 = parent.get("size_x")?;
    let parent_h: f32 = parent.get("size_y")?;
    Ok((parent_w * anchor_x, parent_h * anchor_y))
}

fn get_local_position_pivot_offset(entity: &Table, local_scale: f32) -> mlua::Result<(f32, f32)> {
    let w: f32 = entity.get("size_x")?;
    let h: f32 = entity.get("size_y")?;
    let scale = local_scale.max(0.0);

    let pivot_x = read_optional_f32(entity, "pivot_x", "pivotX")
        .or_else(|| read_optional_f32(entity, "position_pivot_x", "positionPivotX"));
    let pivot_y = read_optional_f32(entity, "pivot_y", "pivotY")
        .or_else(|| read_optional_f32(entity, "position_pivot_y", "positionPivotY"));
    if pivot_x.is_some() || pivot_y.is_some() {
        return Ok((
            w * scale * pivot_x.unwrap_or(0.0),
            h * scale * pivot_y.unwrap_or(0.0),
        ));
    }

    let pivot = entity
        .get::<String>("position_pivot")
        .or_else(|_| entity.get::<String>("positionPivot"))
        .unwrap_or_default()
        .to_ascii_lowercase();

    match pivot.as_str() {
        "center" => Ok((w * scale * 0.5, h * scale * 0.5)),
        "top_right" | "topright" => Ok((w * scale, 0.0)),
        _ => Ok((0.0, 0.0)),
    }
}

fn get_local_rotation_pivot(entity: &Table, local_scale: f32) -> mlua::Result<(f32, f32)> {
    let w: f32 = entity.get("size_x")?;
    let h: f32 = entity.get("size_y")?;
    let scale = local_scale.max(0.0);
    let pivot_x = read_optional_f32(entity, "rotation_pivot_x", "rotationPivotX")
        .or_else(|| read_optional_f32(entity, "pivot_x", "pivotX"));
    let pivot_y = read_optional_f32(entity, "rotation_pivot_y", "rotationPivotY")
        .or_else(|| read_optional_f32(entity, "pivot_y", "pivotY"));
    if pivot_x.is_some() || pivot_y.is_some() {
        return Ok((
            w * scale * pivot_x.unwrap_or(0.0),
            h * scale * pivot_y.unwrap_or(0.0),
        ));
    }

    if uses_middle_rotation_pivot(entity) {
        return Ok((w * scale * 0.5, h * scale * 0.5));
    }
    Ok((0.0, 0.0))
}

pub fn get_global_scale(entity: &Table) -> mlua::Result<f32> {
    let mut chain = Vec::<Table>::new();
    let mut current_entity = entity.clone();

    loop {
        chain.push(current_entity.clone());

        if let Ok(Some(parent)) = current_entity.get::<Option<Table>>("parent") {
            current_entity = parent;
        } else {
            break;
        }
    }

    let mut scale = 1.0f32;
    for current in chain.into_iter().rev() {
        scale *= read_entity_scale(&current);
    }
    Ok(scale.max(0.0))
}

pub fn get_global_size(entity: &Table) -> mlua::Result<(f32, f32)> {
    let w: f32 = entity.get("size_x")?;
    let h: f32 = entity.get("size_y")?;
    let scale = get_global_scale(entity)?;
    Ok((w * scale, h * scale))
}

pub fn get_global_transform(entity: &Table) -> mlua::Result<(f32, f32, f32)> {
    let mut chain = Vec::<Table>::new();
    let mut current_entity = entity.clone();

    loop {
        chain.push(current_entity.clone());

        if let Ok(Some(parent)) = current_entity.get::<Option<Table>>("parent") {
            current_entity = parent;
        } else {
            break;
        }
    }

    let mut world_x = 0.0f32;
    let mut world_y = 0.0f32;
    let mut world_rotation = 0.0f32;
    let mut world_scale = 1.0f32;

    for current in chain.into_iter().rev() {
        let parent_scale = world_scale.max(0.0);
        let local_scale = read_entity_scale(&current).max(0.0);
        let local_x: f32 = current.get("x")?;
        let local_y: f32 = current.get("y")?;
        let (anchor_x, anchor_y) = get_local_anchor_offset(&current)?;
        let (pos_pivot_x, pos_pivot_y) = get_local_position_pivot_offset(&current, local_scale)?;
        let local_origin_x = anchor_x + local_x - pos_pivot_x;
        let local_origin_y = anchor_y + local_y - pos_pivot_y;
        let local_rotation: f32 = current.get("rotation").unwrap_or(0.0);
        let (pivot_x, pivot_y) = get_local_rotation_pivot(&current, local_scale)?;
        let (rp_x, rp_y) = rotate_point(pivot_x, pivot_y, local_rotation);
        let origin_shift_x = (local_origin_x + pivot_x - rp_x) * parent_scale;
        let origin_shift_y = (local_origin_y + pivot_y - rp_y) * parent_scale;

        let (rx, ry) = rotate_point(origin_shift_x, origin_shift_y, world_rotation);
        world_x += rx;
        world_y += ry;
        world_rotation += local_rotation;
        world_scale = parent_scale * local_scale;
    }

    Ok((world_x, world_y, world_rotation))
}

pub fn get_global_position(entity: &Table) -> mlua::Result<(f32, f32)> {
    let (x, y, _) = get_global_transform(entity)?;
    Ok((x, y))
}

pub fn get_global_rotation(entity: &Table) -> mlua::Result<f32> {
    let (_, _, r) = get_global_transform(entity)?;
    Ok(r)
}

pub fn uses_middle_pivot(entity: &Table) -> bool {
    uses_middle_rotation_pivot(entity)
}

pub fn get_global_rotation_pivot(entity: &Table) -> mlua::Result<(f32, f32)> {
    let (x, y, r) = get_global_transform(entity)?;
    let (px, py) = if uses_middle_rotation_pivot(entity) {
        let (w, h) = get_global_size(entity)?;
        (w * 0.5, h * 0.5)
    } else {
        (0.0, 0.0)
    };
    let (rx, ry) = rotate_point(px, py, r);
    Ok((x + rx, y + ry))
}

fn get_listener_rotation_pivot(entity: &Table) -> mlua::Result<(f32, f32)> {
    let (x, y, rotation) = get_global_transform(entity)?;
    let (width, height) = get_global_size(entity)?;
    let pivot_x = read_optional_f32(entity, "rotation_pivot_x", "rotationPivotX")
        .or_else(|| read_optional_f32(entity, "pivot_x", "pivotX"))
        .unwrap_or(if uses_middle_rotation_pivot(entity) {
            0.5
        } else {
            0.0
        });
    let pivot_y = read_optional_f32(entity, "rotation_pivot_y", "rotationPivotY")
        .or_else(|| read_optional_f32(entity, "pivot_y", "pivotY"))
        .unwrap_or(if uses_middle_rotation_pivot(entity) {
            0.5
        } else {
            0.0
        });
    let (offset_x, offset_y) = rotate_point(width * pivot_x, height * pivot_y, rotation);
    Ok((x + offset_x, y + offset_y))
}

fn point_hits_entity(entity: &Table, point_x: f32, point_y: f32) -> mlua::Result<bool> {
    let (_, _, rotation) = get_global_transform(entity)?;
    let (width, height) = get_global_size(entity)?;
    if width <= 0.0 || height <= 0.0 {
        return Ok(false);
    }

    let pivot_x_fraction = read_optional_f32(entity, "rotation_pivot_x", "rotationPivotX")
        .or_else(|| read_optional_f32(entity, "pivot_x", "pivotX"))
        .unwrap_or(if uses_middle_rotation_pivot(entity) {
            0.5
        } else {
            0.0
        });
    let pivot_y_fraction = read_optional_f32(entity, "rotation_pivot_y", "rotationPivotY")
        .or_else(|| read_optional_f32(entity, "pivot_y", "pivotY"))
        .unwrap_or(if uses_middle_rotation_pivot(entity) {
            0.5
        } else {
            0.0
        });
    let (pivot_x, pivot_y) = get_listener_rotation_pivot(entity)?;
    let bounds_x = pivot_x - width * pivot_x_fraction;
    let bounds_y = pivot_y - height * pivot_y_fraction;
    let (rotated_x, rotated_y) = rotate_point(point_x - pivot_x, point_y - pivot_y, -rotation);
    let sample_x = pivot_x + rotated_x;
    let sample_y = pivot_y + rotated_y;

    Ok(sample_x >= bounds_x
        && sample_x <= bounds_x + width
        && sample_y >= bounds_y
        && sample_y <= bounds_y + height)
}

fn compare_entity_order(a_z: f64, a_id: usize, b_z: f64, b_id: usize) -> std::cmp::Ordering {
    match a_z.partial_cmp(&b_z).unwrap_or(std::cmp::Ordering::Equal) {
        std::cmp::Ordering::Equal => a_id.cmp(&b_id),
        other => other,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TriangleCorner {
    BottomLeft,
    BottomRight,
    TopLeft,
    TopRight,
}

#[derive(Clone, Copy)]
enum ColliderShape {
    Box,
    Circle,
    RightTriangle(TriangleCorner),
}

fn parse_triangle_corner(raw: &str) -> TriangleCorner {
    match raw.to_ascii_lowercase().as_str() {
        "bl" | "bottomleft" | "leftbottom" => TriangleCorner::BottomLeft,
        "br" | "bottomright" | "rightbottom" => TriangleCorner::BottomRight,
        "tl" | "topleft" | "lefttop" => TriangleCorner::TopLeft,
        "tr" | "topright" | "righttop" => TriangleCorner::TopRight,
        _ => TriangleCorner::BottomLeft,
    }
}

fn parse_collider_shape(raw_shape: &str, raw_corner: &str) -> ColliderShape {
    match raw_shape.to_ascii_lowercase().as_str() {
        "circle" => ColliderShape::Circle,
        "triangle" | "right_triangle" | "righttriangle" | "rightangledtriangle" => {
            ColliderShape::RightTriangle(parse_triangle_corner(raw_corner))
        }
        _ => ColliderShape::Box,
    }
}

struct RapierBodySync {
    entity_id: usize,
    entity: Table,
    rigidbody: Option<Table>,
    body_handle: RigidBodyHandle,
    size_x: f32,
    size_y: f32,
    is_static: bool,
}

struct RapierColliderSync {
    entity_id: usize,
    collider: Table,
    is_trigger: bool,
}

struct RapierRopeSync {
    rope: Table,
    body_a: RigidBodyHandle,
    body_b: RigidBodyHandle,
    joint_handle: ImpulseJointHandle,
}

struct PhysicsWorld {
    islands: IslandManager,
    broad_phase: DefaultBroadPhase,
    narrow_phase: NarrowPhase,
    bodies: RigidBodySet,
    colliders: ColliderSet,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    ccd_solver: CCDSolver,
    body_sync: Vec<RapierBodySync>,
    collider_sync: Vec<RapierColliderSync>,
    collider_map: HashMap<ColliderHandle, usize>,
    body_by_entity_id: HashMap<usize, RigidBodyHandle>,
    entity_by_id: HashMap<usize, Table>,
}

struct EntityPhysicsInfo {
    entity_id: usize,
    entity: Table,
    rigidbody: Option<Table>,
    collider: Option<Table>,
    ropes: Vec<Table>,
}

fn extract_physics_components(
    components: &Table,
) -> mlua::Result<(Option<Table>, Option<Table>, Vec<Table>)> {
    let mut rigidbody: Option<Table> = None;
    let mut collider: Option<Table> = None;
    let mut ropes: Vec<Table> = Vec::new();

    for component in components.sequence_values::<Table>() {
        let component = match component {
            Ok(value) => value,
            Err(_) => continue,
        };
        let tag = component
            .get::<String>("__neolove_component")
            .ok()
            .unwrap_or_default();
        match tag.as_str() {
            "Rigidbody2D" => {
                if rigidbody.is_none() {
                    rigidbody = Some(component);
                }
            }
            "Collider2D" => {
                if collider.is_none() {
                    collider = Some(component);
                }
            }
            "Rope2D" => ropes.push(component),
            _ => {}
        }
    }

    Ok((rigidbody, collider, ropes))
}

fn physics_topology_signature(physics_infos: &[EntityPhysicsInfo]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    physics_infos.len().hash(&mut hasher);

    for info in physics_infos {
        info.entity_id.hash(&mut hasher);

        if let Ok(size_x) = info.entity.get::<f32>("size_x") {
            size_x.to_bits().hash(&mut hasher);
        }
        if let Ok(size_y) = info.entity.get::<f32>("size_y") {
            size_y.to_bits().hash(&mut hasher);
        }
        get_global_scale(&info.entity)
            .unwrap_or(1.0)
            .to_bits()
            .hash(&mut hasher);

        info.rigidbody.is_some().hash(&mut hasher);
        if let Some(rb) = info.rigidbody.as_ref() {
            rb.get::<bool>("is_static")
                .unwrap_or(true)
                .hash(&mut hasher);
            rb.get::<bool>("freeze_x")
                .unwrap_or(false)
                .hash(&mut hasher);
            rb.get::<bool>("freeze_y")
                .unwrap_or(false)
                .hash(&mut hasher);
            rb.get::<bool>("freeze_rotation")
                .unwrap_or(false)
                .hash(&mut hasher);
            rb.get::<bool>("collision_enabled")
                .unwrap_or(true)
                .hash(&mut hasher);
        }

        info.collider.is_some().hash(&mut hasher);
        if let Some(collider) = info.collider.as_ref() {
            collider
                .get::<bool>("enabled")
                .unwrap_or(true)
                .hash(&mut hasher);
            collider
                .get::<bool>("is_trigger")
                .unwrap_or(false)
                .hash(&mut hasher);
            collider
                .get::<bool>("non_physics")
                .unwrap_or(false)
                .hash(&mut hasher);
            collider
                .get::<String>("shape")
                .unwrap_or_else(|_| "box".to_string())
                .hash(&mut hasher);
            collider
                .get::<String>("triangle_corner")
                .unwrap_or_else(|_| "bl".to_string())
                .hash(&mut hasher);
            collider
                .get::<f32>("offset_x")
                .unwrap_or(0.0)
                .to_bits()
                .hash(&mut hasher);
            collider
                .get::<f32>("offset_y")
                .unwrap_or(0.0)
                .to_bits()
                .hash(&mut hasher);
            collider
                .get::<f32>("size_x")
                .unwrap_or(0.0)
                .to_bits()
                .hash(&mut hasher);
            collider
                .get::<f32>("size_y")
                .unwrap_or(0.0)
                .to_bits()
                .hash(&mut hasher);
        }
    }

    hasher.finish()
}

fn triangle_local_points(
    corner: TriangleCorner,
    entity_w: f32,
    entity_h: f32,
    offset_x: f32,
    offset_y: f32,
    collider_w: f32,
    collider_h: f32,
) -> ((f32, f32), (f32, f32), (f32, f32)) {
    let x0 = offset_x - entity_w * 0.5;
    let y0 = offset_y - entity_h * 0.5;
    let x1 = x0 + collider_w;
    let y1 = y0 + collider_h;

    match corner {
        TriangleCorner::BottomLeft => ((x0, y1), (x0, y0), (x1, y1)),
        TriangleCorner::BottomRight => ((x1, y1), (x1, y0), (x0, y1)),
        TriangleCorner::TopLeft => ((x0, y0), (x1, y0), (x0, y1)),
        TriangleCorner::TopRight => ((x1, y0), (x0, y0), (x1, y1)),
    }
}

fn read_id_set_from_table(table: &Table) -> mlua::Result<HashSet<usize>> {
    let mut ids = HashSet::new();
    for pair in table.pairs::<Value, Value>() {
        let (key, _) = pair?;
        match key {
            Value::Integer(i) if i > 0 => {
                ids.insert(i as usize);
            }
            Value::Number(n) if n.is_finite() && n >= 1.0 && n.fract() == 0.0 => {
                ids.insert(n as usize);
            }
            _ => {}
        }
    }
    Ok(ids)
}

fn write_id_set_to_table(lua: &Lua, ids: &HashSet<usize>) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    for id in ids {
        table.set(*id, true)?;
    }
    Ok(table)
}

impl Runtime {
    pub fn new(env: PathBuf) -> Runtime {
        Runtime {
            entities: Rc::new(RefCell::new(HashMap::new())),
            entity_listeners: Rc::new(RefCell::new(HashMap::new())),
            next_entity_listener_id: Rc::new(RefCell::new(1)),
            systems: Rc::new(RefCell::new(Vec::new())),
            environment: env,
            lua: Lua::new(),
            entity_max: 1,
            // default to uncapped; users can opt into a cap via app.setMaxFps
            max_fps: Rc::new(RefCell::new(None)),
            // default to showing fps counter in debug runs
            show_fps: Rc::new(RefCell::new(true)),
            exit_requested: Rc::new(RefCell::new(false)),
            physics_world: None,
            physics_signature: 0,
            platform: new_shared_platform_state(),
            render_state: new_shared_render_state(),
        }
    }

    pub(crate) fn platform_state(&self) -> SharedPlatformState {
        self.platform.clone()
    }

    pub(crate) fn render_state(&self) -> SharedRenderState {
        self.render_state.clone()
    }

    pub fn set_platform_window_state(&self, width: f32, height: f32) {
        if let Ok(mut platform) = self.platform.lock() {
            platform.set_window(WindowState { width, height });
        }
    }

    pub fn set_platform_mouse_state(&self, x: f32, y: f32) {
        if let Ok(mut platform) = self.platform.lock() {
            platform.set_mouse_position(x, y);
        }
    }

    pub fn max_fps(&self) -> Option<f32> {
        *self.max_fps.borrow()
    }

    pub fn show_fps(&self) -> bool {
        *self.show_fps.borrow()
    }

    pub fn exit_requested(&self) -> bool {
        *self.exit_requested.borrow()
    }

    fn set_mouse_table(&mut self) -> mlua::Result<()> {
        let mouse = self
            .platform
            .lock()
            .map_err(|_| mlua::Error::external("platform lock poisoned"))?
            .mouse();
        let globals = self.lua.globals();
        if let Ok(mouse_table) = globals.get::<Table>("mouse") {
            mouse_table.set("x", mouse.x)?;
            mouse_table.set("y", mouse.y)?;
        } else {
            let mouse_table = self.lua.create_table()?;
            mouse_table.set("x", mouse.x)?;
            mouse_table.set("y", mouse.y)?;
            globals.set("mouse", mouse_table)?;
        }
        Ok(())
    }

    fn set_window_table(&mut self) -> mlua::Result<()> {
        let window = self
            .platform
            .lock()
            .map_err(|_| mlua::Error::external("platform lock poisoned"))?
            .window();
        let globals = self.lua.globals();
        if let Ok(table) = globals.get::<Table>("window") {
            table.set("x", window.width)?;
            table.set("y", window.height)?;
        } else {
            let table = self.lua.create_table()?;
            table.set("x", window.width)?;
            table.set("y", window.height)?;
            globals.set("window", table)?;
        }

        if let Some(root_entity) = self.entities.borrow().get(&0) {
            let root: Table = self.lua.registry_value(&root_entity.luau_key)?;
            root.set("size_x", window.width)?;
            root.set("size_y", window.height)?;
        }
        Ok(())
    }

    pub fn start(&mut self) -> mlua::Result<()> {
        self.lua.set_compiler(
            Compiler::new()
                .set_optimization_level(2)
                .set_debug_level(1)
                .set_type_info_level(1),
        );

        let require = self.lua.create_require_function(TextRequirer::new())?;
        self.lua.globals().set("require", require)?;

        self.set_mouse_table()?;
        self.set_window_table()?;

        // App
        {
            let app = self.lua.create_table()?;
            app.set("bg", color4_table(&self.lua, 255, 255, 255, 255)?)?;
            app.set("nearestNeighborScaling", true)?;
            if let Ok(mut platform) = self.platform.lock() {
                platform.set_clear_color(PlatformColor::WHITE);
            }

            let max_fps_setter = self.max_fps.clone();
            let set_max_fps = self.lua.create_function(move |_lua, fps: Option<f32>| {
                let mut max_fps = max_fps_setter.borrow_mut();
                match fps {
                    Some(fps) if fps.is_finite() && fps > 0.0 => *max_fps = Some(fps),
                    _ => *max_fps = None,
                }
                Ok(())
            })?;
            app.set("setMaxFps", set_max_fps)?;

            let max_fps_getter = self.max_fps.clone();
            let get_max_fps = self
                .lua
                .create_function(move |_lua, ()| Ok(*max_fps_getter.borrow()))?;
            app.set("getMaxFps", get_max_fps)?;

            let show_fps_setter = self.show_fps.clone();
            let set_show_fps = self
                .lua
                .create_function(move |_lua, enabled: Option<bool>| {
                    *show_fps_setter.borrow_mut() = enabled.unwrap_or(true);
                    Ok(())
                })?;
            app.set("setShowFps", set_show_fps)?;

            let show_fps_getter = self.show_fps.clone();
            let get_show_fps = self
                .lua
                .create_function(move |_lua, ()| Ok(*show_fps_getter.borrow()))?;
            app.set("getShowFps", get_show_fps)?;

            let set_nearest_neighbor_scaling =
                self.lua
                    .create_function(move |lua, enabled: Option<bool>| {
                        let app: Table = lua.globals().get("app")?;
                        app.set("nearestNeighborScaling", enabled.unwrap_or(true))?;
                        Ok(())
                    })?;
            app.set("setNearestNeighborScaling", set_nearest_neighbor_scaling)?;

            let get_nearest_neighbor_scaling = self.lua.create_function(move |lua, ()| {
                let app: Table = lua.globals().get("app")?;
                Ok(app.get::<bool>("nearestNeighborScaling").unwrap_or(true))
            })?;
            app.set("getNearestNeighborScaling", get_nearest_neighbor_scaling)?;

            self.lua.globals().set("app", app)?;
        }

        let env_root = self
            .environment
            .canonicalize()
            .map_err(mlua::Error::external)?;

        {
            let softrequire_root = env_root.clone();
            let softrequire_cache = Rc::new(RefCell::new(HashMap::<String, RegistryKey>::new()));
            let softrequire = self.lua.create_function(
                move |lua, (module_input, allowed): (String, Option<Table>)| {
                    if let Some(path) = resolve_existing_softrequire_path(
                        &softrequire_root,
                        &module_input,
                    )
                    .map_err(mlua::Error::external)?
                    {
                        let path_key = path.to_string_lossy().to_string();

                        {
                            let cache = softrequire_cache.borrow();
                            if let Some(registry_key) = cache.get(&path_key) {
                                let cached: Value = lua.registry_value(registry_key)?;
                                return Ok(cached);
                            }
                        }

                        let source = fs::read_to_string(&path).map_err(mlua::Error::external)?;
                        let function = load_softrequire_chunk(
                            lua,
                            source.as_str(),
                            &format!("@{}", path.display()),
                            allowed,
                        )?;
                        let result: Value = function.call(())?;

                        let registry_key = lua.create_registry_value(result.clone())?;
                        softrequire_cache
                            .borrow_mut()
                            .insert(path_key, registry_key);
                        return Ok(result);
                    }

                    let source_key = softrequire_source_cache_key(&module_input);
                    {
                        let cache = softrequire_cache.borrow();
                        if let Some(registry_key) = cache.get(&source_key) {
                            let cached: Value = lua.registry_value(registry_key)?;
                            return Ok(cached);
                        }
                    }

                    let chunk_name = format!("@<{}>", source_key);
                    let function = match load_softrequire_chunk(
                        lua,
                        module_input.as_str(),
                        chunk_name.as_str(),
                        allowed,
                    ) {
                        Ok(function) => function,
                        Err(error) => {
                            return Err(mlua::Error::external(format!(
                                "softrequire could not resolve the input as a project module path, and inline source compilation failed: {error}"
                            )));
                        }
                    };
                    let result: Value = function.call(())?;

                    let registry_key = lua.create_registry_value(result.clone())?;
                    softrequire_cache
                        .borrow_mut()
                        .insert(source_key, registry_key);
                    Ok(result)
                },
            )?;
            self.lua.globals().set("softrequire", softrequire)?;
        }

        crate::user_input::add_user_input_module(&self.lua, self.platform.clone())?;
        crate::audio_system::add_audio_module(&self.lua)?;
        crate::assets::add_assets_module(&self.lua, env_root.clone())?;
        crate::fs_module::add_fs_module(&self.lua, env_root.clone())?;
        crate::http::add_http_module(&self.lua)?;
        crate::servers::add_servers_module(&self.lua, env_root.clone())?;
        crate::commands::add_commands_module(&self.lua, env_root.clone())?;
        crate::shader::add_shader_module(&self.lua, env_root.clone())?;

        let entry_file = env_root.join("main.luau");

        let entry_parent = entry_file
            .parent()
            .ok_or_else(|| mlua::Error::external("main.luau has no parent dir"))?;
        let entry_stem = entry_file
            .file_stem()
            .ok_or_else(|| mlua::Error::external("main.luau has no file_stem"))?;
        let entry_module = entry_parent.join(entry_stem);

        let ecs = self.lua.create_table()?;
        let transforms = self.lua.create_table()?;

        let exit_requested = self.exit_requested.clone();
        let die = self.lua.create_function(move |_lua, ()| {
            *exit_requested.borrow_mut() = true;
            Ok(())
        })?;

        self.lua.globals().set("die", die)?;

        let listener_state = self.entity_listeners.clone();
        let next_listener_id = self.next_entity_listener_id.clone();
        let listen_impl = self.lua.create_function(
            move |lua, (entity, event_name, callback): (Table, String, Function)| {
                let event = EntityListenEvent::from_name(&event_name).ok_or_else(|| {
                    mlua::Error::external(
                        "entity listen event must be one of leftClick, rightClick, middleClick, scrollUp, or scrollDown",
                    )
                })?;
                let entity_id = entity
                    .get::<usize>("id")
                    .map_err(|_| mlua::Error::external("entity listener target has no id"))?;
                let listener_id = {
                    let mut next_listener_id = next_listener_id.borrow_mut();
                    let listener_id = *next_listener_id;
                    *next_listener_id = next_listener_id.saturating_add(1);
                    listener_id
                };
                let connected = Rc::new(Cell::new(true));
                let callback_key = lua.create_registry_value(callback)?;

                listener_state.borrow_mut().insert(
                    listener_id,
                    EntityListener {
                        entity_id,
                        event,
                        callback: callback_key,
                        connected: connected.clone(),
                    },
                );

                create_entity_listener_connection(
                    lua,
                    listener_state.clone(),
                    listener_id,
                    connected,
                )
            },
        )?;
        self.lua
            .globals()
            .set("__neolove_entity_listen_impl", listen_impl)?;

        // Transforms
        {
            let get_world_position = self.lua.create_function(move |_lua, entity: Table| {
                let (x, y) = get_global_position(&entity)?;
                Ok((x, y))
            })?;
            let get_world_rotation = self.lua.create_function(move |_lua, entity: Table| {
                let rotation = get_global_rotation(&entity)?;
                Ok(rotation)
            })?;

            let do_they_overlap = self.lua.create_function(move |_lua, entities: Table| {
                // go through the entities and see if one overlaps with any of them
                // if so, then return true
                // otherwise, false

                for pair1 in entities.pairs::<Value, Table>() {
                    let (_, entity1) = pair1?;
                    for pair2 in entities.pairs::<Value, Table>() {
                        let (_, entity2) = pair2?;
                        if entity1 == entity2 {
                            continue;
                        }

                        let (x1, y1) = get_global_position(&entity1)?;
                        let (w1, h1) = get_global_size(&entity1)?;

                        let (x2, y2) = get_global_position(&entity2)?;
                        let (w2, h2) = get_global_size(&entity2)?;

                        if x1 < x2 + w2 && x1 + w1 > x2 && y1 < y2 + h2 && y1 + h1 > y2 {
                            return Ok(true);
                        }
                    }
                }

                Ok(false)
            })?;

            let raycast_entities = self.entities.clone();
            let raycast = self.lua.create_function(
                move |lua,
                      (origin_x, origin_y, dir_x, dir_y, max_distance, options): (
                    f32,
                    f32,
                    f32,
                    f32,
                    Option<f32>,
                    Option<Table>,
                )| {
                    let direction_len_sq = dir_x * dir_x + dir_y * dir_y;
                    if direction_len_sq <= f32::EPSILON || !direction_len_sq.is_finite() {
                        return Ok(None::<Table>);
                    }

                    let direction_len = direction_len_sq.sqrt();
                    let ray_x = dir_x / direction_len;
                    let ray_y = dir_y / direction_len;
                    let max_distance = max_distance
                        .unwrap_or(f32::INFINITY)
                        .max(0.0)
                        .min(1_000_000.0);

                    let mut ignored_ids: HashSet<usize> = HashSet::new();
                    if let Some(options) = options {
                        if let Ok(ignore_value) = options.get::<Value>("ignore") {
                            collect_ignored_ids(ignore_value, &mut ignored_ids)?;
                        }
                        if let Ok(ignore_value) = options.get::<Value>("ignoreEntity") {
                            collect_ignored_ids(ignore_value, &mut ignored_ids)?;
                        }
                    }

                    let mut best_hit: Option<(Table, f32, f32, f32, f32, f32)> = None;
                    let entities = raycast_entities.borrow();
                    for (id, entity_data) in entities.iter() {
                        if *id == 0 || ignored_ids.contains(id) {
                            continue;
                        }

                        let entity = match lua.registry_value::<Table>(&entity_data.luau_key) {
                            Ok(entity) => entity,
                            Err(_) => continue,
                        };
                        let raycastable = entity.get::<Option<bool>>("raycastable").unwrap_or(None);
                        if matches!(raycastable, Some(false)) {
                            continue;
                        }

                        let (width, height) = get_global_size(&entity).unwrap_or((0.0, 0.0));
                        if width <= 0.0 || height <= 0.0 {
                            continue;
                        }

                        let (entity_x, entity_y) = match get_global_position(&entity) {
                            Ok(pos) => pos,
                            Err(_) => continue,
                        };
                        let min_x = entity_x;
                        let min_y = entity_y;
                        let max_x = entity_x + width;
                        let max_y = entity_y + height;

                        let hit = raycast_aabb(
                            origin_x,
                            origin_y,
                            ray_x,
                            ray_y,
                            min_x,
                            min_y,
                            max_x,
                            max_y,
                            max_distance,
                        );

                        if let Some((distance, hit_x, hit_y, normal_x, normal_y)) = hit {
                            if best_hit
                                .as_ref()
                                .map(|(_, best_distance, _, _, _, _)| distance < *best_distance)
                                .unwrap_or(true)
                            {
                                best_hit =
                                    Some((entity, distance, hit_x, hit_y, normal_x, normal_y));
                            }
                        }
                    }

                    if let Some((entity, distance, hit_x, hit_y, normal_x, normal_y)) = best_hit {
                        let hit_table = lua.create_table()?;
                        hit_table.set("entity", entity.clone())?;
                        hit_table.set("id", entity.get::<usize>("id").unwrap_or(0))?;
                        hit_table.set("distance", distance)?;
                        hit_table.set("x", hit_x)?;
                        hit_table.set("y", hit_y)?;
                        hit_table.set("normalX", normal_x)?;
                        hit_table.set("normalY", normal_y)?;
                        hit_table.set("normal_x", normal_x)?;
                        hit_table.set("normal_y", normal_y)?;
                        return Ok(Some(hit_table));
                    }

                    Ok(None::<Table>)
                },
            )?;

            transforms.set("getWorldPosition", get_world_position)?;
            transforms.set("getWorldRotation", get_world_rotation)?;

            transforms.set("doTheyOverlap", do_they_overlap)?;
            transforms.set("raycast", raycast)?;
        }

        // Systems
        {
            let systems = self.systems.clone();
            let add_system = self.lua.create_function(move |lua, system: Table| {
                let key = lua.create_registry_value(system)?;
                systems.borrow_mut().push(key);
                Ok(())
            })?;

            ecs.set("addSystem", add_system)?;
        }

        // Entities
        {
            let entities = self.entities.clone();
            let entities_delete = self.entities.clone();
            let entity_listeners = self.entity_listeners.clone();
            let entity_max = Rc::new(RefCell::new(self.entity_max));
            let entity_max_clone = entity_max.clone();
            let table_remove: Function = self.lua.globals().get::<Table>("table")?.get("remove")?;

            let new =
                self.lua.create_function(
                    move |lua,
                          (name, _parent, x, y): (
                        String,
                        Option<Table>,
                        Option<f64>,
                        Option<f64>,
                    )| {
                        let luau = create_entity_table(
                            lua,
                            &name,
                            x.unwrap_or(0.0),
                            y.unwrap_or(0.0),
                            _parent,
                        )?;

                        let mut max = entity_max_clone.borrow_mut();
                        *max += 1;
                        let id = *max;

                        luau.set("id", id)?;

                        let reg_key = lua.create_registry_value(&luau)?;

                        let entity = hierarchy::Entity {
                            components: Vec::new(),
                            children: Vec::new(),
                            parent: None,
                            id,
                            luau_key: reg_key,
                        };

                        entities.borrow_mut().insert(id, entity);

                        Ok(luau)
                    },
                )?;

            ecs.set("newEntity", new)?;

            let table_remove_delete = table_remove.clone();
            let delete = self.lua.create_function(move |_lua, entity: Table| {
                // Recursive deletion
                let mut ids_to_remove = Vec::new();
                let mut stack = vec![entity.clone()];

                while let Some(current) = stack.pop() {
                    if let Ok(id) = current.get::<usize>("id") {
                        ids_to_remove.push(id);
                    }

                    if let Ok(children) = current.get::<Table>("children") {
                        for pair in children.pairs::<Value, Table>() {
                            if let Ok((_, child)) = pair {
                                stack.push(child);
                            }
                        }
                    }
                }

                let mut entities = entities_delete.borrow_mut();
                for id in &ids_to_remove {
                    entities.remove(id);
                }
                drop(entities);

                disconnect_entity_listeners_for_entities(_lua, &entity_listeners, &ids_to_remove)?;

                if let Ok(Some(parent)) = entity.get::<Option<Table>>("parent") {
                    let children: Table = parent.get("children")?;

                    let len = children.len()?;
                    for i in 1..=len {
                        if children.get::<Table>(i)? == entity {
                            table_remove_delete.call::<()>((children, i))?;
                            break;
                        }
                    }
                }
                Ok(())
            })?;

            ecs.set("deleteEntity", delete)?;

            let duplicate =
                self.lua
                    .create_function(move |lua, (target_entity, parent): (Table, Table)| {
                        crate::prefabs::instantiate_entity_tree_from_source(
                            lua,
                            &target_entity,
                            Some(parent),
                        )
                    })?;

            ecs.set("duplicateEntity", duplicate)?;

            let find_first_child =
                self.lua
                    .create_function(move |_lua, (parent, name): (Table, String)| {
                        if let Ok(children) = parent.get::<Table>("children") {
                            for pair in children.pairs::<Value, Table>() {
                                if let Ok((_, child)) = pair {
                                    if let Ok(child_name) = child.get::<String>("name") {
                                        if child_name == name {
                                            return Ok(Some(child));
                                        }
                                    }
                                }
                            }
                        }
                        Ok(None)
                    })?;

            ecs.set("findFirstChild", find_first_child)?;

            // create root entity
            let root_table = create_entity_table(&self.lua, "root", 0.0, 0.0, None)?;
            root_table.set("id", 0)?;

            let root_key = self.lua.create_registry_value(&root_table)?;
            let root_entity = hierarchy::Entity {
                components: Vec::new(),
                children: Vec::new(),
                parent: None,
                id: 0,
                luau_key: root_key,
            };
            self.entities.borrow_mut().insert(0, root_entity);
            ecs.set("root", root_table)?;
        }

        // Components
        {
            crate::core::add_core_components(
                &self.lua,
                self.platform.clone(),
                self.render_state.clone(),
                self.environment.clone(),
            )?; // a lot of heavy lifting

            let table_remove: Function = self.lua.globals().get::<Table>("table")?.get("remove")?;

            let add_component =
                self.lua
                    .create_function(move |lua, (entity, component): (Table, Value)| {
                        let template = match component {
                            Value::Table(component) => component,
                            Value::Nil => {
                                return Err(mlua::Error::external(
                                    "component prototype is nil; the requested component may have been removed",
                                ));
                            }
                            other => {
                                return Err(mlua::Error::external(format!(
                                    "component prototype must be a table, got {}",
                                    other.type_name()
                                )));
                            }
                        };

                        let components: Table = entity.get("components")?;
                        let comp = deep_copy_table(lua, &template)?;
                        comp.set("entity", &entity)?;
                        attach_component_methods(lua, &comp)?;
                        let component_name = describe_component_name(&comp, Some(&entity));
                        let awake: Function = comp.get("awake").map_err(|_| {
                            mlua::Error::external(format!(
                                "component '{component_name}' has no awake function"
                            ))
                        })?;
                        protect_lua_call(
                            &format!("running component awake callback ({component_name})"),
                            || awake.call::<()>((&entity, &comp)),
                        )?;
                        components.push(&comp)?;
                        Ok(comp)
                    })?;

            ecs.set("addComponent", add_component)?;

            let table_remove_component = table_remove.clone();
            let remove_component =
                self.lua
                    .create_function(move |_lua, (entity, target): (Table, Value)| {
                        let components: Table = entity.get("components")?;
                        let mut remove_index: Option<usize> = None;

                        match target {
                            Value::Integer(i) if i > 0 => {
                                remove_index = Some(i as usize);
                            }
                            Value::Number(n) if n.is_finite() && n >= 1.0 && n.fract() == 0.0 => {
                                remove_index = Some(n as usize);
                            }
                            Value::Table(target_table) => {
                                let len = components.len()? as usize;
                                for i in 1..=len {
                                    if let Ok(component) = components.get::<Table>(i) {
                                        if component == target_table {
                                            remove_index = Some(i);
                                            break;
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }

                        let Some(index) = remove_index else {
                            return Ok(false);
                        };

                        let len = components.len()? as usize;
                        if index == 0 || index > len {
                            return Ok(false);
                        }

                        let component: Table = components.get(index)?;
                        if let Ok(destroy) = component.get::<Function>("destroy") {
                            let component_name = describe_component_name(&component, Some(&entity));
                            protect_lua_call(
                                &format!("running component destroy callback ({component_name})"),
                                || destroy.call::<()>((&entity, &component)),
                            )?;
                        } else if let Ok(on_destroy) = component.get::<Function>("onDestroy") {
                            let component_name = describe_component_name(&component, Some(&entity));
                            protect_lua_call(
                                &format!("running component onDestroy callback ({component_name})"),
                                || on_destroy.call::<()>((&entity, &component)),
                            )?;
                        }
                        component.set("entity", Value::Nil)?;

                        table_remove_component.call::<()>((&components, index))?;
                        Ok(true)
                    })?;

            ecs.set("removeComponent", remove_component)?;
        }

        self.lua.globals().set("ecs", ecs)?;
        self.lua.globals().set("transform", transforms.clone())?;
        self.lua.globals().set("transforms", transforms)?;
        crate::prefabs::add_prefab_module(&self.lua)?;

        self.lua
            .load(entry_file.as_path())
            .set_name(format!("@{}", entry_module.display()))
            .exec()?;

        Ok(())
    }

    fn poll_http_callbacks(&self) {
        let globals = self.lua.globals();
        let http = match globals.get::<Table>("http") {
            Ok(table) => table,
            Err(_) => return,
        };
        let poll = match http.get::<Function>("_poll") {
            Ok(function) => function,
            Err(_) => return,
        };
        if let Err(e) = protect_lua_call("polling HTTP callbacks", || poll.call::<()>(())) {
            eprintln!(
                "\x1b[31mLua Error:\x1b[0m Failed to poll HTTP callbacks\n{}",
                describe_lua_error(&e)
            );
        }
    }

    fn poll_server_callbacks(&self) {
        let globals = self.lua.globals();
        let servers = match globals.get::<Table>("servers") {
            Ok(table) => table,
            Err(_) => return,
        };
        let poll = match servers.get::<Function>("_poll") {
            Ok(function) => function,
            Err(_) => return,
        };
        if let Err(e) = protect_lua_call("polling server callbacks", || poll.call::<()>(())) {
            eprintln!(
                "\x1b[31mLua Error:\x1b[0m Failed to poll server callbacks\n{}",
                describe_lua_error(&e)
            );
        }
    }

    fn dispatch_entity_listeners(&self) {
        let (mouse, input) = match self.platform.lock() {
            Ok(platform) => (platform.mouse(), platform.input().clone()),
            Err(_) => {
                eprintln!(
                    "\x1b[31mLua Error:\x1b[0m Failed to read input state for entity listeners"
                );
                return;
            }
        };

        let mut triggered_events = HashSet::<EntityListenEvent>::new();
        if input.mouse_pressed.contains("left") {
            triggered_events.insert(EntityListenEvent::LeftClick);
        }
        if input.mouse_pressed.contains("right") {
            triggered_events.insert(EntityListenEvent::RightClick);
        }
        if input.mouse_pressed.contains("middle") {
            triggered_events.insert(EntityListenEvent::MiddleClick);
        }
        if input.wheel_y > 0.0 {
            triggered_events.insert(EntityListenEvent::ScrollUp);
        }
        if input.wheel_y < 0.0 {
            triggered_events.insert(EntityListenEvent::ScrollDown);
        }
        if triggered_events.is_empty() {
            return;
        }

        let mut hovered_entities = Vec::<(Table, f64, usize)>::new();
        {
            let entities = self.entities.borrow();
            for entity_data in entities.values() {
                let entity = match self.lua.registry_value::<Table>(&entity_data.luau_key) {
                    Ok(entity) => entity,
                    Err(_) => continue,
                };
                match point_hits_entity(&entity, mouse.x, mouse.y) {
                    Ok(true) => {
                        let z = entity.get::<f64>("z").unwrap_or(0.0);
                        let entity_id = entity.get::<usize>("id").unwrap_or(0);
                        hovered_entities.push((entity, z, entity_id));
                    }
                    Ok(false) => {}
                    Err(error) => {
                        eprintln!(
                            "\x1b[31mLua Error:\x1b[0m Failed to hit-test entity listener target: {}",
                            error
                        );
                    }
                }
            }
        }

        hovered_entities.sort_by(|a, b| compare_entity_order(a.1, a.2, b.1, b.2).reverse());

        let mut queue = Vec::<(Table, Function, Table)>::new();
        {
            let listeners = self.entity_listeners.borrow();
            for (entity, _, entity_id) in hovered_entities {
                for listener in listeners.values() {
                    if !listener.connected.get()
                        || listener.entity_id != entity_id
                        || !triggered_events.contains(&listener.event)
                    {
                        continue;
                    }

                    let callback = match self.lua.registry_value::<Function>(&listener.callback) {
                        Ok(callback) => callback,
                        Err(error) => {
                            eprintln!(
                                "\x1b[31mLua Error:\x1b[0m Failed to resolve entity listener callback: {}",
                                error
                            );
                            continue;
                        }
                    };
                    let payload = match create_entity_listener_event(
                        &self.lua,
                        listener.event,
                        mouse.x,
                        mouse.y,
                        input.wheel_x,
                        input.wheel_y,
                    ) {
                        Ok(payload) => payload,
                        Err(error) => {
                            eprintln!(
                                "\x1b[31mLua Error:\x1b[0m Failed to build entity listener event: {}",
                                error
                            );
                            continue;
                        }
                    };
                    queue.push((entity.clone(), callback, payload));
                }
            }
        }

        for (entity, callback, payload) in queue {
            if let Err(error) = protect_lua_call("running entity listener callback", || {
                callback.call::<()>((entity.clone(), payload.clone()))
            }) {
                eprintln!(
                    "\x1b[31mLua Error in entity listener callback:\x1b[0m\n{}",
                    describe_lua_error(&error)
                );
            }
        }
    }

    fn rebuild_physics_world(&mut self, physics_infos: &[EntityPhysicsInfo]) -> mlua::Result<()> {
        let mut bodies = RigidBodySet::new();
        let mut colliders = ColliderSet::new();

        let mut body_sync: Vec<RapierBodySync> = Vec::new();
        let mut collider_sync: Vec<RapierColliderSync> = Vec::new();
        let mut collider_map: HashMap<ColliderHandle, usize> = HashMap::new();
        let mut body_by_entity_id: HashMap<usize, RigidBodyHandle> = HashMap::new();
        let mut entity_by_id: HashMap<usize, Table> = HashMap::new();

        for info in physics_infos {
            if info.entity_id > 0 {
                entity_by_id.insert(info.entity_id, info.entity.clone());
            }
        }

        for info in physics_infos {
            let entity_id = info.entity_id;
            if entity_id == 0 {
                continue;
            }

            let entity = &info.entity;
            let rigidbody = info.rigidbody.clone();
            let collider = info.collider.clone();
            if rigidbody.is_none() && collider.is_none() {
                continue;
            }

            let (entity_w, entity_h) = get_global_size(entity).unwrap_or((0.0, 0.0));
            let entity_w = entity_w.max(0.0);
            let entity_h = entity_h.max(0.0);
            let entity_x = entity.get::<f32>("x").unwrap_or(0.0);
            let entity_y = entity.get::<f32>("y").unwrap_or(0.0);
            let entity_rotation = entity.get::<f32>("rotation").unwrap_or(0.0);
            let body_mass = rigidbody
                .as_ref()
                .and_then(|rb| rb.get::<f32>("mass").ok())
                .unwrap_or(1.0)
                .max(0.0001);

            let mut is_static = rigidbody
                .as_ref()
                .and_then(|rb| rb.get::<bool>("is_static").ok())
                .unwrap_or(true);
            if rigidbody.is_none() {
                is_static = true;
            }

            let mut builder = if is_static {
                RigidBodyBuilder::fixed()
            } else {
                RigidBodyBuilder::dynamic()
            };
            builder = builder
                .translation(vector![
                    entity_x + entity_w * 0.5,
                    entity_y + entity_h * 0.5
                ])
                .rotation(entity_rotation);

            if let Some(ref rb) = rigidbody {
                let freeze_x = rb.get::<bool>("freeze_x").unwrap_or(false);
                let freeze_y = rb.get::<bool>("freeze_y").unwrap_or(false);
                let freeze_rotation = rb.get::<bool>("freeze_rotation").unwrap_or(false);
                let velocity_x = rb.get::<f32>("velocity_x").unwrap_or(0.0);
                let velocity_y = rb.get::<f32>("velocity_y").unwrap_or(0.0);
                let angular_velocity = rb.get::<f32>("angular_velocity").unwrap_or(0.0);

                builder = builder
                    .linvel(vector![velocity_x, velocity_y])
                    .angvel(angular_velocity)
                    .linear_damping(rb.get::<f32>("linear_damping").unwrap_or(0.0).max(0.0))
                    .angular_damping(rb.get::<f32>("angular_damping").unwrap_or(0.0).max(0.0))
                    .enabled_translations(!freeze_x, !freeze_y);
                if freeze_rotation {
                    builder = builder.lock_rotations();
                }
            }

            let body_handle = bodies.insert(builder.build());
            body_by_entity_id.insert(entity_id, body_handle);
            body_sync.push(RapierBodySync {
                entity_id,
                entity: entity.clone(),
                rigidbody: rigidbody.clone(),
                body_handle,
                size_x: entity_w,
                size_y: entity_h,
                is_static,
            });

            if let Some(collider_component) = collider {
                collider_component.set("touching", false)?;
                collider_component.set("last_hit_id", 0)?;

                if !collider_component.get::<bool>("enabled").unwrap_or(true) {
                    continue;
                }

                let collision_enabled = rigidbody
                    .as_ref()
                    .and_then(|rb| rb.get::<bool>("collision_enabled").ok())
                    .unwrap_or(true);
                if !collision_enabled {
                    continue;
                }

                let offset_x = collider_component.get::<f32>("offset_x").unwrap_or(0.0);
                let offset_y = collider_component.get::<f32>("offset_y").unwrap_or(0.0);
                let global_scale = get_global_scale(entity).unwrap_or(1.0);
                let collider_w = {
                    let w = collider_component.get::<f32>("size_x").unwrap_or(0.0);
                    if w > 0.0 {
                        w * global_scale
                    } else {
                        entity_w
                    }
                };
                let collider_h = {
                    let h = collider_component.get::<f32>("size_y").unwrap_or(0.0);
                    if h > 0.0 {
                        h * global_scale
                    } else {
                        entity_h
                    }
                };
                if collider_w <= 0.0 || collider_h <= 0.0 {
                    continue;
                }

                let rb_restitution = rigidbody
                    .as_ref()
                    .and_then(|rb| rb.get::<f32>("restitution").ok())
                    .unwrap_or(0.25)
                    .clamp(0.0, 1.0);
                let rb_friction = rigidbody
                    .as_ref()
                    .and_then(|rb| rb.get::<f32>("friction").ok())
                    .unwrap_or(0.45)
                    .max(0.0);
                let collider_restitution_raw =
                    collider_component.get::<f32>("restitution").unwrap_or(-1.0);
                let collider_restitution = if collider_restitution_raw >= 0.0 {
                    collider_restitution_raw.clamp(0.0, 1.0)
                } else {
                    rb_restitution
                };
                let collider_friction = collider_component
                    .get::<f32>("friction")
                    .unwrap_or(rb_friction)
                    .max(0.0);
                let shape = parse_collider_shape(
                    &collider_component
                        .get::<String>("shape")
                        .unwrap_or_else(|_| "box".to_string()),
                    &collider_component
                        .get::<String>("triangle_corner")
                        .unwrap_or_else(|_| "bl".to_string()),
                );
                let is_trigger = collider_component
                    .get::<bool>("is_trigger")
                    .unwrap_or(false);
                let non_physics = collider_component
                    .get::<bool>("non_physics")
                    .unwrap_or(false);

                let mut collider_builder = match shape {
                    ColliderShape::Box => ColliderBuilder::cuboid(
                        (collider_w * 0.5).max(0.0001),
                        (collider_h * 0.5).max(0.0001),
                    )
                    .translation(vector![
                        offset_x + collider_w * 0.5 - entity_w * 0.5,
                        offset_y + collider_h * 0.5 - entity_h * 0.5,
                    ]),
                    ColliderShape::Circle => {
                        let radius = (collider_w.min(collider_h) * 0.5).max(0.0001);
                        ColliderBuilder::ball(radius).translation(vector![
                            offset_x + collider_w * 0.5 - entity_w * 0.5,
                            offset_y + collider_h * 0.5 - entity_h * 0.5,
                        ])
                    }
                    ColliderShape::RightTriangle(corner) => {
                        let (a, b, c) = triangle_local_points(
                            corner, entity_w, entity_h, offset_x, offset_y, collider_w, collider_h,
                        );
                        ColliderBuilder::triangle(
                            point![a.0, a.1],
                            point![b.0, b.1],
                            point![c.0, c.1],
                        )
                    }
                };
                collider_builder = collider_builder
                    .sensor(is_trigger || non_physics)
                    .restitution(collider_restitution)
                    .friction(collider_friction);
                if !is_static {
                    collider_builder = collider_builder.mass(body_mass);
                } else {
                    collider_builder = collider_builder.density(0.0);
                }

                let collider_handle = colliders.insert_with_parent(
                    collider_builder.build(),
                    body_handle,
                    &mut bodies,
                );
                let index = collider_sync.len();
                collider_sync.push(RapierColliderSync {
                    entity_id,
                    collider: collider_component,
                    is_trigger,
                });
                collider_map.insert(collider_handle, index);
            }
        }

        self.physics_world = Some(PhysicsWorld {
            islands: IslandManager::new(),
            broad_phase: DefaultBroadPhase::new(),
            narrow_phase: NarrowPhase::new(),
            bodies,
            colliders,
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
            body_sync,
            collider_sync,
            collider_map,
            body_by_entity_id,
            entity_by_id,
        });

        Ok(())
    }

    fn simulate_rapier_physics(&mut self, dt: f32) -> mlua::Result<()> {
        let step_dt = dt.clamp(0.0, 0.25);
        if step_dt <= f32::EPSILON {
            return Ok(());
        }

        let mut physics_infos: Vec<EntityPhysicsInfo> = Vec::new();
        let mut has_physics_work = false;
        {
            let entities = self.entities.borrow();
            physics_infos.reserve(entities.len());
            for entity_data in entities.values() {
                if let Ok(entity) = self.lua.registry_value::<Table>(&entity_data.luau_key) {
                    let entity_id = entity.get::<usize>("id").unwrap_or(0);
                    let (rigidbody, collider, ropes) =
                        if let Ok(components) = entity.get::<Table>("components") {
                            extract_physics_components(&components)?
                        } else {
                            (None, None, Vec::new())
                        };
                    if rigidbody.is_some() || collider.is_some() || !ropes.is_empty() {
                        has_physics_work = true;
                    }
                    physics_infos.push(EntityPhysicsInfo {
                        entity_id,
                        entity,
                        rigidbody,
                        collider,
                        ropes,
                    });
                }
            }
        }

        physics_infos.sort_by_key(|info| info.entity_id);

        if !has_physics_work {
            self.physics_world = None;
            self.physics_signature = 0;
            return Ok(());
        }

        let signature = physics_topology_signature(&physics_infos);
        if self.physics_world.is_none() || signature != self.physics_signature {
            self.rebuild_physics_world(&physics_infos)?;
            self.physics_signature = signature;
        }

        let world = match self.physics_world.as_mut() {
            Some(world) => world,
            None => return Ok(()),
        };

        let mut rope_sync: Vec<RapierRopeSync> = Vec::new();
        let mut current_collision_ids: HashMap<usize, HashSet<usize>> = HashMap::new();
        let mut current_trigger_ids: HashMap<usize, HashSet<usize>> = HashMap::new();

        for sync in &world.collider_sync {
            sync.collider.set("touching", false)?;
            sync.collider.set("last_hit_id", 0)?;
        }

        for sync in &world.body_sync {
            let Some(body) = world.bodies.get_mut(sync.body_handle) else {
                continue;
            };

            let entity_x = sync.entity.get::<f32>("x").unwrap_or(0.0);
            let entity_y = sync.entity.get::<f32>("y").unwrap_or(0.0);
            let entity_rotation = sync.entity.get::<f32>("rotation").unwrap_or(0.0);
            body.set_translation(
                vector![entity_x + sync.size_x * 0.5, entity_y + sync.size_y * 0.5],
                true,
            );
            body.set_rotation(nalgebra::UnitComplex::new(entity_rotation), true);

            if let Some(rb) = sync.rigidbody.as_ref() {
                let freeze_x = rb.get::<bool>("freeze_x").unwrap_or(false);
                let freeze_y = rb.get::<bool>("freeze_y").unwrap_or(false);
                let freeze_rotation = rb.get::<bool>("freeze_rotation").unwrap_or(false);
                let mut velocity_x = rb.get::<f32>("velocity_x").unwrap_or(0.0);
                let mut velocity_y = rb.get::<f32>("velocity_y").unwrap_or(0.0);
                let mut angular_velocity = rb.get::<f32>("angular_velocity").unwrap_or(0.0);
                let max_speed = rb.get::<f32>("max_speed").unwrap_or(0.0).max(0.0);
                let max_angular_speed = rb.get::<f32>("max_angular_speed").unwrap_or(0.0).max(0.0);
                let is_static = rb.get::<bool>("is_static").unwrap_or(false);
                let body_mass = rb.get::<f32>("mass").unwrap_or(1.0).max(0.0001);

                if !is_static {
                    let force_x = rb.get::<f32>("force_x").unwrap_or(0.0);
                    let force_y = rb.get::<f32>("force_y").unwrap_or(0.0);
                    let acceleration_x = rb.get::<f32>("acceleration_x").unwrap_or(0.0);
                    let acceleration_y = rb.get::<f32>("acceleration_y").unwrap_or(0.0);
                    let gravity_x = rb.get::<f32>("gravity_x").unwrap_or(0.0);
                    let gravity_y = rb.get::<f32>("gravity_y").unwrap_or(980.0);
                    let gravity_scale = rb.get::<f32>("gravity_scale").unwrap_or(1.0);
                    let torque = rb.get::<f32>("torque").unwrap_or(0.0);
                    let mut inertia = rb.get::<f32>("inertia").unwrap_or(0.0);
                    if inertia <= 0.0 {
                        inertia = body_mass
                            * (sync.size_x * sync.size_x + sync.size_y * sync.size_y).max(1.0)
                            / 12.0;
                    }

                    velocity_x +=
                        (acceleration_x + gravity_x * gravity_scale + force_x / body_mass)
                            * step_dt;
                    velocity_y +=
                        (acceleration_y + gravity_y * gravity_scale + force_y / body_mass)
                            * step_dt;
                    if !freeze_rotation {
                        angular_velocity += (torque / inertia.max(0.0001)) * step_dt;
                    }
                }

                if freeze_x {
                    velocity_x = 0.0;
                }
                if freeze_y {
                    velocity_y = 0.0;
                }
                if freeze_rotation {
                    angular_velocity = 0.0;
                }

                if max_speed > 0.0 {
                    let speed_sq = velocity_x * velocity_x + velocity_y * velocity_y;
                    if speed_sq > max_speed * max_speed {
                        let speed = speed_sq.sqrt().max(0.0001);
                        let scale = max_speed / speed;
                        velocity_x *= scale;
                        velocity_y *= scale;
                    }
                }
                if max_angular_speed > 0.0 {
                    angular_velocity =
                        angular_velocity.clamp(-max_angular_speed, max_angular_speed);
                }

                body.set_body_type(
                    if is_static {
                        rapier2d::prelude::RigidBodyType::Fixed
                    } else {
                        rapier2d::prelude::RigidBodyType::Dynamic
                    },
                    true,
                );
                body.set_linvel(vector![velocity_x, velocity_y], true);
                body.set_angvel(angular_velocity, true);
                body.set_linear_damping(rb.get::<f32>("linear_damping").unwrap_or(0.0).max(0.0));
                body.set_angular_damping(rb.get::<f32>("angular_damping").unwrap_or(0.0).max(0.0));
                body.set_enabled_translations(!freeze_x, !freeze_y, true);
                body.lock_rotations(freeze_rotation, true);
            }
        }

        world.impulse_joints = ImpulseJointSet::new();
        for info in &physics_infos {
            for rope in &info.ropes {
                rope.set("tension", 0.0)?;

                let enabled = rope.get::<bool>("enabled").unwrap_or(true);
                if !enabled {
                    continue;
                }
                rope.set("snapped", false)?;
                let entity_a = match rope.get::<Option<Table>>("entity_a") {
                    Ok(Some(value)) => value,
                    _ => continue,
                };
                let entity_b = match rope.get::<Option<Table>>("entity_b") {
                    Ok(Some(value)) => value,
                    _ => continue,
                };
                let entity_a_id = entity_a.get::<usize>("id").unwrap_or(0);
                let entity_b_id = entity_b.get::<usize>("id").unwrap_or(0);
                let Some(&body_a) = world.body_by_entity_id.get(&entity_a_id) else {
                    continue;
                };
                let Some(&body_b) = world.body_by_entity_id.get(&entity_b_id) else {
                    continue;
                };

                let min_length = rope.get::<f32>("min_length").unwrap_or(0.0).max(0.0);
                let max_length = rope.get::<f32>("max_length").unwrap_or(0.0).max(min_length);
                let rope_length = max_length.max(0.001);
                let joint_handle = world.impulse_joints.insert(
                    body_a,
                    body_b,
                    RopeJointBuilder::new(rope_length).contacts_enabled(true),
                    true,
                );
                rope_sync.push(RapierRopeSync {
                    rope: rope.clone(),
                    body_a,
                    body_b,
                    joint_handle,
                });
            }
        }

        let mut pipeline = PhysicsPipeline::new();
        let mut integration_parameters = IntegrationParameters::default();
        integration_parameters.dt = step_dt;

        pipeline.step(
            &vector![0.0, 0.0],
            &integration_parameters,
            &mut world.islands,
            &mut world.broad_phase,
            &mut world.narrow_phase,
            &mut world.bodies,
            &mut world.colliders,
            &mut world.impulse_joints,
            &mut world.multibody_joints,
            &mut world.ccd_solver,
            None,
            &(),
            &(),
        );

        let mut grounded_entities = HashSet::<usize>::new();
        for pair in world.narrow_phase.contact_pairs() {
            if !pair.has_any_active_contact {
                continue;
            }
            let Some(&a_index) = world.collider_map.get(&pair.collider1) else {
                continue;
            };
            let Some(&b_index) = world.collider_map.get(&pair.collider2) else {
                continue;
            };
            let a = &world.collider_sync[a_index];
            let b = &world.collider_sync[b_index];

            a.collider.set("touching", true)?;
            b.collider.set("touching", true)?;
            a.collider.set("last_hit_id", b.entity_id)?;
            b.collider.set("last_hit_id", a.entity_id)?;

            let is_trigger_pair = a.is_trigger || b.is_trigger;
            let target = if is_trigger_pair {
                &mut current_trigger_ids
            } else {
                &mut current_collision_ids
            };
            target.entry(a.entity_id).or_default().insert(b.entity_id);
            target.entry(b.entity_id).or_default().insert(a.entity_id);

            if let Some(manifold) = pair.manifolds.first() {
                let normal = manifold.data.normal;
                if normal.y > 0.35 {
                    grounded_entities.insert(a.entity_id);
                }
                if normal.y < -0.35 {
                    grounded_entities.insert(b.entity_id);
                }
            }
        }

        for (handle1, handle2, intersecting) in world.narrow_phase.intersection_pairs() {
            if !intersecting {
                continue;
            }
            let Some(&a_index) = world.collider_map.get(&handle1) else {
                continue;
            };
            let Some(&b_index) = world.collider_map.get(&handle2) else {
                continue;
            };
            let a = &world.collider_sync[a_index];
            let b = &world.collider_sync[b_index];
            a.collider.set("touching", true)?;
            b.collider.set("touching", true)?;
            a.collider.set("last_hit_id", b.entity_id)?;
            b.collider.set("last_hit_id", a.entity_id)?;

            let is_trigger_pair = a.is_trigger || b.is_trigger;
            let target = if is_trigger_pair {
                &mut current_trigger_ids
            } else {
                &mut current_collision_ids
            };
            target.entry(a.entity_id).or_default().insert(b.entity_id);
            target.entry(b.entity_id).or_default().insert(a.entity_id);
        }

        let mut collider_by_id: HashMap<usize, Table> = HashMap::new();
        for sync in &world.collider_sync {
            collider_by_id.insert(sync.entity_id, sync.collider.clone());
        }

        for sync in &world.collider_sync {
            let Some(self_entity) = world.entity_by_id.get(&sync.entity_id).cloned() else {
                continue;
            };

            let previous_collision_ids =
                if let Ok(table) = sync.collider.get::<Table>("__prev_collision_ids") {
                    read_id_set_from_table(&table)?
                } else {
                    HashSet::new()
                };
            let previous_trigger_ids =
                if let Ok(table) = sync.collider.get::<Table>("__prev_trigger_ids") {
                    read_id_set_from_table(&table)?
                } else {
                    HashSet::new()
                };

            let active_collision_ids = current_collision_ids
                .get(&sync.entity_id)
                .cloned()
                .unwrap_or_default();
            let active_trigger_ids = current_trigger_ids
                .get(&sync.entity_id)
                .cloned()
                .unwrap_or_default();

            let fire_event = |event_name: &str,
                              event_name_alt: &str,
                              other_id: usize,
                              collider: &Table,
                              self_entity: &Table|
             -> mlua::Result<()> {
                let other_entity = world.entity_by_id.get(&other_id).cloned();
                let other_collider = collider_by_id.get(&other_id).cloned();

                if let Ok(callback) = collider.get::<Function>(event_name) {
                    protect_lua_call(
                        &format!("running collider event callback '{event_name}'"),
                        || {
                            callback.call::<()>((
                                self_entity.clone(),
                                collider.clone(),
                                other_entity.clone(),
                                other_collider.clone(),
                                other_id,
                            ))
                        },
                    )?;
                    return Ok(());
                }

                if let Ok(callback) = collider.get::<Function>(event_name_alt) {
                    protect_lua_call(
                        &format!("running collider event callback '{event_name_alt}'"),
                        || {
                            callback.call::<()>((
                                self_entity.clone(),
                                collider.clone(),
                                other_entity,
                                other_collider,
                                other_id,
                            ))
                        },
                    )?;
                }
                Ok(())
            };

            for other_id in &active_collision_ids {
                if previous_collision_ids.contains(other_id) {
                    fire_event(
                        "onCollisionStay",
                        "on_collision_stay",
                        *other_id,
                        &sync.collider,
                        &self_entity,
                    )?;
                } else {
                    fire_event(
                        "onCollisionEnter",
                        "on_collision_enter",
                        *other_id,
                        &sync.collider,
                        &self_entity,
                    )?;
                }
            }
            for other_id in &previous_collision_ids {
                if !active_collision_ids.contains(other_id) {
                    fire_event(
                        "onCollisionExit",
                        "on_collision_exit",
                        *other_id,
                        &sync.collider,
                        &self_entity,
                    )?;
                }
            }

            for other_id in &active_trigger_ids {
                if previous_trigger_ids.contains(other_id) {
                    fire_event(
                        "onTriggerStay",
                        "on_trigger_stay",
                        *other_id,
                        &sync.collider,
                        &self_entity,
                    )?;
                } else {
                    fire_event(
                        "onTriggerEnter",
                        "on_trigger_enter",
                        *other_id,
                        &sync.collider,
                        &self_entity,
                    )?;
                }
            }
            for other_id in &previous_trigger_ids {
                if !active_trigger_ids.contains(other_id) {
                    fire_event(
                        "onTriggerExit",
                        "on_trigger_exit",
                        *other_id,
                        &sync.collider,
                        &self_entity,
                    )?;
                }
            }

            sync.collider.set(
                "__prev_collision_ids",
                write_id_set_to_table(&self.lua, &active_collision_ids)?,
            )?;
            sync.collider.set(
                "__prev_trigger_ids",
                write_id_set_to_table(&self.lua, &active_trigger_ids)?,
            )?;
        }

        let window: Table = self.lua.globals().get("window")?;
        let window_w = window.get::<f32>("x").unwrap_or(0.0);
        let window_h = window.get::<f32>("y").unwrap_or(0.0);

        for sync in &world.body_sync {
            let Some(body) = world.bodies.get(sync.body_handle) else {
                continue;
            };

            let mut x = body.translation().x - sync.size_x * 0.5;
            let mut y = body.translation().y - sync.size_y * 0.5;
            let rotation = body.rotation().angle();
            let mut velocity_x = body.linvel().x;
            let mut velocity_y = body.linvel().y;
            let mut angular_velocity = body.angvel();
            let mut grounded = grounded_entities.contains(&sync.entity_id);

            if sync.is_static {
                velocity_x = 0.0;
                velocity_y = 0.0;
                angular_velocity = 0.0;
            }

            if let Some(rigidbody) = sync.rigidbody.as_ref() {
                let bounds_mode = rigidbody
                    .get::<String>("bounds_mode")
                    .unwrap_or_else(|_| "none".to_string())
                    .to_ascii_lowercase();
                let restitution = rigidbody
                    .get::<f32>("restitution")
                    .unwrap_or(0.25)
                    .clamp(0.0, 1.0);

                if bounds_mode == "window" {
                    if x < 0.0 {
                        x = 0.0;
                        if velocity_x < 0.0 {
                            velocity_x = -velocity_x * restitution;
                        }
                    } else if x + sync.size_x > window_w {
                        x = (window_w - sync.size_x).max(0.0);
                        if velocity_x > 0.0 {
                            velocity_x = -velocity_x * restitution;
                        }
                    }

                    if y < 0.0 {
                        y = 0.0;
                        if velocity_y < 0.0 {
                            velocity_y = -velocity_y * restitution;
                        }
                    } else if y + sync.size_y > window_h {
                        y = (window_h - sync.size_y).max(0.0);
                        if velocity_y > 0.0 {
                            velocity_y = -velocity_y * restitution;
                        }
                        grounded = true;
                    }
                }

                let max_speed = rigidbody.get::<f32>("max_speed").unwrap_or(0.0).max(0.0);
                if max_speed > 0.0 {
                    let speed_sq = velocity_x * velocity_x + velocity_y * velocity_y;
                    if speed_sq > max_speed * max_speed {
                        let speed = speed_sq.sqrt().max(0.0001);
                        let scale = max_speed / speed;
                        velocity_x *= scale;
                        velocity_y *= scale;
                    }
                }
                let max_angular_speed = rigidbody
                    .get::<f32>("max_angular_speed")
                    .unwrap_or(0.0)
                    .max(0.0);
                if max_angular_speed > 0.0 {
                    angular_velocity =
                        angular_velocity.clamp(-max_angular_speed, max_angular_speed);
                }
                let sleep_epsilon = rigidbody
                    .get::<f32>("sleep_epsilon")
                    .unwrap_or(1.0)
                    .max(0.0);
                if grounded && velocity_y.abs() <= sleep_epsilon {
                    velocity_y = 0.0;
                }

                rigidbody.set("velocity_x", velocity_x)?;
                rigidbody.set("velocity_y", velocity_y)?;
                rigidbody.set("angular_velocity", angular_velocity)?;
                rigidbody.set("force_x", 0.0)?;
                rigidbody.set("force_y", 0.0)?;
                rigidbody.set("torque", 0.0)?;
                rigidbody.set("grounded", grounded)?;
            }

            sync.entity.set("x", x)?;
            sync.entity.set("y", y)?;
            sync.entity.set("rotation", rotation)?;
        }

        for rope in rope_sync {
            let Some(body_a) = world.bodies.get(rope.body_a) else {
                continue;
            };
            let Some(body_b) = world.bodies.get(rope.body_b) else {
                continue;
            };
            let dx = body_b.translation().x - body_a.translation().x;
            let dy = body_b.translation().y - body_a.translation().y;
            let distance = (dx * dx + dy * dy).sqrt();
            let mut tension = 0.0f32;
            if let Some(joint) = world.impulse_joints.get(rope.joint_handle) {
                tension = joint.impulses.norm() / step_dt.max(0.0001);
            }

            rope.rope.set("current_length", distance)?;
            rope.rope.set("tension", tension)?;

            let break_force = rope.rope.get::<f32>("break_force").unwrap_or(0.0).max(0.0);
            if break_force > 0.0 && tension >= break_force {
                rope.rope.set("enabled", false)?;
                rope.rope.set("snapped", true)?;
            }
        }

        Ok(())
    }
    pub fn update(&mut self, dt: f32) -> Result<(), String> {
        crate::core::begin_ui_frame();

        self.set_mouse_table()
            .map_err(|error| format!("failed to sync mouse state into Lua: {error}"))?;
        self.set_window_table()
            .map_err(|error| format!("failed to sync window state into Lua: {error}"))?;
        self.poll_http_callbacks();
        self.poll_server_callbacks();
        self.dispatch_entity_listeners();

        let clear = (|| -> mlua::Result<PlatformColor> {
            let app: Table = self.lua.globals().get("app")?;
            let bg: Table = app.get("bg")?;
            let r: u8 = bg.get("r")?;
            let g: u8 = bg.get("g")?;
            let b: u8 = bg.get("b")?;
            let a: u8 = bg.get("a")?;
            Ok(PlatformColor::rgba(r, g, b, a))
        })()
        .map_err(|error| {
            format!(
                "failed to resolve app background color:\n{}",
                describe_lua_error(&error)
            )
        })?;
        self.platform
            .lock()
            .map_err(|_| "platform lock poisoned while updating clear color".to_string())?
            .set_clear_color(clear);

        {
            let keys = self.systems.borrow();
            for key in keys.iter() {
                let system: Table = match self.lua.registry_value(key) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("\x1b[31mLua Error:\x1b[0m Failed to get system: {}", e);
                        continue;
                    }
                };
                if let Ok(Value::Function(update)) = system.get::<Value>("update") {
                    if let Err(e) = protect_lua_call("running system update callback", || {
                        update.call::<()>((system.clone(), dt))
                    }) {
                        eprintln!(
                            "\x1b[31mLua Error in system update:\x1b[0m\n{}",
                            describe_lua_error(&e)
                        );
                    }
                }
            }
        }

        let mut ordered_entities: Vec<(Table, f64, usize)> = Vec::new();

        {
            let entities = self.entities.borrow();
            ordered_entities.reserve(entities.len());
            for entity in entities.values() {
                if let Ok(table) = self.lua.registry_value::<Table>(&entity.luau_key) {
                    let z = table.get::<f64>("z").unwrap_or(0.0);
                    let id = table.get::<usize>("id").unwrap_or(0);
                    ordered_entities.push((table, z, id));
                }
            }
        }

        ordered_entities.sort_by(|a, b| compare_entity_order(a.1, a.2, b.1, b.2));

        let mut rendering_components: Vec<(Table, Table, Function)> = Vec::new();
        rendering_components.reserve(ordered_entities.len());

        for (ent, _, _) in ordered_entities {
            // run through all the components

            let components: Table = match ent.get("components") {
                Ok(c) => c,
                Err(e) => {
                    eprintln!(
                        "\x1b[31mLua Error:\x1b[0m Entity missing components table: {}",
                        e
                    );
                    continue;
                }
            };

            for component in components.sequence_values::<Table>() {
                let component = match component {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!(
                            "\x1b[31mLua Error:\x1b[0m Failed to iterate components: {}",
                            e
                        );
                        continue;
                    }
                };
                let update: Function = match component.get("update") {
                    Ok(u) => u,
                    Err(e) => {
                        eprintln!("\x1b[31mLua Error:\x1b[0m Component missing update: {}", e);
                        continue;
                    }
                };

                let is_rendering = component.get::<bool>("NEOLOVE_RENDERING").unwrap_or(false);
                if !is_rendering {
                    let component_name = describe_component_name(&component, Some(&ent));
                    if let Err(e) = protect_lua_call(
                        &format!("running component update callback ({component_name})"),
                        || update.call::<()>((&ent, component, dt)),
                    ) {
                        eprintln!(
                            "\x1b[31mLua Error in component update:\x1b[0m\n{}",
                            describe_lua_error(&e)
                        );
                    }
                } else {
                    rendering_components.push((ent.clone(), component, update));
                }
            }
        }

        if let Err(e) = self.simulate_rapier_physics(dt) {
            eprintln!(
                "\x1b[31mLua Error in Rapier2D physics:\x1b[0m\n{}",
                describe_lua_error(&e)
            );
        }

        for trio in rendering_components {
            let component_name = describe_component_name(&trio.1, Some(&trio.0));
            if let Err(e) = protect_lua_call(
                &format!("running rendering component update callback ({component_name})"),
                || trio.2.call::<()>((trio.0, trio.1, dt)),
            ) {
                eprintln!(
                    "\x1b[31mLua Error in rendering component update:\x1b[0m\n{}",
                    describe_lua_error(&e)
                );
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn assert_close(actual: f32, expected: f32) {
        let diff = (actual - expected).abs();
        assert!(
            diff <= 0.001,
            "expected {expected}, got {actual}, diff {diff}"
        );
    }

    fn temp_project_root(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("neolove_window_{name}_{unique}"))
    }

    fn start_test_runtime(name: &str) -> mlua::Result<(Runtime, PathBuf)> {
        let root = temp_project_root(name);
        std::fs::create_dir_all(&root).map_err(mlua::Error::external)?;
        std::fs::write(root.join("main.luau"), "-- test runtime\n")
            .map_err(mlua::Error::external)?;

        let mut runtime = Runtime::new(root.clone());
        runtime.set_platform_window_state(640.0, 480.0);
        runtime.start()?;
        Ok((runtime, root))
    }

    #[test]
    fn child_translation_inherits_parent_scale() -> mlua::Result<()> {
        let lua = Lua::new();
        let parent = create_entity_table(&lua, "parent", 0.0, 0.0, None)?;
        parent.set("scale", 0.5)?;

        let left = create_entity_table(&lua, "left", 0.0, 0.0, Some(parent.clone()))?;
        let right = create_entity_table(&lua, "right", 32.0, 0.0, Some(parent))?;

        let (left_x, _, _) = get_global_transform(&left)?;
        let (right_x, _, _) = get_global_transform(&right)?;
        let (left_w, _) = get_global_size(&left)?;

        assert_close(left_x, 0.0);
        assert_close(right_x, 16.0);
        assert_close(left_w, 16.0);
        assert_close(left_x + left_w, right_x);
        Ok(())
    }

    #[test]
    fn position_pivot_center_scales_with_parent() -> mlua::Result<()> {
        let lua = Lua::new();
        let parent = create_entity_table(&lua, "parent", 10.0, 4.0, None)?;
        parent.set("scale", 2.0)?;

        let child = create_entity_table(&lua, "child", 40.0, 30.0, Some(parent))?;
        child.set("size_x", 10.0)?;
        child.set("size_y", 20.0)?;
        child.set("position_pivot", "center")?;

        let (x, y, _) = get_global_transform(&child)?;
        assert_close(x, 80.0);
        assert_close(y, 44.0);
        Ok(())
    }

    #[test]
    fn parent_rotation_applies_after_scale() -> mlua::Result<()> {
        let lua = Lua::new();
        let parent = create_entity_table(&lua, "parent", 0.0, 0.0, None)?;
        parent.set("scale", 0.5)?;
        parent.set("rotation", std::f32::consts::FRAC_PI_2)?;

        let child = create_entity_table(&lua, "child", 10.0, 0.0, Some(parent))?;
        let (x, y, _) = get_global_transform(&child)?;

        assert_close(x, 0.0);
        assert_close(y, 5.0);
        Ok(())
    }

    #[test]
    fn anchor_offsets_use_parent_bounds() -> mlua::Result<()> {
        let lua = Lua::new();
        let parent = create_entity_table(&lua, "parent", 0.0, 0.0, None)?;
        parent.set("size_x", 100.0)?;
        parent.set("size_y", 50.0)?;
        parent.set("scale", 2.0)?;

        let child = create_entity_table(&lua, "child", -10.0, -5.0, Some(parent))?;
        child.set("anchor_x", 1.0)?;
        child.set("anchor_y", 0.5)?;

        let (x, y, _) = get_global_transform(&child)?;
        assert_close(x, 180.0);
        assert_close(y, 40.0);
        Ok(())
    }

    #[test]
    fn numeric_pivot_offsets_override_position_pivot() -> mlua::Result<()> {
        let lua = Lua::new();
        let entity = create_entity_table(&lua, "pivoted", 50.0, 30.0, None)?;
        entity.set("size_x", 20.0)?;
        entity.set("size_y", 10.0)?;
        entity.set("position_pivot", "center")?;
        entity.set("pivot_x", 0.5)?;
        entity.set("pivot_y", 1.0)?;

        let (x, y, _) = get_global_transform(&entity)?;
        assert_close(x, 40.0);
        assert_close(y, 20.0);
        Ok(())
    }

    #[test]
    fn middle_pivot_rotation_hit_test_uses_unrotated_bounds() -> mlua::Result<()> {
        let lua = Lua::new();
        let entity = create_entity_table(&lua, "rotated", 0.0, 0.0, None)?;
        entity.set("size_x", 100.0)?;
        entity.set("size_y", 50.0)?;
        entity.set("rotation_pivot", "middle")?;
        entity.set("rotation", std::f32::consts::FRAC_PI_2)?;

        assert!(point_hits_entity(&entity, 50.0, 25.0)?);
        assert!(!point_hits_entity(&entity, 5.0, 5.0)?);
        Ok(())
    }

    #[test]
    fn rendering_order_is_stable_for_equal_z() -> mlua::Result<()> {
        let (mut runtime, root) = start_test_runtime("render_order")?;

        let ecs: Table = runtime.lua.globals().get("ecs")?;
        let new_entity: Function = ecs.get("newEntity")?;
        let add_component: Function = ecs.get("addComponent")?;
        let first: Table =
            new_entity.call(("first".to_string(), None::<Table>, Some(0.0), Some(0.0)))?;
        let second: Table =
            new_entity.call(("second".to_string(), None::<Table>, Some(0.0), Some(0.0)))?;

        let render_order = Rc::new(RefCell::new(Vec::<String>::new()));
        for entity in [&first, &second] {
            let order_writer = render_order.clone();
            let component = runtime.lua.create_table()?;
            component.set("__neolove_component", "TestRenderOrder")?;
            component.set("NEOLOVE_RENDERING", true)?;
            component.set(
                "awake",
                runtime
                    .lua
                    .create_function(|_lua, (_entity, _component): (Table, Table)| Ok(()))?,
            )?;
            component.set(
                "update",
                runtime.lua.create_function(
                    move |_lua, (entity, _component, _dt): (Table, Table, f32)| {
                        order_writer
                            .borrow_mut()
                            .push(entity.get::<String>("name")?);
                        Ok(())
                    },
                )?,
            )?;
            let _instance: Table = add_component.call((entity.clone(), Value::Table(component)))?;
        }

        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        let order = render_order.borrow();
        assert_eq!(order.len(), 2);
        assert_eq!(order[0], "first");
        assert_eq!(order[1], "second");

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn entity_listener_dispatches_and_disconnects() -> mlua::Result<()> {
        let (mut runtime, root) = start_test_runtime("entity_listener")?;

        let ecs: Table = runtime.lua.globals().get("ecs")?;
        let new_entity: Function = ecs.get("newEntity")?;
        let entity: Table =
            new_entity.call(("button".to_string(), None::<Table>, Some(20.0), Some(30.0)))?;
        entity.set("size_x", 120.0)?;
        entity.set("size_y", 80.0)?;

        let call_count = Rc::new(RefCell::new(0usize));
        let last_kind = Rc::new(RefCell::new(String::new()));
        let count_writer = call_count.clone();
        let kind_writer = last_kind.clone();
        let callback =
            runtime
                .lua
                .create_function(move |_lua, (_entity, event): (Table, Table)| {
                    *count_writer.borrow_mut() += 1;
                    *kind_writer.borrow_mut() = event.get::<String>("kind")?;
                    Ok(())
                })?;

        let listen: Function = entity.get("listen")?;
        let connection: Table = listen.call((entity.clone(), "leftClick".to_string(), callback))?;

        runtime.set_platform_mouse_state(40.0, 50.0);
        {
            let mut platform = runtime.platform.lock().unwrap();
            platform
                .input_mut()
                .mouse_pressed
                .insert("left".to_string());
        }

        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        assert_eq!(*call_count.borrow(), 1);
        assert_eq!(last_kind.borrow().as_str(), "leftClick");

        let disconnect: Function = connection.get("Disconnect")?;
        let disconnected: bool = disconnect.call(connection.clone())?;
        assert!(disconnected);

        {
            let mut platform = runtime.platform.lock().unwrap();
            platform.begin_frame();
            platform
                .input_mut()
                .mouse_pressed
                .insert("left".to_string());
        }

        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        assert_eq!(*call_count.borrow(), 1);

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn entity_and_component_tables_expose_instance_methods() -> mlua::Result<()> {
        let (runtime, root) = start_test_runtime("entity_methods")?;

        let ecs: Table = runtime.lua.globals().get("ecs")?;
        let new_entity: Function = ecs.get("newEntity")?;
        let parent: Table =
            new_entity.call(("parent".to_string(), None::<Table>, Some(0.0), Some(0.0)))?;
        let child: Table = new_entity.call((
            "child".to_string(),
            Some(parent.clone()),
            Some(12.0),
            Some(18.0),
        ))?;

        let find_first_child: Function = parent.get("FindFirstChild")?;
        let found: Option<Table> = find_first_child.call((parent.clone(), "child".to_string()))?;
        assert!(found.is_some());

        let component = runtime.lua.create_table()?;
        component.set("__neolove_component", "TestComponent")?;
        component.set(
            "awake",
            runtime
                .lua
                .create_function(|_lua, (_entity, _component): (Table, Table)| Ok(()))?,
        )?;
        component.set(
            "update",
            runtime
                .lua
                .create_function(|_lua, (_entity, _component, _dt): (Table, Table, f32)| Ok(()))?,
        )?;

        let add_component: Function = child.get("AddComponent")?;
        let instance: Table = add_component.call((child.clone(), Value::Table(component)))?;
        assert!(instance.get::<Function>("Remove").is_ok());

        let remove: Function = instance.get("Remove")?;
        let removed: bool = remove.call(instance.clone())?;
        assert!(removed);
        let components: Table = child.get("components")?;
        assert_eq!(components.len()?, 0);

        let duplicate: Function = child.get("Duplicate")?;
        let copy: Table = duplicate.call((child.clone(), None::<Table>))?;
        let copy_parent: Option<Table> = copy.get("parent")?;
        assert_eq!(
            copy_parent
                .ok_or_else(|| mlua::Error::external("duplicate has no parent"))?
                .to_pointer(),
            parent.to_pointer()
        );

        let delete: Function = child.get("Delete")?;
        delete.call::<()>(child.clone())?;
        let children: Table = parent.get("children")?;
        assert_eq!(children.len()?, 1);

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }
}
