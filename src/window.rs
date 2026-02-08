use macroquad::prelude::*;
use mlua::{Function, Lua, RegistryKey, Table, TextRequirer, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use crate::hierarchy;

pub struct Runtime {
    entities: Rc<RefCell<HashMap<hierarchy::EntityId, hierarchy::Entity>>>,
    systems: Rc<RefCell<Vec<RegistryKey>>>,
    environment: PathBuf,
    lua: Lua,
    entity_max: usize,
    max_fps: Rc<RefCell<Option<f32>>>,
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

fn create_entity_table(
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
    table.set("z", 0.0)?;
    table.set("size_x", 32.0)?;
    table.set("size_y", 32.0)?;
    table.set("components", lua.create_table()?)?;
    if let Some(par) = parent {
        table.set("parent", &par)?;
        let children: Table = par.get("children")?;
        children.push(&table)?;
    }
    table.set("children", lua.create_table()?)?;
    Ok(table)
}

pub fn get_global_position(entity: &Table) -> mlua::Result<(f32, f32)> {
    let mut current_entity = entity.clone();
    let mut total_x = 0.0;
    let mut total_y = 0.0;

    loop {
        let x: f32 = current_entity.get("x")?;
        let y: f32 = current_entity.get("y")?;
        total_x += x;
        total_y += y;

        if let Ok(Some(parent)) = current_entity.get::<Option<Table>>("parent") {
            current_entity = parent;
        } else {
            break;
        }
    }

    Ok((total_x, total_y))
}

impl Runtime {
    pub fn new(env: PathBuf) -> Runtime {
        Runtime {
            entities: Rc::new(RefCell::new(HashMap::new())),
            systems: Rc::new(RefCell::new(Vec::new())),
            environment: env,
            lua: Lua::new(),
            entity_max: 1,
            // default to 60fps cap; users can raise/lower/disable via app.setMaxFps
            max_fps: Rc::new(RefCell::new(Some(60.0))),
        }
    }

    pub fn max_fps(&self) -> Option<f32> {
        *self.max_fps.borrow()
    }

    fn set_mouse_table(&mut self) -> mlua::Result<()> {
        let (mouse_x, mouse_y) = mouse_position();

        let mouse_table = self.lua.create_table()?;
        mouse_table.set("x", mouse_x)?;
        mouse_table.set("y", mouse_y)?;
        self.lua.globals().set("mouse", mouse_table)?;
        Ok(())
    }

    fn set_window_table(&mut self) -> mlua::Result<()> {
        let width = screen_width();
        let height = screen_height();
        let table = self.lua.create_table()?;
        table.set("x", width)?;
        table.set("y", height)?;
        self.lua.globals().set("window", table)?;
        Ok(())
    }

    pub fn start(&mut self) -> mlua::Result<()> {
        let require = self
            .lua
            .create_require_function(TextRequirer::new())?;
        self.lua.globals().set("require", require)?;

        self.set_mouse_table()?;
        self.set_window_table()?;

        // App
        {
            let app = self.lua.create_table()?;
            app.set("bg", color4_table(&self.lua, 255, 255, 255, 255)?)?;

            let max_fps_setter = self.max_fps.clone();
            let set_max_fps = self
                .lua
                .create_function(move |_lua, fps: Option<f32>| {
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

            self.lua.globals().set("app", app)?;
        }

        let env_root = self
            .environment
            .canonicalize()
            .map_err(mlua::Error::external)?;

        crate::assets::add_assets_module(&self.lua, env_root.clone())?;

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

        let die = self
            .lua
            .create_function(move |_lua, ()| {
                std::process::exit(1);
                #[allow(unreachable_code)]
                Ok(())
            })?;

        self.lua.globals().set("die", die)?;

        // Transforms
        {
            let get_world_position = self
                .lua
                .create_function(move |_lua, entity: Table| {
                    let (x, y) = get_global_position(&entity)?;
                    Ok((x, y))
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
                        let w1: f32 = entity1.get("size_x")?;
                        let h1: f32 = entity1.get("size_y")?;

                        let (x2, y2) = get_global_position(&entity2)?;
                        let w2: f32 = entity2.get("size_x")?;
                        let h2: f32 = entity2.get("size_y")?;

                        if x1 < x2 + w2 && x1 + w1 > x2 && y1 < y2 + h2 && y1 + h1 > y2 {
                            return Ok(true);
                        }
                    }
                }

                Ok(false)
            })?;

            transforms
                .set("getWorldPosition", get_world_position)?;

            transforms.set("doTheyOverlap", do_they_overlap)?;
        }

        // Systems
        {
            let systems = self.systems.clone();
            let add_system = self
                .lua
                .create_function(move |lua, system: Table| {
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
            let entities_duplicate = self.entities.clone();
            let entity_max = Rc::new(RefCell::new(self.entity_max));
            let entity_max_clone = entity_max.clone();
            let entity_max_duplicate = entity_max.clone();

            let new = self
                .lua
                .create_function(
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

            let delete = self
                .lua
                .create_function(move |lua, entity: Table| {
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
                    for id in ids_to_remove {
                        entities.remove(&id);
                    }

                    if let Ok(Some(parent)) = entity.get::<Option<Table>>("parent") {
                        let children: Table = parent.get("children")?;

                        let table_remove: Function =
                            lua.globals().get::<Table>("table")?.get("remove")?;

                        let len = children.len()?;
                        for i in 1..=len {
                            if children.get::<Table>(i)? == entity {
                                table_remove.call::<()>((children, i))?;
                                break;
                            }
                        }
                    }
                    Ok(())
                })?;

            ecs.set("deleteEntity", delete)?;

            let duplicate = self
                .lua
                .create_function(move |lua, (target_entity, parent): (Table, Table)| {
                    let new_entity = deep_copy_table(lua, &target_entity)?;

                    let mut max = entity_max_duplicate.borrow_mut();
                    *max += 1;
                    let id = *max;
                    new_entity.set("id", id)?;

                    // Set parent
                    new_entity.set("parent", &parent)?;

                    // Add to parent's children
                    let children: Table = parent.get("children")?;
                    children.push(&new_entity)?;

                    new_entity.set("children", lua.create_table()?)?;

                    let reg_key = lua.create_registry_value(&new_entity)?;
                    let entity_struct = hierarchy::Entity {
                        components: Vec::new(),
                        children: Vec::new(),
                        parent: Some(parent.get::<usize>("id").unwrap_or(0)), // best-effort
                        id,
                        luau_key: reg_key,
                    };
                    entities_duplicate.borrow_mut().insert(id, entity_struct);

                    let components: Table = new_entity.get("components")?;
                    for pair in components.pairs::<Value, Table>() {
                        let (_, comp) = pair?;
                        comp.set("entity", &new_entity)?;
                        if let Ok(awake) = comp.get::<Function>("awake") {
                            awake.call::<()>((&new_entity, &comp))?;
                        }
                    }

                    Ok(new_entity)
                })?;

            ecs.set("duplicateEntity", duplicate)?;

            let find_first_child = self
                .lua
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
            crate::core::add_core_components(&self.lua)?; // a lot of heavy lifting

            let add_component = self
                .lua
                .create_function(move |lua, (entity, component): (Table, Table)| {
                    let components: Table = entity.get("components")?;
                    let comp = deep_copy_table(lua, &component)?;
                    comp.set("entity", &entity)?;
                    let awake: Function = comp.get("awake").map_err(|_| {
                        mlua::Error::external("component has no awake function")
                    })?;
                    awake.call::<()>((&entity, &comp))?;
                    components.push(&comp)?;
                    Ok(comp)
                })?;

            ecs.set("addComponent", add_component)?;
        }

        self.lua.globals().set("ecs", ecs)?;
        self.lua.globals().set("transform", transforms)?;

        self.lua
            .load(entry_file.as_path())
            .set_name(format!("@{}", entry_module.display()))
            .exec()?;

        Ok(())
    }

    pub fn update(&mut self, dt: f32) {
        if let Err(e) = self.set_mouse_table() {
            eprintln!("\x1b[31mLua Error:\x1b[0m Failed to set mouse: {}", e);
        }
        if let Err(e) = self.set_window_table() {
            eprintln!("\x1b[31mLua Error:\x1b[0m Failed to set window: {}", e);
        }

        let clear = (|| -> mlua::Result<Color> {
            let app: Table = self.lua.globals().get("app")?;
            let bg: Table = app.get("bg")?;
            let r: u8 = bg.get("r")?;
            let g: u8 = bg.get("g")?;
            let b: u8 = bg.get("b")?;
            let a: u8 = bg.get("a")?;
            Ok(Color::from_rgba(r, g, b, a))
        })()
        .unwrap_or(Color::from_rgba(255, 255, 255, 255));

        clear_background(clear);

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
                if let Err(e) = update.call::<()>((system.clone(), dt)) {
                    eprintln!("\x1b[31mLua Error in system update:\x1b[0m\n{}", e);
                }
            }
        }

        let mut entity_data: Vec<(usize, f64)> = Vec::new();

        {
            let entities = self.entities.borrow();
            for (id, entity) in entities.iter() {
                if let Ok(table) = self.lua.registry_value::<Table>(&entity.luau_key) {
                    let z = table.get::<f64>("z").unwrap_or(0.0);
                    entity_data.push((*id, z));
                }
            }
        }

        entity_data.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut rendering_components: Vec<(Table, Table, Function)> = Vec::new();

        for (id, _) in entity_data {
            let ent: Option<Table> = {
                let entities = self.entities.borrow();
                if let Some(entity) = entities.get(&id) {
                    self.lua.registry_value(&entity.luau_key).ok()
                } else {
                    None
                }
            };

            let ent = match ent {
                Some(e) => e,
                None => continue,
            };

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

            for component in components.pairs::<usize, Table>() {
                let (_key, component) = match component {
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
                        eprintln!(
                            "\x1b[31mLua Error:\x1b[0m Component missing update: {}",
                            e
                        );
                        continue;
                    }
                };

                let is_rendering = component
                    .contains_key("NEOLOVE_RENDERING")
                    .unwrap_or(false);
                if !is_rendering {
                    if let Err(e) = update.call::<()>((&ent, component, dt)) {
                        eprintln!("\x1b[31mLua Error in component update:\x1b[0m\n{}", e);
                    }
                } else {
                    rendering_components.push((ent.clone(), component, update));
                }
            }
        }

        for trio in rendering_components {
            if let Err(e) = trio.2.call::<()>((trio.0, trio.1, dt)) {
                eprintln!(
                    "\x1b[31mLua Error in rendering component update:\x1b[0m\n{}",
                    e
                );
            }
        }
    }
}
