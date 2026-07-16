#[cfg(target_os = "android")]
mod android_module;
#[cfg(target_os = "android")]
mod animation;
#[cfg(target_os = "android")]
mod assets;
#[cfg(target_os = "android")]
mod audio_system;
#[cfg(target_os = "android")]
mod commands;
#[cfg(target_os = "android")]
mod core;
#[cfg(target_os = "android")]
mod fs_module;
#[cfg(target_os = "android")]
pub mod hierarchy;
#[cfg(target_os = "android")]
mod http;
#[cfg(target_os = "android")]
mod lighting;
mod lua_error;
mod rng;
#[cfg(target_os = "android")]
mod mobile_emulation;
#[cfg(target_os = "android")]
mod mobile_module;
#[cfg(target_os = "android")]
mod media;
#[cfg(target_os = "android")]
mod platform;
#[cfg(target_os = "android")]
mod prefabs;
#[cfg(target_os = "android")]
mod renderer;
#[cfg(target_os = "android")]
#[allow(dead_code)]
#[path = "editor/scene.rs"]
mod scene;
#[cfg(target_os = "android")]
mod servers;
#[cfg(target_os = "android")]
mod shader;
#[cfg(target_os = "android")]
mod tweening;
#[cfg(target_os = "android")]
mod user_input;
#[cfg(target_os = "android")]
pub mod window;

#[cfg(target_os = "android")]
use std::ffi::CString;
#[cfg(target_os = "android")]
use std::fs;
#[cfg(target_os = "android")]
use std::io::Read;
#[cfg(target_os = "android")]
use std::path::{Path, PathBuf};
#[cfg(target_os = "android")]
use std::time::{Duration, Instant};

#[cfg(target_os = "android")]
use jni::JavaVM;
#[cfg(target_os = "android")]
use jni::objects::{JObject, JString, JValue};
#[cfg(target_os = "android")]
use ndk::native_window::NativeWindow;
#[cfg(target_os = "android")]
use winit::platform::android::activity::{
    AndroidApp, InputStatus, MainEvent, PollEvent, WindowManagerFlags,
    input::{Axis, InputEvent, KeyAction, Keycode, MotionAction},
};

#[cfg(target_os = "android")]
use crate::android_module::AndroidInfo;
#[cfg(target_os = "android")]
use crate::platform::{SharedPlatformState, lock_platform_state};
#[cfg(target_os = "android")]
use crate::renderer::SoftwareRenderer;

#[cfg(target_os = "android")]
const PAYLOAD_MAGIC: &[u8; 8] = b"NLPKGv1\0";
#[cfg(target_os = "android")]
const COMPRESSED_PAYLOAD_MAGIC: &[u8; 8] = b"NLPKGv2\0";
#[cfg(target_os = "android")]
const PROJECT_PAYLOAD_ASSET: &str = "neolove_project.payload";

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub fn android_main(app: AndroidApp) {
    if let Err(error) = run_android(app) {
        eprintln!("NeoLOVE Android runtime failed: {error}");
    }
}

