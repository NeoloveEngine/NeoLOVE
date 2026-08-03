//! Developer-facing microphone and camera capture.
//!
//! Device discovery and permission requests are deliberately asynchronous.  A
//! platform is never asked for capture permission while this module is being
//! installed; hardware is opened only after `media.requestAccess(...)`.

use crate::assets::ImageHandle;
use image::RgbaImage;
use mlua::{Function, Lua, RegistryKey, Table, UserData, UserDataMethods, Value, Variadic};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, Weak};

const DEFAULT_AUDIO_READ_FRAMES: usize = 1_024;
const MAX_AUDIO_READ_FRAMES: usize = 16_384;
// Request ids must not collide when a Lua runtime is replaced while a browser
// permission promise from the old runtime is still settling.
static NEXT_MEDIA_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

fn next_media_request_id() -> u64 {
    NEXT_MEDIA_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeviceKind {
    Microphone,
    Camera,
    Both,
}

impl DeviceKind {
    fn parse(value: Option<&str>) -> mlua::Result<Self> {
        match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            None | Some("") | Some("all") | Some("both") => Ok(Self::Both),
            Some("audio") | Some("audioinput") | Some("microphone") | Some("mic") => {
                Ok(Self::Microphone)
            }
            Some("video") | Some("videoinput") | Some("camera") => Ok(Self::Camera),
            Some(other) => Err(mlua::Error::external(format!(
                "media device kind must be 'microphone', 'camera', or 'all', got '{other}'"
            ))),
        }
    }

    fn includes_microphone(self) -> bool {
        matches!(self, Self::Microphone | Self::Both)
    }

    fn includes_camera(self) -> bool {
        matches!(self, Self::Camera | Self::Both)
    }
}

#[derive(Clone, Debug, Default)]
#[cfg_attr(
    all(not(target_os = "emscripten"), not(target_os = "android")),
    allow(dead_code)
)]
struct AudioConstraints {
    device_id: Option<String>,
    sample_rate: Option<u32>,
    channels: Option<u16>,
    echo_cancellation: Option<bool>,
    noise_suppression: Option<bool>,
    auto_gain_control: Option<bool>,
}

#[derive(Clone, Debug, Default)]
#[cfg_attr(
    all(not(target_os = "emscripten"), not(target_os = "android")),
    allow(dead_code)
)]
struct VideoConstraints {
    device_id: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    frame_rate: Option<u32>,
    facing_mode: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct MediaRequestOptions {
    audio: Option<AudioConstraints>,
    video: Option<VideoConstraints>,
}

impl MediaRequestOptions {
    fn kind(&self) -> DeviceKind {
        match (self.audio.is_some(), self.video.is_some()) {
            (true, true) => DeviceKind::Both,
            (true, false) => DeviceKind::Microphone,
            (false, true) => DeviceKind::Camera,
            (false, false) => DeviceKind::Both,
        }
    }
}

#[derive(Clone, Debug)]
struct MediaDevice {
    id: String,
    kind: DeviceKind,
    label: String,
    is_default: bool,
}

#[derive(Clone, Debug)]
struct MediaError {
    code: String,
    message: String,
}

impl MediaError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    fn cancelled() -> Self {
        Self::new("cancelled", "media request was cancelled")
    }

    #[cfg(all(not(target_os = "emscripten"), not(target_os = "android")))]
    fn from_platform(message: impl Into<String>) -> Self {
        let message = message.into();
        let normalized = message.to_ascii_lowercase();
        let code = if normalized.contains("denied")
            || normalized.contains("notallowed")
            || normalized.contains("not allowed")
            || normalized.contains("permission")
        {
            "permission_denied"
        } else if normalized.contains("not found")
            || normalized.contains("no device")
            || normalized.contains("unavailable")
        {
            "device_unavailable"
        } else if normalized.contains("unsupported") || normalized.contains("not implemented") {
            "unsupported"
        } else if normalized.contains("busy") || normalized.contains("in use") {
            "device_busy"
        } else {
            "capture_failed"
        };
        Self::new(code, message)
    }
}

#[derive(Clone, Debug)]
struct AudioFormat {
    sample_rate: u32,
    channels: u16,
}

#[derive(Clone, Debug)]
struct VideoFormat {
    width: u32,
    height: u32,
    frame_rate: u32,
}

struct AccessSuccess {
    backend: backend::StreamBackend,
    audio: Option<AudioFormat>,
    video: Option<VideoFormat>,
}

enum BackendEvent {
    Devices {
        request_id: u64,
        result: Result<Vec<MediaDevice>, MediaError>,
    },
    Access {
        request_id: u64,
        result: Result<AccessSuccess, MediaError>,
    },
}

#[derive(Clone, Debug)]
struct AudioRead {
    samples: Vec<f32>,
    sample_rate: u32,
    channels: u16,
    dropped_samples: usize,
}

#[derive(Clone, Debug)]
struct VideoRead {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    timestamp: f64,
    dropped_frames: usize,
}

struct MediaStreamInner {
    backend: backend::StreamBackend,
    audio: Option<AudioFormat>,
    video: Option<VideoFormat>,
    /// One mutable asset per stream. Every captured frame updates this asset's
    /// pixels/revision instead of allocating an unbounded sequence of renderer
    /// texture IDs. Images retained by Luau are therefore live views.
    video_image: Mutex<Option<ImageHandle>>,
}

impl MediaStreamInner {
    fn stop(&self) {
        backend::stop_stream(&self.backend);
        if let Ok(mut image) = self.video_image.lock()
            && let Some(image) = image.take()
        {
            // Camera pixels are privacy-sensitive. Wipe every retained clone
            // when capture stops, but keep the handle renderable so a preview
            // sprite cannot fail a frame while gameplay tears it down.
            let _ = image.replace_rgba_image(RgbaImage::new(1, 1));
        }
    }
}

impl Drop for MediaStreamInner {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Clone)]
struct MediaStreamHandle(Arc<MediaStreamInner>);

fn update_live_video_image(
    slot: &Mutex<Option<ImageHandle>>,
    frame: VideoRead,
) -> mlua::Result<(ImageHandle, u32, u32, f64, usize)> {
    let image = RgbaImage::from_raw(frame.width, frame.height, frame.rgba)
        .ok_or_else(|| mlua::Error::external("camera returned an invalid RGBA frame"))?;
    let mut slot = slot
        .lock()
        .map_err(|_| mlua::Error::external("camera image lock poisoned"))?;
    let handle = match slot.as_ref() {
        Some(handle) => {
            handle.replace_rgba_image(image)?;
            handle.clone()
        }
        None => {
            let handle = ImageHandle::from_rgba_image(image);
            *slot = Some(handle.clone());
            handle
        }
    };
    Ok((
        handle,
        frame.width,
        frame.height,
        frame.timestamp,
        frame.dropped_frames,
    ))
}

fn audio_read_limit(value: Option<usize>) -> mlua::Result<usize> {
    let value = value.unwrap_or(DEFAULT_AUDIO_READ_FRAMES);
    if value == 0 || value > MAX_AUDIO_READ_FRAMES {
        return Err(mlua::Error::external(format!(
            "maxFrames must be between 1 and {MAX_AUDIO_READ_FRAMES}"
        )));
    }
    Ok(value)
}

fn audio_read_table(lua: &Lua, read: AudioRead, include_samples: bool) -> mlua::Result<Table> {
    let chunk = lua.create_table()?;
    chunk.set("sampleRate", read.sample_rate)?;
    chunk.set("channels", read.channels)?;
    chunk.set(
        "frameCount",
        read.samples.len() / usize::from(read.channels.max(1)),
    )?;
    chunk.set("droppedSamples", read.dropped_samples)?;
    chunk.set("format", "f32le")?;
    if include_samples {
        let samples = lua.create_table_with_capacity(read.samples.len(), 0)?;
        for (index, sample) in read.samples.into_iter().enumerate() {
            samples.raw_set(index + 1, sample)?;
        }
        chunk.set("samples", samples)?;
    } else {
        let mut bytes = Vec::with_capacity(read.samples.len() * std::mem::size_of::<f32>());
        for sample in read.samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        chunk.set("data", lua.create_string(bytes)?)?;
    }
    Ok(chunk)
}

impl UserData for MediaStreamHandle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("stop", |_lua, this, ()| {
            this.0.stop();
            Ok(())
        });
        methods.add_method("isActive", |_lua, this, ()| {
            Ok(backend::stream_is_active(&this.0.backend))
        });
        methods.add_method("hasAudio", |_lua, this, ()| Ok(this.0.audio.is_some()));
        methods.add_method("hasVideo", |_lua, this, ()| Ok(this.0.video.is_some()));
        methods.add_method("getAudioFormat", |lua, this, ()| {
            let Some(format) = &this.0.audio else {
                return Ok(Value::Nil);
            };
            let result = lua.create_table()?;
            result.set("sampleRate", format.sample_rate)?;
            result.set("channels", format.channels)?;
            Ok(Value::Table(result))
        });
        methods.add_method("getVideoFormat", |lua, this, ()| {
            let Some(format) = &this.0.video else {
                return Ok(Value::Nil);
            };
            let result = lua.create_table()?;
            result.set("width", format.width)?;
            result.set("height", format.height)?;
            result.set("frameRate", format.frame_rate)?;
            Ok(Value::Table(result))
        });
        methods.add_method("readAudio", |lua, this, max_frames: Option<usize>| {
            if this.0.audio.is_none() {
                return Err(mlua::Error::external(
                    "this media stream has no microphone track",
                ));
            }
            if !backend::stream_is_active(&this.0.backend) {
                return Err(mlua::Error::external("media stream is stopped"));
            }
            let max_frames = audio_read_limit(max_frames)?;
            let Some(read) = backend::read_audio(&this.0.backend, max_frames)? else {
                return Ok(Value::Nil);
            };
            Ok(Value::Table(audio_read_table(lua, read, true)?))
        });
        methods.add_method("readAudioBytes", |lua, this, max_frames: Option<usize>| {
            if this.0.audio.is_none() {
                return Err(mlua::Error::external(
                    "this media stream has no microphone track",
                ));
            }
            if !backend::stream_is_active(&this.0.backend) {
                return Err(mlua::Error::external("media stream is stopped"));
            }
            let max_frames = audio_read_limit(max_frames)?;
            let Some(read) = backend::read_audio(&this.0.backend, max_frames)? else {
                return Ok(Value::Nil);
            };
            Ok(Value::Table(audio_read_table(lua, read, false)?))
        });
        methods.add_method("readVideoFrame", |lua, this, ()| {
            if this.0.video.is_none() {
                return Err(mlua::Error::external(
                    "this media stream has no camera track",
                ));
            }
            if !backend::stream_is_active(&this.0.backend) {
                return Err(mlua::Error::external("media stream is stopped"));
            }
            let Some(frame) = backend::read_video(&this.0.backend)? else {
                return Ok(Value::Nil);
            };
            let (image, width, height, timestamp, dropped_frames) =
                update_live_video_image(&this.0.video_image, frame)?;
            let result = lua.create_table()?;
            result.set("image", lua.create_userdata(image)?)?;
            result.set("width", width)?;
            result.set("height", height)?;
            result.set("timestamp", timestamp)?;
            result.set("droppedFrames", dropped_frames)?;
            Ok(Value::Table(result))
        });
        methods.add_method("getLastError", |_lua, this, ()| {
            Ok(backend::stream_last_error(&this.0.backend))
        });
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PermissionStatus {
    Prompt,
    Granted,
    Denied,
    Unavailable,
}

