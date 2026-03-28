use std::collections::HashSet;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub(crate) const WHITE: Self = Self::rgba(255, 255, 255, 255);

    pub(crate) const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct MouseState {
    pub x: f32,
    pub y: f32,
    pub delta_x: f32,
    pub delta_y: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct WindowState {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct InputState {
    pub keys_down: HashSet<String>,
    pub keys_pressed: HashSet<String>,
    pub keys_released: HashSet<String>,
    pub mouse_down: HashSet<String>,
    pub mouse_pressed: HashSet<String>,
    pub mouse_released: HashSet<String>,
    pub wheel_x: f32,
    pub wheel_y: f32,
    pub last_key_pressed: Option<String>,
    pub char_pressed: Option<String>,
    pub mouse_locked: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct FrameState {
    pub clear_color: Color,
}

impl Default for FrameState {
    fn default() -> Self {
        Self {
            clear_color: Color::WHITE,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct PlatformState {
    mouse: MouseState,
    window: WindowState,
    input: InputState,
    frame: FrameState,
}

impl PlatformState {
    pub(crate) fn mouse(&self) -> MouseState {
        self.mouse
    }

    pub(crate) fn set_mouse_position(&mut self, x: f32, y: f32) {
        self.mouse.delta_x = x - self.mouse.x;
        self.mouse.delta_y = y - self.mouse.y;
        self.mouse.x = x;
        self.mouse.y = y;
    }

    pub(crate) fn reset_mouse_delta(&mut self) {
        self.mouse.delta_x = 0.0;
        self.mouse.delta_y = 0.0;
    }

    pub(crate) fn window(&self) -> WindowState {
        self.window
    }

    pub(crate) fn set_window(&mut self, window: WindowState) {
        self.window = window;
    }

    pub(crate) fn input(&self) -> &InputState {
        &self.input
    }

    pub(crate) fn input_mut(&mut self) -> &mut InputState {
        &mut self.input
    }

    pub(crate) fn clear_color(&self) -> Color {
        self.frame.clear_color
    }

    pub(crate) fn set_clear_color(&mut self, color: Color) {
        self.frame.clear_color = color;
    }

    pub(crate) fn begin_frame(&mut self) {
        self.input.keys_pressed.clear();
        self.input.keys_released.clear();
        self.input.mouse_pressed.clear();
        self.input.mouse_released.clear();
        self.input.wheel_x = 0.0;
        self.input.wheel_y = 0.0;
        self.input.last_key_pressed = None;
        self.input.char_pressed = None;
        self.reset_mouse_delta();
    }
}

pub(crate) type SharedPlatformState = Arc<Mutex<PlatformState>>;

pub(crate) fn new_shared_platform_state() -> SharedPlatformState {
    Arc::new(Mutex::new(PlatformState::default()))
}