#[cfg(target_os = "android")]
fn run_android(app: AndroidApp) -> Result<(), String> {
    crate::android_module::set_android_info(android_info_from_app(&app));
    crate::android_module::set_android_app(app.clone());
    app.set_window_flags(
        WindowManagerFlags::KEEP_SCREEN_ON,
        WindowManagerFlags::empty(),
    );
    app.enable_motion_axis(Axis::Vscroll);
    app.enable_motion_axis(Axis::Hscroll);

    let payload = read_project_payload(&app)?;
    let base_data_root = app
        .internal_data_path()
        .ok_or_else(|| "Android internal data directory is unavailable".to_string())?;
    let project_root = extract_android_project(&payload, &base_data_root)?;
    let data_root = base_data_root.join("game_data");
    fs::create_dir_all(&data_root)
        .map_err(|error| format!("failed to create Android data directory: {error}"))?;
    std::env::set_current_dir(&project_root).map_err(|error| {
        format!(
            "failed to set Android current directory to {}: {error}",
            project_root.display()
        )
    })?;

    let mut runtime = window::Runtime::with_data_root(project_root, data_root);
    runtime.start().map_err(|error| {
        format!(
            "failed to start runtime:\n{}",
            lua_error::describe_lua_error(&error)
        )
    })?;

    let platform_state = runtime.platform_state();
    let render_state = runtime.render_state();
    let mut renderer = SoftwareRenderer::new(1, 1);
    let mut native_window: Option<NativeWindow> = None;
    let mut last_update = Instant::now();
    let mut focused = true;
    let mut destroyed = false;

    while !destroyed && !runtime.exit_requested() {
        app.poll_events(Some(Duration::from_millis(0)), |event| match event {
            PollEvent::Wake | PollEvent::Timeout => {}
            PollEvent::Main(main_event) => match main_event {
                MainEvent::InitWindow { .. } => native_window = app.native_window(),
                MainEvent::TerminateWindow { .. } => native_window = None,
                MainEvent::WindowResized { .. } | MainEvent::RedrawNeeded { .. } => {
                    if native_window.is_none() {
                        native_window = app.native_window();
                    }
                }
                MainEvent::GainedFocus => focused = true,
                MainEvent::LostFocus => focused = false,
                MainEvent::Destroy => destroyed = true,
                _ => {}
            },
            _ => {}
        });

        app.input_events(|event| {
            handle_android_input(event, &platform_state);
            InputStatus::Handled
        });

        let Some(window) = native_window.as_ref() else {
            std::thread::sleep(Duration::from_millis(32));
            last_update = Instant::now();
            continue;
        };
        if !focused {
            std::thread::sleep(Duration::from_millis(32));
            last_update = Instant::now();
            continue;
        }

        let update_start = Instant::now();
        let dt = update_start.duration_since(last_update).as_secs_f32();
        last_update = update_start;

        runtime
            .update(dt.clamp(0.0, 0.25))
            .map_err(|error| format!("runtime update failed: {error}"))?;

        present_android_frame(window, &mut renderer, &platform_state, &render_state)?;

        {
            let mut platform = lock_platform_state(&platform_state);
            platform.begin_frame();
        }

        if let Some(max_fps) = runtime.max_fps() {
            let target = Duration::from_secs_f32(1.0 / max_fps.max(1.0));
            let elapsed = update_start.elapsed();
            if elapsed < target {
                std::thread::sleep(target - elapsed);
            }
        }
    }

    Ok(())
}

#[cfg(target_os = "android")]
fn present_android_frame(
    window: &NativeWindow,
    renderer: &mut SoftwareRenderer,
    platform_state: &SharedPlatformState,
    render_state: &crate::renderer::SharedRenderState,
) -> Result<(), String> {
    let width = window.width().max(1) as u32;
    let height = window.height().max(1) as u32;
    renderer.resize(width, height);
    renderer
        .render(platform_state, render_state)
        .map_err(|error| format!("Android software renderer failed: {error}"))?;

    let native_window = window.ptr().as_ptr();
    let format = ndk_sys::ANativeWindow_LegacyFormat::WINDOW_FORMAT_RGBA_8888.0 as i32;
    unsafe {
        ndk_sys::ANativeWindow_setBuffersGeometry(
            native_window,
            width as i32,
            height as i32,
            format,
        );
    }

    let mut buffer = unsafe { std::mem::zeroed::<ndk_sys::ANativeWindow_Buffer>() };
    let lock_status =
        unsafe { ndk_sys::ANativeWindow_lock(native_window, &mut buffer, std::ptr::null_mut()) };
    if lock_status != 0 {
        return Err(format!(
            "ANativeWindow_lock failed with status {lock_status}"
        ));
    }

    let copy_result = copy_rgba_to_native_buffer(renderer.pixels(), width, height, &buffer);
    let post_status = unsafe { ndk_sys::ANativeWindow_unlockAndPost(native_window) };
    if let Err(error) = copy_result {
        return Err(error);
    }
    if post_status != 0 {
        return Err(format!(
            "ANativeWindow_unlockAndPost failed with status {post_status}"
        ));
    }

    Ok(())
}

