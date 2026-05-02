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
mod tweening;
mod user_input;
pub mod window;

use std::env;
use std::ffi::{c_char, c_void, CString};
use std::path::PathBuf;
use std::sync::Once;

use crate::platform::{lock_platform_state, SharedPlatformState};
use crate::renderer::{
    DrawCommand, FontHandle, SoftwareRenderer, TextAlignX, TextAlignY, TextRenderRequest,
    TextScaleMode, TextWrapMode,
};

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
    fn neolove_web_draw_text(
        text: *const c_char,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        rotation: f32,
        pivot_x: f32,
        pivot_y: f32,
        r: i32,
        g: i32,
        b: i32,
        a: i32,
        scale: f32,
        min_scale: f32,
        align_x: i32,
        align_y: i32,
        text_scale: i32,
        wrap: i32,
        padding_x: f32,
        padding_y: f32,
        line_spacing: f32,
        letter_spacing: f32,
        font_kind: i32,
    );
    fn neolove_web_report_status(message: *const c_char);
    fn neolove_web_report_error(message: *const c_char);
    fn neolove_web_debug_log(message: *const c_char);
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
// Rust-side `format!` logging is useful during early startup, but leaving it on for
// hundreds of frames in the wasm build is expensive and can destabilize the debug path.
const WEB_DEBUG_TICK_LIMIT: u32 = 160;
const WEB_DEBUG_PIXEL_SAMPLE_LIMIT: u32 = 8;

