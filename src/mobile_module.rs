use mlua::Lua;

use crate::platform::{SharedPlatformState, lock_platform_state};

fn is_real_mobile() -> bool {
    cfg!(target_os = "android") || cfg!(target_os = "ios")
}

pub(crate) fn is_mobile() -> bool {
    is_real_mobile() || crate::mobile_emulation::enabled()
}

pub(crate) fn add_mobile_module(lua: &Lua, platform: SharedPlatformState) -> mlua::Result<()> {
    let module = lua.create_table()?;

    module.set("isMobile", lua.create_function(|_lua, ()| Ok(is_mobile()))?)?;
    module.set(
        "isEmulated",
        lua.create_function(|_lua, ()| Ok(crate::mobile_emulation::enabled()))?,
    )?;
    module.set(
        "isOnline",
        lua.create_function(|_lua, ()| {
            Ok(crate::mobile_emulation::MobileEmulation::from_env().online())
        })?,
    )?;
    module.set(
        "isWifiEnabled",
        lua.create_function(|_lua, ()| {
            Ok(crate::mobile_emulation::MobileEmulation::from_env().wifi)
        })?,
    )?;
    module.set(
        "isCellularEnabled",
        lua.create_function(|_lua, ()| {
            Ok(crate::mobile_emulation::MobileEmulation::from_env().cellular)
        })?,
    )?;
    module.set(
        "isLowPowerMode",
        lua.create_function(|_lua, ()| {
            Ok(crate::mobile_emulation::MobileEmulation::from_env().low_power)
        })?,
    )?;
    module.set(
        "getNetworkType",
        lua.create_function(|_lua, ()| {
            Ok(crate::mobile_emulation::MobileEmulation::from_env().network_type())
        })?,
    )?;
    module.set(
        "getOrientation",
        lua.create_function(|_lua, ()| {
            Ok(crate::mobile_emulation::MobileEmulation::from_env()
                .orientation
                .as_str())
        })?,
    )?;
    module.set(
        "isLandscape",
        lua.create_function(|_lua, ()| {
            Ok(
                crate::mobile_emulation::MobileEmulation::from_env().orientation
                    == crate::mobile_emulation::MobileOrientation::Landscape,
            )
        })?,
    )?;
    module.set(
        "getDeviceSize",
        lua.create_function(move |_lua, ()| {
            let profile = crate::mobile_emulation::MobileEmulation::from_env();
            let (width, height) = if profile.enabled {
                profile.oriented_size()
            } else {
                let window = lock_platform_state(&platform).window();
                (
                    window.width.max(1.0).round() as u32,
                    window.height.max(1.0).round() as u32,
                )
            };
            Ok((width, height))
        })?,
    )?;
    module.set(
        "getSafeAreaInsets",
        lua.create_function(|_lua, ()| {
            let profile = crate::mobile_emulation::MobileEmulation::from_env();
            let portrait =
                profile.orientation == crate::mobile_emulation::MobileOrientation::Portrait;
            let top = if is_mobile() && portrait { 47 } else { 0 };
            let bottom = if is_mobile() && portrait { 34 } else { 0 };
            Ok((top, 0, bottom, 0))
        })?,
    )?;

    lua.globals().set("mobile", module)
}
