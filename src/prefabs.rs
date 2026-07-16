use crate::lua_error::protect_lua_call;
use crate::window::create_entity_table;
use mlua::{Function, Lua, String as LuaString, Table, Value};
use serde_json::Value as JsonValue;
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Default)]
struct CloneState {
    table_targets: HashMap<usize, Table>,
    entity_targets: HashSet<usize>,
    manual_targets: HashSet<usize>,
    filled: HashSet<usize>,
    in_progress: HashSet<usize>,
}

fn table_ptr(table: &Table) -> usize {
    table.to_pointer() as usize
}

fn is_entity_like(table: &Table) -> bool {
    matches!(table.raw_get::<Value>("children"), Ok(Value::Table(_)))
        && matches!(table.raw_get::<Value>("components"), Ok(Value::Table(_)))
}

fn is_reserved_entity_key(value: &Value) -> bool {
    matches!(
        value,
        Value::String(name)
            if matches!(
                name.to_str().ok().as_deref(),
                Some("id" | "parent" | "children" | "components")
            )
    )
}

fn clone_table_contents(lua: &Lua, source: &Table, target: &Table, state: &mut CloneState) -> mlua::Result<()> {
    for pair in source.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let cloned_key = clone_value_with_state(lua, key, state)?;
        let cloned_value = clone_value_with_state(lua, value, state)?;
        target.raw_set(cloned_key, cloned_value)?;
    }

    if let Some(metatable) = source.metatable() {
        let cloned = clone_table_value(lua, &metatable, state)?;
        target.set_metatable(Some(cloned))?;
    }

    if source.is_readonly() {
        target.set_readonly(true);
    }

    Ok(())
}

fn clone_table_value(lua: &Lua, source: &Table, state: &mut CloneState) -> mlua::Result<Table> {
    let ptr = table_ptr(source);

    if let Some(target) = state.table_targets.get(&ptr).cloned() {
        if state.entity_targets.contains(&ptr) || state.manual_targets.contains(&ptr) {
            return Ok(target);
        }

        if state.filled.contains(&ptr) || state.in_progress.contains(&ptr) {
            return Ok(target);
        }

        state.in_progress.insert(ptr);
        clone_table_contents(lua, source, &target, state)?;
        state.in_progress.remove(&ptr);
        state.filled.insert(ptr);
        return Ok(target);
    }

    if is_entity_like(source) {
        return Ok(source.clone());
    }

    let target = lua.create_table()?;
    state.table_targets.insert(ptr, target.clone());
    state.in_progress.insert(ptr);
    clone_table_contents(lua, source, &target, state)?;
    state.in_progress.remove(&ptr);
    state.filled.insert(ptr);
    Ok(target)
}

fn clone_value_with_state(lua: &Lua, value: Value, state: &mut CloneState) -> mlua::Result<Value> {
    match value {
        Value::Table(table) => Ok(Value::Table(clone_table_value(lua, &table, state)?)),
        other => Ok(other),
    }
}

fn collect_entity_tree(
    entity: &Table,
    visited: &mut HashSet<usize>,
    entities: &mut Vec<Table>,
) -> mlua::Result<()> {
    let ptr = table_ptr(entity);
    if !visited.insert(ptr) {
        return Ok(());
    }

    entities.push(entity.clone());

    if let Ok(children) = entity.raw_get::<Table>("children") {
        for child in children.sequence_values::<Table>() {
            collect_entity_tree(&child?, visited, entities)?;
        }
    }

    Ok(())
}

fn copy_entity_metadata(
    lua: &Lua,
    source: &Table,
    target: &Table,
    state: &mut CloneState,
) -> mlua::Result<()> {
    if let Some(metatable) = source.metatable() {
        let cloned = clone_table_value(lua, &metatable, state)?;
        target.set_metatable(Some(cloned))?;
    }

    if source.is_readonly() {
        target.set_readonly(true);
    }

    Ok(())
}

fn capture_entity_state(
    lua: &Lua,
    source: &Table,
    parent: Option<Table>,
    state: &mut CloneState,
    visited: &mut HashSet<usize>,
) -> mlua::Result<Table> {
    let ptr = table_ptr(source);
    let snapshot = state
        .table_targets
        .get(&ptr)
        .cloned()
        .ok_or_else(|| mlua::Error::external("missing prefab snapshot target"))?;

    if !visited.insert(ptr) {
        if let Some(parent) = parent {
            snapshot.raw_set("parent", parent)?;
        } else {
            snapshot.raw_set("parent", Value::Nil)?;
        }
        return Ok(snapshot);
    }

    for pair in source.pairs::<Value, Value>() {
        let (key, value) = pair?;
        if is_reserved_entity_key(&key) {
            continue;
        }
        let cloned_key = clone_value_with_state(lua, key, state)?;
        let cloned_value = clone_value_with_state(lua, value, state)?;
        snapshot.raw_set(cloned_key, cloned_value)?;
    }

    if let Some(parent) = parent {
        snapshot.raw_set("parent", parent)?;
    } else {
        snapshot.raw_set("parent", Value::Nil)?;
    }

    let snapshot_children = if let Ok(source_children) = source.raw_get::<Table>("children") {
        state
            .table_targets
            .get(&table_ptr(&source_children))
            .cloned()
            .unwrap_or(lua.create_table()?)
    } else {
        lua.create_table()?
    };
    snapshot.raw_set("children", snapshot_children.clone())?;

    let snapshot_components = if let Ok(source_components) = source.raw_get::<Table>("components") {
        state
            .table_targets
            .get(&table_ptr(&source_components))
            .cloned()
            .unwrap_or(lua.create_table()?)
    } else {
        lua.create_table()?
    };
    snapshot.raw_set("components", snapshot_components.clone())?;

    if let Ok(source_components) = source.raw_get::<Table>("components") {
        for component in source_components.sequence_values::<Table>() {
            let component = clone_table_value(lua, &component?, state)?;
            snapshot_components.push(component)?;
        }
    }

    if let Ok(source_children) = source.raw_get::<Table>("children") {
        for child in source_children.sequence_values::<Table>() {
            let child_snapshot =
                capture_entity_state(lua, &child?, Some(snapshot.clone()), state, visited)?;
            snapshot_children.push(child_snapshot)?;
        }
    }

    copy_entity_metadata(lua, source, &snapshot, state)?;
    Ok(snapshot)
}

