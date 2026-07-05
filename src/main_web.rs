mod assets;
mod animation;
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
    fn neolove_web_clear_canvas(r: i32, g: i32, b: i32, a: i32);
    fn neolove_web_composite_rgba(pixels: *const u8, width: i32, height: i32, x: i32, y: i32);
    fn neolove_web_draw_image(
        image_id: usize,
        revision: f64,
        pixels: *const u8,
        image_width: i32,
        image_height: i32,
        source_x: f32,
        source_y: f32,
        source_w: f32,
        source_h: f32,
        dest_x: f32,
        dest_y: f32,
        dest_w: f32,
        dest_h: f32,
        rotation: f32,
        pivot_x: f32,
        pivot_y: f32,
        alpha: f32,
        linear_filter: i32,
    );
    fn neolove_web_draw_shader(
        fragment_source: *const c_char,
        uniforms_json: *const c_char,
        vertices: *const f32,
        vertex_count: i32,
        texture_id: usize,
        texture_revision: f64,
        texture_pixels: *const u8,
        texture_width: i32,
        texture_height: i32,
        linear_filter: i32,
    );
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
        font_path: *const c_char,
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
const WEB_DEBUG_TICK_LIMIT: u32 = 0;
const WEB_DEBUG_PIXEL_SAMPLE_LIMIT: u32 = 0;

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
        render_web_commands_in_order(&mut self.renderer, &self.platform_state, pixel_commands)
            .map_err(|error| format!("web renderer failed: {error}"))?;
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
        // Frame pixels were already composited by render_web_commands_in_order.
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


fn render_web_commands_in_order(
    renderer: &mut SoftwareRenderer,
    platform: &SharedPlatformState,
    commands: Vec<DrawCommand>,
) -> Result<(), String> {
    let clear = lock_platform_state(platform).clear_color();
    unsafe {
        neolove_web_clear_canvas(clear.r as i32, clear.g as i32, clear.b as i32, clear.a as i32);
    }

    let viewport = renderer.dimensions();
    let mut pending = Vec::new();
    for command in commands {
        if !crate::renderer::command_intersects_viewport(&command, viewport.0, viewport.1) {
            continue;
        }
        if is_web_native_image(&command) {
            flush_software_chunk(renderer, viewport, std::mem::take(&mut pending))?;
            draw_web_image(command)?;
        } else if command_has_shader(&command) {
            flush_software_chunk(renderer, viewport, std::mem::take(&mut pending))?;
            draw_web_shader_command(command)?;
        } else {
            pending.push(command);
        }
    }
    let result = flush_software_chunk(renderer, viewport, pending);
    renderer.resize(viewport.0, viewport.1);
    result
}

fn is_web_native_image(command: &DrawCommand) -> bool {
    matches!(
        command,
        DrawCommand::Image {
            tint,
            shader: None,
            ..
        } if tint.r == 255 && tint.g == 255 && tint.b == 255
    )
}

fn draw_web_image(command: DrawCommand) -> Result<(), String> {
    let DrawCommand::Image {
        image,
        dest,
        source,
        rotation,
        pivot,
        tint,
        filter,
        shader: None,
    } = command
    else {
        return Err("internal web renderer error: expected an unshaded image command".to_string());
    };

    let revision = image.revision().map_err(|error| error.to_string())?;
    let image_id = image.id().map_err(|error| error.to_string())?;
    image
        .with_image(|pixels| {
            let source = source.unwrap_or(crate::renderer::Rect {
                x: 0.0,
                y: 0.0,
                w: pixels.width() as f32,
                h: pixels.height() as f32,
            });
            unsafe {
                neolove_web_draw_image(
                    image_id,
                    revision as f64,
                    pixels.as_raw().as_ptr(),
                    pixels.width() as i32,
                    pixels.height() as i32,
                    source.x,
                    source.y,
                    source.w,
                    source.h,
                    dest.x,
                    dest.y,
                    dest.w,
                    dest.h,
                    rotation,
                    pivot.x,
                    pivot.y,
                    tint.a as f32 / 255.0,
                    i32::from(matches!(filter, crate::renderer::TextureFilter::Linear)),
                );
            }
        })
        .map_err(|error| error.to_string())
}

