use mlua::{AnyUserData, Lua, Table, UserData, UserDataMethods};
use std::collections::HashMap;
use std::fs;
#[cfg(all(not(target_os = "emscripten"), feature = "vulkan"))]
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default)]
pub(crate) struct ShaderUniforms {
    floats: HashMap<String, Vec<f32>>,
    textures: HashMap<String, crate::assets::ImageHandle>,
}

impl ShaderUniforms {
    fn set_floats(&mut self, name: String, values: Vec<f32>) -> Result<(), String> {
        if !self.floats.contains_key(&name) && self.floats.len() >= MAX_SHADER_FLOAT_UNIFORMS {
            return Err(format!(
                "shader already stores the maximum of {MAX_SHADER_FLOAT_UNIFORMS} float/vector uniforms"
            ));
        }
        self.floats.insert(name, values);
        Ok(())
    }

    fn set_texture(
        &mut self,
        name: String,
        image: crate::assets::ImageHandle,
    ) -> Result<(), String> {
        if !self.textures.contains_key(&name) && self.textures.len() >= MAX_SHADER_TEXTURE_UNIFORMS
        {
            return Err(format!(
                "shader already stores the maximum of {MAX_SHADER_TEXTURE_UNIFORMS} extra texture uniforms"
            ));
        }
        self.textures.insert(name, image);
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ShaderHandle {
    #[allow(dead_code)]
    pub(crate) vertex_source: String,
    #[allow(dead_code)]
    pub(crate) fragment_source: String,
    pub(crate) uniforms: Arc<Mutex<ShaderUniforms>>,
}

pub(crate) const MAX_SHADER_FLOAT_UNIFORMS: usize = 16;
pub(crate) const MAX_SHADER_TEXTURE_UNIFORMS: usize = 4;

#[cfg(target_os = "emscripten")]
#[derive(Clone, Debug)]
pub(crate) struct WebShaderSnapshot {
    pub(crate) fragment_source: String,
    pub(crate) uniforms_json: String,
}

#[cfg(all(not(target_os = "emscripten"), feature = "vulkan"))]
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

const THREE_D_SHADERS_AVAILABLE: bool =
    cfg!(target_os = "emscripten") || cfg!(all(not(target_os = "emscripten"), feature = "vulkan"));

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

/// Convert NeoLOVE's portable fragment dialect to WebGL 1 GLSL. Shader assets
/// created by the editor use `#version 450` so the Vulkan path can parse them;
/// WebGL 1 instead needs no version directive and explicit matching varyings.
/// Keeping this normalization here also makes one shader asset usable for 2D
/// drawables and MeshRenderer3D on both GPU backends.
#[cfg(any(target_os = "emscripten", test))]
fn build_web_fragment_source(fragment_source: &str) -> String {
    let mut body = Vec::new();
    for raw_line in fragment_source.lines() {
        let trimmed = raw_line.trim();
        if trimmed.starts_with("#version") || trimmed.starts_with("precision ") {
            continue;
        }
        if trimmed.starts_with("varying ")
            && (trimmed.ends_with(" uv;") || trimmed.ends_with(" color;"))
        {
            continue;
        }
        body.push(raw_line.replace("texture(", "texture2D("));
    }

    format!(
        "precision mediump float;\n\
varying mediump vec2 uv;\n\
varying mediump vec4 color;\n{}\n",
        body.join("\n")
    )
}

#[cfg(all(not(target_os = "emscripten"), feature = "vulkan"))]
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

#[cfg(all(not(target_os = "emscripten"), feature = "vulkan"))]
fn uniform_arity(ty: &str) -> Option<usize> {
    match ty {
        "float" => Some(1),
        "vec2" => Some(2),
        "vec3" => Some(3),
        "vec4" => Some(4),
        _ => None,
    }
}

#[cfg(all(not(target_os = "emscripten"), feature = "vulkan"))]
fn uniform_swizzle(arity: usize) -> &'static str {
    match arity {
        1 => ".x",
        2 => ".xy",
        3 => ".xyz",
        _ => "",
    }
}

#[cfg(all(not(target_os = "emscripten"), feature = "vulkan"))]
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

