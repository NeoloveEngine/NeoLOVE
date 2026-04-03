#[cfg(not(target_os = "emscripten"))]
mod native {
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
            let (stream, handle) = OutputStream::try_default().map_err(|error| {
                mlua::Error::external(format!("failed to initialize audio output: {error}"))
            })?;
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
            lua.create_function(move |_lua, (sound_ud, volume): (AnyUserData, Option<f32>)| {
                let sound = sound_ud.borrow::<SoundHandle>()?;
                sound.ensure_uploaded()?;
                with_audio_backend(|audio| audio.play(&sound, false, volume.unwrap_or(1.0)))
            })?,
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
}

#[cfg(target_os = "emscripten")]
mod native {
    use crate::assets::SoundHandle;
    use mlua::{AnyUserData, Lua};
    use std::ffi::c_char;

    unsafe extern "C" {
        fn neolove_web_audio_play(
            sound_id: i32,
            samples: *const f32,
            samples_len: i32,
            sample_rate: i32,
            channels: i32,
            looped: i32,
            volume: f32,
        ) -> i32;
        fn neolove_web_audio_stop(sound_id: i32) -> i32;
        fn neolove_web_audio_set_volume(sound_id: i32, volume: f32) -> i32;
        fn neolove_web_take_audio_error(buffer: *mut c_char, capacity: i32) -> i32;
    }

    fn take_audio_error() -> String {
        let mut buffer = [0u8; 512];
        let written =
            unsafe { neolove_web_take_audio_error(buffer.as_mut_ptr() as *mut c_char, 512) };
        if written <= 0 {
            return "web audio operation failed".to_string();
        }
        String::from_utf8_lossy(&buffer[..written as usize]).into_owned()
    }

    fn check_bridge_result(result: i32, action: &str) -> mlua::Result<()> {
        if result != 0 {
            return Ok(());
        }
        Err(mlua::Error::external(format!(
            "{action}: {}",
            take_audio_error()
        )))
    }

    fn play_sound(sound: &SoundHandle, looped: bool, volume: f32) -> mlua::Result<()> {
        let sound_id = sound.id() as i32;
        let volume = volume.clamp(0.0, 1.0);
        let result = sound.with_samples(|sample_rate, channels, samples| {
            if channels == 0 {
                return Err(mlua::Error::external("sound must have at least one channel"));
            }
            if samples.is_empty() {
                return Err(mlua::Error::external("sound has no samples"));
            }
            if samples.len() % channels as usize != 0 {
                return Err(mlua::Error::external(
                    "sound sample buffer length must be a multiple of channels",
                ));
            }
            if samples.len() > i32::MAX as usize {
                return Err(mlua::Error::external(
                    "sound sample buffer is too large for the web audio bridge",
                ));
            }
            if sample_rate > i32::MAX as u32 {
                return Err(mlua::Error::external(
                    "sound sample rate is too large for the web audio bridge",
                ));
            }

            Ok(unsafe {
                neolove_web_audio_play(
                    sound_id,
                    samples.as_ptr(),
                    samples.len() as i32,
                    sample_rate as i32,
                    channels as i32,
                    if looped { 1 } else { 0 },
                    volume,
                )
            })
        })?;
        check_bridge_result(result, "failed to play audio")
    }

    pub(crate) fn add_audio_module(lua: &Lua) -> mlua::Result<()> {
        let audio = lua.create_table()?;

        audio.set(
            "play",
            lua.create_function(
                move |_lua, (sound_ud, looped, volume): (AnyUserData, Option<bool>, Option<f32>)| {
                    let sound = sound_ud.borrow::<SoundHandle>()?;
                    sound.ensure_uploaded()?;
                    play_sound(&sound, looped.unwrap_or(false), volume.unwrap_or(1.0))
                },
            )?,
        )?;
        audio.set(
            "playOnce",
            lua.create_function(move |_lua, (sound_ud, volume): (AnyUserData, Option<f32>)| {
                let sound = sound_ud.borrow::<SoundHandle>()?;
                sound.ensure_uploaded()?;
                play_sound(&sound, false, volume.unwrap_or(1.0))
            })?,
        )?;
        audio.set(
            "stop",
            lua.create_function(move |_lua, sound_ud: AnyUserData| {
                let sound = sound_ud.borrow::<SoundHandle>()?;
                sound.ensure_uploaded()?;
                check_bridge_result(
                    unsafe { neolove_web_audio_stop(sound.id() as i32) },
                    "failed to stop audio",
                )
            })?,
        )?;
        audio.set(
            "setVolume",
            lua.create_function(move |_lua, (sound_ud, volume): (AnyUserData, f32)| {
                let sound = sound_ud.borrow::<SoundHandle>()?;
                sound.ensure_uploaded()?;
                check_bridge_result(
                    unsafe {
                        neolove_web_audio_set_volume(sound.id() as i32, volume.clamp(0.0, 1.0))
                    },
                    "failed to set audio volume",
                )
            })?,
        )?;

        lua.globals().set("audio", audio)?;
        Ok(())
    }
}

pub(crate) use native::add_audio_module;
