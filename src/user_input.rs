use macroquad::input::{
    KeyCode, MouseButton, get_char_pressed, get_last_key_pressed, is_key_down, is_key_pressed,
    is_key_released, is_mouse_button_down, is_mouse_button_pressed, is_mouse_button_released,
    mouse_wheel,
};
use mlua::Lua;

fn normalize_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn parse_key_code(key: &str) -> Option<KeyCode> {
    match normalize_name(key).as_str() {
        "a" => Some(KeyCode::A),
        "b" => Some(KeyCode::B),
        "c" => Some(KeyCode::C),
        "d" => Some(KeyCode::D),
        "e" => Some(KeyCode::E),
        "f" => Some(KeyCode::F),
        "g" => Some(KeyCode::G),
        "h" => Some(KeyCode::H),
        "i" => Some(KeyCode::I),
        "j" => Some(KeyCode::J),
        "k" => Some(KeyCode::K),
        "l" => Some(KeyCode::L),
        "m" => Some(KeyCode::M),
        "n" => Some(KeyCode::N),
        "o" => Some(KeyCode::O),
        "p" => Some(KeyCode::P),
        "q" => Some(KeyCode::Q),
        "r" => Some(KeyCode::R),
        "s" => Some(KeyCode::S),
        "t" => Some(KeyCode::T),
        "u" => Some(KeyCode::U),
        "v" => Some(KeyCode::V),
        "w" => Some(KeyCode::W),
        "x" => Some(KeyCode::X),
        "y" => Some(KeyCode::Y),
        "z" => Some(KeyCode::Z),
        "0" | "key0" => Some(KeyCode::Key0),
        "1" | "key1" => Some(KeyCode::Key1),
        "2" | "key2" => Some(KeyCode::Key2),
        "3" | "key3" => Some(KeyCode::Key3),
        "4" | "key4" => Some(KeyCode::Key4),
        "5" | "key5" => Some(KeyCode::Key5),
        "6" | "key6" => Some(KeyCode::Key6),
        "7" | "key7" => Some(KeyCode::Key7),
        "8" | "key8" => Some(KeyCode::Key8),
        "9" | "key9" => Some(KeyCode::Key9),
        "space" => Some(KeyCode::Space),
        "apostrophe" | "quote" | "singlequote" => Some(KeyCode::Apostrophe),
        "comma" => Some(KeyCode::Comma),
        "minus" | "dash" => Some(KeyCode::Minus),
        "period" | "dot" => Some(KeyCode::Period),
        "slash" | "forwardslash" => Some(KeyCode::Slash),
        "semicolon" => Some(KeyCode::Semicolon),
        "equal" | "equals" => Some(KeyCode::Equal),
        "leftbracket" | "lbracket" => Some(KeyCode::LeftBracket),
        "backslash" => Some(KeyCode::Backslash),
        "rightbracket" | "rbracket" => Some(KeyCode::RightBracket),
        "graveaccent" | "grave" | "backtick" => Some(KeyCode::GraveAccent),
        "world1" => Some(KeyCode::World1),
        "world2" => Some(KeyCode::World2),
        "escape" | "esc" => Some(KeyCode::Escape),
        "enter" | "return" => Some(KeyCode::Enter),
        "tab" => Some(KeyCode::Tab),
        "backspace" => Some(KeyCode::Backspace),
        "insert" => Some(KeyCode::Insert),
        "delete" | "del" => Some(KeyCode::Delete),
        "right" | "arrowright" | "rightarrow" => Some(KeyCode::Right),
        "left" | "arrowleft" | "leftarrow" => Some(KeyCode::Left),
        "down" | "arrowdown" | "downarrow" => Some(KeyCode::Down),
        "up" | "arrowup" | "uparrow" => Some(KeyCode::Up),
        "pageup" => Some(KeyCode::PageUp),
        "pagedown" => Some(KeyCode::PageDown),
        "home" => Some(KeyCode::Home),
        "end" => Some(KeyCode::End),
        "capslock" => Some(KeyCode::CapsLock),
        "scrolllock" => Some(KeyCode::ScrollLock),
        "numlock" => Some(KeyCode::NumLock),
        "printscreen" => Some(KeyCode::PrintScreen),
        "pause" => Some(KeyCode::Pause),
        "f1" => Some(KeyCode::F1),
        "f2" => Some(KeyCode::F2),
        "f3" => Some(KeyCode::F3),
        "f4" => Some(KeyCode::F4),
        "f5" => Some(KeyCode::F5),
        "f6" => Some(KeyCode::F6),
        "f7" => Some(KeyCode::F7),
        "f8" => Some(KeyCode::F8),
        "f9" => Some(KeyCode::F9),
        "f10" => Some(KeyCode::F10),
        "f11" => Some(KeyCode::F11),
        "f12" => Some(KeyCode::F12),
        "f13" => Some(KeyCode::F13),
        "f14" => Some(KeyCode::F14),
        "f15" => Some(KeyCode::F15),
        "f16" => Some(KeyCode::F16),
        "f17" => Some(KeyCode::F17),
        "f18" => Some(KeyCode::F18),
        "f19" => Some(KeyCode::F19),
        "f20" => Some(KeyCode::F20),
        "f21" => Some(KeyCode::F21),
        "f22" => Some(KeyCode::F22),
        "f23" => Some(KeyCode::F23),
        "f24" => Some(KeyCode::F24),
        "f25" => Some(KeyCode::F25),
        "kp0" | "numpad0" => Some(KeyCode::Kp0),
        "kp1" | "numpad1" => Some(KeyCode::Kp1),
        "kp2" | "numpad2" => Some(KeyCode::Kp2),
        "kp3" | "numpad3" => Some(KeyCode::Kp3),
        "kp4" | "numpad4" => Some(KeyCode::Kp4),
        "kp5" | "numpad5" => Some(KeyCode::Kp5),
        "kp6" | "numpad6" => Some(KeyCode::Kp6),
        "kp7" | "numpad7" => Some(KeyCode::Kp7),
        "kp8" | "numpad8" => Some(KeyCode::Kp8),
        "kp9" | "numpad9" => Some(KeyCode::Kp9),
        "kpdecimal" | "numpaddecimal" => Some(KeyCode::KpDecimal),
        "kpdivide" | "numpaddivide" => Some(KeyCode::KpDivide),
        "kpmultiply" | "numpadmultiply" => Some(KeyCode::KpMultiply),
        "kpsubtract" | "numpadsubtract" => Some(KeyCode::KpSubtract),
        "kpadd" | "numpadadd" => Some(KeyCode::KpAdd),
        "kpenter" | "numpadenter" => Some(KeyCode::KpEnter),
        "kpequal" | "numpadequal" => Some(KeyCode::KpEqual),
        "leftshift" | "lshift" | "shift" => Some(KeyCode::LeftShift),
        "leftcontrol" | "leftctrl" | "lcontrol" | "lctrl" | "control" | "ctrl" => {
            Some(KeyCode::LeftControl)
        }
        "leftalt" | "lalt" | "alt" | "option" => Some(KeyCode::LeftAlt),
        "leftsuper" | "lsuper" | "super" | "meta" | "cmd" | "command" | "win" | "windows" => {
            Some(KeyCode::LeftSuper)
        }
        "rightshift" | "rshift" => Some(KeyCode::RightShift),
        "rightcontrol" | "rightctrl" | "rcontrol" | "rctrl" => Some(KeyCode::RightControl),
        "rightalt" | "ralt" => Some(KeyCode::RightAlt),
        "rightsuper" | "rsuper" => Some(KeyCode::RightSuper),
        "menu" => Some(KeyCode::Menu),
        "back" => Some(KeyCode::Back),
        _ => None,
    }
}

