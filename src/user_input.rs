use crate::platform::SharedPlatformState;
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
                let platform = platform
                    .lock()
                    .map_err(|_| mlua::Error::external("platform lock poisoned"))?;
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
                let platform = platform
                    .lock()
                    .map_err(|_| mlua::Error::external("platform lock poisoned"))?;
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
                let platform = platform
                    .lock()
                    .map_err(|_| mlua::Error::external("platform lock poisoned"))?;
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
                let platform = platform
                    .lock()
                    .map_err(|_| mlua::Error::external("platform lock poisoned"))?;
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
                let platform = platform
                    .lock()
                    .map_err(|_| mlua::Error::external("platform lock poisoned"))?;
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
                let platform = platform
                    .lock()
                    .map_err(|_| mlua::Error::external("platform lock poisoned"))?;
                Ok(platform.input().mouse_released.contains(&button))
            })?,
        )?;
    }

    {
        let platform = platform.clone();
        input.set(
            "getMouseWheel",
            lua.create_function(move |_lua, ()| {
                let platform = platform
                    .lock()
                    .map_err(|_| mlua::Error::external("platform lock poisoned"))?;
                Ok((platform.input().wheel_x, platform.input().wheel_y))
            })?,
        )?;
    }

    {
        let platform = platform.clone();
        input.set(
            "isScrollingIn",
            lua.create_function(move |_lua, ()| {
                let platform = platform
                    .lock()
                    .map_err(|_| mlua::Error::external("platform lock poisoned"))?;
                Ok(platform.input().wheel_y > 0.0)
            })?,
        )?;
    }

    {
        let platform = platform.clone();
        input.set(
            "isScrollingOut",
            lua.create_function(move |_lua, ()| {
                let platform = platform
                    .lock()
                    .map_err(|_| mlua::Error::external("platform lock poisoned"))?;
                Ok(platform.input().wheel_y < 0.0)
            })?,
        )?;
    }

    {
        let platform = platform.clone();
        input.set(
            "getScrollInAmount",
            lua.create_function(move |_lua, ()| {
                let platform = platform
                    .lock()
                    .map_err(|_| mlua::Error::external("platform lock poisoned"))?;
                Ok(platform.input().wheel_y)
            })?,
        )?;
    }

    {
        let platform = platform.clone();
        input.set(
            "getMouseDelta",
            lua.create_function(move |_lua, ()| {
                let platform = platform
                    .lock()
                    .map_err(|_| mlua::Error::external("platform lock poisoned"))?;
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
                let mut platform = platform
                    .lock()
                    .map_err(|_| mlua::Error::external("platform lock poisoned"))?;
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
                let platform = platform
                    .lock()
                    .map_err(|_| mlua::Error::external("platform lock poisoned"))?;
                Ok(platform.input().mouse_locked)
            })?,
        )?;
    }

    {
        let platform = platform.clone();
        input.set(
            "getLastKeyPressed",
            lua.create_function(move |_lua, ()| {
                let platform = platform
                    .lock()
                    .map_err(|_| mlua::Error::external("platform lock poisoned"))?;
                Ok(platform.input().last_key_pressed.clone())
            })?,
        )?;
    }

    {
        let platform = platform.clone();
        input.set(
            "getCharPressed",
            lua.create_function(move |_lua, ()| {
                let platform = platform
                    .lock()
                    .map_err(|_| mlua::Error::external("platform lock poisoned"))?;
                Ok(platform.input().char_pressed.clone())
            })?,
        )?;
    }

    lua.globals().set("input", input.clone())?;
    lua.globals().set("userInput", input)?;
    Ok(())
}
