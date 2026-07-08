use crate::platform::Color;
use crate::platform::{lock_platform_state, SharedPlatformState};
use crate::renderer::{last_frame_commands, SharedRenderState, SoftwareRenderer};
use base64::Engine as _;
use image::{Rgba, RgbaImage};
use mlua::{Lua, Table, UserData, UserDataMethods, Value, Variadic};
use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
#[cfg(not(target_os = "emscripten"))]
use rodio::{Decoder as AudioDecoder, Source};

#[derive(Debug)]
struct ImageAsset {
    #[cfg_attr(
        not(any(target_os = "emscripten", feature = "vulkan")),
        allow(dead_code)
    )]
    id: usize,
    image: RgbaImage,
    unloaded: bool,
    revision: u64,
    export_root: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub(crate) struct ImageHandle(Arc<Mutex<ImageAsset>>);

fn next_image_id() -> usize {
    static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug)]
struct SoundAsset {
    sample_rate: u32,
    channels: u16,
    samples: Vec<f32>,
    bytes: Vec<u8>,
    unloaded: bool,
    export_root: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub(crate) struct SoundHandle(Arc<Mutex<SoundAsset>>);

#[derive(Debug)]
pub(crate) struct AssetManager {
    resource_root: PathBuf,
    data_root: PathBuf,
    images: HashMap<PathBuf, Weak<Mutex<ImageAsset>>>,
    encoded_images: HashMap<String, Weak<Mutex<ImageAsset>>>,
    sounds: HashMap<PathBuf, Weak<Mutex<SoundAsset>>>,
}

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

fn has_explicit_base64_prefix(value: &str) -> bool {
    let value = value.trim();
    value
        .get(.."data:image/png;base64,".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:image/png;base64,"))
        || value
            .get(.."base64:".len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("base64:"))
}

/// Decode a raw base64 PNG or a `data:image/png;base64,...`/`base64:` value.
/// A non-base64-looking string returns `Ok(None)` so callers can treat it as a
/// normal path. Explicitly-prefixed malformed data returns a useful error.
pub(crate) fn decode_base64_png(value: &str) -> Result<Option<(String, Vec<u8>)>, String> {
    let value = value.trim();
    let explicit = has_explicit_base64_prefix(value);
    let payload = if value
        .get(.."data:image/png;base64,".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:image/png;base64,"))
    {
        &value["data:image/png;base64,".len()..]
    } else if value
        .get(.."base64:".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("base64:"))
    {
        &value["base64:".len()..]
    } else {
        value
    };
    let normalized: String = payload.chars().filter(|character| !character.is_ascii_whitespace()).collect();
    let looks_encoded = normalized.len() >= 12
        && normalized
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=' | b'-' | b'_'));
    if !explicit && !looks_encoded {
        return Ok(None);
    }

    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&normalized)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(&normalized))
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(&normalized))
        .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&normalized));
    let decoded = match decoded {
        Ok(decoded) => decoded,
        Err(error) if explicit => return Err(format!("invalid base64 PNG: {error}")),
        Err(_) => return Ok(None),
    };
    if !decoded.starts_with(PNG_SIGNATURE) {
        if explicit {
            return Err("base64 image data is not a PNG".to_string());
        }
        return Ok(None);
    }
    Ok(Some((normalized, decoded)))
}

fn lua_color4(lua: &Lua, color: Color) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("r", color.r)?;
    table.set("g", color.g)?;
    table.set("b", color.b)?;
    table.set("a", color.a)?;
    Ok(table)
}

fn color4_table_to_color(table: Table) -> mlua::Result<Color> {
    let r: f32 = table.get("r")?;
    let g: f32 = table.get("g")?;
    let b: f32 = table.get("b")?;
    let a: f32 = table.get("a")?;
    Ok(Color::rgba(
        r.clamp(0.0, 255.0) as u8,
        g.clamp(0.0, 255.0) as u8,
        b.clamp(0.0, 255.0) as u8,
        a.clamp(0.0, 255.0) as u8,
    ))
}

fn value_to_f32(value: &Value) -> Option<f32> {
    match value {
        Value::Integer(i) => Some(*i as f32),
        Value::Number(n) => Some(*n as f32),
        _ => None,
    }
}

fn parse_color_args(args: &[Value]) -> mlua::Result<Color> {
    match args {
        [Value::Table(t)] => color4_table_to_color(t.clone()),
        [r, g, b] => Ok(Color::rgba(
            value_to_f32(r)
                .ok_or_else(|| mlua::Error::external("invalid r"))?
                .clamp(0.0, 255.0) as u8,
            value_to_f32(g)
                .ok_or_else(|| mlua::Error::external("invalid g"))?
                .clamp(0.0, 255.0) as u8,
            value_to_f32(b)
                .ok_or_else(|| mlua::Error::external("invalid b"))?
                .clamp(0.0, 255.0) as u8,
            255,
        )),
        [r, g, b, a] => Ok(Color::rgba(
            value_to_f32(r)
                .ok_or_else(|| mlua::Error::external("invalid r"))?
                .clamp(0.0, 255.0) as u8,
            value_to_f32(g)
                .ok_or_else(|| mlua::Error::external("invalid g"))?
                .clamp(0.0, 255.0) as u8,
            value_to_f32(b)
                .ok_or_else(|| mlua::Error::external("invalid b"))?
                .clamp(0.0, 255.0) as u8,
            value_to_f32(a)
                .ok_or_else(|| mlua::Error::external("invalid a"))?
                .clamp(0.0, 255.0) as u8,
        )),
        _ => Err(mlua::Error::external(format!(
            "expected color4 table or r,g,b[,a], got {} args",
            args.len()
        ))),
    }
}

