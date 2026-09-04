use mlua::Table;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DistanceModel3D {
    Linear,
    Inverse,
    Exponential,
}

impl DistanceModel3D {
    fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "linear" => Self::Linear,
            "exponential" | "exponent" => Self::Exponential,
            _ => Self::Inverse,
        }
    }

    #[cfg(target_os = "emscripten")]
    fn as_i32(self) -> i32 {
        match self {
            Self::Linear => 0,
            Self::Inverse => 1,
            Self::Exponential => 2,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct SpatialOptions3D {
    voice_id: usize,
    looped: bool,
    volume: f32,
    min_distance: f32,
    max_distance: f32,
    rolloff: f32,
    distance_model: DistanceModel3D,
}

fn table_alias<T: mlua::FromLua>(table: &Table, snake: &str, camel: &str) -> Option<T> {
    table
        .get::<Option<T>>(snake)
        .ok()
        .flatten()
        .or_else(|| table.get::<Option<T>>(camel).ok().flatten())
}

fn sanitize_spatial_options(
    voice_id: usize,
    looped: bool,
    volume: f32,
    min_distance: f32,
    max_distance: f32,
    rolloff: f32,
    distance_model: DistanceModel3D,
) -> SpatialOptions3D {
    let min_distance = if min_distance.is_finite() {
        min_distance.max(0.001)
    } else {
        1.0
    };
    let max_distance = if max_distance.is_finite() {
        max_distance.max(min_distance)
    } else {
        100.0f32.max(min_distance)
    };
    SpatialOptions3D {
        voice_id,
        looped,
        volume: if volume.is_finite() {
            volume.clamp(0.0, 1.0)
        } else {
            1.0
        },
        min_distance,
        max_distance,
        rolloff: if rolloff.is_finite() {
            rolloff.max(0.0)
        } else {
            1.0
        },
        distance_model,
    }
}

fn parse_spatial_options(table: Option<Table>, fallback_voice_id: usize) -> SpatialOptions3D {
    let voice_id = table
        .as_ref()
        .and_then(|options| table_alias::<i64>(options, "voice_id", "voiceId"))
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value != 0)
        .unwrap_or(fallback_voice_id);
    let looped = table
        .as_ref()
        .and_then(|options| table_alias(options, "looping", "looped"))
        .unwrap_or(false);
    let volume = table
        .as_ref()
        .and_then(|options| table_alias(options, "volume", "volume"))
        .unwrap_or(1.0);
    let min_distance = table
        .as_ref()
        .and_then(|options| table_alias(options, "min_distance", "minDistance"))
        .unwrap_or(1.0);
    let max_distance = table
        .as_ref()
        .and_then(|options| table_alias(options, "max_distance", "maxDistance"))
        .unwrap_or(100.0);
    let rolloff = table
        .as_ref()
        .and_then(|options| table_alias(options, "rolloff", "rolloff"))
        .unwrap_or(1.0);
    let distance_model = table
        .as_ref()
        .and_then(|options| table_alias::<String>(options, "distance_model", "distanceModel"))
        .map(|value| DistanceModel3D::parse(&value))
        .unwrap_or(DistanceModel3D::Inverse);
    sanitize_spatial_options(
        voice_id,
        looped,
        volume,
        min_distance,
        max_distance,
        rolloff,
        distance_model,
    )
}

/// WebAudio-compatible distance attenuation. Keeping this policy in Rust also
/// makes native and browser-authored AudioSource3D values mean the same thing.
#[cfg(not(target_os = "emscripten"))]
fn spatial_gain_3d(distance: f32, options: SpatialOptions3D) -> f32 {
    let distance = if distance.is_finite() {
        distance.clamp(options.min_distance, options.max_distance)
    } else {
        options.max_distance
    };
    let gain = match options.distance_model {
        DistanceModel3D::Linear => {
            let span = (options.max_distance - options.min_distance).max(0.001);
            1.0 - options.rolloff * (distance - options.min_distance) / span
        }
        DistanceModel3D::Inverse => {
            options.min_distance
                / (options.min_distance + options.rolloff * (distance - options.min_distance))
        }
        DistanceModel3D::Exponential => (distance / options.min_distance).powf(-options.rolloff),
    };
    gain.clamp(0.0, 1.0)
}

#[cfg(not(target_os = "emscripten"))]
mod native {
    use super::{SpatialOptions3D, parse_spatial_options, spatial_gain_3d};
    use crate::assets::SoundHandle;
    use mlua::{AnyUserData, Lua, Table};
    use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source, SpatialSink};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};

    enum PlayingSink {
        Flat(Arc<Sink>),
        Spatial {
            sink: Arc<SpatialSink>,
            emitter: [f32; 3],
        },
    }

    #[derive(Clone, Copy)]
    struct Listener3D {
        position: [f32; 3],
        forward: [f32; 3],
        right: [f32; 3],
        ear_distance: f32,
    }

    impl Default for Listener3D {
        fn default() -> Self {
            Self {
                position: [0.0, 0.0, 0.0],
                forward: [0.0, 0.0, -1.0],
                right: [1.0, 0.0, 0.0],
                ear_distance: 0.2,
            }
        }
    }

    struct SpatialVoice3D {
        sink: Arc<SpatialSink>,
        emitter: [f32; 3],
        options: SpatialOptions3D,
    }

    impl PlayingSink {
        fn stop(&self) {
            match self {
                Self::Flat(sink) => sink.stop(),
                Self::Spatial { sink, .. } => sink.stop(),
            }
        }

        fn set_volume(&self, volume: f32) {
            match self {
                Self::Flat(sink) => sink.set_volume(volume),
                Self::Spatial { sink, .. } => sink.set_volume(volume),
            }
        }
    }

    struct AudioBackend {
        _stream: OutputStream,
        handle: OutputStreamHandle,
        sinks: Mutex<HashMap<usize, PlayingSink>>,
        listener: Mutex<[f32; 2]>,
        spatial_3d: Mutex<HashMap<usize, SpatialVoice3D>>,
        listener_3d: Mutex<Listener3D>,
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
                listener: Mutex::new([0.0, 0.0]),
                spatial_3d: Mutex::new(HashMap::new()),
                listener_3d: Mutex::new(Listener3D::default()),
            })
        }

        fn normalized(value: [f32; 3], fallback: [f32; 3]) -> [f32; 3] {
            let length = (value[0] * value[0] + value[1] * value[1] + value[2] * value[2]).sqrt();
            if length.is_finite() && length > 1.0e-6 {
                [value[0] / length, value[1] / length, value[2] / length]
            } else {
                fallback
            }
        }

        fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
            [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ]
        }

        fn spatial_positions(
            listener: Listener3D,
            emitter: [f32; 3],
        ) -> ([f32; 3], [f32; 3], [f32; 3]) {
            let delta = [
                emitter[0] - listener.position[0],
                emitter[1] - listener.position[1],
                emitter[2] - listener.position[2],
            ];
            // Rodio applies its own distance law. Feed it unit-distance,
            // listener-relative coordinates for directional stereo, then apply
            // the authored distance model explicitly through sink volume.
            let source = Self::normalized(delta, listener.forward);
            let half_ear = listener.ear_distance * 0.5;
            let left = [
                -listener.right[0] * half_ear,
                -listener.right[1] * half_ear,
                -listener.right[2] * half_ear,
            ];
            let right = [
                listener.right[0] * half_ear,
                listener.right[1] * half_ear,
                listener.right[2] * half_ear,
            ];
            (source, left, right)
        }

        fn update_spatial_voice(listener: Listener3D, voice: &SpatialVoice3D) {
            let (source, left, right) = Self::spatial_positions(listener, voice.emitter);
            voice.sink.set_emitter_position(source);
            voice.sink.set_left_ear_position(left);
            voice.sink.set_right_ear_position(right);
            let dx = voice.emitter[0] - listener.position[0];
            let dy = voice.emitter[1] - listener.position[1];
            let dz = voice.emitter[2] - listener.position[2];
            let distance = (dx * dx + dy * dy + dz * dz).sqrt();
            voice
                .sink
                .set_volume(voice.options.volume * spatial_gain_3d(distance, voice.options));
        }

        fn play_spatial_3d(
            &self,
            sound: &SoundHandle,
            emitter: [f32; 3],
            options: SpatialOptions3D,
        ) -> mlua::Result<usize> {
            let bytes = sound.bytes()?;
            let decoder = Decoder::new(Cursor::new(bytes)).map_err(|error| {
                mlua::Error::external(format!("failed to decode 3D audio data: {error}"))
            })?;
            let listener = *self
                .listener_3d
                .lock()
                .map_err(|_| mlua::Error::external("3D audio listener lock poisoned"))?;
            let (source, left, right) = Self::spatial_positions(listener, emitter);
            let sink = Arc::new(
                SpatialSink::try_new(&self.handle, source, left, right).map_err(|error| {
                    mlua::Error::external(format!(
                        "failed to create 3D spatial audio sink: {error}"
                    ))
                })?,
            );
            if options.looped {
                sink.append(decoder.repeat_infinite());
            } else {
                sink.append(decoder);
            }
            let voice = SpatialVoice3D {
                sink: sink.clone(),
                emitter,
                options,
            };
            Self::update_spatial_voice(listener, &voice);
            let mut voices = self
                .spatial_3d
                .lock()
                .map_err(|_| mlua::Error::external("3D audio voice lock poisoned"))?;
            if let Some(existing) = voices.insert(options.voice_id, voice) {
                existing.sink.stop();
            }
            sink.play();
            Ok(options.voice_id)
        }

        fn update_spatial_3d(
            &self,
            voice_id: usize,
            emitter: [f32; 3],
            options: SpatialOptions3D,
        ) -> mlua::Result<bool> {
            let listener = *self
                .listener_3d
                .lock()
                .map_err(|_| mlua::Error::external("3D audio listener lock poisoned"))?;
            let mut voices = self
                .spatial_3d
                .lock()
                .map_err(|_| mlua::Error::external("3D audio voice lock poisoned"))?;
            if voices
                .get(&voice_id)
                .is_some_and(|voice| voice.sink.empty())
            {
                voices.remove(&voice_id);
                return Ok(false);
            }
            let Some(voice) = voices.get_mut(&voice_id) else {
                return Ok(false);
            };
            voice.emitter = emitter;
            voice.options = options;
            Self::update_spatial_voice(listener, voice);
            Ok(true)
        }

        fn set_listener_3d(
            &self,
            position: [f32; 3],
            forward: [f32; 3],
            up: [f32; 3],
            ear_distance: f32,
        ) -> mlua::Result<()> {
            let forward = Self::normalized(forward, [0.0, 0.0, -1.0]);
            let up = Self::normalized(up, [0.0, 1.0, 0.0]);
            let right = Self::normalized(Self::cross(forward, up), [1.0, 0.0, 0.0]);
            let listener = Listener3D {
                position: position.map(|value| if value.is_finite() { value } else { 0.0 }),
                forward,
                right,
                ear_distance: if ear_distance.is_finite() {
                    ear_distance.clamp(0.001, 10.0)
                } else {
                    0.2
                },
            };
            *self
                .listener_3d
                .lock()
                .map_err(|_| mlua::Error::external("3D audio listener lock poisoned"))? = listener;
            let voices = self
                .spatial_3d
                .lock()
                .map_err(|_| mlua::Error::external("3D audio voice lock poisoned"))?;
            for voice in voices.values() {
                Self::update_spatial_voice(listener, voice);
            }
            Ok(())
        }

        fn stop_spatial_3d(&self, voice_id: usize) -> mlua::Result<()> {
            let mut voices = self
                .spatial_3d
                .lock()
                .map_err(|_| mlua::Error::external("3D audio voice lock poisoned"))?;
            if let Some(voice) = voices.remove(&voice_id) {
                voice.sink.stop();
            }
            Ok(())
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
            if let Some(existing) = sinks.insert(sound.id(), PlayingSink::Flat(sink.clone())) {
                existing.stop();
            }
            sink.play();
            Ok(())
        }

        fn play_spatial(
            &self,
            sound: &SoundHandle,
            x: f32,
            y: f32,
            looped: bool,
            volume: f32,
        ) -> mlua::Result<()> {
            let bytes = sound.bytes()?;
            let decoder = Decoder::new(Cursor::new(bytes)).map_err(|error| {
                mlua::Error::external(format!("failed to decode audio data: {error}"))
            })?;
            let listener = *self
                .listener
                .lock()
                .map_err(|_| mlua::Error::external("audio listener lock poisoned"))?;
            let left_ear = [listener[0] - 0.1, listener[1], 0.0];
            let right_ear = [listener[0] + 0.1, listener[1], 0.0];
            let emitter = [x, y, 0.0];
            let sink = Arc::new(
                SpatialSink::try_new(&self.handle, emitter, left_ear, right_ear).map_err(
                    |error| {
                        mlua::Error::external(format!(
                            "failed to create spatial audio sink: {error}"
                        ))
                    },
                )?,
            );
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
            if let Some(existing) = sinks.insert(
                sound.id(),
                PlayingSink::Spatial {
                    sink: sink.clone(),
                    emitter,
                },
            ) {
                existing.stop();
            }
            sink.play();
            Ok(())
        }

        fn set_position(&self, sound: &SoundHandle, x: f32, y: f32) -> mlua::Result<bool> {
            let mut sinks = self
                .sinks
                .lock()
                .map_err(|_| mlua::Error::external("audio sink lock poisoned"))?;
            let Some(PlayingSink::Spatial { sink, emitter }) = sinks.get_mut(&sound.id()) else {
                return Ok(false);
            };
            *emitter = [x, y, 0.0];
            sink.set_emitter_position(*emitter);
            Ok(true)
        }

        fn set_listener_position(&self, x: f32, y: f32) -> mlua::Result<()> {
            *self
                .listener
                .lock()
                .map_err(|_| mlua::Error::external("audio listener lock poisoned"))? = [x, y];
            let sinks = self
                .sinks
                .lock()
                .map_err(|_| mlua::Error::external("audio sink lock poisoned"))?;
            for sink in sinks.values() {
                if let PlayingSink::Spatial { sink, .. } = sink {
                    sink.set_left_ear_position([x - 0.1, y, 0.0]);
                    sink.set_right_ear_position([x + 0.1, y, 0.0]);
                }
            }
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

        audio.set(
            "playSpatial",
            lua.create_function(
                move |_lua,
                      (sound_ud, x, y, looped, volume): (
                    AnyUserData,
                    f32,
                    f32,
                    Option<bool>,
                    Option<f32>,
                )| {
                    let sound = sound_ud.borrow::<SoundHandle>()?;
                    sound.ensure_uploaded()?;
                    with_audio_backend(|audio| {
                        audio.play_spatial(
                            &sound,
                            x,
                            y,
                            looped.unwrap_or(false),
                            volume.unwrap_or(1.0),
                        )
                    })
                },
            )?,
        )?;
        audio.set(
            "setPosition",
            lua.create_function(move |_lua, (sound_ud, x, y): (AnyUserData, f32, f32)| {
                let sound = sound_ud.borrow::<SoundHandle>()?;
                with_audio_backend(|audio| audio.set_position(&sound, x, y))
            })?,
        )?;
        audio.set(
            "setListenerPosition",
            lua.create_function(move |_lua, (x, y): (f32, f32)| {
                with_audio_backend(|audio| audio.set_listener_position(x, y))
            })?,
        )?;

        audio.set(
            "playSpatial3D",
            lua.create_function(
                |_lua,
                 (sound_ud, x, y, z, options): (
                    AnyUserData,
                    f32,
                    f32,
                    f32,
                    Option<Table>,
                )| {
                    let sound = sound_ud.borrow::<SoundHandle>()?;
                    sound.ensure_uploaded()?;
                    let options = parse_spatial_options(options, sound.id());
                    with_audio_backend(|audio| {
                        audio.play_spatial_3d(&sound, [x, y, z], options)
                    })
                },
            )?,
        )?;
        audio.set(
            "updateSpatial3D",
            lua.create_function(
                |_lua, (voice_id, x, y, z, options): (i64, f32, f32, f32, Option<Table>)| {
                    let voice_id = usize::try_from(voice_id).map_err(|_| {
                        mlua::Error::external("3D audio voice id must be non-negative")
                    })?;
                    let options = parse_spatial_options(options, voice_id);
                    with_audio_backend(|audio| {
                        audio.update_spatial_3d(voice_id, [x, y, z], options)
                    })
                },
            )?,
        )?;
        audio.set(
            "stopSpatial3D",
            lua.create_function(|_lua, voice_id: i64| {
                let voice_id = usize::try_from(voice_id)
                    .map_err(|_| mlua::Error::external("3D audio voice id must be non-negative"))?;
                with_audio_backend(|audio| audio.stop_spatial_3d(voice_id))
            })?,
        )?;
        audio.set(
            "setListener3D",
            lua.create_function(
                |_lua,
                 (x, y, z, fx, fy, fz, ux, uy, uz, ear_distance): (
                    f32,
                    f32,
                    f32,
                    f32,
                    f32,
                    f32,
                    f32,
                    f32,
                    f32,
                    Option<f32>,
                )| {
                    with_audio_backend(|audio| {
                        audio.set_listener_3d(
                            [x, y, z],
                            [fx, fy, fz],
                            [ux, uy, uz],
                            ear_distance.unwrap_or(0.2),
                        )
                    })
                },
            )?,
        )?;

        lua.globals().set("audio", audio)?;
        Ok(())
    }
}

