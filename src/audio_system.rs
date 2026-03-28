use crate::assets::SoundHandle;
use mlua::{AnyUserData, Lua};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex};

struct AudioBackend {
    _stream: OutputStream,
    handle: OutputStreamHandle,
    sinks: Mutex<HashMap<usize, Arc<Sink>>>,
}

impl AudioBackend {
    fn new() -> mlua::Result<Self> {
        let (stream, handle) = OutputStream::try_default()
            .map_err(|error| mlua::Error::external(format!("failed to initialize audio output: {error}")))?;
        Ok(Self {
            _stream: stream,
            handle,
            sinks: Mutex::new(HashMap::new()),
        })
    }

    fn play(&self, sound: &SoundHandle, looped: bool, volume: f32) -> mlua::Result<()> {
        let bytes = sound.bytes()?;
        let decoder = Decoder::new(Cursor::new(bytes)).map_err(|error| {
            mlua::Error::external(format!("failed to decode audio data: {error}"))
        })?;
        let sink = Arc::new(Sink::try_new(&self.handle).map_err(|error| {
            mlua::Error::external(format!("failed to create audio sink: {error}"))
        })?);
        sink.set_volume(volume.clamp(0.0, 1.0));
        if looped {
            sink.append(decoder.repeat_infinite());
        } else {
            sink.append(decoder);
        }
        let mut sinks = self
            .sinks
            .lock()
            .map_err(|_| mlua::Error::external("audio sink lock poisoned"))?;
        if let Some(existing) = sinks.insert(sound.id(), sink.clone()) {
            existing.stop();
        }
        sink.play();
        Ok(())
    }

    fn stop(&self, sound: &SoundHandle) -> mlua::Result<()> {
        let mut sinks = self
            .sinks
            .lock()
            .map_err(|_| mlua::Error::external("audio sink lock poisoned"))?;
        if let Some(existing) = sinks.remove(&sound.id()) {
            existing.stop();
        }
        Ok(())
    }

    fn set_volume(&self, sound: &SoundHandle, volume: f32) -> mlua::Result<()> {
        let sinks = self
            .sinks
            .lock()
            .map_err(|_| mlua::Error::external("audio sink lock poisoned"))?;
        if let Some(existing) = sinks.get(&sound.id()) {
            existing.set_volume(volume.clamp(0.0, 1.0));
        }
        Ok(())
    }
}

thread_local! {
    static AUDIO: RefCell<Option<AudioBackend>> = const { RefCell::new(None) };
}

fn with_audio_backend<R>(f: impl FnOnce(&AudioBackend) -> mlua::Result<R>) -> mlua::Result<R> {
    AUDIO.with(|cell| {
        if cell.borrow().is_none() {
            *cell.borrow_mut() = Some(AudioBackend::new()?);
        }
        let borrowed = cell.borrow();
        let backend = borrowed
            .as_ref()
            .ok_or_else(|| mlua::Error::external("failed to initialize audio backend"))?;
        f(backend)
    })
}

pub(crate) fn add_audio_module(lua: &Lua) -> mlua::Result<()> {
    let audio = lua.create_table()?;

    audio.set(
        "play",
        lua.create_function(
            move |_lua, (sound_ud, looped, volume): (AnyUserData, Option<bool>, Option<f32>)| {
                let sound = sound_ud.borrow::<SoundHandle>()?;
                sound.ensure_uploaded()?;
                with_audio_backend(|audio| {
                    audio.play(&sound, looped.unwrap_or(false), volume.unwrap_or(1.0))
                })
            },
        )?,
    )?;

    audio.set(
        "playOnce",
        lua.create_function(
            move |_lua, (sound_ud, volume): (AnyUserData, Option<f32>)| {
                let sound = sound_ud.borrow::<SoundHandle>()?;
                sound.ensure_uploaded()?;
                with_audio_backend(|audio| audio.play(&sound, false, volume.unwrap_or(1.0)))
            },
        )?,
    )?;

    audio.set(
        "stop",
        lua.create_function(move |_lua, sound_ud: AnyUserData| {
            let sound = sound_ud.borrow::<SoundHandle>()?;
            sound.ensure_uploaded()?;
            with_audio_backend(|audio| audio.stop(&sound))
        })?,
    )?;

    audio.set(
        "setVolume",
        lua.create_function(move |_lua, (sound_ud, volume): (AnyUserData, f32)| {
            let sound = sound_ud.borrow::<SoundHandle>()?;
            sound.ensure_uploaded()?;
            with_audio_backend(|audio| audio.set_volume(&sound, volume))
        })?,
    )?;

    lua.globals().set("audio", audio)?;
    Ok(())
}
