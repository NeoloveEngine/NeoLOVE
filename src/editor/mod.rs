//! The NeoLOVE visual editor.
//!
//! A lightweight, self-contained scene editor inspired by the Unity and Godot
//! editors. It opens a window with a dockable hierarchy, a 2D viewport and an
//! inspector, lets you build a scene out of entities and components (including
//! script components with inspector-exposed public variables), saves it as
//! JSON, and exports a runnable `main.luau` for the NeoLOVE runtime.
//!
//! The editor reuses the project's existing dependencies (winit, softbuffer and
//! fontdue) but renders its own immediate-mode UI, so it shares no state with
//! the Lua-driven game runtime. Its appearance is themeable via `editor.json`,
//! which defaults to a Visual Studio Code "Dark+" palette.

mod app;
mod scene;
mod ui;

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use winit::dpi::LogicalSize;
use winit::event::{
    ElementState, Event, KeyboardInput, MouseButton, MouseScrollDelta, VirtualKeyCode, WindowEvent,
};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::window::WindowBuilder;

use app::EditorApp;
use scene::Scene;
use ui::{FrameInput, Fonts, Painter, Ui};

const DEFAULT_SCENE_FILE: &str = "scene.neoscene";
const CONFIG_FILE: &str = "editor.json";
const WINDOW_W: f64 = 1280.0;
const WINDOW_H: f64 = 760.0;

