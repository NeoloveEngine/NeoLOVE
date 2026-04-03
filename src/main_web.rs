mod assets;
mod audio_system;
mod commands;
mod core;
mod fs_module;
pub mod hierarchy;
mod http;
mod lua_error;
mod platform;
mod prefabs;
mod renderer;
mod servers;
mod shader;
mod user_input;
pub mod window;

use std::env;
use std::ffi::{c_char, c_void};
use std::path::PathBuf;

use crate::platform::SharedPlatformState;
use crate::renderer::SoftwareRenderer;

unsafe extern "C" {
    fn emscripten_set_main_loop_arg(
        func: extern "C" fn(*mut c_void),
        arg: *mut c_void,
        fps: i32,
        simulate_infinite_loop: i32,
    );
    fn emscripten_cancel_main_loop();

    fn neolove_web_bootstrap();
    fn neolove_web_now_seconds() -> f64;
    fn neolove_web_canvas_width() -> i32;
    fn neolove_web_canvas_height() -> i32;
    fn neolove_web_mouse_x() -> f64;
    fn neolove_web_mouse_y() -> f64;
    fn neolove_web_mouse_button_state(index: i32, kind: i32) -> i32;
    fn neolove_web_wheel_x() -> f64;
    fn neolove_web_wheel_y() -> f64;
    fn neolove_web_key_state(name: *const c_char, kind: i32) -> i32;
    fn neolove_web_take_last_key(buffer: *mut c_char, capacity: i32) -> i32;
    fn neolove_web_take_char(buffer: *mut c_char, capacity: i32) -> i32;
    fn neolove_web_begin_frame();
    fn neolove_web_present_rgba(pixels: *const u8, width: i32, height: i32);
    fn neolove_web_report_status(message: *const c_char);
    fn neolove_web_report_error(message: *const c_char);
    fn neolove_web_mark_ready();
}

struct WebKey {
    name: &'static str,
    c_name: &'static [u8],
}

const WEB_KEYS: &[WebKey] = &[
    WebKey { name: "a", c_name: b"a\0" },
    WebKey { name: "b", c_name: b"b\0" },
    WebKey { name: "c", c_name: b"c\0" },
    WebKey { name: "d", c_name: b"d\0" },
    WebKey { name: "e", c_name: b"e\0" },
    WebKey { name: "f", c_name: b"f\0" },
    WebKey { name: "g", c_name: b"g\0" },
    WebKey { name: "h", c_name: b"h\0" },
    WebKey { name: "i", c_name: b"i\0" },
    WebKey { name: "j", c_name: b"j\0" },
    WebKey { name: "k", c_name: b"k\0" },
    WebKey { name: "l", c_name: b"l\0" },
    WebKey { name: "m", c_name: b"m\0" },
    WebKey { name: "n", c_name: b"n\0" },
    WebKey { name: "o", c_name: b"o\0" },
    WebKey { name: "p", c_name: b"p\0" },
    WebKey { name: "q", c_name: b"q\0" },
    WebKey { name: "r", c_name: b"r\0" },
    WebKey { name: "s", c_name: b"s\0" },
    WebKey { name: "t", c_name: b"t\0" },
    WebKey { name: "u", c_name: b"u\0" },
    WebKey { name: "v", c_name: b"v\0" },
    WebKey { name: "w", c_name: b"w\0" },
    WebKey { name: "x", c_name: b"x\0" },
    WebKey { name: "y", c_name: b"y\0" },
    WebKey { name: "z", c_name: b"z\0" },
    WebKey { name: "0", c_name: b"0\0" },
    WebKey { name: "1", c_name: b"1\0" },
    WebKey { name: "2", c_name: b"2\0" },
    WebKey { name: "3", c_name: b"3\0" },
    WebKey { name: "4", c_name: b"4\0" },
    WebKey { name: "5", c_name: b"5\0" },
    WebKey { name: "6", c_name: b"6\0" },
    WebKey { name: "7", c_name: b"7\0" },
    WebKey { name: "8", c_name: b"8\0" },
    WebKey { name: "9", c_name: b"9\0" },
    WebKey { name: "space", c_name: b"space\0" },
    WebKey { name: "escape", c_name: b"escape\0" },
    WebKey { name: "enter", c_name: b"enter\0" },
    WebKey { name: "tab", c_name: b"tab\0" },
    WebKey { name: "backspace", c_name: b"backspace\0" },
    WebKey { name: "left", c_name: b"left\0" },
    WebKey { name: "right", c_name: b"right\0" },
    WebKey { name: "up", c_name: b"up\0" },
    WebKey { name: "down", c_name: b"down\0" },
    WebKey { name: "leftshift", c_name: b"leftshift\0" },
    WebKey { name: "rightshift", c_name: b"rightshift\0" },
    WebKey { name: "leftcontrol", c_name: b"leftcontrol\0" },
    WebKey { name: "rightcontrol", c_name: b"rightcontrol\0" },
    WebKey { name: "leftalt", c_name: b"leftalt\0" },
    WebKey { name: "rightalt", c_name: b"rightalt\0" },
    WebKey { name: "leftsuper", c_name: b"leftsuper\0" },
    WebKey { name: "rightsuper", c_name: b"rightsuper\0" },
    WebKey { name: "f1", c_name: b"f1\0" },
    WebKey { name: "f2", c_name: b"f2\0" },
    WebKey { name: "f3", c_name: b"f3\0" },
    WebKey { name: "f4", c_name: b"f4\0" },
    WebKey { name: "f5", c_name: b"f5\0" },
    WebKey { name: "f6", c_name: b"f6\0" },
    WebKey { name: "f7", c_name: b"f7\0" },
    WebKey { name: "f8", c_name: b"f8\0" },
    WebKey { name: "f9", c_name: b"f9\0" },
    WebKey { name: "f10", c_name: b"f10\0" },
    WebKey { name: "f11", c_name: b"f11\0" },
    WebKey { name: "f12", c_name: b"f12\0" },
];

