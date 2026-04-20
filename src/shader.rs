use mlua::{AnyUserData, Lua, Table, UserData, UserDataMethods};
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
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

#[cfg(not(target_os = "emscripten"))]
pub(crate) const MAX_SHADER_FLOAT_UNIFORMS: usize = 16;
#[cfg(not(target_os = "emscripten"))]
pub(crate) const MAX_SHADER_TEXTURE_UNIFORMS: usize = 4;

#[cfg(not(target_os = "emscripten"))]
#[derive(Clone, Debug)]
pub(crate) struct ShaderRuntimeSnapshot {
    pub(crate) pipeline_key: u64,
    pub(crate) fragment_source: String,
    pub(crate) uses_uniform_buffer: bool,
    pub(crate) uniform_slots: [[f32; 4]; MAX_SHADER_FLOAT_UNIFORMS],
    pub(crate) texture_bindings: Vec<(u32, crate::assets::ImageHandle)>,
}

pub(crate) const DEFAULT_VERTEX_SHADER: &str = r#"#version 450
layout(location = 0) in vec2 position;
layout(location = 1) in vec4 color;
layout(location = 2) in vec2 uv;

layout(location = 0) out vec4 out_color;
layout(location = 1) out vec2 out_uv;

void main() {
    gl_Position = vec4(position, 0.0, 1.0);
    out_color = color;
    out_uv = uv;
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

#[cfg(not(target_os = "emscripten"))]
fn parse_declared_uniform(trimmed: &str) -> Option<(&str, &str)> {
    let statement = trimmed.strip_suffix(';')?;
    let mut parts = statement.split_whitespace();
    let keyword = parts.next()?;
    if keyword != "uniform" {
        return None;
    }
    let ty = parts.next()?;
    let name = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    Some((ty, name))
}

#[cfg(not(target_os = "emscripten"))]
fn uniform_arity(ty: &str) -> Option<usize> {
    match ty {
        "float" => Some(1),
        "vec2" => Some(2),
        "vec3" => Some(3),
        "vec4" => Some(4),
        _ => None,
    }
}

#[cfg(not(target_os = "emscripten"))]
fn uniform_swizzle(arity: usize) -> &'static str {
    match arity {
        1 => ".x",
        2 => ".xy",
        3 => ".xyz",
        _ => "",
    }
}

#[cfg(not(target_os = "emscripten"))]
fn build_runtime_fragment_source(
    fragment_source: &str,
) -> Result<(String, Vec<(String, usize)>, Vec<String>), String> {
    let mut body_lines = Vec::new();
    let mut float_uniforms = Vec::new();
    let mut texture_uniforms = Vec::new();

    for raw_line in fragment_source.lines() {
        let trimmed = raw_line.trim();

        if trimmed.starts_with("#version") || trimmed.starts_with("precision ") {
            continue;
        }

        if trimmed.starts_with("varying ") {
            continue;
        }

        if let Some((ty, name)) = parse_declared_uniform(trimmed) {
            if ty == "sampler2D" {
                texture_uniforms.push(name.to_string());
                continue;
            }
            if let Some(arity) = uniform_arity(ty) {
                float_uniforms.push((name.to_string(), arity));
                continue;
            }
        }

        body_lines.push(raw_line.replace("texture2D(", "texture("));
    }

    if float_uniforms.len() > MAX_SHADER_FLOAT_UNIFORMS {
        return Err(format!(
            "shader uses {} float/vector uniforms, but only {} are supported",
            float_uniforms.len(),
            MAX_SHADER_FLOAT_UNIFORMS
        ));
    }
    if texture_uniforms.len() > MAX_SHADER_TEXTURE_UNIFORMS + 1 {
        return Err(format!(
            "shader uses {} texture uniforms, but only {} are supported",
            texture_uniforms.len(),
            MAX_SHADER_TEXTURE_UNIFORMS + 1
        ));
    }

    let mut out = String::from(
        "#version 450\n\
layout(location = 0) in vec4 color;\n\
layout(location = 1) in vec2 uv;\n\
layout(location = 0) out vec4 f_color;\n\
layout(binding = 0) uniform texture2D __neolove_Texture_image;\n\
layout(binding = 1) uniform sampler __neolove_Texture_sampler;\n\
#define Texture sampler2D(__neolove_Texture_image, __neolove_Texture_sampler)\n",
    );

    if !float_uniforms.is_empty() {
        out.push_str(&format!(
            "layout(binding = 2) uniform NeoLoveUniforms {{ vec4 __neolove_uniforms[{}]; }};\n",
            MAX_SHADER_FLOAT_UNIFORMS
        ));
        for (index, (name, arity)) in float_uniforms.iter().enumerate() {
            let swizzle = uniform_swizzle(*arity);
            out.push_str(&format!(
                "#define {name} (__neolove_uniforms[{index}]{swizzle})\n"
            ));
        }
    }

    let mut next_binding = 3u32;
    for name in &texture_uniforms {
        if name == "Texture" {
            continue;
        }
        out.push_str(&format!(
            "layout(binding = {next_binding}) uniform texture2D __neolove_{name}_image;\n\
layout(binding = {}) uniform sampler __neolove_{name}_sampler;\n\
#define {name} sampler2D(__neolove_{name}_image, __neolove_{name}_sampler)\n",
            next_binding + 1
        ));
        next_binding += 2;
    }

    out.push('\n');
    out.push_str(&body_lines.join("\n").replace("gl_FragColor", "f_color"));
    out.push('\n');

    Ok((out, float_uniforms, texture_uniforms))
}

#[cfg(not(target_os = "emscripten"))]
impl ShaderHandle {
    pub(crate) fn snapshot_for_runtime(&self) -> Result<ShaderRuntimeSnapshot, String> {
        let (fragment_source, float_uniforms, texture_uniforms) =
            build_runtime_fragment_source(&self.fragment_source)?;

        let uniforms = self
            .uniforms
            .lock()
            .map_err(|_| "shader uniform lock poisoned".to_string())?;

        let mut uniform_slots = [[0.0; 4]; MAX_SHADER_FLOAT_UNIFORMS];
        for (index, (name, arity)) in float_uniforms.iter().enumerate() {
            if let Some(values) = uniforms.floats.get(name) {
                for channel in 0..(*arity).min(values.len()).min(4) {
                    uniform_slots[index][channel] = values[channel];
                }
            }
        }

        let mut texture_bindings = Vec::new();
        let mut next_binding = 3u32;
        for name in texture_uniforms {
            if name == "Texture" {
                continue;
            }
            if let Some(image) = uniforms.textures.get(&name) {
                texture_bindings.push((next_binding, image.clone()));
            }
            next_binding += 2;
        }

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        fragment_source.hash(&mut hasher);
        let pipeline_key = hasher.finish();

        Ok(ShaderRuntimeSnapshot {
            pipeline_key,
            fragment_source,
            uses_uniform_buffer: !float_uniforms.is_empty(),
            uniform_slots,
            texture_bindings,
        })
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