impl PermissionStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Prompt => "prompt",
            Self::Granted => "granted",
            Self::Denied => "denied",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Clone, Copy)]
struct PermissionSet {
    microphone: PermissionStatus,
    camera: PermissionStatus,
}

struct PendingRequest {
    callback: RegistryKey,
    cancelled: Arc<AtomicBool>,
    kind: DeviceKind,
}

struct MediaState {
    pending: HashMap<u64, PendingRequest>,
    sender: Sender<BackendEvent>,
    receiver: Receiver<BackendEvent>,
    streams: Vec<Weak<MediaStreamInner>>,
    permissions: PermissionSet,
}

impl Drop for MediaState {
    fn drop(&mut self) {
        for (request_id, pending) in &self.pending {
            pending.cancelled.store(true, Ordering::Release);
            backend::cancel_request(*request_id);
        }
        for stream in &self.streams {
            if let Some(stream) = stream.upgrade() {
                stream.stop();
            }
        }
        backend::stop_all();
    }
}

fn parse_optional_u32(table: &Table, key: &str, min: u32, max: u32) -> mlua::Result<Option<u32>> {
    let Some(value) = table.get::<Option<f64>>(key)? else {
        return Ok(None);
    };
    if !value.is_finite()
        || value.fract() != 0.0
        || value < f64::from(min)
        || value > f64::from(max)
    {
        return Err(mlua::Error::external(format!(
            "{key} must be an integer between {min} and {max}"
        )));
    }
    Ok(Some(value as u32))
}

fn parse_audio_constraints(table: Table) -> mlua::Result<AudioConstraints> {
    Ok(AudioConstraints {
        device_id: table.get("deviceId")?,
        sample_rate: parse_optional_u32(&table, "sampleRate", 8_000, 384_000)?,
        channels: parse_optional_u32(&table, "channels", 1, 32)?.map(|value| value as u16),
        echo_cancellation: table.get("echoCancellation")?,
        noise_suppression: table.get("noiseSuppression")?,
        auto_gain_control: table.get("autoGainControl")?,
    })
}

fn parse_video_constraints(table: Table) -> mlua::Result<VideoConstraints> {
    let facing_mode: Option<String> = table.get("facingMode")?;
    if let Some(value) = facing_mode.as_deref()
        && !matches!(value, "user" | "environment" | "left" | "right")
    {
        return Err(mlua::Error::external(
            "facingMode must be 'user', 'environment', 'left', or 'right'",
        ));
    }
    Ok(VideoConstraints {
        device_id: table.get("deviceId")?,
        width: parse_optional_u32(&table, "width", 1, 16_384)?,
        height: parse_optional_u32(&table, "height", 1, 16_384)?,
        frame_rate: parse_optional_u32(&table, "frameRate", 1, 240)?,
        facing_mode,
    })
}

fn parse_audio_track(value: Value) -> mlua::Result<Option<AudioConstraints>> {
    match value {
        Value::Nil => Ok(None),
        Value::Boolean(false) => Ok(None),
        Value::Boolean(true) => Ok(Some(AudioConstraints::default())),
        Value::Table(table) => Ok(Some(parse_audio_constraints(table)?)),
        other => Err(mlua::Error::external(format!(
            "audio must be a boolean or constraints table, got {}",
            other.type_name()
        ))),
    }
}

fn parse_video_track(value: Value) -> mlua::Result<Option<VideoConstraints>> {
    match value {
        Value::Nil => Ok(None),
        Value::Boolean(false) => Ok(None),
        Value::Boolean(true) => Ok(Some(VideoConstraints::default())),
        Value::Table(table) => Ok(Some(parse_video_constraints(table)?)),
        other => Err(mlua::Error::external(format!(
            "video must be a boolean or constraints table, got {}",
            other.type_name()
        ))),
    }
}

fn alias_value(options: &Table, primary: &str, alias: &str) -> mlua::Result<Value> {
    let primary_value = options.get::<Value>(primary)?;
    if !matches!(primary_value, Value::Nil) {
        return Ok(primary_value);
    }
    options.get(alias)
}

fn parse_request_options(options: Table) -> mlua::Result<MediaRequestOptions> {
    let request = MediaRequestOptions {
        audio: parse_audio_track(alias_value(&options, "audio", "microphone")?)?,
        video: parse_video_track(alias_value(&options, "video", "camera")?)?,
    };
    if request.audio.is_none() && request.video.is_none() {
        return Err(mlua::Error::external(
            "media.requestAccess requires audio/microphone and/or video/camera to be enabled",
        ));
    }
    Ok(request)
}

fn device_to_lua(lua: &Lua, device: MediaDevice) -> mlua::Result<Table> {
    let result = lua.create_table()?;
    result.set("id", device.id)?;
    result.set(
        "kind",
        match device.kind {
            DeviceKind::Microphone => "microphone",
            DeviceKind::Camera => "camera",
            DeviceKind::Both => "unknown",
        },
    )?;
    result.set("label", device.label)?;
    result.set("isDefault", device.is_default)?;
    Ok(result)
}

fn error_payload(lua: &Lua, error: MediaError) -> mlua::Result<Table> {
    let payload = lua.create_table()?;
    payload.set("ok", false)?;
    payload.set("code", error.code)?;
    payload.set("error", error.message)?;
    Ok(payload)
}

fn update_permissions(
    state: &mut MediaState,
    kind: DeviceKind,
    result: &Result<AccessSuccess, MediaError>,
) {
    // A combined getUserMedia-style failure does not reliably identify which
    // of the two permissions failed. Do not falsely mark both denied.
    if kind == DeviceKind::Both && result.is_err() {
        return;
    }
    let status = match result {
        Ok(_) => Some(PermissionStatus::Granted),
        Err(error) if error.code == "permission_denied" => Some(PermissionStatus::Denied),
        Err(error) if error.code == "unsupported" => Some(PermissionStatus::Unavailable),
        Err(_) => None,
    };
    let Some(status) = status else {
        return;
    };
    if kind.includes_microphone() {
        state.permissions.microphone = status;
    }
    if kind.includes_camera() {
        state.permissions.camera = status;
    }
}

fn dispatch_events(lua: &Lua, state: &Rc<RefCell<MediaState>>) -> mlua::Result<()> {
    let sender = state.borrow().sender.clone();
    backend::poll_events(&sender);

    loop {
        let event = match state.borrow().receiver.try_recv() {
            Ok(event) => event,
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
        };
        let request_id = match &event {
            BackendEvent::Devices { request_id, .. } | BackendEvent::Access { request_id, .. } => {
                *request_id
            }
        };
        let Some(pending) = state.borrow_mut().pending.remove(&request_id) else {
            if let BackendEvent::Access {
                result: Ok(success),
                ..
            } = event
            {
                backend::stop_stream(&success.backend);
            }
            continue;
        };

        let cancelled = pending.cancelled.load(Ordering::Acquire);
        let payload = match event {
            BackendEvent::Devices { result, .. } if cancelled => {
                error_payload(lua, MediaError::cancelled())?
            }
            BackendEvent::Access { result, .. } if cancelled => {
                if let Ok(success) = result {
                    backend::stop_stream(&success.backend);
                }
                error_payload(lua, MediaError::cancelled())?
            }
            BackendEvent::Devices {
                result: Ok(devices),
                ..
            } => {
                let payload = lua.create_table()?;
                payload.set("ok", true)?;
                let list = lua.create_table_with_capacity(devices.len(), 0)?;
                for (index, device) in devices.into_iter().enumerate() {
                    list.raw_set(index + 1, device_to_lua(lua, device)?)?;
                }
                payload.set("devices", list)?;
                payload
            }
            BackendEvent::Devices {
                result: Err(error), ..
            } => error_payload(lua, error)?,
            BackendEvent::Access { result, .. } => {
                update_permissions(&mut state.borrow_mut(), pending.kind, &result);
                match result {
                    Ok(success) => {
                        let inner = Arc::new(MediaStreamInner {
                            backend: success.backend,
                            audio: success.audio,
                            video: success.video,
                            video_image: Mutex::new(None),
                        });
                        state.borrow_mut().streams.push(Arc::downgrade(&inner));
                        let payload = lua.create_table()?;
                        payload.set("ok", true)?;
                        payload.set("stream", lua.create_userdata(MediaStreamHandle(inner))?)?;
                        payload
                    }
                    Err(error) => error_payload(lua, error)?,
                }
            }
        };

        let callback: Function = lua.registry_value(&pending.callback)?;
        let call_result = crate::lua_error::protect_lua_call("running media callback", || {
            callback.call::<()>(payload)
        });
        lua.remove_registry_value(pending.callback)?;
        if let Err(error) = call_result {
            eprintln!(
                "\x1b[31mLua Error in media callback:\x1b[0m\n{}",
                crate::lua_error::describe_lua_error(&error)
            );
        }
    }

    state
        .borrow_mut()
        .streams
        .retain(|stream| stream.strong_count() > 0);
    Ok(())
}