fn flush_software_chunk(
    renderer: &mut SoftwareRenderer,
    viewport: (u32, u32),
    commands: Vec<DrawCommand>,
) -> Result<(), String> {
    if commands.is_empty() {
        return Ok(());
    }
    let Some(bounds) = crate::renderer::commands_dirty_bounds(&commands, viewport) else {
        return Ok(());
    };
    renderer.resize(bounds.w, bounds.h);
    renderer.clear_transparent();
    renderer.draw_unshaded_commands(crate::renderer::translate_commands(
        commands,
        -(bounds.x as f32),
        -(bounds.y as f32),
    ))?;
    unsafe {
        neolove_web_composite_rgba(
            renderer.pixels().as_ptr(),
            bounds.w as i32,
            bounds.h as i32,
            bounds.x as i32,
            bounds.y as i32,
        );
    }
    Ok(())
}

fn command_has_shader(command: &DrawCommand) -> bool {
    match command {
        DrawCommand::Rect { shader, .. }
        | DrawCommand::Triangle { shader, .. }
        | DrawCommand::Circle { shader, .. }
        | DrawCommand::Image { shader, .. } => shader.is_some(),
        DrawCommand::Text(_) => false,
    }
}

fn draw_web_shader_command(command: DrawCommand) -> Result<(), String> {
    let (shader, vertices, texture) = match command {
        DrawCommand::Rect {
            x,
            y,
            w,
            h,
            rotation,
            offset,
            color,
            shader: Some(shader),
        } => {
            let pivot = (x + w * offset.x, y + h * offset.y);
            let corners = [
                rotate_web_point(x, y, pivot.0, pivot.1, rotation),
                rotate_web_point(x + w, y, pivot.0, pivot.1, rotation),
                rotate_web_point(x + w, y + h, pivot.0, pivot.1, rotation),
                rotate_web_point(x, y + h, pivot.0, pivot.1, rotation),
            ];
            (
                shader,
                web_quad_vertices(
                    corners,
                    [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                    color,
                ),
                None,
            )
        }
        DrawCommand::Triangle {
            a,
            b,
            c,
            color,
            shader: Some(shader),
        } => (
            shader,
            web_vertices(&[(a, [0.0, 0.0]), (b, [1.0, 0.0]), (c, [0.5, 1.0])], color),
            None,
        ),
        DrawCommand::Circle {
            center,
            radius,
            color,
            shader: Some(shader),
        } => {
            let segments = ((radius * std::f32::consts::TAU / 4.0).ceil() as usize).clamp(24, 128);
            let mut points = Vec::with_capacity(segments * 3);
            for index in 0..segments {
                let a0 = index as f32 / segments as f32 * std::f32::consts::TAU;
                let a1 = (index + 1) as f32 / segments as f32 * std::f32::consts::TAU;
                points.push((center, [0.5, 0.5]));
                points.push((
                    crate::renderer::Vec2 {
                        x: center.x + a0.cos() * radius,
                        y: center.y + a0.sin() * radius,
                    },
                    [1.0, 0.0],
                ));
                points.push((
                    crate::renderer::Vec2 {
                        x: center.x + a1.cos() * radius,
                        y: center.y + a1.sin() * radius,
                    },
                    [0.0, 1.0],
                ));
            }
            (shader, web_vertices(&points, color), None)
        }
        DrawCommand::Image {
            image,
            dest,
            source,
            rotation,
            pivot,
            tint,
            filter,
            shader: Some(shader),
        } => {
            let (image_width, image_height) =
                image.dimensions().map_err(|error| error.to_string())?;
            let source = source.unwrap_or(crate::renderer::Rect {
                x: 0.0,
                y: 0.0,
                w: image_width as f32,
                h: image_height as f32,
            });
            let u0 = source.x / image_width.max(1) as f32;
            let v0 = source.y / image_height.max(1) as f32;
            let u1 = (source.x + source.w) / image_width.max(1) as f32;
            let v1 = (source.y + source.h) / image_height.max(1) as f32;
            let corners = [
                rotate_web_point(dest.x, dest.y, pivot.x, pivot.y, rotation),
                rotate_web_point(dest.x + dest.w, dest.y, pivot.x, pivot.y, rotation),
                rotate_web_point(dest.x + dest.w, dest.y + dest.h, pivot.x, pivot.y, rotation),
                rotate_web_point(dest.x, dest.y + dest.h, pivot.x, pivot.y, rotation),
            ];
            (
                shader,
                web_quad_vertices(corners, [[u0, v0], [u1, v0], [u1, v1], [u0, v1]], tint),
                Some((image, filter)),
            )
        }
        other => return flush_unexpected_unshaded(other),
    };

    let snapshot = shader.snapshot_for_web()?;
    let fragment_source = CString::new(snapshot.fragment_source.replace('\0', " "))
        .map_err(|error| format!("invalid shader source for web: {error}"))?;
    let uniforms_json = CString::new(snapshot.uniforms_json.replace('\0', " "))
        .map_err(|error| format!("invalid shader uniforms for web: {error}"))?;

    if let Some((image, filter)) = texture {
        let revision = image.revision().map_err(|error| error.to_string())?;
        let image_id = image.id().map_err(|error| error.to_string())?;
        image
            .with_image(|pixels| unsafe {
                neolove_web_draw_shader(
                    fragment_source.as_ptr(),
                    uniforms_json.as_ptr(),
                    vertices.as_ptr(),
                    (vertices.len() / 8) as i32,
                    image_id,
                    revision as f64,
                    pixels.as_raw().as_ptr(),
                    pixels.width() as i32,
                    pixels.height() as i32,
                    i32::from(matches!(filter, crate::renderer::TextureFilter::Linear)),
                );
            })
            .map_err(|error| error.to_string())
    } else {
        unsafe {
            neolove_web_draw_shader(
                fragment_source.as_ptr(),
                uniforms_json.as_ptr(),
                vertices.as_ptr(),
                (vertices.len() / 8) as i32,
                0,
                0.0,
                std::ptr::null(),
                0,
                0,
                0,
            );
        }
        Ok(())
    }
}

fn rotate_web_point(
    x: f32,
    y: f32,
    pivot_x: f32,
    pivot_y: f32,
    rotation: f32,
) -> crate::renderer::Vec2 {
    let dx = x - pivot_x;
    let dy = y - pivot_y;
    let cos_r = rotation.cos();
    let sin_r = rotation.sin();
    crate::renderer::Vec2 {
        x: pivot_x + dx * cos_r - dy * sin_r,
        y: pivot_y + dx * sin_r + dy * cos_r,
    }
}

fn web_quad_vertices(
    corners: [crate::renderer::Vec2; 4],
    uv: [[f32; 2]; 4],
    color: crate::platform::Color,
) -> Vec<f32> {
    web_vertices(
        &[
            (corners[0], uv[0]),
            (corners[1], uv[1]),
            (corners[2], uv[2]),
            (corners[0], uv[0]),
            (corners[2], uv[2]),
            (corners[3], uv[3]),
        ],
        color,
    )
}

fn web_vertices(
    points: &[(crate::renderer::Vec2, [f32; 2])],
    color: crate::platform::Color,
) -> Vec<f32> {
    let rgba = [
        color.r as f32 / 255.0,
        color.g as f32 / 255.0,
        color.b as f32 / 255.0,
        color.a as f32 / 255.0,
    ];
    let mut vertices = Vec::with_capacity(points.len() * 8);
    for (point, uv) in points {
        vertices.extend_from_slice(&[
            point.x, point.y, uv[0], uv[1], rgba[0], rgba[1], rgba[2], rgba[3],
        ]);
    }
    vertices
}

fn flush_unexpected_unshaded(_command: DrawCommand) -> Result<(), String> {
    Err("internal web renderer error: expected a shader command".to_string())
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

fn font_path_cstring(value: &FontHandle) -> Option<CString> {
    match value {
        FontHandle::Default => None,
        FontHandle::Path(path) => CString::new(path.replace('\0', " ")).ok(),
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
        let font_path = font_path_cstring(&request.font);
        let font_path_ptr = font_path
            .as_ref()
            .map(|value| value.as_ptr())
            .unwrap_or(std::ptr::null());
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
                font_path_ptr,
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
