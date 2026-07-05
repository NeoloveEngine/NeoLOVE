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
mod inspector;
mod logger;
pub(crate) mod scene;
mod ui;

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use winit::dpi::LogicalSize;
use winit::event::{
    ElementState, Event, KeyboardInput, MouseButton, MouseScrollDelta, VirtualKeyCode, WindowEvent,
};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::window::{Window, WindowBuilder};

use app::EditorApp;
use logger::LoggerWindow;
use scene::Scene;
use ui::{FrameInput, Fonts, Painter, Theme, Ui};

const DEFAULT_SCENE_FILE: &str = "scene.neoscene";
const CONFIG_FILE: &str = "editor.json";
const WINDOW_W: f64 = 1280.0;
const WINDOW_H: f64 = 760.0;
/// Baseline editor refresh used to recover from code paths that mutate visible
/// state without explicitly requesting a redraw. Input and animations may
/// still request frames immediately between these ticks.
const IDLE_FRAME_INTERVAL: Duration = Duration::from_millis(50);

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

    // A second, initially hidden window that becomes the live logger while a
    // scene runs. Created up front so its softbuffer surface mirrors the main
    // window's straightforward (non-self-referential) lifetimes.
    let logger_window = WindowBuilder::new()
        .with_title("NeoLOVE — Logger")
        .with_inner_size(LogicalSize::new(940.0, 620.0))
        .with_visible(false)
        .build(&event_loop)
        .map_err(|e| format!("failed to create logger window: {e}"))?;
    let logger_id = logger_window.id();
    let logger_context = unsafe { softbuffer::Context::new(&logger_window) }
        .map_err(|e| format!("failed to create logger surface context: {e}"))?;
    let mut logger_surface = unsafe { softbuffer::Surface::new(&logger_context, &logger_window) }
        .map_err(|e| format!("failed to create logger surface: {e}"))?;
    let mut logger_ui: Option<LoggerWindow> = None;
    let mut logger_input = PendingInput::default();
    let mut logger_visible = false;

    let mut input = PendingInput::default();
    let mut last_title = editor.title();
    let mut next_idle_redraw = Instant::now() + IDLE_FRAME_INTERVAL;
    window.request_redraw();

    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::WaitUntil(next_idle_redraw);

        // Maintain a 20 FPS baseline even when no code path explicitly asks
        // for a frame. This also polls preview state often enough for startup
        // errors to surface promptly.
        if let Event::NewEvents(_) = event {
            // A run just started: open/refresh the live logger window.
            if let Some(session) = editor.take_logger_session() {
                logger_ui = Some(LoggerWindow::new(session.state));
                logger_visible = true;
                logger_window.set_visible(true);
                logger_window.focus_window();
                logger_window.request_redraw();
            }
            let now = Instant::now();
            if now >= next_idle_redraw {
                window.request_redraw();
                if logger_visible {
                    logger_window.request_redraw();
                }
                next_idle_redraw = now + IDLE_FRAME_INTERVAL;
            }
            if editor.poll_run() {
                window.request_redraw();
            }
        }

        match event {
            // Logger window: a small, self-contained input + close handler.
            Event::WindowEvent { window_id, event } if window_id == logger_id => {
                handle_logger_event(event, &logger_window, &mut logger_input, &mut logger_visible);
            }
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
                    input.shift = state.shift();
                }
                WindowEvent::CursorMoved { position, .. } => {
                    input.mouse_x = position.x as f32;
                    input.mouse_y = position.y as f32;
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
                        // Side buttons report different codes across mice and
                        // platforms; accept the common back/forward values.
                        MouseButton::Other(1 | 8) => {
                            if pressed {
                                input.back_pressed = true;
                            }
                        }
                        MouseButton::Other(2 | 9) => {
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
                        VirtualKeyCode::D if input.ctrl => input.duplicate = true,
                        VirtualKeyCode::Y if input.ctrl => input.redo = true,
                        VirtualKeyCode::Z if input.ctrl && input.shift => input.redo = true,
                        VirtualKeyCode::Z if input.ctrl => input.undo = true,
                        VirtualKeyCode::A if input.ctrl && input.shift => input.invert_selection = true,
                        VirtualKeyCode::A if input.ctrl => input.select_all = true,
                        VirtualKeyCode::G if input.ctrl && input.shift => input.unparent_selection = true,
                        VirtualKeyCode::G if input.ctrl => input.group_selection = true,
                        VirtualKeyCode::H if input.shift => input.show_all = true,
                        VirtualKeyCode::H => input.hide_selection = true,
                        VirtualKeyCode::L if input.shift => input.unlock_all = true,
                        VirtualKeyCode::L => input.lock_selection = true,
                        VirtualKeyCode::Home => input.frame_all = true,
                        VirtualKeyCode::Space if input.shift => input.maximize_view = true,
                        VirtualKeyCode::G => input.toggle_grid = true,
                        VirtualKeyCode::S if input.shift => input.toggle_snap = true,
                        VirtualKeyCode::F2 => input.rename = true,
                        VirtualKeyCode::F => input.focus_selection = true,
                        VirtualKeyCode::Key0 | VirtualKeyCode::Numpad0 => input.reset_view = true,
                        VirtualKeyCode::Left => input.nudge_x = -1.0,
                        VirtualKeyCode::Right => input.nudge_x = 1.0,
                        VirtualKeyCode::Up => input.nudge_y = -1.0,
                        VirtualKeyCode::Down => input.nudge_y = 1.0,
                        _ => {}
                    }
                    window.request_redraw();
                }
                WindowEvent::Resized(_) => window.request_redraw(),
                _ => {}
            },
            Event::RedrawRequested(id) if id == logger_id => {
                if logger_visible {
                    if let Some(logger) = logger_ui.as_mut() {
                        if let Err(error) = redraw_logger(
                            &logger_window,
                            &mut logger_surface,
                            logger,
                            &mut logger_input,
                            &fonts,
                            editor.theme(),
                        ) {
                            eprintln!("logger render error: {error}");
                        }
                    }
                }
            }
            Event::RedrawRequested(_) => {
                let shot = if smoke_test { smoke_png.as_deref() } else { None };
                if let Err(error) =
                    redraw(&window, &mut surface, &mut editor, &mut input, &fonts, shot)
                {
                    eprintln!("editor render error: {error}");
                    *control_flow = ControlFlow::Exit;
                    return;
                }
                next_idle_redraw = Instant::now() + IDLE_FRAME_INTERVAL;
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

        if *control_flow != ControlFlow::Exit {
            let mut deadline = next_idle_redraw;
            // Preserve the explicit preview polling bound if the baseline FPS
            // is changed to a slower cadence in the future.
            if editor.run_pending() {
                deadline = deadline.min(Instant::now() + Duration::from_millis(250));
            }
            *control_flow = ControlFlow::WaitUntil(deadline);
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
    scroll: f32,
    typed: String,
    backspace: bool,
    enter: bool,
    escape: bool,
    delete: bool,
    ctrl: bool,
    shift: bool,
    copy: bool,
    paste: bool,
    save: bool,
    duplicate: bool,
    undo: bool,
    redo: bool,
    select_all: bool,
    invert_selection: bool,
    group_selection: bool,
    unparent_selection: bool,
    hide_selection: bool,
    show_all: bool,
    lock_selection: bool,
    unlock_all: bool,
    frame_all: bool,
    maximize_view: bool,
    toggle_grid: bool,
    toggle_snap: bool,
    focus_selection: bool,
    rename: bool,
    reset_view: bool,
    nudge_x: f32,
    nudge_y: f32,
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
            scroll: 0.0,
            typed: String::new(),
            backspace: false,
            enter: false,
            escape: false,
            delete: false,
            ctrl: false,
            shift: false,
            copy: false,
            paste: false,
            save: false,
            duplicate: false,
            undo: false,
            redo: false,
            select_all: false,
            invert_selection: false,
            group_selection: false,
            unparent_selection: false,
            hide_selection: false,
            show_all: false,
            lock_selection: false,
            unlock_all: false,
            frame_all: false,
            maximize_view: false,
            toggle_grid: false,
            toggle_snap: false,
            focus_selection: false,
            rename: false,
            reset_view: false,
            nudge_x: 0.0,
            nudge_y: 0.0,
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
            scroll: self.scroll,
            typed: std::mem::take(&mut self.typed),
            backspace: self.backspace,
            enter: self.enter,
            escape: self.escape,
            delete: self.delete,
            copy: self.copy,
            paste: self.paste,
            save: self.save,
            duplicate: self.duplicate,
            undo: self.undo,
            redo: self.redo,
            select_all: self.select_all,
            invert_selection: self.invert_selection,
            group_selection: self.group_selection,
            unparent_selection: self.unparent_selection,
            hide_selection: self.hide_selection,
            show_all: self.show_all,
            lock_selection: self.lock_selection,
            unlock_all: self.unlock_all,
            frame_all: self.frame_all,
            maximize_view: self.maximize_view,
            toggle_grid: self.toggle_grid,
            toggle_snap: self.toggle_snap,
            ctrl: self.ctrl,
            shift: self.shift,
            focus_selection: self.focus_selection,
            rename: self.rename,
            reset_view: self.reset_view,
            nudge_x: self.nudge_x,
            nudge_y: self.nudge_y,
            nudge_big: self.shift,
        };
        self.mouse_pressed = false;
        self.right_pressed = false;
        self.back_pressed = false;
        self.forward_pressed = false;
        self.double_click = false;
        self.scroll = 0.0;
        self.backspace = false;
        self.enter = false;
        self.escape = false;
        self.delete = false;
        self.copy = false;
        self.paste = false;
        self.save = false;
        self.duplicate = false;
        self.undo = false;
        self.redo = false;
        self.select_all = false;
        self.invert_selection = false;
        self.group_selection = false;
        self.unparent_selection = false;
        self.hide_selection = false;
        self.show_all = false;
        self.lock_selection = false;
        self.unlock_all = false;
        self.frame_all = false;
        self.maximize_view = false;
        self.toggle_grid = false;
        self.toggle_snap = false;
        self.focus_selection = false;
        self.rename = false;
        self.reset_view = false;
        self.nudge_x = 0.0;
        self.nudge_y = 0.0;
        frame
    }
}