#[cfg(target_os = "android")]
fn copy_rgba_to_native_buffer(
    pixels: &[u8],
    width: u32,
    height: u32,
    buffer: &ndk_sys::ANativeWindow_Buffer,
) -> Result<(), String> {
    if buffer.bits.is_null() {
        return Err("Android native window buffer had a null pixel pointer".to_string());
    }
    let dst_width = buffer.width.max(0) as usize;
    let dst_height = buffer.height.max(0) as usize;
    let stride = buffer.stride.max(0) as usize;
    let rows = (height as usize).min(dst_height);
    let cols = (width as usize).min(dst_width).min(stride);
    let required = width as usize * height as usize * 4;
    if pixels.len() < required {
        return Err("renderer pixel buffer was smaller than the Android surface".to_string());
    }

    unsafe {
        let dst = buffer.bits as *mut u8;
        for y in 0..rows {
            let src_start = y * width as usize * 4;
            let dst_start = y * stride * 4;
            std::ptr::copy_nonoverlapping(
                pixels.as_ptr().add(src_start),
                dst.add(dst_start),
                cols * 4,
            );
        }
    }

    Ok(())
}

#[cfg(target_os = "android")]
fn handle_android_input(event: &InputEvent<'_>, platform_state: &SharedPlatformState) {
    match event {
        InputEvent::MotionEvent(motion) => {
            let pointer_count = motion.pointer_count();
            if pointer_count == 0 {
                return;
            }
            let pointer_index = motion.pointer_index().min(pointer_count - 1);
            let pointer = motion.pointer_at_index(pointer_index);
            let x = pointer.x();
            let y = pointer.y();
            let mut platform = lock_platform_state(platform_state);
            platform.set_mouse_position(x, y);
            match motion.action() {
                MotionAction::Down | MotionAction::PointerDown | MotionAction::ButtonPress => {
                    if platform.input_mut().mouse_down.insert("left".to_string()) {
                        platform
                            .input_mut()
                            .mouse_pressed
                            .insert("left".to_string());
                    }
                }
                MotionAction::Up
                | MotionAction::PointerUp
                | MotionAction::Cancel
                | MotionAction::ButtonRelease => {
                    platform.input_mut().mouse_down.remove("left");
                    platform
                        .input_mut()
                        .mouse_released
                        .insert("left".to_string());
                }
                MotionAction::Move | MotionAction::HoverMove => {}
                MotionAction::Scroll => {
                    platform.input_mut().wheel_x += pointer.axis_value(Axis::Hscroll);
                    platform.input_mut().wheel_y += pointer.axis_value(Axis::Vscroll);
                }
                _ => {}
            }
        }
        InputEvent::KeyEvent(key) => {
            let Some(name) = android_key_name(key.key_code()) else {
                return;
            };
            let mut platform = lock_platform_state(platform_state);
            match key.action() {
                KeyAction::Down => {
                    if platform.input_mut().keys_down.insert(name.to_string()) {
                        platform.input_mut().keys_pressed.insert(name.to_string());
                    }
                    platform.input_mut().last_key_pressed = Some(name.to_string());
                }
                KeyAction::Up => {
                    platform.input_mut().keys_down.remove(name);
                    platform.input_mut().keys_released.insert(name.to_string());
                }
                KeyAction::Multiple => {}
            }
        }
        _ => {}
    }
}

#[cfg(target_os = "android")]
fn android_key_name(key: Keycode) -> Option<&'static str> {
    Some(match key {
        Keycode::A => "a",
        Keycode::B => "b",
        Keycode::C => "c",
        Keycode::D => "d",
        Keycode::E => "e",
        Keycode::F => "f",
        Keycode::G => "g",
        Keycode::H => "h",
        Keycode::I => "i",
        Keycode::J => "j",
        Keycode::K => "k",
        Keycode::L => "l",
        Keycode::M => "m",
        Keycode::N => "n",
        Keycode::O => "o",
        Keycode::P => "p",
        Keycode::Q => "q",
        Keycode::R => "r",
        Keycode::S => "s",
        Keycode::T => "t",
        Keycode::U => "u",
        Keycode::V => "v",
        Keycode::W => "w",
        Keycode::X => "x",
        Keycode::Y => "y",
        Keycode::Z => "z",
        Keycode::Keycode0 => "0",
        Keycode::Keycode1 => "1",
        Keycode::Keycode2 => "2",
        Keycode::Keycode3 => "3",
        Keycode::Keycode4 => "4",
        Keycode::Keycode5 => "5",
        Keycode::Keycode6 => "6",
        Keycode::Keycode7 => "7",
        Keycode::Keycode8 => "8",
        Keycode::Keycode9 => "9",
        Keycode::Space => "space",
        Keycode::Escape => "escape",
        Keycode::Enter => "enter",
        Keycode::Tab => "tab",
        Keycode::Del => "backspace",
        Keycode::DpadLeft => "left",
        Keycode::DpadRight => "right",
        Keycode::DpadUp => "up",
        Keycode::DpadDown => "down",
        Keycode::ShiftLeft => "leftshift",
        Keycode::ShiftRight => "rightshift",
        Keycode::CtrlLeft => "leftcontrol",
        Keycode::CtrlRight => "rightcontrol",
        Keycode::AltLeft => "leftalt",
        Keycode::AltRight => "rightalt",
        Keycode::F1 => "f1",
        Keycode::F2 => "f2",
        Keycode::F3 => "f3",
        Keycode::F4 => "f4",
        Keycode::F5 => "f5",
        Keycode::F6 => "f6",
        Keycode::F7 => "f7",
        Keycode::F8 => "f8",
        Keycode::F9 => "f9",
        Keycode::F10 => "f10",
        Keycode::F11 => "f11",
        Keycode::F12 => "f12",
        Keycode::Back => "escape",
        _ => return None,
    })
}

