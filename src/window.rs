use macroquad::prelude::*;
use mlua::{Function, Lua, RegistryKey, Table, TextRequirer, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::SystemTime;

use crate::hierarchy;

pub struct Runtime {
    Entities: HashMap<hierarchy::EntityId, hierarchy::Entity>,
    Systems: Rc<RefCell<Vec<RegistryKey>>>,
    Environment: PathBuf,
    lua: Lua,
}

impl Runtime {
    pub fn new(env: PathBuf) -> Runtime {
        Runtime {
            Entities: HashMap::new(),
            Systems: Rc::new(RefCell::new(Vec::new())),
            Environment: env,
            lua: Lua::new(),
        }
    }

    pub fn start(&mut self) {
        let require = self.lua
            .create_require_function(TextRequirer::new())
            .expect("failed to create require function");
        self.lua.globals()
            .set("require", require)
            .expect("failed to set require global");

        let env_root = self.Environment.canonicalize().expect("bad environment path");
        let entry_file = env_root.join("main.luau");

        let entry_module = entry_file
            .parent()
            .expect("main.luau has no parent dir")
            .join(entry_file.file_stem().expect("main.luau has no file_stem"));

        let ecs = self.lua.create_table().expect("failed to create ecs table");

        {
            let systems = self.Systems.clone();
            let add_system = self.lua
                .create_function(move |lua, system: Table| {
                    let key = lua.create_registry_value(system)?;
                    systems.borrow_mut().push(key);
                    Ok(())
                })
                .expect("failed to create addSystem function");

            ecs.set("addSystem", add_system)
                .expect("failed to set addSystem");
        }

        self.lua.globals().set("ECS", ecs).expect("failed to set ECS global");

        self.lua.load(entry_file.as_path())
            .set_name(format!("@{}", entry_module.display()))
            .exec()
            .expect("failed to load main environment");
    }

    pub fn update(&mut self, dt: f32) {
        let keys = self.Systems.borrow();
        for key in keys.iter() {
            let system: Table = self.lua.registry_value(key).unwrap();
            if let Ok(Value::Function(update)) = system.get::<Value>("update") {
                update.call::<()>((system.clone(), dt)).unwrap();
            }
        }
    }
}