fn encode_wav_bytes(sample_rate: u32, channels: u16, samples: &[f32]) -> mlua::Result<Vec<u8>> {
    if channels == 0 {
        return Err(mlua::Error::external("channels must be >= 1"));
    }
    if !samples.len().is_multiple_of(channels as usize) {
        return Err(mlua::Error::external(
            "sample buffer length must be a multiple of channels",
        ));
    }

    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut bytes = Vec::new();
    {
        let cursor = Cursor::new(&mut bytes);
        let mut writer = hound::WavWriter::new(cursor, spec).map_err(mlua::Error::external)?;
        for &sample in samples {
            let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            writer.write_sample(value).map_err(mlua::Error::external)?;
        }
        writer.finalize().map_err(mlua::Error::external)?;
    }
    Ok(bytes)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn resolve_export_path(root: &Path, input: &str, extension: &str) -> mlua::Result<PathBuf> {
    let path = PathBuf::from(input);
    let candidate = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    let mut resolved = normalize_path(&candidate);
    match resolved.extension().and_then(|value| value.to_str()) {
        Some(current) if current.eq_ignore_ascii_case(extension) => {}
        Some(_) => {
            return Err(mlua::Error::external(format!(
                "export path must use .{extension}: {input}"
            )));
        }
        None => {
            resolved.set_extension(extension);
        }
    }
    Ok(resolved)
}

fn ensure_parent_dir(path: &Path) -> mlua::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(mlua::Error::external)?;
    }
    Ok(())
}

fn asset_io_error(action: &str, path: &Path, error: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::external(format!("failed to {action} '{}': {error}", path.display()))
}

fn asset_decode_error(kind: &str, path: &Path, error: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::external(format!(
        "failed to decode {kind} '{}': {error}",
        path.display()
    ))
}

impl ImageHandle {
    #[allow(dead_code)]
    pub(crate) fn from_rgba_image(image: RgbaImage) -> Self {
        Self(Arc::new(Mutex::new(ImageAsset {
            id: next_image_id(),
            image,
            unloaded: false,
            revision: 0,
            export_root: None,
        })))
    }

    #[cfg(any(target_os = "emscripten", feature = "vulkan"))]
    pub(crate) fn id(&self) -> mlua::Result<usize> {
        let image = self
            .0
            .lock()
            .map_err(|_| mlua::Error::external("image lock poisoned"))?;
        Ok(image.id)
    }

    pub(crate) fn with_image<R>(&self, f: impl FnOnce(&RgbaImage) -> R) -> mlua::Result<R> {
        let image = self
            .0
            .lock()
            .map_err(|_| mlua::Error::external("image lock poisoned"))?;
        if image.unloaded {
            return Err(mlua::Error::external("image is unloaded"));
        }
        Ok(f(&image.image))
    }

    #[cfg(test)]
    fn with_image_mut<R>(&self, f: impl FnOnce(&mut RgbaImage) -> R) -> mlua::Result<R> {
        let mut image = self
            .0
            .lock()
            .map_err(|_| mlua::Error::external("image lock poisoned"))?;
        if image.unloaded {
            return Err(mlua::Error::external("image is unloaded"));
        }
        Ok(f(&mut image.image))
    }

    pub(crate) fn dimensions(&self) -> mlua::Result<(u32, u32)> {
        self.with_image(|image| image.dimensions())
    }

    pub(crate) fn sample_rgba(&self, x: u32, y: u32) -> mlua::Result<[u8; 4]> {
        self.with_image(|image| {
            if x >= image.width() || y >= image.height() {
                None
            } else {
                Some(image.get_pixel(x, y).0)
            }
        })?
        .ok_or_else(|| mlua::Error::external("pixel out of bounds"))
    }

    pub(crate) fn unload(&self) {
        if let Ok(mut image) = self.0.lock() {
            image.image = RgbaImage::new(0, 0);
            image.unloaded = true;
            image.revision = image.revision.wrapping_add(1);
        }
    }

    pub(crate) fn ensure_uploaded(&self) -> mlua::Result<()> {
        self.with_image(|_| ())
    }

    #[cfg(any(target_os = "emscripten", feature = "vulkan"))]
    pub(crate) fn revision(&self) -> mlua::Result<u64> {
        let image = self
            .0
            .lock()
            .map_err(|_| mlua::Error::external("image lock poisoned"))?;
        if image.unloaded {
            return Err(mlua::Error::external("image is unloaded"));
        }
        Ok(image.revision)
    }

    #[cfg(all(not(target_os = "emscripten"), feature = "vulkan"))]
    pub(crate) fn clone_rgba_image(&self) -> mlua::Result<RgbaImage> {
        self.with_image(Clone::clone)
    }

    pub(crate) fn export_png(&self, user_path: &str) -> mlua::Result<()> {
        let (image, export_root) = {
            let image = self
                .0
                .lock()
                .map_err(|_| mlua::Error::external("image lock poisoned"))?;
            if image.unloaded {
                return Err(mlua::Error::external("image is unloaded"));
            }
            (image.image.clone(), image.export_root.clone())
        };
        let export_root = export_root
            .ok_or_else(|| mlua::Error::external("image export is unavailable for this handle"))?;
        let path = resolve_export_path(&export_root, user_path, "png")?;
        ensure_parent_dir(&path)
            .map_err(|error| asset_io_error("create export directory for image", &path, error))?;
        image::DynamicImage::ImageRgba8(image)
            .save_with_format(&path, image::ImageFormat::Png)
            .map_err(|error| asset_io_error("write png image", &path, error))
    }
}

