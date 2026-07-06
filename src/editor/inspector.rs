use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;

use mlua::{Lua, Table, Value, Variadic, VmState};

use crate::scene::{DictionaryEntry, ScriptVar, VarControl, VarKey, VarValue};

const MAX_INSPECTOR_INSTRUCTIONS: usize = 200_000;

pub(crate) fn parse_inspector_variables(
    source: &str,
    source_name: &str,
) -> Result<Vec<ScriptVar>, String> {
    let lua = Lua::new();
    let environment = lua.create_table().map_err(|error| error.to_string())?;
    let declaration_order = Rc::new(Cell::new(0usize));
    let order_for_inspector = declaration_order.clone();
    let inspector = lua
        .create_function(move |lua, arguments: Variadic<Value>| {
            if arguments.is_empty() {
                return Err(mlua::Error::external(
                    "Inspector requires at least one default value",
                ));
            }
            let marker = lua.create_table()?;
            marker.raw_set("__neolove_inspector", true)?;
            marker.raw_set("order", order_for_inspector.get())?;
            order_for_inspector.set(order_for_inspector.get() + 1);
            let values = lua.create_table()?;
            for value in arguments {
                values.push(value)?;
            }
            marker.raw_set("arguments", values)?;
            Ok(marker)
        })
        .map_err(|error| error.to_string())?;
    environment
        .raw_set("Inspector", inspector)
        .map_err(|error| error.to_string())?;

    let color4 = lua
        .create_function(|lua, (r, g, b, a): (f32, f32, f32, Option<f32>)| {
            let marker = lua.create_table()?;
            marker.raw_set("__neolove_color4", true)?;
            marker.raw_set("r", r)?;
            marker.raw_set("g", g)?;
            marker.raw_set("b", b)?;
            marker.raw_set("a", a.unwrap_or(255.0))?;
            Ok(marker)
        })
        .map_err(|error| error.to_string())?;
    environment
        .raw_set("Color4", color4)
        .map_err(|error| error.to_string())?;

    // These globals are type-shaped defaults for scene references. At runtime
    // Inspector still returns its first argument; in the editor they become an
    // initially-unassigned entity/component picker.
    environment
        .raw_set(
            "IEntity",
            create_reference_stub(&lua, "entity").map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
    environment
        .raw_set(
            "IComponent",
            create_reference_stub(&lua, "component").map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;

    // Preserve reference types for concrete values created at module scope,
    // such as `local target = ecs.newEntity(...)` and
    // `local shape = target:AddComponent(core.Shape2D)`.
    let entity_factory = lua
        .create_function(|lua, _arguments: Variadic<Value>| create_entity_stub(lua))
        .map_err(|error| error.to_string())?;
    let component_factory = lua
        .create_function(|lua, _arguments: Variadic<Value>| {
            create_reference_stub(lua, "component")
        })
        .map_err(|error| error.to_string())?;
    let ecs = create_lenient_stub(&lua).map_err(|error| error.to_string())?;
    ecs.raw_set("newEntity", entity_factory.clone())
        .map_err(|error| error.to_string())?;
    ecs.raw_set("findFirstChild", entity_factory.clone())
        .map_err(|error| error.to_string())?;
    ecs.raw_set("duplicateEntity", entity_factory.clone())
        .map_err(|error| error.to_string())?;
    ecs.raw_set("addComponent", component_factory)
        .map_err(|error| error.to_string())?;
    ecs.raw_set(
        "root",
        create_entity_stub(&lua).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    environment
        .raw_set("ecs", ecs)
        .map_err(|error| error.to_string())?;

    let prefabs = create_lenient_stub(&lua).map_err(|error| error.to_string())?;
    prefabs
        .raw_set("instantiate", entity_factory.clone())
        .map_err(|error| error.to_string())?;
    prefabs
        .raw_set("duplicate", entity_factory)
        .map_err(|error| error.to_string())?;
    environment
        .raw_set("prefabs", prefabs.clone())
        .map_err(|error| error.to_string())?;
    // Keep the historical singular spelling permissive too.
    environment
        .raw_set("prefab", prefabs)
        .map_err(|error| error.to_string())?;
    environment
        .raw_set("_G", environment.clone())
        .map_err(|error| error.to_string())?;

    // Behaviour scripts often touch engine globals (`prefab`, `ecs`, `require`,
    // ...) at module scope, e.g. `local T = prefab.load("...")`. The runtime
    // isn't present while we only parse the script for its `Inspector(...)`
    // fields, so any such name would be nil and abort the whole parse. Resolve
    // unknown globals to a permissive stub that can be indexed, called, and
    // assigned to without error, so module-scope engine usage is harmless here.
    let lenient_stub = create_lenient_stub(&lua).map_err(|error| error.to_string())?;
    let real_globals = lua.globals();
    let index_stub = lenient_stub.clone();
    let metatable = lua.create_table().map_err(|error| error.to_string())?;
    metatable
        .raw_set(
            "__index",
            lua.create_function(move |_, (_env, key): (Table, Value)| {
                // `require` would hit the filesystem; always stub it out. Other
                // real standard-library globals (string, math, table, ...) pass
                // through so legitimate top-level code keeps working.
                if let Value::String(name) = &key {
                    if name != "require" {
                        if let Ok(value) = real_globals.get::<Value>(name.clone()) {
                            if !matches!(value, Value::Nil) {
                                return Ok(value);
                            }
                        }
                    }
                }
                Ok(Value::Table(index_stub.clone()))
            })
            .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
    environment
        .set_metatable(Some(metatable))
        .map_err(|error| error.to_string())?;

    let instruction_count = Rc::new(Cell::new(0usize));
    let instruction_count_interrupt = instruction_count.clone();
    lua.set_interrupt(move |_lua| {
        let next = instruction_count_interrupt.get() + 1;
        instruction_count_interrupt.set(next);
        if next > MAX_INSPECTOR_INSTRUCTIONS {
            Err(mlua::Error::external(
                "script exceeded the Inspector parsing instruction limit",
            ))
        } else {
            Ok(VmState::Continue)
        }
    });

    let component: Table = lua
        .load(source)
        .set_name(source_name)
        .set_environment(environment)
        .eval()
        .map_err(|error| format!("failed to inspect {source_name}: {error}"))?;

    let mut variables = Vec::<(usize, ScriptVar)>::new();
    for pair in component.pairs::<Value, Value>() {
        let (key, value) = pair.map_err(|error| error.to_string())?;
        let Value::String(name) = key else {
            continue;
        };
        let Value::Table(marker) = value else {
            continue;
        };
        if !marker
            .raw_get::<bool>("__neolove_inspector")
            .unwrap_or(false)
        {
            continue;
        }
        let order = marker.raw_get::<usize>("order").unwrap_or(variables.len());
        let arguments: Table = marker
            .raw_get("arguments")
            .map_err(|error| format!("invalid Inspector declaration: {error}"))?;
        let variable = parse_declaration(name.to_string_lossy().as_ref(), &arguments)?;
        variables.push((order, variable));
    }
    variables.sort_by_key(|(order, _)| *order);
    Ok(variables.into_iter().map(|(_, variable)| variable).collect())
}

/// A permissive placeholder table used in place of engine globals during
/// Inspector parsing. Indexing it yields itself, calling it yields itself, and
/// assigning to its fields is a no-op, so chains like
/// `prefab.instantiate(prefab.load("..."), parent)` evaluate without the real
/// runtime being present.
fn create_lenient_stub(lua: &Lua) -> mlua::Result<Table> {
    let stub = lua.create_table()?;
    let metatable = lua.create_table()?;

    let index_target = stub.clone();
    metatable.raw_set(
        "__index",
        lua.create_function(move |_, (_table, _key): (Table, Value)| {
            Ok(Value::Table(index_target.clone()))
        })?,
    )?;

    let call_target = stub.clone();
    metatable.raw_set(
        "__call",
        lua.create_function(move |_, _args: Variadic<Value>| {
            Ok(Value::Table(call_target.clone()))
        })?,
    )?;

    metatable.raw_set(
        "__newindex",
        lua.create_function(|_, (_table, _key, _value): (Table, Value, Value)| Ok(()))?,
    )?;

    stub.set_metatable(Some(metatable))?;
    Ok(stub)
}

fn create_reference_stub<'lua>(lua: &'lua Lua, kind: &str) -> mlua::Result<Table> {
    let stub = create_lenient_stub(lua)?;
    stub.raw_set("__neolove_inspector_reference", kind)?;
    Ok(stub)
}

fn create_entity_stub(lua: &Lua) -> mlua::Result<Table> {
    let stub = create_reference_stub(lua, "entity")?;
    let metatable = stub.metatable().expect("lenient stub metatable");
    let fallback = stub.clone();
    metatable.raw_set(
        "__index",
        lua.create_function(move |lua, (_table, key): (Table, Value)| {
            if let Value::String(key) = key {
                let key = key.to_string_lossy();
                if key == "AddComponent" || key == "addComponent" {
                    return Ok(Value::Function(lua.create_function(
                        |lua, _arguments: Variadic<Value>| {
                            create_reference_stub(lua, "component")
                        },
                    )?));
                }
            }
            Ok(Value::Table(fallback.clone()))
        })?,
    )?;
    Ok(stub)
}

fn parse_declaration(name: &str, arguments: &Table) -> Result<ScriptVar, String> {
    let first = arguments
        .raw_get::<Value>(1)
        .map_err(|error| format!("Inspector({name}) has no default value: {error}"))?;
    let argument_count = arguments.raw_len();
    let mut control = VarControl::Field;
    let value = if argument_count >= 2 {
        match (&first, arguments.raw_get::<Value>(2)) {
            (Value::Integer(min), Ok(Value::Integer(max))) => {
                let min = *min as f32;
                let max = max as f32;
                let fractional = arguments.raw_get::<bool>(3).unwrap_or(false);
                control = VarControl::Slider {
                    min: min.min(max),
                    max: min.max(max),
                    fractional,
                };
                VarValue::Number(min)
            }
            (Value::Number(min), Ok(Value::Number(max))) => {
                let min = *min as f32;
                let max = max as f32;
                let fractional = arguments.raw_get::<bool>(3).unwrap_or(false)
                    || min.fract() != 0.0
                    || max.fract() != 0.0;
                control = VarControl::Slider {
                    min: min.min(max),
                    max: min.max(max),
                    fractional,
                };
                VarValue::Number(min)
            }
            (Value::Integer(min), Ok(Value::Number(max))) => {
                let min = *min as f32;
                let max = max as f32;
                control = VarControl::Slider {
                    min: min.min(max),
                    max: min.max(max),
                    fractional: true,
                };
                VarValue::Number(min)
            }
            (Value::Number(min), Ok(Value::Integer(max))) => {
                let min = *min as f32;
                let max = max as f32;
                control = VarControl::Slider {
                    min: min.min(max),
                    max: min.max(max),
                    fractional: true,
                };
                VarValue::Number(min)
            }
            _ => {
                return Err(format!(
                    "Inspector({name}, ...) only accepts a second argument for numeric sliders"
                ));
            }
        }
    } else {
        lua_value_to_var(first, &mut HashSet::new())?
    };

    Ok(ScriptVar {
        name: name.to_string(),
        value,
        control,
    })
}

fn lua_value_to_var(value: Value, visiting: &mut HashSet<usize>) -> Result<VarValue, String> {
    match value {
        Value::Integer(value) => Ok(VarValue::Number(value as f32)),
        Value::Number(value) => Ok(VarValue::Number(value as f32)),
        Value::Boolean(value) => Ok(VarValue::Bool(value)),
        Value::String(value) => Ok(VarValue::Text(value.to_string_lossy())),
        Value::Table(table) => table_to_var(table, visiting),
        other => Err(format!(
            "Inspector does not support default values of type {}",
            other.type_name()
        )),
    }
}

fn table_to_var(table: Table, visiting: &mut HashSet<usize>) -> Result<VarValue, String> {
    match table
        .raw_get::<String>("__neolove_inspector_reference")
        .ok()
        .as_deref()
    {
        Some("entity") => return Ok(VarValue::Entity(None)),
        Some("component") => return Ok(VarValue::Component(None)),
        _ => {}
    }

    // Also accept concrete engine-shaped values if a script constructs one
    // without going through the sandboxed ecs helpers above.
    if !matches!(
        table.raw_get::<Value>("__neolove_component"),
        Ok(Value::Nil) | Err(_)
    ) {
        return Ok(VarValue::Component(None));
    }
    let has_entity_id = matches!(
        table.raw_get::<Value>("id"),
        Ok(Value::Integer(_)) | Ok(Value::Number(_))
    );
    let has_components = matches!(table.raw_get::<Value>("components"), Ok(Value::Table(_)));
    if has_entity_id && has_components {
        return Ok(VarValue::Entity(None));
    }

    if table.raw_get::<bool>("__neolove_color4").unwrap_or(false) {
        let channel = |name: &str| {
            table
                .raw_get::<f32>(name)
                .unwrap_or(0.0)
                .round()
                .clamp(0.0, 255.0) as u8
        };
        return Ok(VarValue::Color([
            channel("r"),
            channel("g"),
            channel("b"),
            channel("a"),
        ]));
    }

    let pointer = table.to_pointer() as usize;
    if !visiting.insert(pointer) {
        return Err("Inspector tables cannot contain cycles".to_string());
    }

    let mut entries = Vec::<(VarKey, Value)>::new();
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair.map_err(|error| error.to_string())?;
        let key = match key {
            Value::Integer(value) => VarKey::Number(value as f32),
            Value::Number(value) => VarKey::Number(value as f32),
            Value::Boolean(value) => VarKey::Bool(value),
            Value::String(value) => VarKey::Text(value.to_string_lossy()),
            other => {
                visiting.remove(&pointer);
                return Err(format!(
                    "Inspector dictionary keys cannot use type {}",
                    other.type_name()
                ));
            }
        };
        entries.push((key, value));
    }

    let sequence_len = entries.len();
    let is_list = entries.iter().all(|(key, _)| match key {
        VarKey::Number(value) => {
            value.fract() == 0.0 && *value >= 1.0 && *value <= sequence_len as f32
        }
        _ => false,
    }) && (1..=sequence_len).all(|index| {
        entries.iter().any(|(key, _)| {
            matches!(key, VarKey::Number(value) if *value == index as f32)
        })
    });

    let result = if is_list {
        entries.sort_by(|(a, _), (b, _)| match (a, b) {
            (VarKey::Number(a), VarKey::Number(b)) => a.total_cmp(b),
            _ => std::cmp::Ordering::Equal,
        });
        VarValue::List(
            entries
                .into_iter()
                .map(|(_, value)| lua_value_to_var(value, visiting))
                .collect::<Result<Vec<_>, _>>()?,
        )
    } else {
        VarValue::Dictionary(
            entries
                .into_iter()
                .map(|(key, value)| {
                    Ok(DictionaryEntry {
                        key,
                        value: lua_value_to_var(value, visiting)?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
        )
    };
    visiting.remove(&pointer);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scalar_slider_color_list_and_dictionary_declarations() {
        let source = r#"
            local Component = {
                speed = Inspector(4),
                volume = Inspector(0, 10),
                opacity = Inspector(0, 1, true),
                title = Inspector("Player"),
                enabled = Inspector(true),
                tint = Inspector(Color4(10, 20, 30, 40)),
                items = Inspector({ 1, "two", false }),
                lookup = Inspector({ name = "Neo", [4] = true }),
            }
            function Component.awake(entity, self) end
            return Component
        "#;
        let variables = parse_inspector_variables(source, "test.luau").expect("parse");
        assert_eq!(variables.len(), 8);
        assert!(matches!(variables[0].value, VarValue::Number(4.0)));
        assert!(matches!(
            variables[1].control,
            VarControl::Slider { fractional: false, .. }
        ));
        assert!(matches!(
            variables[2].control,
            VarControl::Slider { fractional: true, .. }
        ));
        assert!(matches!(variables[5].value, VarValue::Color([10, 20, 30, 40])));
        assert!(matches!(variables[6].value, VarValue::List(ref values) if values.len() == 3));
        assert!(matches!(variables[7].value, VarValue::Dictionary(ref values) if values.len() == 2));
    }

    #[test]
    fn module_scope_engine_calls_do_not_abort_parsing() {
        // `prefab.load`/`require` at module scope used to make Inspector parsing
        // fail outright, dropping every Inspector(...) field. They must now be
        // tolerated so the declared fields are still extracted.
        let source = r#"
            local Behaviour = {
                Padding = Inspector(10),
            }
            local ButtonTemplate = prefab.load("prefabs/ui/UI_Background.neoprefab")
            local Helper = require("scripts/Helper")
            local TestButtons = 3
            function Behaviour.awake(entity, self)
                for i = 1, TestButtons do
                    local bt = prefab.instantiate(ButtonTemplate, entity)
                    bt.x = bt.size_x * i + self.Padding * i
                end
            end
            return Behaviour
        "#;
        let variables = parse_inspector_variables(source, "Behaviour.luau").expect("parse");
        assert_eq!(variables.len(), 1);
        assert_eq!(variables[0].name, "Padding");
        assert!(matches!(variables[0].value, VarValue::Number(10.0)));
    }

    #[test]
    fn parses_entity_and_component_placeholders_and_concrete_values() {
        let source = r#"
            local actualEntity = ecs.newEntity("Target", ecs.root)
            local actualComponent = actualEntity:AddComponent(core.Rect2D)
            local entityTable = { id = 7, components = {} }
            local componentTable = { __neolove_component = "Rect2D" }
            local Component = {
                entityType = Inspector(IEntity),
                entityValue = Inspector(actualEntity),
                componentType = Inspector(IComponent),
                componentValue = Inspector(actualComponent),
                entityTable = Inspector(entityTable),
                componentTable = Inspector(componentTable),
            }
            return Component
        "#;
        let variables = parse_inspector_variables(source, "references.luau").expect("parse");
        assert_eq!(variables.len(), 6);
        assert!(matches!(variables[0].value, VarValue::Entity(None)));
        assert!(matches!(variables[1].value, VarValue::Entity(None)));
        assert!(matches!(variables[2].value, VarValue::Component(None)));
        assert!(matches!(variables[3].value, VarValue::Component(None)));
        assert!(matches!(variables[4].value, VarValue::Entity(None)));
        assert!(matches!(variables[5].value, VarValue::Component(None)));
    }
}