pub(crate) fn capture_entity_tree_template(lua: &Lua, root: &Table) -> mlua::Result<Table> {
    let mut visited = HashSet::new();
    let mut entities = Vec::new();
    collect_entity_tree(root, &mut visited, &mut entities)?;

    let mut state = CloneState::default();
    for entity in entities {
        let ptr = table_ptr(&entity);
        state.table_targets.insert(ptr, lua.create_table()?);
        state.entity_targets.insert(ptr);

        if let Ok(children) = entity.raw_get::<Table>("children") {
            let ptr = table_ptr(&children);
            state.table_targets.insert(ptr, lua.create_table()?);
            state.manual_targets.insert(ptr);
        }

        if let Ok(components) = entity.raw_get::<Table>("components") {
            let ptr = table_ptr(&components);
            state.table_targets.insert(ptr, lua.create_table()?);
            state.manual_targets.insert(ptr);
        }
    }

    let mut filled = HashSet::new();
    capture_entity_state(lua, root, None, &mut state, &mut filled)
}

fn create_entity_shells_recursive(
    lua: &Lua,
    source: &Table,
    parent: Option<Table>,
    created: &mut HashMap<usize, Table>,
) -> mlua::Result<Table> {
    let ptr = table_ptr(source);
    if let Some(existing) = created.get(&ptr) {
        return Ok(existing.clone());
    }

    let ecs: Table = lua.globals().get("ecs")?;
    let new_entity: Function = ecs.get("newEntity")?;

    let name = source
        .raw_get::<String>("name")
        .unwrap_or_else(|_| "prefab".to_string());
    let x = source.raw_get::<f64>("x").ok();
    let y = source.raw_get::<f64>("y").ok();
    let entity: Table = new_entity.call((name, parent.clone(), x, y))?;
    created.insert(ptr, entity.clone());

    if let Ok(children) = source.raw_get::<Table>("children") {
        for child in children.sequence_values::<Table>() {
            create_entity_shells_recursive(lua, &child?, Some(entity.clone()), created)?;
        }
    }

    Ok(entity)
}

fn apply_entity_state_recursive(
    lua: &Lua,
    source: &Table,
    state: &mut CloneState,
    visited: &mut HashSet<usize>,
) -> mlua::Result<Table> {
    let ptr = table_ptr(source);
    let entity = state
        .table_targets
        .get(&ptr)
        .cloned()
        .ok_or_else(|| mlua::Error::external("missing instantiated entity target"))?;

    if !visited.insert(ptr) {
        return Ok(entity);
    }

    for pair in source.pairs::<Value, Value>() {
        let (key, value) = pair?;
        if is_reserved_entity_key(&key) {
            continue;
        }
        let cloned_key = clone_value_with_state(lua, key, state)?;
        let cloned_value = clone_value_with_state(lua, value, state)?;
        entity.raw_set(cloned_key, cloned_value)?;
    }

    let components: Table = entity.raw_get("components")?;
    let mut physics_component_count = 0i64;
    if let Ok(source_components) = source.raw_get::<Table>("components") {
        for component in source_components.sequence_values::<Table>() {
            let component = clone_table_value(lua, &component?, state)?;
            component.raw_set("entity", entity.clone())?;
            crate::window::attach_component_methods(lua, &component)?;
            if component
                .get::<String>("__neolove_component")
                .map(|name| crate::window::is_physics_component_name(&name))
                .unwrap_or(false)
            {
                physics_component_count += 1;
            }
            components.push(component)?;
        }
    }
    entity.raw_set("__neolove_physics_component_count", physics_component_count)?;
    entity.raw_set("__neolove_has_physics_components", physics_component_count > 0)?;

    if let Ok(source_children) = source.raw_get::<Table>("children") {
        for child in source_children.sequence_values::<Table>() {
            apply_entity_state_recursive(lua, &child?, state, visited)?;
        }
    }

    copy_entity_metadata(lua, source, &entity, state)?;
    Ok(entity)
}

fn awake_instantiated_components(root: &Table) -> mlua::Result<()> {
    let mut entities = Vec::new();
    let mut visited = HashSet::new();
    collect_entity_tree(root, &mut visited, &mut entities)?;

    // Snapshot the prefab-authored components before callbacks run. Components
    // created by an awake callback are initialized by ecs.addComponent itself.
    let mut callbacks = Vec::<(Table, Table, Function, String, Vec<(Value, Value)>)>::new();
    for entity in entities {
        let entity_name = entity
            .get::<String>("name")
            .unwrap_or_else(|_| "unnamed".to_string());
        if let Ok(components) = entity.raw_get::<Table>("components") {
            for component in components.sequence_values::<Table>() {
                let component = component?;
                let Ok(awake) = component.get::<Function>("awake") else {
                    continue;
                };
                let component_name = component
                    .get::<String>("__neolove_component")
                    .or_else(|_| component.get::<String>("name"))
                    .unwrap_or_else(|_| "script".to_string());
                let serialized_state = component
                    .clone()
                    .pairs::<Value, Value>()
                    .collect::<mlua::Result<Vec<_>>>()?;
                callbacks.push((
                    entity.clone(),
                    component,
                    awake,
                    format!("component '{component_name}' on entity '{entity_name}'"),
                    serialized_state,
                ));
            }
        }
    }

    for (entity, component, awake, description, serialized_state) in callbacks {
        protect_lua_call(
            &format!("running prefab component awake callback ({description})"),
            || awake.call::<()>((entity, component.clone())),
        )?;
        // Core awake callbacks install defaults, while script callbacks may
        // initialize runtime-only fields and listeners. Keep those side effects
        // but restore the prefab-authored state so defaults cannot erase image,
        // layout, color, or Inspector values.
        for (key, value) in serialized_state {
            component.raw_set(key, value)?;
        }
    }
    Ok(())
}