#[cfg(target_os = "android")]
fn read_project_payload(app: &AndroidApp) -> Result<Vec<u8>, String> {
    let asset_name =
        CString::new(PROJECT_PAYLOAD_ASSET).expect("project payload asset name is static");
    let mut asset = app
        .asset_manager()
        .open(&asset_name)
        .ok_or_else(|| format!("APK asset {PROJECT_PAYLOAD_ASSET} was not found"))?;
    let mut bytes = Vec::with_capacity(asset.get_length());
    asset
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read APK project payload: {error}"))?;
    Ok(bytes)
}

#[cfg(target_os = "android")]
fn extract_android_project(payload: &[u8], base_data_root: &Path) -> Result<PathBuf, String> {
    let root = base_data_root
        .join("project_cache")
        .join(format!("{:016x}", hash64(payload)));
    let marker = root.join(".neolove_ready");
    if marker.exists() {
        return Ok(root);
    }
    if root.exists() {
        fs::remove_dir_all(&root)
            .map_err(|error| format!("failed to clean Android project cache: {error}"))?;
    }
    fs::create_dir_all(&root)
        .map_err(|error| format!("failed to create Android project cache: {error}"))?;
    unpack_payload(payload, &root)?;
    fs::write(&marker, b"ok")
        .map_err(|error| format!("failed to mark Android project cache ready: {error}"))?;
    Ok(root)
}

#[cfg(target_os = "android")]
fn read_exact<'a>(data: &'a [u8], index: &mut usize, len: usize) -> Result<&'a [u8], String> {
    if *index + len > data.len() {
        return Err("project payload is truncated".to_string());
    }
    let chunk = &data[*index..*index + len];
    *index += len;
    Ok(chunk)
}