fn permission_for(state: &MediaState, kind: DeviceKind) -> PermissionStatus {
    backend::permission_status(kind).unwrap_or(match kind {
        DeviceKind::Microphone => state.permissions.microphone,
        DeviceKind::Camera => state.permissions.camera,
        DeviceKind::Both => PermissionStatus::Unavailable,
    })
}

pub(crate) fn add_media_module(lua: &Lua) -> mlua::Result<()> {
    let (sender, receiver) = mpsc::channel();
    let state = Rc::new(RefCell::new(MediaState {
        pending: HashMap::new(),
        sender,
        receiver,
        streams: Vec::new(),
        permissions: backend::initial_permissions(),
    }));

    let module = lua.create_table()?;

    let enumerate_state = state.clone();
    module.set(
        "enumerateDevices",
        lua.create_function(move |lua, args: Variadic<Value>| {
            let (kind, callback) = match args.as_slice() {
                [Value::Function(callback)] => (DeviceKind::Both, callback.clone()),
                [Value::String(kind), Value::Function(callback)] => {
                    (DeviceKind::parse(Some(kind.to_str()?.as_ref()))?, callback.clone())
                }
                _ => {
                    return Err(mlua::Error::external(
                        "expected media.enumerateDevices(callback) or media.enumerateDevices(kind, callback)",
                    ));
                }
            };
            let (request_id, sender, cancelled) = {
                let mut state = enumerate_state.borrow_mut();
                let request_id = next_media_request_id();
                let cancelled = Arc::new(AtomicBool::new(false));
                state.pending.insert(
                    request_id,
                    PendingRequest {
                        callback: lua.create_registry_value(callback)?,
                        cancelled: cancelled.clone(),
                        kind,
                    },
                );
                (request_id, state.sender.clone(), cancelled)
            };
            backend::start_enumerate(request_id, kind, sender, cancelled);
            Ok(request_id)
        })?,
    )?;

    let request_state = state.clone();
    module.set(
        "requestAccess",
        lua.create_function(move |lua, (options, callback): (Table, Function)| {
            let options = parse_request_options(options)?;
            let kind = options.kind();
            let (request_id, sender, cancelled) = {
                let mut state = request_state.borrow_mut();
                let request_id = next_media_request_id();
                let cancelled = Arc::new(AtomicBool::new(false));
                state.pending.insert(
                    request_id,
                    PendingRequest {
                        callback: lua.create_registry_value(callback)?,
                        cancelled: cancelled.clone(),
                        kind,
                    },
                );
                (request_id, state.sender.clone(), cancelled)
            };
            backend::start_access(request_id, options, sender, cancelled);
            Ok(request_id)
        })?,
    )?;

    let cancel_state = state.clone();
    module.set(
        "cancelRequest",
        lua.create_function(move |_lua, request_id: u64| {
            let (sender, cancelled) = {
                let state = cancel_state.borrow();
                let Some(pending) = state.pending.get(&request_id) else {
                    return Ok(false);
                };
                if pending.cancelled.swap(true, Ordering::AcqRel) {
                    return Ok(false);
                }
                (state.sender.clone(), pending.cancelled.clone())
            };
            backend::cancel_request(request_id);
            let _ = sender.send(BackendEvent::Devices {
                request_id,
                result: Err(MediaError::cancelled()),
            });
            cancelled.store(true, Ordering::Release);
            Ok(true)
        })?,
    )?;

    let permission_state = state.clone();
    module.set(
        "getPermissionStatus",
        lua.create_function(move |_lua, kind: String| {
            let kind = DeviceKind::parse(Some(&kind))?;
            if kind == DeviceKind::Both {
                return Err(mlua::Error::external(
                    "getPermissionStatus expects 'microphone' or 'camera'",
                ));
            }
            Ok(permission_for(&permission_state.borrow(), kind).as_str())
        })?,
    )?;

    let permissions_state = state.clone();
    module.set(
        "permissions",
        lua.create_function(move |lua, ()| {
            let state = permissions_state.borrow();
            let result = lua.create_table()?;
            result.set(
                "microphone",
                permission_for(&state, DeviceKind::Microphone).as_str(),
            )?;
            result.set(
                "camera",
                permission_for(&state, DeviceKind::Camera).as_str(),
            )?;
            Ok(result)
        })?,
    )?;

    module.set(
        "isSupported",
        lua.create_function(move |_lua, kind: String| {
            let kind = DeviceKind::parse(Some(&kind))?;
            Ok(backend::is_supported(kind))
        })?,
    )?;

    let stop_state = state.clone();
    module.set(
        "stopAll",
        lua.create_function(move |_lua, ()| {
            let streams: Vec<_> = stop_state
                .borrow()
                .streams
                .iter()
                .filter_map(Weak::upgrade)
                .collect();
            let count = streams
                .iter()
                .filter(|stream| backend::stream_is_active(&stream.backend))
                .count();
            for stream in streams {
                stream.stop();
            }
            backend::stop_all();
            Ok(count)
        })?,
    )?;

    let poll_state = state;
    module.set(
        "_poll",
        lua.create_function(move |lua, ()| dispatch_events(lua, &poll_state))?,
    )?;

    // Focused microphone facade. It keeps the asynchronous, permission-safe
    // media backend while making the common list -> choose device -> request
    // flow discoverable without callers having to construct a mixed-track
    // request by hand.
    let microphone = lua.create_table()?;
    let enumerate_media: Function = module.get("enumerateDevices")?;
    let enumerate_microphones = lua.create_function(move |_lua, callback: Function| {
        enumerate_media.call::<u64>(("microphone", callback))
    })?;
    microphone.set("listDevices", enumerate_microphones.clone())?;
    microphone.set("enumerateDevices", enumerate_microphones)?;

    let request_media: Function = module.get("requestAccess")?;
    microphone.set(
        "requestAccess",
        lua.create_function(move |lua, args: Variadic<Value>| {
            let (audio, callback) = match args.as_slice() {
                [Value::Function(callback)] => (Value::Boolean(true), callback.clone()),
                [Value::Table(constraints), Value::Function(callback)] => {
                    (Value::Table(constraints.clone()), callback.clone())
                }
                [Value::String(device_id), Value::Function(callback)] => {
                    let constraints = lua.create_table()?;
                    constraints.set("deviceId", device_id.clone())?;
                    (Value::Table(constraints), callback.clone())
                }
                _ => {
                    return Err(mlua::Error::external(
                        "expected microphone.requestAccess(callback), microphone.requestAccess(constraints, callback), or microphone.requestAccess(deviceId, callback)",
                    ));
                }
            };
            let options = lua.create_table()?;
            options.set("audio", audio)?;
            request_media.call::<u64>((options, callback))
        })?,
    )?;

    let request_device_media: Function = module.get("requestAccess")?;
    microphone.set(
        "requestDevice",
        lua.create_function(move |lua, (device_id, callback): (String, Function)| {
            let constraints = lua.create_table()?;
            constraints.set("deviceId", device_id)?;
            let options = lua.create_table()?;
            options.set("audio", constraints)?;
            request_device_media.call::<u64>((options, callback))
        })?,
    )?;

    let permission_media: Function = module.get("getPermissionStatus")?;
    microphone.set(
        "getPermissionStatus",
        lua.create_function(move |_lua, ()| permission_media.call::<String>("microphone"))?,
    )?;
    let supported_media: Function = module.get("isSupported")?;
    microphone.set(
        "isSupported",
        lua.create_function(move |_lua, ()| supported_media.call::<bool>("microphone"))?,
    )?;
    microphone.set("cancelRequest", module.get::<Function>("cancelRequest")?)?;

    module.set("microphone", microphone.clone())?;
    lua.globals().set("microphone", microphone)?;
    lua.globals().set("media", module)
}