pub(crate) fn instantiate_entity_tree_from_source(
    lua: &Lua,
    source: &Table,
    parent: Option<Table>,
) -> mlua::Result<Table> {
    let mut created = HashMap::new();
    let root = create_entity_shells_recursive(lua, source, parent, &mut created)?;

    let mut state = CloneState::default();
    for (ptr, entity) in created {
        state.table_targets.insert(ptr, entity);
        state.entity_targets.insert(ptr);
    }

    let mut source_entities = Vec::new();
    let mut source_visited = HashSet::new();
    collect_entity_tree(source, &mut source_visited, &mut source_entities)?;
    for source_entity in source_entities {
        let target_entity = state
            .table_targets
            .get(&table_ptr(&source_entity))
            .cloned()
            .ok_or_else(|| mlua::Error::external("missing instantiated entity"))?;

        if let Ok(source_children) = source_entity.raw_get::<Table>("children") {
            let target_children: Table = target_entity.raw_get("children")?;
            let ptr = table_ptr(&source_children);
            state.table_targets.insert(ptr, target_children);
            state.manual_targets.insert(ptr);
        }

        if let Ok(source_components) = source_entity.raw_get::<Table>("components") {
            let target_components: Table = target_entity.raw_get("components")?;
            let ptr = table_ptr(&source_components);
            state.table_targets.insert(ptr, target_components);
            state.manual_targets.insert(ptr);
        }
    }

    let mut visited = HashSet::new();
    apply_entity_state_recursive(lua, source, &mut state, &mut visited)?;
    awake_instantiated_components(&root)?;
    Ok(root)
}

fn clone_overrides_into(lua: &Lua, target: &Table, overrides: &Table) -> mlua::Result<()> {
    let mut state = CloneState::default();
    for pair in overrides.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let cloned_key = clone_value_with_state(lua, key, &mut state)?;
        let cloned_value = clone_value_with_state(lua, value, &mut state)?;
        target.raw_set(cloned_key, cloned_value)?;
    }
    Ok(())
}

fn build_component_template(
    lua: &Lua,
    source: &Table,
    overrides: Option<Table>,
) -> mlua::Result<Table> {
    let mut state = CloneState::default();
    let component = clone_table_value(lua, source, &mut state)?;

    if matches!(component.raw_get::<Value>("entity"), Ok(Value::Nil))
        && component.get::<Function>("awake").is_ok()
    {
        let scratch = create_entity_table(lua, "__prefab_component__", 0.0, 0.0, None)?;
        let awake: Function = component.get("awake")?;
        let component_name = component
            .get::<String>("__neolove_component")
            .unwrap_or_else(|_| "prefab component".to_string());
        protect_lua_call(
            &format!("building prefab component template ({component_name})"),
            || awake.call::<()>((scratch.clone(), component.clone())),
        )?;
        component.raw_set("entity", Value::Nil)?;
    }

    if let Some(overrides) = overrides {
        clone_overrides_into(lua, &component, &overrides)?;
    }

    Ok(component)
}

fn color4(lua: &Lua, r: u8, g: u8, b: u8, a: u8) -> mlua::Result<Table> {
    let color = lua.create_table()?;
    color.set("r", r)?;
    color.set("g", g)?;
    color.set("b", b)?;
    color.set("a", a)?;
    Ok(color)
}

fn core_component(lua: &Lua, name: &str) -> mlua::Result<Table> {
    let core: Table = lua.globals().get("core")?;
    core.get(name)
}

fn add_component_template(entity: &Table, component: Table) -> mlua::Result<()> {
    let components: Table = entity.raw_get("components")?;
    components.push(component)?;
    Ok(())
}

fn build_ui_label(lua: &Lua) -> mlua::Result<Table> {
    let root = create_entity_table(lua, "ui_label", 0.0, 0.0, None)?;
    root.set("size_x", 220.0)?;
    root.set("size_y", 40.0)?;

    let text = build_component_template(lua, &core_component(lua, "TextBox")?, None)?;
    text.set("text", "Label")?;
    text.set("size_mode", "entity")?;
    text.set("scale", 22.0)?;
    text.set("align_x", "left")?;
    text.set("align_y", "center")?;
    text.set("padding_x", 12.0)?;
    text.set("padding_y", 6.0)?;
    text.set("text_scale", "fit_height")?;
    text.set("color", color4(lua, 241, 245, 249, 255)?)?;
    add_component_template(&root, text)?;

    Ok(root)
}

fn build_ui_panel(lua: &Lua) -> mlua::Result<Table> {
    let root = create_entity_table(lua, "ui_panel", 0.0, 0.0, None)?;
    root.set("size_x", 280.0)?;
    root.set("size_y", 156.0)?;

    let background = build_component_template(lua, &core_component(lua, "Shape2D")?, None)?;
    background.set("shape", "box")?;
    background.set("color", color4(lua, 20, 28, 38, 236)?)?;
    add_component_template(&root, background)?;

    let accent = create_entity_table(lua, "accent", 0.0, 0.0, Some(root.clone()))?;
    accent.set("size_x", 280.0)?;
    accent.set("size_y", 8.0)?;
    let accent_shape = build_component_template(lua, &core_component(lua, "Shape2D")?, None)?;
    accent_shape.set("shape", "box")?;
    accent_shape.set("color", color4(lua, 56, 189, 248, 255)?)?;
    add_component_template(&accent, accent_shape)?;

    let title = create_entity_table(lua, "title", 18.0, 18.0, Some(root.clone()))?;
    title.set("size_x", 244.0)?;
    title.set("size_y", 26.0)?;
    let title_text = build_component_template(lua, &core_component(lua, "TextBox")?, None)?;
    title_text.set("text", "Panel Title")?;
    title_text.set("size_mode", "entity")?;
    title_text.set("scale", 22.0)?;
    title_text.set("align_x", "left")?;
    title_text.set("align_y", "center")?;
    title_text.set("text_scale", "fit_height")?;
    title_text.set("color", color4(lua, 248, 250, 252, 255)?)?;
    add_component_template(&title, title_text)?;

    let body = create_entity_table(lua, "body", 18.0, 54.0, Some(root))?;
    body.set("size_x", 244.0)?;
    body.set("size_y", 78.0)?;
    let body_text = build_component_template(lua, &core_component(lua, "TextBox")?, None)?;
    body_text.set("text", "Prefab-backed panel body copy lives here.")?;
    body_text.set("size_mode", "entity")?;
    body_text.set("scale", 17.0)?;
    body_text.set("min_scale", 12.0)?;
    body_text.set("align_x", "left")?;
    body_text.set("align_y", "top")?;
    body_text.set("wrap", "word")?;
    body_text.set("padding_x", 0.0)?;
    body_text.set("padding_y", 0.0)?;
    body_text.set("line_spacing", 1.1)?;
    body_text.set("color", color4(lua, 191, 219, 254, 255)?)?;
    add_component_template(&body, body_text)?;

    Ok(body.get::<Table>("parent")?)
}

