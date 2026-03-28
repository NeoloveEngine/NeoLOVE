use mlua::{Lua, Table};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
            Component::RootDir | Component::Prefix(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

fn resolve_path(root: &Path, input: &str) -> mlua::Result<PathBuf> {
    let path = PathBuf::from(input);
    let candidate = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    let resolved = normalize_path(&candidate);
    if !resolved.starts_with(root) {
        return Err(mlua::Error::external(format!(
            "path escapes project root: {}",
            input
        )));
    }
    Ok(resolved)
}

fn ensure_parent_dir(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn io_error(action: &str, path: &Path, error: &std::io::Error) -> mlua::Error {
    mlua::Error::external(format!(
        "failed to {action} '{}': {error}",
        path.display()
    ))
}

fn io_pair_error(action: &str, from: &Path, to: &Path, error: &std::io::Error) -> mlua::Error {
    mlua::Error::external(format!(
        "failed to {action} '{}' -> '{}': {error}",
        from.display(),
        to.display()
    ))
}

fn contextual_io_error(action: &str, path: &Path, error: std::io::Error) -> std::io::Error {
    std::io::Error::new(
        error.kind(),
        format!("failed to {action} '{}': {error}", path.display()),
    )
}

fn contextual_io_pair_error(
    action: &str,
    from: &Path,
    to: &Path,
    error: std::io::Error,
) -> std::io::Error {
    std::io::Error::new(
        error.kind(),
        format!(
            "failed to {action} '{}' -> '{}': {error}",
            from.display(),
            to.display()
        ),
    )
}

fn path_to_project_string(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let value = relative.to_string_lossy().replace('\\', "/");
    if value.is_empty() {
        ".".to_string()
    } else {
        value
    }
}

fn create_walk_entry(lua: &Lua, root: &Path, path: &Path) -> mlua::Result<Table> {
    let metadata = fs::metadata(path).map_err(|error| io_error("stat", path, &error))?;
    let entry = lua.create_table()?;
    let kind = if metadata.is_dir() {
        "directory"
    } else {
        "file"
    };
    entry.set("path", path_to_project_string(root, path))?;
    entry.set(
        "name",
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".".to_string()),
    )?;
    entry.set("kind", kind)?;
    entry.set("is_file", metadata.is_file())?;
    entry.set("is_dir", metadata.is_dir())?;
    Ok(entry)
}

fn collect_walk_entries(
    path: &Path,
    recursive: bool,
    entries: &mut Vec<PathBuf>,
) -> std::io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_file() {
        entries.push(path.to_path_buf());
        return Ok(());
    }

    let mut children = Vec::new();
    for entry in fs::read_dir(path).map_err(|error| contextual_io_error("read directory", path, error))? {
        let entry = entry.map_err(|error| contextual_io_error("read directory entry", path, error))?;
        children.push(entry.path());
    }
    children.sort();

    for child in children {
        entries.push(child.clone());
        if recursive && child.is_dir() {
            collect_walk_entries(&child, true, entries)?;
        }
    }

    Ok(())
}

fn copy_path(source: &Path, destination: &Path) -> std::io::Result<()> {
    if source == destination {
        return Err(std::io::Error::other("source and destination are the same"));
    }
    if source.is_dir() {
        if destination.starts_with(source) {
            return Err(std::io::Error::other("cannot copy a directory into itself"));
        }
        fs::create_dir_all(destination)
            .map_err(|error| contextual_io_pair_error("create directory", source, destination, error))?;
        let mut children = Vec::new();
        for entry in fs::read_dir(source)
            .map_err(|error| contextual_io_pair_error("read directory", source, destination, error))?
        {
            let entry = entry.map_err(|error| {
                contextual_io_pair_error("read directory entry", source, destination, error)
            })?;
            children.push(entry.path());
        }
        children.sort();
        for child in children {
            let child_destination = destination.join(
                child
                    .file_name()
                    .ok_or_else(|| std::io::Error::other("missing child file name"))?,
            );
            copy_path(&child, &child_destination)?;
        }
        return Ok(());
    }

    ensure_parent_dir(destination)?;
    fs::copy(source, destination)
        .map_err(|error| contextual_io_pair_error("copy file", source, destination, error))?;
    Ok(())
}

