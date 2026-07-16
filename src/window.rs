use mlua::{
    Compiler, Function, Lua, MultiValue, RegistryKey, Table, TextRequirer, Thread, ThreadStatus,
    Value,
};
use rapier2d::prelude::{
    CCDSolver, ColliderBuilder, ColliderHandle, ColliderSet, DefaultBroadPhase,
    GenericJointBuilder, ImpulseJointHandle, ImpulseJointSet, IntegrationParameters, IslandManager,
    JointAxesMask, JointAxis, MultibodyJointSet, NarrowPhase, PhysicsPipeline, RigidBodyBuilder,
    RigidBodyHandle, RigidBodySet, RopeJointBuilder, nalgebra, point, vector,
};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
#[cfg(target_os = "emscripten")]
use std::ffi::{CString, c_char};
use std::fs;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
#[cfg(target_os = "emscripten")]
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::hierarchy;
use crate::lua_error::{describe_lua_error, protect_lua_call};
use crate::platform::{
    Antialiasing, Color as PlatformColor, SharedPlatformState, WindowState, lock_platform_state,
    new_shared_platform_state,
};
use crate::renderer::{SharedRenderState, new_shared_render_state};

pub struct Runtime {
    entities: Rc<RefCell<HashMap<hierarchy::EntityId, hierarchy::Entity>>>,
    entity_listeners: Rc<RefCell<HashMap<u64, EntityListener>>>,
    next_entity_listener_id: Rc<RefCell<u64>>,
    hovered_entity_ids: Rc<RefCell<HashSet<usize>>>,
    entity_hover_initialized: Rc<Cell<bool>>,
    systems: Rc<RefCell<Vec<RegistryKey>>>,
    pending_component_awakes: Rc<RefCell<Vec<PendingComponentAwake>>>,
    environment: PathBuf,
    data_root: PathBuf,
    lua: Lua,
    entity_max: usize,
    exit_requested: Rc<RefCell<bool>>,
    exit_reason: Rc<RefCell<Option<String>>>,
    physics_world: Option<PhysicsWorld>,
    physics_signature: u64,
    platform: SharedPlatformState,
    render_state: SharedRenderState,
    root_table: Option<Table>,
    mouse_table_key: Option<RegistryKey>,
    window_table_key: Option<RegistryKey>,
    app_table_key: Option<RegistryKey>,
    max_fps_state: Rc<RefCell<Option<f32>>>,
    show_fps_state: Rc<RefCell<Option<bool>>>,
    async_tasks: Rc<RefCell<Vec<AsyncTask>>>,
    async_cancelled: Rc<RefCell<HashSet<u64>>>,
    next_async_id: Rc<Cell<u64>>,
    /// Optional sink that mirrors `print`/error output to an external observer
    /// (the editor's live logger window). `None` for a normal standalone run.
    log_sink: Rc<RefCell<Option<std::sync::mpsc::Sender<RuntimeLogLine>>>>,
}

struct PendingComponentAwake {
    entity: Table,
    component: Table,
}

/// A single line of runtime output, forwarded to the editor's logger window.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RuntimeLogLine {
    pub level: String,
    pub message: String,
}

/// A flattened, serializable view of one live entity, sent to the editor's
/// logger window so it can show the running scene's hierarchy and inspector.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct EntitySnapshot {
    pub id: usize,
    pub name: String,
    pub parent: Option<usize>,
    pub x: f32,
    pub y: f32,
    pub rotation: f32,
    pub scale: f32,
    pub enabled: bool,
    /// Every public field on the live entity table, excluding hierarchy and
    /// component containers which have dedicated snapshot representations.
    #[serde(default)]
    pub fields: Vec<(String, String)>,
    pub components: Vec<ComponentSnapshot>,
}

/// One component on a snapshotted entity, with its public fields
/// pre-stringified for display.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ComponentSnapshot {
    pub name: String,
    pub fields: Vec<(String, String)>,
}