impl SoundHandle {
    pub(crate) fn id(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }

    pub(crate) fn sample_rate(&self) -> mlua::Result<u32> {
        let sound = self
            .0
            .lock()
            .map_err(|_| mlua::Error::external("sound lock poisoned"))?;
        if sound.unloaded {
            return Err(mlua::Error::external("sound is unloaded"));
        }
        Ok(sound.sample_rate)
    }

    pub(crate) fn channels(&self) -> mlua::Result<u16> {
        let sound = self
            .0
            .lock()
            .map_err(|_| mlua::Error::external("sound lock poisoned"))?;
        if sound.unloaded {
            return Err(mlua::Error::external("sound is unloaded"));
        }
        Ok(sound.channels)
    }

    pub(crate) fn bytes(&self) -> mlua::Result<Vec<u8>> {
        let sound = self
            .0
            .lock()
            .map_err(|_| mlua::Error::external("sound lock poisoned"))?;
        if sound.unloaded {
            return Err(mlua::Error::external("sound is unloaded"));
        }
        Ok(sound.bytes.clone())
    }

    #[allow(dead_code)]
    pub(crate) fn with_samples<R>(
        &self,
        f: impl FnOnce(u32, u16, &[f32]) -> mlua::Result<R>,
    ) -> mlua::Result<R> {
        let sound = self
            .0
            .lock()
            .map_err(|_| mlua::Error::external("sound lock poisoned"))?;
        if sound.unloaded {
            return Err(mlua::Error::external("sound is unloaded"));
        }
        f(sound.sample_rate, sound.channels, &sound.samples)
    }

    pub(crate) fn unload(&self) {
        if let Ok(mut sound) = self.0.lock() {
            sound.samples.clear();
            sound.bytes.clear();
            sound.unloaded = true;
        }
    }

    pub(crate) fn ensure_uploaded(&self) -> mlua::Result<()> {
        let sound = self
            .0
            .lock()
            .map_err(|_| mlua::Error::external("sound lock poisoned"))?;
        if sound.unloaded {
            return Err(mlua::Error::external("sound is unloaded"));
        }
        Ok(())
    }

    pub(crate) fn export_wav(&self, user_path: &str) -> mlua::Result<()> {
        let (bytes, export_root) = {
            let sound = self
                .0
                .lock()
                .map_err(|_| mlua::Error::external("sound lock poisoned"))?;
            if sound.unloaded {
                return Err(mlua::Error::external("sound is unloaded"));
            }
            let bytes = if sound.sample_rate > 0 && sound.channels > 0 && !sound.samples.is_empty() {
                encode_wav_bytes(sound.sample_rate, sound.channels, &sound.samples)?
            } else if sound.bytes.starts_with(b"RIFF") {
                sound.bytes.clone()
            } else {
                return Err(mlua::Error::external(
                    "decoded sample data is unavailable for WAV export on this target",
                ));
            };
            (bytes, sound.export_root.clone())
        };
        let export_root = export_root
            .ok_or_else(|| mlua::Error::external("sound export is unavailable for this handle"))?;
        let path = resolve_export_path(&export_root, user_path, "wav")?;
        ensure_parent_dir(&path)
            .map_err(|error| asset_io_error("create export directory for sound", &path, error))?;
        std::fs::write(&path, bytes).map_err(|error| asset_io_error("write wav file", &path, error))
    }
}

impl UserData for ImageHandle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("width", |_lua, this, ()| Ok(this.dimensions()?.0));
        methods.add_method("height", |_lua, this, ()| Ok(this.dimensions()?.1));
        methods.add_method("size", |_lua, this, ()| this.dimensions());
        methods.add_method("getPixel", |lua, this, (x, y): (u32, u32)| {
            let [r, g, b, a] = this.sample_rgba(x, y)?;
            lua_color4(lua, Color::rgba(r, g, b, a))
        });
        methods.add_method("setPixel", |_lua, this, args: Variadic<Value>| {
            if args.len() < 3 {
                return Err(mlua::Error::external(
                    "setPixel expects (x, y, color) or (x, y, r, g, b[, a])",
                ));
            }
            let x = value_to_f32(&args[0])
                .ok_or_else(|| mlua::Error::external("setPixel expects numeric x as arg1"))?;
            let y = value_to_f32(&args[1])
                .ok_or_else(|| mlua::Error::external("setPixel expects numeric y as arg2"))?;
            if x < 0.0 || y < 0.0 {
                return Err(mlua::Error::external("pixel out of bounds"));
            }
            let color = parse_color_args(&args[2..])?;
            let mut image = this
                .0
                .lock()
                .map_err(|_| mlua::Error::external("image lock poisoned"))?;
            if image.unloaded {
                return Err(mlua::Error::external("image is unloaded"));
            }
            let x = x as u32;
            let y = y as u32;
            if x >= image.image.width() || y >= image.image.height() {
                return Err(mlua::Error::external("pixel out of bounds"));
            }
            image
                .image
                .put_pixel(x, y, Rgba([color.r, color.g, color.b, color.a]));
            image.revision = image.revision.wrapping_add(1);
            Ok(())
        });
        methods.add_method("fill", |_lua, this, args: Variadic<Value>| {
            let color = parse_color_args(&args)?;
            let mut image = this
                .0
                .lock()
                .map_err(|_| mlua::Error::external("image lock poisoned"))?;
            if image.unloaded {
                return Err(mlua::Error::external("image is unloaded"));
            }
            {
                for pixel in image.image.pixels_mut() {
                    *pixel = Rgba([color.r, color.g, color.b, color.a]);
                }
            }
            image.revision = image.revision.wrapping_add(1);
            Ok(())
        });
        methods.add_method("upload", |_lua, this, ()| this.ensure_uploaded());
        methods.add_method("export", |_lua, this, path: String| this.export_png(&path));
        methods.add_method("save", |_lua, this, path: String| this.export_png(&path));
        methods.add_method("unload", |_lua, this, ()| {
            this.unload();
            Ok(())
        });
        methods.add_method("isUnloaded", |_lua, this, ()| {
            let image = this
                .0
                .lock()
                .map_err(|_| mlua::Error::external("image lock poisoned"))?;
            Ok(image.unloaded)
        });
    }
}

