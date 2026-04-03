use crate::lua_error::protect_lua_call;
use crate::window::create_entity_table;
use mlua::{Function, Lua, Table, Value};
use std::collections::{HashMap, HashSet};

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
    if let Ok(source_components) = source.raw_get::<Table>("components") {
        for component in source_components.sequence_values::<Table>() {
            let component = clone_table_value(lua, &component?, state)?;
            component.raw_set("entity", entity.clone())?;
            crate::window::attach_component_methods(lua, &component)?;
            components.push(component)?;
        }
    }

    if let Ok(source_children) = source.raw_get::<Table>("children") {
        for child in source_children.sequence_values::<Table>() {
            apply_entity_state_recursive(lua, &child?, state, visited)?;
        }
    }

    copy_entity_metadata(lua, source, &entity, state)?;
    Ok(entity)
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

pub(crate) fn add_prefab_module(lua: &Lua) -> mlua::Result<()> {
    let module = lua.create_table()?;
    let registry = lua.create_table()?;

    let capture = lua.create_function(move |lua, entity: Table| capture_entity_tree_template(lua, &entity))?;
    module.set("capture", capture)?;

    let component = lua.create_function(move |lua, (source, overrides): (Table, Option<Table>)| {
        build_component_template(lua, &source, overrides)
    })?;
    module.set("component", component)?;

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
