use mlua::Lua;
use std::sync::OnceLock;
#[cfg(target_os = "android")]
use winit::platform::android::activity::AndroidApp;

#[derive(Clone, Debug, Default)]
pub(crate) struct AndroidInfo {
    pub device_id: Option<String>,
    pub sdk_int: Option<i64>,
    pub brand: Option<String>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub device: Option<String>,
    pub product: Option<String>,
}

static ANDROID_INFO: OnceLock<AndroidInfo> = OnceLock::new();
#[cfg(target_os = "android")]
static ANDROID_APP: OnceLock<AndroidApp> = OnceLock::new();

#[allow(dead_code)]
pub(crate) fn set_android_info(info: AndroidInfo) {
    let _ = ANDROID_INFO.set(info);
}

#[cfg(target_os = "android")]
pub(crate) fn set_android_app(app: AndroidApp) {
    let _ = ANDROID_APP.set(app);
}

fn android_info() -> AndroidInfo {
    ANDROID_INFO.get().cloned().unwrap_or_default()
}

pub(crate) fn show_keyboard(implicit: bool) -> bool {
    show_keyboard_platform(implicit)
}

pub(crate) fn hide_keyboard(implicit_only: bool) -> bool {
    hide_keyboard_platform(implicit_only)
}

#[cfg(target_os = "android")]
fn show_keyboard_platform(implicit: bool) -> bool {
    let Some(app) = ANDROID_APP.get() else {
        return false;
    };
    app.show_soft_input(implicit);
    true
}

#[cfg(not(target_os = "android"))]
fn show_keyboard_platform(_implicit: bool) -> bool {
    false
}

#[cfg(target_os = "android")]
fn hide_keyboard_platform(implicit_only: bool) -> bool {
    let Some(app) = ANDROID_APP.get() else {
        return false;
    };
    app.hide_soft_input(implicit_only);
    true
}

#[cfg(not(target_os = "android"))]
fn hide_keyboard_platform(_implicit_only: bool) -> bool {
    false
}

pub(crate) fn add_android_module(lua: &Lua) -> mlua::Result<()> {
    let module = lua.create_table()?;

    module.set(
        "isAndroid",
        lua.create_function(|_lua, ()| Ok(cfg!(target_os = "android")))?,
    )?;
    module.set(
        "getDeviceId",
        lua.create_function(|_lua, ()| Ok(android_info().device_id))?,
    )?;
    module.set(
        "getSdkInt",
        lua.create_function(|_lua, ()| Ok(android_info().sdk_int))?,
    )?;
    module.set(
        "getApiLevel",
        lua.create_function(|_lua, ()| Ok(android_info().sdk_int))?,
    )?;
    module.set(
        "getBrand",
        lua.create_function(|_lua, ()| Ok(android_info().brand))?,
    )?;
    module.set(
        "getManufacturer",
        lua.create_function(|_lua, ()| Ok(android_info().manufacturer))?,
    )?;
    module.set(
        "getModel",
        lua.create_function(|_lua, ()| Ok(android_info().model))?,
    )?;
    module.set(
        "getDevice",
        lua.create_function(|_lua, ()| Ok(android_info().device))?,
    )?;
    module.set(
        "getProduct",
        lua.create_function(|_lua, ()| Ok(android_info().product))?,
    )?;
    module.set(
        "showKeyboard",
        lua.create_function(|_lua, implicit: Option<bool>| {
            Ok(show_keyboard(implicit.unwrap_or(true)))
        })?,
    )?;
    module.set(
        "openKeyboard",
        module.get::<mlua::Function>("showKeyboard")?,
    )?;
    module.set(
        "hideKeyboard",
        lua.create_function(|_lua, implicit_only: Option<bool>| {
            Ok(hide_keyboard(implicit_only.unwrap_or(false)))
        })?,
    )?;
    module.set(
        "closeKeyboard",
        module.get::<mlua::Function>("hideKeyboard")?,
    )?;

    lua.globals().set("android", module)
}