impl UserData for SoundHandle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("sampleRate", |_lua, this, ()| this.sample_rate());
        methods.add_method("channels", |_lua, this, ()| this.channels());
        methods.add_method("len", |_lua, this, ()| {
            let sound = this
                .0
                .lock()
                .map_err(|_| mlua::Error::external("sound lock poisoned"))?;
            if sound.unloaded {
                return Err(mlua::Error::external("sound is unloaded"));
            }
            Ok(sound.samples.len() as u32)
        });
        methods.add_method("getSample", |_lua, this, index: i64| {
            if index < 0 {
                return Err(mlua::Error::external("sample index out of bounds"));
            }
            let sound = this
                .0
                .lock()
                .map_err(|_| mlua::Error::external("sound lock poisoned"))?;
            if sound.unloaded {
                return Err(mlua::Error::external("sound is unloaded"));
            }
            sound
                .samples
                .get(index as usize)
                .copied()
                .ok_or_else(|| mlua::Error::external("sample index out of bounds"))
        });
        methods.add_method("setSample", |_lua, this, (index, value): (i64, f32)| {
            if index < 0 {
                return Err(mlua::Error::external("sample index out of bounds"));
            }
            let mut sound = this
                .0
                .lock()
                .map_err(|_| mlua::Error::external("sound lock poisoned"))?;
            if sound.unloaded {
                return Err(mlua::Error::external("sound is unloaded"));
            }
            let index = index as usize;
            if index >= sound.samples.len() {
                return Err(mlua::Error::external("sample index out of bounds"));
            }
            sound.samples[index] = value.clamp(-1.0, 1.0);
            sound.bytes = encode_wav_bytes(sound.sample_rate, sound.channels, &sound.samples)?;
            Ok(())
        });
        methods.add_method("upload", |_lua, this, ()| this.ensure_uploaded());
        methods.add_method("export", |_lua, this, path: String| this.export_wav(&path));
        methods.add_method("save", |_lua, this, path: String| this.export_wav(&path));
        methods.add_method("unload", |_lua, this, ()| {
            this.unload();
            Ok(())
        });
        methods.add_method("isUnloaded", |_lua, this, ()| {
            let sound = this
                .0
                .lock()
                .map_err(|_| mlua::Error::external("sound lock poisoned"))?;
            Ok(sound.unloaded)
        });
    }
}

impl AssetManager {
    #[cfg(test)]
    pub(crate) fn new(env_root: PathBuf) -> Self {
        Self::with_data_root(env_root.clone(), env_root)
    }

    pub(crate) fn with_data_root(resource_root: PathBuf, data_root: PathBuf) -> Self {
        Self {
            resource_root,
            data_root,
            images: HashMap::new(),
            encoded_images: HashMap::new(),
            sounds: HashMap::new(),
        }
    }

    fn resolve_path(&self, user_path: &str) -> PathBuf {
        let path = PathBuf::from(user_path);
        if path.is_absolute() {
            return normalize_path(&path);
        }

        let project_relative = user_path.starts_with("./")
            || user_path.starts_with("../")
            || user_path.starts_with("assets/")
            || user_path.starts_with("assets\\");
        let data_path = if project_relative {
            self.data_root.join(&path)
        } else {
            self.data_root.join("assets").join(&path)
        };
        if data_path.exists() {
            return normalize_path(&data_path);
        }

        let resource_path = if project_relative {
            self.resource_root.join(path)
        } else {
            self.resource_root.join("assets").join(path)
        };
        normalize_path(&resource_path)
    }