/// Launch the visual editor for the project rooted at `project_root`.
///
/// A `scene.neoscene` file in the project is loaded if present; otherwise a
/// starter scene is created. Editor appearance and dock layout are read from
/// `editor.json`, which is created with defaults on first launch so it can be
/// customized. Saving and exporting write back into the project directory.
pub fn run_editor(project_root: PathBuf) -> Result<(), String> {
    let scene_path = project_root.join(DEFAULT_SCENE_FILE);
    let config_path = project_root.join(CONFIG_FILE);

    let scene = load_or_default(&scene_path);
    let config = app::load_config(&config_path);
    // Write the config on first launch so users have a file to customize.
    if !config_path.exists() {
        if let Err(error) = app::save_config(&config_path, &config) {
            eprintln!("warning: failed to write {}: {error}", config_path.display());
        }
    }

    let mut editor = EditorApp::new(project_root, scene_path, scene, config);
    let fonts = ui::load_fonts()?;

    // When set, render a single frame and exit. Used for headless smoke testing.
    // If the value names a `.png` path, the frame is also written there.
    let smoke_var = std::env::var_os("NEOLOVE_EDITOR_SMOKE");
    let smoke_test = smoke_var.is_some();
    let smoke_png: Option<PathBuf> = smoke_var
        .map(PathBuf::from)
        .filter(|p| p.extension().is_some_and(|e| e == "png"));

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title(editor.title())
        .with_inner_size(LogicalSize::new(WINDOW_W, WINDOW_H))
        .with_min_inner_size(LogicalSize::new(640.0, 420.0))
        .build(&event_loop)
        .map_err(|e| format!("failed to create editor window: {e}"))?;

    let context = unsafe { softbuffer::Context::new(&window) }
        .map_err(|e| format!("failed to create editor surface context: {e}"))?;
    let mut surface = unsafe { softbuffer::Surface::new(&context, &window) }
        .map_err(|e| format!("failed to create editor surface: {e}"))?;

    let mut input = PendingInput::default();
    let mut last_title = editor.title();
    window.request_redraw();

    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    if editor.request_close() {
                        editor.flush_config();
                        *control_flow = ControlFlow::Exit;
                    } else {
                        // A save-confirmation dialog was opened; stay running.
                        window.request_redraw();
                    }
                }
                WindowEvent::ModifiersChanged(state) => {
                    input.ctrl = state.ctrl() || state.logo();
                }
                WindowEvent::CursorMoved { position, .. } => {
                    let (nx, ny) = (position.x as f32, position.y as f32);
                    input.delta_x += nx - input.mouse_x;
                    input.delta_y += ny - input.mouse_y;
                    input.mouse_x = nx;
                    input.mouse_y = ny;
                    if input.mouse_down || input.middle_down {
                        window.request_redraw();
                    }
                }
                WindowEvent::MouseInput { state, button, .. } => {
                    let pressed = state == ElementState::Pressed;
                    match button {
                        MouseButton::Left => {
                            input.mouse_down = pressed;
                            if pressed {
                                input.mouse_pressed = true;
                                // Double-click detection.
                                let now = Instant::now();
                                if now.duration_since(input.last_click) < Duration::from_millis(400)
                                    && (input.mouse_x - input.last_click_x).abs() < 4.0
                                    && (input.mouse_y - input.last_click_y).abs() < 4.0
                                {
                                    input.double_click = true;
                                }
                                input.last_click = now;
                                input.last_click_x = input.mouse_x;
                                input.last_click_y = input.mouse_y;
                            }
                        }
                        MouseButton::Right => {
                            if pressed {
                                input.right_pressed = true;
                            }
                        }
                        MouseButton::Middle => input.middle_down = pressed,
                        MouseButton::Other(1) => {
                            if pressed {
                                input.back_pressed = true;
                            }
                        }
                        MouseButton::Other(2) => {
                            if pressed {
                                input.forward_pressed = true;
                            }
                        }
                        _ => {}
                    }
                    window.request_redraw();
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    input.scroll += match delta {
                        MouseScrollDelta::LineDelta(_, y) => y,
                        MouseScrollDelta::PixelDelta(pos) => (pos.y as f32) / 32.0,
                    };
                    window.request_redraw();
                }
                WindowEvent::ReceivedCharacter(ch) => {
                    if !ch.is_control() {
                        input.typed.push(ch);
                        window.request_redraw();
                    }
                }
                WindowEvent::KeyboardInput {
                    input:
                        KeyboardInput {
                            virtual_keycode: Some(key),
                            state: ElementState::Pressed,
                            ..
                        },
                    ..
                } => {
                    match key {
                        VirtualKeyCode::Back => input.backspace = true,
                        VirtualKeyCode::Delete => input.delete = true,
                        VirtualKeyCode::Return | VirtualKeyCode::NumpadEnter => input.enter = true,
                        VirtualKeyCode::Escape => input.escape = true,
                        VirtualKeyCode::C if input.ctrl => input.copy = true,
                        VirtualKeyCode::V if input.ctrl => input.paste = true,
                        VirtualKeyCode::S if input.ctrl => input.save = true,
                        _ => {}
                    }
                    window.request_redraw();
                }
                WindowEvent::Resized(_) => window.request_redraw(),
                _ => {}
            },
            Event::RedrawRequested(_) => {
                let shot = if smoke_test { smoke_png.as_deref() } else { None };
                if let Err(error) =
                    redraw(&window, &mut surface, &mut editor, &mut input, &fonts, shot)
                {
                    eprintln!("editor render error: {error}");
                    *control_flow = ControlFlow::Exit;
                    return;
                }
                editor.flush_config();
                if editor.should_quit() {
                    editor.flush_config();
                    *control_flow = ControlFlow::Exit;
                    return;
                }
                let title = editor.title();
                if title != last_title {
                    window.set_title(&title);
                    last_title = title;
                }
                if smoke_test {
                    *control_flow = ControlFlow::Exit;
                }
            }
            _ => {}
        }
    });
}

/// Retained input that survives between redraws, drained into a [`FrameInput`].
struct PendingInput {
    mouse_x: f32,
    mouse_y: f32,
    mouse_down: bool,
    mouse_pressed: bool,
    middle_down: bool,
    right_pressed: bool,
    back_pressed: bool,
    forward_pressed: bool,
    double_click: bool,
    delta_x: f32,
    delta_y: f32,
    scroll: f32,
    typed: String,
    backspace: bool,
    enter: bool,
    escape: bool,
    delete: bool,
    ctrl: bool,
    copy: bool,
    paste: bool,
    save: bool,
    last_click: Instant,
    last_click_x: f32,
    last_click_y: f32,
}

impl Default for PendingInput {
    fn default() -> Self {
        Self {
            mouse_x: 0.0,
            mouse_y: 0.0,
            mouse_down: false,
            mouse_pressed: false,
            middle_down: false,
            right_pressed: false,
            back_pressed: false,
            forward_pressed: false,
            double_click: false,
            delta_x: 0.0,
            delta_y: 0.0,
            scroll: 0.0,
            typed: String::new(),
            backspace: false,
            enter: false,
            escape: false,
            delete: false,
            ctrl: false,
            copy: false,
            paste: false,
            save: false,
            last_click: Instant::now() - Duration::from_secs(10),
            last_click_x: 0.0,
            last_click_y: 0.0,
        }
    }
}

