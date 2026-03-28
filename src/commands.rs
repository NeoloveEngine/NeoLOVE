use mlua::{Lua, Table};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn resolve_cwd(env_root: &Path, cwd: Option<String>) -> Result<PathBuf, String> {
    let candidate = match cwd {
        Some(value) => {
            let p = PathBuf::from(value);
            if p.is_absolute() { p } else { env_root.join(p) }
        }
        None => env_root.to_path_buf(),
    };
    let resolved = normalize_path(&candidate);
    if !resolved.starts_with(env_root) {
        return Err("cwd escapes project root".to_string());
    }
    Ok(resolved)
}

fn parse_args(args: Option<Table>) -> mlua::Result<Vec<String>> {
    let mut out = Vec::<String>::new();
    let Some(table) = args else {
        return Ok(out);
    };
    for value in table.sequence_values::<String>() {
        out.push(value?);
    }
    Ok(out)
}

fn command_error(command: &str, cwd: &Path, error: &std::io::Error) -> String {
    format!(
        "failed to run '{}' in '{}': {}",
        command,
        cwd.display(),
        error
    )
}

pub(crate) fn add_commands_module(lua: &Lua, env_root: PathBuf) -> mlua::Result<()> {
    let module = lua.create_table()?;

    let run_root = env_root.clone();
    module.set(
        "run",
        lua.create_function(
            move |lua, (command, args, cwd): (String, Option<Table>, Option<String>)| {
                let args = parse_args(args)?;
                let cwd = resolve_cwd(&run_root, cwd).map_err(mlua::Error::external)?;
                let command = command.trim().to_string();

                let out = lua.create_table()?;
                if command.is_empty() {
                    out.set("ok", false)?;
                    out.set("status_code", -1)?;
                    out.set("stdout", "")?;
                    out.set("stderr", "")?;
                    out.set("error", "command cannot be empty")?;
                    return Ok(out);
                }

                let cmd_out = Command::new(&command)
                    .args(args.iter())
                    .current_dir(&cwd)
                    .output();

                match cmd_out {
                    Ok(output) => {
                        let status_code = output.status.code().unwrap_or(-1);
                        out.set("ok", output.status.success())?;
                        out.set("status_code", status_code)?;
                        out.set(
                            "stdout",
                            String::from_utf8_lossy(&output.stdout).to_string(),
                        )?;
                        out.set(
                            "stderr",
                            String::from_utf8_lossy(&output.stderr).to_string(),
                        )?;
                        out.set("error", mlua::Value::Nil)?;
                    }
                    Err(error) => {
                        out.set("ok", false)?;
                        out.set("status_code", -1)?;
                        out.set("stdout", "")?;
                        out.set("stderr", "")?;
                        out.set("error", command_error(&command, &cwd, &error))?;
                    }
                }

                Ok(out)
            },
        )?,
    )?;

    let run_detached_root = env_root;
    module.set(
        "runDetached",
        lua.create_function(
            move |lua, (command, args, cwd): (String, Option<Table>, Option<String>)| {
                let args = parse_args(args)?;
                let cwd = resolve_cwd(&run_detached_root, cwd).map_err(mlua::Error::external)?;
                let command = command.trim().to_string();

                let out = lua.create_table()?;
                if command.is_empty() {
                    out.set("ok", false)?;
                    out.set("pid", 0)?;
                    out.set("error", "command cannot be empty")?;
                    return Ok(out);
                }

                let spawn = Command::new(&command)
                    .args(args.iter())
                    .current_dir(&cwd)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();

                match spawn {
                    Ok(child) => {
                        out.set("ok", true)?;
                        out.set("pid", child.id())?;
                        out.set("error", mlua::Value::Nil)?;
                    }
                    Err(error) => {
                        out.set("ok", false)?;
                        out.set("pid", 0)?;
                        out.set("error", command_error(&command, &cwd, &error))?;
                    }
                }

                Ok(out)
            },
        )?,
    )?;

    lua.globals().set("commands", module.clone())?;
    lua.globals().set("command", module)?;
    Ok(())
}