    let body = body_lines.join("\n").replace("gl_FragColor", "f_color");
    if !body.contains("void main") {
        return Err("fragment shader is missing void main".to_string());
    }
    // Runtime fragment shaders historically authored display-referred output.
    // The native scene now blends in a linear RGBA16F target, so retain that
    // public contract by decoding the custom result before it enters the HDR
    // frame graph. The final presentation pass performs the matching encode.
    let body = body.replacen("void main", "void __neolove_display_main", 1);
    out.push('\n');
    out.push_str(&body);
    out.push_str(
        "\nvoid main() {\n\
    __neolove_display_main();\n\
    f_color.rgb = pow(max(f_color.rgb, vec3(0.0)), vec3(2.2));\n\
}\n",
    );

    Ok((out, float_uniforms, texture_uniforms))
}

#[cfg(all(not(target_os = "emscripten"), feature = "vulkan"))]
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

#[cfg(target_os = "emscripten")]
impl ShaderHandle {
    pub(crate) fn snapshot_for_web(&self) -> Result<WebShaderSnapshot, String> {
        let uniforms = self
            .uniforms
            .lock()
            .map_err(|_| "shader uniform lock poisoned".to_string())?;

        Ok(WebShaderSnapshot {
            fragment_source: build_web_fragment_source(&self.fragment_source),
            uniforms_json: serde_json::json!({ "floats": &uniforms.floats }).to_string(),
        })
    }
}