#[cfg(target_os = "emscripten")]
mod native {
    use super::parse_spatial_options;
    use crate::assets::SoundHandle;
    use mlua::{AnyUserData, Lua, Table};
    use std::ffi::c_char;

    unsafe extern "C" {
        fn neolove_web_audio_play(
            sound_id: i32,
            bytes: *const u8,
            bytes_len: i32,
            looped: i32,
            volume: f32,
        ) -> i32;
        fn neolove_web_audio_stop(sound_id: i32) -> i32;
        fn neolove_web_audio_set_volume(sound_id: i32, volume: f32) -> i32;
        fn neolove_web_audio_play_spatial(
            sound_id: i32,
            bytes: *const u8,
            bytes_len: i32,
            looped: i32,
            volume: f32,
            x: f32,
            y: f32,
        ) -> i32;
        fn neolove_web_audio_set_position(sound_id: i32, x: f32, y: f32) -> i32;
        fn neolove_web_audio_set_listener_position(x: f32, y: f32) -> i32;
        fn neolove_web_audio_play_spatial_3d(
            voice_id: i32,
            bytes: *const u8,
            bytes_len: i32,
            looped: i32,
            volume: f32,
            x: f32,
            y: f32,
            z: f32,
            min_distance: f32,
            max_distance: f32,
            rolloff: f32,
            distance_model: i32,
        ) -> i32;
        fn neolove_web_audio_update_spatial_3d(
            voice_id: i32,
            x: f32,
            y: f32,
            z: f32,
            volume: f32,
            min_distance: f32,
            max_distance: f32,
            rolloff: f32,
            distance_model: i32,
        ) -> i32;
        fn neolove_web_audio_stop_spatial_3d(voice_id: i32) -> i32;
        fn neolove_web_audio_set_listener_3d(
            x: f32,
            y: f32,
            z: f32,
            forward_x: f32,
            forward_y: f32,
            forward_z: f32,
            up_x: f32,
            up_y: f32,
            up_z: f32,
        ) -> i32;
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
        let bytes = sound.bytes()?;
        if bytes.is_empty() {
            return Err(mlua::Error::external("sound has no encoded audio bytes"));
        }
        if bytes.len() > i32::MAX as usize {
            return Err(mlua::Error::external(
                "encoded sound is too large for the web audio bridge",
            ));
        }

        let result = unsafe {
            neolove_web_audio_play(
                sound_id,
                bytes.as_ptr(),
                bytes.len() as i32,
                if looped { 1 } else { 0 },
                volume,
            )
        };
        check_bridge_result(result, "failed to play audio")
    }