#[cfg(all(
    not(target_os = "emscripten"),
    not(target_os = "android"),
    any(target_os = "linux", target_os = "windows", target_os = "macos")
))]
mod backend {
    use super::*;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, Sample, SampleFormat, SizedSample, StreamConfig};
    use nokhwa::Camera;
    use nokhwa::pixel_format::RgbAFormat;
    use nokhwa::utils::{
        ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType,
    };
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    const CAPTURE_START_TIMEOUT: Duration = Duration::from_secs(60);
    const AUDIO_BUFFER_SECONDS: usize = 5;

    struct NativeAudio {
        queue: Mutex<VecDeque<f32>>,
        dropped_samples: AtomicUsize,
        max_samples: AtomicUsize,
        sample_rate: AtomicUsize,
        channels: AtomicUsize,
    }

    struct NativeVideo {
        latest: Mutex<Option<VideoRead>>,
        dropped_frames: AtomicUsize,
    }

    struct NativeShared {
        stop: AtomicBool,
        active: AtomicBool,
        last_error: Mutex<Option<String>>,
        audio: Option<Arc<NativeAudio>>,
        video: Option<Arc<NativeVideo>>,
    }

    pub(super) struct StreamBackend(Arc<NativeShared>);

    fn set_runtime_error(shared: &NativeShared, message: String) {
        if let Ok(mut error) = shared.last_error.lock() {
            *error = Some(message);
        }
        shared.active.store(false, Ordering::Release);
        shared.stop.store(true, Ordering::Release);
    }

    fn should_stop(shared: &NativeShared, cancelled: &AtomicBool) -> bool {
        shared.stop.load(Ordering::Acquire) || cancelled.load(Ordering::Acquire)
    }

    fn audio_device(device_id: Option<&str>) -> Result<cpal::Device, MediaError> {
        let host = cpal::default_host();
        let Some(device_id) = device_id.filter(|id| !id.is_empty() && *id != "audio:default")
        else {
            return host.default_input_device().ok_or_else(|| {
                MediaError::new("device_unavailable", "no default microphone is available")
            });
        };
        let raw = device_id.strip_prefix("audio:").unwrap_or(device_id);
        let index = raw.parse::<usize>().map_err(|_| {
            MediaError::new(
                "device_unavailable",
                format!("unknown microphone device id '{device_id}'"),
            )
        })?;
        host.input_devices()
            .map_err(|error| {
                MediaError::from_platform(format!("failed to enumerate microphones: {error}"))
            })?
            .nth(index)
            .ok_or_else(|| {
                MediaError::new(
                    "device_unavailable",
                    format!("microphone '{device_id}' is no longer available"),
                )
            })
    }

    fn choose_audio_config(
        device: &cpal::Device,
        constraints: &AudioConstraints,
    ) -> Result<cpal::SupportedStreamConfig, MediaError> {
        if constraints.sample_rate.is_none() && constraints.channels.is_none() {
            return device.default_input_config().map_err(|error| {
                MediaError::from_platform(format!("failed to query microphone format: {error}"))
            });
        }

        let desired_rate = constraints.sample_rate.unwrap_or(48_000);
        let desired_channels = constraints.channels.unwrap_or(1);
        let mut best: Option<(u64, cpal::SupportedStreamConfig)> = None;
        let ranges = device.supported_input_configs().map_err(|error| {
            MediaError::from_platform(format!("failed to query microphone formats: {error}"))
        })?;
        for range in ranges {
            let rate = desired_rate.clamp(range.min_sample_rate().0, range.max_sample_rate().0);
            let channel_delta = i64::from(range.channels()) - i64::from(desired_channels);
            let rate_delta = i64::from(rate) - i64::from(desired_rate);
            let score = channel_delta.unsigned_abs() * 1_000_000 + rate_delta.unsigned_abs();
            let config = range.with_sample_rate(cpal::SampleRate(rate));
            if best
                .as_ref()
                .is_none_or(|(best_score, _)| score < *best_score)
            {
                best = Some((score, config));
            }
        }
        best.map(|(_, config)| config).ok_or_else(|| {
            MediaError::new(
                "device_unavailable",
                "microphone reports no supported input formats",
            )
        })
    }

    fn push_audio<T>(input: &[T], audio: &NativeAudio)
    where
        T: Sample + Copy,
        f32: FromSample<T>,
    {
        let Ok(mut queue) = audio.queue.try_lock() else {
            audio
                .dropped_samples
                .fetch_add(input.len(), Ordering::Relaxed);
            return;
        };
        let max_samples = audio.max_samples.load(Ordering::Relaxed).max(1);
        // A backend callback can itself be larger than the whole retention
        // window. Keep only its newest tail, then make room for that tail in
        // the existing queue so the bound is never exceeded.
        let skipped_input = input.len().saturating_sub(max_samples);
        let retained_input = &input[skipped_input..];
        let evict_existing = queue
            .len()
            .saturating_add(retained_input.len())
            .saturating_sub(max_samples)
            .min(queue.len());
        for _ in 0..evict_existing {
            queue.pop_front();
        }
        audio.dropped_samples.fetch_add(
            skipped_input.saturating_add(evict_existing),
            Ordering::Relaxed,
        );
        queue.extend(retained_input.iter().copied().map(f32::from_sample));
    }

    fn build_input_stream<T>(
        device: &cpal::Device,
        config: &StreamConfig,
        shared: &Arc<NativeShared>,
        audio: &Arc<NativeAudio>,
    ) -> Result<cpal::Stream, MediaError>
    where
        T: SizedSample + Sample + Copy,
        f32: FromSample<T>,
    {
        let data = audio.clone();
        let errors = shared.clone();
        device
            .build_input_stream(
                config,
                move |input: &[T], _| push_audio(input, &data),
                move |error| {
                    set_runtime_error(&errors, format!("microphone stream failed: {error}"))
                },
                None,
            )
            .map_err(|error| {
                MediaError::from_platform(format!("failed to open microphone: {error}"))
            })
    }

    fn start_audio_worker(
        shared: Arc<NativeShared>,
        constraints: AudioConstraints,
        cancelled: Arc<AtomicBool>,
    ) -> Receiver<Result<AudioFormat, MediaError>> {
        let (ready_tx, ready_rx) = mpsc::channel();
        thread::spawn(move || {
            let result = (|| {
                if should_stop(&shared, &cancelled) {
                    return Err(MediaError::cancelled());
                }
                let device = audio_device(constraints.device_id.as_deref())?;
                let supported = choose_audio_config(&device, &constraints)?;
                let format = AudioFormat {
                    sample_rate: supported.sample_rate().0,
                    channels: supported.channels(),
                };
                let config: StreamConfig = supported.clone().into();
                let audio = shared.audio.as_ref().expect("audio capture state");
                audio
                    .sample_rate
                    .store(format.sample_rate as usize, Ordering::Relaxed);
                audio
                    .channels
                    .store(format.channels as usize, Ordering::Relaxed);
                audio.max_samples.store(
                    format.sample_rate as usize
                        * usize::from(format.channels)
                        * AUDIO_BUFFER_SECONDS,
                    Ordering::Relaxed,
                );
                let stream = match supported.sample_format() {
                    SampleFormat::I8 => build_input_stream::<i8>(&device, &config, &shared, audio),
                    SampleFormat::I16 => {
                        build_input_stream::<i16>(&device, &config, &shared, audio)
                    }
                    SampleFormat::I32 => {
                        build_input_stream::<i32>(&device, &config, &shared, audio)
                    }
                    SampleFormat::I64 => {
                        build_input_stream::<i64>(&device, &config, &shared, audio)
                    }
                    SampleFormat::U8 => build_input_stream::<u8>(&device, &config, &shared, audio),
                    SampleFormat::U16 => {
                        build_input_stream::<u16>(&device, &config, &shared, audio)
                    }
                    SampleFormat::U32 => {
                        build_input_stream::<u32>(&device, &config, &shared, audio)
                    }
                    SampleFormat::U64 => {
                        build_input_stream::<u64>(&device, &config, &shared, audio)
                    }
                    SampleFormat::F32 => {
                        build_input_stream::<f32>(&device, &config, &shared, audio)
                    }
                    SampleFormat::F64 => {
                        build_input_stream::<f64>(&device, &config, &shared, audio)
                    }
                    other => Err(MediaError::new(
                        "unsupported_format",
                        format!("microphone sample format '{other}' is unsupported"),
                    )),
                }?;
                stream.play().map_err(|error| {
                    MediaError::from_platform(format!("failed to start microphone: {error}"))
                })?;
                let _ = ready_tx.send(Ok(format));
                while !should_stop(&shared, &cancelled) {
                    thread::sleep(Duration::from_millis(20));
                }
                drop(stream);
                Ok(())
            })();
            if let Err(error) = result {
                let _ = ready_tx.send(Err(error));
            }
        });
        ready_rx
    }

    fn camera_index(device_id: Option<&str>) -> Result<CameraIndex, MediaError> {
        let devices = nokhwa::query(ApiBackend::Auto).map_err(|error| {
            MediaError::from_platform(format!("failed to enumerate cameras: {error}"))
        })?;
        let Some(device_id) = device_id.filter(|id| !id.is_empty() && *id != "camera:default")
        else {
            return devices
                .first()
                .map(|device| device.index().clone())
                .ok_or_else(|| MediaError::new("device_unavailable", "no camera is available"));
        };
        let raw = device_id.strip_prefix("camera:").unwrap_or(device_id);
        devices
            .into_iter()
            .find(|device| device.index().as_string() == raw)
            .map(|device| device.index().clone())
            .ok_or_else(|| {
                MediaError::new(
                    "device_unavailable",
                    format!("camera '{device_id}' is no longer available"),
                )
            })
    }

    fn open_camera(
        index: CameraIndex,
        constraints: &VideoConstraints,
    ) -> Result<Camera, MediaError> {
        let width = constraints.width.unwrap_or(640);
        let height = constraints.height.unwrap_or(480);
        let fps = constraints.frame_rate.unwrap_or(30);
        let mut last_error = None;
        for frame_format in [
            FrameFormat::MJPEG,
            FrameFormat::YUYV,
            FrameFormat::NV12,
            FrameFormat::RAWRGB,
            FrameFormat::RAWBGR,
        ] {
            let requested = RequestedFormat::new::<RgbAFormat>(RequestedFormatType::Closest(
                CameraFormat::new_from(width, height, frame_format, fps),
            ));
            match Camera::new(index.clone(), requested) {
                Ok(camera) => return Ok(camera),
                Err(error) => last_error = Some(error.to_string()),
            }
        }
        Err(MediaError::from_platform(format!(
            "failed to configure camera: {}",
            last_error.unwrap_or_else(|| "no decodable format is available".to_string())
        )))
    }

    #[cfg(target_os = "macos")]
    fn request_camera_permission(cancelled: &AtomicBool) -> Result<(), MediaError> {
        let (sender, receiver) = mpsc::channel();
        nokhwa::nokhwa_initialize(move |granted| {
            let _ = sender.send(granted);
        });
        let started = Instant::now();
        loop {
            if cancelled.load(Ordering::Acquire) {
                return Err(MediaError::cancelled());
            }
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(true) => return Ok(()),
                Ok(false) => {
                    return Err(MediaError::new(
                        "permission_denied",
                        "camera permission was denied",
                    ));
                }
                Err(mpsc::RecvTimeoutError::Timeout)
                    if started.elapsed() < CAPTURE_START_TIMEOUT => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(MediaError::new(
                        "permission_denied",
                        "timed out waiting for camera permission",
                    ));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(MediaError::new(
                        "capture_failed",
                        "camera permission request was interrupted",
                    ));
                }
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn request_camera_permission(cancelled: &AtomicBool) -> Result<(), MediaError> {
        if cancelled.load(Ordering::Acquire) {
            Err(MediaError::cancelled())
        } else {
            Ok(())
        }
    }

    fn start_video_worker(
        shared: Arc<NativeShared>,
        constraints: VideoConstraints,
        cancelled: Arc<AtomicBool>,
    ) -> Receiver<Result<VideoFormat, MediaError>> {
        let (ready_tx, ready_rx) = mpsc::channel();
        thread::spawn(move || {
            let result = (|| {
                request_camera_permission(&cancelled)?;
                if should_stop(&shared, &cancelled) {
                    return Err(MediaError::cancelled());
                }
                let index = camera_index(constraints.device_id.as_deref())?;
                let mut camera = open_camera(index, &constraints)?;
                camera.open_stream().map_err(|error| {
                    MediaError::from_platform(format!("failed to start camera: {error}"))
                })?;
                let camera_format = camera.camera_format();
                let format = VideoFormat {
                    width: camera_format.width(),
                    height: camera_format.height(),
                    frame_rate: camera_format.frame_rate(),
                };
                let _ = ready_tx.send(Ok(format));
                let video = shared.video.as_ref().expect("video capture state");
                let started = Instant::now();
                let mut consecutive_errors = 0u8;
                while !should_stop(&shared, &cancelled) {
                    match camera
                        .frame()
                        .and_then(|frame| frame.decode_image::<RgbAFormat>())
                    {
                        Ok(frame) => {
                            consecutive_errors = 0;
                            let next = VideoRead {
                                width: frame.width(),
                                height: frame.height(),
                                rgba: frame.into_raw(),
                                timestamp: started.elapsed().as_secs_f64(),
                                dropped_frames: 0,
                            };
                            if let Ok(mut latest) = video.latest.lock() {
                                if latest.replace(next).is_some() {
                                    video.dropped_frames.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                        Err(error) => {
                            consecutive_errors = consecutive_errors.saturating_add(1);
                            if consecutive_errors >= 10 {
                                set_runtime_error(
                                    &shared,
                                    format!("camera stream failed: {error}"),
                                );
                                break;
                            }
                            thread::sleep(Duration::from_millis(10));
                        }
                    }
                }
                let _ = camera.stop_stream();
                Ok(())
            })();
            if let Err(error) = result {
                let _ = ready_tx.send(Err(error));
            }
        });
        ready_rx
    }

    fn wait_ready<T>(
        receiver: &Receiver<Result<T, MediaError>>,
        cancelled: &AtomicBool,
    ) -> Result<T, MediaError> {
        let started = Instant::now();
        loop {
            if cancelled.load(Ordering::Acquire) {
                return Err(MediaError::cancelled());
            }
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(result) => return result,
                Err(mpsc::RecvTimeoutError::Timeout)
                    if started.elapsed() < CAPTURE_START_TIMEOUT => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(MediaError::new(
                        "capture_failed",
                        "timed out while opening the media device",
                    ));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(MediaError::new(
                        "capture_failed",
                        "media capture worker stopped during startup",
                    ));
                }
            }
        }
    }

    pub(super) fn start_access(
        request_id: u64,
        options: MediaRequestOptions,
        sender: Sender<BackendEvent>,
        cancelled: Arc<AtomicBool>,
    ) {
        thread::spawn(move || {
            if cancelled.load(Ordering::Acquire) {
                return;
            }
            let audio_state = options.audio.as_ref().map(|_| {
                Arc::new(NativeAudio {
                    queue: Mutex::new(VecDeque::new()),
                    dropped_samples: AtomicUsize::new(0),
                    max_samples: AtomicUsize::new(1),
                    sample_rate: AtomicUsize::new(0),
                    channels: AtomicUsize::new(0),
                })
            });
            let video_state = options.video.as_ref().map(|_| {
                Arc::new(NativeVideo {
                    latest: Mutex::new(None),
                    dropped_frames: AtomicUsize::new(0),
                })
            });
            let shared = Arc::new(NativeShared {
                stop: AtomicBool::new(false),
                active: AtomicBool::new(false),
                last_error: Mutex::new(None),
                audio: audio_state,
                video: video_state,
            });

            let result = (|| {
                let audio_receiver = options.audio.clone().map(|constraints| {
                    start_audio_worker(shared.clone(), constraints, cancelled.clone())
                });
                let audio = match audio_receiver {
                    Some(receiver) => Some(wait_ready(&receiver, &cancelled)?),
                    None => None,
                };

                let video_receiver = options.video.clone().map(|constraints| {
                    start_video_worker(shared.clone(), constraints, cancelled.clone())
                });
                let video = match video_receiver {
                    Some(receiver) => Some(wait_ready(&receiver, &cancelled)?),
                    None => None,
                };

                if cancelled.load(Ordering::Acquire) {
                    return Err(MediaError::cancelled());
                }
                shared.active.store(true, Ordering::Release);
                Ok(AccessSuccess {
                    backend: StreamBackend(shared.clone()),
                    audio,
                    video,
                })
            })();

            if result.is_err() {
                shared.stop.store(true, Ordering::Release);
            }
            let _ = sender.send(BackendEvent::Access { request_id, result });
        });
    }

    fn enumerate_microphones() -> Result<Vec<MediaDevice>, MediaError> {
        let host = cpal::default_host();
        let default_name = host
            .default_input_device()
            .and_then(|device| device.name().ok());
        let devices = host.input_devices().map_err(|error| {
            MediaError::from_platform(format!("failed to enumerate microphones: {error}"))
        })?;
        Ok(devices
            .enumerate()
            .map(|(index, device)| {
                let label = device
                    .name()
                    .unwrap_or_else(|_| format!("Microphone {}", index + 1));
                MediaDevice {
                    id: format!("audio:{index}"),
                    is_default: default_name.as_deref() == Some(label.as_str()),
                    kind: DeviceKind::Microphone,
                    label,
                }
            })
            .collect())
    }

    fn enumerate_cameras() -> Result<Vec<MediaDevice>, MediaError> {
        let devices = nokhwa::query(ApiBackend::Auto).map_err(|error| {
            MediaError::from_platform(format!("failed to enumerate cameras: {error}"))
        })?;
        Ok(devices
            .into_iter()
            .enumerate()
            .map(|(position, device)| MediaDevice {
                id: format!("camera:{}", device.index().as_string()),
                label: device.human_name(),
                kind: DeviceKind::Camera,
                is_default: position == 0,
            })
            .collect())
    }

    pub(super) fn start_enumerate(
        request_id: u64,
        kind: DeviceKind,
        sender: Sender<BackendEvent>,
        cancelled: Arc<AtomicBool>,
    ) {
        thread::spawn(move || {
            if cancelled.load(Ordering::Acquire) {
                return;
            }
            let result = (|| {
                let mut devices = Vec::new();
                let mut errors = Vec::new();
                if kind.includes_microphone() {
                    match enumerate_microphones() {
                        Ok(mut microphones) => devices.append(&mut microphones),
                        Err(error) => errors.push(error),
                    }
                }
                if kind.includes_camera() {
                    match enumerate_cameras() {
                        Ok(mut cameras) => devices.append(&mut cameras),
                        Err(error) => errors.push(error),
                    }
                }
                if devices.is_empty() && !errors.is_empty() {
                    Err(errors.remove(0))
                } else {
                    Ok(devices)
                }
            })();
            let _ = sender.send(BackendEvent::Devices { request_id, result });
        });
    }

    pub(super) fn poll_events(_sender: &Sender<BackendEvent>) {}

    pub(super) fn cancel_request(_request_id: u64) {}

    pub(super) fn stop_stream(stream: &StreamBackend) {
        stream.0.active.store(false, Ordering::Release);
        stream.0.stop.store(true, Ordering::Release);
        if let Some(audio) = &stream.0.audio {
            if let Ok(mut queue) = audio.queue.lock() {
                queue.clear();
            }
            audio.dropped_samples.store(0, Ordering::Relaxed);
        }
        if let Some(video) = &stream.0.video {
            if let Ok(mut latest) = video.latest.lock() {
                *latest = None;
            }
            video.dropped_frames.store(0, Ordering::Relaxed);
        }
    }

    pub(super) fn stream_is_active(stream: &StreamBackend) -> bool {
        stream.0.active.load(Ordering::Acquire) && !stream.0.stop.load(Ordering::Acquire)
    }

    pub(super) fn read_audio(
        stream: &StreamBackend,
        max_frames: usize,
    ) -> mlua::Result<Option<AudioRead>> {
        let Some(audio) = stream.0.audio.as_ref() else {
            return Err(mlua::Error::external(
                "this media stream has no microphone track",
            ));
        };
        let channels = audio.channels.load(Ordering::Relaxed).max(1);
        let max_samples = max_frames.saturating_mul(channels);
        let mut queue = audio
            .queue
            .lock()
            .map_err(|_| mlua::Error::external("microphone buffer lock poisoned"))?;
        let samples_to_read = queue.len().min(max_samples);
        let samples_to_read = samples_to_read - samples_to_read % channels;
        if samples_to_read == 0 {
            return Ok(None);
        }
        let samples = queue.drain(..samples_to_read).collect();
        Ok(Some(AudioRead {
            samples,
            sample_rate: audio.sample_rate.load(Ordering::Relaxed) as u32,
            channels: channels as u16,
            dropped_samples: audio.dropped_samples.swap(0, Ordering::Relaxed),
        }))
    }

    pub(super) fn read_video(stream: &StreamBackend) -> mlua::Result<Option<VideoRead>> {
        let Some(video) = stream.0.video.as_ref() else {
            return Err(mlua::Error::external(
                "this media stream has no camera track",
            ));
        };
        let mut latest = video
            .latest
            .lock()
            .map_err(|_| mlua::Error::external("camera frame lock poisoned"))?;
        let Some(mut frame) = latest.take() else {
            return Ok(None);
        };
        frame.dropped_frames = video.dropped_frames.swap(0, Ordering::Relaxed);
        Ok(Some(frame))
    }

    pub(super) fn stream_last_error(stream: &StreamBackend) -> Option<String> {
        stream
            .0
            .last_error
            .lock()
            .ok()
            .and_then(|error| error.clone())
    }

    pub(super) fn initial_permissions() -> PermissionSet {
        PermissionSet {
            microphone: PermissionStatus::Prompt,
            camera: PermissionStatus::Prompt,
        }
    }

    pub(super) fn permission_status(_kind: DeviceKind) -> Option<PermissionStatus> {
        None
    }

    pub(super) fn is_supported(kind: DeviceKind) -> bool {
        match kind {
            DeviceKind::Microphone | DeviceKind::Camera | DeviceKind::Both => true,
        }
    }

    pub(super) fn stop_all() {}

    #[cfg(test)]
    pub(super) fn test_stream() -> (StreamBackend, Arc<AtomicBool>) {
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(NativeShared {
            stop: AtomicBool::new(false),
            active: AtomicBool::new(true),
            last_error: Mutex::new(None),
            audio: None,
            video: None,
        });
        // Mirror the native stop flag into a probe from a tiny observer thread.
        let observed = stop.clone();
        let watched = shared.clone();
        thread::spawn(move || {
            while !watched.stop.load(Ordering::Acquire) {
                thread::yield_now();
            }
            observed.store(true, Ordering::Release);
        });
        (StreamBackend(shared), stop)
    }

    #[cfg(test)]
    mod native_tests {
        use super::*;

        #[test]
        fn oversized_callback_keeps_only_newest_samples() {
            let audio = NativeAudio {
                queue: Mutex::new(VecDeque::from([90.0, 91.0])),
                dropped_samples: AtomicUsize::new(0),
                max_samples: AtomicUsize::new(4),
                sample_rate: AtomicUsize::new(48_000),
                channels: AtomicUsize::new(1),
            };
            push_audio(&[0.0f32, 1.0, 2.0, 3.0, 4.0, 5.0], &audio);
            let samples: Vec<_> = audio
                .queue
                .lock()
                .expect("audio queue lock should remain available")
                .iter()
                .copied()
                .collect();
            assert_eq!(samples, vec![2.0, 3.0, 4.0, 5.0]);
            assert_eq!(audio.dropped_samples.load(Ordering::Relaxed), 4);
        }

        #[test]
        fn stop_clears_buffered_microphone_and_camera_data() {
            let audio = Arc::new(NativeAudio {
                queue: Mutex::new(VecDeque::from([0.25, -0.25])),
                dropped_samples: AtomicUsize::new(3),
                max_samples: AtomicUsize::new(10),
                sample_rate: AtomicUsize::new(48_000),
                channels: AtomicUsize::new(1),
            });
            let video = Arc::new(NativeVideo {
                latest: Mutex::new(Some(VideoRead {
                    rgba: vec![1, 2, 3, 255],
                    width: 1,
                    height: 1,
                    timestamp: 0.0,
                    dropped_frames: 0,
                })),
                dropped_frames: AtomicUsize::new(4),
            });
            let backend = StreamBackend(Arc::new(NativeShared {
                stop: AtomicBool::new(false),
                active: AtomicBool::new(true),
                last_error: Mutex::new(None),
                audio: Some(audio.clone()),
                video: Some(video.clone()),
            }));

            stop_stream(&backend);

            assert!(
                audio
                    .queue
                    .lock()
                    .expect("audio queue lock should remain available")
                    .is_empty()
            );
            assert_eq!(audio.dropped_samples.load(Ordering::Relaxed), 0);
            assert!(
                video
                    .latest
                    .lock()
                    .expect("video frame lock should remain available")
                    .is_none()
            );
            assert_eq!(video.dropped_frames.load(Ordering::Relaxed), 0);
        }
    }
}

#[cfg(target_os = "emscripten")]
mod backend {
    use super::*;
    use serde_json::{Value as JsonValue, json};
    use std::ffi::{CString, c_char};

    unsafe extern "C" {
        fn neolove_web_media_enumerate(request_id: i32, kind: i32) -> i32;
        fn neolove_web_media_request(request_id: i32, constraints_json: *const c_char) -> i32;
        fn neolove_web_media_cancel(request_id: i32);
        fn neolove_web_media_poll(
            request_id: *mut i32,
            event_kind: *mut i32,
            ok: *mut i32,
            stream_id: *mut i32,
        ) -> i32;
        fn neolove_web_media_copy_event_field(
            field: i32,
            buffer: *mut c_char,
            capacity: i32,
        ) -> i32;
        fn neolove_web_media_permission(kind: i32, buffer: *mut c_char, capacity: i32) -> i32;
        fn neolove_web_media_supported(kind: i32) -> i32;
        fn neolove_web_media_stop(stream_id: i32);
        fn neolove_web_media_stop_all();
        fn neolove_web_media_is_active(stream_id: i32) -> i32;
        fn neolove_web_media_audio_info(
            stream_id: i32,
            sample_rate: *mut i32,
            channels: *mut i32,
            available_samples: *mut i32,
            dropped_samples: *mut i32,
        ) -> i32;
        fn neolove_web_media_read_audio(stream_id: i32, samples: *mut f32, max_samples: i32)
        -> i32;
        fn neolove_web_media_video_info(
            stream_id: i32,
            width: *mut i32,
            height: *mut i32,
            timestamp: *mut f64,
            dropped_frames: *mut i32,
        ) -> i32;
        fn neolove_web_media_copy_video(stream_id: i32, pixels: *mut u8, capacity: i32) -> i32;
        fn neolove_web_media_copy_stream_error(
            stream_id: i32,
            buffer: *mut c_char,
            capacity: i32,
        ) -> i32;
    }

    pub(super) struct StreamBackend {
        id: i32,
    }

    fn kind_code(kind: DeviceKind) -> i32 {
        match kind {
            DeviceKind::Both => 0,
            DeviceKind::Microphone => 1,
            DeviceKind::Camera => 2,
        }
    }

    fn copy_string(mut copy: impl FnMut(*mut c_char, i32) -> i32) -> Option<String> {
        let required = copy(std::ptr::null_mut(), 0);
        if required == 0 {
            return Some(String::new());
        }
        let capacity = if required < 0 {
            required.checked_neg()?
        } else {
            required.saturating_add(1)
        };
        if capacity <= 0 {
            return None;
        }
        let mut bytes = vec![0u8; capacity as usize];
        let written = copy(bytes.as_mut_ptr().cast(), capacity);
        if written < 0 {
            return None;
        }
        bytes.truncate(written as usize);
        String::from_utf8(bytes).ok()
    }

    fn event_field(field: i32) -> String {
        copy_string(|buffer, capacity| unsafe {
            neolove_web_media_copy_event_field(field, buffer, capacity)
        })
        .unwrap_or_default()
    }

    fn audio_json(constraints: &AudioConstraints) -> JsonValue {
        let mut value = serde_json::Map::new();
        if let Some(device_id) = &constraints.device_id {
            value.insert("deviceId".to_string(), json!(device_id));
        }
        if let Some(sample_rate) = constraints.sample_rate {
            value.insert("sampleRate".to_string(), json!(sample_rate));
        }
        if let Some(channels) = constraints.channels {
            value.insert("channels".to_string(), json!(channels));
        }
        if let Some(enabled) = constraints.echo_cancellation {
            value.insert("echoCancellation".to_string(), json!(enabled));
        }
        if let Some(enabled) = constraints.noise_suppression {
            value.insert("noiseSuppression".to_string(), json!(enabled));
        }
        if let Some(enabled) = constraints.auto_gain_control {
            value.insert("autoGainControl".to_string(), json!(enabled));
        }
        JsonValue::Object(value)
    }

    fn video_json(constraints: &VideoConstraints) -> JsonValue {
        let mut value = serde_json::Map::new();
        if let Some(device_id) = &constraints.device_id {
            value.insert("deviceId".to_string(), json!(device_id));
        }
        if let Some(width) = constraints.width {
            value.insert("width".to_string(), json!(width));
        }
        if let Some(height) = constraints.height {
            value.insert("height".to_string(), json!(height));
        }
        if let Some(frame_rate) = constraints.frame_rate {
            value.insert("frameRate".to_string(), json!(frame_rate));
        }
        if let Some(facing_mode) = &constraints.facing_mode {
            value.insert("facingMode".to_string(), json!(facing_mode));
        }
        JsonValue::Object(value)
    }

    pub(super) fn start_access(
        request_id: u64,
        options: MediaRequestOptions,
        sender: Sender<BackendEvent>,
        cancelled: Arc<AtomicBool>,
    ) {
        if cancelled.load(Ordering::Acquire) {
            return;
        }
        let request_id_i32 = match i32::try_from(request_id) {
            Ok(value) => value,
            Err(_) => {
                let _ = sender.send(BackendEvent::Access {
                    request_id,
                    result: Err(MediaError::new(
                        "capture_failed",
                        "media request id overflow",
                    )),
                });
                return;
            }
        };
        let constraints = json!({
            "audio": options.audio.as_ref().map(audio_json).unwrap_or(JsonValue::Bool(false)),
            "video": options.video.as_ref().map(video_json).unwrap_or(JsonValue::Bool(false)),
        });
        let encoded = match CString::new(constraints.to_string()) {
            Ok(value) => value,
            Err(_) => {
                let _ = sender.send(BackendEvent::Access {
                    request_id,
                    result: Err(MediaError::new(
                        "invalid_options",
                        "media constraints contain a NUL byte",
                    )),
                });
                return;
            }
        };
        if unsafe { neolove_web_media_request(request_id_i32, encoded.as_ptr()) } == 0 {
            let _ = sender.send(BackendEvent::Access {
                request_id,
                result: Err(MediaError::new(
                    "unsupported",
                    "browser media capture could not be started",
                )),
            });
        }
    }

    pub(super) fn start_enumerate(
        request_id: u64,
        kind: DeviceKind,
        sender: Sender<BackendEvent>,
        cancelled: Arc<AtomicBool>,
    ) {
        if cancelled.load(Ordering::Acquire) {
            return;
        }
        let Ok(request_id_i32) = i32::try_from(request_id) else {
            let _ = sender.send(BackendEvent::Devices {
                request_id,
                result: Err(MediaError::new(
                    "capture_failed",
                    "media request id overflow",
                )),
            });
            return;
        };
        if unsafe { neolove_web_media_enumerate(request_id_i32, kind_code(kind)) } == 0 {
            let _ = sender.send(BackendEvent::Devices {
                request_id,
                result: Err(MediaError::new(
                    "unsupported",
                    "browser device enumeration is unavailable",
                )),
            });
        }
    }

    fn parse_devices(payload: &str) -> Result<Vec<MediaDevice>, MediaError> {
        let value: JsonValue = serde_json::from_str(payload).map_err(|error| {
            MediaError::new(
                "capture_failed",
                format!("invalid browser device response: {error}"),
            )
        })?;
        let list = value
            .get("devices")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| {
                MediaError::new(
                    "capture_failed",
                    "browser device response has no devices list",
                )
            })?;
        Ok(list
            .iter()
            .filter_map(|device| {
                let kind = match device.get("kind")?.as_str()? {
                    "microphone" => DeviceKind::Microphone,
                    "camera" => DeviceKind::Camera,
                    _ => return None,
                };
                Some(MediaDevice {
                    id: device.get("id")?.as_str()?.to_string(),
                    kind,
                    label: device
                        .get("label")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("")
                        .to_string(),
                    is_default: device
                        .get("isDefault")
                        .and_then(JsonValue::as_bool)
                        .unwrap_or(false),
                })
            })
            .collect())
    }

    fn parse_access(stream_id: i32, payload: &str) -> Result<AccessSuccess, MediaError> {
        let value: JsonValue = serde_json::from_str(payload).map_err(|error| {
            MediaError::new(
                "capture_failed",
                format!("invalid browser stream response: {error}"),
            )
        })?;
        let audio = value
            .get("audio")
            .filter(|value| !value.is_null())
            .map(|value| AudioFormat {
                sample_rate: value
                    .get("sampleRate")
                    .and_then(JsonValue::as_u64)
                    .unwrap_or(48_000) as u32,
                channels: value
                    .get("channels")
                    .and_then(JsonValue::as_u64)
                    .unwrap_or(1) as u16,
            });
        let video = value
            .get("video")
            .filter(|value| !value.is_null())
            .map(|value| VideoFormat {
                width: value.get("width").and_then(JsonValue::as_u64).unwrap_or(0) as u32,
                height: value.get("height").and_then(JsonValue::as_u64).unwrap_or(0) as u32,
                frame_rate: value
                    .get("frameRate")
                    .and_then(JsonValue::as_f64)
                    .unwrap_or(0.0)
                    .round() as u32,
            });
        Ok(AccessSuccess {
            backend: StreamBackend { id: stream_id },
            audio,
            video,
        })
    }

    pub(super) fn poll_events(sender: &Sender<BackendEvent>) {
        loop {
            let mut request_id = 0;
            let mut event_kind = 0;
            let mut ok = 0;
            let mut stream_id = -1;
            if unsafe {
                neolove_web_media_poll(&mut request_id, &mut event_kind, &mut ok, &mut stream_id)
            } == 0
            {
                break;
            }
            let request_id = request_id.max(0) as u64;
            let payload = event_field(0);
            let error = event_field(1);
            let code = event_field(2);
            let platform_error = || {
                MediaError::new(
                    if code.is_empty() {
                        "capture_failed"
                    } else {
                        code.as_str()
                    },
                    if error.is_empty() {
                        "browser media request failed"
                    } else {
                        error.as_str()
                    },
                )
            };
            let event = if event_kind == 0 {
                BackendEvent::Devices {
                    request_id,
                    result: if ok != 0 {
                        parse_devices(&payload)
                    } else {
                        Err(platform_error())
                    },
                }
            } else {
                BackendEvent::Access {
                    request_id,
                    result: if ok != 0 {
                        parse_access(stream_id, &payload)
                    } else {
                        Err(platform_error())
                    },
                }
            };
            let _ = sender.send(event);
        }
    }

    pub(super) fn cancel_request(request_id: u64) {
        if let Ok(request_id) = i32::try_from(request_id) {
            unsafe { neolove_web_media_cancel(request_id) };
        }
    }

    pub(super) fn stop_stream(stream: &StreamBackend) {
        unsafe { neolove_web_media_stop(stream.id) };
    }

    pub(super) fn stream_is_active(stream: &StreamBackend) -> bool {
        unsafe { neolove_web_media_is_active(stream.id) != 0 }
    }

    pub(super) fn read_audio(
        stream: &StreamBackend,
        max_frames: usize,
    ) -> mlua::Result<Option<AudioRead>> {
        let mut sample_rate = 0;
        let mut channels = 0;
        let mut available = 0;
        let mut dropped = 0;
        if unsafe {
            neolove_web_media_audio_info(
                stream.id,
                &mut sample_rate,
                &mut channels,
                &mut available,
                &mut dropped,
            )
        } == 0
        {
            return Err(mlua::Error::external(
                "this media stream has no microphone track",
            ));
        }
        let channels = channels.max(1) as usize;
        let wanted = available.max(0) as usize;
        let wanted = wanted.min(max_frames.saturating_mul(channels));
        let wanted = wanted - wanted % channels;
        if wanted == 0 {
            return Ok(None);
        }
        let mut samples = vec![0.0f32; wanted];
        let read =
            unsafe { neolove_web_media_read_audio(stream.id, samples.as_mut_ptr(), wanted as i32) };
        if read <= 0 {
            return Ok(None);
        }
        samples.truncate(read as usize);
        Ok(Some(AudioRead {
            samples,
            sample_rate: sample_rate.max(0) as u32,
            channels: channels as u16,
            dropped_samples: dropped.max(0) as usize,
        }))
    }

    pub(super) fn read_video(stream: &StreamBackend) -> mlua::Result<Option<VideoRead>> {
        let mut width = 0;
        let mut height = 0;
        let mut timestamp = 0.0;
        let mut dropped = 0;
        let required = unsafe {
            neolove_web_media_video_info(
                stream.id,
                &mut width,
                &mut height,
                &mut timestamp,
                &mut dropped,
            )
        };
        if required == -1 {
            return Err(mlua::Error::external(
                "this media stream has no camera track",
            ));
        }
        if required < 0 {
            return Err(mlua::Error::external(
                stream_last_error(stream)
                    .unwrap_or_else(|| "failed to read browser camera frame".to_string()),
            ));
        }
        if required == 0 {
            return Ok(None);
        }
        let mut rgba = vec![0u8; required as usize];
        let written =
            unsafe { neolove_web_media_copy_video(stream.id, rgba.as_mut_ptr(), required) };
        if written <= 0 {
            return Ok(None);
        }
        rgba.truncate(written as usize);
        Ok(Some(VideoRead {
            rgba,
            width: width.max(0) as u32,
            height: height.max(0) as u32,
            timestamp,
            dropped_frames: dropped.max(0) as usize,
        }))
    }

    pub(super) fn stream_last_error(stream: &StreamBackend) -> Option<String> {
        copy_string(|buffer, capacity| unsafe {
            neolove_web_media_copy_stream_error(stream.id, buffer, capacity)
        })
        .filter(|message| !message.is_empty())
    }

    pub(super) fn initial_permissions() -> PermissionSet {
        PermissionSet {
            microphone: PermissionStatus::Prompt,
            camera: PermissionStatus::Prompt,
        }
    }

    pub(super) fn permission_status(kind: DeviceKind) -> Option<PermissionStatus> {
        let value = copy_string(|buffer, capacity| unsafe {
            neolove_web_media_permission(kind_code(kind), buffer, capacity)
        })?;
        match value.as_str() {
            "prompt" => Some(PermissionStatus::Prompt),
            "granted" => Some(PermissionStatus::Granted),
            "denied" => Some(PermissionStatus::Denied),
            "unavailable" => Some(PermissionStatus::Unavailable),
            _ => None,
        }
    }

    pub(super) fn is_supported(kind: DeviceKind) -> bool {
        unsafe { neolove_web_media_supported(kind_code(kind)) != 0 }
    }

    pub(super) fn stop_all() {
        unsafe { neolove_web_media_stop_all() };
    }
}