impl PendingInput {
    /// Build the frame's input snapshot and clear the one-shot edge events.
    fn take_frame(&mut self) -> FrameInput {
        let frame = FrameInput {
            mouse_x: self.mouse_x,
            mouse_y: self.mouse_y,
            mouse_pressed: self.mouse_pressed,
            mouse_down: self.mouse_down,
            double_click: self.double_click,
            right_pressed: self.right_pressed,
            middle_down: self.middle_down,
            back_pressed: self.back_pressed,
            forward_pressed: self.forward_pressed,
            delta_x: self.delta_x,
            delta_y: self.delta_y,
            scroll: self.scroll,
            typed: std::mem::take(&mut self.typed),
            backspace: self.backspace,
            enter: self.enter,
            escape: self.escape,
            delete: self.delete,
            copy: self.copy,
            paste: self.paste,
            save: self.save,
        };
        self.mouse_pressed = false;
        self.right_pressed = false;
        self.back_pressed = false;
        self.forward_pressed = false;
        self.double_click = false;
        self.delta_x = 0.0;
        self.delta_y = 0.0;
        self.scroll = 0.0;
        self.backspace = false;
        self.enter = false;
        self.escape = false;
        self.delete = false;
        self.copy = false;
        self.paste = false;
        self.save = false;
        frame
    }
}

fn redraw(
    window: &winit::window::Window,
    surface: &mut softbuffer::Surface,
    editor: &mut EditorApp,
    input: &mut PendingInput,
    fonts: &Fonts,
    screenshot: Option<&Path>,
) -> Result<(), String> {
    let size = window.inner_size();
    let width = size.width.max(1);
    let height = size.height.max(1);

    surface
        .resize(
            NonZeroU32::new(width).expect("width clamped to >= 1"),
            NonZeroU32::new(height).expect("height clamped to >= 1"),
        )
        .map_err(|e| format!("failed to resize editor surface: {e}"))?;

    let mut buffer = surface
        .buffer_mut()
        .map_err(|e| format!("failed to acquire editor surface buffer: {e}"))?;

    let painter = Painter::new(&mut buffer, width as usize, height as usize, fonts.clone());
    let frame_input = input.take_frame();
    let theme = editor.theme();
    let mut ctx = Ui::new(
        painter,
        frame_input,
        theme,
        editor.take_focus(),
        editor.take_edit_buffer(),
    );
    editor.frame(&mut ctx);
    let wants_redraw = ctx.wants_redraw;
    let (focus, edit_buffer) = ctx.into_focus_state();
    editor.set_focus(focus, edit_buffer);

    if let Some(path) = screenshot {
        save_screenshot(&buffer, width, height, path);
    }

    buffer
        .present()
        .map_err(|e| format!("failed to present editor surface: {e}"))?;

    if wants_redraw {
        window.request_redraw();
    }
    Ok(())
}

/// Convert the presented `0x00RRGGBB` framebuffer to a PNG for smoke testing.
fn save_screenshot(buffer: &[u32], width: u32, height: u32, path: &Path) {
    let mut img = image::RgbaImage::new(width, height);
    for (i, pixel) in buffer.iter().enumerate() {
        let x = (i as u32) % width;
        let y = (i as u32) / width;
        if y >= height {
            break;
        }
        let r = ((pixel >> 16) & 0xff) as u8;
        let g = ((pixel >> 8) & 0xff) as u8;
        let b = (pixel & 0xff) as u8;
        img.put_pixel(x, y, image::Rgba([r, g, b, 255]));
    }
    if let Err(error) = img.save(path) {
        eprintln!("warning: failed to write screenshot {}: {error}", path.display());
    }
}

fn load_or_default(scene_path: &Path) -> Scene {
    if scene_path.exists() {
        match Scene::load(scene_path) {
            Ok(scene) => return scene,
            Err(error) => {
                eprintln!("warning: failed to load {}: {error}", scene_path.display());
            }
        }
    }
    Scene::default()
}