struct WebApp {
    runtime: window::Runtime,
    platform_state: SharedPlatformState,
    render_state: crate::renderer::SharedRenderState,
    renderer: SoftwareRenderer,
    last_frame_time: f64,
    frame_interval: f64,
    ready_sent: bool,
    debug_frames_logged: u32,
    debug_stage_logs: u32,
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
            ready_sent: false,
            debug_frames_logged: 0,
            debug_stage_logs: 0,
        })
    }

    fn tick(&mut self) -> Result<(), String> {
        let tick_index = self.debug_stage_logs + 1;
        if tick_index <= WEB_DEBUG_TICK_LIMIT {
            debug_log(&format!("tick stage {tick_index}: begin"));
        }
        let width = unsafe { neolove_web_canvas_width() }.max(1) as u32;
        let height = unsafe { neolove_web_canvas_height() }.max(1) as u32;
        self.runtime
            .set_platform_window_state(width as f32, height as f32);
        self.runtime.set_platform_mouse_state(
            unsafe { neolove_web_mouse_x() } as f32,
            unsafe { neolove_web_mouse_y() } as f32,
        );
        self.renderer.resize(width, height);

        if tick_index <= WEB_DEBUG_TICK_LIMIT {
            debug_log(&format!(
                "tick stage {tick_index}: before sync_input {width}x{height}"
            ));
        }
        self.sync_input()?;
        if tick_index <= WEB_DEBUG_TICK_LIMIT {
            debug_log(&format!("tick stage {tick_index}: after sync_input"));
        }

        let now = unsafe { neolove_web_now_seconds() };
        let mut dt = now - self.last_frame_time;
        self.last_frame_time = now;
        if !dt.is_finite() || dt <= 0.0 {
            dt = 1.0 / 60.0;
        }
        let target_interval = self
            .runtime
            .max_fps()
            .filter(|fps| fps.is_finite() && *fps > 0.0)
            .map(|fps| 1.0 / fps as f64)
            .unwrap_or(0.0);
        if tick_index <= WEB_DEBUG_TICK_LIMIT {
            debug_log(&format!(
                "tick stage {tick_index}: after max_fps target_interval={target_interval:.6} ready_sent={}",
                self.ready_sent
            ));
        }

        if self.ready_sent && target_interval > 0.0 {
            if self.frame_interval + dt < target_interval {
                self.frame_interval += dt;
                return Ok(());
            }
            dt += self.frame_interval;
            self.frame_interval = 0.0;
        }
        let clamped_dt = dt.clamp(0.0, 0.25) as f32;

        if tick_index <= WEB_DEBUG_TICK_LIMIT {
            debug_log(&format!(
                "tick stage {tick_index}: before runtime.update dt={clamped_dt:.6}"
            ));
        }
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.runtime.update(clamped_dt)))
            .map_err(|payload| {
                format!(
                    "runtime panicked during frame update: {}",
                    lua_error::describe_panic(payload.as_ref())
                )
            })?
            .map_err(|error| format!("runtime update failed: {error}"))?;
        if tick_index <= WEB_DEBUG_TICK_LIMIT {
            debug_log(&format!("tick stage {tick_index}: after runtime.update returned"));
        }

        if tick_index <= WEB_DEBUG_TICK_LIMIT {
            debug_log(&format!("tick stage {tick_index}: before renderer.render"));
            let clear = lock_platform_state(&self.platform_state).clear_color();
            debug_log(&format!(
                "tick stage {tick_index}: pre-render exit_requested={} clear=[{},{},{},{}]",
                self.runtime.exit_requested(),
                clear.r,
                clear.g,
                clear.b,
                clear.a
            ));
        }
        let commands = crate::renderer::drain_commands(&self.render_state)
            .map_err(|error| format!("failed to drain render commands: {error}"))?;
        let (pixel_commands, text_commands) = split_text_commands(commands);
        if tick_index <= WEB_DEBUG_TICK_LIMIT {
            let (rects, triangles, circles, images) = summarize_pixel_commands(&pixel_commands);
            debug_log(&format!(
                "tick stage {tick_index}: drained commands pixel={} text={} rect={} triangle={} circle={} image={}",
                pixel_commands.len(),
                text_commands.len(),
                rects,
                triangles,
                circles,
                images
            ));
        }
        self.renderer
            .render_commands(&self.platform_state, pixel_commands)
            .map_err(|error| format!("software renderer failed: {error}"))?;
        if tick_index <= WEB_DEBUG_TICK_LIMIT {
            let clear = lock_platform_state(&self.platform_state).clear_color();
            debug_log(&format!(
                "tick stage {tick_index}: post-render exit_requested={} clear=[{},{},{},{}]",
                self.runtime.exit_requested(),
                clear.r,
                clear.g,
                clear.b,
                clear.a
            ));
        }

        if self.debug_frames_logged < WEB_DEBUG_PIXEL_SAMPLE_LIMIT {
            let pixels = self.renderer.pixels();
            let clear = lock_platform_state(&self.platform_state).clear_color();
            let non_clear = pixels
                .chunks_exact(4)
                .filter(|rgba| {
                    rgba[0] != clear.r
                        || rgba[1] != clear.g
                        || rgba[2] != clear.b
                        || rgba[3] != clear.a
                })
                .count();
            let center = ((height as usize / 2) * width as usize + (width as usize / 2)) * 4;
            let center_rgba = pixels
                .get(center..center.saturating_add(4))
                .unwrap_or(&[0, 0, 0, 0]);
            debug_log(&format!(
                "rust frame {}: {}x{} non_clear={} clear=[{},{},{},{}] center=[{},{},{},{}]",
                self.debug_frames_logged + 1,
                width,
                height,
                non_clear,
                clear.r,
                clear.g,
                clear.b,
                clear.a,
                center_rgba[0],
                center_rgba[1],
                center_rgba[2],
                center_rgba[3]
            ));
            self.debug_frames_logged += 1;
        }

        if tick_index <= WEB_DEBUG_TICK_LIMIT {
            debug_log(&format!(
                "tick stage {tick_index}: before present_rgba bytes={}",
                self.renderer.pixels().len()
            ));
        }
        unsafe {
            neolove_web_present_rgba(
                self.renderer.pixels().as_ptr(),
                width as i32,
                height as i32,
            );
        }
        if tick_index <= WEB_DEBUG_TICK_LIMIT {
            debug_log(&format!("tick stage {tick_index}: after present_rgba"));
            debug_log(&format!(
                "tick stage {tick_index}: before draw_web_text_commands count={}",
                text_commands.len()
            ));
        }
        draw_web_text_commands(&text_commands);
        if tick_index <= WEB_DEBUG_TICK_LIMIT {
            debug_log(&format!("tick stage {tick_index}: after draw_web_text_commands"));
        }
        if !self.ready_sent && width > 1 && height > 1 {
            unsafe { neolove_web_mark_ready() };
            self.ready_sent = true;
        }

        self.finish_frame(tick_index);

        if tick_index <= WEB_DEBUG_TICK_LIMIT {
            debug_log(&format!("tick stage {tick_index}: end"));
        }
        self.debug_stage_logs += 1;
        if tick_index <= WEB_DEBUG_TICK_LIMIT {
            debug_log(&format!("tick stage {tick_index}: returning ok"));
        }

        Ok(())
    }

    fn finish_frame(&mut self, tick_index: u32) {
        if tick_index <= WEB_DEBUG_TICK_LIMIT {
            debug_log(&format!("tick stage {tick_index}: before begin_frame"));
        }
        {
            let mut platform = lock_platform_state(&self.platform_state);
            platform.begin_frame();
        }
        unsafe { neolove_web_begin_frame() };
        if tick_index <= WEB_DEBUG_TICK_LIMIT {
            debug_log(&format!("tick stage {tick_index}: after begin_frame"));
        }
    }

    fn should_exit(&self) -> bool {
        self.runtime.exit_requested()
    }

    fn sync_input(&self) -> Result<(), String> {
        let mut keys_down = Vec::new();
        let mut keys_pressed = Vec::new();
        let mut keys_released = Vec::new();

        for key in WEB_KEYS {
            let name = key.name.to_string();
            let c_name = key.c_name.as_ptr() as *const c_char;

            if unsafe { neolove_web_key_state(c_name, 0) } != 0 {
                keys_down.push(name.clone());
            }

            if unsafe { neolove_web_key_state(c_name, 1) } != 0 {
                keys_pressed.push(name.clone());
            }

            if unsafe { neolove_web_key_state(c_name, 2) } != 0 {
                keys_released.push(name);
            }
        }

        let mut mouse_down = Vec::new();
        let mut mouse_pressed = Vec::new();
        let mut mouse_released = Vec::new();
        for (name, index) in WEB_MOUSE_BUTTONS {
            let button_name = (*name).to_string();
            if unsafe { neolove_web_mouse_button_state(*index, 0) } != 0 {
                mouse_down.push(button_name.clone());
            }

            if unsafe { neolove_web_mouse_button_state(*index, 1) } != 0 {
                mouse_pressed.push(button_name.clone());
            }

            if unsafe { neolove_web_mouse_button_state(*index, 2) } != 0 {
                mouse_released.push(button_name);
            }
        }

        let wheel_x = unsafe { neolove_web_wheel_x() } as f32;
        let wheel_y = unsafe { neolove_web_wheel_y() } as f32;
        let last_key = take_bridge_string(neolove_web_take_last_key)?;
        let char_pressed = take_bridge_string(neolove_web_take_char)?;

        let mut platform = lock_platform_state(&self.platform_state);
        let input = platform.input_mut();
        input.keys_down.clear();
        input.keys_down.extend(keys_down);
        input.keys_pressed.extend(keys_pressed);
        input.keys_released.extend(keys_released);
        input.mouse_down.clear();
        input.mouse_down.extend(mouse_down);
        input.mouse_pressed.extend(mouse_pressed);
        input.mouse_released.extend(mouse_released);
        input.wheel_x += wheel_x;
        input.wheel_y += wheel_y;
        if let Some(last_key) = last_key {
            input.last_key_pressed = Some(last_key);
        }
        if let Some(ch) = char_pressed {
            input.char_pressed = Some(ch);
        }

        Ok(())
    }
}