fn build_ui_dialog(lua: &Lua) -> mlua::Result<Table> {
    let root = create_entity_table(lua, "ui_dialog", 0.0, 0.0, None)?;
    root.set("size_x", 360.0)?;
    root.set("size_y", 220.0)?;

    let background = build_component_template(lua, &core_component(lua, "Shape2D")?, None)?;
    background.set("shape", "box")?;
    background.set("color", color4(lua, 8, 15, 27, 244)?)?;
    add_component_template(&root, background)?;

    let header = create_entity_table(lua, "header", 0.0, 0.0, Some(root.clone()))?;
    header.set("size_x", 360.0)?;
    header.set("size_y", 54.0)?;
    let header_shape = build_component_template(lua, &core_component(lua, "Shape2D")?, None)?;
    header_shape.set("shape", "box")?;
    header_shape.set("color", color4(lua, 30, 41, 59, 255)?)?;
    add_component_template(&header, header_shape)?;

    let title = create_entity_table(lua, "title", 20.0, 14.0, Some(root.clone()))?;
    title.set("size_x", 320.0)?;
    title.set("size_y", 28.0)?;
    let title_text = build_component_template(lua, &core_component(lua, "TextBox")?, None)?;
    title_text.set("text", "Dialog Title")?;
    title_text.set("size_mode", "entity")?;
    title_text.set("scale", 24.0)?;
    title_text.set("text_scale", "fit_height")?;
    title_text.set("align_x", "left")?;
    title_text.set("align_y", "center")?;
    title_text.set("color", color4(lua, 248, 250, 252, 255)?)?;
    add_component_template(&title, title_text)?;

    let body = create_entity_table(lua, "body", 20.0, 68.0, Some(root.clone()))?;
    body.set("size_x", 320.0)?;
    body.set("size_y", 92.0)?;
    let body_text = build_component_template(lua, &core_component(lua, "TextBox")?, None)?;
    body_text.set("text", "Dialogs can be assembled as prefab trees with exact component state preserved.")?;
    body_text.set("size_mode", "entity")?;
    body_text.set("scale", 18.0)?;
    body_text.set("min_scale", 12.0)?;
    body_text.set("wrap", "word")?;
    body_text.set("align_x", "left")?;
    body_text.set("align_y", "top")?;
    body_text.set("line_spacing", 1.15)?;
    body_text.set("color", color4(lua, 203, 213, 225, 255)?)?;
    add_component_template(&body, body_text)?;

    let footer = create_entity_table(lua, "footer", 20.0, 176.0, Some(root.clone()))?;
    footer.set("size_x", 320.0)?;
    footer.set("size_y", 22.0)?;
    let footer_text = build_component_template(lua, &core_component(lua, "TextBox")?, None)?;
    footer_text.set("text", "Press enter to continue")?;
    footer_text.set("size_mode", "entity")?;
    footer_text.set("scale", 15.0)?;
    footer_text.set("align_x", "right")?;
    footer_text.set("align_y", "center")?;
    footer_text.set("text_scale", "fit_height")?;
    footer_text.set("color", color4(lua, 125, 211, 252, 255)?)?;
    add_component_template(&footer, footer_text)?;

    Ok(root)
}

fn build_ui_status_chip(lua: &Lua) -> mlua::Result<Table> {
    let root = create_entity_table(lua, "ui_status_chip", 0.0, 0.0, None)?;
    root.set("size_x", 180.0)?;
    root.set("size_y", 42.0)?;

    let background = build_component_template(lua, &core_component(lua, "Shape2D")?, None)?;
    background.set("shape", "box")?;
    background.set("color", color4(lua, 22, 101, 52, 224)?)?;
    add_component_template(&root, background)?;

    let dot = create_entity_table(lua, "dot", 12.0, 11.0, Some(root.clone()))?;
    dot.set("size_x", 20.0)?;
    dot.set("size_y", 20.0)?;
    let dot_shape = build_component_template(lua, &core_component(lua, "Shape2D")?, None)?;
    dot_shape.set("shape", "circle")?;
    dot_shape.set("color", color4(lua, 134, 239, 172, 255)?)?;
    add_component_template(&dot, dot_shape)?;

    let text = create_entity_table(lua, "text", 42.0, 0.0, Some(root.clone()))?;
    text.set("size_x", 124.0)?;
    text.set("size_y", 42.0)?;
    let label = build_component_template(lua, &core_component(lua, "TextBox")?, None)?;
    label.set("text", "SYSTEM ONLINE")?;
    label.set("size_mode", "entity")?;
    label.set("scale", 16.0)?;
    label.set("align_x", "left")?;
    label.set("align_y", "center")?;
    label.set("text_scale", "fit_height")?;
    label.set("color", color4(lua, 240, 253, 244, 255)?)?;
    add_component_template(&text, label)?;

    Ok(root)
}

fn prefab_json_error(path: &str, message: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::external(format!("failed to load prefab '{path}': {message}"))
}

fn normalize_require_path(path: &str) -> String {
    let mut path = path.replace('\\', "/");
    if path.ends_with(".luau") {
        path.truncate(path.len() - ".luau".len());
    } else if path.ends_with(".lua") {
        path.truncate(path.len() - ".lua".len());
    }
    if path.starts_with("./") || path.starts_with("../") || path.starts_with('@') {
        path
    } else {
        format!("./{path}")
    }
}

fn json_string<'a>(value: &'a JsonValue, field: &str, path: &str) -> mlua::Result<&'a str> {
    value
        .get(field)
        .and_then(JsonValue::as_str)
        .ok_or_else(|| prefab_json_error(path, format!("missing or invalid '{field}'")))
}

fn json_number(value: &JsonValue, field: &str, default: f64) -> f64 {
    value.get(field).and_then(JsonValue::as_f64).unwrap_or(default)
}

