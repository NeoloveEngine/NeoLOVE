use macroquad::color::Color;
use macroquad::texture::{Image, Texture2D};
use mlua::{AnyUserData, Lua, Table, UserData, UserDataMethods, Value, Variadic};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Debug)]
struct ImageAsset {
    image: Image,
    texture: Texture2D,
    dirty: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ImageHandle(Rc<RefCell<ImageAsset>>);

#[derive(Debug)]
struct SoundAsset {
    sample_rate: u32,
    channels: u16,
    samples: Vec<f32>, // interleaved, normalized [-1.0, 1.0]
}

#[derive(Clone, Debug)]
pub(crate) struct SoundHandle(Rc<RefCell<SoundAsset>>);

#[derive(Debug)]
pub(crate) struct AssetManager {
    env_root: PathBuf,
    images: HashMap<PathBuf, Rc<RefCell<ImageAsset>>>,
    sounds: HashMap<PathBuf, Rc<RefCell<SoundAsset>>>,
}

fn lua_color4(lua: &Lua, r: u8, g: u8, b: u8, a: u8) -> Table {
    let color = lua.create_table().unwrap();
    color.set("r", r).unwrap();
    color.set("g", g).unwrap();
    color.set("b", b).unwrap();
    color.set("a", a).unwrap();
    color
}

fn color4_table_to_color(table: Table) -> mlua::Result<Color> {
    let r: f32 = table.get("r")?;
    let g: f32 = table.get("g")?;
    let b: f32 = table.get("b")?;
    let a: f32 = table.get("a")?;
    Ok(Color::from_rgba(
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

fn parse_color_args(lua: &Lua, args: &[Value]) -> mlua::Result<Color> {
    let _ = lua; // reserved for future overloads
    match args {
        [Value::Table(t)] => color4_table_to_color(t.clone()),
        [r, g, b] => {
            let (r, g, b) = (
                value_to_f32(r).ok_or_else(|| mlua::Error::external("invalid r"))?,
                value_to_f32(g).ok_or_else(|| mlua::Error::external("invalid g"))?,
                value_to_f32(b).ok_or_else(|| mlua::Error::external("invalid b"))?,
            );
            Ok(Color::from_rgba(
                r.clamp(0.0, 255.0) as u8,
                g.clamp(0.0, 255.0) as u8,
                b.clamp(0.0, 255.0) as u8,
                255,
            ))
        }
        [r, g, b, a] => {
            let (r, g, b, a) = (
                value_to_f32(r).ok_or_else(|| mlua::Error::external("invalid r"))?,
                value_to_f32(g).ok_or_else(|| mlua::Error::external("invalid g"))?,
                value_to_f32(b).ok_or_else(|| mlua::Error::external("invalid b"))?,
                value_to_f32(a).ok_or_else(|| mlua::Error::external("invalid a"))?,
            );
            Ok(Color::from_rgba(
                r.clamp(0.0, 255.0) as u8,
                g.clamp(0.0, 255.0) as u8,
                b.clamp(0.0, 255.0) as u8,
                a.clamp(0.0, 255.0) as u8,
            ))
        }
        _ => Err(mlua::Error::external(format!(
            "expected color4 table or r,g,b[,a], got {} args",
            args.len()
        ))),
    }
}

impl ImageHandle {
    pub(crate) fn texture(&self) -> Texture2D {
        self.0.borrow().texture.weak_clone()
    }

    pub(crate) fn ensure_uploaded(&self) {
        let mut asset = self.0.borrow_mut();
        if asset.dirty {
            asset.texture.update(&asset.image);
            asset.dirty = false;
        }
    }
}

impl UserData for ImageHandle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("width", |_lua, this, ()| Ok(this.0.borrow().image.width as u32));
        methods.add_method("height", |_lua, this, ()| Ok(this.0.borrow().image.height as u32));
        methods.add_method("size", |_lua, this, ()| {
            let img = this.0.borrow();
            Ok((img.image.width as u32, img.image.height as u32))
        });

        methods.add_method("getPixel", |lua, this, (x, y): (u32, u32)| {
            let img = this.0.borrow();
            if x >= img.image.width as u32 || y >= img.image.height as u32 {
                return Err(mlua::Error::external("pixel out of bounds"));
            }
            let c = img.image.get_pixel(x, y);
            Ok(lua_color4(
                lua,
                (c.r * 255.0) as u8,
                (c.g * 255.0) as u8,
                (c.b * 255.0) as u8,
                (c.a * 255.0) as u8,
            ))
        });

        methods.add_method("setPixel", |lua, this, args: Variadic<Value>| {
            if args.len() < 3 {
                return Err(mlua::Error::external(
                    "setPixel expects (x, y, color) or (x, y, r, g, b[, a])",
                ));
            }

            let x = match &args[0] {
                Value::Integer(i) => *i as i64,
                Value::Number(n) => *n as i64,
                _ => {
                    return Err(mlua::Error::external(
                        "setPixel expects numeric x as arg1",
                    ))
                }
            };
            let y = match &args[1] {
                Value::Integer(i) => *i as i64,
                Value::Number(n) => *n as i64,
                _ => {
                    return Err(mlua::Error::external(
                        "setPixel expects numeric y as arg2",
                    ))
                }
            };
            if x < 0 || y < 0 {
                return Err(mlua::Error::external("pixel out of bounds"));
            }

            let color = parse_color_args(lua, &args[2..])?;
            let mut asset = this.0.borrow_mut();
            let x = x as u32;
            let y = y as u32;
            if x >= asset.image.width as u32 || y >= asset.image.height as u32 {
                return Err(mlua::Error::external("pixel out of bounds"));
            }
            asset.image.set_pixel(x, y, color);
            asset.dirty = true;
            Ok(())
        });

        methods.add_method("fill", |lua, this, args: Variadic<Value>| {
            let color = parse_color_args(lua, &args)?;
            let mut asset = this.0.borrow_mut();
            let bytes: [u8; 4] = color.into();
            for px in asset.image.get_image_data_mut() {
                *px = bytes;
            }
            asset.dirty = true;
            Ok(())
        });

        methods.add_method("upload", |_lua, this, ()| {
            let mut asset = this.0.borrow_mut();
            asset.texture.update(&asset.image);
            asset.dirty = false;
            Ok(())
        });

        methods.add_meta_method("__tostring", |_lua, this, ()| {
            let img = this.0.borrow();
            Ok(format!(
                "Image(w={}, h={}, dirty={})",
                img.image.width, img.image.height, img.dirty
            ))
        });
    }
}

impl UserData for SoundHandle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("sampleRate", |_lua, this, ()| Ok(this.0.borrow().sample_rate));
        methods.add_method("channels", |_lua, this, ()| Ok(this.0.borrow().channels));
        methods.add_method("len", |_lua, this, ()| Ok(this.0.borrow().samples.len() as u32));

        methods.add_method("getSample", |_lua, this, index: i64| {
            if index < 0 {
                return Err(mlua::Error::external("sample index out of bounds"));
            }
            let asset = this.0.borrow();
            let i = index as usize;
            asset
                .samples
                .get(i)
                .copied()
                .ok_or_else(|| mlua::Error::external("sample index out of bounds"))
        });

        methods.add_method("setSample", |_lua, this, (index, value): (i64, f32)| {
            if index < 0 {
                return Err(mlua::Error::external("sample index out of bounds"));
            }
            let mut asset = this.0.borrow_mut();
            let i = index as usize;
            if i >= asset.samples.len() {
                return Err(mlua::Error::external("sample index out of bounds"));
            }
            asset.samples[i] = value.clamp(-1.0, 1.0);
            Ok(())
        });

        methods.add_meta_method("__tostring", |_lua, this, ()| {
            let s = this.0.borrow();
            Ok(format!(
                "Sound(sr={}, ch={}, samples={})",
                s.sample_rate,
                s.channels,
                s.samples.len()
            ))
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
        let p = PathBuf::from(user_path);
        if p.is_absolute() {
            return p;
        }

        if user_path.starts_with("./")
            || user_path.starts_with("../")
            || user_path.starts_with("assets/")
            || user_path.starts_with("assets\\")
        {
            return self.env_root.join(p);
        }

        self.env_root.join("assets").join(p)
    }

    fn canonical_for_cache(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    pub(crate) fn load_image(&mut self, user_path: &str) -> mlua::Result<ImageHandle> {
        let resolved = self.resolve_path(user_path);
        let cache_key = Self::canonical_for_cache(&resolved);
        if let Some(existing) = self.images.get(&cache_key) {
            return Ok(ImageHandle(existing.clone()));
        }

        let bytes = std::fs::read(&resolved).map_err(mlua::Error::external)?;
        let image = Image::from_file_with_format(&bytes, None).map_err(mlua::Error::external)?;
        let texture = Texture2D::from_image(&image);
        let rc = Rc::new(RefCell::new(ImageAsset {
            image,
            texture,
            dirty: false,
        }));
        self.images.insert(cache_key, rc.clone());

        Ok(ImageHandle(rc.clone()))
    }

    pub(crate) fn new_image(&mut self, width: u16, height: u16, color: Color) -> ImageHandle {
        let image = Image::gen_image_color(width, height, color);
        let texture = Texture2D::from_image(&image);
        ImageHandle(Rc::new(RefCell::new(ImageAsset {
            image,
            texture,
            dirty: false,
        })))
    }

    pub(crate) fn load_sound_wav(&mut self, user_path: &str) -> mlua::Result<SoundHandle> {
        let resolved = self.resolve_path(user_path);
        let cache_key = Self::canonical_for_cache(&resolved);
        if let Some(existing) = self.sounds.get(&cache_key) {
            return Ok(SoundHandle(existing.clone()));
        }

        let bytes = std::fs::read(&resolved).map_err(mlua::Error::external)?;
        let mut reader =
            hound::WavReader::new(Cursor::new(bytes)).map_err(mlua::Error::external)?;

        let spec = reader.spec();
        let sample_rate = spec.sample_rate;
        let channels = spec.channels;
        let bits = spec.bits_per_sample;

        let mut samples = Vec::new();

        match spec.sample_format {
            hound::SampleFormat::Float => {
                for s in reader.samples::<f32>() {
                    let s = s.map_err(mlua::Error::external)?;
                    samples.push(s.clamp(-1.0, 1.0));
                }
            }
            hound::SampleFormat::Int => {
                let max = ((1u64 << (bits.saturating_sub(1))) as f32) - 1.0;
                if bits <= 16 {
                    for s in reader.samples::<i16>() {
                        let s = s.map_err(mlua::Error::external)? as f32;
                        samples.push((s / max).clamp(-1.0, 1.0));
                    }
                } else {
                    for s in reader.samples::<i32>() {
                        let s = s.map_err(mlua::Error::external)? as f32;
                        samples.push((s / max).clamp(-1.0, 1.0));
                    }
                }
            }
        }

        let rc = Rc::new(RefCell::new(SoundAsset {
            sample_rate,
            channels,
            samples,
        }));
        self.sounds.insert(cache_key, rc.clone());

        Ok(SoundHandle(rc))
    }

    pub(crate) fn new_sound(&mut self, sample_rate: u32, channels: u16, samples: Vec<f32>) -> SoundHandle {
        SoundHandle(Rc::new(RefCell::new(SoundAsset {
            sample_rate,
            channels,
            samples,
        })))
    }
}

pub(crate) fn add_assets_module(lua: &Lua, env_root: PathBuf) -> mlua::Result<()> {
    let manager = Rc::new(RefCell::new(AssetManager::new(env_root)));
    let assets = lua.create_table()?;

    {
        let manager = manager.clone();
        let load_image = lua.create_function(move |lua, path: String| {
            let handle = manager.borrow_mut().load_image(&path)?;
            let ud: AnyUserData = lua.create_userdata(handle)?;
            Ok(ud)
        })?;
        assets.set("loadImage", load_image)?;
    }

    {
        let manager = manager.clone();
        let new_image = lua.create_function(move |lua, (w, h, color): (u32, u32, Option<Table>)| {
            let color = if let Some(t) = color {
                color4_table_to_color(t)?
            } else {
                Color::from_rgba(255, 255, 255, 255)
            };
            let handle = manager
                .borrow_mut()
                .new_image(w.min(u16::MAX as u32) as u16, h.min(u16::MAX as u32) as u16, color);
            let ud: AnyUserData = lua.create_userdata(handle)?;
            Ok(ud)
        })?;
        assets.set("newImage", new_image)?;
    }

    {
        let manager = manager.clone();
        let load_sound = lua.create_function(move |lua, path: String| {
            let handle = manager.borrow_mut().load_sound_wav(&path)?;
            let ud: AnyUserData = lua.create_userdata(handle)?;
            Ok(ud)
        })?;
        assets.set("loadSound", load_sound)?;
    }

    {
        let manager = manager.clone();
        let new_sound = lua.create_function(
            move |lua, (sample_rate, channels, len, fill): (u32, u16, u32, Option<f32>)| {
                let fill = fill.unwrap_or(0.0).clamp(-1.0, 1.0);
                let mut samples = vec![fill; len as usize];
                // ensure interleaved length is multiple of channels if caller passed frames
                if channels > 0 && samples.len() % channels as usize != 0 {
                    let rem = samples.len() % channels as usize;
                    samples.extend(std::iter::repeat(fill).take(channels as usize - rem));
                }
                let handle = manager.borrow_mut().new_sound(sample_rate, channels, samples);
                let ud: AnyUserData = lua.create_userdata(handle)?;
                Ok(ud)
            },
        )?;
        assets.set("newSound", new_sound)?;
    }

    lua.globals().set("assets", assets)?;
    Ok(())
}
