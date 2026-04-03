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
        return format!("mlua::Error: {}", describe_lua_error(error));
    }
    "non-string panic payload".to_string()
}

fn format_traceback(traceback: &str) -> Option<String> {
    let trimmed = traceback.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn join_context(context: &str, message: String) -> String {
    let trimmed = context.trim();
    if trimmed.is_empty() {
        return message;
    }
    if message.is_empty() || message.starts_with(trimmed) {
        return message;
    }
    format!("{trimmed}\n{message}")
}

fn flatten_lua_error(error: &mlua::Error) -> (String, Option<String>) {
    match error {
        mlua::Error::CallbackError { traceback, cause } => {
            let (message, nested_traceback) = flatten_lua_error(cause.as_ref());
            (message, nested_traceback.or_else(|| format_traceback(traceback)))
        }
        mlua::Error::WithContext { context, cause } => {
            let (message, traceback) = flatten_lua_error(cause.as_ref());
            (join_context(context, message), traceback)
        }
        mlua::Error::ExternalError(err) => {
            let message = err.to_string().trim().to_string();
            (message, None)
        }
        _ => {
            let message = error.to_string().trim().to_string();
            (message, None)
        }
    }
}

pub fn describe_lua_error(error: &mlua::Error) -> String {
    let (mut message, traceback) = flatten_lua_error(error);
    if message.trim().is_empty() {
        return format!("(empty Lua error message)\nDebug: {error:?}");
    }

    if !message.contains("stack traceback:") {
        if let Some(traceback) = traceback {
            message.push('\n');
            message.push_str(&traceback);
        }
    }

    message
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