fn editor_value_to_lua(lua: &Lua, value: &JsonValue, path: &str) -> mlua::Result<Option<Value>> {
    let kind = json_string(value, "t", path)?;
    let payload = value.get("v").unwrap_or(&JsonValue::Null);
    match kind {
        "Number" => payload
            .as_f64()
            .map(|value| Some(Value::Number(value)))
            .ok_or_else(|| prefab_json_error(path, "invalid number component property")),
        "Int" => payload
            .as_i64()
            .ok_or_else(|| prefab_json_error(path, "invalid integer component property"))
            .and_then(|value| {
                mlua::Integer::try_from(value)
                    .map(|value| Some(Value::Integer(value)))
                    .map_err(|_| {
                        prefab_json_error(path, "integer component property is out of range")
                    })
            }),
        "Bool" => payload
            .as_bool()
            .map(|value| Some(Value::Boolean(value)))
            .ok_or_else(|| prefab_json_error(path, "invalid boolean component property")),
        "Text" => {
            let value = payload
                .as_str()
                .ok_or_else(|| prefab_json_error(path, "invalid text component property"))?;
            Ok(Some(Value::String(lua.create_string(value)?)))
        }
        "Color" => {
            let channels = payload
                .as_array()
                .ok_or_else(|| prefab_json_error(path, "invalid color component property"))?;
            if channels.len() != 4 {
                return Err(prefab_json_error(path, "color component property must have four channels"));
            }
            let channel = |index: usize| {
                channels[index]
                    .as_u64()
                    .and_then(|value| u8::try_from(value).ok())
                    .ok_or_else(|| prefab_json_error(path, "color channel must be between 0 and 255"))
            };
            Ok(Some(Value::Table(color4(
                lua,
                channel(0)?,
                channel(1)?,
                channel(2)?,
                channel(3)?,
            )?)))
        }
        "Enum" => {
            let value = payload
                .get("value")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| prefab_json_error(path, "invalid enum component property"))?;
            Ok(Some(Value::String(lua.create_string(value)?)))
        }
        "Image" => {
            let image_path = payload
                .as_str()
                .ok_or_else(|| prefab_json_error(path, "invalid image component property"))?;
            if image_path.is_empty() {
                return Ok(None);
            }
            let assets: Table = lua.globals().get("assets")?;
            let load_image: Function = assets.get("loadImage")?;
            load_image.call::<Value>(image_path).map(Some)
        }
        other => Err(prefab_json_error(
            path,
            format!("unsupported component property type '{other}'"),
        )),
    }
}

fn editor_variable_to_lua(
    lua: &Lua,
    value: &JsonValue,
    path: &str,
    entities: &HashMap<u64, Table>,
    components: &HashMap<(u64, usize), Table>,
) -> mlua::Result<Value> {
    let kind = json_string(value, "type", path)?;
    let payload = value.get("value").unwrap_or(&JsonValue::Null);
    match kind {
        "Number" => payload
            .as_f64()
            .map(Value::Number)
            .ok_or_else(|| prefab_json_error(path, "invalid number script variable")),
        "Bool" => payload
            .as_bool()
            .map(Value::Boolean)
            .ok_or_else(|| prefab_json_error(path, "invalid boolean script variable")),
        "Text" => {
            let value = payload
                .as_str()
                .ok_or_else(|| prefab_json_error(path, "invalid text script variable"))?;
            Ok(Value::String(lua.create_string(value)?))
        }
        "Color" => {
            let channels = payload
                .as_array()
                .ok_or_else(|| prefab_json_error(path, "invalid color script variable"))?;
            if channels.len() != 4 {
                return Err(prefab_json_error(path, "color script variable must have four channels"));
            }
            let channel = |index: usize| {
                channels[index]
                    .as_u64()
                    .and_then(|value| u8::try_from(value).ok())
                    .ok_or_else(|| prefab_json_error(path, "color channel must be between 0 and 255"))
            };
            Ok(Value::Table(color4(
                lua,
                channel(0)?,
                channel(1)?,
                channel(2)?,
                channel(3)?,
            )?))
        }
        "Image" => {
            let asset_path = payload
                .as_str()
                .ok_or_else(|| prefab_json_error(path, "invalid image variable"))?;
            if asset_path.is_empty() {
                return Ok(Value::Nil);
            }
            let assets: Table = lua.globals().get("assets")?;
            assets.get::<Function>("loadImage")?.call(asset_path)
        }
        "Audio" => {
            let asset_path = payload
                .as_str()
                .ok_or_else(|| prefab_json_error(path, "invalid sound variable"))?;
            if asset_path.is_empty() {
                return Ok(Value::Nil);
            }
            let assets: Table = lua.globals().get("assets")?;
            assets.get::<Function>("loadSound")?.call(asset_path)
        }
        "Shader" => {
            let asset_path = payload
                .as_str()
                .ok_or_else(|| prefab_json_error(path, "invalid shader variable"))?;
            if asset_path.is_empty() {
                return Ok(Value::Nil);
            }
            let shaders: Table = lua.globals().get("shaders")?;
            shaders.get::<Function>("loadFragment")?.call(asset_path)
        }
        "Animation" => {
            let asset_path = payload
                .as_str()
                .ok_or_else(|| prefab_json_error(path, "invalid animation variable"))?;
            if asset_path.is_empty() {
                return Ok(Value::Nil);
            }
            let animation: Table = lua.globals().get("animation")?;
            animation.get::<Function>("load")?.call(asset_path)
        }
        "Entity" => Ok(payload
            .as_u64()
            .and_then(|id| entities.get(&id).cloned())
            .map(Value::Table)
            .unwrap_or(Value::Nil)),
        "Component" => {
            let reference = payload.as_object().and_then(|reference| {
                let entity = reference.get("entity")?.as_u64()?;
                let component = usize::try_from(reference.get("component")?.as_u64()?).ok()?;
                Some((entity, component))
            });
            Ok(reference
                .and_then(|reference| components.get(&reference).cloned())
                .map(Value::Table)
                .unwrap_or(Value::Nil))
        }
        "List" => {
            let values = payload
                .as_array()
                .ok_or_else(|| prefab_json_error(path, "invalid list script variable"))?;
            let table = lua.create_table()?;
            for value in values {
                table.push(editor_variable_to_lua(
                    lua,
                    value,
                    path,
                    entities,
                    components,
                )?)?;
            }
            Ok(Value::Table(table))
        }
        "Dictionary" => {
            let entries = payload
                .as_array()
                .ok_or_else(|| prefab_json_error(path, "invalid dictionary script variable"))?;
            let table = lua.create_table()?;
            for entry in entries {
                let key = entry
                    .get("key")
                    .ok_or_else(|| prefab_json_error(path, "dictionary entry is missing 'key'"))?;
                let value = entry
                    .get("value")
                    .ok_or_else(|| prefab_json_error(path, "dictionary entry is missing 'value'"))?;
                table.raw_set(
                    editor_variable_key_to_lua(lua, key, path)?,
                    editor_variable_to_lua(lua, value, path, entities, components)?,
                )?;
            }
            Ok(Value::Table(table))
        }
        other => Err(prefab_json_error(
            path,
            format!("unsupported script variable type '{other}'"),
        )),
    }
}

