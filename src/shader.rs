use mlua::{AnyUserData, Lua, Table, UserData, UserDataMethods};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default)]
pub(crate) struct ShaderUniforms {
    floats: HashMap<String, Vec<f32>>,
    textures: HashMap<String, crate::assets::ImageHandle>,
}

#[derive(Clone, Debug)]
pub(crate) struct ShaderHandle {
    #[allow(dead_code)]
    pub(crate) vertex_source: String,
    #[allow(dead_code)]
    pub(crate) fragment_source: String,
    pub(crate) uniforms: Arc<Mutex<ShaderUniforms>>,
}

pub(crate) const DEFAULT_VERTEX_SHADER: &str = r#"#version 450
layout(location = 0) in vec2 position;
layout(location = 1) in vec2 uv;
layout(location = 2) in vec4 color;

layout(location = 0) out vec2 out_uv;
layout(location = 1) out vec4 out_color;

void main() {
    gl_Position = vec4(position, 0.0, 1.0);
    out_uv = uv;
    out_color = color;
}"#;

fn resolve_path(root: &Path, input: &str) -> PathBuf {
    let path = PathBuf::from(input);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

fn load_shader_from_sources(vertex_source: &str, fragment_source: &str) -> ShaderHandle {
    ShaderHandle {
        vertex_source: vertex_source.to_string(),
        fragment_source: fragment_source.to_string(),
        uniforms: Arc::new(Mutex::new(ShaderUniforms::default())),
    }
}

#[allow(dead_code)]
pub(crate) fn bind_shader_from_userdata(_shader_ud: &AnyUserData) -> mlua::Result<()> {
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn unbind_shader() {}

impl UserData for ShaderHandle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("setUniform1f", |_lua, this, (name, x): (String, f32)| {
            let mut uniforms = this
                .uniforms
                .lock()
                .map_err(|_| mlua::Error::external("shader uniform lock poisoned"))?;
            uniforms.floats.insert(name, vec![x]);
            Ok(())
        });
        methods.add_method(
            "setUniform2f",
            |_lua, this, (name, x, y): (String, f32, f32)| {
                let mut uniforms = this
                    .uniforms
                    .lock()
                    .map_err(|_| mlua::Error::external("shader uniform lock poisoned"))?;
                uniforms.floats.insert(name, vec![x, y]);
                Ok(())
            },
        );
        methods.add_method(
            "setUniform3f",
            |_lua, this, (name, x, y, z): (String, f32, f32, f32)| {
                let mut uniforms = this
                    .uniforms
                    .lock()
                    .map_err(|_| mlua::Error::external("shader uniform lock poisoned"))?;
                uniforms.floats.insert(name, vec![x, y, z]);
                Ok(())
            },
        );
        methods.add_method(
            "setUniform4f",
            |_lua, this, (name, x, y, z, w): (String, f32, f32, f32, f32)| {
                let mut uniforms = this
                    .uniforms
                    .lock()
                    .map_err(|_| mlua::Error::external("shader uniform lock poisoned"))?;
                uniforms.floats.insert(name, vec![x, y, z, w]);
                Ok(())
            },
        );
        methods.add_method(
            "setUniformColor",
            |_lua, this, (name, color): (String, Table)| {
                let mut uniforms = this
                    .uniforms
                    .lock()
                    .map_err(|_| mlua::Error::external("shader uniform lock poisoned"))?;
                uniforms.floats.insert(
                    name,
                    vec![
                        color.get::<f32>("r")?,
                        color.get::<f32>("g")?,
                        color.get::<f32>("b")?,
                        color.get::<f32>("a")?,
                    ],
                );
                Ok(())
            },
        );
        methods.add_method(
            "setTexture",
            |_lua, this, (name, image_ud): (String, AnyUserData)| {
                let image = image_ud.borrow::<crate::assets::ImageHandle>()?;
                image.ensure_uploaded()?;
                let mut uniforms = this
                    .uniforms
                    .lock()
                    .map_err(|_| mlua::Error::external("shader uniform lock poisoned"))?;
                uniforms.textures.insert(name, image.clone());
                Ok(())
            },
        );
    }
}

pub(crate) fn add_shader_module(lua: &Lua, env_root: PathBuf) -> mlua::Result<()> {
    let shaders = lua.create_table()?;
    shaders.set("DEFAULT_VERTEX_SHADER", DEFAULT_VERTEX_SHADER)?;

    let load_root = env_root.clone();
    shaders.set(
        "load",
        lua.create_function(
            move |lua, (vertex_path, fragment_path, _options): (String, String, Option<Table>)| {
                let vertex_source = fs::read_to_string(resolve_path(&load_root, &vertex_path))
                    .map_err(mlua::Error::external)?;
                let fragment_source = fs::read_to_string(resolve_path(&load_root, &fragment_path))
                    .map_err(mlua::Error::external)?;
                lua.create_userdata(load_shader_from_sources(&vertex_source, &fragment_source))
            },
        )?,
    )?;

    let fragment_root = env_root.clone();
    shaders.set(
        "loadFragment",
        lua.create_function(
            move |lua, (fragment_path, _options): (String, Option<Table>)| {
                let fragment_source =
                    fs::read_to_string(resolve_path(&fragment_root, &fragment_path))
                        .map_err(mlua::Error::external)?;
                lua.create_userdata(load_shader_from_sources(
                    DEFAULT_VERTEX_SHADER,
                    &fragment_source,
                ))
            },
        )?,
    )?;

    shaders.set(
        "fromSource",
        lua.create_function(
            move |lua, (vertex_source, fragment_source, _options): (String, String, Option<Table>)| {
                lua.create_userdata(load_shader_from_sources(&vertex_source, &fragment_source))
            },
        )?,
    )?;

    shaders.set(
        "fromFragmentSource",
        lua.create_function(
            move |lua, (fragment_source, _options): (String, Option<Table>)| {
                lua.create_userdata(load_shader_from_sources(
                    DEFAULT_VERTEX_SHADER,
                    &fragment_source,
                ))
            },
        )?,
    )?;

    lua.globals().set("shaders", shaders)?;
    Ok(())
}