#[cfg(target_os = "android")]
fn read_u16(data: &[u8], index: &mut usize) -> Result<u16, String> {
    let bytes = read_exact(data, index, 2)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

#[cfg(target_os = "android")]
fn read_u32(data: &[u8], index: &mut usize) -> Result<u32, String> {
    let bytes = read_exact(data, index, 4)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

#[cfg(target_os = "android")]
fn read_u64(data: &[u8], index: &mut usize) -> Result<u64, String> {
    let bytes = read_exact(data, index, 8)?;
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

#[cfg(target_os = "android")]
fn unpack_payload(payload: &[u8], output_dir: &Path) -> Result<(), String> {
    if payload.starts_with(COMPRESSED_PAYLOAD_MAGIC) {
        let cursor = std::io::Cursor::new(&payload[COMPRESSED_PAYLOAD_MAGIC.len()..]);
        let mut archive = zip::ZipArchive::new(cursor)
            .map_err(|error| format!("compressed payload is invalid: {error}"))?;
        let mut entry = archive
            .by_name("project.payload")
            .map_err(|error| format!("compressed payload has no project data: {error}"))?;
        let mut decoded = Vec::new();
        entry
            .read_to_end(&mut decoded)
            .map_err(|error| format!("failed to decompress Android project payload: {error}"))?;
        return unpack_payload(&decoded, output_dir);
    }

    let mut index = 0usize;
    let magic = read_exact(payload, &mut index, PAYLOAD_MAGIC.len())?;
    if magic != PAYLOAD_MAGIC {
        return Err("project payload magic mismatch".to_string());
    }

    let file_count = read_u32(payload, &mut index)? as usize;
    for _ in 0..file_count {
        let path_len = read_u16(payload, &mut index)? as usize;
        let path_bytes = read_exact(payload, &mut index, path_len)?;
        let rel_path = std::str::from_utf8(path_bytes)
            .map_err(|error| format!("invalid UTF-8 path in project payload: {error}"))?;
        let rel_path_buf = PathBuf::from(rel_path);
        if rel_path_buf.is_absolute()
            || rel_path_buf
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err("project payload contains an unsafe relative path".to_string());
        }

        let data_len = read_u64(payload, &mut index)? as usize;
        let file_data = read_exact(payload, &mut index, data_len)?;
        let target_path = output_dir.join(rel_path_buf);
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
        }
        fs::write(&target_path, file_data)
            .map_err(|error| format!("failed to write {}: {error}", target_path.display()))?;
    }

    if index != payload.len() {
        return Err("project payload has trailing bytes".to_string());
    }
    Ok(())
}

#[cfg(target_os = "android")]
fn hash64(data: &[u8]) -> u64 {
    let mut hash = 1469598103934665603u64;
    for byte in data {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    hash
}

#[cfg(target_os = "android")]
fn android_info_from_app(app: &AndroidApp) -> AndroidInfo {
    let mut info = AndroidInfo {
        sdk_int: Some(AndroidApp::sdk_version() as i64),
        ..AndroidInfo::default()
    };

    let Ok(vm) = (unsafe { JavaVM::from_raw(app.vm_as_ptr() as *mut jni::sys::JavaVM) }) else {
        return info;
    };
    let Ok(mut env) = vm.attach_current_thread() else {
        return info;
    };

    info.device_id = android_secure_id(&mut env, app);
    info.sdk_int = android_sdk_int(&mut env).or(info.sdk_int);
    info.brand = android_build_string(&mut env, "BRAND");
    info.manufacturer = android_build_string(&mut env, "MANUFACTURER");
    info.model = android_build_string(&mut env, "MODEL");
    info.device = android_build_string(&mut env, "DEVICE");
    info.product = android_build_string(&mut env, "PRODUCT");
    info
}

#[cfg(target_os = "android")]
fn android_secure_id(env: &mut jni::JNIEnv<'_>, app: &AndroidApp) -> Option<String> {
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as jni::sys::jobject) };
    let resolver = env
        .call_method(
            &activity,
            "getContentResolver",
            "()Landroid/content/ContentResolver;",
            &[],
        )
        .ok()?
        .l()
        .ok()?;
    let secure_class = env.find_class("android/provider/Settings$Secure").ok()?;
    let android_id_key = env
        .get_static_field(&secure_class, "ANDROID_ID", "Ljava/lang/String;")
        .ok()?
        .l()
        .ok()?;
    let value = env
        .call_static_method(
            secure_class,
            "getString",
            "(Landroid/content/ContentResolver;Ljava/lang/String;)Ljava/lang/String;",
            &[JValue::Object(&resolver), JValue::Object(&android_id_key)],
        )
        .ok()?
        .l()
        .ok()?;
    jstring_to_string(env, value)
}

#[cfg(target_os = "android")]
fn android_sdk_int(env: &mut jni::JNIEnv<'_>) -> Option<i64> {
    let version_class = env.find_class("android/os/Build$VERSION").ok()?;
    Some(
        env.get_static_field(version_class, "SDK_INT", "I")
            .ok()?
            .i()
            .ok()? as i64,
    )
}

#[cfg(target_os = "android")]
fn android_build_string(env: &mut jni::JNIEnv<'_>, field: &str) -> Option<String> {
    let build_class = env.find_class("android/os/Build").ok()?;
    let value = env
        .get_static_field(build_class, field, "Ljava/lang/String;")
        .ok()?
        .l()
        .ok()?;
    jstring_to_string(env, value)
}

#[cfg(target_os = "android")]
fn jstring_to_string(env: &mut jni::JNIEnv<'_>, value: JObject<'_>) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let text = JString::from(value);
    env.get_string(&text).ok().map(|value| value.into())
}