fn editor_variable_key_to_lua(lua: &Lua, key: &JsonValue, path: &str) -> mlua::Result<Value> {
    let kind = json_string(key, "type", path)?;
    let value = key.get("value").unwrap_or(&JsonValue::Null);
    match kind {
        "Number" => value
            .as_f64()
            .map(Value::Number)
            .ok_or_else(|| prefab_json_error(path, "invalid numeric dictionary key")),
        "Bool" => value
            .as_bool()
            .map(Value::Boolean)
            .ok_or_else(|| prefab_json_error(path, "invalid boolean dictionary key")),
        "Text" => value
            .as_str()
            .ok_or_else(|| prefab_json_error(path, "invalid text dictionary key"))
            .and_then(|value| lua.create_string(value).map(Value::String)),
        other => Err(prefab_json_error(
            path,
            format!("unsupported dictionary key type '{other}'"),
        )),
    }
}

fn load_editor_component(
    lua: &Lua,
    source: &JsonValue,
    path: &str,
    project_require: &Function,
) -> mlua::Result<Table> {
    match json_string(source, "kind", path)? {
        "Core" => {
            let name = json_string(source, "name", path)?;
            let component = build_component_template(lua, &core_component(lua, name)?, None)?;
            let props = source
                .get("props")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| prefab_json_error(path, "core component is missing 'props'"))?;
            for prop in props {
                let name = json_string(prop, "name", path)?;
                let optional = prop.get("optional").and_then(JsonValue::as_bool).unwrap_or(false);
                let value = prop
                    .get("value")
                    .ok_or_else(|| prefab_json_error(path, "component property is missing 'value'"))?;
                if optional
                    && matches!(
                        value.get("v").and_then(JsonValue::as_str),
                        Some("")
                    )
                {
                    continue;
                }
                if let Some(value) = editor_value_to_lua(lua, value, path)? {
                    component.raw_set(name, value)?;
                }
            }
            Ok(component)
        }
        "Script" => {
            let module_path = json_string(source, "path", path)?;
            if module_path.is_empty() {
                return Err(prefab_json_error(path, "script component has no module path"));
            }
            let prototype: Table = project_require.call(normalize_require_path(module_path))?;
            let mut state = CloneState::default();
            clone_table_value(lua, &prototype, &mut state)
        }
        other => Err(prefab_json_error(
            path,
            format!("unsupported component kind '{other}'"),
        )),
    }
}