#[cfg(target_os = "android")]
mod backend {
    use super::*;

    pub(super) struct StreamBackend;

    pub(super) fn start_access(
        request_id: u64,
        _options: MediaRequestOptions,
        sender: Sender<BackendEvent>,
        cancelled: Arc<AtomicBool>,
    ) {
        if cancelled.load(Ordering::Acquire) {
            return;
        }
        let _ = sender.send(BackendEvent::Access {
            request_id,
            result: Err(MediaError::new(
                "unsupported",
                "native Android microphone/camera capture is not available in this build",
            )),
        });
    }

    pub(super) fn start_enumerate(
        request_id: u64,
        _kind: DeviceKind,
        sender: Sender<BackendEvent>,
        cancelled: Arc<AtomicBool>,
    ) {
        if !cancelled.load(Ordering::Acquire) {
            let _ = sender.send(BackendEvent::Devices {
                request_id,
                result: Ok(Vec::new()),
            });
        }
    }

    pub(super) fn poll_events(_sender: &Sender<BackendEvent>) {}
    pub(super) fn cancel_request(_request_id: u64) {}
    pub(super) fn stop_stream(_stream: &StreamBackend) {}
    pub(super) fn stream_is_active(_stream: &StreamBackend) -> bool {
        false
    }
    pub(super) fn read_audio(
        _stream: &StreamBackend,
        _max_frames: usize,
    ) -> mlua::Result<Option<AudioRead>> {
        Ok(None)
    }
    pub(super) fn read_video(_stream: &StreamBackend) -> mlua::Result<Option<VideoRead>> {
        Ok(None)
    }
    pub(super) fn stream_last_error(_stream: &StreamBackend) -> Option<String> {
        None
    }
    pub(super) fn initial_permissions() -> PermissionSet {
        PermissionSet {
            microphone: PermissionStatus::Unavailable,
            camera: PermissionStatus::Unavailable,
        }
    }
    pub(super) fn permission_status(_kind: DeviceKind) -> Option<PermissionStatus> {
        None
    }
    pub(super) fn is_supported(_kind: DeviceKind) -> bool {
        false
    }
    pub(super) fn stop_all() {}
}