fn parse_mouse_button(button: &str) -> Option<MouseButton> {
    match normalize_name(button).as_str() {
        "left" | "lmb" | "mouseleft" => Some(MouseButton::Left),
        "right" | "rmb" | "mouseright" => Some(MouseButton::Right),
        "middle" | "mmb" | "mousemiddle" | "wheel" => Some(MouseButton::Middle),
        _ => None,
    }
}

fn key_code_name(key_code: KeyCode) -> &'static str {
    match key_code {
        KeyCode::Space => "Space",
        KeyCode::Apostrophe => "Apostrophe",
        KeyCode::Comma => "Comma",
        KeyCode::Minus => "Minus",
        KeyCode::Period => "Period",
        KeyCode::Slash => "Slash",
        KeyCode::Key0 => "0",
        KeyCode::Key1 => "1",
        KeyCode::Key2 => "2",
        KeyCode::Key3 => "3",
        KeyCode::Key4 => "4",
        KeyCode::Key5 => "5",
        KeyCode::Key6 => "6",
        KeyCode::Key7 => "7",
        KeyCode::Key8 => "8",
        KeyCode::Key9 => "9",
        KeyCode::Semicolon => "Semicolon",
        KeyCode::Equal => "Equal",
        KeyCode::A => "A",
        KeyCode::B => "B",
        KeyCode::C => "C",
        KeyCode::D => "D",
        KeyCode::E => "E",
        KeyCode::F => "F",
        KeyCode::G => "G",
        KeyCode::H => "H",
        KeyCode::I => "I",
        KeyCode::J => "J",
        KeyCode::K => "K",
        KeyCode::L => "L",
        KeyCode::M => "M",
        KeyCode::N => "N",
        KeyCode::O => "O",
        KeyCode::P => "P",
        KeyCode::Q => "Q",
        KeyCode::R => "R",
        KeyCode::S => "S",
        KeyCode::T => "T",
        KeyCode::U => "U",
        KeyCode::V => "V",
        KeyCode::W => "W",
        KeyCode::X => "X",
        KeyCode::Y => "Y",
        KeyCode::Z => "Z",
        KeyCode::LeftBracket => "LeftBracket",
        KeyCode::Backslash => "Backslash",
        KeyCode::RightBracket => "RightBracket",
        KeyCode::GraveAccent => "GraveAccent",
        KeyCode::World1 => "World1",
        KeyCode::World2 => "World2",
        KeyCode::Escape => "Escape",
        KeyCode::Enter => "Enter",
        KeyCode::Tab => "Tab",
        KeyCode::Backspace => "Backspace",
        KeyCode::Insert => "Insert",
        KeyCode::Delete => "Delete",
        KeyCode::Right => "Right",
        KeyCode::Left => "Left",
        KeyCode::Down => "Down",
        KeyCode::Up => "Up",
        KeyCode::PageUp => "PageUp",
        KeyCode::PageDown => "PageDown",
        KeyCode::Home => "Home",
        KeyCode::End => "End",
        KeyCode::CapsLock => "CapsLock",
        KeyCode::ScrollLock => "ScrollLock",
        KeyCode::NumLock => "NumLock",
        KeyCode::PrintScreen => "PrintScreen",
        KeyCode::Pause => "Pause",
        KeyCode::F1 => "F1",
        KeyCode::F2 => "F2",
        KeyCode::F3 => "F3",
        KeyCode::F4 => "F4",
        KeyCode::F5 => "F5",
        KeyCode::F6 => "F6",
        KeyCode::F7 => "F7",
        KeyCode::F8 => "F8",
        KeyCode::F9 => "F9",
        KeyCode::F10 => "F10",
        KeyCode::F11 => "F11",
        KeyCode::F12 => "F12",
        KeyCode::F13 => "F13",
        KeyCode::F14 => "F14",
        KeyCode::F15 => "F15",
        KeyCode::F16 => "F16",
        KeyCode::F17 => "F17",
        KeyCode::F18 => "F18",
        KeyCode::F19 => "F19",
        KeyCode::F20 => "F20",
        KeyCode::F21 => "F21",
        KeyCode::F22 => "F22",
        KeyCode::F23 => "F23",
        KeyCode::F24 => "F24",
        KeyCode::F25 => "F25",
        KeyCode::Kp0 => "Kp0",
        KeyCode::Kp1 => "Kp1",
        KeyCode::Kp2 => "Kp2",
        KeyCode::Kp3 => "Kp3",
        KeyCode::Kp4 => "Kp4",
        KeyCode::Kp5 => "Kp5",
        KeyCode::Kp6 => "Kp6",
        KeyCode::Kp7 => "Kp7",
        KeyCode::Kp8 => "Kp8",
        KeyCode::Kp9 => "Kp9",
        KeyCode::KpDecimal => "KpDecimal",
        KeyCode::KpDivide => "KpDivide",
        KeyCode::KpMultiply => "KpMultiply",
        KeyCode::KpSubtract => "KpSubtract",
        KeyCode::KpAdd => "KpAdd",
        KeyCode::KpEnter => "KpEnter",
        KeyCode::KpEqual => "KpEqual",
        KeyCode::LeftShift => "LeftShift",
        KeyCode::LeftControl => "LeftControl",
        KeyCode::LeftAlt => "LeftAlt",
        KeyCode::LeftSuper => "LeftSuper",
        KeyCode::RightShift => "RightShift",
        KeyCode::RightControl => "RightControl",
        KeyCode::RightAlt => "RightAlt",
        KeyCode::RightSuper => "RightSuper",
        KeyCode::Menu => "Menu",
        KeyCode::Back => "Back",
        KeyCode::Unknown => "Unknown",
    }
}

