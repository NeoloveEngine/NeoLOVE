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
    table.set("scale_x", 1.0)?;
    table.set("scale_y", 1.0)?;
    table.set("size_x", 32.0)?;
    table.set("size_y", 32.0)?;
    table.set("components", lua.create_table()?)?;
    if parent.is_some() {
        let par = parent.unwrap();
        table.set("parent", &par)?;
        let children: Table = par.get("children")?;
        children.push(&table).expect("could not push myself");
    }
    table.set("children", lua.create_table()?)?;
    Ok(table)
}

fn get_global_position(entity: &Table) -> mlua::Result<(f32, f32)> {
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
        }
    }

    fn set_mouse_table(&mut self) {
        let (mouse_x, mouse_y) = mouse_position();

        let mouse_table = self.lua.create_table().unwrap();
        mouse_table.set("x", mouse_x).unwrap();
        mouse_table.set("y", mouse_y).unwrap();

        self.lua
            .globals()
            .set("mouse", mouse_table)
            .expect("could not set mouse");
    }

    fn set_window_table(&mut self) {
        let width = screen_width();
        let height = screen_height();
        let table = self.lua.create_table().unwrap();
        table.set("x", width).unwrap();
        table.set("y", height).unwrap();
        self.lua.globals().set("window", table).unwrap();
    }

    pub fn start(&mut self) {
        let require = self
            .lua
            .create_require_function(TextRequirer::new())
            .expect("failed to create require function");
        self.lua
            .globals()
            .set("require", require)
            .expect("failed to set require global");

        self.set_mouse_table();
        self.set_window_table();

        let env_root = self
            .environment
            .canonicalize()
            .expect("bad environment path");
        let entry_file = env_root.join("main.luau");

        let entry_module = entry_file
            .parent()
            .expect("main.luau has no parent dir")
            .join(entry_file.file_stem().expect("main.luau has no file_stem"));

        let ecs = self.lua.create_table().expect("failed to create ecs table");
        let transforms = self
            .lua
            .create_table()
            .expect("failed to create transform table");

        let die = self
            .lua
            .create_function(move |_lua, ()| {
                std::process::exit(1);
                #[allow(unreachable_code)]
                Ok(())
            })
            .unwrap();

        self.lua.globals().set("die", die).unwrap();

        let colours = self.lua.create_table().expect("failed to create bg table");

        colours.set("R", 255).unwrap();
        colours.set("G", 255).unwrap();
        colours.set("B", 255).unwrap();

        self.lua
            .globals()
            .set("bg", colours)
            .expect("failed to make bg");

        // Transforms
        {
            let get_world_position = self
                .lua
                .create_function(move |_lua, entity: Table| {
                    let (x, y) =
                        get_global_position(&entity).expect("could not get global position");
                    Ok((x, y))
                })
                .expect("could not create function");

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
            });

            transforms
                .set("getWorldPosition", get_world_position)
                .expect("could not create global position function");

            transforms
                .set(
                    "doTheyOverlap",
                    do_they_overlap.expect("failed to create overlap function"),
                )
                .expect("failed to set overlap function");
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
                })
                .expect("failed to create addSystem function");

            ecs.set("addSystem", add_system)
                .expect("failed to set addSystem");
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
                )
                .unwrap();

            ecs.set("newEntity", new).expect("failed to set newEntity");

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
                        let children: Table =
                            parent.get("children").expect("parent has no children");

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
                })
                .expect("failed to create delete function");

            ecs.set("deleteEntity", delete)
                .expect("failed to set deleteEntity");

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
                        parent: Some(parent.get("id").unwrap_or(0)), // Assuming parent has ID
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
                })
                .expect("failed to create duplicate function");

            ecs.set("duplicateEntity", duplicate)
                .expect("failed to set duplicateEntity");

            // create root entity
            let root_table = create_entity_table(&self.lua, "root", 0.0, 0.0, None).unwrap();
            root_table.set("id", 0).unwrap();

            let root_key = self.lua.create_registry_value(&root_table).unwrap();
            let root_entity = hierarchy::Entity {
                components: Vec::new(),
                children: Vec::new(),
                parent: None,
                id: 0,
                luau_key: root_key,
            };
            self.entities.borrow_mut().insert(0, root_entity);
            ecs.set("root", root_table).expect("failed to set root");
        }

        // Components
        {
            let add_component = self
                .lua
                .create_function(move |lua, (entity, component): (Table, Table)| {
                    let components: Table =
                        entity.get("components").expect("could not get components");
                    let comp = deep_copy_table(lua, &component)?;
                    comp.set("entity", &entity)?;
                    let awake: Function = comp.get("awake").expect("could not get awake");
                    awake.call::<()>((&entity, &comp)).expect("failed to awake");
                    components.push(&comp).expect("failed to add component");
                    Ok(comp)
                })
                .expect("failed to add component");

            ecs.set("addComponent", add_component)
                .expect("failed to set addComponent");
        }

        self.lua
            .globals()
            .set("ecs", ecs)
            .expect("failed to set ECS global");

        self.lua
            .globals()
            .set("transform", transforms)
            .expect("failed to set transform");

        if let Err(e) = self
            .lua
            .load(entry_file.as_path())
            .set_name(format!("@{}", entry_module.display()))
            .exec()
        {
            eprintln!("\x1b[31mLua Error:\x1b[0m {}", e);
            std::process::exit(1);
        }
    }

    pub fn update(&mut self, dt: f32) {
        self.set_mouse_table();
        self.set_window_table();

        let background: Table = self.lua.globals().get("bg").expect("failed to get bg");
        let r: u8 = background.get("R").expect("failed to get R");
        let g: u8 = background.get("G").expect("failed to get G");
        let b: u8 = background.get("B").expect("failed to get B");

        clear_background(Color::from_rgba(r, g, b, 255));

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
                    std::process::exit(1);
                }
            }
        }

        // Collect keys first to avoid holding the borrow during iteration
        let entity_ids: Vec<usize> = self.entities.borrow().keys().cloned().collect();

        for id in entity_ids {
            // Retrieve the entity table within a short-lived borrow scope
            // This ensures we don't hold the entities borrow while executing Lua code
            let ent: Option<Table> = {
                let entities = self.entities.borrow();
                if let Some(entity) = entities.get(&id) {
                    // We assume getting the value from registry doesn't trigger callbacks that modify entities
                    self.lua.registry_value(&entity.luau_key).ok()
                } else {
                    None
                }
            };

            // If entity was deleted or failed to retrieve, skip
            let ent = match ent {
                Some(e) => e,
                None => continue,
            };

            // for now, we draw a box as a test
            // later on, we will add "native components" which give more information about drawing & what to draw

            let (x, y) = get_global_position(&ent).expect("failed to get global position");

            let size_x: f32 = ent.get("size_x").expect("failed to get size_x");
            let size_y: f32 = ent.get("size_y").expect("failed to get size_y");

            draw_rectangle(
                x,
                y,
                size_x,
                size_y,
                Color::from_rgba(255 - r, 255 - g, 255 - b, 255),
            );

            // run through all the components

            let components: Table = ent.get("components").expect("failed to get components");

            for component in components.pairs::<usize, Table>() {
                let (_key, component) = component.unwrap();

                let update: Function = component.get("update").expect("failed to get update");
                if let Err(e) = update.call::<()>((&ent, component, dt)) {
                    eprintln!("\x1b[31mLua Error in component update:\x1b[0m\n{}", e);
                    // We don't exit here to allow other components/entities to update,
                    // but depending on severity we might want to.
                    // The previous code panicked on unwrap or similar, so let's stick to safe error printing.
                    // Actually original code did not have try-catch around update, it just unwrapped.
                    // Let's keep it safe.
                }
            }
        }
    }
}
