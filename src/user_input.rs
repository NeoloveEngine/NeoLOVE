use crate::platform::{lock_platform_state, SharedPlatformState};
use mlua::Lua;

pub(crate) fn normalize_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

pub(crate) fn add_user_input_module(lua: &Lua, platform: SharedPlatformState) -> mlua::Result<()> {
    let input = lua.create_table()?;

    {
        let platform = platform.clone();
        input.set(
            "isKeyDown",
            lua.create_function(move |_lua, key: String| {
                let key = normalize_name(&key);
                let platform = lock_platform_state(&platform);
                Ok(platform.input().keys_down.contains(&key))
            })?,
        )?;
    }

    {
        let platform = platform.clone();
        input.set(
            "isKeyPressed",
            lua.create_function(move |_lua, key: String| {
                let key = normalize_name(&key);
                let platform = lock_platform_state(&platform);
                Ok(platform.input().keys_pressed.contains(&key))
            })?,
        )?;
    }

    {
        let platform = platform.clone();
        input.set(
            "isKeyReleased",
            lua.create_function(move |_lua, key: String| {
                let key = normalize_name(&key);
                let platform = lock_platform_state(&platform);
                Ok(platform.input().keys_released.contains(&key))
            })?,
        )?;
    }

    {
        let platform = platform.clone();
        input.set(
            "isMouseDown",
            lua.create_function(move |_lua, button: Option<String>| {
                let button = normalize_name(button.as_deref().unwrap_or("left"));
                let platform = lock_platform_state(&platform);
                Ok(platform.input().mouse_down.contains(&button))
            })?,
        )?;
    }

    {
        let platform = platform.clone();
        input.set(
            "isMousePressed",
            lua.create_function(move |_lua, button: Option<String>| {
                let button = normalize_name(button.as_deref().unwrap_or("left"));
                let platform = lock_platform_state(&platform);
                Ok(platform.input().mouse_pressed.contains(&button))
            })?,
        )?;
    }

    {
        let platform = platform.clone();
        input.set(
            "isMouseReleased",
            lua.create_function(move |_lua, button: Option<String>| {
                let button = normalize_name(button.as_deref().unwrap_or("left"));
                let platform = lock_platform_state(&platform);
                Ok(platform.input().mouse_released.contains(&button))
            })?,
        )?;
    }

    {
        let platform = platform.clone();
        input.set(
            "getMouseWheel",
            lua.create_function(move |_lua, ()| {
                let platform = lock_platform_state(&platform);
                Ok((platform.input().wheel_x, platform.input().wheel_y))
            })?,
        )?;
    }

    {
        let platform = platform.clone();
        input.set(
            "isScrollingIn",
            lua.create_function(move |_lua, ()| {
                let platform = lock_platform_state(&platform);
                Ok(platform.input().wheel_y > 0.0)
            })?,
        )?;
    }

    {
        let platform = platform.clone();
        input.set(
            "isScrollingOut",
            lua.create_function(move |_lua, ()| {
                let platform = lock_platform_state(&platform);
                Ok(platform.input().wheel_y < 0.0)
            })?,
        )?;
    }

    {
        let platform = platform.clone();
        input.set(
            "getScrollInAmount",
            lua.create_function(move |_lua, ()| {
                let platform = lock_platform_state(&platform);
                Ok(platform.input().wheel_y)
            })?,
        )?;
    }

    {
        let platform = platform.clone();
        input.set(
            "getMouseDelta",
            lua.create_function(move |_lua, ()| {
                let platform = lock_platform_state(&platform);
                let mouse = platform.mouse();
                Ok((mouse.delta_x, mouse.delta_y))
            })?,
        )?;
    }

    {
        let platform = platform.clone();
        input.set(
            "setMouseLocked",
            lua.create_function(move |_lua, locked: bool| {
                let mut platform = lock_platform_state(&platform);
                platform.input_mut().mouse_locked = locked;
                Ok(())
            })?,
        )?;
    }

    {
        let platform = platform.clone();
        input.set(
            "isMouseLocked",
            lua.create_function(move |_lua, ()| {
                let platform = lock_platform_state(&platform);
                Ok(platform.input().mouse_locked)
            })?,
        )?;
    }

    {
        let platform = platform.clone();
        input.set(
            "getLastKeyPressed",
            lua.create_function(move |_lua, ()| {
                let platform = lock_platform_state(&platform);
                Ok(platform.input().last_key_pressed.clone())
            })?,
        )?;
    }

    {
        let platform = platform.clone();
        input.set(
            "getCharPressed",
            lua.create_function(move |_lua, ()| {
                let platform = lock_platform_state(&platform);
                Ok(platform.input().char_pressed.clone())
            })?,
        )?;
    }

    input.set(
        "showKeyboard",
        lua.create_function(|_lua, implicit: Option<bool>| {
            Ok(crate::android_module::show_keyboard(
                implicit.unwrap_or(true),
            ))
        })?,
    )?;
    input.set("openKeyboard", input.get::<mlua::Function>("showKeyboard")?)?;
    input.set(
        "hideKeyboard",
        lua.create_function(|_lua, implicit_only: Option<bool>| {
            Ok(crate::android_module::hide_keyboard(
                implicit_only.unwrap_or(false),
            ))
        })?,
    )?;
    input.set(
        "closeKeyboard",
        input.get::<mlua::Function>("hideKeyboard")?,
    )?;

    lua.globals().set("input", input.clone())?;
    lua.globals().set("userInput", input)?;
    Ok(())
}