pub(crate) fn add_user_input_module(lua: &Lua) -> mlua::Result<()> {
    let input = lua.create_table()?;

    input.set(
        "isKeyDown",
        lua.create_function(move |_lua, key: String| {
            let key_code = parse_key_code(&key)
                .ok_or_else(|| mlua::Error::external(format!("unknown key code: {key}")))?;
            Ok(is_key_down(key_code))
        })?,
    )?;

    input.set(
        "isKeyPressed",
        lua.create_function(move |_lua, key: String| {
            let key_code = parse_key_code(&key)
                .ok_or_else(|| mlua::Error::external(format!("unknown key code: {key}")))?;
            Ok(is_key_pressed(key_code))
        })?,
    )?;

    input.set(
        "isKeyReleased",
        lua.create_function(move |_lua, key: String| {
            let key_code = parse_key_code(&key)
                .ok_or_else(|| mlua::Error::external(format!("unknown key code: {key}")))?;
            Ok(is_key_released(key_code))
        })?,
    )?;

    input.set(
        "isMouseDown",
        lua.create_function(move |_lua, button: Option<String>| {
            let button = button.unwrap_or_else(|| "left".to_string());
            let mouse_button = parse_mouse_button(&button).ok_or_else(|| {
                mlua::Error::external(format!("unknown mouse button: {button}"))
            })?;
            Ok(is_mouse_button_down(mouse_button))
        })?,
    )?;

    input.set(
        "isMousePressed",
        lua.create_function(move |_lua, button: Option<String>| {
            let button = button.unwrap_or_else(|| "left".to_string());
            let mouse_button = parse_mouse_button(&button).ok_or_else(|| {
                mlua::Error::external(format!("unknown mouse button: {button}"))
            })?;
            Ok(is_mouse_button_pressed(mouse_button))
        })?,
    )?;

    input.set(
        "isMouseReleased",
        lua.create_function(move |_lua, button: Option<String>| {
            let button = button.unwrap_or_else(|| "left".to_string());
            let mouse_button = parse_mouse_button(&button).ok_or_else(|| {
                mlua::Error::external(format!("unknown mouse button: {button}"))
            })?;
            Ok(is_mouse_button_released(mouse_button))
        })?,
    )?;

    input.set(
        "getMouseWheel",
        lua.create_function(move |_lua, ()| {
            let (x, y) = mouse_wheel();
            Ok((x, y))
        })?,
    )?;

    input.set(
        "getLastKeyPressed",
        lua.create_function(move |_lua, ()| {
            Ok(get_last_key_pressed().map(|key_code| key_code_name(key_code).to_string()))
        })?,
    )?;

    input.set(
        "getCharPressed",
        lua.create_function(move |_lua, ()| Ok(get_char_pressed().map(|c| c.to_string())))?,
    )?;

    lua.globals().set("input", input.clone())?;
    lua.globals().set("userInput", input)?;
    Ok(())
}