pub(crate) fn add_fs_module(lua: &Lua, env_root: PathBuf) -> mlua::Result<()> {
    let module = lua.create_table()?;

    let read_root = env_root.clone();
    module.set(
        "readFile",
        lua.create_function(move |_lua, path: String| {
            let path = resolve_path(&read_root, &path)?;
            fs::read_to_string(&path).map_err(|error| io_error("read file", &path, &error))
        })?,
    )?;

    let write_root = env_root.clone();
    module.set(
        "writeFile",
        lua.create_function(move |_lua, (path, content): (String, String)| {
            let path = resolve_path(&write_root, &path)?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| io_error("create directory", parent, &error))?;
            }
            fs::write(&path, content).map_err(|error| io_error("write file", &path, &error))
        })?,
    )?;

    let append_root = env_root.clone();
    module.set(
        "appendFile",
        lua.create_function(move |_lua, (path, content): (String, String)| {
            let path = resolve_path(&append_root, &path)?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| io_error("create directory", parent, &error))?;
            }
            let mut file = OpenOptions::new()
                .append(true)
                .create(true)
                .open(&path)
                .map_err(|error| io_error("open file for append", &path, &error))?;
            file.write_all(content.as_bytes())
                .map_err(|error| io_error("append file", &path, &error))
        })?,
    )?;

    let exists_root = env_root.clone();
    module.set(
        "exists",
        lua.create_function(move |_lua, path: String| {
            let path = resolve_path(&exists_root, &path)?;
            Ok(path.exists())
        })?,
    )?;

    let is_file_root = env_root.clone();
    module.set(
        "isFile",
        lua.create_function(move |_lua, path: String| {
            let path = resolve_path(&is_file_root, &path)?;
            Ok(path.is_file())
        })?,
    )?;

    let is_dir_root = env_root.clone();
    module.set(
        "isDir",
        lua.create_function(move |_lua, path: String| {
            let path = resolve_path(&is_dir_root, &path)?;
            Ok(path.is_dir())
        })?,
    )?;

    let mkdir_root = env_root.clone();
    module.set(
        "createDir",
        lua.create_function(move |_lua, path: String| {
            let path = resolve_path(&mkdir_root, &path)?;
            fs::create_dir_all(&path).map_err(|error| io_error("create directory", &path, &error))
        })?,
    )?;

    let walk_root = env_root.clone();
    module.set(
        "walk",
        lua.create_function(
            move |lua, (path, recursive): (Option<String>, Option<bool>)| {
                let start = match path {
                    Some(path) => resolve_path(&walk_root, &path)?,
                    None => walk_root.clone(),
                };
                let mut entries = Vec::new();
                collect_walk_entries(&start, recursive.unwrap_or(true), &mut entries)
                    .map_err(|error| io_error("walk path", &start, &error))?;
                let result = lua.create_table()?;
                for path in entries {
                    result.push(create_walk_entry(lua, &walk_root, &path)?)?;
                }
                Ok(result)
            },
        )?,
    )?;

    let rename_root = env_root.clone();
    module.set(
        "rename",
        lua.create_function(move |_lua, (from, to): (String, String)| {
            let from = resolve_path(&rename_root, &from)?;
            let to = resolve_path(&rename_root, &to)?;
            ensure_parent_dir(&to).map_err(|error| io_error("create parent directory", &to, &error))?;
            fs::rename(&from, &to).map_err(|error| io_pair_error("rename", &from, &to, &error))
        })?,
    )?;

    let copy_root = env_root.clone();
    module.set(
        "copy",
        lua.create_function(move |_lua, (from, to): (String, String)| {
            let from = resolve_path(&copy_root, &from)?;
            let to = resolve_path(&copy_root, &to)?;
            copy_path(&from, &to).map_err(|error| io_pair_error("copy", &from, &to, &error))
        })?,
    )?;

    let rm_root = env_root;
    module.set(
        "removeFile",
        lua.create_function(move |_lua, path: String| {
            let path = resolve_path(&rm_root, &path)?;
            match fs::remove_file(&path) {
                Ok(()) => Ok(true),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(err) => Err(io_error("remove file", &path, &err)),
            }
        })?,
    )?;

    lua.globals().set("fs", module)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("neolove_{name}_{unique}"))
    }

    #[test]
    fn resolve_path_rejects_project_escape() {
        let root = PathBuf::from("/tmp/neolove_project");
        let result = resolve_path(&root, "../escape.txt");
        assert!(result.is_err());
    }

    #[test]
    fn copy_path_recurses_directories() -> std::io::Result<()> {
        let root = temp_root("fs_copy");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(source.join("nested"))?;
        fs::write(source.join("nested").join("file.txt"), "hello")?;

        copy_path(&source, &destination)?;

        assert_eq!(
            fs::read_to_string(destination.join("nested").join("file.txt"))?,
            "hello"
        );

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn collect_walk_entries_returns_sorted_descendants() -> std::io::Result<()> {
        let root = temp_root("fs_walk");
        fs::create_dir_all(root.join("b"))?;
        fs::write(root.join("a.txt"), "a")?;
        fs::write(root.join("b").join("c.txt"), "c")?;

        let mut entries = Vec::new();
        collect_walk_entries(&root, true, &mut entries)?;
        let rendered: Vec<String> = entries
            .iter()
            .map(|path| path_to_project_string(&root, path))
            .collect();
        assert_eq!(rendered, vec!["a.txt", "b", "b/c.txt"]);

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn copy_path_rejects_copying_directory_into_itself() -> std::io::Result<()> {
        let root = temp_root("fs_copy_loop");
        let source = root.join("source");
        fs::create_dir_all(source.join("nested"))?;

        let error = copy_path(&source, &source.join("nested").join("copy"));
        assert!(error.is_err());

        fs::remove_dir_all(root)?;
        Ok(())
    }
}
