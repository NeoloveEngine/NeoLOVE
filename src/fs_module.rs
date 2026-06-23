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
    Ok(normalize_path(&candidate))
}

fn resolve_read_path(resource_root: &Path, data_root: &Path, input: &str) -> mlua::Result<PathBuf> {
    let data_path = resolve_path(data_root, input)?;
    if data_path.exists() || Path::new(input).is_absolute() || data_root == resource_root {
        return Ok(data_path);
    }
    resolve_path(resource_root, input)
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

fn picked_path_to_string(path: PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

fn is_webasm_target() -> bool {
    cfg!(target_arch = "wasm32")
}

#[cfg(not(target_arch = "wasm32"))]
fn pick_file() -> Option<String> {
    rfd::FileDialog::new().pick_file().map(picked_path_to_string)
}

#[cfg(target_arch = "wasm32")]
fn pick_file() -> Option<String> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
fn pick_folder() -> Option<String> {
    rfd::FileDialog::new()
        .pick_folder()
        .map(picked_path_to_string)
}

#[cfg(target_arch = "wasm32")]
fn pick_folder() -> Option<String> {
    None
}

fn create_walk_entry(lua: &Lua, root: &Path, path: &Path) -> mlua::Result<Table> {
    let metadata = fs::metadata(path).map_err(|error| io_error("stat", path, &error))?;
    let entry = lua.create_table()?;
    let is_file = metadata.is_file();
    let is_dir = metadata.is_dir();
    let kind = if is_dir {
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
    entry.set("isFile", is_file)?;
    entry.set("isDir", is_dir)?;
    entry.set("is_file", is_file)?;
    entry.set("is_dir", is_dir)?;
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
    add_fs_module_with_data_root(lua, env_root.clone(), env_root)
}

pub(crate) fn add_fs_module_with_data_root(
    lua: &Lua,
    resource_root: PathBuf,
    data_root: PathBuf,
) -> mlua::Result<()> {
    let module = lua.create_table()?;

    module.set(
        "isWebasm",
        lua.create_function(|_lua, ()| Ok(is_webasm_target()))?,
    )?;
    module.set(
        "isWebAssembly",
        lua.create_function(|_lua, ()| Ok(is_webasm_target()))?,
    )?;
    module.set(
        "openFilePicker",
        lua.create_function(|_lua, ()| Ok(pick_file()))?,
    )?;
    module.set(
        "openFolderPicker",
        lua.create_function(|_lua, ()| Ok(pick_folder()))?,
    )?;

    module.set(
        "getDataDirectory",
        lua.create_function({
            let data_root = data_root.clone();
            move |_lua, ()| Ok(data_root.to_string_lossy().into_owned())
        })?,
    )?;
    module.set(
        "dataPath",
        lua.create_function({
            let data_root = data_root.clone();
            move |_lua, path: String| {
                Ok(resolve_path(&data_root, &path)?
                    .to_string_lossy()
                    .into_owned())
            }
        })?,
    )?;

    let read_resource_root = resource_root.clone();
    let read_data_root = data_root.clone();
    module.set(
        "readFile",
        lua.create_function(move |_lua, path: String| {
            let path = resolve_read_path(&read_resource_root, &read_data_root, &path)?;
            fs::read_to_string(&path).map_err(|error| io_error("read file", &path, &error))
        })?,
    )?;

    let write_root = data_root.clone();
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

    let append_root = data_root.clone();
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

    let exists_resource_root = resource_root.clone();
    let exists_data_root = data_root.clone();
    module.set(
        "exists",
        lua.create_function(move |_lua, path: String| {
            let path = resolve_read_path(&exists_resource_root, &exists_data_root, &path)?;
            Ok(path.exists())
        })?,
    )?;

    let is_file_resource_root = resource_root.clone();
    let is_file_data_root = data_root.clone();
    module.set(
        "isFile",
        lua.create_function(move |_lua, path: String| {
            let path =
                resolve_read_path(&is_file_resource_root, &is_file_data_root, &path)?;
            Ok(path.is_file())
        })?,
    )?;

    let is_dir_resource_root = resource_root.clone();
    let is_dir_data_root = data_root.clone();
    module.set(
        "isDir",
        lua.create_function(move |_lua, path: String| {
            let path = resolve_read_path(&is_dir_resource_root, &is_dir_data_root, &path)?;
            Ok(path.is_dir())
        })?,
    )?;

    let mkdir_root = data_root.clone();
    module.set(
        "createDir",
        lua.create_function(move |_lua, path: String| {
            let path = resolve_path(&mkdir_root, &path)?;
            fs::create_dir_all(&path).map_err(|error| io_error("create directory", &path, &error))
        })?,
    )?;

    let walk_resource_root = resource_root.clone();
    let walk_data_root = data_root.clone();
    module.set(
        "walk",
        lua.create_function(
            move |lua, (path, recursive): (Option<String>, Option<bool>)| {
                let start = match path {
                    Some(path) => {
                        resolve_read_path(&walk_resource_root, &walk_data_root, &path)?
                    }
                    None => walk_data_root.clone(),
                };
                let mut entries = Vec::new();
                collect_walk_entries(&start, recursive.unwrap_or(true), &mut entries)
                    .map_err(|error| io_error("walk path", &start, &error))?;
                let result = lua.create_table()?;
                let display_root = if start.starts_with(&walk_data_root) {
                    &walk_data_root
                } else if start.starts_with(&walk_resource_root) {
                    &walk_resource_root
                } else {
                    &start
                };
                for path in entries {
                    result.push(create_walk_entry(lua, display_root, &path)?)?;
                }
                Ok(result)
            },
        )?,
    )?;

    let rename_data_root = data_root.clone();
    module.set(
        "rename",
        lua.create_function(move |_lua, (from, to): (String, String)| {
            let from = resolve_path(&rename_data_root, &from)?;
            let to = resolve_path(&rename_data_root, &to)?;
            ensure_parent_dir(&to).map_err(|error| io_error("create parent directory", &to, &error))?;
            fs::rename(&from, &to).map_err(|error| io_pair_error("rename", &from, &to, &error))
        })?,
    )?;

    let copy_resource_root = resource_root;
    let copy_data_root = data_root.clone();
    module.set(
        "copy",
        lua.create_function(move |_lua, (from, to): (String, String)| {
            let from = resolve_read_path(&copy_resource_root, &copy_data_root, &from)?;
            let to = resolve_path(&copy_data_root, &to)?;
            copy_path(&from, &to).map_err(|error| io_pair_error("copy", &from, &to, &error))
        })?,
    )?;

    let rm_root = data_root;
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
    fn resolve_path_allows_paths_outside_project_root() -> mlua::Result<()> {
        let root = PathBuf::from("/tmp/neolove_project");
        let result = resolve_path(&root, "../escape.txt")?;
        assert_eq!(result, PathBuf::from("/tmp/escape.txt"));
        Ok(())
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
    fn separate_data_root_writes_externally_and_reads_packaged_fallbacks() -> mlua::Result<()> {
        let root = temp_root("fs_roots");
        let resource_root = root.join("project");
        let data_root = root.join("game_data");
        let absolute_file = root.join("absolute.txt");
        fs::create_dir_all(&resource_root).map_err(mlua::Error::external)?;
        fs::create_dir_all(&data_root).map_err(mlua::Error::external)?;
        fs::write(resource_root.join("bundled.txt"), "bundled")
            .map_err(mlua::Error::external)?;

        let lua = Lua::new();
        add_fs_module_with_data_root(&lua, resource_root.clone(), data_root.clone())?;
        lua.globals()
            .set("absoluteFile", absolute_file.to_string_lossy().into_owned())?;
        lua.load(
            r#"
            assert(fs.isWebasm() == false)
            assert(fs.isWebAssembly() == false)
            assert(fs.getDataDirectory() ~= "")
            assert(fs.readFile("bundled.txt") == "bundled")
            fs.writeFile("save/state.txt", "saved")
            assert(fs.readFile("save/state.txt") == "saved")
            fs.copy("bundled.txt", "copied.txt")
            fs.writeFile(absoluteFile, "absolute")
            assert(fs.dataPath("save/state.txt") ~= "")
            "#,
        )
        .exec()?;

        assert_eq!(
            fs::read_to_string(data_root.join("save/state.txt"))
                .map_err(mlua::Error::external)?,
            "saved"
        );
        assert_eq!(
            fs::read_to_string(data_root.join("copied.txt")).map_err(mlua::Error::external)?,
            "bundled"
        );
        assert_eq!(
            fs::read_to_string(&absolute_file).map_err(mlua::Error::external)?,
            "absolute"
        );

        fs::remove_dir_all(root).map_err(mlua::Error::external)?;
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