const WEB_MOUSE_BUTTONS: &[(&str, i32)] = &[("left", 0), ("middle", 1), ("right", 2), ("other", 3)];

struct WebApp {
    runtime: window::Runtime,
    platform_state: SharedPlatformState,
    render_state: crate::renderer::SharedRenderState,
    renderer: SoftwareRenderer,
    last_frame_time: f64,
    frame_interval: f64,
}

impl WebApp {
    fn new() -> Result<Self, String> {
        unsafe { neolove_web_bootstrap() };

        let width = unsafe { neolove_web_canvas_width() }.max(1) as u32;
        let height = unsafe { neolove_web_canvas_height() }.max(1) as u32;
        let project_root = PathBuf::from("/project");
        env::set_current_dir(&project_root).map_err(|error| {
            format!(
                "failed to set current directory to {}: {error}",
                project_root.display()
            )
        })?;

        let mut runtime = window::Runtime::new(project_root.clone());
        runtime.set_platform_window_state(width as f32, height as f32);

        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.start()))
            .map_err(|payload| {
                format!(
                    "runtime panicked during startup: {}",
                    lua_error::describe_panic(payload.as_ref())
                )
            })?
            .map_err(|error| {
                format!(
                    "failed to start runtime:\n{}",
                    lua_error::describe_lua_error(&error)
                )
            })?;

        let platform_state = runtime.platform_state();
        let render_state = runtime.render_state();

        Ok(Self {
            runtime,
            platform_state,
            render_state,
            renderer: SoftwareRenderer::new(width, height),
            last_frame_time: unsafe { neolove_web_now_seconds() },
            frame_interval: 0.0,
        })
    }

    fn tick(&mut self) -> Result<(), String> {
        let width = unsafe { neolove_web_canvas_width() }.max(1) as u32;
        let height = unsafe { neolove_web_canvas_height() }.max(1) as u32;
        self.runtime
            .set_platform_window_state(width as f32, height as f32);
        self.runtime.set_platform_mouse_state(
            unsafe { neolove_web_mouse_x() } as f32,
            unsafe { neolove_web_mouse_y() } as f32,
        );
        self.renderer.resize(width, height);

        self.sync_input()?;

        let now = unsafe { neolove_web_now_seconds() };
        let mut dt = (now - self.last_frame_time).max(0.0);
        self.last_frame_time = now;
        let target_interval = self
            .runtime
            .max_fps()
            .filter(|fps| fps.is_finite() && *fps > 0.0)
            .map(|fps| 1.0 / fps as f64)
            .unwrap_or(0.0);

        if target_interval > 0.0 {
            if self.frame_interval + dt < target_interval {
                self.frame_interval += dt;
                return Ok(());
            }
            dt += self.frame_interval;
            self.frame_interval = 0.0;
        }
        let clamped_dt = dt.clamp(0.0, 0.25) as f32;

        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.runtime.update(clamped_dt)))
            .map_err(|payload| {
                format!(
                    "runtime panicked during frame update: {}",
                    lua_error::describe_panic(payload.as_ref())
                )
            })?
            .map_err(|error| format!("runtime update failed: {error}"))?;

        self.renderer
            .render(&self.platform_state, &self.render_state)
            .map_err(|error| format!("software renderer failed: {error}"))?;

        unsafe {
            neolove_web_present_rgba(
                self.renderer.pixels().as_ptr(),
                width as i32,
                height as i32,
            );
        }

        self.platform_state
            .lock()
            .map_err(|_| "platform state lock poisoned while finalizing frame input".to_string())?
            .begin_frame();
        unsafe { neolove_web_begin_frame() };

        Ok(())
    }

    fn should_exit(&self) -> bool {
        self.runtime.exit_requested()
    }

    fn sync_input(&self) -> Result<(), String> {
        let mut platform = self
            .platform_state
            .lock()
            .map_err(|_| "platform state lock poisoned".to_string())?;

        for key in WEB_KEYS {
            let name = key.name.to_string();
            let c_name = key.c_name.as_ptr() as *const c_char;

            if unsafe { neolove_web_key_state(c_name, 0) } != 0 {
                platform.input_mut().keys_down.insert(name.clone());
            } else {
                platform.input_mut().keys_down.remove(name.as_str());
            }

            if unsafe { neolove_web_key_state(c_name, 1) } != 0 {
                platform.input_mut().keys_pressed.insert(name.clone());
            }

            if unsafe { neolove_web_key_state(c_name, 2) } != 0 {
                platform.input_mut().keys_released.insert(name);
            }
        }

        for (name, index) in WEB_MOUSE_BUTTONS {
            let button_name = (*name).to_string();
            if unsafe { neolove_web_mouse_button_state(*index, 0) } != 0 {
                platform.input_mut().mouse_down.insert(button_name.clone());
            } else {
                platform.input_mut().mouse_down.remove(button_name.as_str());
            }

            if unsafe { neolove_web_mouse_button_state(*index, 1) } != 0 {
                platform.input_mut().mouse_pressed.insert(button_name.clone());
            }

            if unsafe { neolove_web_mouse_button_state(*index, 2) } != 0 {
                platform.input_mut().mouse_released.insert(button_name);
            }
        }

        platform.input_mut().wheel_x += unsafe { neolove_web_wheel_x() } as f32;
        platform.input_mut().wheel_y += unsafe { neolove_web_wheel_y() } as f32;

        if let Some(last_key) = take_bridge_string(neolove_web_take_last_key)? {
            platform.input_mut().last_key_pressed = Some(last_key);
        }

        if let Some(ch) = take_bridge_string(neolove_web_take_char)? {
            platform.input_mut().char_pressed = Some(ch);
        }

        Ok(())
    }
}