impl UserData for ShaderHandle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("setUniform1f", |_lua, this, (name, x): (String, f32)| {
            let mut uniforms = this
                .uniforms
                .lock()
                .map_err(|_| mlua::Error::external("shader uniform lock poisoned"))?;
            uniforms
                .set_floats(name, vec![x])
                .map_err(mlua::Error::external)
        });
        methods.add_method(
            "setUniform2f",
            |_lua, this, (name, x, y): (String, f32, f32)| {
                let mut uniforms = this
                    .uniforms
                    .lock()
                    .map_err(|_| mlua::Error::external("shader uniform lock poisoned"))?;
                uniforms
                    .set_floats(name, vec![x, y])
                    .map_err(mlua::Error::external)
            },
        );
        methods.add_method(
            "setUniform3f",
            |_lua, this, (name, x, y, z): (String, f32, f32, f32)| {
                let mut uniforms = this
                    .uniforms
                    .lock()
                    .map_err(|_| mlua::Error::external("shader uniform lock poisoned"))?;
                uniforms
                    .set_floats(name, vec![x, y, z])
                    .map_err(mlua::Error::external)
            },
        );
        methods.add_method(
            "setUniform4f",
            |_lua, this, (name, x, y, z, w): (String, f32, f32, f32, f32)| {
                let mut uniforms = this
                    .uniforms
                    .lock()
                    .map_err(|_| mlua::Error::external("shader uniform lock poisoned"))?;
                uniforms
                    .set_floats(name, vec![x, y, z, w])
                    .map_err(mlua::Error::external)
            },
        );
        methods.add_method(
            "setUniformColor",
            |_lua, this, (name, color): (String, Table)| {
                let mut uniforms = this
                    .uniforms
                    .lock()
                    .map_err(|_| mlua::Error::external("shader uniform lock poisoned"))?;
                uniforms
                    .set_floats(
                        name,
                        vec![
                            color.get::<f32>("r")? / 255.0,
                            color.get::<f32>("g")? / 255.0,
                            color.get::<f32>("b")? / 255.0,
                            color.get::<f32>("a")? / 255.0,
                        ],
                    )
                    .map_err(mlua::Error::external)
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
                uniforms
                    .set_texture(name, image.clone())
                    .map_err(mlua::Error::external)
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
    let load_fragment = lua.create_function(
        move |lua, (fragment_path, _options): (String, Option<Table>)| {
            let fragment_source = fs::read_to_string(resolve_path(&fragment_root, &fragment_path))
                .map_err(mlua::Error::external)?;
            lua.create_userdata(load_shader_from_sources(
                DEFAULT_VERTEX_SHADER,
                &fragment_source,
            ))
        },
    )?;
    shaders.set("loadFragment", load_fragment.clone())?;
    // MeshRenderer3D consumes the same programmable fragment stage as the 2D
    // drawables. The explicit alias makes that contract discoverable without
    // introducing a second shader representation or cache.
    shaders.set("load3DFragment", load_fragment)?;

    shaders.set(
        "fromSource",
        lua.create_function(
            move |lua, (vertex_source, fragment_source, _options): (String, String, Option<Table>)| {
                lua.create_userdata(load_shader_from_sources(&vertex_source, &fragment_source))
            },
        )?,
    )?;

    let from_fragment_source = lua.create_function(
        move |lua, (fragment_source, _options): (String, Option<Table>)| {
            lua.create_userdata(load_shader_from_sources(
                DEFAULT_VERTEX_SHADER,
                &fragment_source,
            ))
        },
    )?;
    shaders.set("fromFragmentSource", from_fragment_source.clone())?;
    shaders.set("from3DFragmentSource", from_fragment_source)?;

    let supports_3d = lua.create_function(|_lua, ()| Ok(THREE_D_SHADERS_AVAILABLE))?;
    shaders.set("supports3D", supports_3d.clone())?;
    shaders.set("supports3DShaders", supports_3d)?;

    lua.globals().set("shaders", shaders)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_d_fragment_aliases_share_the_fragment_shader_contract() {
        let lua = Lua::new();
        add_shader_module(&lua, PathBuf::from(".")).expect("shader module");
        let shaders: Table = lua.globals().get("shaders").expect("shaders global");

        let create: mlua::Function = shaders
            .get("from3DFragmentSource")
            .expect("3D source constructor");
        let shader: AnyUserData = create
            .call((
                "#version 450\nvoid main() { gl_FragColor = color; }",
                mlua::Value::Nil,
            ))
            .expect("3D fragment handle");
        lua.globals()
            .set("materialShader", shader.clone())
            .expect("shader test global");
        lua.load("materialShader:setUniformColor('Tint', { r = 255, g = 128, b = 0, a = 64 })")
            .exec()
            .expect("color uniform");
        let shader = shader.borrow::<ShaderHandle>().expect("shader userdata");
        assert_eq!(shader.vertex_source, DEFAULT_VERTEX_SHADER);
        assert!(shader.fragment_source.contains("gl_FragColor"));
        let uniforms = shader.uniforms.lock().expect("uniforms");
        let tint = uniforms.floats.get("Tint").expect("Tint uniform");
        assert_eq!(tint[0], 1.0);
        assert!((tint[1] - 128.0 / 255.0).abs() < 1e-6);
        assert_eq!(tint[2], 0.0);
        assert!((tint[3] - 64.0 / 255.0).abs() < 1e-6);
        drop(uniforms);
        drop(shader);

        let supports: bool = shaders
            .get::<mlua::Function>("supports3DShaders")
            .expect("capability query")
            .call(())
            .expect("capability result");
        assert_eq!(supports, THREE_D_SHADERS_AVAILABLE);
        assert!(shaders.contains_key("load3DFragment").expect("load alias"));
    }

    #[test]
    fn portable_fragment_source_is_normalized_for_webgl_one() {
        let source = build_web_fragment_source(
            "#version 450\nprecision highp float;\nuniform sampler2D Texture;\n\
             void main() { gl_FragColor = texture(Texture, uv) * color; }",
        );
        assert!(!source.contains("#version"));
        assert_eq!(source.matches("precision ").count(), 1);
        assert!(source.contains("varying mediump vec2 uv;"));
        assert!(source.contains("varying mediump vec4 color;"));
        assert!(source.contains("texture2D(Texture, uv)"));
    }

    #[cfg(all(not(target_os = "emscripten"), feature = "vulkan"))]
    #[test]
    fn native_custom_fragment_preserves_display_space_contract_in_hdr_scene() {
        let (source, uniforms, textures) = build_runtime_fragment_source(
            "#version 450\nuniform float Gain;\n\
             void main() { gl_FragColor = texture2D(Texture, uv) * color * Gain; }",
        )
        .expect("runtime fragment source");
        assert!(source.contains("void __neolove_display_main()"));
        assert!(source.contains("__neolove_display_main();"));
        assert!(source.contains("f_color.rgb = pow(max(f_color.rgb"));
        assert!(!source.contains("gl_FragColor"));
        assert_eq!(uniforms, vec![("Gain".to_string(), 1)]);
        assert!(textures.is_empty());
    }

    #[test]
    fn uniform_storage_has_backend_portable_limits() {
        let mut uniforms = ShaderUniforms::default();
        for index in 0..MAX_SHADER_FLOAT_UNIFORMS {
            uniforms
                .set_floats(format!("Value{index}"), vec![index as f32])
                .expect("uniform within limit");
        }
        assert!(
            uniforms
                .set_floats("Overflow".to_string(), vec![0.0])
                .expect_err("uniform overflow")
                .contains("maximum")
        );
        uniforms
            .set_floats("Value0".to_string(), vec![42.0])
            .expect("replacing a uniform does not consume a slot");
    }
}