    fn play_spatial_sound(
        sound: &SoundHandle,
        x: f32,
        y: f32,
        looped: bool,
        volume: f32,
    ) -> mlua::Result<()> {
        let bytes = sound.bytes()?;
        if bytes.is_empty() || bytes.len() > i32::MAX as usize {
            return Err(mlua::Error::external(
                "sound has invalid encoded audio bytes",
            ));
        }
        check_bridge_result(
            unsafe {
                neolove_web_audio_play_spatial(
                    sound.id() as i32,
                    bytes.as_ptr(),
                    bytes.len() as i32,
                    i32::from(looped),
                    volume.clamp(0.0, 1.0),
                    x,
                    y,
                )
            },
            "failed to play spatial audio",
        )
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
            lua.create_function(
                move |_lua, (sound_ud, volume): (AnyUserData, Option<f32>)| {
                    let sound = sound_ud.borrow::<SoundHandle>()?;
                    sound.ensure_uploaded()?;
                    play_sound(&sound, false, volume.unwrap_or(1.0))
                },
            )?,
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
        audio.set(
            "playSpatial",
            lua.create_function(
                move |_lua,
                      (sound_ud, x, y, looped, volume): (
                    AnyUserData,
                    f32,
                    f32,
                    Option<bool>,
                    Option<f32>,
                )| {
                    let sound = sound_ud.borrow::<SoundHandle>()?;
                    sound.ensure_uploaded()?;
                    play_spatial_sound(&sound, x, y, looped.unwrap_or(false), volume.unwrap_or(1.0))
                },
            )?,
        )?;
        audio.set(
            "setPosition",
            lua.create_function(move |_lua, (sound_ud, x, y): (AnyUserData, f32, f32)| {
                let sound = sound_ud.borrow::<SoundHandle>()?;
                Ok(unsafe { neolove_web_audio_set_position(sound.id() as i32, x, y) } != 0)
            })?,
        )?;
        audio.set(
            "setListenerPosition",
            lua.create_function(move |_lua, (x, y): (f32, f32)| {
                check_bridge_result(
                    unsafe { neolove_web_audio_set_listener_position(x, y) },
                    "failed to set audio listener position",
                )
            })?,
        )?;

        audio.set(
            "playSpatial3D",
            lua.create_function(
                |_lua,
                 (sound_ud, x, y, z, options): (
                    AnyUserData,
                    f32,
                    f32,
                    f32,
                    Option<Table>,
                )| {
                    let sound = sound_ud.borrow::<SoundHandle>()?;
                    sound.ensure_uploaded()?;
                    let options = parse_spatial_options(options, sound.id());
                    let bytes = sound.bytes()?;
                    if bytes.is_empty() || bytes.len() > i32::MAX as usize {
                        return Err(mlua::Error::external(
                            "sound has invalid encoded audio bytes",
                        ));
                    }
                    check_bridge_result(
                        unsafe {
                            neolove_web_audio_play_spatial_3d(
                                options.voice_id as i32,
                                bytes.as_ptr(),
                                bytes.len() as i32,
                                i32::from(options.looped),
                                options.volume,
                                x,
                                y,
                                z,
                                options.min_distance,
                                options.max_distance,
                                options.rolloff,
                                options.distance_model.as_i32(),
                            )
                        },
                        "failed to play 3D spatial audio",
                    )?;
                    Ok(options.voice_id)
                },
            )?,
        )?;
        audio.set(
            "updateSpatial3D",
            lua.create_function(
                |_lua, (voice_id, x, y, z, options): (i64, f32, f32, f32, Option<Table>)| {
                    let voice_id = usize::try_from(voice_id).map_err(|_| {
                        mlua::Error::external("3D audio voice id must be non-negative")
                    })?;
                    let options = parse_spatial_options(options, voice_id);
                    Ok(unsafe {
                        neolove_web_audio_update_spatial_3d(
                            voice_id as i32,
                            x,
                            y,
                            z,
                            options.volume,
                            options.min_distance,
                            options.max_distance,
                            options.rolloff,
                            options.distance_model.as_i32(),
                        )
                    } != 0)
                },
            )?,
        )?;
        audio.set(
            "stopSpatial3D",
            lua.create_function(|_lua, voice_id: i64| {
                let voice_id = usize::try_from(voice_id)
                    .map_err(|_| mlua::Error::external("3D audio voice id must be non-negative"))?;
                check_bridge_result(
                    unsafe { neolove_web_audio_stop_spatial_3d(voice_id as i32) },
                    "failed to stop 3D spatial audio",
                )
            })?,
        )?;
        audio.set(
            "setListener3D",
            lua.create_function(
                |_lua,
                 (x, y, z, fx, fy, fz, ux, uy, uz, _ear_distance): (
                    f32,
                    f32,
                    f32,
                    f32,
                    f32,
                    f32,
                    f32,
                    f32,
                    f32,
                    Option<f32>,
                )| {
                    check_bridge_result(
                        unsafe {
                            neolove_web_audio_set_listener_3d(x, y, z, fx, fy, fz, ux, uy, uz)
                        },
                        "failed to update the 3D audio listener",
                    )
                },
            )?,
        )?;

        lua.globals().set("audio", audio)?;
        Ok(())
    }
}