/// Feed input to the logger window. It needs only pointer interaction plus
/// close (which hides rather than exits, so the next run can reopen it).
fn handle_logger_event(
    event: WindowEvent,
    window: &Window,
    input: &mut PendingInput,
    visible: &mut bool,
) {
    match event {
        WindowEvent::CloseRequested => {
            *visible = false;
            window.set_visible(false);
        }
        WindowEvent::CursorMoved { position, .. } => {
            input.mouse_x = position.x as f32;
            input.mouse_y = position.y as f32;
            window.request_redraw();
        }
        WindowEvent::MouseInput {
            state,
            button: MouseButton::Left,
            ..
        } => {
            let pressed = state == ElementState::Pressed;
            input.mouse_down = pressed;
            if pressed {
                input.mouse_pressed = true;
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
        WindowEvent::Resized(_) => window.request_redraw(),
        _ => {}
    }
}

/// Render one logger-window frame from the shared live state.
fn redraw_logger(
    window: &Window,
    surface: &mut softbuffer::Surface,
    logger: &mut LoggerWindow,
    input: &mut PendingInput,
    fonts: &Fonts,
    theme: Theme,
) -> Result<(), String> {
    let size = window.inner_size();
    let width = size.width.max(1);
    let height = size.height.max(1);

    surface
        .resize(
            NonZeroU32::new(width).expect("width clamped to >= 1"),
            NonZeroU32::new(height).expect("height clamped to >= 1"),
        )
        .map_err(|e| format!("failed to resize logger surface: {e}"))?;

    let mut buffer = surface
        .buffer_mut()
        .map_err(|e| format!("failed to acquire logger surface buffer: {e}"))?;

    let painter = Painter::new(&mut buffer, width as usize, height as usize, fonts.clone());
    let frame_input = input.take_frame();
    let mut ctx = Ui::new(painter, frame_input, theme, None, String::new());
    logger.frame(&mut ctx);

    buffer
        .present()
        .map_err(|e| format!("failed to present logger surface: {e}"))?;
    Ok(())
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