struct AsyncTask {
    id: u64,
    thread: Thread,
    handle: Table,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum EntityListenEvent {
    LeftClick,
    RightClick,
    MiddleClick,
    ScrollUp,
    ScrollDown,
    MouseEntered,
    MouseExited,
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
            "mouseenter" | "mouseentered" | "mouseover" | "hoverenter" | "entered" | "enter" => {
                Some(Self::MouseEntered)
            }
            "mouseexit" | "mouseexited" | "mouseleave" | "mouseleft" | "hoverexit"
            | "hoverleave" | "exited" | "exit" => Some(Self::MouseExited),
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
            Self::MouseEntered => "mouseEntered",
            Self::MouseExited => "mouseExited",
        }
    }

    fn is_hover_edge(self) -> bool {
        matches!(self, Self::MouseEntered | Self::MouseExited)
    }

    fn button(self) -> Option<&'static str> {
        match self {
            Self::LeftClick => Some("left"),
            Self::RightClick => Some("right"),
            Self::MiddleClick => Some("middle"),
            Self::ScrollUp | Self::ScrollDown | Self::MouseEntered | Self::MouseExited => None,
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

fn table_color_channel(table: &Table, primary: &str, legacy: &str, default: f32) -> f32 {
    table
        .raw_get::<f32>(primary)
        .or_else(|_| table.raw_get::<f32>(legacy))
        .unwrap_or(default)
}

fn table_to_platform_color(table: &Table) -> PlatformColor {
    let r = table_color_channel(table, "r", "R", 255.0);
    let g = table_color_channel(table, "g", "G", 255.0);
    let b = table_color_channel(table, "b", "B", 255.0);
    let a = table_color_channel(table, "a", "A", 255.0);
    PlatformColor::rgba(
        r.clamp(0.0, 255.0) as u8,
        g.clamp(0.0, 255.0) as u8,
        b.clamp(0.0, 255.0) as u8,
        a.clamp(0.0, 255.0) as u8,
    )
}

fn deep_copy_table(lua: &Lua, table: &Table) -> mlua::Result<Table> {
    let mut seen = HashMap::<usize, Table>::new();
    deep_copy_table_inner(lua, table, &mut seen)
}

fn deep_copy_table_inner(
    lua: &Lua,
    table: &Table,
    seen: &mut HashMap<usize, Table>,
) -> mlua::Result<Table> {
    let identity = table.to_pointer() as usize;
    if let Some(existing) = seen.get(&identity) {
        return Ok(existing.clone());
    }

    let copy = lua.create_table()?;
    seen.insert(identity, copy.clone());
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let copied_value = match value {
            Value::Table(t) => Value::Table(deep_copy_table_inner(lua, &t, seen)?),
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

fn clear_scene_entities(
    lua: &Lua,
    entities: &Rc<RefCell<HashMap<hierarchy::EntityId, hierarchy::Entity>>>,
    entity_listeners: &Rc<RefCell<HashMap<u64, EntityListener>>>,
    pending_component_awakes: &Rc<RefCell<Vec<PendingComponentAwake>>>,
    hovered_entity_ids: &Rc<RefCell<HashSet<usize>>>,
    entity_hover_initialized: &Rc<Cell<bool>>,
    root_table: &Table,
) -> mlua::Result<()> {
    let ids_to_remove: Vec<usize> = {
        let entities = entities.try_borrow().map_err(|_| {
            mlua::Error::external("cannot clear scene while the entity registry is being changed")
        })?;
        entities.keys().copied().filter(|id| *id != 0).collect()
    };
    let removed_ids: HashSet<usize> = ids_to_remove.iter().copied().collect();

    {
        let mut entities = entities.try_borrow_mut().map_err(|_| {
            mlua::Error::external("cannot clear scene while the entity registry is being read")
        })?;
        for id in &ids_to_remove {
            entities.remove(id);
        }
    }

    disconnect_entity_listeners_for_entities(lua, entity_listeners, &ids_to_remove)?;

    pending_component_awakes.borrow_mut().retain(|pending| {
        pending
            .entity
            .get::<usize>("id")
            .is_ok_and(|id| !removed_ids.contains(&id))
    });
    hovered_entity_ids.borrow_mut().clear();
    entity_hover_initialized.set(false);
    root_table.set("children", lua.create_table()?)?;

    Ok(())
}

fn create_entity_listener_connection(
    lua: &Lua,
    listeners: Rc<RefCell<HashMap<u64, EntityListener>>>,
    listener_id: u64,
    connected: Rc<Cell<bool>>,
    registry_lua: Lua,
) -> mlua::Result<Table> {
    let connection = lua.create_table()?;

    let disconnect_listeners = listeners.clone();
    let disconnect_connected = connected.clone();
    let disconnect_registry_lua = registry_lua;
    let disconnect = lua.create_function(move |_lua, _self: Table| {
        let removed = disconnect_entity_listener(
            &disconnect_registry_lua,
            &disconnect_listeners,
            listener_id,
        )?;
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
    local_x: f32,
    local_y: f32,
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
    payload.set("localX", local_x)?;
    payload.set("localY", local_y)?;
    payload.set("local_x", local_x)?;
    payload.set("local_y", local_y)?;
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
        | EntityListenEvent::MiddleClick
        | EntityListenEvent::MouseEntered
        | EntityListenEvent::MouseExited => 0.0,
    };
    payload.set("amount", amount)?;

    Ok(payload)
}

fn describe_component_name(component: &Table, entity: Option<&Table>) -> String {
    let component_name = component
        .get::<String>("__neolove_component")
        .ok()
        .or_else(|| component.get::<String>("name").ok())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty());
    let entity_name = entity
        .and_then(|entity| entity.get::<String>("name").ok())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty());

    match (component_name, entity_name) {
        (Some(component), Some(entity)) => {
            format!("component '{component}' on entity '{entity}'")
        }
        (Some(component), None) => format!("component '{component}'"),
        (None, Some(entity)) => format!("anonymous component on entity '{entity}'"),
        (None, None) => "anonymous component".to_string(),
    }
}

/// Best-effort source location (`file:line`) for a component table.
///
/// Component tables don't record where they were authored, but any Luau
/// function they define does. We inspect the component's fields for a function
/// and read its debug info, preferring `update` itself when present, otherwise
/// falling back to the first function we find (a sibling callback that lives in
/// the same source file). Luau debug info only tracks line numbers, not
/// columns, so no character offset is available.
fn describe_component_source(component: &Table) -> Option<String> {
    fn format_info(func: &Function) -> Option<String> {
        let info = func.info();
        if info.what != "Lua" {
            // C functions (engine built-ins) have no useful source location.
            return None;
        }
        let source = info.short_src.or_else(|| {
            info.source
                .map(|s| s.trim_start_matches(['@', '=']).to_string())
        })?;
        let source = source.trim();
        if source.is_empty() {
            return None;
        }
        Some(match info.line_defined {
            Some(line) => format!("{source}:{line}"),
            None => source.to_string(),
        })
    }

    if let Ok(Value::Function(update)) = component.get::<Value>("update") {
        if let Some(location) = format_info(&update) {
            return Some(location);
        }
    }

    for pair in component.pairs::<Value, Value>() {
        let Ok((_, Value::Function(func))) = pair else {
            continue;
        };
        if let Some(location) = format_info(&func) {
            return Some(location);
        }
    }

    None
}

fn component_is_attached_to_entity(entity: &Table, component: &Table) -> bool {
    let Ok(owner) = component.raw_get::<Table>("entity") else {
        return false;
    };
    if owner != entity.clone() {
        return false;
    }
    let Ok(components) = entity.raw_get::<Table>("components") else {
        return false;
    };
    for index in 1..=components.raw_len() {
        if components
            .raw_get::<Table>(index)
            .is_ok_and(|attached| attached == component.clone())
        {
            return true;
        }
    }
    false
}

#[cfg(target_os = "emscripten")]
unsafe extern "C" {
    fn neolove_web_debug_log(message: *const c_char);
}

#[cfg(target_os = "emscripten")]
fn web_debug_log(message: &str) {
    let Ok(message) = CString::new(message) else {
        return;
    };

    unsafe {
        neolove_web_debug_log(message.as_ptr());
    }
}

#[cfg(not(target_os = "emscripten"))]
fn web_debug_log(_message: &str) {}

#[cfg(target_os = "emscripten")]
fn next_web_update_index() -> usize {
    static UPDATE_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
    UPDATE_LOG_COUNT.fetch_add(1, Ordering::Relaxed) + 1
}

#[cfg(not(target_os = "emscripten"))]
fn next_web_update_index() -> usize {
    0
}

#[cfg(target_os = "emscripten")]
fn web_update_trace(index: usize) -> Option<usize> {
    // Keep verbose wasm-side runtime tracing to early startup frames only.
    const WEB_UPDATE_TRACE_LIMIT: usize = 160;
    if index <= WEB_UPDATE_TRACE_LIMIT {
        Some(index)
    } else {
        None
    }
}

#[cfg(not(target_os = "emscripten"))]
fn web_update_trace(_index: usize) -> Option<usize> {
    None
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

    let is_inside =
        lua.create_function(move |_lua, (entity, world_x, world_y): (Table, f32, f32)| {
            point_hits_entity(&entity, world_x, world_y)
        })?;
    entity.set("isInside", is_inside.clone())?;
    entity.set("IsInside", is_inside)?;

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

fn resolve_project_path(root: &Path, input: &str) -> PathBuf {
    let path = PathBuf::from(input);
    let candidate = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    normalize_path(&candidate)
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

fn look_at_rotation(from_x: f32, from_y: f32, to_x: f32, to_y: f32) -> f32 {
    (to_y - from_y).atan2(to_x - from_x)
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
    if scale.is_finite() { scale } else { 1.0 }
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

/// Convert a world-space point into the entity's rendered local rectangle.
///
/// `get_global_transform` already returns the transformed top-left corner after
/// position and rotation-pivot compensation. Inverting the accumulated world
/// rotation around that corner therefore mirrors the renderer exactly, while
/// also covering custom pivots and rotated parent chains without rebuilding a
/// second (and subtly divergent) pivot transform just for pointer listeners.
fn entity_local_point(entity: &Table, point_x: f32, point_y: f32) -> mlua::Result<(f32, f32)> {
    let (origin_x, origin_y, rotation) = get_global_transform(entity)?;
    Ok(rotate_point(
        point_x - origin_x,
        point_y - origin_y,
        -rotation,
    ))
}

fn point_hits_entity(entity: &Table, point_x: f32, point_y: f32) -> mlua::Result<bool> {
    let (width, height) = get_global_size(entity)?;
    if width <= 0.0 || height <= 0.0 {
        return Ok(false);
    }

    let (local_x, local_y) = entity_local_point(entity, point_x, point_y)?;
    Ok(local_x >= 0.0 && local_x <= width && local_y >= 0.0 && local_y <= height)
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
    // Entity pose (local x, y, rotation) that physics last wrote back to the
    // entity. Compared against the entity's current pose at the start of the
    // next step so a script that manually assigns `entity.x/y/rotation` on a
    // dynamic body teleports the physics body instead of being overwritten.
    last_sync_pose: Cell<Option<(f32, f32, f32)>>,
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

struct RapierBoltSync {
    bolt: Table,
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
    body_sync_by_entity_id: HashMap<usize, usize>,
    entity_by_id: HashMap<usize, Table>,
}

struct EntityPhysicsInfo {
    entity_id: usize,
    entity: Table,
    rigidbody: Option<Table>,
    collider: Option<Table>,
    ropes: Vec<Table>,
    bolts: Vec<Table>,
    legacy_bolts: Vec<Table>,
}

fn extract_physics_components(
    components: &Table,
) -> mlua::Result<(
    Option<Table>,
    Option<Table>,
    Vec<Table>,
    Vec<Table>,
    Vec<Table>,
)> {
    let mut rigidbody: Option<Table> = None;
    let mut collider: Option<Table> = None;
    let mut ropes: Vec<Table> = Vec::new();
    let mut bolts: Vec<Table> = Vec::new();
    let mut legacy_bolts: Vec<Table> = Vec::new();

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
            "Bolt2D" => bolts.push(component),
            "LegacyBolt2D" => legacy_bolts.push(component),
            _ => {}
        }
    }

    Ok((rigidbody, collider, ropes, bolts, legacy_bolts))
}

fn snapshot_value(
    lua: &Lua,
    value: Value,
    depth: usize,
    seen: &mut HashSet<usize>,
) -> Option<String> {
    match value {
        Value::Nil => Some("nil".to_string()),
        Value::Boolean(value) => Some(value.to_string()),
        Value::Integer(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        Value::String(value) => Some(value.to_string_lossy()),
        Value::Function(_) | Value::Thread(_) => None,
        Value::Table(table) => {
            if let (Ok(id), Ok(_)) = (
                table.raw_get::<usize>("id"),
                table.raw_get::<Table>("components"),
            ) {
                let name = table.raw_get::<String>("name").unwrap_or_default();
                return Some(if name.is_empty() {
                    format!("Entity({id})")
                } else {
                    format!("Entity(\"{name}\", {id})")
                });
            }
            if let Ok(name) = table.raw_get::<String>("__neolove_component") {
                return Some(format!("Component(\"{name}\")"));
            }
            if let (Ok(r), Ok(g), Ok(b)) = (
                table.raw_get::<f64>("r"),
                table.raw_get::<f64>("g"),
                table.raw_get::<f64>("b"),
            ) {
                let a = table.raw_get::<f64>("a").unwrap_or(255.0);
                return Some(format!("Color4({r}, {g}, {b}, {a})"));
            }
            if depth >= 2 {
                return Some("{...}".to_string());
            }
            let pointer = table.to_pointer() as usize;
            if !seen.insert(pointer) {
                return Some("<cycle>".to_string());
            }
            let mut entries = Vec::new();
            for pair in table.pairs::<Value, Value>() {
                let Ok((key, value)) = pair else { continue };
                let key = match key {
                    Value::String(key) => {
                        let key = key.to_string_lossy();
                        if key.starts_with("__") {
                            continue;
                        }
                        key
                    }
                    Value::Integer(key) => key.to_string(),
                    Value::Number(key) => key.to_string(),
                    Value::Boolean(key) => key.to_string(),
                    _ => continue,
                };
                if let Some(value) = snapshot_value(lua, value, depth + 1, seen) {
                    entries.push((key, value));
                }
            }
            seen.remove(&pointer);
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let truncated = entries.len() > 16;
            entries.truncate(16);
            let mut rendered = entries
                .into_iter()
                .map(|(key, value)| format!("{key} = {value}"))
                .collect::<Vec<_>>();
            if truncated {
                rendered.push("...".to_string());
            }
            Some(format!("{{{}}}", rendered.join(", ")))
        }
        other => {
            let tostring = lua.globals().get::<Function>("tostring").ok()?;
            tostring.call::<String>(other).ok()
        }
    }
}

/// Collect public fields for the logger's inspector, skipping internal markers,
/// methods and fields represented separately by the snapshot schema.
fn snapshot_table_fields(lua: &Lua, table: &Table, excluded: &[&str]) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    for pair in table.pairs::<Value, Value>() {
        let Ok((key, value)) = pair else {
            continue;
        };
        let Value::String(name) = key else {
            continue;
        };
        let name = name.to_string_lossy();
        if name.starts_with("__") || excluded.contains(&name.as_str()) {
            continue;
        }
        let Some(rendered) = snapshot_value(lua, value, 0, &mut HashSet::new()) else {
            continue;
        };
        fields.push((name, rendered));
        if fields.len() >= 256 {
            break;
        }
    }
    fields.sort_by(|a, b| a.0.cmp(&b.0));
    fields
}

fn snapshot_component_name(component: &Table) -> String {
    component
        .raw_get::<String>("__neolove_component")
        .ok()
        .or_else(|| component.raw_get::<String>("name").ok())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Script Component".to_string())
}

pub(crate) fn is_physics_component_name(name: &str) -> bool {
    matches!(
        name,
        "Rigidbody2D" | "Collider2D" | "Rope2D" | "Bolt2D" | "LegacyBolt2D"
    )
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
        info.entity
            .get::<String>("rotation_pivot")
            .or_else(|_| info.entity.get::<String>("rotationPivot"))
            .unwrap_or_default()
            .hash(&mut hasher);
        read_optional_f32(&info.entity, "rotation_pivot_x", "rotationPivotX")
            .unwrap_or(f32::NAN)
            .to_bits()
            .hash(&mut hasher);
        read_optional_f32(&info.entity, "rotation_pivot_y", "rotationPivotY")
            .unwrap_or(f32::NAN)
            .to_bits()
            .hash(&mut hasher);
        read_optional_f32(&info.entity, "pivot_x", "pivotX")
            .unwrap_or(f32::NAN)
            .to_bits()
            .hash(&mut hasher);
        read_optional_f32(&info.entity, "pivot_y", "pivotY")
            .unwrap_or(f32::NAN)
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

        info.bolts.len().hash(&mut hasher);
        info.legacy_bolts.len().hash(&mut hasher);
    }

    hasher.finish()
}

fn physics_pivot_local_from_center(entity: &Table, width: f32, height: f32) -> (f32, f32) {
    let (pivot_x, pivot_y) = physics_pivot_fraction(entity);

    (width * (pivot_x - 0.5), height * (pivot_y - 0.5))
}

fn physics_pivot_fraction(entity: &Table) -> (f32, f32) {
    let default = if uses_middle_rotation_pivot(entity) {
        0.5
    } else {
        0.0
    };
    let pivot_x = read_optional_f32(entity, "rotation_pivot_x", "rotationPivotX")
        .or_else(|| read_optional_f32(entity, "pivot_x", "pivotX"))
        .unwrap_or(default);
    let pivot_y = read_optional_f32(entity, "rotation_pivot_y", "rotationPivotY")
        .or_else(|| read_optional_f32(entity, "pivot_y", "pivotY"))
        .unwrap_or(default);
    (pivot_x, pivot_y)
}

fn physics_pivot_local_from_top_left(entity: &Table, width: f32, height: f32) -> (f32, f32) {
    let (pivot_x, pivot_y) = physics_pivot_fraction(entity);
    (width * pivot_x, height * pivot_y)
}

fn physics_body_pose_from_entity(
    entity: &Table,
    width: f32,
    height: f32,
) -> mlua::Result<(f32, f32, f32, f32, f32)> {
    let (origin_x, origin_y, rotation) = get_global_transform(entity)?;
    let (pivot_x, pivot_y) = physics_pivot_local_from_top_left(entity, width, height);
    let (rx, ry) = rotate_point(pivot_x, pivot_y, rotation);
    Ok((origin_x + rx, origin_y + ry, rotation, pivot_x, pivot_y))
}

fn physics_entity_position_from_body(
    entity: &Table,
    body_x: f32,
    body_y: f32,
) -> mlua::Result<(f32, f32)> {
    let local_scale = read_entity_scale(entity).max(0.0);
    let width = entity.get::<f32>("size_x").unwrap_or(0.0).max(0.0) * local_scale;
    let height = entity.get::<f32>("size_y").unwrap_or(0.0).max(0.0) * local_scale;
    let (pivot_x, pivot_y) = physics_pivot_local_from_top_left(entity, width, height);
    let (pos_pivot_x, pos_pivot_y) = get_local_position_pivot_offset(entity, local_scale)?;
    let (anchor_x, anchor_y) = get_local_anchor_offset(entity)?;

    let (parent_x, parent_y) = if let Some(parent) = entity.get::<Option<Table>>("parent")? {
        let (parent_origin_x, parent_origin_y, parent_rotation) = get_global_transform(&parent)?;
        let parent_scale = get_global_scale(&parent)?.max(0.0001);
        let (rx, ry) = rotate_point(
            body_x - parent_origin_x,
            body_y - parent_origin_y,
            -parent_rotation,
        );
        (rx / parent_scale, ry / parent_scale)
    } else {
        (body_x, body_y)
    };

    Ok((
        parent_x - anchor_x + pos_pivot_x - pivot_x,
        parent_y - anchor_y + pos_pivot_y - pivot_y,
    ))
}

fn physics_body_position_from_entity_position(
    entity: &Table,
    local_x: f32,
    local_y: f32,
) -> mlua::Result<(f32, f32)> {
    let local_scale = read_entity_scale(entity).max(0.0);
    let width = entity.get::<f32>("size_x").unwrap_or(0.0).max(0.0) * local_scale;
    let height = entity.get::<f32>("size_y").unwrap_or(0.0).max(0.0) * local_scale;
    let (pivot_x, pivot_y) = physics_pivot_local_from_top_left(entity, width, height);
    let (pos_pivot_x, pos_pivot_y) = get_local_position_pivot_offset(entity, local_scale)?;
    let (anchor_x, anchor_y) = get_local_anchor_offset(entity)?;
    let local_pivot_x = anchor_x + local_x - pos_pivot_x + pivot_x;
    let local_pivot_y = anchor_y + local_y - pos_pivot_y + pivot_y;

    if let Some(parent) = entity.get::<Option<Table>>("parent")? {
        let (parent_origin_x, parent_origin_y, parent_rotation) = get_global_transform(&parent)?;
        let parent_scale = get_global_scale(&parent)?.max(0.0);
        let (rx, ry) = rotate_point(
            local_pivot_x * parent_scale,
            local_pivot_y * parent_scale,
            parent_rotation,
        );
        Ok((parent_origin_x + rx, parent_origin_y + ry))
    } else {
        Ok((local_pivot_x, local_pivot_y))
    }
}

fn triangle_local_points(
    corner: TriangleCorner,
    pivot_x: f32,
    pivot_y: f32,
    offset_x: f32,
    offset_y: f32,
    collider_w: f32,
    collider_h: f32,
) -> ((f32, f32), (f32, f32), (f32, f32)) {
    let x0 = offset_x - pivot_x;
    let y0 = offset_y - pivot_y;
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
        Self::with_data_root(env.clone(), env)
    }

    pub fn with_data_root(env: PathBuf, data_root: PathBuf) -> Runtime {
        Runtime {
            entities: Rc::new(RefCell::new(HashMap::new())),
            entity_listeners: Rc::new(RefCell::new(HashMap::new())),
            next_entity_listener_id: Rc::new(RefCell::new(1)),
            hovered_entity_ids: Rc::new(RefCell::new(HashSet::new())),
            entity_hover_initialized: Rc::new(Cell::new(false)),
            systems: Rc::new(RefCell::new(Vec::new())),
            pending_component_awakes: Rc::new(RefCell::new(Vec::new())),
            environment: env,
            data_root,
            lua: Lua::new(),
            entity_max: 1,
            exit_requested: Rc::new(RefCell::new(false)),
            exit_reason: Rc::new(RefCell::new(None)),
            physics_world: None,
            physics_signature: 0,
            platform: new_shared_platform_state(),
            render_state: new_shared_render_state(),
            root_table: None,
            mouse_table_key: None,
            window_table_key: None,
            app_table_key: None,
            max_fps_state: Rc::new(RefCell::new(None)),
            show_fps_state: Rc::new(RefCell::new(None)),
            async_tasks: Rc::new(RefCell::new(Vec::new())),
            async_cancelled: Rc::new(RefCell::new(HashSet::new())),
            next_async_id: Rc::new(Cell::new(1)),
            log_sink: Rc::new(RefCell::new(None)),
        }
    }

    /// Mirror runtime `print`/error output to `sink` (in addition to stdout) so
    /// the editor's logger window can display it live.
    pub fn set_log_sink(&self, sink: std::sync::mpsc::Sender<RuntimeLogLine>) {
        *self.log_sink.borrow_mut() = Some(sink);
    }

    /// Push a line to the log sink if one is installed. Never fails the caller —
    /// a disconnected observer simply drops the line.
    fn forward_log(
        log_sink: &Rc<RefCell<Option<std::sync::mpsc::Sender<RuntimeLogLine>>>>,
        level: &str,
        message: String,
    ) {
        if let Some(sink) = log_sink.borrow().as_ref() {
            let _ = sink.send(RuntimeLogLine {
                level: level.to_string(),
                message,
            });
        }
    }

    /// Walk the live entity tree into a serializable snapshot for the editor's
    /// logger window. Reads the authoritative live Lua entity/component tables.
    /// Best-effort: anything unreadable is skipped.
    pub fn snapshot_entities(&self) -> Vec<EntitySnapshot> {
        let entities = self.entities.borrow();
        let mut out = Vec::with_capacity(entities.len());
        for entity in entities.values() {
            let Ok(table) = self.lua.registry_value::<Table>(&entity.luau_key) else {
                continue;
            };
            let read_f32 = |key: &str| {
                table
                    .get::<f32>(key)
                    .ok()
                    .filter(|v| v.is_finite())
                    .unwrap_or(0.0)
            };
            let components = table
                .raw_get::<Table>("components")
                .ok()
                .map(|components| {
                    components
                        .sequence_values::<Table>()
                        .filter_map(Result::ok)
                        .map(|component| ComponentSnapshot {
                            name: snapshot_component_name(&component),
                            fields: snapshot_table_fields(&self.lua, &component, &["entity"]),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let parent = table
                .raw_get::<Table>("parent")
                .ok()
                .and_then(|parent| parent.raw_get::<usize>("id").ok());
            let mut fields = snapshot_table_fields(
                &self.lua,
                &table,
                &["id", "name", "parent", "children", "components"],
            );
            if !fields.iter().any(|(name, _)| name == "enabled") {
                fields.push(("enabled".to_string(), "true".to_string()));
                fields.sort_by(|a, b| a.0.cmp(&b.0));
            }
            out.push(EntitySnapshot {
                id: entity.id,
                name: table.get::<String>("name").unwrap_or_default(),
                parent,
                x: read_f32("x"),
                y: read_f32("y"),
                rotation: read_f32("rotation"),
                scale: table
                    .get::<f32>("scale")
                    .ok()
                    .filter(|v| v.is_finite())
                    .unwrap_or(1.0),
                enabled: table.get::<bool>("enabled").unwrap_or(true),
                fields,
                components,
            });
        }
        out.sort_by_key(|entity| entity.id);
        out
    }

    pub(crate) fn platform_state(&self) -> SharedPlatformState {
        self.platform.clone()
    }

    pub(crate) fn render_state(&self) -> SharedRenderState {
        self.render_state.clone()
    }

    pub fn set_platform_window_state(&self, width: f32, height: f32) {
        let width = if width.is_finite() {
            width.max(0.0)
        } else {
            0.0
        };
        let height = if height.is_finite() {
            height.max(0.0)
        } else {
            0.0
        };
        let mut platform = lock_platform_state(&self.platform);
        platform.set_window(WindowState { width, height });
    }

    pub fn set_platform_mouse_state(&self, x: f32, y: f32) {
        let x = if x.is_finite() { x } else { 0.0 };
        let y = if y.is_finite() { y } else { 0.0 };
        let mut platform = lock_platform_state(&self.platform);
        platform.set_mouse_position(x, y);
    }

    pub fn max_fps(&self) -> Option<f32> {
        #[cfg(target_os = "emscripten")]
        {
            return *self.max_fps_state.borrow();
        }

        #[cfg(not(target_os = "emscripten"))]
        self.resolve_app_max_fps().ok().flatten()
    }

    pub fn show_fps(&self) -> bool {
        #[cfg(target_os = "emscripten")]
        {
            return (*self.show_fps_state.borrow()).unwrap_or(true);
        }

        #[cfg(not(target_os = "emscripten"))]
        self.resolve_app_show_fps().unwrap_or(true)
    }

    pub fn exit_requested(&self) -> bool {
        *self.exit_requested.borrow()
    }

    pub fn exit_reason(&self) -> Option<String> {
        self.exit_reason.borrow().clone()
    }

    #[cfg(not(target_os = "emscripten"))]
    fn ensure_runtime_table(
        lua: &Lua,
        globals: &Table,
        key_slot: &mut Option<RegistryKey>,
        global_name: &str,
    ) -> mlua::Result<Table> {
        if let Some(key) = key_slot.as_ref() {
            return lua.registry_value(key);
        }

        let table = lua.create_table()?;
        globals.set(global_name, table.clone())?;
        *key_slot = Some(lua.create_registry_value(table.clone())?);
        Ok(table)
    }

    #[cfg(target_os = "emscripten")]
    fn install_mouse_table_proxy(&mut self) -> mlua::Result<()> {
        if self.mouse_table_key.is_some() {
            return Ok(());
        }

        let table = self.lua.create_table()?;
        let metatable = self.lua.create_table()?;
        let platform = self.platform.clone();
        let index = self
            .lua
            .create_function(move |_lua, (_table, key): (Table, String)| {
                let mouse = lock_platform_state(&platform).mouse();
                Ok(match key.as_str() {
                    "x" => Value::Number(mouse.x as f64),
                    "y" => Value::Number(mouse.y as f64),
                    "dx" | "delta_x" | "deltaX" => Value::Number(mouse.delta_x as f64),
                    "dy" | "delta_y" | "deltaY" => Value::Number(mouse.delta_y as f64),
                    _ => Value::Nil,
                })
            })?;
        metatable.raw_set("__index", index)?;
        table.set_metatable(Some(metatable))?;
        table.set_readonly(true);
        let globals = self.lua.globals();
        globals.raw_set("mouse", table.clone())?;
        self.mouse_table_key = Some(self.lua.create_registry_value(table)?);
        Ok(())
    }

    #[cfg(target_os = "emscripten")]
    fn install_window_table_proxy(&mut self) -> mlua::Result<()> {
        if self.window_table_key.is_some() {
            return Ok(());
        }

        let table = self.lua.create_table()?;
        let metatable = self.lua.create_table()?;
        let platform = self.platform.clone();
        let index = self
            .lua
            .create_function(move |_lua, (_table, key): (Table, String)| {
                let window = lock_platform_state(&platform).window();
                Ok(match key.as_str() {
                    "x" | "width" => Value::Number(window.width as f64),
                    "y" | "height" => Value::Number(window.height as f64),
                    _ => Value::Nil,
                })
            })?;
        metatable.raw_set("__index", index)?;
        table.set_metatable(Some(metatable))?;
        table.set_readonly(true);
        let globals = self.lua.globals();
        globals.raw_set("window", table.clone())?;
        self.window_table_key = Some(self.lua.create_registry_value(table)?);
        Ok(())
    }

    fn set_mouse_table(&mut self) -> mlua::Result<()> {
        #[cfg(target_os = "emscripten")]
        self.install_mouse_table_proxy()?;

        #[cfg(not(target_os = "emscripten"))]
        {
            let mouse = lock_platform_state(&self.platform).mouse();
            let globals = self.lua.globals();
            let mouse_table = Self::ensure_runtime_table(
                &self.lua,
                &globals,
                &mut self.mouse_table_key,
                "mouse",
            )?;
            mouse_table.raw_set("x", mouse.x)?;
            mouse_table.raw_set("y", mouse.y)?;
        }
        Ok(())
    }

    fn set_window_table(&mut self) -> mlua::Result<()> {
        #[cfg(target_os = "emscripten")]
        self.install_window_table_proxy()?;

        #[cfg(not(target_os = "emscripten"))]
        {
            let window = lock_platform_state(&self.platform).window();
            let globals = self.lua.globals();
            let table = Self::ensure_runtime_table(
                &self.lua,
                &globals,
                &mut self.window_table_key,
                "window",
            )?;
            table.raw_set("x", window.width)?;
            table.raw_set("y", window.height)?;

            if let Some(root) = self.root_table.as_ref() {
                root.raw_set("size_x", window.width)?;
                root.raw_set("size_y", window.height)?;
            }
        }
        Ok(())
    }

    fn app_table(&self) -> mlua::Result<Table> {
        match self.lua.globals().raw_get::<Value>("app")? {
            Value::Table(app) => Ok(app),
            _ => {
                if let Some(key) = &self.app_table_key {
                    return self.lua.registry_value(key);
                }
                Err(mlua::Error::RuntimeError(
                    "global app table is missing".to_string(),
                ))
            }
        }
    }

    fn resolve_app_clear_color(&self) -> mlua::Result<PlatformColor> {
        let app = self.app_table()?;
        let bg = match app.raw_get::<Value>("bg")? {
            Value::Table(bg) => Some(bg),
            _ => self.lua.globals().get::<Table>("bg").ok(),
        };
        let Some(bg) = bg else {
            return Ok(PlatformColor::WHITE);
        };
        Ok(table_to_platform_color(&bg))
    }

    fn resolve_app_antialiasing(&self) -> mlua::Result<Antialiasing> {
        let app = self.app_table()?;
        let value = app
            .raw_get::<Option<String>>("antiAliasing")?
            .or_else(|| app.raw_get::<Option<String>>("antialiasing").ok().flatten())
            .unwrap_or_else(|| "high".to_string());
        Ok(Antialiasing::parse(&value))
    }

    #[cfg(not(target_os = "emscripten"))]
    fn resolve_app_max_fps(&self) -> mlua::Result<Option<f32>> {
        let app = self.app_table()?;
        Ok(app
            .raw_get::<Option<f32>>("maxFps")?
            .filter(|fps| fps.is_finite() && *fps > 0.0))
    }

    #[cfg(not(target_os = "emscripten"))]
    fn resolve_app_show_fps(&self) -> mlua::Result<bool> {
        let app = self.app_table()?;
        Ok(app.raw_get::<Option<bool>>("showFps")?.unwrap_or(true))
    }

    fn install_async_module(&self) -> mlua::Result<()> {
        self.async_tasks.borrow_mut().clear();
        self.async_cancelled.borrow_mut().clear();
        self.next_async_id.set(1);

        let module = self.lua.create_table()?;
        let metatable = self.lua.create_table()?;
        let tasks = self.async_tasks.clone();
        let cancelled = self.async_cancelled.clone();
        let next_id = self.next_async_id.clone();
        let call =
            self.lua
                .create_function(move |lua, (_module, callback): (Table, Function)| {
                    let id = next_id.get();
                    next_id.set(id.saturating_add(1));

                    let handle = lua.create_table()?;
                    handle.raw_set("id", id)?;
                    handle.raw_set("done", false)?;
                    handle.raw_set("cancelled", false)?;
                    handle.raw_set("status", "queued")?;
                    handle.raw_set("error", Value::Nil)?;
                    handle.raw_set("result", Value::Nil)?;
                    handle.raw_set("results", lua.create_table()?)?;

                    let is_done =
                        lua.create_function(|_lua, handle: Table| handle.raw_get::<bool>("done"))?;
                    handle.raw_set("isDone", is_done.clone())?;
                    handle.raw_set("IsDone", is_done)?;

                    let task_cancelled = cancelled.clone();
                    let cancel = lua.create_function(move |_lua, handle: Table| {
                        if handle.raw_get::<bool>("done")? {
                            return Ok(false);
                        }
                        task_cancelled.borrow_mut().insert(id);
                        handle.raw_set("done", true)?;
                        handle.raw_set("cancelled", true)?;
                        handle.raw_set("status", "cancelled")?;
                        Ok(true)
                    })?;
                    handle.raw_set("cancel", cancel.clone())?;
                    handle.raw_set("Cancel", cancel)?;

                    let get_status = lua.create_function(|_lua, handle: Table| {
                        handle.raw_get::<String>("status")
                    })?;
                    handle.raw_set("getStatus", get_status.clone())?;
                    handle.raw_set("GetStatus", get_status)?;

                    let get_error = lua.create_function(|_lua, handle: Table| {
                        handle.raw_get::<Option<String>>("error")
                    })?;
                    handle.raw_set("getError", get_error.clone())?;
                    handle.raw_set("GetError", get_error)?;

                    let get_result = lua.create_function(|_lua, handle: Table| {
                        let results = handle.raw_get::<Table>("results")?;
                        let mut values = Vec::with_capacity(results.raw_len());
                        for index in 1..=results.raw_len() {
                            values.push(results.raw_get::<Value>(index)?);
                        }
                        Ok(MultiValue::from_vec(values))
                    })?;
                    handle.raw_set("getResult", get_result.clone())?;
                    handle.raw_set("GetResult", get_result)?;

                    tasks.borrow_mut().push(AsyncTask {
                        id,
                        thread: lua.create_thread(callback)?,
                        handle: handle.clone(),
                    });
                    Ok(handle)
                })?;
        metatable.raw_set("__call", call)?;
        module.set_metatable(Some(metatable))?;

        let yield_now: Function = self
            .lua
            .load("return function(...) return coroutine.yield(...) end")
            .eval()?;
        module.raw_set("yield", yield_now)?;

        let tasks = self.async_tasks.clone();
        module.raw_set(
            "count",
            self.lua.create_function(move |_lua, ()| {
                Ok(tasks
                    .borrow()
                    .iter()
                    .filter(|task| !task.handle.raw_get::<bool>("done").unwrap_or(true))
                    .count())
            })?,
        )?;

        let tasks = self.async_tasks.clone();
        let cancelled = self.async_cancelled.clone();
        module.raw_set(
            "cancelAll",
            self.lua.create_function(move |_lua, ()| {
                let mut count = 0;
                for task in tasks.borrow().iter() {
                    if !task.handle.raw_get::<bool>("done")? {
                        cancelled.borrow_mut().insert(task.id);
                        task.handle.raw_set("done", true)?;
                        task.handle.raw_set("cancelled", true)?;
                        task.handle.raw_set("status", "cancelled")?;
                        count += 1;
                    }
                }
                Ok(count)
            })?,
        )?;

        self.lua.globals().raw_set("async", module)
    }

    fn poll_async_tasks(&self) {
        let tasks = std::mem::take(&mut *self.async_tasks.borrow_mut());
        let mut remaining = Vec::with_capacity(tasks.len());

        for task in tasks {
            if self.async_cancelled.borrow_mut().remove(&task.id) {
                continue;
            }

            let _ = task.handle.raw_set("status", "running");
            match task.thread.resume::<MultiValue>(()) {
                Ok(_values) if task.thread.status() == ThreadStatus::Resumable => {
                    let _ = task.handle.raw_set("status", "suspended");
                    remaining.push(task);
                }
                Ok(values) => {
                    let values = values.into_vec();
                    let results = match task.handle.raw_get::<Table>("results") {
                        Ok(results) => results,
                        Err(error) => {
                            eprintln!(
                                "\x1b[31mLua async task error:\x1b[0m Failed to store result: {}",
                                describe_lua_error(&error)
                            );
                            continue;
                        }
                    };
                    for (index, value) in values.iter().cloned().enumerate() {
                        let _ = results.raw_set(index + 1, value);
                    }
                    let _ = task
                        .handle
                        .raw_set("result", values.first().cloned().unwrap_or(Value::Nil));
                    let _ = task.handle.raw_set("status", "completed");
                    let _ = task.handle.raw_set("done", true);
                }
                Err(error) => {
                    let message = describe_lua_error(&error);
                    let _ = task.handle.raw_set("error", message.clone());
                    let _ = task.handle.raw_set("status", "error");
                    let _ = task.handle.raw_set("done", true);
                    eprintln!("\x1b[31mLua async task error:\x1b[0m\n{message}");
                }
            }
        }

        let mut newly_queued = self.async_tasks.borrow_mut();
        remaining.append(&mut newly_queued);
        *newly_queued = remaining;
    }

    fn has_pending_component_awakes(&self) -> bool {
        !self.pending_component_awakes.borrow().is_empty()
    }

    fn drain_pending_component_awakes(&self) -> mlua::Result<()> {
        loop {
            let pending = std::mem::take(&mut *self.pending_component_awakes.borrow_mut());
            if pending.is_empty() {
                return Ok(());
            }

            for PendingComponentAwake { entity, component } in pending {
                if !component_is_attached_to_entity(&entity, &component) {
                    continue;
                }
                let Ok(awake) = component.get::<Function>("awake") else {
                    continue;
                };
                let component_name = describe_component_name(&component, Some(&entity));
                protect_lua_call(
                    &format!("running component awake callback ({component_name})"),
                    || awake.call::<()>((&entity, &component)),
                )?;
            }
        }
    }

    /// Replace `print` with one that still writes to stdout but also mirrors
    /// each line to the optional log sink (the editor's logger window).
    fn install_log_forwarder(&self) -> mlua::Result<()> {
        let log_sink = self.log_sink.clone();
        let print = self
            .lua
            .create_function(move |lua, args: mlua::Variadic<Value>| {
                // Mirror Luau's `print`: tab-separated `tostring` of each arg.
                let tostring: mlua::Function = lua.globals().get("tostring")?;
                let mut parts = Vec::with_capacity(args.len());
                for value in args {
                    parts.push(tostring.call::<String>(value)?);
                }
                let line = parts.join("\t");
                println!("{line}");
                Runtime::forward_log(&log_sink, "info", line);
                Ok(())
            })?;
        self.lua.globals().set("print", print)?;
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

        self.install_log_forwarder()?;

        self.set_mouse_table()?;
        self.set_window_table()?;

        // App
        {
            *self.max_fps_state.borrow_mut() = None;
            *self.show_fps_state.borrow_mut() = Some(true);
            let app = self.lua.create_table()?;
            app.set("bg", color4_table(&self.lua, 255, 255, 255, 255)?)?;
            app.set("maxFps", Value::Nil)?;
            app.set("showFps", true)?;
            app.set("nearestNeighborScaling", true)?;
            app.set("antiAliasing", "high")?;
            {
                let mut platform = lock_platform_state(&self.platform);
                platform.set_clear_color(PlatformColor::WHITE);
            }

            let _max_fps_state = self.max_fps_state.clone();
            let set_max_fps = self.lua.create_function(move |lua, fps: Option<f32>| {
                let normalized = fps.filter(|fps| fps.is_finite() && *fps > 0.0);
                #[cfg(target_os = "emscripten")]
                {
                    *_max_fps_state.borrow_mut() = normalized;
                }
                let app: Table = lua.globals().get("app")?;
                match normalized {
                    Some(fps) => app.raw_set("maxFps", fps)?,
                    None => app.raw_set("maxFps", Value::Nil)?,
                }
                Ok(())
            })?;
            app.set("setMaxFps", set_max_fps)?;

            let _max_fps_state = self.max_fps_state.clone();
            let get_max_fps = self.lua.create_function(move |_lua, ()| {
                #[cfg(target_os = "emscripten")]
                {
                    Ok(*_max_fps_state.borrow())
                }
                #[cfg(not(target_os = "emscripten"))]
                {
                    let app: Table = _lua.globals().get("app")?;
                    Ok(app
                        .raw_get::<Option<f32>>("maxFps")?
                        .filter(|fps| fps.is_finite() && *fps > 0.0))
                }
            })?;
            app.set("getMaxFps", get_max_fps)?;

            let _show_fps_state = self.show_fps_state.clone();
            let set_show_fps = self
                .lua
                .create_function(move |lua, enabled: Option<bool>| {
                    let enabled = enabled.unwrap_or(true);
                    #[cfg(target_os = "emscripten")]
                    {
                        *_show_fps_state.borrow_mut() = Some(enabled);
                    }
                    let app: Table = lua.globals().get("app")?;
                    app.raw_set("showFps", enabled)?;
                    Ok(())
                })?;
            app.set("setShowFps", set_show_fps)?;

            let _show_fps_state = self.show_fps_state.clone();
            let get_show_fps = self.lua.create_function(move |_lua, ()| {
                #[cfg(target_os = "emscripten")]
                {
                    Ok((*_show_fps_state.borrow()).unwrap_or(true))
                }
                #[cfg(not(target_os = "emscripten"))]
                {
                    let app: Table = _lua.globals().get("app")?;
                    Ok(app.raw_get::<Option<bool>>("showFps")?.unwrap_or(true))
                }
            })?;
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

            let set_antialiasing = self.lua.create_function(move |lua, mode: Option<String>| {
                let mode = mode.unwrap_or_else(|| "high".to_string());
                let normalized = match Antialiasing::parse(&mode) {
                    Antialiasing::Off => "off",
                    Antialiasing::Standard => "standard",
                    Antialiasing::High => "high",
                };
                let app: Table = lua.globals().get("app")?;
                app.set("antiAliasing", normalized)
            })?;
            app.set("setAntiAliasing", set_antialiasing)?;

            let get_antialiasing = self.lua.create_function(move |lua, ()| {
                let app: Table = lua.globals().get("app")?;
                Ok(app
                    .get::<Option<String>>("antiAliasing")?
                    .unwrap_or_else(|| "high".to_string()))
            })?;
            app.set("getAntiAliasing", get_antialiasing)?;

            self.app_table_key = Some(self.lua.create_registry_value(app.clone())?);
            self.lua.globals().set("app", app)?;
        }

        let env_root = self
            .environment
            .canonicalize()
            .map_err(mlua::Error::external)?;
        fs::create_dir_all(&self.data_root).map_err(mlua::Error::external)?;
        let data_root = self
            .data_root
            .canonicalize()
            .map_err(mlua::Error::external)?;

        {
            let softrequire_root = env_root.clone();
            let softrequire_cache = Rc::new(RefCell::new(HashMap::<String, RegistryKey>::new()));
            let softrequire_registry_lua = self.lua.clone();
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
                                let cached: Value =
                                    softrequire_registry_lua.registry_value(registry_key)?;
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

                        let registry_key =
                            softrequire_registry_lua.create_registry_value(result.clone())?;
                        softrequire_cache
                            .borrow_mut()
                            .insert(path_key, registry_key);
                        return Ok(result);
                    }

                    let source_key = softrequire_source_cache_key(&module_input);
                    {
                        let cache = softrequire_cache.borrow();
                        if let Some(registry_key) = cache.get(&source_key) {
                            let cached: Value =
                                softrequire_registry_lua.registry_value(registry_key)?;
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

                    let registry_key =
                        softrequire_registry_lua.create_registry_value(result.clone())?;
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
        crate::assets::add_assets_module_with_data_root(
            &self.lua,
            env_root.clone(),
            data_root.clone(),
            self.platform.clone(),
            self.render_state.clone(),
        )?;
        crate::fs_module::add_fs_module_with_data_root(&self.lua, env_root.clone(), data_root)?;
        crate::android_module::add_android_module(&self.lua)?;
        crate::mobile_module::add_mobile_module(&self.lua, self.platform.clone())?;
        crate::media::add_media_module(&self.lua)?;
        crate::http::add_http_module(&self.lua)?;
        crate::servers::add_servers_module(&self.lua, env_root.clone())?;
        crate::commands::add_commands_module(&self.lua, env_root.clone())?;
        crate::shader::add_shader_module(&self.lua, env_root.clone())?;
        crate::tweening::add_tweening_module(&self.lua)?;
        crate::animation::add_animation_module(&self.lua, env_root.clone())?;
        self.install_async_module()?;

        // Inspector declarations are editor metadata. At runtime they evaluate
        // to their first/default argument so component code remains ordinary
        // Luau with no wrapper objects involved.
        let inspector = self.lua.create_function(|_lua, arguments: MultiValue| {
            Ok(arguments.into_iter().next().unwrap_or(Value::Nil))
        })?;
        self.lua.globals().set("Inspector", inspector)?;

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
        let exit_reason = self.exit_reason.clone();
        let die = self
            .lua
            .create_function(move |_lua, reason: Option<String>| {
                let reason = reason
                    .map(|reason| reason.trim().to_string())
                    .filter(|reason| !reason.is_empty())
                    .unwrap_or_else(|| "die() called".to_string());
                #[cfg(target_os = "emscripten")]
                web_debug_log(&format!("die() called: {reason}"));
                *exit_reason.borrow_mut() = Some(reason);
                *exit_requested.borrow_mut() = true;
                Ok(())
            })?;

        self.lua.globals().set("die", die)?;

        let listener_state = self.entity_listeners.clone();
        let next_listener_id = self.next_entity_listener_id.clone();
        let listener_registry_lua = self.lua.clone();
        let listen_impl = self.lua.create_function(
            move |lua, (entity, event_name, callback): (Table, String, Function)| {
                let event = EntityListenEvent::from_name(&event_name).ok_or_else(|| {
                    mlua::Error::external(
                        "entity listen event must be one of leftClick, rightClick, middleClick, scrollUp, scrollDown, mouseEntered, or mouseExited",
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
                let callback_key = listener_registry_lua.create_registry_value(callback)?;

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
                    listener_registry_lua.clone(),
                )
            },
        )?;
        self.lua
            .globals()
            .set("__neolove_entity_listen_impl", listen_impl)?;

        // Transforms
        {
            let raycast_registry_lua = self.lua.clone();
            let get_world_position = self.lua.create_function(move |_lua, entity: Table| {
                let (x, y) = get_global_position(&entity)?;
                Ok((x, y))
            })?;
            let get_world_rotation = self.lua.create_function(move |_lua, entity: Table| {
                let rotation = get_global_rotation(&entity)?;
                Ok(rotation)
            })?;
            let look_at = self.lua.create_function(
                move |_lua, (from_x, from_y, to_x, to_y): (f32, f32, f32, f32)| {
                    Ok(look_at_rotation(from_x, from_y, to_x, to_y))
                },
            )?;

            let do_they_overlap = self.lua.create_function(move |_lua, entities: Table| {
                // Collect the Lua entity tables once before comparing them. Re-entering
                // `pairs()` on the same table while the outer iterator is still alive has
                // been fragile in the web runtime once gameplay starts spawning enemies.
                let mut collected = Vec::new();
                for pair in entities.pairs::<Value, Table>() {
                    let (_, entity) = pair?;
                    collected.push(entity);
                }

                for (index, entity1) in collected.iter().enumerate() {
                    let (x1, y1) = get_global_position(entity1)?;
                    let (w1, h1) = get_global_size(entity1)?;

                    for entity2 in collected.iter().skip(index + 1) {
                        let (x2, y2) = get_global_position(entity2)?;
                        let (w2, h2) = get_global_size(entity2)?;

                        if x1 < x2 + w2 && x1 + w1 > x2 && y1 < y2 + h2 && y1 + h1 > y2 {
                            return Ok(true);
                        }
                    }
                }

                Ok(false)
            })?;

            let entities_in_front_registry_lua = self.lua.clone();
            let entities_in_front_state = self.entities.clone();
            let get_entities_in_front = self.lua.create_function(
                move |lua, (world_x, world_y, minimum_z): (f32, f32, Option<f64>)| {
                    let minimum_z = minimum_z.unwrap_or(f64::NEG_INFINITY);
                    let mut matches = Vec::<(Table, f64, usize)>::new();

                    for (id, entity_data) in entities_in_front_state.borrow().iter() {
                        if *id == 0 {
                            continue;
                        }

                        let entity = match entities_in_front_registry_lua
                            .registry_value::<Table>(&entity_data.luau_key)
                        {
                            Ok(entity) => entity,
                            Err(_) => continue,
                        };
                        let z = entity.get::<f64>("z").unwrap_or(0.0);
                        if z < minimum_z {
                            continue;
                        }
                        if point_hits_entity(&entity, world_x, world_y).unwrap_or(false) {
                            matches.push((entity, z, *id));
                        }
                    }

                    matches.sort_by(|a, b| compare_entity_order(a.1, a.2, b.1, b.2).reverse());
                    let result = lua.create_table_with_capacity(matches.len(), 0)?;
                    for (index, (entity, _, _)) in matches.into_iter().enumerate() {
                        result.raw_set(index + 1, entity)?;
                    }
                    Ok(result)
                },
            )?;

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

                        let entity = match raycast_registry_lua
                            .registry_value::<Table>(&entity_data.luau_key)
                        {
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
            transforms.set("lookAt", look_at.clone())?;
            transforms.set("look_at", look_at)?;

            transforms.set("doTheyOverlap", do_they_overlap)?;
            transforms.set("GetEntitiesInFront", get_entities_in_front.clone())?;
            transforms.set("getEntitiesInFront", get_entities_in_front)?;
            transforms.set("raycast", raycast)?;
        }

        // Systems
        {
            let systems = self.systems.clone();
            let systems_registry_lua = self.lua.clone();
            let add_system = self.lua.create_function(move |_lua, system: Table| {
                if let Ok(Value::Function(awake)) = system.get::<Value>("awake") {
                    protect_lua_call("running system awake callback", || {
                        awake.call::<()>(system.clone())
                    })?;
                }
                let key = systems_registry_lua.create_registry_value(system)?;
                systems.try_borrow_mut().map_err(|_| {
                    mlua::Error::external(
                        "cannot add a system while the system registry is already being changed",
                    )
                })?.push(key);
                Ok(())
            })?;

            ecs.set("addSystem", add_system)?;
        }

        // Entities
        {
            let entities = self.entities.clone();
            let entities_delete = self.entities.clone();
            let entity_listeners = self.entity_listeners.clone();
            let listener_cleanup_lua = self.lua.clone();
            let entity_max = Rc::new(RefCell::new(self.entity_max));
            let entity_max_clone = entity_max.clone();
            let entities_registry_lua = self.lua.clone();
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

                        let id = {
                            let mut max = entity_max_clone.try_borrow_mut().map_err(|_| {
                                mlua::Error::external(
                                    "cannot create an entity while another entity is being created",
                                )
                            })?;
                            *max = max.saturating_add(1);
                            *max
                        };

                        luau.set("id", id)?;

                        let reg_key = entities_registry_lua.create_registry_value(&luau)?;

                        let entity = hierarchy::Entity {
                            components: Vec::new(),
                            children: Vec::new(),
                            parent: None,
                            id,
                            luau_key: reg_key,
                        };

                        entities.try_borrow_mut().map_err(|_| {
                            mlua::Error::external(
                                "cannot create an entity while the entity registry is being read or changed",
                            )
                        })?.insert(id, entity);

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

                let mut entities = entities_delete.try_borrow_mut().map_err(|_| {
                    mlua::Error::external(
                        "cannot delete an entity while the entity registry is being read or changed",
                    )
                })?;
                for id in &ids_to_remove {
                    entities.remove(id);
                }
                drop(entities);

                disconnect_entity_listeners_for_entities(
                    &listener_cleanup_lua,
                    &entity_listeners,
                    &ids_to_remove,
                )?;

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
            let window = lock_platform_state(&self.platform).window();
            root_table.raw_set("size_x", window.width)?;
            root_table.raw_set("size_y", window.height)?;

            let root_key = self.lua.create_registry_value(&root_table)?;
            let root_entity = hierarchy::Entity {
                components: Vec::new(),
                children: Vec::new(),
                parent: None,
                id: 0,
                luau_key: root_key,
            };
            self.entities.borrow_mut().insert(0, root_entity);
            self.root_table = Some(root_table.clone());
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

            let pending_component_awakes = self.pending_component_awakes.clone();
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
                        let is_core_component =
                            comp.raw_get::<bool>("__neolove_core_component").unwrap_or(false);
                        if is_core_component
                            && let Ok(Value::Function(awake)) = comp.get::<Value>("awake")
                        {
                            protect_lua_call(
                                &format!("running component awake callback ({component_name})"),
                                || awake.call::<()>((&entity, &comp)),
                            )?;
                        }
                        if let Ok(component_kind) = comp.get::<String>("__neolove_component") {
                            if is_physics_component_name(&component_kind) {
                                let current = entity
                                    .raw_get::<i64>("__neolove_physics_component_count")
                                    .unwrap_or(0)
                                    .max(0);
                                let next = current.saturating_add(1);
                                entity.raw_set("__neolove_physics_component_count", next)?;
                                entity.raw_set("__neolove_has_physics_components", next > 0)?;
                            }
                        }
                        components.push(&comp)?;
                        if !is_core_component && comp.get::<Function>("awake").is_ok() {
                            pending_component_awakes.borrow_mut().push(PendingComponentAwake {
                                entity: entity.clone(),
                                component: comp.clone(),
                            });
                        }
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
                        let removed_physics_component = component
                            .get::<String>("__neolove_component")
                            .map(|name| is_physics_component_name(&name))
                            .unwrap_or(false);
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
                        if removed_physics_component {
                            let current = entity
                                .raw_get::<i64>("__neolove_physics_component_count")
                                .unwrap_or(0)
                                .max(0);
                            let next = current.saturating_sub(1);
                            entity.raw_set("__neolove_physics_component_count", next)?;
                            entity.raw_set("__neolove_has_physics_components", next > 0)?;
                        }
                        Ok(true)
                    })?;

            ecs.set("removeComponent", remove_component)?;
        }

        // Load a `.neoscene` file authored in the visual editor, instantiating
        // its entities and components into the running world. This reuses the
        // editor's exact Luau code generation, so what you build in the editor
        // and what `loadScene` produces are guaranteed to match.
        {
            let load_scene_root = env_root.clone();
            let load_scene_chunk_name = format!("@{}", env_root.join("main").display());
            let load_scene_entities = self.entities.clone();
            let load_scene_entity_listeners = self.entity_listeners.clone();
            let load_scene_pending_awakes = self.pending_component_awakes.clone();
            let load_scene_hovered_entities = self.hovered_entity_ids.clone();
            let load_scene_hover_initialized = self.entity_hover_initialized.clone();
            let load_scene = self.lua.create_function(move |lua, path: String| {
                let scene_path = resolve_project_path(&load_scene_root, &path);
                let bytes = std::fs::read(&scene_path).map_err(|e| {
                    mlua::Error::RuntimeError(format!(
                        "loadScene: failed to read '{}' (resolved to '{}'): {e}",
                        path,
                        scene_path.display()
                    ))
                })?;
                let scene = crate::scene::Scene::from_bytes(&bytes)
                    .map_err(|e| mlua::Error::RuntimeError(format!("loadScene: {e}")))?;
                let ecs: Table = lua.globals().get("ecs")?;
                let root_table: Table = ecs.get("root")?;
                clear_scene_entities(
                    lua,
                    &load_scene_entities,
                    &load_scene_entity_listeners,
                    &load_scene_pending_awakes,
                    &load_scene_hovered_entities,
                    &load_scene_hover_initialized,
                    &root_table,
                )?;
                lua.load(scene.to_luau_runtime())
                    .set_name(load_scene_chunk_name.clone())
                    .exec()
            })?;
            ecs.set("loadScene", load_scene)?;
        }

        self.lua.globals().set("ecs", ecs)?;
        self.lua.globals().set("transform", transforms.clone())?;
        self.lua.globals().set("transforms", transforms)?;
        crate::prefabs::add_prefab_module(&self.lua, &env_root)?;

        self.lua
            .load(entry_file.as_path())
            .set_name(format!("@{}", entry_module.display()))
            .exec()?;

        if let Ok(clear) = self.resolve_app_clear_color() {
            let mut platform = lock_platform_state(&self.platform);
            platform.set_clear_color(clear);
        }

        #[cfg(target_os = "emscripten")]
        {
            self.lua.gc_stop();
            web_debug_log("web runtime: automatic Luau GC stopped");
        }

        Ok(())
    }

    fn poll_http_callbacks(&self) {
        #[cfg(not(target_os = "emscripten"))]
        {
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
    }

    fn poll_media_callbacks(&self) {
        let globals = self.lua.globals();
        let media = match globals.get::<Table>("media") {
            Ok(table) => table,
            Err(_) => return,
        };
        let poll = match media.get::<Function>("_poll") {
            Ok(function) => function,
            Err(_) => return,
        };
        if let Err(error) = protect_lua_call("polling media callbacks", || poll.call::<()>(())) {
            eprintln!(
                "\x1b[31mLua Error:\x1b[0m Failed to poll media callbacks\n{}",
                describe_lua_error(&error)
            );
        }
    }

    fn poll_server_callbacks(&self) {
        #[cfg(not(target_os = "emscripten"))]
        {
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
    }

    fn dispatch_entity_listeners(&self) {
        let (mouse, input) = {
            let platform = lock_platform_state(&self.platform);
            (platform.mouse(), platform.input().clone())
        };
        let camera = self
            .render_state
            .lock()
            .map(|state| state.camera_offset())
            .unwrap_or_default();
        let world_mouse_x = mouse.x - camera.x;
        let world_mouse_y = mouse.y - camera.y;

        let mut pointer_events = Vec::<EntityListenEvent>::new();
        if input.mouse_pressed.contains("left") {
            pointer_events.push(EntityListenEvent::LeftClick);
        }
        if input.mouse_pressed.contains("right") {
            pointer_events.push(EntityListenEvent::RightClick);
        }
        if input.mouse_pressed.contains("middle") {
            pointer_events.push(EntityListenEvent::MiddleClick);
        }
        if input.wheel_y > 0.0 {
            pointer_events.push(EntityListenEvent::ScrollUp);
        }
        if input.wheel_y < 0.0 {
            pointer_events.push(EntityListenEvent::ScrollDown);
        }

        let has_hover_edge_listeners = {
            let listeners = self.entity_listeners.borrow();
            listeners
                .values()
                .any(|listener| listener.connected.get() && listener.event.is_hover_edge())
        };
        if !has_hover_edge_listeners {
            self.hovered_entity_ids.borrow_mut().clear();
            self.entity_hover_initialized.set(false);
        }
        if pointer_events.is_empty() && !has_hover_edge_listeners {
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
                match point_hits_entity(&entity, world_mouse_x, world_mouse_y) {
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

        let hovered_ids: HashSet<usize> = hovered_entities
            .iter()
            .map(|(_, _, entity_id)| *entity_id)
            .collect();
        let mut entered_ids = HashSet::<usize>::new();
        let mut exited_ids = HashSet::<usize>::new();
        if has_hover_edge_listeners {
            let mut previous_hovered = self.hovered_entity_ids.borrow_mut();
            if self.entity_hover_initialized.get() {
                entered_ids.extend(hovered_ids.difference(&previous_hovered).copied());
                exited_ids.extend(previous_hovered.difference(&hovered_ids).copied());
            } else {
                self.entity_hover_initialized.set(true);
            }
            *previous_hovered = hovered_ids;
        }

        let mut event_targets = Vec::<(Table, usize, Vec<EntityListenEvent>)>::new();
        if !exited_ids.is_empty() {
            let entities = self.entities.borrow();
            let mut exited_ids: Vec<usize> = exited_ids.into_iter().collect();
            exited_ids.sort_unstable();
            for entity_id in exited_ids {
                let Some(entity_data) = entities.get(&entity_id) else {
                    continue;
                };
                let Ok(entity) = self.lua.registry_value::<Table>(&entity_data.luau_key) else {
                    continue;
                };
                event_targets.push((entity, entity_id, vec![EntityListenEvent::MouseExited]));
            }
        }
        for (entity, _, entity_id) in hovered_entities {
            let mut events = Vec::new();
            if entered_ids.contains(&entity_id) {
                events.push(EntityListenEvent::MouseEntered);
            }
            events.extend(pointer_events.iter().copied());
            if !events.is_empty() {
                event_targets.push((entity, entity_id, events));
            }
        }

        let mut queue = Vec::<(Table, Function, Table)>::new();
        {
            let listeners = self.entity_listeners.borrow();
            for (entity, entity_id, events) in event_targets {
                for event in events {
                    for listener in listeners.values() {
                        if !listener.connected.get()
                            || listener.entity_id != entity_id
                            || listener.event != event
                        {
                            continue;
                        }

                        let callback = match self.lua.registry_value::<Function>(&listener.callback)
                        {
                            Ok(callback) => callback,
                            Err(error) => {
                                eprintln!(
                                    "\x1b[31mLua Error:\x1b[0m Failed to resolve entity listener callback: {}",
                                    error
                                );
                                continue;
                            }
                        };
                        let (local_x, local_y) = match entity_local_point(
                            &entity,
                            world_mouse_x,
                            world_mouse_y,
                        ) {
                            Ok(point) => point,
                            Err(error) => {
                                eprintln!(
                                    "\x1b[31mLua Error:\x1b[0m Failed to transform entity listener pointer: {}",
                                    error
                                );
                                continue;
                            }
                        };
                        let payload = match create_entity_listener_event(
                            &self.lua,
                            event,
                            mouse.x,
                            mouse.y,
                            local_x,
                            local_y,
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
        let mut body_sync_by_entity_id: HashMap<usize, usize> = HashMap::new();
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
            let (body_x, body_y, entity_rotation, pivot_x, pivot_y) =
                physics_body_pose_from_entity(entity, entity_w, entity_h)?;
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
                .translation(vector![body_x, body_y])
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
                    .enabled_translations(!freeze_x, !freeze_y)
                    .ccd_enabled(!is_static)
                    .additional_solver_iterations(if is_static { 0 } else { 4 });
                if freeze_rotation {
                    builder = builder.lock_rotations();
                }
            }

            let body_handle = bodies.insert(builder.build());
            body_by_entity_id.insert(entity_id, body_handle);
            body_sync_by_entity_id.insert(entity_id, body_sync.len());
            body_sync.push(RapierBodySync {
                entity_id,
                entity: entity.clone(),
                rigidbody: rigidbody.clone(),
                body_handle,
                size_x: entity_w,
                size_y: entity_h,
                is_static,
                last_sync_pose: Cell::new(None),
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
                    if w > 0.0 { w * global_scale } else { entity_w }
                };
                let collider_h = {
                    let h = collider_component.get::<f32>("size_y").unwrap_or(0.0);
                    if h > 0.0 { h * global_scale } else { entity_h }
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
                        offset_x + collider_w * 0.5 - pivot_x,
                        offset_y + collider_h * 0.5 - pivot_y,
                    ]),
                    ColliderShape::Circle => {
                        let radius = (collider_w.min(collider_h) * 0.5).max(0.0001);
                        ColliderBuilder::ball(radius).translation(vector![
                            offset_x + collider_w * 0.5 - pivot_x,
                            offset_y + collider_h * 0.5 - pivot_y,
                        ])
                    }
                    ColliderShape::RightTriangle(corner) => {
                        let (a, b, c) = triangle_local_points(
                            corner, pivot_x, pivot_y, offset_x, offset_y, collider_w, collider_h,
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
            body_sync_by_entity_id,
            entity_by_id,
        });

        Ok(())
    }

    fn simulate_rapier_physics(&mut self, dt: f32, web_trace: Option<usize>) -> mlua::Result<()> {
        let step_dt = dt.clamp(0.0, 0.25);
        if step_dt <= f32::EPSILON {
            if let Some(trace) = web_trace {
                web_debug_log(&format!(
                    "runtime.update {trace}: skipping rapier step because dt={step_dt:.6}"
                ));
            }
            return Ok(());
        }

        let mut physics_entity_count = 0usize;
        let scanned_entity_count = {
            let entities = self.entities.borrow();
            for entity_data in entities.values() {
                let Ok(entity) = self.lua.registry_value::<Table>(&entity_data.luau_key) else {
                    continue;
                };
                if entity
                    .raw_get::<bool>("__neolove_has_physics_components")
                    .unwrap_or(false)
                {
                    physics_entity_count += 1;
                }
            }
            entities.len()
        };
        if let Some(trace) = web_trace {
            web_debug_log(&format!(
                "runtime.update {trace}: rapier marker scan entities={scanned_entity_count} physics_entities={physics_entity_count}"
            ));
        }
        if physics_entity_count == 0 {
            if let Some(trace) = web_trace {
                web_debug_log(&format!(
                    "runtime.update {trace}: rapier no-physics branch world_present={} signature={}",
                    self.physics_world.is_some(),
                    self.physics_signature,
                ));
            }
            self.physics_world = None;
            self.physics_signature = 0;
            if let Some(trace) = web_trace {
                web_debug_log(&format!(
                    "runtime.update {trace}: cleared rapier world because no physics components are registered"
                ));
            }
            return Ok(());
        }

        let mut physics_infos: Vec<EntityPhysicsInfo> = Vec::new();
        let mut has_physics_work = false;
        let mut rigidbody_count = 0usize;
        let mut collider_count = 0usize;
        let mut rope_count = 0usize;
        let mut bolt_count = 0usize;
        {
            let entities = self.entities.borrow();
            physics_infos.try_reserve(entities.len()).map_err(|error| {
                mlua::Error::external(format!(
                    "failed to reserve physics info vector for {} entities: {error}",
                    entities.len()
                ))
            })?;
            for entity_data in entities.values() {
                if let Ok(entity) = self.lua.registry_value::<Table>(&entity_data.luau_key) {
                    if !entity
                        .raw_get::<bool>("__neolove_has_physics_components")
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    let entity_id = entity.get::<usize>("id").unwrap_or(0);
                    let (rigidbody, collider, ropes, bolts, legacy_bolts) =
                        if let Ok(components) = entity.get::<Table>("components") {
                            extract_physics_components(&components)?
                        } else {
                            (None, None, Vec::new(), Vec::new(), Vec::new())
                        };
                    if rigidbody.is_some() {
                        rigidbody_count += 1;
                    }
                    if collider.is_some() {
                        collider_count += 1;
                    }
                    rope_count += ropes.len();
                    bolt_count += bolts.len() + legacy_bolts.len();
                    if rigidbody.is_some()
                        || collider.is_some()
                        || !ropes.is_empty()
                        || !bolts.is_empty()
                        || !legacy_bolts.is_empty()
                    {
                        has_physics_work = true;
                    }
                    physics_infos.push(EntityPhysicsInfo {
                        entity_id,
                        entity,
                        rigidbody,
                        collider,
                        ropes,
                        bolts,
                        legacy_bolts,
                    });
                }
            }
        }

        physics_infos.sort_by_key(|info| info.entity_id);
        if let Some(trace) = web_trace {
            web_debug_log(&format!(
                "runtime.update {trace}: rapier scan results infos={} rigidbodies={} colliders={} ropes={} bolts={} has_work={has_physics_work}",
                physics_infos.len(),
                rigidbody_count,
                collider_count,
                rope_count,
                bolt_count,
            ));
        }

        if !has_physics_work {
            self.physics_world = None;
            self.physics_signature = 0;
            if let Some(trace) = web_trace {
                web_debug_log(&format!(
                    "runtime.update {trace}: skipping rapier because scanned physics work is empty"
                ));
            }
            return Ok(());
        }

        let signature = physics_topology_signature(&physics_infos);
        if self.physics_world.is_none() || signature != self.physics_signature {
            if let Some(trace) = web_trace {
                web_debug_log(&format!(
                    "runtime.update {trace}: rebuilding rapier world signature={} previous_signature={}",
                    signature, self.physics_signature
                ));
            }
            self.rebuild_physics_world(&physics_infos)?;
            self.physics_signature = signature;
        }

        let world = match self.physics_world.as_mut() {
            Some(world) => world,
            None => return Ok(()),
        };
        if let Some(trace) = web_trace {
            web_debug_log(&format!(
                "runtime.update {trace}: rapier world ready bodies={} colliders={} body_sync={} collider_sync={}",
                world.bodies.len(),
                world.colliders.len(),
                world.body_sync.len(),
                world.collider_sync.len(),
            ));
        }

        let mut rope_sync: Vec<RapierRopeSync> = Vec::new();
        let mut bolt_sync: Vec<RapierBoltSync> = Vec::new();
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

            if sync.is_static {
                let (body_x, body_y, entity_rotation, _, _) =
                    physics_body_pose_from_entity(&sync.entity, sync.size_x, sync.size_y)?;
                body.set_translation(vector![body_x, body_y], true);
                body.set_rotation(nalgebra::UnitComplex::new(entity_rotation), true);
            } else if let Some((last_x, last_y, last_rotation)) = sync.last_sync_pose.get() {
                // Dynamic body: if a script assigned the entity's position or
                // rotation since the last step, teleport the physics body to
                // match. Otherwise the simulation would keep integrating from
                // its own internal pose and overwrite the script's value on
                // write-back. Velocities are still applied below.
                const POSE_EPSILON: f32 = 1e-4;
                let entity_x = sync.entity.get::<f32>("x").unwrap_or(last_x);
                let entity_y = sync.entity.get::<f32>("y").unwrap_or(last_y);
                let entity_rotation = sync.entity.get::<f32>("rotation").unwrap_or(last_rotation);
                if (entity_x - last_x).abs() > POSE_EPSILON
                    || (entity_y - last_y).abs() > POSE_EPSILON
                    || (entity_rotation - last_rotation).abs() > POSE_EPSILON
                {
                    let (body_x, body_y, global_rotation, _, _) =
                        physics_body_pose_from_entity(&sync.entity, sync.size_x, sync.size_y)?;
                    body.set_translation(vector![body_x, body_y], true);
                    body.set_rotation(nalgebra::UnitComplex::new(global_rotation), true);
                }
            }

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
                body.enable_ccd(!is_static);
                body.set_additional_solver_iterations(if is_static { 0 } else { 4 });
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

        for info in &physics_infos {
            for (bolt, legacy_mode) in info
                .bolts
                .iter()
                .map(|bolt| (bolt, false))
                .chain(info.legacy_bolts.iter().map(|bolt| (bolt, true)))
            {
                bolt.set("current_force", 0.0)?;
                bolt.set("force", 0.0)?;

                if !bolt.get::<bool>("enabled").unwrap_or(true) {
                    continue;
                }

                let strength = bolt.get::<f32>("strength").unwrap_or(1.0).clamp(0.0, 1.0);
                if strength <= 0.0 {
                    continue;
                }

                let Some(target_entity) = bolt
                    .get::<Option<Table>>("target_entity")
                    .ok()
                    .flatten()
                    .or_else(|| bolt.get::<Option<Table>>("target").ok().flatten())
                else {
                    continue;
                };

                let target_entity_id = target_entity.get::<usize>("id").unwrap_or(0);
                let Some(&owner_body) = world.body_by_entity_id.get(&info.entity_id) else {
                    continue;
                };
                let Some(&target_body) = world.body_by_entity_id.get(&target_entity_id) else {
                    continue;
                };
                if owner_body == target_body {
                    continue;
                }

                let Some(&owner_sync_index) = world.body_sync_by_entity_id.get(&info.entity_id)
                else {
                    continue;
                };
                let Some(&target_sync_index) = world.body_sync_by_entity_id.get(&target_entity_id)
                else {
                    continue;
                };
                let Some(owner_sync) = world.body_sync.get(owner_sync_index) else {
                    continue;
                };
                let Some(target_sync) = world.body_sync.get(target_sync_index) else {
                    continue;
                };

                let (owner_pivot_x, owner_pivot_y) = physics_pivot_local_from_center(
                    &owner_sync.entity,
                    owner_sync.size_x,
                    owner_sync.size_y,
                );
                let (target_pivot_x, target_pivot_y) = physics_pivot_local_from_center(
                    &target_sync.entity,
                    target_sync.size_x,
                    target_sync.size_y,
                );
                let x = bolt.get::<f32>("x").unwrap_or(0.0);
                let y = bolt.get::<f32>("y").unwrap_or(0.0);
                let offset_x_alias = bolt.get::<f32>("offset_x").unwrap_or(0.0);
                let offset_y_alias = bolt.get::<f32>("offset_y").unwrap_or(0.0);
                let offset_x = if x != 0.0 || offset_x_alias == 0.0 {
                    x
                } else {
                    offset_x_alias
                };
                let offset_y = if y != 0.0 || offset_y_alias == 0.0 {
                    y
                } else {
                    offset_y_alias
                };
                let contacts_enabled = bolt.get::<bool>("contacts_enabled").unwrap_or(true);

                let builder = if legacy_mode {
                    if strength >= 0.999 {
                        GenericJointBuilder::new(JointAxesMask::LIN_AXES)
                            .local_anchor1(point![
                                target_pivot_x + offset_x,
                                target_pivot_y + offset_y
                            ])
                            .local_anchor2(point![owner_pivot_x, owner_pivot_y])
                            .contacts_enabled(contacts_enabled)
                    } else {
                        let stiffness = 80.0 + strength * strength * 5000.0;
                        let damping = 2.0 * stiffness.sqrt();
                        let max_force = 20000.0 * strength;
                        GenericJointBuilder::new(JointAxesMask::empty())
                            .local_anchor1(point![
                                target_pivot_x + offset_x,
                                target_pivot_y + offset_y
                            ])
                            .local_anchor2(point![owner_pivot_x, owner_pivot_y])
                            .contacts_enabled(contacts_enabled)
                            .motor_position(JointAxis::LinX, 0.0, stiffness, damping)
                            .motor_position(JointAxis::LinY, 0.0, stiffness, damping)
                            .motor_max_force(JointAxis::LinX, max_force)
                            .motor_max_force(JointAxis::LinY, max_force)
                    }
                } else if strength >= 0.999 {
                    GenericJointBuilder::new(JointAxesMask::LIN_AXES | JointAxesMask::ANG_AXES)
                        .local_anchor1(point![target_pivot_x + offset_x, target_pivot_y + offset_y])
                        .local_anchor2(point![owner_pivot_x, owner_pivot_y])
                        .contacts_enabled(contacts_enabled)
                } else {
                    let angular_stiffness = strength * strength * 2400.0;
                    let angular_damping = 2.0 * angular_stiffness.sqrt();
                    let max_torque = 9000.0 * strength;
                    GenericJointBuilder::new(JointAxesMask::LIN_AXES)
                        .local_anchor1(point![target_pivot_x + offset_x, target_pivot_y + offset_y])
                        .local_anchor2(point![owner_pivot_x, owner_pivot_y])
                        .contacts_enabled(contacts_enabled)
                        .motor_position(JointAxis::AngX, 0.0, angular_stiffness, angular_damping)
                        .motor_max_force(JointAxis::AngX, max_torque)
                };

                let joint_handle =
                    world
                        .impulse_joints
                        .insert(target_body, owner_body, builder, true);
                bolt_sync.push(RapierBoltSync {
                    bolt: bolt.clone(),
                    joint_handle,
                });
            }
        }

        let mut pipeline = PhysicsPipeline::new();
        let mut integration_parameters = IntegrationParameters::default();
        integration_parameters.dt = step_dt;
        integration_parameters.length_unit = 100.0;
        integration_parameters.num_solver_iterations = NonZeroUsize::new(8).expect("8 is non-zero");
        integration_parameters.num_internal_pgs_iterations = 2;
        integration_parameters.num_internal_stabilization_iterations = 4;
        integration_parameters.num_additional_friction_iterations = 2;
        integration_parameters.max_ccd_substeps = 4;
        if let Some(trace) = web_trace {
            web_debug_log(&format!(
                "runtime.update {trace}: before rapier pipeline step dt={step_dt:.6} bodies={} colliders={} rope_joints={} bolt_joints={} total_joints={}",
                world.bodies.len(),
                world.colliders.len(),
                rope_sync.len(),
                bolt_sync.len(),
                world.impulse_joints.len(),
            ));
        }

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
        if let Some(trace) = web_trace {
            web_debug_log(&format!(
                "runtime.update {trace}: after rapier pipeline step contacts={} intersections={}",
                world.narrow_phase.contact_pairs().count(),
                world.narrow_phase.intersection_pairs().count(),
            ));
        }

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

            let body_tx = body.translation().x;
            let body_ty = body.translation().y;
            let (mut x, mut y) = physics_entity_position_from_body(&sync.entity, body_tx, body_ty)?;
            // Rapier tracks rotation in world space (parent rotations summed in via
            // get_global_transform), but the entity's `rotation` field is local to its
            // parent. Subtract the parent's global rotation so a collider on a child of a
            // rotated object doesn't inherit the parent's rotation every frame.
            let mut rotation = body.rotation().angle();
            if let Some(parent) = sync.entity.get::<Option<Table>>("parent")? {
                rotation -= get_global_rotation(&parent)?;
            }
            let mut velocity_x = body.linvel().x;
            let mut velocity_y = body.linvel().y;
            let mut angular_velocity = body.angvel();
            let mut grounded = grounded_entities.contains(&sync.entity_id);
            let mut position_clamped = false;

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
                        position_clamped = true;
                        if velocity_x < 0.0 {
                            velocity_x = -velocity_x * restitution;
                        }
                    } else if x + sync.size_x > window_w {
                        x = (window_w - sync.size_x).max(0.0);
                        position_clamped = true;
                        if velocity_x > 0.0 {
                            velocity_x = -velocity_x * restitution;
                        }
                    }

                    if y < 0.0 {
                        y = 0.0;
                        position_clamped = true;
                        if velocity_y < 0.0 {
                            velocity_y = -velocity_y * restitution;
                        }
                    } else if y + sync.size_y > window_h {
                        y = (window_h - sync.size_y).max(0.0);
                        position_clamped = true;
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

            if let Some(body) = world.bodies.get_mut(sync.body_handle) {
                body.set_linvel(vector![velocity_x, velocity_y], true);
                body.set_angvel(angular_velocity, true);
                if position_clamped {
                    let (clamped_body_x, clamped_body_y) =
                        physics_body_position_from_entity_position(&sync.entity, x, y)?;
                    body.set_translation(vector![clamped_body_x, clamped_body_y], true);
                }
            }

            sync.entity.set("x", x)?;
            sync.entity.set("y", y)?;
            sync.entity.set("rotation", rotation)?;
            // Remember the pose we just wrote so next step can detect a
            // script-driven position/rotation change and teleport the body.
            sync.last_sync_pose.set(Some((x, y, rotation)));
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

        for bolt in bolt_sync {
            let mut force = 0.0f32;
            if let Some(joint) = world.impulse_joints.get(bolt.joint_handle) {
                force = joint.impulses.norm() / step_dt.max(0.0001);
            }
            bolt.bolt.set("current_force", force)?;
            bolt.bolt.set("force", force)?;
        }

        Ok(())
    }
    pub fn update(&mut self, dt: f32) -> Result<(), String> {
        let update_index = next_web_update_index();
        let web_trace = web_update_trace(update_index);
        let panic_stage = Cell::new("initializing runtime.update");

        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.update_inner(dt, web_trace, &panic_stage)
        }))
        .map_err(|payload| {
            format!(
                "runtime.update {update_index} panicked during {}: {}",
                panic_stage.get(),
                crate::lua_error::describe_panic(payload.as_ref())
            )
        })?
    }

    fn update_inner(
        &mut self,
        dt: f32,
        web_trace: Option<usize>,
        panic_stage: &Cell<&'static str>,
    ) -> Result<(), String> {
        panic_stage.set("runtime.update startup");
        if let Some(trace) = web_trace {
            web_debug_log(&format!("runtime.update {trace}: begin dt={dt:.6}"));
            web_debug_log(&format!(
                "runtime.update {trace}: exit_requested at begin = {}",
                *self.exit_requested.borrow()
            ));
        }

        panic_stage.set("begin_ui_frame");
        crate::core::begin_ui_frame();
        if let Some(trace) = web_trace {
            web_debug_log(&format!("runtime.update {trace}: after begin_ui_frame"));
        }

        panic_stage.set("set_mouse_table");
        if let Some(trace) = web_trace {
            web_debug_log(&format!("runtime.update {trace}: before set_mouse_table"));
        }
        self.set_mouse_table()
            .map_err(|error| format!("failed to sync mouse state into Lua: {error}"))?;
        if let Some(trace) = web_trace {
            web_debug_log(&format!("runtime.update {trace}: after set_mouse_table"));
        }

        panic_stage.set("set_window_table");
        if let Some(trace) = web_trace {
            web_debug_log(&format!("runtime.update {trace}: before set_window_table"));
        }
        self.set_window_table()
            .map_err(|error| format!("failed to sync window state into Lua: {error}"))?;
        if let Some(trace) = web_trace {
            web_debug_log(&format!("runtime.update {trace}: after set_window_table"));
        }

        panic_stage.set("poll_http_callbacks");
        if let Some(trace) = web_trace {
            web_debug_log(&format!(
                "runtime.update {trace}: before poll_http_callbacks"
            ));
        }
        self.poll_http_callbacks();
        if let Some(trace) = web_trace {
            web_debug_log(&format!(
                "runtime.update {trace}: after poll_http_callbacks"
            ));
        }

        panic_stage.set("poll_media_callbacks");
        self.poll_media_callbacks();

        panic_stage.set("poll_server_callbacks");
        if let Some(trace) = web_trace {
            web_debug_log(&format!(
                "runtime.update {trace}: before poll_server_callbacks"
            ));
        }
        self.poll_server_callbacks();
        if let Some(trace) = web_trace {
            web_debug_log(&format!(
                "runtime.update {trace}: after poll_server_callbacks"
            ));
        }

        panic_stage.set("poll_async_tasks");
        self.poll_async_tasks();

        panic_stage.set("component awake callbacks");
        self.drain_pending_component_awakes()
            .map_err(|error| format!("component awake failed:\n{}", describe_lua_error(&error)))?;

        panic_stage.set("update_tweening");
        if let Some(trace) = web_trace {
            web_debug_log(&format!("runtime.update {trace}: before update_tweening"));
        }
        if let Ok(tweening) = self.lua.globals().get::<Table>("tweening")
            && let Ok(update) = tweening.get::<Function>("_update")
        {
            update.call::<()>(dt as f64).map_err(|error| {
                format!("failed to update tweening:\n{}", describe_lua_error(&error))
            })?;
        }
        if let Ok(animation) = self.lua.globals().get::<Table>("animation")
            && let Ok(update) = animation.get::<Function>("_update")
        {
            update.call::<()>(dt as f64).map_err(|error| {
                format!(
                    "failed to update animation:\n{}",
                    describe_lua_error(&error)
                )
            })?;
        }
        if let Some(trace) = web_trace {
            web_debug_log(&format!("runtime.update {trace}: after update_tweening"));
        }

        panic_stage.set("dispatch_entity_listeners");
        if let Some(trace) = web_trace {
            web_debug_log(&format!(
                "runtime.update {trace}: before dispatch_entity_listeners"
            ));
        }
        self.dispatch_entity_listeners();
        if let Some(trace) = web_trace {
            web_debug_log(&format!(
                "runtime.update {trace}: after dispatch_entity_listeners"
            ));
        }

        panic_stage.set("resolve_app_clear_color");
        if let Some(trace) = web_trace {
            web_debug_log(&format!(
                "runtime.update {trace}: before resolve clear color"
            ));
        }
        let clear = self.resolve_app_clear_color().map_err(|error| {
            format!(
                "failed to resolve app background color:\n{}",
                describe_lua_error(&error)
            )
        })?;
        if let Some(trace) = web_trace {
            web_debug_log(&format!(
                "runtime.update {trace}: resolved clear color rgba({}, {}, {}, {})",
                clear.r, clear.g, clear.b, clear.a
            ));
        }
        let antialiasing = self.resolve_app_antialiasing().map_err(|error| {
            format!(
                "failed to resolve app anti-aliasing mode:\n{}",
                describe_lua_error(&error)
            )
        })?;
        {
            let mut platform = lock_platform_state(&self.platform);
            platform.set_clear_color(clear);
            platform.set_antialiasing(antialiasing);
        }
        panic_stage.set("systems loop");
        if let Some(trace) = web_trace {
            web_debug_log(&format!("runtime.update {trace}: after set_clear_color"));
        }

        {
            // Resolve a snapshot first. A system is allowed to register another
            // system from update(); holding the RefCell borrow across the Lua
            // callback made that legal operation panic inside mlua's native
            // callback trampoline, which cannot unwind safely.
            let systems = {
                let keys = self.systems.try_borrow().map_err(|_| {
                    "cannot update systems while the system registry is being changed".to_string()
                })?;
                keys.iter()
                    .map(|key| self.lua.registry_value::<Table>(key))
                    .collect::<Vec<_>>()
            };
            if let Some(trace) = web_trace {
                web_debug_log(&format!(
                    "runtime.update {trace}: before systems loop count={}",
                    systems.len()
                ));
            }
            for (system_index, system) in systems.into_iter().enumerate() {
                let system: Table = match system {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("\x1b[31mLua Error:\x1b[0m Failed to get system: {}", e);
                        continue;
                    }
                };
                if let Some(trace) = web_trace {
                    web_debug_log(&format!(
                        "runtime.update {trace}: system {} loaded",
                        system_index + 1
                    ));
                }
                if let Ok(Value::Function(update)) = system.get::<Value>("update") {
                    panic_stage.set("system update callback");
                    if let Some(trace) = web_trace {
                        web_debug_log(&format!(
                            "runtime.update {trace}: before system {} update",
                            system_index + 1
                        ));
                    }
                    protect_lua_call("running system update callback", || {
                        update.call::<()>((system.clone(), dt))
                    })
                    .map_err(|error| {
                        format!("system update failed:\n{}", describe_lua_error(&error))
                    })?;
                    if let Some(trace) = web_trace {
                        web_debug_log(&format!(
                            "runtime.update {trace}: after system {} update",
                            system_index + 1
                        ));
                    }
                } else if let Some(trace) = web_trace {
                    web_debug_log(&format!(
                        "runtime.update {trace}: system {} has no update function",
                        system_index + 1
                    ));
                }
            }
        }
        if let Some(trace) = web_trace {
            web_debug_log(&format!("runtime.update {trace}: after systems loop"));
            web_debug_log(&format!(
                "runtime.update {trace}: exit_requested after systems = {}",
                *self.exit_requested.borrow()
            ));
        }

        panic_stage.set("component awake callbacks");
        self.drain_pending_component_awakes()
            .map_err(|error| format!("component awake failed:\n{}", describe_lua_error(&error)))?;

        let mut ordered_entities: Vec<(usize, f64, usize)> = Vec::new();

        {
            panic_stage.set("ordered entity collection");
            let entities = self.entities.borrow();
            if let Some(trace) = web_trace {
                web_debug_log(&format!(
                    "runtime.update {trace}: collecting ordered entities count={}",
                    entities.len()
                ));
            }
            ordered_entities
                .try_reserve(entities.len())
                .map_err(|error| {
                    format!(
                        "failed to reserve ordered entity vector for {} entities: {error}",
                        entities.len()
                    )
                })?;
            for entity in entities.values() {
                if let Ok(table) = self.lua.registry_value::<Table>(&entity.luau_key) {
                    let z = table.get::<f64>("z").unwrap_or(0.0);
                    ordered_entities.push((entity.id, z, entity.id));
                }
            }
        }
        if let Some(trace) = web_trace {
            web_debug_log(&format!(
                "runtime.update {trace}: collected ordered entities count={}",
                ordered_entities.len()
            ));
        }

        panic_stage.set("ordered entity sort");
        ordered_entities.sort_by(|a, b| compare_entity_order(a.1, a.2, b.1, b.2));
        if let Some(trace) = web_trace {
            web_debug_log(&format!("runtime.update {trace}: after entity sort"));
        }

        let mut rendering_component_count = 0usize;
        let mut rendering_components: Vec<(usize, usize, Table, Table, Function)> = Vec::new();
        panic_stage.set("rendering component reservation");
        rendering_components
            .try_reserve(ordered_entities.len())
            .map_err(|error| {
                format!(
                    "failed to reserve rendering component vector for {} ordered entities: {error}",
                    ordered_entities.len()
                )
            })?;
        panic_stage.set("component scan");
        for (entity_id, _, _) in ordered_entities.iter() {
            let ent = {
                let entities = self.entities.borrow();
                let Some(entity_data) = entities.get(entity_id) else {
                    continue;
                };
                match self.lua.registry_value::<Table>(&entity_data.luau_key) {
                    Ok(table) => table,
                    Err(e) => {
                        eprintln!(
                            "\x1b[31mLua Error:\x1b[0m Failed to get entity table: {}",
                            e
                        );
                        continue;
                    }
                }
            };
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

            let mut component_index = 0usize;
            for component in components.sequence_values::<Table>() {
                if self.has_pending_component_awakes() {
                    panic_stage.set("component awake callbacks");
                    self.drain_pending_component_awakes().map_err(|error| {
                        format!("component awake failed:\n{}", describe_lua_error(&error))
                    })?;
                }
                component_index += 1;
                let component: Table = match component {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!(
                            "\x1b[31mLua Error:\x1b[0m Failed to iterate components: {}",
                            e
                        );
                        continue;
                    }
                };
                let component_name = describe_component_name(&component, Some(&ent));
                let update: Function = match component.get::<Value>("update") {
                    Ok(Value::Function(update)) => update,
                    Ok(other) => {
                        let location = describe_component_source(&component)
                            .map(|loc| format!(" (defined in {loc})"))
                            .unwrap_or_default();
                        if other.is_nil() {
                            eprintln!(
                                "\x1b[31mLua Error:\x1b[0m {component_name} is missing an 'update' function{location}"
                            );
                        } else {
                            eprintln!(
                                "\x1b[31mLua Error:\x1b[0m {component_name} has an 'update' field that is a {}, not a function{location}",
                                other.type_name()
                            );
                        }
                        continue;
                    }
                    Err(e) => {
                        let location = describe_component_source(&component)
                            .map(|loc| format!(" (defined in {loc})"))
                            .unwrap_or_default();
                        eprintln!(
                            "\x1b[31mLua Error:\x1b[0m Failed to read 'update' on {component_name}{location}: {}",
                            e
                        );
                        continue;
                    }
                };

                let is_rendering = component.get::<bool>("NEOLOVE_RENDERING").unwrap_or(false);
                if !is_rendering {
                    panic_stage.set("component update callback");
                    protect_lua_call(
                        &format!("running component update callback ({component_name})"),
                        || update.call::<()>((ent.clone(), component, dt)),
                    )
                    .map_err(|error| {
                        format!("component update failed:\n{}", describe_lua_error(&error))
                    })?;
                } else {
                    rendering_component_count += 1;
                    rendering_components.push((
                        *entity_id,
                        component_index,
                        ent.clone(),
                        component,
                        update,
                    ));
                }
            }
        }
        if let Some(trace) = web_trace {
            web_debug_log(&format!(
                "runtime.update {trace}: after component scan rendering_count={rendering_component_count}"
            ));
        }

        if self.has_pending_component_awakes() {
            panic_stage.set("component awake callbacks");
            self.drain_pending_component_awakes().map_err(|error| {
                format!("component awake failed:\n{}", describe_lua_error(&error))
            })?;
        }

        panic_stage.set("simulate_rapier_physics");
        if let Some(trace) = web_trace {
            web_debug_log(&format!(
                "runtime.update {trace}: before simulate_rapier_physics"
            ));
        }
        self.simulate_rapier_physics(dt, web_trace)
            .map_err(|error| {
                format!(
                    "Rapier2D physics update failed:\n{}",
                    describe_lua_error(&error)
                )
            })?;
        if let Some(trace) = web_trace {
            web_debug_log(&format!(
                "runtime.update {trace}: after simulate_rapier_physics"
            ));
        }

        // Resolve cameras after physics has written final entity transforms but
        // before any drawable component performs culling, input hit-testing, or
        // queues commands. Commands themselves remain in world space until the
        // renderer drains the frame, so one camera offset applies atomically.
        let (camera_components, rendering_components): (Vec<_>, Vec<_>) = rendering_components
            .into_iter()
            .partition(|(_, _, _, component, _)| {
                component.get::<bool>("NEOLOVE_CAMERA").unwrap_or(false)
            });
        {
            let mut state = self
                .render_state
                .lock()
                .map_err(|_| "render state lock poisoned while beginning camera frame")?;
            state.begin_camera_frame();
        }
        panic_stage.set("camera pre-pass");
        for (_entity_id, _component_index, ent, component, update) in camera_components {
            let component_name = describe_component_name(&component, Some(&ent));
            protect_lua_call(
                &format!("running camera component update callback ({component_name})"),
                || update.call::<()>((ent.clone(), component, dt)),
            )
            .map_err(|error| {
                format!(
                    "camera component update failed:\n{}",
                    describe_lua_error(&error)
                )
            })?;
        }
        self.render_state
            .lock()
            .map_err(|_| "render state lock poisoned while finishing camera frame")?
            .finish_camera_frame();

        panic_stage.set("rendering pass");
        if let Some(trace) = web_trace {
            web_debug_log(&format!("runtime.update {trace}: before rendering pass"));
        }
        for (entity_id, component_index, ent, component, update) in rendering_components {
            let component_name = describe_component_name(&component, Some(&ent));

            panic_stage.set("rendering component update callback");
            if let Some(trace) = web_trace {
                web_debug_log(&format!(
                    "runtime.update {trace}: before rendering component entity={} index={} name={component_name}",
                    entity_id, component_index
                ));
            }
            protect_lua_call(
                &format!("running rendering component update callback ({component_name})"),
                || update.call::<()>((ent.clone(), component, dt)),
            )
            .map_err(|error| {
                format!(
                    "rendering component update failed:\n{}",
                    describe_lua_error(&error)
                )
            })?;
            if let Some(trace) = web_trace {
                web_debug_log(&format!(
                    "runtime.update {trace}: after rendering component entity={} index={} name={component_name}",
                    entity_id, component_index
                ));
            }
        }
        if let Some(trace) = web_trace {
            web_debug_log(&format!("runtime.update {trace}: after rendering pass"));
        }
        panic_stage.set("runtime.update completed");
        if let Some(trace) = web_trace {
            web_debug_log(&format!(
                "runtime.update {trace}: end exit_requested={}",
                *self.exit_requested.borrow()
            ));
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
    fn look_at_uses_world_space_radians() {
        assert_close(look_at_rotation(10.0, 20.0, 30.0, 20.0), 0.0);
        assert_close(
            look_at_rotation(10.0, 20.0, 10.0, 30.0),
            std::f32::consts::FRAC_PI_2,
        );
        assert_close(
            look_at_rotation(10.0, 20.0, 0.0, 20.0).abs(),
            std::f32::consts::PI,
        );
        assert_close(
            look_at_rotation(10.0, 20.0, 10.0, 10.0),
            -std::f32::consts::FRAC_PI_2,
        );
    }

    #[test]
    fn camera_component_centers_scene_switches_and_falls_back_to_origin() -> mlua::Result<()> {
        let (mut runtime, root) = start_test_runtime("camera_component")?;
        runtime
            .lua
            .load(
                r#"
                cameraEntity = ecs.newEntity("Camera A", nil, 100, 50)
                cameraA = cameraEntity:AddComponent(core.Camera)

                target = ecs.newEntity("Target", nil, 100, 50)
                target.size_x = 20
                target.size_y = 10
                target:AddComponent(core.Rect2D)
                "#,
            )
            .exec()?;

        let rendered_rect_position = |runtime: &Runtime| -> mlua::Result<(f32, f32)> {
            let commands = crate::renderer::drain_commands(&runtime.render_state())
                .map_err(mlua::Error::external)?;
            commands
                .into_iter()
                .find_map(|command| match command {
                    crate::renderer::DrawCommand::Rect { x, y, .. } => Some((x, y)),
                    _ => None,
                })
                .ok_or_else(|| mlua::Error::external("camera test rect was not rendered"))
        };

        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        let (x, y) = rendered_rect_position(&runtime)?;
        assert_close(x, 320.0);
        assert_close(y, 240.0);

        runtime
            .lua
            .load(
                r#"
                cameraEntityB = ecs.newEntity("Camera B", nil, 0, 0)
                cameraB = cameraEntityB:AddComponent(core.Camera)
                assert(cameraB:SetActive() == true)
                assert(cameraB:IsActive() == true)
                "#,
            )
            .exec()?;
        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        let (x, y) = rendered_rect_position(&runtime)?;
        assert_close(x, 420.0);
        assert_close(y, 290.0);

        runtime
            .lua
            .load("cameraA.enabled = false; cameraB.enabled = false")
            .exec()?;
        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        let (x, y) = rendered_rect_position(&runtime)?;
        assert_close(x, 100.0);
        assert_close(y, 50.0);

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn rect2d_middle_rotation_pivot_keeps_rect_centered_on_pivot() -> mlua::Result<()> {
        let (mut runtime, root) = start_test_runtime("rect2d_middle_pivot")?;
        runtime
            .lua
            .load(
                r#"
                entity = ecs.newEntity("Rect", nil, 100, 50)
                entity.size_x = 20
                entity.size_y = 10
                entity.rotation_pivot = "middle"
                entity:AddComponent(core.Rect2D)
                "#,
            )
            .exec()?;

        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        let command = crate::renderer::drain_commands(&runtime.render_state())
            .map_err(mlua::Error::external)?
            .into_iter()
            .find_map(|command| match command {
                crate::renderer::DrawCommand::Rect {
                    x, y, w, h, offset, ..
                } => Some((x, y, w, h, offset)),
                _ => None,
            })
            .ok_or_else(|| mlua::Error::external("rect was not rendered"))?;

        let (x, y, w, h, offset) = command;
        // The unrotated top-left corner must stay at the entity origin so the
        // rect is not shifted by half its size, and the pivot fraction must land
        // exactly on the entity center.
        assert_close(x, 100.0);
        assert_close(y, 50.0);
        assert_close(x + w * offset.x, 110.0);
        assert_close(y + h * offset.y, 55.0);

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn text_input_and_sprite_sheet_are_runtime_components() -> mlua::Result<()> {
        let (mut runtime, root) = start_test_runtime("text_input_sprite_sheet")?;
        runtime
            .lua
            .load(
                r#"
                local inputEntity = ecs.newEntity("Input", nil, 0, 0)
                inputEntity.size_x = 180
                inputEntity.size_y = 40
                testInput = ecs.addComponent(inputEntity, core.TextInput)
                assert(testInput.placeholder == "Type here")
                assert(testInput.enabled == true and testInput.locked == false)
                testInput:setBold(0, 1)
                assert(#testInput.__rich_text_ranges == 1)
                testInput.locked = true
                testInput:focus()
                assert(testInput.focused == false)
                testInput.locked = false

                local spriteEntity = ecs.newEntity("Sheet", nil, 0, 0)
                testSheet = ecs.addComponent(spriteEntity, core.SpriteSheet2D)
                assert(testSheet.frame_width == 32 and testSheet.frame_height == 32)
                testSheet:setFrame(3)
                assert(testSheet.frame == 3)
                testSheet:stop()
                assert(testSheet.frame == 0 and testSheet.playing == false)
                "#,
            )
            .exec()?;
        runtime.set_platform_mouse_state(20.0, 20.0);
        runtime
            .platform
            .lock()
            .expect("platform mutex should not be poisoned during test")
            .input_mut()
            .mouse_pressed
            .insert("left".to_string());
        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        {
            let mut platform = runtime
                .platform
                .lock()
                .expect("platform mutex should not be poisoned during test");
            platform.begin_frame();
            platform.input_mut().char_pressed = Some("A".to_string());
        }
        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        let input: Table = runtime.lua.globals().get("testInput")?;
        assert_eq!(input.get::<String>("text")?, "A");
        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn ui_widgets_register_with_vscode_dark_defaults() -> mlua::Result<()> {
        let (runtime, root) = start_test_runtime("ui_widgets")?;
        runtime
            .lua
            .load(
                r#"
                local function rgba(c)
                    return { c.r, c.g, c.b, c.a }
                end
                local function same(a, b)
                    return a[1] == b[1] and a[2] == b[2] and a[3] == b[3] and a[4] == b[4]
                end

                local panelEntity = ecs.newEntity("Panel", nil, 0, 0)
                local panel = ecs.addComponent(panelEntity, core.Panel)
                assert(panel.__neolove_component == "Panel")
                assert(same(rgba(panel.background_color), {37, 37, 38, 255}))
                -- Frame is a backwards-compatible alias of Panel.
                local frame = ecs.addComponent(ecs.newEntity("Frame", nil, 0, 0), core.Frame)
                assert(frame.__neolove_component == "Panel")

                local button = ecs.addComponent(ecs.newEntity("Button", nil, 0, 0), core.Button)
                assert(button.__neolove_component == "Button")
                assert(same(rgba(button.background_color), {14, 99, 156, 255}))
                assert(same(rgba(button.hover_background_color), {17, 119, 187, 255}))

                local dropdown = ecs.addComponent(ecs.newEntity("Dropdown", nil, 0, 0), core.Dropdown)
                assert(dropdown.__neolove_component == "Dropdown")
                assert(same(rgba(dropdown.item_selected_background_color), {9, 71, 113, 255}))

                testSlider = ecs.addComponent(ecs.newEntity("Slider", nil, 0, 0), core.Slider)
                assert(testSlider.__neolove_component == "Slider")
                assert(testSlider.min == 0 and testSlider.max == 100 and testSlider.value == 0)
                assert(same(rgba(testSlider.fill_color), {0, 122, 204, 255}))
                testSlider:setValue(25)
                assert(testSlider.value == 25 and math.abs(testSlider.fraction - 0.25) < 0.001)
                "#,
            )
            .exec()?;
        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn slider_drag_updates_value_and_fires_callback() -> mlua::Result<()> {
        let (mut runtime, root) = start_test_runtime("slider_drag")?;
        runtime
            .lua
            .load(
                r#"
                local entity = ecs.newEntity("Slider", nil, 100, 100)
                entity.size_x = 200
                entity.size_y = 24
                testSlider = ecs.addComponent(entity, core.Slider)
                testSlider.min = 0
                testSlider.max = 100
                ChangedValue = nil
                function testSlider.onChanged(_entity, _self, value)
                    ChangedValue = value
                end
                "#,
            )
            .exec()?;
        // Press near the right end of the track and hold to drag the thumb there.
        runtime.set_platform_mouse_state(292.0, 112.0);
        {
            let mut platform = runtime
                .platform
                .lock()
                .expect("platform mutex should not be poisoned during test");
            platform
                .input_mut()
                .mouse_pressed
                .insert("left".to_string());
            platform.input_mut().mouse_down.insert("left".to_string());
        }
        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        let slider: Table = runtime.lua.globals().get("testSlider")?;
        let value = slider.get::<f32>("value")?;
        assert!(
            value > 80.0,
            "slider value should track the pointer, was {value}"
        );
        let changed: Option<f32> = runtime.lua.globals().get("ChangedValue")?;
        assert!(
            changed.is_some(),
            "onChanged should fire when the value moves"
        );
        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn behaviour_awake_is_deferred_until_first_frame() -> mlua::Result<()> {
        let (mut runtime, root) = start_test_runtime("deferred_awake")?;
        runtime
            .lua
            .load(
                r#"
                local entity = ecs.newEntity("thing", ecs.root)
                local Behaviour = { value = 0 }

                function Behaviour.awake(_entity, self)
                    AwakeCount = (AwakeCount or 0) + 1
                    AwakeValue = self.value
                    self.awoken = true
                end

                function Behaviour.update(_entity, self, _dt)
                    UpdateCount = (UpdateCount or 0) + 1
                    if self.awoken then
                        UpdateSawAwake = (UpdateSawAwake or 0) + 1
                    end
                end

                local component = entity:AddComponent(Behaviour)
                component.value = 42
                AwakeImmediately = AwakeCount
                "#,
            )
            .exec()?;

        let globals = runtime.lua.globals();
        assert!(globals.get::<Option<i64>>("AwakeImmediately")?.is_none());
        assert!(globals.get::<Option<i64>>("AwakeCount")?.is_none());

        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;

        let globals = runtime.lua.globals();
        assert_eq!(globals.get::<i64>("AwakeCount")?, 1);
        assert_eq!(globals.get::<i64>("AwakeValue")?, 42);
        assert_eq!(globals.get::<i64>("UpdateCount")?, 1);
        assert_eq!(globals.get::<i64>("UpdateSawAwake")?, 1);

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn behaviour_created_by_system_awakes_before_first_component_update() -> mlua::Result<()> {
        let (mut runtime, root) = start_test_runtime("system_created_awake")?;
        runtime
            .lua
            .load(
                r#"
                local entity = ecs.newEntity("thing", ecs.root)
                local Behaviour = { value = 0 }

                function Behaviour.awake(_entity, self)
                    AwakeFromSystem = (AwakeFromSystem or 0) + 1
                    AwakeFromSystemValue = self.value
                    self.awoken = true
                end

                function Behaviour.update(_entity, self, _dt)
                    BehaviourUpdateCount = (BehaviourUpdateCount or 0) + 1
                    if self.awoken then
                        BehaviourUpdateSawAwake = (BehaviourUpdateSawAwake or 0) + 1
                    end
                end

                local system = { added = false }
                function system.update(self, _dt)
                    if self.added then
                        return
                    end
                    self.added = true
                    local component = entity:AddComponent(Behaviour)
                    component.value = 7
                    AwakeDuringSystemAdd = AwakeFromSystem
                end

                ecs.addSystem(system)
                "#,
            )
            .exec()?;

        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;

        let globals = runtime.lua.globals();
        assert!(
            globals
                .get::<Option<i64>>("AwakeDuringSystemAdd")?
                .is_none()
        );
        assert_eq!(globals.get::<i64>("AwakeFromSystem")?, 1);
        assert_eq!(globals.get::<i64>("AwakeFromSystemValue")?, 7);
        assert_eq!(globals.get::<i64>("BehaviourUpdateCount")?, 1);
        assert_eq!(globals.get::<i64>("BehaviourUpdateSawAwake")?, 1);

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn async_tasks_resume_once_per_update_and_return_results() -> mlua::Result<()> {
        let (mut runtime, root) = start_test_runtime("async_resume")?;
        runtime
            .lua
            .load(
                r#"
                asyncSteps = 0
                asyncTask = async(function()
                    asyncSteps += 1
                    async.yield()
                    asyncSteps += 1
                    return 42, "complete"
                end)
                "#,
            )
            .exec()?;

        assert_eq!(runtime.lua.globals().get::<i64>("asyncSteps")?, 0);
        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        assert_eq!(runtime.lua.globals().get::<i64>("asyncSteps")?, 1);
        runtime
            .lua
            .load("assert(not asyncTask:isDone()); assert(asyncTask:getStatus() == 'suspended')")
            .exec()?;

        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        runtime
            .lua
            .load(
                r#"
                assert(asyncSteps == 2)
                assert(asyncTask:isDone())
                assert(asyncTask:getStatus() == "completed")
                local numberResult, textResult = asyncTask:getResult()
                assert(numberResult == 42)
                assert(textResult == "complete")
                "#,
            )
            .exec()?;

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn async_tasks_can_be_cancelled_and_report_errors() -> mlua::Result<()> {
        let (mut runtime, root) = start_test_runtime("async_cancel_error")?;
        runtime
            .lua
            .load(
                r#"
                cancelledTask = async(function()
                    error("cancelled task should never run")
                end)
                assert(cancelledTask:cancel())

                failedTask = async(function()
                    error("expected async failure")
                end)
                "#,
            )
            .exec()?;

        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        runtime
            .lua
            .load(
                r#"
                assert(cancelledTask:isDone())
                assert(cancelledTask.cancelled)
                assert(cancelledTask:getStatus() == "cancelled")
                assert(failedTask:isDone())
                assert(failedTask:getStatus() == "error")
                assert(string.find(failedTask:getError(), "expected async failure", 1, true))
                "#,
            )
            .exec()?;

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
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
        // Inside the rendered 90-degree rectangle but outside its original
        // axis-aligned bounds.
        assert!(point_hits_entity(&entity, 50.0, 70.0)?);
        // Inside the original bounds but outside the rendered rectangle.
        assert!(!point_hits_entity(&entity, 10.0, 25.0)?);
        assert!(!point_hits_entity(&entity, 5.0, 5.0)?);

        let (local_x, local_y) = entity_local_point(&entity, 50.0, 70.0)?;
        assert_close(local_x, 95.0);
        assert_close(local_y, 25.0);
        Ok(())
    }

    #[test]
    fn entity_is_inside_method_uses_world_space_transformed_bounds() -> mlua::Result<()> {
        let lua = Lua::new();
        let parent = create_entity_table(&lua, "parent", 100.0, 50.0, None)?;
        parent.set("scale", 2.0)?;

        let entity = create_entity_table(&lua, "child", 10.0, 5.0, Some(parent))?;
        entity.set("size_x", 20.0)?;
        entity.set("size_y", 10.0)?;

        let is_inside: Function = entity.get("IsInside")?;
        let lower_alias: Function = entity.get("isInside")?;
        assert!(is_inside.call::<bool>((entity.clone(), 130.0f32, 70.0f32))?);
        assert!(lower_alias.call::<bool>((entity.clone(), 160.0f32, 80.0f32))?);
        assert!(!is_inside.call::<bool>((entity, 99.0f32, 70.0f32))?);
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
    fn missing_app_background_defaults_to_white() -> mlua::Result<()> {
        let (runtime, root) = start_test_runtime("missing_app_background")?;

        let app: Table = runtime.lua.globals().get("app")?;
        app.raw_set("bg", Value::Nil)?;

        assert_eq!(runtime.resolve_app_clear_color()?, PlatformColor::WHITE);

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn legacy_global_bg_table_is_used_when_app_bg_is_missing() -> mlua::Result<()> {
        let (runtime, root) = start_test_runtime("legacy_global_bg")?;

        let app: Table = runtime.lua.globals().get("app")?;
        app.raw_set("bg", Value::Nil)?;

        let legacy_bg = runtime.lua.create_table()?;
        legacy_bg.set("R", 12)?;
        legacy_bg.set("G", 34)?;
        legacy_bg.set("B", 56)?;
        runtime.lua.globals().set("bg", legacy_bg)?;

        assert_eq!(
            runtime.resolve_app_clear_color()?,
            PlatformColor::rgba(12, 34, 56, 255)
        );

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn resolve_app_clear_color_uses_current_global_app_table() -> mlua::Result<()> {
        let (runtime, root) = start_test_runtime("replace_global_app")?;

        let replacement_app = runtime.lua.create_table()?;
        replacement_app.set("nearestNeighborScaling", true)?;
        replacement_app.set("bg", color4_table(&runtime.lua, 12, 34, 56, 78)?)?;
        runtime.lua.globals().set("app", replacement_app)?;

        assert_eq!(
            runtime.resolve_app_clear_color()?,
            PlatformColor::rgba(12, 34, 56, 78)
        );

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn app_runtime_settings_use_current_global_app_table() -> mlua::Result<()> {
        let (runtime, root) = start_test_runtime("replace_global_app_runtime_settings")?;

        let replacement_app = runtime.lua.create_table()?;
        replacement_app.set("nearestNeighborScaling", true)?;
        replacement_app.set("bg", color4_table(&runtime.lua, 255, 255, 255, 255)?)?;
        replacement_app.set("maxFps", 72.0)?;
        replacement_app.set("showFps", false)?;
        runtime.lua.globals().set("app", replacement_app)?;

        assert_eq!(runtime.max_fps(), Some(72.0));
        assert!(!runtime.show_fps());

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn rendering_component_update_can_mutate_component_fields() -> mlua::Result<()> {
        let (mut runtime, root) = start_test_runtime("rendering_component_mutation")?;

        let ecs: Table = runtime.lua.globals().get("ecs")?;
        let new_entity: Function = ecs.get("newEntity")?;
        let add_component: Function = ecs.get("addComponent")?;
        let entity: Table = new_entity.call((
            "renderable".to_string(),
            None::<Table>,
            Some(0.0),
            Some(0.0),
        ))?;
        entity.set("size_x", 10.0)?;
        entity.set("size_y", 10.0)?;

        let component = runtime.lua.create_table()?;
        component.set("__neolove_component", "MutableRender")?;
        component.set("NEOLOVE_RENDERING", true)?;
        component.set("counter", 0)?;
        component.set(
            "awake",
            runtime
                .lua
                .create_function(|_lua, (_entity, _component): (Table, Table)| Ok(()))?,
        )?;
        component.set(
            "update",
            runtime.lua.create_function(
                |_lua, (_entity, component, _dt): (Table, Table, f32)| {
                    let next = component.get::<i64>("counter").unwrap_or(0) + 1;
                    component.set("counter", next)?;
                    component.set("label", format!("step-{next}"))?;
                    Ok(())
                },
            )?,
        )?;

        let instance: Table = add_component.call((entity, Value::Table(component)))?;

        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        assert_eq!(instance.get::<i64>("counter")?, 1);
        assert_eq!(instance.get::<String>("label")?, "step-1");

        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        assert_eq!(instance.get::<i64>("counter")?, 2);
        assert_eq!(instance.get::<String>("label")?, "step-2");

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn textbox_letter_bounds_dot_call_refreshes_before_layout() -> mlua::Result<()> {
        let (mut runtime, root) = start_test_runtime("textbox_letter_bounds_dot")?;

        runtime
            .lua
            .load(
                r#"
                local entity = ecs.newEntity("text", ecs.root, 24, 32)
                local text = entity:AddComponent(core.TextBox)
                text.text = "abc"

                local probe = {
                    charPosition = 0,
                    awake = function(_entity, component)
                        component.__neolove_component = "LetterBoundsDotProbe"
                        component.ctb = text
                    end,
                    update = function(_entity, component, _dt)
                        local x, y, w, h = component.ctb.getLetterBounds(component.charPosition - 1)
                        letterBoundsDotResult = { x = x, y = y, w = w, h = h }
                        local firstX, firstY, firstW, firstH = component.ctb.getLetterBounds(0)
                        closestLetterStart = component.ctb.getClosestLetterIndex(firstX + firstW * 0.25, firstY + firstH * 0.5)
                        closestLetterEnd = component.ctb.getClosestCharacterIndex(firstX + firstW * 0.75, firstY + firstH * 0.5)
                    end,
                }

                entity:AddComponent(probe)
                "#,
            )
            .exec()?;

        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;

        let result: Table = runtime.lua.globals().get("letterBoundsDotResult")?;
        result.get::<f64>("x")?;
        result.get::<f64>("y")?;
        assert_eq!(result.get::<f64>("w")?, 0.0);
        assert!(result.get::<f64>("h")? > 0.0);
        assert_eq!(runtime.lua.globals().get::<i64>("closestLetterStart")?, 0);
        assert_eq!(runtime.lua.globals().get::<i64>("closestLetterEnd")?, 1);

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn textbox_character_pixel_offset_updates_layout_bounds() -> mlua::Result<()> {
        let (runtime, root) = start_test_runtime("textbox_character_offset")?;
        runtime
            .lua
            .load(
                r#"
                local entity = ecs.newEntity("offset text", ecs.root, 20, 30)
                local text = entity:AddComponent(core.TextBox)
                text.text = "AB"
                text.scale = 24
                local beforeX, beforeY = text:getLetterPosition(1)
                text:setCharacterOffset(1, 7, -3)
                local afterX, afterY = text:getLetterPosition(1)
                characterOffsetDeltaX = afterX - beforeX
                characterOffsetDeltaY = afterY - beforeY
                "#,
            )
            .set_name("@textbox_character_offset.luau")
            .exec()?;

        assert_close(runtime.lua.globals().get("characterOffsetDeltaX")?, 7.0);
        assert_close(runtime.lua.globals().get("characterOffsetDeltaY")?, -3.0);
        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn overlap_check_compares_entities_without_reiterating_lua_table() -> mlua::Result<()> {
        let (runtime, root) = start_test_runtime("overlap_check")?;

        let ecs: Table = runtime.lua.globals().get("ecs")?;
        let transform: Table = runtime.lua.globals().get("transform")?;
        let new_entity: Function = ecs.get("newEntity")?;
        let do_they_overlap: Function = transform.get("doTheyOverlap")?;

        let first: Table =
            new_entity.call(("first".to_string(), None::<Table>, Some(10.0), Some(20.0)))?;
        first.set("size_x", 40.0)?;
        first.set("size_y", 40.0)?;

        let second: Table =
            new_entity.call(("second".to_string(), None::<Table>, Some(30.0), Some(35.0)))?;
        second.set("size_x", 40.0)?;
        second.set("size_y", 40.0)?;

        let overlap_list = runtime.lua.create_table()?;
        overlap_list.set(1, first.clone())?;
        overlap_list.set(2, second.clone())?;
        let overlaps: bool = do_they_overlap.call(overlap_list)?;
        assert!(overlaps);

        second.set("x", 200.0)?;
        second.set("y", 200.0)?;
        let separated_list = runtime.lua.create_table()?;
        separated_list.set(1, first)?;
        separated_list.set(2, second)?;
        let overlaps: bool = do_they_overlap.call(separated_list)?;
        assert!(!overlaps);

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn entity_scaler_positions_entity_by_parent_percent_and_offset() -> mlua::Result<()> {
        let (mut runtime, root) = start_test_runtime("entity_scaler")?;

        let ecs: Table = runtime.lua.globals().get("ecs")?;
        let core: Table = runtime.lua.globals().get("core")?;
        let new_entity: Function = ecs.get("newEntity")?;
        let parent: Table =
            new_entity.call(("parent".to_string(), None::<Table>, Some(10.0), Some(20.0)))?;
        parent.set("size_x", 200.0)?;
        parent.set("size_y", 100.0)?;
        let child: Table =
            new_entity.call(("child".to_string(), Some(parent), Some(0.0), Some(0.0)))?;
        child.set("size_x", 20.0)?;
        child.set("size_y", 10.0)?;
        let add_component: Function = child.get("AddComponent")?;
        let scaler: Table =
            add_component.call((child.clone(), core.get::<Table>("EntityScaler")?))?;
        scaler.set("x_percent", 0.5)?;
        scaler.set("y_percent", 0.5)?;
        scaler.set("size_x_percent", 0.25)?;
        scaler.set("size_y_percent", 0.5)?;
        scaler.set("offset_x", 5.0)?;
        scaler.set("offset_y", -10.0)?;
        scaler.set("pivot_x", 0.5)?;
        scaler.set("pivot_y", 0.5)?;

        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;

        assert_eq!(child.get::<f32>("anchor_x")?, 0.5);
        assert_eq!(child.get::<f32>("anchor_y")?, 0.5);
        assert_eq!(child.get::<f32>("x")?, 5.0);
        assert_eq!(child.get::<f32>("y")?, -10.0);
        assert_eq!(child.get::<f32>("pivot_x")?, 0.5);
        assert_eq!(child.get::<f32>("pivot_y")?, 0.5);
        assert_eq!(child.get::<f32>("size_x")?, 50.0);
        assert_eq!(child.get::<f32>("size_y")?, 50.0);
        let (world_x, world_y) = get_global_position(&child)?;
        assert_eq!(world_x, 90.0);
        assert_eq!(world_y, 35.0);

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn inspector_declarations_evaluate_to_runtime_defaults() -> mlua::Result<()> {
        let (runtime, root) = start_test_runtime("inspector_defaults")?;
        let inspector: Function = runtime.lua.globals().get("Inspector")?;
        assert_eq!(inspector.call::<f32>((3.0, 10.0, true))?, 3.0);
        assert_eq!(inspector.call::<String>("hello")?, "hello");
        let default_table = runtime.lua.create_table()?;
        default_table.set(1, "value")?;
        let returned: Table = inspector.call(default_table.clone())?;
        assert_eq!(returned.to_pointer(), default_table.to_pointer());

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn logger_snapshot_includes_live_hierarchy_components_and_public_properties() -> mlua::Result<()>
    {
        let (runtime, root) = start_test_runtime("logger_snapshot")?;
        runtime
            .lua
            .load(
                r#"
                local group = ecs.newEntity("Group", ecs.root, 4, 5)
                local child = ecs.newEntity("Inspectable", group, 10, 20)
                child.custom = { label = "ready", count = 3 }
                local Probe = {
                    __neolove_component = "RuntimeProbe",
                    enabled = true,
                    tint = Color4(10, 20, 30, 40),
                    settings = { speed = 12, mode = "fast" },
                }
                function Probe.update(_entity, _self, _dt) end
                child:AddComponent(Probe)
                "#,
            )
            .exec()?;

        let snapshots = runtime.snapshot_entities();
        let group = snapshots
            .iter()
            .find(|entity| entity.name == "Group")
            .expect("group snapshot");
        let child = snapshots
            .iter()
            .find(|entity| entity.name == "Inspectable")
            .expect("child snapshot");
        assert_eq!(child.parent, Some(group.id));
        assert!(child.fields.iter().any(|(name, _)| name == "size_x"));
        assert!(child.fields.iter().any(|(name, value)| {
            name == "custom" && value.contains("label = ready") && value.contains("count = 3")
        }));

        let component = child
            .components
            .first()
            .expect("runtime component snapshot");
        assert_eq!(component.name, "RuntimeProbe");
        assert!(component.fields.iter().any(|(name, value)| {
            name == "tint" && value == "Color4(10, 20, 30, 40)"
        }));
        assert!(component.fields.iter().any(|(name, value)| {
            name == "settings" && value.contains("mode = fast") && value.contains("speed = 12")
        }));
        assert!(!component.fields.iter().any(|(name, _)| name == "entity"));
        assert!(!component.fields.iter().any(|(name, _)| name == "update"));

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn particle_system_emits_bounded_drawable_particles_and_controls_playback() -> mlua::Result<()>
    {
        let (mut runtime, root) = start_test_runtime("particle_system")?;
        runtime
            .lua
            .load(
                r#"
                local entity = ecs.newEntity("emitter", ecs.root, 100, 120)
                entity.size_x = 80
                entity.size_y = 40
                testParticles = entity:AddComponent(core.ParticleSystem2D)
                testParticles.playing = false
                testParticles.gravity_y = 0
                testParticles.color_sequence = {
                    { time = 0, color = Color4(10, 20, 30) },
                    { time = 1, color = Color4(40, 50, 60) },
                }
                testParticles.transparency_sequence = {
                    { time = 0, value = 0.5 },
                    { time = 1, value = 0.5 },
                }
                testParticles:emit(4)
                "#,
            )
            .set_name("@particle_system_test.luau")
            .exec()?;

        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        let component: Table = runtime.lua.globals().get("testParticles")?;
        assert_eq!(component.get::<usize>("particle_count")?, 4);
        let commands = crate::renderer::drain_commands(&runtime.render_state())
            .map_err(mlua::Error::external)?;
        assert_eq!(
            commands
                .iter()
                .filter(|command| matches!(command, crate::renderer::DrawCommand::Circle { .. }))
                .count(),
            4
        );
        assert!(commands.iter().all(|command| match command {
            crate::renderer::DrawCommand::Circle { color, .. } => {
                *color == crate::platform::Color::rgba(10, 20, 30, 128)
            }
            _ => true,
        }));

        let stop: Function = component.get("stop")?;
        stop.call::<()>(component.clone())?;
        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        assert_eq!(component.get::<usize>("particle_count")?, 0);

        component.set("emission_rate", 20.0)?;
        let play: Function = component.get("play")?;
        play.call::<()>(component.clone())?;
        runtime.update(0.25).map_err(mlua::Error::external)?;
        assert_eq!(component.get::<usize>("particle_count")?, 5);

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn spatial_sound_component_exposes_safe_playback_controls() -> mlua::Result<()> {
        let (mut runtime, root) = start_test_runtime("spatial_sound")?;
        runtime
            .lua
            .load(
                r#"
                local entity = ecs.newEntity("speaker", ecs.root, 100, 120)
                testSpatialSound = entity:AddComponent(core.SpatialSound2D)
                testSpatialPlayResult = testSpatialSound:play()
                "#,
            )
            .set_name("@spatial_sound_test.luau")
            .exec()?;

        let component: Table = runtime.lua.globals().get("testSpatialSound")?;
        assert_eq!(
            component.get::<String>("__neolove_component")?,
            "SpatialSound2D"
        );
        assert!(!runtime.lua.globals().get::<bool>("testSpatialPlayResult")?);
        assert!(component.get::<Function>("play").is_ok());
        assert!(component.get::<Function>("stop").is_ok());
        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn component_update_can_spawn_renderable_physics_entities() -> mlua::Result<()> {
        let (mut runtime, root) = start_test_runtime("spawn_physics_from_update")?;
        runtime
            .lua
            .load(
                r#"
                local dispenser = ecs.newEntity("dispenser", ecs.root, 10, 20)
                dispenser.size_x = 40
                dispenser.size_y = 20

                local Behaviour = {
                    enabled = true,
                    colour = Color4(120, 221, 255),
                    spawned = false,
                }
                function Behaviour.awake(_entity, _self) end
                function Behaviour.update(entity, self, _dt)
                    if not self.enabled or self.spawned then
                        return
                    end
                    self.spawned = true
                    local particle = ecs.newEntity(
                        "particle",
                        ecs.root,
                        entity.x + entity.size_x / 2,
                        entity.y + entity.size_y / 2
                    )
                    particle.position_pivot = "center"
                    particle.size_x = 12
                    particle.size_y = 12
                    local renderer = particle:AddComponent(core.Shape2D)
                    renderer.shape = "circle"
                    renderer.color = self.colour
                    local collider = particle:AddComponent(core.Collider2D)
                    collider.shape = "circle"
                    particle:AddComponent(core.Rigidbody2D)
                end
                dispenser:AddComponent(Behaviour)
                "#,
            )
            .set_name("@spawn_physics_from_update.luau")
            .exec()?;

        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;

        let ecs: Table = runtime.lua.globals().get("ecs")?;
        let root_entity: Table = ecs.get("root")?;
        let children: Table = root_entity.get("children")?;
        assert_eq!(children.raw_len(), 2);

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn script_set_position_and_rotation_move_dynamic_rigidbody() -> mlua::Result<()> {
        let (mut runtime, root) = start_test_runtime("rigidbody_script_teleport")?;
        runtime
            .lua
            .load(
                r#"
                body = ecs.newEntity("body", ecs.root, 100, 100)
                body.size_x = 20
                body.size_y = 20
                local rb = body:AddComponent(core.Rigidbody2D)
                rb.gravity_scale = 0
                "#,
            )
            .set_name("@rigidbody_script_teleport.luau")
            .exec()?;

        // First frame builds the physics world and records the body's pose.
        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;

        let body: Table = runtime.lua.globals().get("body")?;
        // With no gravity and no velocity the body stays put after one step.
        assert_close(body.get::<f32>("x")?, 100.0);
        assert_close(body.get::<f32>("y")?, 100.0);
        assert_close(body.get::<f32>("rotation")?, 0.0);

        // A script assigns a new position and rotation directly on the entity.
        runtime
            .lua
            .load(
                r#"
                body.x = 250
                body.y = 175
                body.rotation = 1.2
                "#,
            )
            .exec()?;

        // Next frame must teleport the dynamic body instead of overwriting the
        // script's values with the simulation's own (unchanged) pose.
        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;

        assert_close(body.get::<f32>("x")?, 250.0);
        assert_close(body.get::<f32>("y")?, 175.0);
        assert_close(body.get::<f32>("rotation")?, 1.2);

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn script_set_velocity_still_integrates_dynamic_rigidbody() -> mlua::Result<()> {
        let (mut runtime, root) = start_test_runtime("rigidbody_script_velocity")?;
        runtime
            .lua
            .load(
                r#"
                body = ecs.newEntity("body", ecs.root, 100, 100)
                body.size_x = 20
                body.size_y = 20
                local rb = body:AddComponent(core.Rigidbody2D)
                rb.gravity_scale = 0
                rb.velocity_x = 600
                "#,
            )
            .set_name("@rigidbody_script_velocity.luau")
            .exec()?;

        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;

        // Setting velocity still moves the body (the teleport detection must not
        // clobber velocity-driven motion): +600px/s over 1/60s ≈ +10px.
        let body: Table = runtime.lua.globals().get("body")?;
        let x = body.get::<f32>("x")?;
        assert!(x > 105.0, "expected velocity to advance x past 105, got {x}");
        assert_close(body.get::<f32>("y")?, 100.0);

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn rendering_component_errors_are_returned_with_entity_context() -> mlua::Result<()> {
        let (mut runtime, root) = start_test_runtime("rendering_component_error")?;
        runtime
            .lua
            .load(
                r#"
                local particle = ecs.newEntity("broken particle", ecs.root, 10, 20)
                local renderer = particle:AddComponent(core.Shape2D)
                renderer.color = nil
                "#,
            )
            .exec()?;

        let error = runtime
            .update(1.0 / 60.0)
            .expect_err("invalid rendering fields must fail the frame");
        assert!(
            error.contains("rendering component update failed"),
            "{error}"
        );
        assert!(error.contains("Shape2D"), "{error}");
        assert!(error.contains("broken particle"), "{error}");
        assert!(error.contains("nil to table"), "{error}");

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn cyclic_component_tables_are_copied_without_recursing_forever() -> mlua::Result<()> {
        let (runtime, root) = start_test_runtime("cyclic_component")?;
        runtime
            .lua
            .load(
                r#"
                local entity = ecs.newEntity("cyclic", ecs.root, 0, 0)
                local Component = {}
                Component.self = Component
                function Component.awake(_entity, _self) end
                function Component.update(_entity, _self, _dt) end
                cyclicInstance = entity:AddComponent(Component)
                assert(cyclicInstance.self == cyclicInstance)
                "#,
            )
            .exec()?;

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn system_can_register_another_system_during_update() -> mlua::Result<()> {
        let (mut runtime, root) = start_test_runtime("system_register_during_update")?;
        runtime
            .lua
            .load(
                r#"
                firstSystemUpdates = 0
                secondSystemUpdates = 0
                local first = {}
                function first.update(_self, _dt)
                    firstSystemUpdates += 1
                    if firstSystemUpdates == 1 then
                        ecs.addSystem({
                            update = function(_self, _dt)
                                secondSystemUpdates += 1
                            end,
                        })
                    end
                end
                ecs.addSystem(first)
                "#,
            )
            .exec()?;

        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        assert_eq!(runtime.lua.globals().get::<i64>("firstSystemUpdates")?, 1);
        assert_eq!(runtime.lua.globals().get::<i64>("secondSystemUpdates")?, 0);
        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        assert_eq!(runtime.lua.globals().get::<i64>("firstSystemUpdates")?, 2);
        assert_eq!(runtime.lua.globals().get::<i64>("secondSystemUpdates")?, 1);

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn exported_script_component_require_path_starts_runtime() -> mlua::Result<()> {
        use crate::scene::{Component, Scene};

        let root = temp_project_root("script_component_require");
        std::fs::create_dir_all(root.join("scripts")).map_err(mlua::Error::external)?;
        std::fs::write(
            root.join("scripts/WaterDispenser.luau"),
            "local Component = { enabled = Inspector(true) }\nfunction Component.awake(entity, self) end\nfunction Component.update(entity, self, dt) end\nreturn Component\n",
        )
        .map_err(mlua::Error::external)?;
        let mut scene = Scene::default();
        scene.entities[0].components.push(Component::Script {
            path: "scripts/WaterDispenser.luau".into(),
            variables: Vec::new(),
        });
        std::fs::write(root.join("main.luau"), scene.to_luau()).map_err(mlua::Error::external)?;
        if let Some(images) = scene.to_images_luau() {
            std::fs::write(root.join("images.luau"), images).map_err(mlua::Error::external)?;
        }

        let mut runtime = Runtime::new(root.clone());
        runtime.start()?;

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn load_scene_resolves_paths_from_project_root() -> mlua::Result<()> {
        use crate::scene::{Component, Scene};

        let root = temp_project_root("load_scene_project_root");
        std::fs::create_dir_all(&root).map_err(mlua::Error::external)?;
        std::fs::write(
            root.join("main.luau"),
            r#"
                ecs.newEntity("StaleRootChild", ecs.root)
                ecs.newEntity("StaleOrphan", nil)
                ecs.loadScene("scene.neoscene")
            "#,
        )
        .map_err(mlua::Error::external)?;

        let mut scene = Scene::default();
        let entity_id = scene.entities[0].id;
        {
            let entity = scene.entity_mut(entity_id).expect("default entity");
            entity.name = "LoadedFromScene".into();
            entity.x = 24.0;
            entity.y = 36.0;
            entity.components.push(Component::core("Rect2D"));
        }
        scene
            .save(&root.join("scene.neoscene"))
            .map_err(mlua::Error::external)?;

        let mut runtime = Runtime::new(root.clone());
        runtime.start()?;

        let ecs: Table = runtime.lua.globals().get("ecs")?;
        let root_entity: Table = ecs.get("root")?;
        let children: Table = root_entity.get("children")?;
        assert_eq!(children.raw_len(), 1);
        let loaded: Table = children.get(1)?;
        assert_eq!(loaded.get::<String>("name")?, "LoadedFromScene");
        assert_eq!(loaded.get::<f32>("x")?, 24.0);
        assert_eq!(loaded.get::<f32>("y")?, 36.0);
        assert_eq!(loaded.get::<Table>("components")?.raw_len(), 1);
        let names: Vec<String> = runtime
            .snapshot_entities()
            .into_iter()
            .map(|entity| entity.name)
            .collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"root".to_string()));
        assert!(names.contains(&"LoadedFromScene".to_string()));
        assert!(!names.contains(&"StaleRootChild".to_string()));
        assert!(!names.contains(&"StaleOrphan".to_string()));

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn load_scene_replaces_existing_scene_entities() -> mlua::Result<()> {
        use crate::scene::Scene;

        let root = temp_project_root("load_scene_replaces_entities");
        std::fs::create_dir_all(&root).map_err(mlua::Error::external)?;
        std::fs::write(
            root.join("main.luau"),
            r#"
                ecs.loadScene("first.neoscene")
                ecs.loadScene("second.neoscene")
            "#,
        )
        .map_err(mlua::Error::external)?;

        let mut first = Scene::default();
        let first_id = first.entities[0].id;
        first.entity_mut(first_id).expect("first").name = "FirstScene".into();
        let first_child = first.add_entity("FirstSceneChild", 1.0, 2.0).id;
        first.entity_mut(first_child).expect("child").parent = Some(first_id);
        first
            .save(&root.join("first.neoscene"))
            .map_err(mlua::Error::external)?;

        let mut second = Scene::default();
        let second_id = second.entities[0].id;
        second.entity_mut(second_id).expect("second").name = "SecondScene".into();
        second
            .save(&root.join("second.neoscene"))
            .map_err(mlua::Error::external)?;

        let mut runtime = Runtime::new(root.clone());
        runtime.start()?;

        let ecs: Table = runtime.lua.globals().get("ecs")?;
        let root_entity: Table = ecs.get("root")?;
        let children: Table = root_entity.get("children")?;
        assert_eq!(children.raw_len(), 1);
        assert_eq!(
            children.get::<Table>(1)?.get::<String>("name")?,
            "SecondScene"
        );
        let names: Vec<String> = runtime
            .snapshot_entities()
            .into_iter()
            .map(|entity| entity.name)
            .collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"root".to_string()));
        assert!(names.contains(&"SecondScene".to_string()));
        assert!(!names.contains(&"FirstScene".to_string()));
        assert!(!names.contains(&"FirstSceneChild".to_string()));

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn load_scene_loads_images_absent_from_exported_start_scene() -> mlua::Result<()> {
        use crate::scene::{Component, PropValue, Scene};

        // Regression: a scene loaded at runtime references images that the
        // exported start scene never wrote into `./images`. Here the project has
        // no `images.luau` at all, so the loaded scene must inline its own image
        // cache — otherwise `require("./images")` aborts the whole scene chunk
        // and none of its entities (images or sounds) appear.
        let root = temp_project_root("load_scene_images");
        std::fs::create_dir_all(root.join("assets")).map_err(mlua::Error::external)?;

        // A real 1x1 PNG so `assets.loadImage` succeeds.
        let png = {
            let mut image = image::RgbaImage::new(1, 1);
            image.put_pixel(0, 0, image::Rgba([10, 20, 30, 255]));
            let mut bytes = Vec::new();
            image::DynamicImage::ImageRgba8(image)
                .write_to(
                    &mut std::io::Cursor::new(&mut bytes),
                    image::ImageOutputFormat::Png,
                )
                .map_err(mlua::Error::external)?;
            bytes
        };
        std::fs::write(root.join("assets/sprite.png"), png).map_err(mlua::Error::external)?;

        std::fs::write(root.join("main.luau"), r#"ecs.loadScene("level.neoscene")"#)
            .map_err(mlua::Error::external)?;

        let mut scene = Scene::default();
        let sprite_id = scene.entities[0].id;
        scene.entity_mut(sprite_id).expect("sprite").name = "ImageSprite".into();
        let mut sprite = Component::core("Sprite2D");
        if let Component::Core { props, .. } = &mut sprite {
            for prop in props.iter_mut() {
                if let PropValue::Image(path) = &mut prop.value {
                    *path = "assets/sprite.png".into();
                }
            }
        }
        scene
            .entity_mut(sprite_id)
            .expect("sprite")
            .components
            .push(sprite);
        scene
            .save(&root.join("level.neoscene"))
            .map_err(mlua::Error::external)?;

        // Before the fix this errored on `require("./images")`.
        let mut runtime = Runtime::new(root.clone());
        runtime.start()?;

        let names: Vec<String> = runtime
            .snapshot_entities()
            .into_iter()
            .map(|entity| entity.name)
            .collect();
        assert!(names.contains(&"ImageSprite".to_string()));

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn load_scene_still_reads_legacy_json_scene_files() -> mlua::Result<()> {
        use crate::scene::Scene;

        let root = temp_project_root("load_legacy_json_scene");
        std::fs::create_dir_all(&root).map_err(mlua::Error::external)?;
        std::fs::write(root.join("main.luau"), "ecs.loadScene(\"scene.neoscene\")")
            .map_err(mlua::Error::external)?;

        let mut scene = Scene::default();
        scene.entities[0].name = "LegacyJsonScene".into();
        std::fs::write(
            root.join("scene.neoscene"),
            scene.to_json().map_err(mlua::Error::external)?,
        )
        .map_err(mlua::Error::external)?;

        let mut runtime = Runtime::new(root.clone());
        runtime.start()?;

        let ecs: Table = runtime.lua.globals().get("ecs")?;
        let root_entity: Table = ecs.get("root")?;
        let loaded: Table = root_entity.get::<Table>("children")?.get(1)?;
        assert_eq!(loaded.get::<String>("name")?, "LegacyJsonScene");

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn load_scene_resolves_script_requires_from_project_root() -> mlua::Result<()> {
        use crate::scene::{Component, Scene};

        let root = temp_project_root("load_scene_script_require");
        std::fs::create_dir_all(root.join("levels")).map_err(mlua::Error::external)?;
        std::fs::create_dir_all(root.join("scripts")).map_err(mlua::Error::external)?;
        std::fs::write(
            root.join("main.luau"),
            "ecs.loadScene(\"levels/title.neoscene\")",
        )
        .map_err(mlua::Error::external)?;
        std::fs::write(
            root.join("scripts/SceneMarker.luau"),
            "return { __neolove_component = \"SceneMarker\", update = function() end }\n",
        )
        .map_err(mlua::Error::external)?;

        let mut scene = Scene::default();
        let entity_id = scene.entities[0].id;
        {
            let entity = scene.entity_mut(entity_id).expect("default entity");
            entity.name = "ScriptedSceneEntity".into();
            entity.components.push(Component::Script {
                path: "scripts/SceneMarker.luau".into(),
                variables: Vec::new(),
            });
        }
        scene
            .save(&root.join("levels/title.neoscene"))
            .map_err(mlua::Error::external)?;

        let mut runtime = Runtime::new(root.clone());
        runtime.start()?;

        let ecs: Table = runtime.lua.globals().get("ecs")?;
        let root_entity: Table = ecs.get("root")?;
        let loaded: Table = root_entity.get::<Table>("children")?.get(1)?;
        let component: Table = loaded.get::<Table>("components")?.get(1)?;
        assert_eq!(loaded.get::<String>("name")?, "ScriptedSceneEntity");
        assert_eq!(
            component.get::<String>("__neolove_component")?,
            "SceneMarker"
        );

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn runtime_loads_and_instantiates_editor_neoprefab_files() -> mlua::Result<()> {
        use crate::scene::{
            AttachedValue, Component, ComponentReference, DictionaryEntry, Scene, VarKey,
            VarValue,
        };

        let (runtime, root) = start_test_runtime("load_neoprefab")?;
        let mut prefab_scene = Scene::default();
        let root_id = prefab_scene.entities[0].id;
        {
            let entity = prefab_scene.entity_mut(root_id).expect("prefab root");
            entity.name = "LoadedRoot".into();
            entity.x = 12.0;
            entity.y = 18.0;
            entity.size_x = 96.0;
            entity.size_y = 48.0;
            entity.position_pivot = "center".into();
            entity.pivot_x = Some(0.25);
            entity.pivot_y = Some(0.75);
            entity.rotation_pivot = "middle".into();
            entity.rotation_pivot_x = Some(0.6);
            entity.rotation_pivot_y = Some(0.4);
            entity.components.push(Component::core("Rect2D"));
        }
        let child_id = prefab_scene.add_entity("LoadedChild", 7.0, 9.0).id;
        prefab_scene
            .entity_mut(child_id)
            .expect("prefab child")
            .parent = Some(root_id);
        prefab_scene.entity_mut(root_id).expect("prefab root").values = vec![
            AttachedValue {
                name: "score".into(),
                value: VarValue::Number(42.0),
            },
            AttachedValue {
                name: "title".into(),
                value: VarValue::Text("Boss".into()),
            },
            AttachedValue {
                name: "childRef".into(),
                value: VarValue::Entity(Some(child_id)),
            },
            AttachedValue {
                name: "renderer".into(),
                value: VarValue::Component(Some(ComponentReference {
                    entity: root_id,
                    component: 0,
                })),
            },
            AttachedValue {
                name: "settings".into(),
                value: VarValue::Dictionary(vec![DictionaryEntry {
                    key: VarKey::Text("aggressive".into()),
                    value: VarValue::Bool(true),
                }]),
            },
        ];
        let json = serde_json::to_string_pretty(&prefab_scene.subtree(root_id))
            .map_err(mlua::Error::external)?;
        std::fs::write(root.join("enemy.neoprefab"), json).map_err(mlua::Error::external)?;

        let prefabs: Table = runtime.lua.globals().get("prefabs")?;
        let load: Function = prefabs.get("load")?;
        let template: Table = load.call("enemy.neoprefab")?;
        assert_eq!(template.get::<String>("name")?, "LoadedRoot");
        assert_eq!(template.get::<f64>("score")?, 42.0);
        assert_eq!(template.get::<String>("title")?, "Boss");
        assert_eq!(template.get::<String>("position_pivot")?, "center");
        assert_eq!(template.get::<f32>("pivot_x")?, 0.25);
        assert_eq!(template.get::<f32>("pivot_y")?, 0.75);
        assert_eq!(template.get::<String>("rotation_pivot")?, "middle");
        assert_eq!(template.get::<f32>("rotation_pivot_x")?, 0.6);
        assert_eq!(template.get::<f32>("rotation_pivot_y")?, 0.4);

        let instantiate: Function = prefabs.get("instantiate")?;
        let ecs: Table = runtime.lua.globals().get("ecs")?;
        let instance: Table = instantiate.call((template, ecs.get::<Table>("root")?))?;
        assert_eq!(instance.get::<String>("name")?, "LoadedRoot");
        assert_eq!(instance.get::<f32>("x")?, 12.0);
        assert_eq!(instance.get::<f32>("size_x")?, 96.0);
        assert_eq!(instance.get::<String>("position_pivot")?, "center");
        assert_eq!(instance.get::<f32>("pivot_x")?, 0.25);
        assert_eq!(instance.get::<f32>("pivot_y")?, 0.75);
        assert_eq!(instance.get::<String>("rotation_pivot")?, "middle");
        assert_eq!(instance.get::<f32>("rotation_pivot_x")?, 0.6);
        assert_eq!(instance.get::<f32>("rotation_pivot_y")?, 0.4);
        let children: Table = instance.get("children")?;
        let child: Table = children.get(1)?;
        assert_eq!(child.get::<String>("name")?, "LoadedChild");
        let components: Table = instance.get("components")?;
        assert_eq!(components.len()?, 1);
        let child_ref: Table = instance.get("childRef")?;
        assert_eq!(child_ref.to_pointer(), child.to_pointer());
        let renderer: Table = instance.get("renderer")?;
        assert_eq!(renderer.to_pointer(), components.get::<Table>(1)?.to_pointer());
        let settings: Table = instance.get("settings")?;
        assert!(settings.get::<bool>("aggressive")?);

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn runtime_loads_compressed_editor_neoprefab_files() -> mlua::Result<()> {
        use crate::scene::{self, Component, Scene};

        let (runtime, root) = start_test_runtime("load_binary_neoprefab")?;
        let mut prefab_scene = Scene::default();
        let root_id = prefab_scene.entities[0].id;
        {
            let entity = prefab_scene.entity_mut(root_id).expect("prefab root");
            entity.name = "CompressedRoot".into();
            entity.components.push(Component::core("Rect2D"));
        }
        let entities = prefab_scene.subtree(root_id);
        scene::save_prefab(&root.join("compressed.neoprefab"), &entities)
            .map_err(mlua::Error::external)?;

        let prefabs: Table = runtime.lua.globals().get("prefabs")?;
        let load: Function = prefabs.get("load")?;
        let template: Table = load.call("compressed.neoprefab")?;
        assert_eq!(template.get::<String>("name")?, "CompressedRoot");
        let components: Table = template.get("components")?;
        assert_eq!(components.len()?, 1);

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn nested_script_loads_prefab_components_from_project_root() -> mlua::Result<()> {
        use crate::scene::{Component, PropValue, Scene, ScriptVar, VarControl, VarValue};

        let root = temp_project_root("nested_script_prefab_load");
        std::fs::create_dir_all(root.join("scripts")).map_err(mlua::Error::external)?;
        std::fs::create_dir_all(root.join("prefabs")).map_err(mlua::Error::external)?;
        std::fs::write(
            root.join("main.luau"),
            "LoadedTemplate = require(\"./scripts/BuildButtonSpawner\")\n",
        )
        .map_err(mlua::Error::external)?;
        std::fs::write(
            root.join("scripts/BuildButtonSpawner.luau"),
            "return prefab.load(\"prefabs/button.neoprefab\")\n",
        )
        .map_err(mlua::Error::external)?;
        std::fs::write(
            root.join("scripts/HoverFX.luau"),
            r#"
                local Component = { icon = Inspector(IEntity), speed = Inspector(15) }
                function Component.awake(entity, self)
                    self.awakeCount = (self.awakeCount or 0) + 1
                    self.referenceReady = self.icon == nil or self.icon.parent == entity
                    self.speed = 999
                    AwakeOrder = AwakeOrder or {}
                    table.insert(AwakeOrder, entity.name)
                end
                function Component.update(_entity, _self, _dt) end
                return Component
            "#,
        )
        .map_err(mlua::Error::external)?;

        let mut scene = Scene::default();
        let button_id = scene.entities[0].id;
        scene.entity_mut(button_id).expect("button").name = "Button".into();
        let icon_id = scene.add_entity("Icon", 4.0, 5.0).id;
        scene.entity_mut(icon_id).expect("icon").parent = Some(button_id);
        scene
            .entity_mut(button_id)
            .expect("button")
            .components
            .push(Component::Script {
                path: "scripts/HoverFX.luau".into(),
                variables: vec![ScriptVar {
                    name: "icon".into(),
                    value: VarValue::Entity(Some(icon_id)),
                    control: VarControl::Field,
                }],
            });
        scene
            .entity_mut(icon_id)
            .expect("icon")
            .components
            .push(Component::Script {
                path: "scripts/HoverFX.luau".into(),
                variables: Vec::new(),
            });
        let mut scaler = Component::core("EntityScaler");
        let Component::Core { props, .. } = &mut scaler else {
            unreachable!();
        };
        props
            .iter_mut()
            .find(|prop| prop.name == "x_percent")
            .expect("x_percent")
            .value = PropValue::Number(0.75);
        scene
            .entity_mut(icon_id)
            .expect("icon")
            .components
            .push(scaler);
        let json = serde_json::to_string_pretty(&scene.subtree(button_id))
            .map_err(mlua::Error::external)?;
        std::fs::write(root.join("prefabs/button.neoprefab"), json)
            .map_err(mlua::Error::external)?;

        let mut runtime = Runtime::new(root.clone());
        runtime.start()?;

        let template: Table = runtime.lua.globals().get("LoadedTemplate")?;
        let template_icon: Table = template.get::<Table>("children")?.get(1)?;
        let template_component: Table = template.get::<Table>("components")?.get(1)?;
        assert_eq!(template_component.get::<Table>("icon")?, template_icon);
        assert!(matches!(
            template_component.raw_get::<Value>("awakeCount")?,
            Value::Nil
        ));
        assert!(
            runtime
                .lua
                .globals()
                .get::<Option<Table>>("AwakeOrder")?
                .is_none()
        );

        let prefab: Table = runtime.lua.globals().get("prefab")?;
        let duplicate: Function = prefab.get("duplicate")?;
        let ecs: Table = runtime.lua.globals().get("ecs")?;
        let instance: Table = duplicate.call((template, ecs.get::<Table>("root")?))?;
        let instance_icon: Table = instance.get::<Table>("children")?.get(1)?;
        let instance_component: Table = instance.get::<Table>("components")?.get(1)?;
        assert_eq!(instance_component.get::<Table>("icon")?, instance_icon);
        assert_eq!(instance_component.get::<i64>("awakeCount")?, 1);
        assert!(instance_component.get::<bool>("referenceReady")?);
        assert_eq!(instance_component.get::<f32>("speed")?, 15.0);
        let icon_component: Table = instance_icon.get::<Table>("components")?.get(1)?;
        assert_eq!(icon_component.get::<i64>("awakeCount")?, 1);
        assert!(icon_component.get::<bool>("referenceReady")?);
        assert_eq!(icon_component.get::<f32>("speed")?, 15.0);
        let scaler: Table = instance_icon.get::<Table>("components")?.get(2)?;
        assert_eq!(scaler.get::<f32>("x_percent")?, 0.75);
        let awake_order: Table = runtime.lua.globals().get("AwakeOrder")?;
        assert_eq!(awake_order.get::<String>(1)?, "Button");
        assert_eq!(awake_order.get::<String>(2)?, "Icon");

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn get_entities_in_front_filters_point_and_z_and_sorts_frontmost_first() -> mlua::Result<()> {
        let (runtime, root) = start_test_runtime("entities_in_front")?;

        let ecs: Table = runtime.lua.globals().get("ecs")?;
        let transform: Table = runtime.lua.globals().get("transform")?;
        let new_entity: Function = ecs.get("newEntity")?;
        let get_entities_in_front: Function = transform.get("GetEntitiesInFront")?;

        let back: Table =
            new_entity.call(("back".to_string(), None::<Table>, Some(10.0), Some(20.0)))?;
        back.set("size_x", 50.0)?;
        back.set("size_y", 40.0)?;
        back.set("z", 2.0)?;

        let front: Table =
            new_entity.call(("front".to_string(), None::<Table>, Some(20.0), Some(25.0)))?;
        front.set("size_x", 30.0)?;
        front.set("size_y", 30.0)?;
        front.set("z", 8.0)?;

        let outside: Table = new_entity.call((
            "outside".to_string(),
            None::<Table>,
            Some(200.0),
            Some(200.0),
        ))?;
        outside.set("size_x", 40.0)?;
        outside.set("size_y", 40.0)?;
        outside.set("z", 20.0)?;

        let matches: Table = get_entities_in_front.call((30.0f32, 30.0f32, None::<f64>))?;
        assert_eq!(matches.raw_len(), 2);
        assert_eq!(matches.raw_get::<Table>(1)?.get::<String>("name")?, "front");
        assert_eq!(matches.raw_get::<Table>(2)?.get::<String>("name")?, "back");

        let filtered: Table = get_entities_in_front.call((30.0f32, 30.0f32, Some(8.0f64)))?;
        assert_eq!(filtered.raw_len(), 1);
        assert_eq!(
            filtered.raw_get::<Table>(1)?.get::<String>("name")?,
            "front"
        );

        let empty: Table = get_entities_in_front.call((30.0f32, 30.0f32, Some(9.0f64)))?;
        assert_eq!(empty.raw_len(), 0);

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn spritebox_uses_opaque_sprite_pixels_for_hit_testing() -> mlua::Result<()> {
        let (runtime, root) = start_test_runtime("spritebox_hit_testing")?;

        let mut sprite = image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 0]));
        for y in 1..=2 {
            for x in 1..=2 {
                sprite.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
        }
        let image = runtime
            .lua
            .create_userdata(crate::assets::ImageHandle::from_rgba_image(sprite))?;

        let ecs: Table = runtime.lua.globals().get("ecs")?;
        let core: Table = runtime.lua.globals().get("core")?;
        let new_entity: Function = ecs.get("newEntity")?;
        let add_component: Function = ecs.get("addComponent")?;
        let sprite_proto: Table = core.get("Sprite2D")?;
        let spritebox_proto: Table = core.get("Spritebox2D")?;

        let first: Table =
            new_entity.call(("first".to_string(), None::<Table>, Some(10.0), Some(10.0)))?;
        first.set("size_x", 40.0)?;
        first.set("size_y", 40.0)?;
        let first_sprite: Table = add_component.call((first.clone(), sprite_proto.clone()))?;
        first_sprite.set("image", image.clone())?;
        let first_box: Table = add_component.call((first.clone(), spritebox_proto.clone()))?;
        let compute: Function = first_box.get("ComputeSpritebox")?;
        assert!(compute.call::<bool>(first_box.clone())?);
        assert_eq!(first_box.get::<usize>("rect_count")?, 1);

        let is_inside: Function = first_box.get("IsInside")?;
        assert!(is_inside.call::<bool>((first_box.clone(), 25.0f32, 25.0f32))?);
        assert!(!is_inside.call::<bool>((first_box.clone(), 12.0f32, 12.0f32))?);

        let second: Table =
            new_entity.call(("second".to_string(), None::<Table>, Some(25.0), Some(10.0)))?;
        second.set("size_x", 40.0)?;
        second.set("size_y", 40.0)?;
        let second_sprite: Table = add_component.call((second.clone(), sprite_proto))?;
        second_sprite.set("image", image)?;
        let second_box: Table = add_component.call((second.clone(), spritebox_proto))?;
        let compute_second: Function = second_box.get("ComputeSpritebox")?;
        assert!(compute_second.call::<bool>(second_box.clone())?);

        let intersects: Function = first_box.get("IsIntersecting")?;
        assert!(intersects.call::<bool>((first_box.clone(), second.clone()))?);
        assert!(intersects.call::<bool>((first_box.clone(), second_box.clone()))?);

        second.set("x", 60.0)?;
        assert!(!intersects.call::<bool>((first_box, second))?);

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn nine_slice_sprite_queues_slice_images() -> mlua::Result<()> {
        let (mut runtime, root) = start_test_runtime("nine_slice_sprite")?;

        let sprite = image::RgbaImage::from_pixel(3, 3, image::Rgba([255, 255, 255, 255]));
        let image = runtime
            .lua
            .create_userdata(crate::assets::ImageHandle::from_rgba_image(sprite))?;

        let ecs: Table = runtime.lua.globals().get("ecs")?;
        let core: Table = runtime.lua.globals().get("core")?;
        let new_entity: Function = ecs.get("newEntity")?;
        let add_component: Function = ecs.get("addComponent")?;
        let entity: Table =
            new_entity.call(("panel".to_string(), None::<Table>, Some(0.0), Some(0.0)))?;
        entity.set("size_x", 30.0)?;
        entity.set("size_y", 30.0)?;
        let nine_slice_proto: Table = core.get("NineSliceSprite2D")?;
        let nine_slice: Table = add_component.call((entity, nine_slice_proto))?;
        nine_slice.set("image", image)?;
        nine_slice.set("slice_left", 1.0)?;
        nine_slice.set("slice_right", 1.0)?;
        nine_slice.set("slice_top", 1.0)?;
        nine_slice.set("slice_bottom", 1.0)?;

        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        let commands = crate::renderer::drain_commands(&runtime.render_state())
            .map_err(mlua::Error::external)?;
        let image_commands = commands
            .iter()
            .filter(|command| matches!(command, crate::renderer::DrawCommand::Image { .. }))
            .count();
        assert_eq!(image_commands, 9);

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn sprite_sheet_queues_the_selected_atlas_frame() -> mlua::Result<()> {
        let (mut runtime, root) = start_test_runtime("sprite_sheet_frame")?;
        let pixels = image::RgbaImage::from_pixel(8, 4, image::Rgba([255, 255, 255, 255]));
        let image = runtime
            .lua
            .create_userdata(crate::assets::ImageHandle::from_rgba_image(pixels))?;
        let ecs: Table = runtime.lua.globals().get("ecs")?;
        let core: Table = runtime.lua.globals().get("core")?;
        let entity: Table = ecs.get::<Function>("newEntity")?.call((
            "sheet".to_string(),
            None::<Table>,
            Some(0.0),
            Some(0.0),
        ))?;
        entity.set("size_x", 16.0)?;
        entity.set("size_y", 16.0)?;
        let sheet: Table = ecs
            .get::<Function>("addComponent")?
            .call((entity, core.get::<Table>("SpriteSheet2D")?))?;
        sheet.set("image", image)?;
        sheet.set("frame_width", 2.0)?;
        sheet.set("frame_height", 2.0)?;
        sheet.set("frame", 5)?;
        sheet.set("playing", false)?;

        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        let commands = crate::renderer::drain_commands(&runtime.render_state())
            .map_err(mlua::Error::external)?;
        let source = commands.iter().find_map(|command| match command {
            crate::renderer::DrawCommand::Image {
                source: Some(source),
                ..
            } => Some(*source),
            _ => None,
        });
        let source = source.expect("sprite sheet should queue a source rectangle");
        assert_close(source.x, 2.0);
        assert_close(source.y, 2.0);
        assert_close(source.w, 2.0);
        assert_close(source.h, 2.0);
        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn set_window_table_uses_cached_root_table_when_entity_root_key_is_invalid() -> mlua::Result<()>
    {
        let (mut runtime, root) = start_test_runtime("window_root_registry_recovery")?;

        let foreign_lua = Lua::new();
        let foreign_table = foreign_lua.create_table()?;
        let foreign_key = foreign_lua.create_registry_value(foreign_table)?;
        runtime
            .entities
            .borrow_mut()
            .get_mut(&0)
            .expect("root entity should exist")
            .luau_key = foreign_key;

        runtime.set_platform_window_state(320.0, 240.0);
        runtime.set_window_table()?;
        runtime.set_platform_window_state(640.0, 360.0);
        runtime.set_window_table()?;

        let ecs: Table = runtime.lua.globals().get("ecs")?;
        let root_table: Table = ecs.get("root")?;
        assert_eq!(root_table.get::<f32>("size_x")?, 640.0);
        assert_eq!(root_table.get::<f32>("size_y")?, 360.0);

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
            let mut platform = runtime
                .platform
                .lock()
                .expect("platform mutex should not be poisoned during test");
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
            let mut platform = runtime
                .platform
                .lock()
                .expect("platform mutex should not be poisoned during test");
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
    fn entity_listener_hit_test_and_local_pointer_follow_rotation() -> mlua::Result<()> {
        let (mut runtime, root) = start_test_runtime("rotated_entity_listener")?;

        let ecs: Table = runtime.lua.globals().get("ecs")?;
        let new_entity: Function = ecs.get("newEntity")?;
        let entity: Table = new_entity.call((
            "rotated button".to_string(),
            None::<Table>,
            Some(200.0),
            Some(100.0),
        ))?;
        entity.set("size_x", 120.0)?;
        entity.set("size_y", 80.0)?;
        entity.set("rotation", std::f32::consts::FRAC_PI_2)?;

        let received_local = Rc::new(RefCell::new(None::<(f32, f32)>));
        let received_local_writer = received_local.clone();
        let callback =
            runtime
                .lua
                .create_function(move |_lua, (_entity, event): (Table, Table)| {
                    *received_local_writer.borrow_mut() =
                        Some((event.get::<f32>("localX")?, event.get::<f32>("localY")?));
                    Ok(())
                })?;
        let listen: Function = entity.get("listen")?;
        let _connection: Table =
            listen.call((entity.clone(), "leftClick".to_string(), callback))?;

        // This is inside the visibly rotated entity (local 50, 40), but lies
        // left of the entity's original axis-aligned x range.
        runtime.set_platform_mouse_state(160.0, 150.0);
        runtime
            .platform
            .lock()
            .expect("platform mutex should not be poisoned during test")
            .input_mut()
            .mouse_pressed
            .insert("left".to_string());

        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        let (local_x, local_y) = received_local
            .borrow()
            .expect("rotated listener should receive the click");
        assert_close(local_x, 50.0);
        assert_close(local_y, 40.0);

        std::fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn entity_listener_dispatches_mouse_entered_and_exited_edges() -> mlua::Result<()> {
        let (mut runtime, root) = start_test_runtime("entity_listener_hover")?;

        let ecs: Table = runtime.lua.globals().get("ecs")?;
        let new_entity: Function = ecs.get("newEntity")?;
        let entity: Table =
            new_entity.call(("button".to_string(), None::<Table>, Some(20.0), Some(30.0)))?;
        entity.set("size_x", 120.0)?;
        entity.set("size_y", 80.0)?;

        let seen = Rc::new(RefCell::new(Vec::<String>::new()));
        let seen_writer = seen.clone();
        let callback =
            runtime
                .lua
                .create_function(move |_lua, (_entity, event): (Table, Table)| {
                    seen_writer.borrow_mut().push(event.get::<String>("kind")?);
                    Ok(())
                })?;

        let listen: Function = entity.get("listen")?;
        let _entered: Table =
            listen.call((entity.clone(), "MouseEntered".to_string(), callback.clone()))?;
        let _exited: Table = listen.call((entity.clone(), "MouseExited".to_string(), callback))?;

        runtime.set_platform_mouse_state(0.0, 0.0);
        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        assert!(seen.borrow().is_empty());

        runtime.set_platform_mouse_state(40.0, 50.0);
        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        assert_eq!(seen.borrow().as_slice(), ["mouseEntered"]);

        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        assert_eq!(seen.borrow().as_slice(), ["mouseEntered"]);

        runtime.set_platform_mouse_state(200.0, 200.0);
        runtime.update(1.0 / 60.0).map_err(mlua::Error::external)?;
        assert_eq!(seen.borrow().as_slice(), ["mouseEntered", "mouseExited"]);

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