fn load_neoprefab(
    lua: &Lua,
    bytes: &[u8],
    path: &str,
    project_require: &Function,
) -> mlua::Result<Table> {
    let entities = crate::scene::prefab_from_bytes(bytes)
        .map_err(|error| prefab_json_error(path, error))?;
    let document =
        serde_json::to_value(&entities).map_err(|error| prefab_json_error(path, error))?;
    let entities = document
        .as_array()
        .ok_or_else(|| prefab_json_error(path, "root JSON value must be an entity array"))?;
    if entities.is_empty() {
        return Err(prefab_json_error(path, "prefab contains no entities"));
    }

    let mut templates = HashMap::<u64, Table>::new();
    let mut component_templates = HashMap::<(u64, usize), Table>::new();
    let mut parent_ids = Vec::<(u64, Option<u64>)>::new();
    for source in entities {
        let id = source
            .get("id")
            .and_then(JsonValue::as_u64)
            .ok_or_else(|| prefab_json_error(path, "entity is missing a valid 'id'"))?;
        if templates.contains_key(&id) {
            return Err(prefab_json_error(path, format!("duplicate entity id {id}")));
        }
        let name = json_string(source, "name", path)?;
        let template = lua.create_table()?;
        template.set("name", name)?;
        template.set("x", json_number(source, "x", 0.0))?;
        template.set("y", json_number(source, "y", 0.0))?;
        template.set("z", json_number(source, "z", 0.0))?;
        template.set("size_x", json_number(source, "size_x", 100.0))?;
        template.set("size_y", json_number(source, "size_y", 100.0))?;
        template.set("rotation", json_number(source, "rotation", 0.0))?;
        template.set("scale", json_number(source, "scale", 1.0))?;
        template.set("anchor_x", json_number(source, "anchor_x", 0.0))?;
        template.set("anchor_y", json_number(source, "anchor_y", 0.0))?;
        if let Some(position_pivot) = source
            .get("position_pivot")
            .and_then(JsonValue::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            template.set("position_pivot", position_pivot)?;
        }
        if let Some(pivot_x) = source
            .get("pivot_x")
            .and_then(JsonValue::as_f64)
            .filter(|value| value.is_finite())
        {
            template.set("pivot_x", pivot_x)?;
        }
        if let Some(pivot_y) = source
            .get("pivot_y")
            .and_then(JsonValue::as_f64)
            .filter(|value| value.is_finite())
        {
            template.set("pivot_y", pivot_y)?;
        }
        if let Some(rotation_pivot) = source
            .get("rotation_pivot")
            .and_then(JsonValue::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            template.set("rotation_pivot", rotation_pivot)?;
        }
        if let Some(pivot_x) = source
            .get("rotation_pivot_x")
            .and_then(JsonValue::as_f64)
            .filter(|value| value.is_finite())
        {
            template.set("rotation_pivot_x", pivot_x)?;
        }
        if let Some(pivot_y) = source
            .get("rotation_pivot_y")
            .and_then(JsonValue::as_f64)
            .filter(|value| value.is_finite())
        {
            template.set("rotation_pivot_y", pivot_y)?;
        }
        template.set(
            "enabled",
            source.get("enabled").and_then(JsonValue::as_bool).unwrap_or(true),
        )?;
        template.set("children", lua.create_table()?)?;
        let components = lua.create_table()?;
        for (component_index, component) in source
            .get("components")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| prefab_json_error(path, "entity is missing 'components'"))?
            .iter()
            .enumerate()
        {
            let component = load_editor_component(lua, component, path, project_require)?;
            components.push(component.clone())?;
            component_templates.insert((id, component_index), component);
        }
        template.set("components", components)?;

        let parent = match source.get("parent") {
            Some(JsonValue::Number(value)) => value.as_u64(),
            _ => None,
        };
        templates.insert(id, template);
        parent_ids.push((id, parent));
    }

    // References are assigned after every entity and component template exists,
    // allowing script fields to point forward within the prefab subtree.
    for source in entities {
        let id = source
            .get("id")
            .and_then(JsonValue::as_u64)
            .ok_or_else(|| prefab_json_error(path, "entity is missing a valid 'id'"))?;
        let entity = templates
            .get(&id)
            .ok_or_else(|| prefab_json_error(path, "missing entity template"))?;
        if let Some(values) = source.get("values").and_then(JsonValue::as_array) {
            for attached in values {
                let name = json_string(attached, "name", path)?;
                if name.is_empty() {
                    continue;
                }
                let value = attached
                    .get("value")
                    .ok_or_else(|| prefab_json_error(path, "attached value is missing 'value'"))?;
                entity.raw_set(
                    name,
                    editor_variable_to_lua(
                        lua,
                        value,
                        path,
                        &templates,
                        &component_templates,
                    )?,
                )?;
            }
        }
        for (component_index, component_source) in source
            .get("components")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| prefab_json_error(path, "entity is missing 'components'"))?
            .iter()
            .enumerate()
        {
            if !matches!(
                component_source.get("kind").and_then(JsonValue::as_str),
                Some("Script")
            ) {
                continue;
            }
            let component = component_templates
                .get(&(id, component_index))
                .ok_or_else(|| prefab_json_error(path, "missing script component template"))?;
            let variables = component_source
                .get("variables")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| prefab_json_error(path, "script component is missing 'variables'"))?;
            for variable in variables {
                let name = json_string(variable, "name", path)?;
                if name.is_empty() {
                    continue;
                }
                let value = variable
                    .get("value")
                    .ok_or_else(|| prefab_json_error(path, "script variable is missing 'value'"))?;
                component.raw_set(
                    name,
                    editor_variable_to_lua(
                        lua,
                        value,
                        path,
                        &templates,
                        &component_templates,
                    )?,
                )?;
            }
        }
    }

    let mut roots = Vec::new();
    for (id, parent_id) in parent_ids {
        let template = templates
            .get(&id)
            .cloned()
            .ok_or_else(|| prefab_json_error(path, format!("missing entity {id}")))?;
        if let Some(parent) = parent_id.and_then(|parent_id| templates.get(&parent_id).cloned()) {
            template.set("parent", parent.clone())?;
            parent.get::<Table>("children")?.push(template)?;
        } else {
            template.set("parent", Value::Nil)?;
            roots.push(template);
        }
    }

    if roots.len() != 1 {
        return Err(prefab_json_error(
            path,
            format!("prefab must contain exactly one root entity, found {}", roots.len()),
        ));
    }
    Ok(roots.remove(0))
}

fn resolve_source(registry: &Table, value: Value) -> mlua::Result<Table> {
    match value {
        Value::String(name) => {
            let name = name.to_str()?.to_string();
            match registry.raw_get::<Value>(name.as_str())? {
                Value::Table(table) => Ok(table),
                _ => Err(mlua::Error::external(format!(
                    "prefab '{name}' is not registered"
                ))),
            }
        }
        Value::Table(table) => Ok(table),
        other => Err(mlua::Error::external(format!(
            "prefab source must be a table or name, got {}",
            other.type_name()
        ))),
    }
}

