use crate::platform::Color;
use image::{Rgba, RgbaImage};
use mlua::{Lua, Table, UserData, UserDataMethods, Value, Variadic};
use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

#[derive(Debug)]
struct ImageAsset {
    image: RgbaImage,
    unloaded: bool,
    revision: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct ImageHandle(Arc<Mutex<ImageAsset>>);

#[derive(Debug)]
struct SoundAsset {
    sample_rate: u32,
    channels: u16,
    samples: Vec<f32>,
    bytes: Vec<u8>,
    unloaded: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct SoundHandle(Arc<Mutex<SoundAsset>>);

#[derive(Debug)]
pub(crate) struct AssetManager {
    env_root: PathBuf,
    images: HashMap<PathBuf, Weak<Mutex<ImageAsset>>>,
    sounds: HashMap<PathBuf, Weak<Mutex<SoundAsset>>>,
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
    if samples.len() % channels as usize != 0 {
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

impl ImageHandle {
    pub(crate) fn from_rgba_image(image: RgbaImage) -> Self {
        Self(Arc::new(Mutex::new(ImageAsset {
            image,
            unloaded: false,
            revision: 0,
        })))
    }

    pub(crate) fn id(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
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

    pub(crate) fn clone_rgba_image(&self) -> mlua::Result<RgbaImage> {
        self.with_image(Clone::clone)
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
            let updated = this.with_image_mut(|image| {
                let x = x as u32;
                let y = y as u32;
                if x >= image.width() || y >= image.height() {
                    return None;
                }
                image.put_pixel(x, y, Rgba([color.r, color.g, color.b, color.a]));
                Some(())
            })?;
            updated.ok_or_else(|| mlua::Error::external("pixel out of bounds"))?;
            if let Ok(mut image) = this.0.lock() {
                image.revision = image.revision.wrapping_add(1);
            }
            Ok(())
        });
        methods.add_method("fill", |_lua, this, args: Variadic<Value>| {
            let color = parse_color_args(&args)?;
            this.with_image_mut(|image| {
                for pixel in image.pixels_mut() {
                    *pixel = Rgba([color.r, color.g, color.b, color.a]);
                }
            })?;
            if let Ok(mut image) = this.0.lock() {
                image.revision = image.revision.wrapping_add(1);
            }
            Ok(())
        });
        methods.add_method("upload", |_lua, this, ()| this.ensure_uploaded());
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
    pub(crate) fn new(env_root: PathBuf) -> Self {
        Self {
            env_root,
            images: HashMap::new(),
            sounds: HashMap::new(),
        }
    }

    fn resolve_path(&self, user_path: &str) -> PathBuf {
        let path = PathBuf::from(user_path);
        if path.is_absolute() {
            return path;
        }
        if user_path.starts_with("./")
            || user_path.starts_with("../")
            || user_path.starts_with("assets/")
            || user_path.starts_with("assets\\")
        {
            return self.env_root.join(path);
        }
        self.env_root.join("assets").join(path)
    }

    fn canonical_for_cache(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    pub(crate) fn load_image(&mut self, user_path: &str) -> mlua::Result<ImageHandle> {
        let resolved = self.resolve_path(user_path);
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

        let bytes = std::fs::read(&resolved).map_err(mlua::Error::external)?;
        let image = image::load_from_memory(&bytes)
            .map_err(mlua::Error::external)?
            .to_rgba8();
        let handle = Arc::new(Mutex::new(ImageAsset {
            image,
            unloaded: false,
            revision: 0,
        }));
        self.images.insert(cache_key, Arc::downgrade(&handle));
        Ok(ImageHandle(handle))
    }

    pub(crate) fn new_image(&mut self, width: u16, height: u16, color: Color) -> ImageHandle {
        let pixel = Rgba([color.r, color.g, color.b, color.a]);
        let image = RgbaImage::from_pixel(width as u32, height as u32, pixel);
        ImageHandle(Arc::new(Mutex::new(ImageAsset {
            image,
            unloaded: false,
            revision: 0,
        })))
    }

    pub(crate) fn load_sound_wav(&mut self, user_path: &str) -> mlua::Result<SoundHandle> {
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

        let file_bytes = std::fs::read(&resolved).map_err(mlua::Error::external)?;
        let mut reader = hound::WavReader::new(Cursor::new(file_bytes.as_slice()))
            .map_err(mlua::Error::external)?;
        let spec = reader.spec();
        let mut samples = Vec::new();
        match spec.sample_format {
            hound::SampleFormat::Float => {
                for sample in reader.samples::<f32>() {
                    samples.push(sample.map_err(mlua::Error::external)?.clamp(-1.0, 1.0));
                }
            }
            hound::SampleFormat::Int => {
                let max = ((1u64 << spec.bits_per_sample.saturating_sub(1)) as f32) - 1.0;
                if spec.bits_per_sample <= 16 {
                    for sample in reader.samples::<i16>() {
                        samples.push(
                            (sample.map_err(mlua::Error::external)? as f32 / max).clamp(-1.0, 1.0),
                        );
                    }
                } else {
                    for sample in reader.samples::<i32>() {
                        samples.push(
                            (sample.map_err(mlua::Error::external)? as f32 / max).clamp(-1.0, 1.0),
                        );
                    }
                }
            }
        }
        let handle = Arc::new(Mutex::new(SoundAsset {
            sample_rate: spec.sample_rate,
            channels: spec.channels,
            samples,
            bytes: file_bytes,
            unloaded: false,
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
        }))))
    }

    pub(crate) fn unload_image_path(&mut self, user_path: &str) -> bool {
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
        let before_images = self.images.len();
        let before_sounds = self.sounds.len();
        self.images.retain(|_, weak| weak.strong_count() > 0);
        self.sounds.retain(|_, weak| weak.strong_count() > 0);
        (
            before_images - self.images.len(),
            before_sounds - self.sounds.len(),
        )
    }
}

pub(crate) fn add_assets_module(lua: &Lua, env_root: PathBuf) -> mlua::Result<()> {
    let manager = Arc::new(Mutex::new(AssetManager::new(env_root)));
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
                    .load_sound_wav(&path)?;
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
                    if channels > 0 && samples.len() % channels as usize != 0 {
                        let remainder = samples.len() % channels as usize;
                        samples.extend(std::iter::repeat(fill).take(channels as usize - remainder));
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