fn split_text_commands(commands: Vec<DrawCommand>) -> (Vec<DrawCommand>, Vec<TextRenderRequest>) {
    let mut pixel_commands = Vec::with_capacity(commands.len());
    let mut text_commands = Vec::new();
    for command in commands {
        match command {
            DrawCommand::Text(request) => text_commands.push(request),
            other => pixel_commands.push(other),
        }
    }
    (pixel_commands, text_commands)
}

fn summarize_pixel_commands(commands: &[DrawCommand]) -> (usize, usize, usize, usize) {
    let mut rects = 0usize;
    let mut triangles = 0usize;
    let mut circles = 0usize;
    let mut images = 0usize;
    for command in commands {
        match command {
            DrawCommand::Rect { .. } => rects += 1,
            DrawCommand::Triangle { .. } => triangles += 1,
            DrawCommand::Circle { .. } => circles += 1,
            DrawCommand::Image { .. } => images += 1,
            DrawCommand::Text(_) => {}
        }
    }
    (rects, triangles, circles, images)
}

fn text_align_x_code(value: TextAlignX) -> i32 {
    match value {
        TextAlignX::Left => 0,
        TextAlignX::Center => 1,
        TextAlignX::Right => 2,
    }
}

fn text_align_y_code(value: TextAlignY) -> i32 {
    match value {
        TextAlignY::Top => 0,
        TextAlignY::Center => 1,
        TextAlignY::Bottom => 2,
    }
}

fn text_scale_code(value: TextScaleMode) -> i32 {
    match value {
        TextScaleMode::None => 0,
        TextScaleMode::Fit => 1,
        TextScaleMode::FitWidth => 2,
        TextScaleMode::FitHeight => 3,
    }
}

fn text_wrap_code(value: TextWrapMode) -> i32 {
    match value {
        TextWrapMode::None => 0,
        TextWrapMode::Word => 1,
        TextWrapMode::Char => 2,
    }
}

