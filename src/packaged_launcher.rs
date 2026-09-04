#![cfg_attr(windows, windows_subsystem = "windows")]

use flate2::read::DeflateDecoder;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const WRAPPER_MAGIC: &[u8; 16] = b"NEOLOVE_WRAPPED1";
const EMBED_TRAILER_MAGIC: &[u8; 16] = b"NEOLOVE_EMBED_V1";

fn hash64(data: &[u8]) -> u64 {
    let mut hash = 1469598103934665603u64;
    for byte in data {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    hash
}

fn read_compressed_runtime(executable: &Path) -> Result<Vec<u8>, String> {
    let mut file =
        File::open(executable).map_err(|error| format!("failed to open packaged game: {error}"))?;
    let file_len = file
        .metadata()
        .map_err(|error| format!("failed to inspect packaged game: {error}"))?
        .len();
    let trailer_len = 8 + WRAPPER_MAGIC.len() as u64;
    let payload_trailer_len = 8 + EMBED_TRAILER_MAGIC.len() as u64;
    if file_len < trailer_len + payload_trailer_len {
        return Err("packaged game is missing its runtime".to_string());
    }
    file.seek(SeekFrom::End(-(payload_trailer_len as i64)))
        .map_err(|error| format!("failed to seek to packaged game data: {error}"))?;
    let mut payload_length = [0; 8];
    file.read_exact(&mut payload_length)
        .map_err(|error| format!("failed to read packaged game data length: {error}"))?;
    let payload_len = u64::from_le_bytes(payload_length);
    let mut payload_magic = [0; 16];
    file.read_exact(&mut payload_magic)
        .map_err(|error| format!("failed to read packaged game data marker: {error}"))?;
    if &payload_magic != EMBED_TRAILER_MAGIC
        || payload_len > file_len - trailer_len - payload_trailer_len
    {
        return Err("packaged game data trailer is invalid".to_string());
    }

    let wrapper_trailer_start = file_len - payload_trailer_len - payload_len - trailer_len;
    file.seek(SeekFrom::Start(wrapper_trailer_start))
        .map_err(|error| format!("failed to seek to packaged runtime: {error}"))?;
    let mut length = [0; 8];
    file.read_exact(&mut length)
        .map_err(|error| format!("failed to read packaged runtime length: {error}"))?;
    let compressed_len = u64::from_le_bytes(length);
    let mut magic = [0; 16];
    file.read_exact(&mut magic)
        .map_err(|error| format!("failed to read packaged runtime marker: {error}"))?;
    if &magic != WRAPPER_MAGIC || compressed_len > wrapper_trailer_start {
        return Err("packaged runtime trailer is invalid".to_string());
    }
    file.seek(SeekFrom::Start(wrapper_trailer_start - compressed_len))
        .map_err(|error| format!("failed to seek to packaged runtime data: {error}"))?;
    let mut compressed = vec![0; compressed_len as usize];
    file.read_exact(&mut compressed)
        .map_err(|error| format!("failed to read packaged runtime data: {error}"))?;
    Ok(compressed)
}

fn runtime_cache_dir() -> PathBuf {
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty()) {
        return PathBuf::from(root).join("NeoLOVE").join("runtimes");
    }
    #[cfg(target_os = "macos")]
    if let Some(root) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(root)
            .join("Library")
            .join("Caches")
            .join("NeoLOVE")
            .join("runtimes");
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(root) = std::env::var_os("XDG_CACHE_HOME").filter(|value| !value.is_empty()) {
            return PathBuf::from(root).join("neolove").join("runtimes");
        }
        if let Some(root) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
            return PathBuf::from(root)
                .join(".cache")
                .join("neolove")
                .join("runtimes");
        }
    }
    std::env::temp_dir().join("neolove-runtimes")
}

fn cached_runtime_path(compressed: &[u8]) -> PathBuf {
    let extension = if cfg!(windows) { ".exe" } else { "" };
    runtime_cache_dir().join(format!(
        "neolove_runtime_{:016x}{extension}",
        hash64(compressed)
    ))
}

fn ensure_runtime(compressed: &[u8]) -> Result<PathBuf, String> {
    let target = cached_runtime_path(compressed);
    if target.is_file() {
        return Ok(target);
    }
    fs::create_dir_all(
        target
            .parent()
            .expect("cached runtime path always has a cache directory"),
    )
    .map_err(|error| format!("failed to create runtime cache directory: {error}"))?;

    let mut decoder = DeflateDecoder::new(compressed);
    let temporary = target.with_extension(format!(
        "{}.tmp-{}",
        target
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default(),
        std::process::id()
    ));
    let mut output = File::create(&temporary)
        .map_err(|error| format!("failed to create runtime cache: {error}"))?;
    std::io::copy(&mut decoder, &mut output)
        .map_err(|error| format!("failed to decompress game runtime: {error}"))?;
    output
        .flush()
        .map_err(|error| format!("failed to finish runtime cache: {error}"))?;
    drop(output);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755))
            .map_err(|error| format!("failed to make cached runtime executable: {error}"))?;
    }

    match fs::rename(&temporary, &target) {
        Ok(()) => {}
        Err(_) if target.is_file() => {
            let _ = fs::remove_file(&temporary);
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(format!("failed to publish runtime cache: {error}"));
        }
    }
    Ok(target)
}

fn run() -> Result<i32, String> {
    let launcher = std::env::current_exe()
        .map_err(|error| format!("failed to locate packaged game: {error}"))?;
    let compressed = read_compressed_runtime(&launcher)?;
    let runtime = ensure_runtime(&compressed)?;
    let mut command = Command::new(runtime);
    command.env("NEOLOVE_LAUNCHER_PATH", launcher);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        Err(format!("failed to start game runtime: {error}"))
    }
    #[cfg(not(unix))]
    {
        let status = command
            .status()
            .map_err(|error| format!("failed to start game runtime: {error}"))?;
        Ok(status.code().unwrap_or(1))
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code.clamp(0, u8::MAX as i32) as u8),
        Err(error) => {
            eprintln!("NeoLOVE launch error: {error}");
            ExitCode::FAILURE
        }
    }
}