// Keep the module available on less common native targets even when NeoLOVE
// has no capture backend for them yet. This avoids accidentally compiling the
// Linux/Windows/macOS nokhwa implementation where its dependency is absent.
#[cfg(not(any(
    target_os = "emscripten",
    target_os = "android",
    target_os = "linux",
    target_os = "windows",
    target_os = "macos"
)))]
mod backend {
    use super::*;

    pub(super) struct StreamBackend;

    pub(super) fn start_access(
        request_id: u64,
        _options: MediaRequestOptions,
        sender: Sender<BackendEvent>,
        cancelled: Arc<AtomicBool>,
    ) {
        if !cancelled.load(Ordering::Acquire) {
            let _ = sender.send(BackendEvent::Access {
                request_id,
                result: Err(MediaError::new(
                    "unsupported",
                    "microphone/camera capture is not available on this platform",
                )),
            });
        }
    }

    pub(super) fn start_enumerate(
        request_id: u64,
        _kind: DeviceKind,
        sender: Sender<BackendEvent>,
        cancelled: Arc<AtomicBool>,
    ) {
        if !cancelled.load(Ordering::Acquire) {
            let _ = sender.send(BackendEvent::Devices {
                request_id,
                result: Ok(Vec::new()),
            });
        }
    }

    pub(super) fn poll_events(_sender: &Sender<BackendEvent>) {}
    pub(super) fn cancel_request(_request_id: u64) {}
    pub(super) fn stop_stream(_stream: &StreamBackend) {}
    pub(super) fn stream_is_active(_stream: &StreamBackend) -> bool {
        false
    }
    pub(super) fn read_audio(
        _stream: &StreamBackend,
        _max_frames: usize,
    ) -> mlua::Result<Option<AudioRead>> {
        Ok(None)
    }
    pub(super) fn read_video(_stream: &StreamBackend) -> mlua::Result<Option<VideoRead>> {
        Ok(None)
    }
    pub(super) fn stream_last_error(_stream: &StreamBackend) -> Option<String> {
        None
    }
    pub(super) fn initial_permissions() -> PermissionSet {
        PermissionSet {
            microphone: PermissionStatus::Unavailable,
            camera: PermissionStatus::Unavailable,
        }
    }
    pub(super) fn permission_status(_kind: DeviceKind) -> Option<PermissionStatus> {
        None
    }
    pub(super) fn is_supported(_kind: DeviceKind) -> bool {
        false
    }
    pub(super) fn stop_all() {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn parses_track_aliases_and_constraints() -> mlua::Result<()> {
        let lua = Lua::new();
        let options = lua.create_table()?;
        let microphone = lua.create_table()?;
        microphone.set("sampleRate", 48_000)?;
        microphone.set("channels", 2)?;
        options.set("microphone", microphone)?;
        options.set("camera", true)?;
        let parsed = parse_request_options(options)?;
        assert_eq!(
            parsed.audio.as_ref().and_then(|audio| audio.sample_rate),
            Some(48_000)
        );
        assert_eq!(
            parsed.audio.as_ref().and_then(|audio| audio.channels),
            Some(2)
        );
        assert!(parsed.video.is_some());
        Ok(())
    }

    #[test]
    fn rejects_empty_or_invalid_requests() -> mlua::Result<()> {
        let lua = Lua::new();
        assert!(parse_request_options(lua.create_table()?).is_err());
        let options = lua.create_table()?;
        let camera = lua.create_table()?;
        camera.set("frameRate", 0)?;
        options.set("video", camera)?;
        assert!(parse_request_options(options).is_err());
        Ok(())
    }

    #[test]
    fn module_registration_does_not_request_capture() -> mlua::Result<()> {
        let lua = Lua::new();
        add_media_module(&lua)?;
        let media: Table = lua.globals().get("media")?;
        for name in [
            "enumerateDevices",
            "requestAccess",
            "cancelRequest",
            "getPermissionStatus",
            "permissions",
            "isSupported",
            "stopAll",
            "_poll",
        ] {
            let _: Function = media.get(name)?;
        }
        let permissions: Table = media.get::<Function>("permissions")?.call(())?;
        assert_eq!(permissions.get::<String>("microphone")?, "prompt");
        assert_eq!(permissions.get::<String>("camera")?, "prompt");
        let microphone: Table = lua.globals().get("microphone")?;
        let nested: Table = media.get("microphone")?;
        assert_eq!(microphone.to_pointer(), nested.to_pointer());
        for name in [
            "listDevices",
            "enumerateDevices",
            "requestAccess",
            "requestDevice",
            "cancelRequest",
            "getPermissionStatus",
            "isSupported",
        ] {
            let _: Function = microphone.get(name)?;
        }
        assert_eq!(
            microphone
                .get::<Function>("getPermissionStatus")?
                .call::<String>(())?,
            "prompt"
        );
        Ok(())
    }

    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    #[test]
    fn dropping_last_stream_handle_stops_native_capture() {
        let (backend, stopped) = backend::test_stream();
        let inner = Arc::new(MediaStreamInner {
            backend,
            audio: None,
            video: None,
            video_image: Mutex::new(None),
        });
        let clone = inner.clone();
        drop(inner);
        assert!(!stopped.load(Ordering::Acquire));
        drop(clone);
        let started = Instant::now();
        while !stopped.load(Ordering::Acquire) && started.elapsed() < Duration::from_secs(1) {
            std::thread::yield_now();
        }
        assert!(stopped.load(Ordering::Acquire));
    }

    #[test]
    fn cancelled_request_callback_is_dispatched_once() -> mlua::Result<()> {
        let lua = Lua::new();
        let calls = Rc::new(RefCell::new(0usize));
        let seen_code = Rc::new(RefCell::new(String::new()));
        let calls_callback = calls.clone();
        let code_callback = seen_code.clone();
        let callback = lua.create_function(move |_lua, payload: Table| {
            *calls_callback.borrow_mut() += 1;
            *code_callback.borrow_mut() = payload.get("code")?;
            Ok(())
        })?;
        let (sender, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(true));
        let mut pending = HashMap::new();
        pending.insert(
            7,
            PendingRequest {
                callback: lua.create_registry_value(callback)?,
                cancelled,
                kind: DeviceKind::Microphone,
            },
        );
        sender
            .send(BackendEvent::Devices {
                request_id: 7,
                result: Err(MediaError::cancelled()),
            })
            .expect("cancelled device event should enter the test queue");
        sender
            .send(BackendEvent::Devices {
                request_id: 7,
                result: Ok(Vec::new()),
            })
            .expect("duplicate device event should enter the test queue");
        let state = Rc::new(RefCell::new(MediaState {
            pending,
            sender,
            receiver,
            streams: Vec::new(),
            permissions: backend::initial_permissions(),
        }));
        dispatch_events(&lua, &state)?;
        assert_eq!(*calls.borrow(), 1);
        assert_eq!(seen_code.borrow().as_str(), "cancelled");
        Ok(())
    }

    #[test]
    fn camera_frames_reuse_image_identity_and_advance_revision() -> mlua::Result<()> {
        let slot = Mutex::new(None);
        let (first, ..) = update_live_video_image(
            &slot,
            VideoRead {
                rgba: vec![255, 0, 0, 255],
                width: 1,
                height: 1,
                timestamp: 1.0,
                dropped_frames: 0,
            },
        )?;
        let (first_id, first_revision) = first.test_identity_revision();
        let (second, ..) = update_live_video_image(
            &slot,
            VideoRead {
                rgba: vec![0, 255, 0, 255],
                width: 1,
                height: 1,
                timestamp: 2.0,
                dropped_frames: 0,
            },
        )?;
        let (second_id, second_revision) = second.test_identity_revision();
        assert_eq!(first_id, second_id);
        assert_eq!(second_revision, first_revision.wrapping_add(1));
        assert_eq!(first.sample_rgba(0, 0)?, [0, 255, 0, 255]);
        Ok(())
    }

    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    #[test]
    fn stopping_stream_wipes_retained_camera_image_without_unloading_it() {
        let (backend, _stopped) = backend::test_stream();
        let image = ImageHandle::from_rgba_image(
            RgbaImage::from_raw(1, 1, vec![7, 8, 9, 255])
                .expect("one RGBA pixel should form a 1x1 image"),
        );
        let retained = image.clone();
        let stream = MediaStreamInner {
            backend,
            audio: None,
            video: Some(VideoFormat {
                width: 1,
                height: 1,
                frame_rate: 30,
            }),
            video_image: Mutex::new(Some(image)),
        };

        stream.stop();

        assert_eq!(retained.dimensions().expect("wiped dimensions"), (1, 1));
        assert_eq!(
            retained.sample_rgba(0, 0).expect("wiped pixel"),
            [0, 0, 0, 0]
        );
    }
}