fn font_kind_code(value: &FontHandle) -> i32 {
    match value {
        FontHandle::Default => 0,
        FontHandle::Path(_) => 1,
    }
}

fn draw_web_text_commands(commands: &[TextRenderRequest]) {
    for request in commands {
        if request.text.is_empty() || request.color.a == 0 {
            continue;
        }
        let sanitized = request.text.replace('\0', " ");
        let Ok(text) = CString::new(sanitized) else {
            continue;
        };
        unsafe {
            neolove_web_draw_text(
                text.as_ptr(),
                request.bounds.x,
                request.bounds.y,
                request.bounds.w,
                request.bounds.h,
                request.rotation,
                request.pivot.x,
                request.pivot.y,
                request.color.r as i32,
                request.color.g as i32,
                request.color.b as i32,
                request.color.a as i32,
                request.scale,
                request.min_scale,
                text_align_x_code(request.align_x),
                text_align_y_code(request.align_y),
                text_scale_code(request.text_scale),
                text_wrap_code(request.wrap),
                request.padding_x,
                request.padding_y,
                request.line_spacing,
                request.letter_spacing,
                font_kind_code(&request.font),
            );
        }
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

fn debug_log(message: &str) {
    let mut bytes = message
        .as_bytes()
        .iter()
        .copied()
        .filter(|byte| *byte != 0)
        .collect::<Vec<_>>();
    bytes.push(0);
    unsafe {
        neolove_web_debug_log(bytes.as_ptr() as *const c_char);
    }
}

fn install_panic_hook() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            let location = info
                .location()
                .map(|location| format!("{}:{}", location.file(), location.line()))
                .unwrap_or_else(|| "unknown location".to_string());
            let payload = if let Some(text) = info.payload().downcast_ref::<&str>() {
                (*text).to_string()
            } else if let Some(text) = info.payload().downcast_ref::<String>() {
                text.clone()
            } else {
                "non-string panic payload".to_string()
            };
            let message = format!("panic at {location}: {payload}");
            eprintln!("{message}");
        }));
    });
}

extern "C" fn web_main_loop(app_ptr: *mut c_void) {
    let app = unsafe { &mut *(app_ptr as *mut WebApp) };
    let callback_index = app.debug_stage_logs + 1;

    if callback_index <= WEB_DEBUG_TICK_LIMIT {
        debug_log(&format!("web loop {}: enter", callback_index));
    }

    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.tick())) {
        Ok(Ok(())) => {
            if callback_index <= WEB_DEBUG_TICK_LIMIT {
                debug_log(&format!("web loop {}: tick ok", callback_index));
            }
        }
        Ok(Err(error)) => {
            report_bridge_message(&error, true);
            unsafe { emscripten_cancel_main_loop() };
            return;
        }
        Err(payload) => {
            report_bridge_message(
                &format!(
                    "web main loop panicked before frame presentation: {}",
                    lua_error::describe_panic(payload.as_ref())
                ),
                true,
            );
            unsafe { emscripten_cancel_main_loop() };
            return;
        }
    }

    if callback_index <= WEB_DEBUG_TICK_LIMIT {
        debug_log(&format!("web loop {}: before should_exit", callback_index));
    }
    let should_exit = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.should_exit()))
    {
        Ok(should_exit) => should_exit,
        Err(payload) => {
            report_bridge_message(
                &format!(
                    "web main loop panicked during exit check: {}",
                    lua_error::describe_panic(payload.as_ref())
                ),
                true,
            );
            unsafe { emscripten_cancel_main_loop() };
            return;
        }
    };
    if callback_index <= WEB_DEBUG_TICK_LIMIT {
        debug_log(&format!(
            "web loop {}: after should_exit={should_exit}",
            callback_index
        ));
    }

    if should_exit {
        let reason = app
            .runtime
            .exit_reason()
            .unwrap_or_else(|| "exit_requested set without a recorded reason".to_string());
        debug_log(&format!(
            "web loop {}: cancelling main loop because should_exit=true reason={reason}",
            callback_index
        ));
        report_bridge_message(&format!("Game exited: {reason}"), true);
        unsafe { emscripten_cancel_main_loop() };
        return;
    }

    if callback_index <= WEB_DEBUG_TICK_LIMIT {
        debug_log(&format!("web loop {}: return", callback_index));
    }
}

fn main() {
    install_panic_hook();

    let app = match WebApp::new() {
        Ok(app) => app,
        Err(error) => {
            report_bridge_message(&error, true);
            return;
        }
    };

    let app = Box::into_raw(Box::new(app));
    unsafe {
        emscripten_set_main_loop_arg(web_main_loop, app.cast::<c_void>(), 0, 1);
    }
}
