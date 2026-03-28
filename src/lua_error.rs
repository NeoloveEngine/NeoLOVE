use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

fn panic_payload_to_string(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    if let Some(error) = payload.downcast_ref::<mlua::Error>() {
        let display = error.to_string();
        if display.trim().is_empty() {
            return format!("mlua::Error (debug): {error:?}");
        }
        return format!("mlua::Error: {display}\nDebug: {error:?}");
    }
    "non-string panic payload".to_string()
}

pub fn describe_lua_error(error: &mlua::Error) -> String {
    let display = error.to_string();
    let debug = format!("{error:?}");
    if display.trim().is_empty() {
        format!("(empty Lua error message)\nDebug: {debug}")
    } else if display == debug {
        display
    } else {
        format!("{display}\nDebug: {debug}")
    }
}

pub fn describe_panic(payload: &(dyn Any + Send)) -> String {
    panic_payload_to_string(payload)
}

pub fn protect_lua_call<F>(context: &str, call: F) -> mlua::Result<()>
where
    F: FnOnce() -> mlua::Result<()>,
{
    match catch_unwind(AssertUnwindSafe(call)) {
        Ok(result) => result.map_err(|error| {
            mlua::Error::external(format!("{context}\n{}", describe_lua_error(&error)))
        }),
        Err(payload) => Err(mlua::Error::external(format!(
            "Rust panic while {context}\nPanic: {}",
            describe_panic(payload.as_ref())
        ))),
    }
}