pub(crate) use native::add_audio_module;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spatial_3d_distance_models_match_the_web_audio_equations() {
        let inverse =
            sanitize_spatial_options(7, false, 0.8, 2.0, 20.0, 1.0, DistanceModel3D::Inverse);
        assert_eq!(spatial_gain_3d(0.0, inverse), 1.0);
        assert!((spatial_gain_3d(4.0, inverse) - 0.5).abs() < 1.0e-6);
        assert!((spatial_gain_3d(100.0, inverse) - 0.1).abs() < 1.0e-6);

        let linear =
            sanitize_spatial_options(8, false, 1.0, 1.0, 11.0, 1.0, DistanceModel3D::Linear);
        assert!((spatial_gain_3d(6.0, linear) - 0.5).abs() < 1.0e-6);
        assert_eq!(spatial_gain_3d(11.0, linear), 0.0);

        let exponential =
            sanitize_spatial_options(9, true, 1.0, 1.0, 100.0, 2.0, DistanceModel3D::Exponential);
        assert!((spatial_gain_3d(2.0, exponential) - 0.25).abs() < 1.0e-6);
        assert!(exponential.looped);
    }

    #[test]
    fn spatial_3d_options_accept_aliases_and_sanitize_invalid_ranges() -> mlua::Result<()> {
        let lua = mlua::Lua::new();
        let table = lua.create_table()?;
        table.set("voiceId", 42)?;
        table.set("minDistance", -5.0)?;
        table.set("maxDistance", -20.0)?;
        table.set("distanceModel", "linear")?;
        table.set("rolloff", -2.0)?;
        table.set("volume", 4.0)?;
        let options = parse_spatial_options(Some(table), 1);
        assert_eq!(options.voice_id, 42);
        assert_eq!(options.distance_model, DistanceModel3D::Linear);
        assert_eq!(options.min_distance, 0.001);
        assert_eq!(options.max_distance, 0.001);
        assert_eq!(options.rolloff, 0.0);
        assert_eq!(options.volume, 1.0);
        Ok(())
    }
}