fn take_bridge_string(
    reader: unsafe extern "C" fn(*mut c_char, i32) -> i32,
) -> Result<Option<String>, String> {
    let mut buffer = [0u8; 64];
    let written = unsafe { reader(buffer.as_mut_ptr() as *mut c_char, buffer.len() as i32) };
    if written == 0 {
        return Ok(None);
    }
    if written < 0 {
        return Err(format!(
            "web input bridge buffer too small: need {} bytes",
            written.unsigned_abs()
        ));
    }
    let bytes = &buffer[..written as usize];
    String::from_utf8(bytes.to_vec())
        .map(Some)
        .map_err(|error| format!("web input bridge returned invalid UTF-8: {error}"))
}

fn report_bridge_message(message: &str, is_error: bool) {
    let mut bytes = message
        .as_bytes()
        .iter()
        .copied()
        .filter(|byte| *byte != 0)
        .collect::<Vec<_>>();
    bytes.push(0);

    unsafe {
        if is_error {
            neolove_web_report_error(bytes.as_ptr() as *const c_char);
        } else {
            neolove_web_report_status(bytes.as_ptr() as *const c_char);
        }
    }
}

extern "C" fn web_main_loop(app_ptr: *mut c_void) {
    let app = unsafe { &mut *(app_ptr as *mut WebApp) };

    if let Err(error) = app.tick() {
        report_bridge_message(&error, true);
        unsafe { emscripten_cancel_main_loop() };
        return;
    }

    if app.should_exit() {
        report_bridge_message("Game exited.", false);
        unsafe { emscripten_cancel_main_loop() };
    }
}

fn main() {
    let app = match WebApp::new() {
        Ok(app) => app,
        Err(error) => {
            report_bridge_message(&error, true);
            return;
        }
    };

    unsafe { neolove_web_mark_ready() };

    let app = Box::into_raw(Box::new(app));
    unsafe {
        emscripten_set_main_loop_arg(web_main_loop, app.cast::<c_void>(), 0, 1);
    }
}
