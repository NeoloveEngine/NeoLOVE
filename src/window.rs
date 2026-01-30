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

fn create_entity_table(lua: &Lua, name: &str, x: f64, y: f64) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("name", name)?;
    table.set("x", x)?;
    table.set("y", y)?;
    table.set("scale_x", 1.0)?;
    table.set("scale_y", 1.0)?;
    table.set("size_x", 32.0)?;
    table.set("size_y", 32.0)?;
    table.set("components", lua.create_table()?)?;
    Ok(table)
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

        let colours = self.lua.create_table().expect("failed to create bg table");

        colours.set("R", 255).unwrap();
        colours.set("G", 255).unwrap();
        colours.set("B", 255).unwrap();

        self.lua
            .globals()
            .set("bg", colours)
            .expect("failed to make bg");

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
            let entity_max = Rc::new(RefCell::new(self.entity_max));
            let entity_max_clone = entity_max.clone();

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
                        let luau =
                            create_entity_table(lua, &name, x.unwrap_or(0.0), y.unwrap_or(0.0))?;

                        let mut max = entity_max_clone.borrow_mut();
                        *max += 1;
                        let id = *max;

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

            // create root entity
            let root_table = create_entity_table(&self.lua, "root", 0.0, 0.0).unwrap();

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
            .set("ECS", ecs)
            .expect("failed to set ECS global");

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

        for entity in self.entities.borrow().iter() {
            let ent: Table = self.lua.registry_value(&entity.1.luau_key).unwrap();

            // for now, we draw a box as a test
            // later on, we will add "native components" which give more information about drawing & what to draw

            let x: f32 = ent.get("x").expect("failed to get x");
            let y: f32 = ent.get("y").expect("failed to get y");

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
                update
                    .call::<()>((&ent, component, dt))
                    .expect("failed to call update");
            }
        }
    }
}
