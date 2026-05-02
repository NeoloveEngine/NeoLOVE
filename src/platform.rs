use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

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
    // These sets stay tiny, and an ordered tree avoids the hash-table clear/drop path
    // that has been trapping in the web build's end-of-frame input reset.
    pub keys_down: BTreeSet<String>,
    pub keys_pressed: BTreeSet<String>,
    pub keys_released: BTreeSet<String>,
    pub mouse_down: BTreeSet<String>,
    pub mouse_pressed: BTreeSet<String>,
    pub mouse_released: BTreeSet<String>,
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

fn report_poisoned_platform_lock() {
    static REPORTED: AtomicBool = AtomicBool::new(false);
    if !REPORTED.swap(true, Ordering::Relaxed) {
        eprintln!("warning: platform state mutex was poisoned; recovering and continuing");
    }
}

pub(crate) fn lock_platform_state(platform: &SharedPlatformState) -> MutexGuard<'_, PlatformState> {
    match platform.lock() {
        Ok(platform) => platform,
        Err(poisoned) => {
            report_poisoned_platform_lock();
            poisoned.into_inner()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poisoned_platform_lock_is_recovered() {
        let platform = new_shared_platform_state();
        let poisoned = platform.clone();
        let _ = std::panic::catch_unwind(move || {
            let _guard = poisoned.lock().unwrap();
            panic!("poison platform state");
        });

        let mut guard = lock_platform_state(&platform);
        guard.set_clear_color(Color::rgba(12, 34, 56, 78));

        assert_eq!(guard.clear_color(), Color::rgba(12, 34, 56, 78));
    }
}