    fn canonical_for_cache(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    pub(crate) fn load_image(&mut self, user_path: &str) -> mlua::Result<ImageHandle> {
        if has_explicit_base64_prefix(user_path) {
            return self.load_base64_image(user_path);
        }
        let resolved = self.resolve_path(user_path);
        if !resolved.exists() {
            if let Some((cache_key, bytes)) =
                decode_base64_png(user_path).map_err(mlua::Error::external)?
            {
                return self.load_decoded_png(cache_key, bytes);
            }
        }
        let cache_key = Self::canonical_for_cache(&resolved);
        if let Some(existing) = self.images.get(&cache_key).and_then(Weak::upgrade) {
            let unloaded = existing
                .lock()
                .map_err(|_| mlua::Error::external("image lock poisoned"))?
                .unloaded;
            if !unloaded {
                return Ok(ImageHandle(existing));
            }
        }

        let bytes = std::fs::read(&resolved)
            .map_err(|error| asset_io_error("read image", &resolved, error))?;
        let image = image::load_from_memory(&bytes)
            .map_err(|error| asset_decode_error("image", &resolved, error))?
            .to_rgba8();
        let handle = Arc::new(Mutex::new(ImageAsset {
            id: next_image_id(),
            image,
            unloaded: false,
            revision: 0,
            export_root: Some(self.data_root.clone()),
        }));
        self.images.insert(cache_key, Arc::downgrade(&handle));
        Ok(ImageHandle(handle))
    }

    pub(crate) fn load_base64_image(&mut self, encoded: &str) -> mlua::Result<ImageHandle> {
        let explicitly_encoded;
        let encoded = if has_explicit_base64_prefix(encoded) {
            encoded
        } else {
            explicitly_encoded = format!("base64:{encoded}");
            &explicitly_encoded
        };
        let Some((cache_key, bytes)) = decode_base64_png(encoded).map_err(mlua::Error::external)?
        else {
            return Err(mlua::Error::external(
                "expected a base64-encoded PNG or data:image/png;base64 URI",
            ));
        };
        self.load_decoded_png(cache_key, bytes)
    }

    fn load_decoded_png(
        &mut self,
        cache_key: String,
        bytes: Vec<u8>,
    ) -> mlua::Result<ImageHandle> {
        if let Some(existing) = self
            .encoded_images
            .get(&cache_key)
            .and_then(Weak::upgrade)
        {
            let unloaded = existing
                .lock()
                .map_err(|_| mlua::Error::external("image lock poisoned"))?
                .unloaded;
            if !unloaded {
                return Ok(ImageHandle(existing));
            }
        }
        let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
            .map_err(|error| mlua::Error::external(format!("failed to decode base64 PNG: {error}")))?
            .to_rgba8();
        let handle = Arc::new(Mutex::new(ImageAsset {
            id: next_image_id(),
            image,
            unloaded: false,
            revision: 0,
            export_root: Some(self.data_root.clone()),
        }));
        self.encoded_images
            .insert(cache_key, Arc::downgrade(&handle));
        Ok(ImageHandle(handle))
    }

    pub(crate) fn new_image(&mut self, width: u16, height: u16, color: Color) -> ImageHandle {
        let pixel = Rgba([color.r, color.g, color.b, color.a]);
        let image = RgbaImage::from_pixel(width as u32, height as u32, pixel);
        ImageHandle(Arc::new(Mutex::new(ImageAsset {
            id: next_image_id(),
            image,
            unloaded: false,
            revision: 0,
            export_root: Some(self.data_root.clone()),
        })))
    }

    fn image_from_rgba(&self, image: RgbaImage) -> ImageHandle {
        ImageHandle(Arc::new(Mutex::new(ImageAsset {
            id: next_image_id(),
            image,
            unloaded: false,
            revision: 0,
            export_root: Some(self.data_root.clone()),
        })))
    }

    pub(crate) fn load_sound(&mut self, user_path: &str) -> mlua::Result<SoundHandle> {
        let resolved = self.resolve_path(user_path);
        let cache_key = Self::canonical_for_cache(&resolved);
        if let Some(existing) = self.sounds.get(&cache_key).and_then(Weak::upgrade) {
            let unloaded = existing
                .lock()
                .map_err(|_| mlua::Error::external("sound lock poisoned"))?
                .unloaded;
            if !unloaded {
                return Ok(SoundHandle(existing));
            }
        }

        let file_bytes = std::fs::read(&resolved)
            .map_err(|error| asset_io_error("read sound", &resolved, error))?;
        let extension = resolved
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        #[cfg(target_os = "emscripten")]
        let (sample_rate, channels, samples) = {
            if !matches!(
                extension.as_str(),
                "wav" | "mp3" | "ogg" | "oga" | "flac" | "aac" | "m4a" | "aiff" | "aif"
            ) {
                return Err(asset_decode_error(
                    "sound",
                    &resolved,
                    format!("unsupported browser audio format '.{extension}'"),
                ));
            }
            // The browser decodes encoded audio through WebAudio. Editable
            // sample data is intentionally absent in this target.
            (0, 0, Vec::new())
        };

        #[cfg(not(target_os = "emscripten"))]
        let (sample_rate, channels, samples) = {
            if !matches!(extension.as_str(), "wav" | "mp3" | "ogg" | "oga" | "flac") {
                return Err(asset_decode_error(
                    "sound",
                    &resolved,
                    format!(
                        "unsupported audio format '.{extension}'; supported formats are WAV, MP3, OGG/Vorbis, and FLAC"
                    ),
                ));
            }
            let decoder = AudioDecoder::new(Cursor::new(file_bytes.clone())).map_err(|error| {
                asset_decode_error(
                    if extension == "wav" { "wav file" } else { "sound" },
                    &resolved,
                    error,
                )
            })?;
            let sample_rate = decoder.sample_rate();
            let channels = decoder.channels();
            let samples = decoder.convert_samples::<f32>().collect();
            (sample_rate, channels, samples)
        };

        let handle = Arc::new(Mutex::new(SoundAsset {
            sample_rate,
            channels,
            samples,
            bytes: file_bytes,
            unloaded: false,
            export_root: Some(self.data_root.clone()),
        }));
        self.sounds.insert(cache_key, Arc::downgrade(&handle));
        Ok(SoundHandle(handle))
    }

    pub(crate) fn new_sound(
        &mut self,
        sample_rate: u32,
        channels: u16,
        samples: Vec<f32>,
    ) -> mlua::Result<SoundHandle> {
        let bytes = encode_wav_bytes(sample_rate, channels, &samples)?;
        Ok(SoundHandle(Arc::new(Mutex::new(SoundAsset {
            sample_rate,
            channels,
            samples,
            bytes,
            unloaded: false,
            export_root: Some(self.data_root.clone()),
        }))))
    }

    pub(crate) fn unload_image_path(&mut self, user_path: &str) -> bool {
        if let Ok(Some((cache_key, _))) = decode_base64_png(user_path) {
            if let Some(handle) = self
                .encoded_images
                .remove(&cache_key)
                .and_then(|weak| weak.upgrade())
            {
                ImageHandle(handle).unload();
                return true;
            }
        }
        let resolved = self.resolve_path(user_path);
        let Some(handle) = self
            .images
            .remove(&Self::canonical_for_cache(&resolved))
            .and_then(|weak| weak.upgrade())
        else {
            return false;
        };
        ImageHandle(handle).unload();
        true
    }

    pub(crate) fn unload_sound_path(&mut self, user_path: &str) -> bool {
        let resolved = self.resolve_path(user_path);
        let Some(handle) = self
            .sounds
            .remove(&Self::canonical_for_cache(&resolved))
            .and_then(|weak| weak.upgrade())
        else {
            return false;
        };
        SoundHandle(handle).unload();
        true
    }

    pub(crate) fn gc(&mut self) -> (usize, usize) {
        let before_images = self.images.len() + self.encoded_images.len();
        let before_sounds = self.sounds.len();
        self.images.retain(|_, weak| weak.strong_count() > 0);
        self.encoded_images
            .retain(|_, weak| weak.strong_count() > 0);
        self.sounds.retain(|_, weak| weak.strong_count() > 0);
        (
            before_images - self.images.len() - self.encoded_images.len(),
            before_sounds - self.sounds.len(),
        )
    }
}

pub(crate) fn add_assets_module_with_data_root(
    lua: &Lua,
    resource_root: PathBuf,
    data_root: PathBuf,
    platform: SharedPlatformState,
    render_state: SharedRenderState,
) -> mlua::Result<()> {
    let manager = Arc::new(Mutex::new(AssetManager::with_data_root(
        resource_root,
        data_root,
    )));
    let assets = lua.create_table()?;

    {
        let manager = manager.clone();
        assets.set(
            "loadImage",
            lua.create_function(move |lua, path: String| {
                let handle = manager
                    .lock()
                    .map_err(|_| mlua::Error::external("asset manager lock poisoned"))?
                    .load_image(&path)?;
                lua.create_userdata(handle)
            })?,
        )?;
    }

    {
        let manager = manager.clone();
        assets.set(
            "snapPhoto",
            lua.create_function(move |lua, (x, y, x2, y2): (f32, f32, f32, f32)| {
                if ![x, y, x2, y2].iter().all(|value| value.is_finite()) {
                    return Err(mlua::Error::external(
                        "snapPhoto coordinates must be finite numbers",
                    ));
                }
                if x2 <= x || y2 <= y {
                    return Err(mlua::Error::external(
                        "snapPhoto expects x2 > x and y2 > y",
                    ));
                }

                let window = lock_platform_state(&platform).window();
                let screen_width = window.width.max(1.0).ceil() as u32;
                let screen_height = window.height.max(1.0).ceil() as u32;
                let left = x.floor().clamp(0.0, screen_width as f32) as u32;
                let top = y.floor().clamp(0.0, screen_height as f32) as u32;
                let right = x2.ceil().clamp(0.0, screen_width as f32) as u32;
                let bottom = y2.ceil().clamp(0.0, screen_height as f32) as u32;
                if right <= left || bottom <= top {
                    return Err(mlua::Error::external(
                        "snapPhoto rectangle is outside the window",
                    ));
                }

                let commands = last_frame_commands(&render_state)
                    .map_err(mlua::Error::external)?
                    .ok_or_else(|| {
                        mlua::Error::external(
                            "snapPhoto is unavailable before the first frame is rendered",
                        )
                    })?;
                let mut renderer = SoftwareRenderer::new(screen_width, screen_height);
                renderer
                    .render_commands(&platform, commands.as_ref())
                    .map_err(|error| {
                        mlua::Error::external(format!("snapPhoto failed to render frame: {error}"))
                    })?;

                let photo_width = right - left;
                let photo_height = bottom - top;
                let mut photo = RgbaImage::new(photo_width, photo_height);
                let source = renderer.pixels();
                let destination = photo.as_flat_samples_mut().samples;
                for row in 0..photo_height {
                    let source_start = (((top + row) * screen_width + left) * 4) as usize;
                    let source_end = source_start + photo_width as usize * 4;
                    let destination_start = (row * photo_width * 4) as usize;
                    destination[destination_start..destination_start + photo_width as usize * 4]
                        .copy_from_slice(&source[source_start..source_end]);
                }
                let handle = manager
                    .lock()
                    .map_err(|_| mlua::Error::external("asset manager lock poisoned"))?
                    .image_from_rgba(photo);
                lua.create_userdata(handle)
            })?,
        )?;
    }

    {
        let manager = manager.clone();
        assets.set(
            "loadImageBase64",
            lua.create_function(move |lua, encoded: String| {
                let handle = manager
                    .lock()
                    .map_err(|_| mlua::Error::external("asset manager lock poisoned"))?
                    .load_base64_image(&encoded)?;
                lua.create_userdata(handle)
            })?,
        )?;
    }

    {
        let manager = manager.clone();
        assets.set(
            "newImage",
            lua.create_function(move |lua, (w, h, color): (u32, u32, Option<Table>)| {
                let color = match color {
                    Some(table) => color4_table_to_color(table)?,
                    None => Color::WHITE,
                };
                let handle = manager
                    .lock()
                    .map_err(|_| mlua::Error::external("asset manager lock poisoned"))?
                    .new_image(
                        w.min(u16::MAX as u32) as u16,
                        h.min(u16::MAX as u32) as u16,
                        color,
                    );
                lua.create_userdata(handle)
            })?,
        )?;
    }

    {
        let manager = manager.clone();
        assets.set(
            "loadSound",
            lua.create_function(move |lua, path: String| {
                let handle = manager
                    .lock()
                    .map_err(|_| mlua::Error::external("asset manager lock poisoned"))?
                    .load_sound(&path)?;
                lua.create_userdata(handle)
            })?,
        )?;
    }

    {
        let manager = manager.clone();
        assets.set(
            "newSound",
            lua.create_function(
                move |lua, (sample_rate, channels, len, fill): (u32, u16, u32, Option<f32>)| {
                    let fill = fill.unwrap_or(0.0).clamp(-1.0, 1.0);
                    let mut samples = vec![fill; len as usize];
                    if channels > 0 && !samples.len().is_multiple_of(channels as usize) {
                        let remainder = samples.len() % channels as usize;
                        samples.extend(std::iter::repeat_n(fill, channels as usize - remainder));
                    }
                    let handle = manager
                        .lock()
                        .map_err(|_| mlua::Error::external("asset manager lock poisoned"))?
                        .new_sound(sample_rate, channels, samples)?;
                    lua.create_userdata(handle)
                },
            )?,
        )?;
    }

    {
        let manager = manager.clone();
        assets.set(
            "unloadImage",
            lua.create_function(move |_lua, value: Value| match value {
                Value::String(path) => {
                    let path = path.to_str()?.to_string();
                    let mut manager = manager
                        .lock()
                        .map_err(|_| mlua::Error::external("asset manager lock poisoned"))?;
                    Ok(manager.unload_image_path(path.as_str()))
                }
                Value::UserData(user_data) => {
                    if let Ok(handle) = user_data.borrow::<ImageHandle>() {
                        handle.unload();
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                }
                _ => Ok(false),
            })?,
        )?;
    }

    {
        let manager = manager.clone();
        assets.set(
            "unloadSound",
            lua.create_function(move |_lua, value: Value| match value {
                Value::String(path) => {
                    let path = path.to_str()?.to_string();
                    let mut manager = manager
                        .lock()
                        .map_err(|_| mlua::Error::external("asset manager lock poisoned"))?;
                    Ok(manager.unload_sound_path(path.as_str()))
                }
                Value::UserData(user_data) => {
                    if let Ok(handle) = user_data.borrow::<SoundHandle>() {
                        handle.unload();
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                }
                _ => Ok(false),
            })?,
        )?;
    }

    {
        let manager = manager.clone();
        assets.set(
            "gc",
            lua.create_function(move |_lua, ()| {
                let (images, sounds) = manager
                    .lock()
                    .map_err(|_| mlua::Error::external("asset manager lock poisoned"))?
                    .gc();
                Ok((images as u32, sounds as u32))
            })?,
        )?;
    }

    lua.globals().set("assets", assets)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("neolove_{name}_{unique}"))
    }

    fn encoded_test_png() -> String {
        let mut image = RgbaImage::new(2, 1);
        image.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        image.put_pixel(1, 0, Rgba([0, 255, 0, 128]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageOutputFormat::Png)
            .expect("encode test png");
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn load_image_accepts_raw_and_data_uri_base64_png() -> mlua::Result<()> {
        let root = temp_root("asset_base64_png");
        let mut manager = AssetManager::new(root);
        let encoded = encoded_test_png();

        let raw = manager.load_image(&encoded)?;
        assert_eq!(raw.dimensions()?, (2, 1));
        assert_eq!(raw.sample_rgba(0, 0)?, [255, 0, 0, 255]);
        assert_eq!(raw.sample_rgba(1, 0)?, [0, 255, 0, 128]);

        let uri = manager.load_image(&format!("data:image/png;base64,{encoded}"))?;
        assert!(Arc::ptr_eq(&raw.0, &uri.0), "equivalent encodings should share the cache");
        Ok(())
    }

    #[test]
    fn explicit_base64_image_rejects_non_png_data() {
        let root = temp_root("asset_base64_not_png");
        let mut manager = AssetManager::new(root);
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"not a png");
        let error = manager
            .load_base64_image(&encoded)
            .expect_err("non-PNG data must fail")
            .to_string();
        assert!(error.contains("not a PNG"));
    }

    #[test]
    fn snap_photo_returns_clipped_region_as_image_handle() -> mlua::Result<()> {
        let root = temp_root("asset_snap_photo");
        fs::create_dir_all(&root).map_err(mlua::Error::external)?;
        let platform = crate::platform::new_shared_platform_state();
        {
            let mut state = lock_platform_state(&platform);
            state.set_window(crate::platform::WindowState {
                width: 4.0,
                height: 3.0,
            });
            state.set_clear_color(Color::rgba(10, 20, 30, 255));
        }
        let render_state = crate::renderer::new_shared_render_state();
        render_state
            .lock()
            .map_err(|_| mlua::Error::external("render state lock poisoned"))?
            .queue(crate::renderer::DrawCommand::Rect {
                x: 1.0,
                y: 1.0,
                w: 2.0,
                h: 1.0,
                rotation: 0.0,
                offset: crate::renderer::Vec2::default(),
                color: Color::rgba(200, 40, 50, 255),
                shader: None,
            });
        let mut renderer = SoftwareRenderer::new(4, 3);
        renderer
            .render(&platform, &render_state)
            .map_err(mlua::Error::external)?;

        let lua = Lua::new();
        add_assets_module_with_data_root(
            &lua,
            root.clone(),
            root.clone(),
            platform,
            render_state,
        )?;
        let assets: Table = lua.globals().get("assets")?;
        let snap: mlua::Function = assets.get("snapPhoto")?;
        let userdata: mlua::AnyUserData = snap.call((-2.0, 0.0, 3.0, 2.0))?;
        let photo = userdata.borrow::<ImageHandle>()?;
        assert_eq!(photo.dimensions()?, (3, 2));
        assert_eq!(photo.sample_rgba(0, 0)?, [10, 20, 30, 255]);
        assert_eq!(photo.sample_rgba(1, 1)?, [200, 40, 50, 255]);

        fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn image_export_writes_png_and_appends_extension() -> mlua::Result<()> {
        let root = temp_root("asset_image_export");
        fs::create_dir_all(&root).map_err(mlua::Error::external)?;

        let mut manager = AssetManager::new(root.clone());
        let handle = manager.new_image(2, 1, Color::rgba(0, 0, 0, 0));
        handle.with_image_mut(|image| {
            image.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
            image.put_pixel(1, 0, Rgba([0, 255, 0, 255]));
        })?;

        handle.export_png("exports/test_image")?;

        let exported = root.join("exports/test_image.png");
        assert!(exported.exists());
        let decoded = image::open(&exported)
            .map_err(mlua::Error::external)?
            .to_rgba8();
        assert_eq!(decoded.dimensions(), (2, 1));
        assert_eq!(decoded.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(decoded.get_pixel(1, 0).0, [0, 255, 0, 255]);

        fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn sound_export_writes_wav_and_appends_extension() -> mlua::Result<()> {
        let root = temp_root("asset_sound_export");
        fs::create_dir_all(&root).map_err(mlua::Error::external)?;

        let mut manager = AssetManager::new(root.clone());
        let handle = manager.new_sound(22_050, 1, vec![0.0, 0.5, -0.5, 0.25])?;
        handle.export_wav("exports/test_sound")?;

        let exported = root.join("exports/test_sound.wav");
        assert!(exported.exists());
        let mut reader = hound::WavReader::open(&exported).map_err(mlua::Error::external)?;
        let spec = reader.spec();
        assert_eq!(spec.sample_rate, 22_050);
        assert_eq!(spec.channels, 1);
        let samples: Vec<i16> = reader
            .samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(mlua::Error::external)?;
        assert_eq!(samples.len(), 4);

        fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn separate_data_root_receives_exports_and_supports_reload() -> mlua::Result<()> {
        let root = temp_root("asset_data_root");
        let resource_root = root.join("project");
        let data_root = root.join("game_data");
        fs::create_dir_all(&resource_root).map_err(mlua::Error::external)?;
        fs::create_dir_all(&data_root).map_err(mlua::Error::external)?;

        let mut manager = AssetManager::with_data_root(resource_root, data_root.clone());
        let image = manager.new_image(2, 2, Color::rgba(10, 20, 30, 255));
        let sound = manager.new_sound(8_000, 1, vec![0.0, 0.25, -0.25])?;
        image.export_png("runtime/image")?;
        sound.export_wav("runtime/sound")?;

        assert!(data_root.join("runtime/image.png").is_file());
        assert!(data_root.join("runtime/sound.wav").is_file());
        assert_eq!(
            manager.load_image("./runtime/image.png")?.dimensions()?,
            (2, 2)
        );
        assert_eq!(
            manager.load_sound("./runtime/sound.wav")?.sample_rate()?,
            8_000
        );

        fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn export_allows_paths_outside_project_root() -> mlua::Result<()> {
        let root = temp_root("asset_export_escape");
        let outside = temp_root("asset_export_outside");
        fs::create_dir_all(&root).map_err(mlua::Error::external)?;

        let mut manager = AssetManager::new(root.clone());
        let image = manager.new_image(1, 1, Color::WHITE);
        let sound = manager.new_sound(8_000, 1, vec![0.0])?;

        image.export_png(outside.join("escape_image").to_string_lossy().as_ref())?;
        sound.export_wav(outside.join("escape_sound").to_string_lossy().as_ref())?;
        assert!(outside.join("escape_image.png").is_file());
        assert!(outside.join("escape_sound.wav").is_file());

        fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        fs::remove_dir_all(outside).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn load_image_error_mentions_resolved_path() -> mlua::Result<()> {
        let root = temp_root("asset_missing_image");
        fs::create_dir_all(root.join("assets")).map_err(mlua::Error::external)?;

        let mut manager = AssetManager::new(root.clone());
        let missing_path = root.join("assets").join("missing.png");
        let error = manager
            .load_image("missing.png")
            .expect_err("missing image should return an error")
            .to_string();

        assert!(error.contains("failed to read image"));
        assert!(error.contains(missing_path.to_string_lossy().as_ref()));

        fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }

    #[test]
    fn load_sound_error_mentions_resolved_path() -> mlua::Result<()> {
        let root = temp_root("asset_invalid_sound");
        let assets_dir = root.join("assets");
        fs::create_dir_all(&assets_dir).map_err(mlua::Error::external)?;

        let invalid_path = assets_dir.join("broken.wav");
        fs::write(&invalid_path, b"not a wav").map_err(mlua::Error::external)?;

        let mut manager = AssetManager::new(root.clone());
        let error = manager
            .load_sound("broken.wav")
            .expect_err("invalid wav should return an error")
            .to_string();

        assert!(error.contains("failed to decode wav file"));
        assert!(error.contains(invalid_path.to_string_lossy().as_ref()));

        fs::remove_dir_all(root).map_err(mlua::Error::external)?;
        Ok(())
    }
}