pub(crate) fn add_prefab_module(lua: &Lua, project_root: &Path) -> mlua::Result<()> {
    let module = lua.create_table()?;
    let registry = lua.create_table()?;

    // Calls to `require` normally resolve relative to the Luau caller. Prefab
    // component paths are editor-owned project-relative paths, so give them a
    // stable caller anchored beside the project's main module.
    let project_require: Function = lua
        .load("return function(path) return require(path) end")
        .set_name(format!("@{}", project_root.join("main").display()))
        .eval()?;

    let capture = lua.create_function(move |lua, entity: Table| capture_entity_tree_template(lua, &entity))?;
    module.set("capture", capture)?;

    let component = lua.create_function(move |lua, (source, overrides): (Table, Option<Table>)| {
        build_component_template(lua, &source, overrides)
    })?;
    module.set("component", component)?;

    let load_project_require = project_require.clone();
    let load = lua.create_function(move |lua, path: String| {
        let fs: Table = lua.globals().get("fs")?;
        let read_bytes: Function = fs.get("readBytes")?;
        let bytes: LuaString = read_bytes.call(path.as_str())?;
        load_neoprefab(lua, bytes.as_bytes().as_ref(), &path, &load_project_require)
    })?;
    module.set("load", load)?;

    let registry_register = registry.clone();
    let register = lua.create_function(move |lua, (name, source): (String, Value)| {
        let source = resolve_source(&registry_register, source)?;
        let captured = capture_entity_tree_template(lua, &source)?;
        registry_register.raw_set(name.clone(), captured.clone())?;
        Ok(captured)
    })?;
    module.set("register", register)?;

    let registry_get = registry.clone();
    let get = lua.create_function(move |_lua, name: String| {
        match registry_get.raw_get::<Value>(name.as_str())? {
            Value::Table(table) => Ok(Some(table)),
            _ => Ok(None),
        }
    })?;
    module.set("get", get)?;

    let registry_remove = registry.clone();
    let remove = lua.create_function(move |_lua, name: String| {
        let existed = !matches!(registry_remove.raw_get::<Value>(name.as_str())?, Value::Nil);
        if existed {
            registry_remove.raw_set(name.as_str(), Value::Nil)?;
        }
        Ok(existed)
    })?;
    module.set("remove", remove)?;

    let registry_instantiate = registry.clone();
    let instantiate = lua.create_function(move |lua, (source, parent): (Value, Option<Table>)| {
        let source = resolve_source(&registry_instantiate, source)?;
        instantiate_entity_tree_from_source(lua, &source, parent)
    })?;
    module.set("instantiate", instantiate.clone())?;
    module.set("duplicate", instantiate)?;

    let ui = lua.create_table()?;
    let label = build_ui_label(lua)?;
    let panel = build_ui_panel(lua)?;
    let dialog = build_ui_dialog(lua)?;
    let status_chip = build_ui_status_chip(lua)?;
    ui.set("label", label.clone())?;
    ui.set("panel", panel.clone())?;
    ui.set("dialog", dialog.clone())?;
    ui.set("statusChip", status_chip.clone())?;
    ui.set("status_chip", status_chip.clone())?;
    module.set("ui", ui)?;

    registry.raw_set("ui.label", label)?;
    registry.raw_set("ui.panel", panel)?;
    registry.raw_set("ui.dialog", dialog)?;
    registry.raw_set("ui.statusChip", status_chip.clone())?;
    registry.raw_set("ui.status_chip", status_chip)?;

    module.set("_registry", registry)?;

    lua.globals().set("prefabs", module.clone())?;
    lua.globals().set("prefab", module)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install_mock_ecs(lua: &Lua) -> mlua::Result<()> {
        let ecs = lua.create_table()?;
        let next_id = std::rc::Rc::new(std::cell::RefCell::new(0usize));
        let next_id_new = next_id.clone();
        ecs.set(
            "newEntity",
            lua.create_function(
                move |lua, (name, parent, x, y): (String, Option<Table>, Option<f64>, Option<f64>)| {
                    let entity =
                        create_entity_table(lua, &name, x.unwrap_or(0.0), y.unwrap_or(0.0), parent)?;
                    *next_id_new.borrow_mut() += 1;
                    entity.set("id", *next_id_new.borrow())?;
                    Ok(entity)
                },
            )?,
        )?;
        lua.globals().set("ecs", ecs)?;
        Ok(())
    }

    #[test]
    fn instantiate_remaps_internal_entity_refs_and_shared_tables() -> mlua::Result<()> {
        let lua = Lua::new();
        install_mock_ecs(&lua)?;

        let external = create_entity_table(&lua, "external", 0.0, 0.0, None)?;
        external.set("id", 999)?;

        let root = create_entity_table(&lua, "root", 10.0, 20.0, None)?;
        root.set("id", 1)?;
        root.set("size_x", 200.0)?;
        root.set("size_y", 120.0)?;

        let child = create_entity_table(&lua, "child", 8.0, 12.0, Some(root.clone()))?;
        child.set("id", 2)?;

        let shared = lua.create_table()?;
        shared.set("value", 42)?;
        let mt = lua.create_table()?;
        mt.set("__name", "shared_mt")?;
        shared.set_metatable(Some(mt))?;

        root.set("shared", shared.clone())?;
        root.set("linkedChild", child.clone())?;

        let components: Table = root.get("components")?;
        let children: Table = root.get("children")?;
        root.set("childrenRef", children.clone())?;
        root.set("componentsRef", components.clone())?;
        let component = lua.create_table()?;
        component.set("entity", root.clone())?;
        component.set("config", shared.clone())?;
        component.set("target", child.clone())?;
        component.set("external", external.clone())?;
        components.push(component.clone())?;

        let clone = instantiate_entity_tree_from_source(&lua, &root, None)?;
        assert_ne!(clone.to_pointer(), root.to_pointer());

        let clone_children: Table = clone.get("children")?;
        let clone_child: Table = clone_children.get(1)?;
        assert_ne!(clone_child.to_pointer(), child.to_pointer());
        let clone_children_ref: Table = clone.get("childrenRef")?;
        assert_eq!(clone_children_ref.to_pointer(), clone_children.to_pointer());

        let linked_child: Table = clone.get("linkedChild")?;
        assert_eq!(linked_child.to_pointer(), clone_child.to_pointer());

        let clone_components: Table = clone.get("components")?;
        let clone_components_ref: Table = clone.get("componentsRef")?;
        assert_eq!(clone_components_ref.to_pointer(), clone_components.to_pointer());
        let clone_component: Table = clone_components.get(1)?;
        let owner: Table = clone_component.get("entity")?;
        assert_eq!(owner.to_pointer(), clone.to_pointer());

        let clone_shared_root: Table = clone.get("shared")?;
        let clone_shared_component: Table = clone_component.get("config")?;
        assert_eq!(clone_shared_root.to_pointer(), clone_shared_component.to_pointer());
        assert_ne!(clone_shared_root.to_pointer(), shared.to_pointer());
        assert!(clone_shared_root.metatable().is_some());

        let clone_target: Table = clone_component.get("target")?;
        assert_eq!(clone_target.to_pointer(), clone_child.to_pointer());

        let clone_external: Table = clone_component.get("external")?;
        assert_eq!(clone_external.to_pointer(), external.to_pointer());

        Ok(())
    }

    #[test]
    fn capture_detaches_root_parent_and_preserves_internal_parenting() -> mlua::Result<()> {
        let lua = Lua::new();

        let scene_parent = create_entity_table(&lua, "scene_parent", 0.0, 0.0, None)?;
        scene_parent.set("id", 10)?;

        let root = create_entity_table(&lua, "root", 4.0, 6.0, Some(scene_parent.clone()))?;
        root.set("id", 11)?;
        let child = create_entity_table(&lua, "child", 1.0, 2.0, Some(root.clone()))?;
        child.set("id", 12)?;

        let captured = capture_entity_tree_template(&lua, &root)?;
        assert!(matches!(captured.raw_get::<Value>("id")?, Value::Nil));
        assert!(matches!(captured.raw_get::<Value>("parent")?, Value::Nil));

        let children: Table = captured.get("children")?;
        let captured_child: Table = children.get(1)?;
        assert!(matches!(captured_child.raw_get::<Value>("id")?, Value::Nil));

        let captured_parent: Table = captured_child.get("parent")?;
        assert_eq!(captured_parent.to_pointer(), captured.to_pointer());

        Ok(())
    }
}
