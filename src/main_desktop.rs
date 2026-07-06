mod assets;
mod animation;
mod audio_system;
mod commands;
mod core;
mod editor;
mod editor_ipc;
mod fs_module;
#[cfg(feature = "vulkan")]
mod gpu_renderer;
pub mod hierarchy;
mod http;
mod lua_error;
mod platform;
mod prefabs;
mod renderer;
#[path = "editor/scene.rs"]
mod scene;
mod servers;
mod shader;
mod tweening;
mod update;
mod user_input;
pub mod window;

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Once;
use std::time::{Duration, Instant};

use image::imageops::FilterType;
use mlua::Compiler;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(windows)]
use std::process::Command;
use winit::dpi::LogicalSize;
use winit::event::{
    ElementState, Event, KeyboardInput, MouseButton, MouseScrollDelta, VirtualKeyCode, WindowEvent,
};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::window::{CursorGrabMode, Icon, WindowBuilder};
use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

#[cfg(feature = "vulkan")]
use crate::gpu_renderer::VulkanPresenter;
use crate::platform::SharedPlatformState;
use crate::renderer::SoftwareRenderer;

const EMBED_TRAILER_MAGIC: &[u8; 16] = b"NEOLOVE_EMBED_V1";
const PAYLOAD_MAGIC: &[u8; 8] = b"NLPKGv1\0";
const COMPRESSED_PAYLOAD_MAGIC: &[u8; 8] = b"NLPKGv2\0";
const TEMPLATE_LUAURC: &str = include_str!("project_template/.luaurc");
const TEMPLATE_VSCODE_SETTINGS: &str = include_str!("project_template/vscode_settings.json");
const TEMPLATE_NEOLOVE_ENGINE_API: &str =
    include_str!("project_template/neolove_engine_api.d.luau");
const DEFAULT_WINDOW_WIDTH: f32 = 1280.0;
const DEFAULT_WINDOW_HEIGHT: f32 = 720.0;

enum DesktopPresenter {
    #[cfg(feature = "vulkan")]
    Vulkan(VulkanPresenter),
    Software {
        _context: softbuffer::Context,
        surface: softbuffer::Surface,
        renderer: SoftwareRenderer,
        vulkan_error: Option<String>,
    },
}

impl DesktopPresenter {
    fn new(
        event_loop: &EventLoop<()>,
        window: &std::sync::Arc<winit::window::Window>,
    ) -> Result<Self, String> {
        #[cfg(feature = "vulkan")]
        {
            match catch_desktop_panic("failed while initializing the Vulkan presenter", || {
                VulkanPresenter::new(event_loop, window.clone())
            })? {
                Ok((presenter, _surface)) => Ok(Self::Vulkan(presenter)),
                Err(vulkan_error) => {
                    eprintln!(
                        "render warning: Vulkan unavailable, falling back to software renderer: {vulkan_error}"
                    );
                    Self::new_software(window, Some(vulkan_error))
                }
            }
        }

        #[cfg(not(feature = "vulkan"))]
        {
            let _ = event_loop;
            Self::new_software(window, None)
        }
    }

    fn new_software(
        window: &std::sync::Arc<winit::window::Window>,
        vulkan_error: Option<String>,
    ) -> Result<Self, String> {
        let context = unsafe { softbuffer::Context::new(window.as_ref()) }
            .map_err(|error| format!("failed to create software renderer context: {error}"))?;
        let surface = unsafe { softbuffer::Surface::new(&context, window.as_ref()) }
            .map_err(|error| format!("failed to create software renderer surface: {error}"))?;
        let size = window.inner_size();
        Ok(Self::Software {
            _context: context,
            surface,
            renderer: SoftwareRenderer::new(size.width, size.height),
            vulkan_error,
        })
    }

    fn request_resize(&mut self) {
        #[cfg(feature = "vulkan")]
        if let Self::Vulkan(presenter) = self {
            presenter.request_swapchain_recreate();
        }
    }

    fn render(
        &mut self,
        window: &winit::window::Window,
        platform_state: &SharedPlatformState,
        render_state: &crate::renderer::SharedRenderState,
    ) -> Result<(), String> {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(presenter) => {
                let size = window.inner_size();
                presenter.render(platform_state, render_state, size.width, size.height)
            }
            Self::Software {
                surface,
                renderer,
                vulkan_error,
                ..
            } => {
                let size = window.inner_size();
                let width = size.width.max(1);
                let height = size.height.max(1);
                renderer.resize(width, height);
                renderer.render(platform_state, render_state).map_err(|error| {
                    let mut message = format!("software renderer failed: {error}");
                    if error.contains("custom shaders require the Vulkan renderer")
                        && let Some(vulkan_error) = vulkan_error.as_deref()
                    {
                        message.push_str("\nVulkan initialization error: ");
                        message.push_str(vulkan_error);
                    }
                    message
                })?;
                surface
                    .resize(
                        NonZeroU32::new(width).expect("window width is clamped to at least 1"),
                        NonZeroU32::new(height).expect("window height is clamped to at least 1"),
                    )
                    .map_err(|error| format!("failed to resize software surface: {error}"))?;
                let mut buffer = surface
                    .buffer_mut()
                    .map_err(|error| format!("failed to acquire software surface buffer: {error}"))?;
                for (dst, rgba) in buffer.iter_mut().zip(renderer.pixels().chunks_exact(4)) {
                    *dst =
                        (rgba[2] as u32) | ((rgba[1] as u32) << 8) | ((rgba[0] as u32) << 16);
                }
                buffer
                    .present()
                    .map_err(|error| format!("failed to present software surface: {error}"))?;
                Ok(())
            }
        }
    }
}

#[derive(Default, Clone)]
struct ProjectSettings {
    package_name: Option<String>,
    window_title: Option<String>,
    window_icon: Option<String>,
}

fn resolve_from_cwd(user_path: &str) -> std::io::Result<PathBuf> {
    let p = PathBuf::from(user_path);

    if p.is_absolute() {
        return Ok(p);
    }

    let cwd = env::current_dir()?;
    Ok(cwd.join(p))
}

fn user_home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        env::var_os("USERPROFILE")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                let drive = env::var_os("HOMEDRIVE")?;
                let path = env::var_os("HOMEPATH")?;
                let mut home = PathBuf::from(drive);
                home.push(path);
                Some(home)
            })
    }
    #[cfg(not(windows))]
    {
        env::var_os("HOME").map(PathBuf::from)
    }
}

#[cfg(not(windows))]
fn upsert_marked_path_line(file_path: &Path, line: &str, marker: &str) -> std::io::Result<bool> {
    let existing = match fs::read_to_string(file_path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };

    let lines: Vec<&str> = existing.lines().collect();
    let mut merged: Vec<String> = Vec::with_capacity(lines.len() + 2);
    let mut i = 0usize;
    let mut inserted = false;

    while i < lines.len() {
        if lines[i].trim() == marker {
            if !inserted {
                merged.push(marker.to_string());
                merged.push(line.to_string());
                inserted = true;
            }
            i += 1;
            if i < lines.len() {
                i += 1;
            }
            continue;
        }

        merged.push(lines[i].to_string());
        i += 1;
    }

    if !inserted {
        if !merged.is_empty() {
            merged.push(String::new());
        }
        merged.push(marker.to_string());
        merged.push(line.to_string());
    }

    let mut updated = merged.join("\n");
    if !updated.is_empty() {
        updated.push('\n');
    }

    if updated == existing {
        return Ok(false);
    }

    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(file_path, updated)?;
    Ok(true)
}

#[cfg(not(windows))]
fn ensure_path_contains_self_dir(binary_dir: &Path) -> Result<bool, String> {
    let home = user_home_dir().ok_or_else(|| "could not resolve home directory".to_string())?;
    let shell = env::var("SHELL").unwrap_or_default();
    let dir = binary_dir.to_string_lossy();
    let marker = "# neolove path setup";

    let mut changed_any = false;
    if shell.contains("fish") {
        let fish_path = home.join(".config").join("fish").join("config.fish");
        let line = format!("set -gx PATH \"{}\" $PATH", dir);
        let changed =
            upsert_marked_path_line(&fish_path, &line, marker).map_err(|e| e.to_string())?;
        changed_any |= changed;
    } else {
        let mut targets = vec![home.join(".profile")];
        if shell.contains("zsh") {
            targets.push(home.join(".zshrc"));
        } else {
            targets.push(home.join(".bashrc"));
        }
        let line = format!("export PATH=\"{}:$PATH\"", dir);
        for target in targets {
            let changed =
                upsert_marked_path_line(&target, &line, marker).map_err(|e| e.to_string())?;
            changed_any |= changed;
        }
    }

    Ok(changed_any)
}

#[cfg(windows)]
fn ensure_path_contains_self_dir(binary_dir: &Path) -> Result<bool, String> {
    let escaped_dir = binary_dir.to_string_lossy().replace('\'', "''");

    let script = format!(
        "$d='{escaped_dir}'; \
         $p=[Environment]::GetEnvironmentVariable('Path','User'); \
         if(-not $p){{ $p='' }}; \
         $parts=@($p -split ';' | Where-Object {{ $_ -ne '' }}); \
         $filtered=@(); \
         foreach($part in $parts){{ \
            if($part -eq $d){{ continue }}; \
            $exe=Join-Path $part 'neolove.exe'; \
            if((Test-Path $exe) -and ($part -ne $d)){{ continue }}; \
            $filtered += $part; \
         }}; \
         $newPath=(@($filtered + $d) -join ';'); \
         if($newPath -eq $p){{ Write-Output 'exists'; exit 0 }}; \
         [Environment]::SetEnvironmentVariable('Path', $newPath, 'User'); \
         Write-Output 'updated'"
    );

    let output = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .map_err(|e| format!("failed to run powershell for PATH setup: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("failed to update PATH: {}", stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.contains("updated"))
}

fn setup_path_for_neolove() -> Result<bool, String> {
    let exe = env::current_exe().map_err(|e| format!("could not resolve executable path: {e}"))?;
    let binary_dir = exe
        .parent()
        .ok_or_else(|| "executable has no parent directory".to_string())?;
    ensure_path_contains_self_dir(binary_dir)
}

fn parse_quoted(input: &str) -> Option<String> {
    let value = input.trim();
    if value.len() < 2 {
        return None;
    }
    if !(value.starts_with('"') && value.ends_with('"')) {
        return None;
    }
    Some(value[1..value.len() - 1].to_string())
}

fn parse_project_settings(project_root: &Path) -> ProjectSettings {
    let mut settings = ProjectSettings::default();
    let file_path = project_root.join("neolove.toml");
    let Ok(contents) = fs::read_to_string(file_path) else {
        return settings;
    };

    let mut section = String::new();
    for raw_line in contents.lines() {
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_ascii_lowercase();
            continue;
        }

        let Some((key_raw, value_raw)) = line.split_once('=') else {
            continue;
        };
        let key = key_raw.trim().to_ascii_lowercase();
        let Some(value) = parse_quoted(value_raw) else {
            continue;
        };

        match section.as_str() {
            "package" if key == "name" => settings.package_name = Some(value),
            "window" if key == "title" => settings.window_title = Some(value),
            "window" if key == "icon" => settings.window_icon = Some(value),
            _ => {}
        }
    }

    settings
}

fn try_load_window_icon(project_root: &Path, icon_path: &str) -> Option<Icon> {
    let path = project_root.join(icon_path);
    let bytes = fs::read(path).ok()?;
    let image = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let resized = image::imageops::resize(&image, 64, 64, FilterType::Nearest);
    Icon::from_rgba(resized.into_raw(), 64, 64).ok()
}

fn window_options_for_project(project_root: &Path) -> (String, Option<Icon>) {
    let settings = parse_project_settings(project_root);
    let title = settings
        .window_title
        .or(settings.package_name)
        .unwrap_or_else(|| "NeoLOVE".to_string());

    let icon = settings
        .window_icon
        .as_ref()
        .and_then(|path| try_load_window_icon(project_root, path));

    (title, icon)
}

fn should_skip_in_build(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str();
        name == OsStr::new(".git") || name == OsStr::new("target") || name == OsStr::new("dist")
    })
}

fn is_lua_declaration_file(path: &Path) -> bool {
    let lower = path.to_string_lossy().to_ascii_lowercase();
    lower.ends_with(".d.luau") || lower.ends_with(".d.lua")
}

fn collect_project_files(
    root: &Path,
    current: &Path,
    out: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let entries =
        fs::read_dir(current).map_err(|e| format!("failed to read {}: {e}", current.display()))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("failed to read dir entry: {e}"))?;
        let path = entry.path();

        let rel = path
            .strip_prefix(root)
            .map_err(|e| format!("failed to strip prefix: {e}"))?;
        if should_skip_in_build(rel) {
            continue;
        }

        let file_type = entry
            .file_type()
            .map_err(|e| format!("failed to stat {}: {e}", path.display()))?;

        if file_type.is_dir() {
            collect_project_files(root, &path, out)?;
        } else if file_type.is_file() {
            out.push(path);
        }
    }

    Ok(())
}

fn progress_bar(current: usize, total: usize, message: &str) {
    let width = 30usize;
    let safe_total = total.max(1);
    let ratio = (current as f32 / safe_total as f32).clamp(0.0, 1.0);
    let filled = (ratio * width as f32).round() as usize;
    let bar = format!(
        "{}{}",
        "#".repeat(filled.min(width)),
        "-".repeat(width.saturating_sub(filled.min(width)))
    );

    print!("\r[{bar}] {:>3}% {}", (ratio * 100.0) as usize, message);
    let _ = std::io::stdout().flush();
    if current >= total {
        println!();
    }
}

fn write_u16(buf: &mut Vec<u8>, value: u16) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn write_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn write_u64(buf: &mut Vec<u8>, value: u64) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn read_exact<'a>(data: &'a [u8], index: &mut usize, len: usize) -> Result<&'a [u8], String> {
    if *index + len > data.len() {
        return Err("embedded payload is truncated".to_string());
    }
    let chunk = &data[*index..*index + len];
    *index += len;
    Ok(chunk)
}

fn read_u16(data: &[u8], index: &mut usize) -> Result<u16, String> {
    let bytes = read_exact(data, index, 2)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], index: &mut usize) -> Result<u32, String> {
    let bytes = read_exact(data, index, 4)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_u64(data: &[u8], index: &mut usize) -> Result<u64, String> {
    let bytes = read_exact(data, index, 8)?;
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

fn build_payload(project_root: &Path) -> Result<Vec<u8>, String> {
    let mut files = Vec::new();
    collect_project_files(project_root, project_root, &mut files)?;
    files.sort();

    if files.is_empty() {
        return Err("no project files found to embed".to_string());
    }

    let compiler = Compiler::new()
        .set_optimization_level(2)
        .set_debug_level(0)
        .set_type_info_level(1);

    let total_steps = files.len() + 3;
    let mut step = 0usize;

    step += 1;
    progress_bar(step, total_steps, "Scanning project files");

    let mut payload = Vec::new();
    payload.extend_from_slice(PAYLOAD_MAGIC);
    write_u32(&mut payload, files.len() as u32);

    for file in files {
        let rel = file
            .strip_prefix(project_root)
            .map_err(|e| format!("failed to strip project prefix: {e}"))?;
        let rel_string = rel.to_string_lossy().replace('\\', "/");

        let mut bytes =
            fs::read(&file).map_err(|e| format!("failed to read {}: {e}", file.display()))?;

        let extension = file
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if (extension == "luau" || extension == "lua") && !is_lua_declaration_file(rel) {
            bytes = compiler
                .compile(&bytes)
                .map_err(|e| format!("failed to compile {} to bytecode: {e}", rel.display()))?;
        }

        if rel_string.len() > u16::MAX as usize {
            return Err(format!(
                "path too long for embedded payload: {}",
                rel_string
            ));
        }

        write_u16(&mut payload, rel_string.len() as u16);
        payload.extend_from_slice(rel_string.as_bytes());
        write_u64(&mut payload, bytes.len() as u64);
        payload.extend_from_slice(&bytes);

        step += 1;
        progress_bar(
            step,
            total_steps,
            &format!("Embedding {}", rel.to_string_lossy()),
        );
    }

    step += 1;
    progress_bar(step, total_steps, "Finalizing payload");

    compress_build_payload(&payload)
}

fn compress_build_payload(payload: &[u8]) -> Result<Vec<u8>, String> {
    let cursor = std::io::Cursor::new(Vec::new());
    let mut archive = zip::ZipWriter::new(cursor);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(9));
    archive
        .start_file("project.payload", options)
        .map_err(|error| format!("failed to start compressed build payload: {error}"))?;
    archive
        .write_all(payload)
        .map_err(|error| format!("failed to compress build assets: {error}"))?;
    let cursor = archive
        .finish()
        .map_err(|error| format!("failed to finalize compressed build payload: {error}"))?;
    let mut compressed = Vec::with_capacity(COMPRESSED_PAYLOAD_MAGIC.len() + cursor.get_ref().len());
    compressed.extend_from_slice(COMPRESSED_PAYLOAD_MAGIC);
    compressed.extend_from_slice(cursor.get_ref());
    Ok(compressed)
}

fn read_embedded_payload(exe_path: &Path) -> Result<Option<Vec<u8>>, String> {
    let mut file = File::open(exe_path)
        .map_err(|e| format!("failed to open executable {}: {e}", exe_path.display()))?;

    let file_len = file
        .metadata()
        .map_err(|e| format!("failed to stat executable: {e}"))?
        .len();

    let trailer_len = 8u64 + EMBED_TRAILER_MAGIC.len() as u64;
    if file_len < trailer_len {
        return Ok(None);
    }

    file.seek(SeekFrom::End(-(trailer_len as i64)))
        .map_err(|e| format!("failed to seek trailer: {e}"))?;

    let mut len_buf = [0u8; 8];
    file.read_exact(&mut len_buf)
        .map_err(|e| format!("failed to read embedded length: {e}"))?;
    let payload_len = u64::from_le_bytes(len_buf);

    let mut magic = vec![0u8; EMBED_TRAILER_MAGIC.len()];
    file.read_exact(&mut magic)
        .map_err(|e| format!("failed to read embedded magic: {e}"))?;

    if magic.as_slice() != EMBED_TRAILER_MAGIC {
        return Ok(None);
    }

    if payload_len > file_len.saturating_sub(trailer_len) {
        return Err("embedded payload length is invalid".to_string());
    }

    let payload_start = file_len - trailer_len - payload_len;
    file.seek(SeekFrom::Start(payload_start))
        .map_err(|e| format!("failed to seek embedded payload: {e}"))?;

    let mut payload = vec![0u8; payload_len as usize];
    file.read_exact(&mut payload)
        .map_err(|e| format!("failed to read embedded payload: {e}"))?;

    Ok(Some(payload))
}

fn unpack_payload(payload: &[u8], output_dir: &Path) -> Result<(), String> {
    if payload.starts_with(COMPRESSED_PAYLOAD_MAGIC) {
        let cursor = std::io::Cursor::new(&payload[COMPRESSED_PAYLOAD_MAGIC.len()..]);
        let mut archive = zip::ZipArchive::new(cursor)
            .map_err(|error| format!("compressed payload is invalid: {error}"))?;
        let mut entry = archive
            .by_name("project.payload")
            .map_err(|error| format!("compressed payload has no project data: {error}"))?;
        let mut decoded = Vec::new();
        entry
            .read_to_end(&mut decoded)
            .map_err(|error| format!("failed to decompress build assets: {error}"))?;
        return unpack_payload(&decoded, output_dir);
    }
    let mut index = 0usize;
    let magic = read_exact(payload, &mut index, PAYLOAD_MAGIC.len())?;
    if magic != PAYLOAD_MAGIC {
        return Err("embedded payload magic mismatch".to_string());
    }

    let file_count = read_u32(payload, &mut index)? as usize;

    for _ in 0..file_count {
        let path_len = read_u16(payload, &mut index)? as usize;
        let path_bytes = read_exact(payload, &mut index, path_len)?;
        let rel_path = std::str::from_utf8(path_bytes)
            .map_err(|e| format!("invalid UTF-8 path in payload: {e}"))?;

        let rel_path_buf = PathBuf::from(rel_path);
        if rel_path_buf.is_absolute()
            || rel_path_buf
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err("payload contains an unsafe relative path".to_string());
        }

        let data_len = read_u64(payload, &mut index)? as usize;
        let file_data = read_exact(payload, &mut index, data_len)?;

        let target_path = output_dir.join(rel_path_buf);
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
        }
        fs::write(&target_path, file_data)
            .map_err(|e| format!("failed to write {}: {e}", target_path.display()))?;
    }

    if index != payload.len() {
        return Err("embedded payload has trailing bytes".to_string());
    }

    Ok(())
}

fn hash64(data: &[u8]) -> u64 {
    let mut hash = 1469598103934665603u64;
    for b in data {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    hash
}

fn extract_embedded_project(payload: &[u8]) -> Result<PathBuf, String> {
    let cache_key = format!("neolove_embedded_{:016x}", hash64(payload));
    let root = env::temp_dir().join(cache_key);
    let marker = root.join(".neolove_ready");

    if marker.exists() {
        return Ok(root);
    }

    if root.exists() {
        fs::remove_dir_all(&root).map_err(|e| {
            format!(
                "failed to clean existing embedded cache {}: {e}",
                root.display()
            )
        })?;
    }

    fs::create_dir_all(&root)
        .map_err(|e| format!("failed to create embedded cache {}: {e}", root.display()))?;

    unpack_payload(payload, &root)?;

    fs::write(&marker, b"ok")
        .map_err(|e| format!("failed to create embedded cache marker: {e}"))?;

    Ok(root)
}

fn sanitize_executable_name(value: &str) -> String {
    let trimmed = value.trim();
    let mut out = String::new();
    for c in trimmed.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        } else if c.is_ascii_whitespace() {
            out.push('-');
        }
    }

    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "game".to_string()
    } else {
        out
    }
}

fn project_output_stem(project_root: &Path) -> String {
    let settings = parse_project_settings(project_root);
    let name_seed = settings
        .package_name
        .clone()
        .or_else(|| {
            project_root
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "game".to_string());
    sanitize_executable_name(&name_seed)
}

/// Rewrite the PE `Subsystem` field of a Windows executable image in place,
/// switching it from the console subsystem (used by the `neolove` CLI) to the
/// GUI subsystem so a compiled game launches without spawning a terminal window.
///
/// The dev `neolove.exe` stays a console application; only the copy we hand to
/// players is patched. Field offsets follow the PE/COFF specification and are
/// identical for PE32 and PE32+ images.
#[cfg(windows)]
fn patch_subsystem_to_gui(image: &mut [u8]) -> Result<(), String> {
    const IMAGE_SUBSYSTEM_WINDOWS_GUI: u16 = 2;

    if image.len() < 0x40 || &image[0..2] != b"MZ" {
        return Err("engine executable is not a valid PE image (missing MZ header)".to_string());
    }

    let e_lfanew = u32::from_le_bytes(
        image[0x3C..0x40]
            .try_into()
            .expect("slice is 4 bytes after the length check above"),
    ) as usize;
    // PE signature (4) + COFF file header (20) precede the optional header.
    let opt_header = e_lfanew
        .checked_add(24)
        .ok_or_else(|| "engine executable PE header offset overflowed".to_string())?;
    // Subsystem is a 16-bit field at offset 68 within the optional header.
    let subsystem = opt_header + 68;

    if image.len() < subsystem + 2 || &image[e_lfanew..e_lfanew + 4] != b"PE\0\0" {
        return Err("engine executable is not a valid PE image (truncated headers)".to_string());
    }

    image[subsystem..subsystem + 2].copy_from_slice(&IMAGE_SUBSYSTEM_WINDOWS_GUI.to_le_bytes());
    Ok(())
}

fn build_executable(project_root: &Path) -> Result<PathBuf, String> {
    let output_stem = project_output_stem(project_root);

    #[cfg(windows)]
    let output_name = {
        let mut output_name = output_stem;
        if !output_name.to_ascii_lowercase().ends_with(".exe") {
            output_name.push_str(".exe");
        }
        output_name
    };
    #[cfg(not(windows))]
    let output_name = output_stem;

    let payload = build_payload(project_root)?;

    let current_exe = env::current_exe()
        .map_err(|e| format!("failed to resolve current executable path: {e}"))?;
    #[allow(unused_mut)]
    let mut engine_bytes = fs::read(&current_exe).map_err(|e| {
        format!(
            "failed to read engine executable {}: {e}",
            current_exe.display()
        )
    })?;

    // Compiled games should not pop up a console window; flip the copied image to
    // the GUI subsystem before embedding the payload.
    #[cfg(windows)]
    patch_subsystem_to_gui(&mut engine_bytes)
        .map_err(|e| format!("failed to prepare windowed game executable: {e}"))?;

    let output_dir = project_root.join("dist");
    fs::create_dir_all(&output_dir).map_err(|e| {
        format!(
            "failed to create dist directory {}: {e}",
            output_dir.display()
        )
    })?;
    let output_path = output_dir.join(output_name);

    let total_steps = 3usize;
    progress_bar(1, total_steps, "Copying engine executable");

    let mut out_file = File::create(&output_path).map_err(|e| {
        format!(
            "failed to create output executable {}: {e}",
            output_path.display()
        )
    })?;
    out_file
        .write_all(&engine_bytes)
        .map_err(|e| format!("failed to write engine bytes: {e}"))?;

    progress_bar(2, total_steps, "Embedding game payload");
    out_file
        .write_all(&payload)
        .map_err(|e| format!("failed to write payload: {e}"))?;
    out_file
        .write_all(&(payload.len() as u64).to_le_bytes())
        .map_err(|e| format!("failed to write payload length: {e}"))?;
    out_file
        .write_all(EMBED_TRAILER_MAGIC)
        .map_err(|e| format!("failed to write payload trailer magic: {e}"))?;
    out_file
        .flush()
        .map_err(|e| format!("failed to flush output file: {e}"))?;

    #[cfg(unix)]
    {
        let metadata = fs::metadata(&output_path)
            .map_err(|e| format!("failed to read output metadata: {e}"))?;
        let mut perms = metadata.permissions();
        let mode = perms.mode();
        perms.set_mode(mode | 0o111);
        fs::set_permissions(&output_path, perms)
            .map_err(|e| format!("failed to set executable permissions: {e}"))?;
    }

    progress_bar(3, total_steps, "Build complete");

    Ok(output_path)
}

fn engine_source_root() -> Result<PathBuf, String> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if root.join("Cargo.toml").is_file() {
        Ok(root)
    } else {
        Err(format!(
            "webasm build requires engine source files; expected Cargo.toml at {}",
            root.display()
        ))
    }
}

fn run_checked_command(
    command: &mut std::process::Command,
    description: &str,
) -> Result<(), String> {
    let rendered = format!("{command:?}");
    let status = command
        .status()
        .map_err(|e| format!("failed while {description}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{description} failed with status {status}: {rendered}"
        ))
    }
}

fn run_checked_command_quiet(
    command: &mut std::process::Command,
    description: &str,
) -> Result<(), String> {
    let rendered = format!("{command:?}");
    let output = command
        .output()
        .map_err(|e| format!("failed while {description}: {e}"))?;
    if output.status.success() {
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let diagnostics = [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    if diagnostics.is_empty() {
        Err(format!(
            "{description} failed with status {}: {rendered}",
            output.status
        ))
    } else {
        Err(format!(
            "{description} failed with status {}: {rendered}\n{diagnostics}",
            output.status
        ))
    }
}

fn emsdk_root() -> Result<PathBuf, String> {
    let home = user_home_dir().ok_or_else(|| "could not resolve home directory".to_string())?;
    Ok(home.join(".neolove").join("toolchains").join("emsdk"))
}

fn emsdk_command_path(root: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        root.join("emsdk.bat")
    }
    #[cfg(not(windows))]
    {
        root.join("emsdk")
    }
}

#[cfg(windows)]
fn prepare_windows_batch_command(program: &Path) -> std::process::Command {
    let mut command = std::process::Command::new("cmd");
    command.arg("/C").arg(program);
    command
}

fn emcc_path(root: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        root.join("upstream").join("emscripten").join("emcc.bat")
    }
    #[cfg(not(windows))]
    {
        root.join("upstream").join("emscripten").join("emcc")
    }
}

fn find_emsdk_node(root: &Path) -> Result<PathBuf, String> {
    let node_root = root.join("node");
    let entries = fs::read_dir(&node_root)
        .map_err(|e| format!("failed to read emsdk node directory {}: {e}", node_root.display()))?;

    let mut candidates = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("failed to read emsdk node entry: {e}"))?;
        let path = entry.path();
        #[cfg(windows)]
        let candidate = path.join("bin").join("node.exe");
        #[cfg(not(windows))]
        let candidate = path.join("bin").join("node");
        if candidate.is_file() {
            candidates.push(candidate);
        }
    }

    candidates.sort();
    candidates
        .into_iter()
        .next()
        .ok_or_else(|| "emsdk node runtime was not found after installation".to_string())
}

fn apply_emsdk_env(command: &mut std::process::Command, root: &Path) -> Result<(), String> {
    let emcc_dir = root.join("upstream").join("emscripten");
    let node_path = find_emsdk_node(root)?;
    let em_cache = std::env::temp_dir().join("neolove-emscripten-cache");
    fs::create_dir_all(&em_cache).map_err(|e| {
        format!(
            "failed to create writable emscripten cache directory {}: {e}",
            em_cache.display()
        )
    })?;

    let mut paths = vec![root.to_path_buf(), emcc_dir];
    if let Some(existing) = env::var_os("PATH") {
        paths.extend(env::split_paths(&existing));
    }
    let joined = env::join_paths(paths)
        .map_err(|e| format!("failed to construct PATH for emsdk: {e}"))?;

    command.env("EMSDK", root);
    command.env("EMSDK_NODE", node_path);
    command.env("EM_CACHE", em_cache);
    command.env("PATH", joined);
    Ok(())
}

fn ensure_emsdk() -> Result<PathBuf, String> {
    let root = emsdk_root()?;
    let emcc = emcc_path(&root);
    if emcc.is_file() {
        return Ok(root);
    }

    if let Some(parent) = root.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "failed to create emsdk parent directory {}: {e}",
                parent.display()
            )
        })?;
    }

    if root.exists() {
        fs::remove_dir_all(&root)
            .map_err(|e| format!("failed to clean incomplete emsdk install {}: {e}", root.display()))?;
    }

    let mut git = std::process::Command::new("git");
    git.arg("clone")
        .arg("--depth")
        .arg("1")
        .arg("https://github.com/emscripten-core/emsdk.git")
        .arg(&root);
    run_checked_command(&mut git, "cloning emsdk")?;

    let emsdk = emsdk_command_path(&root);
    #[cfg(windows)]
    let mut install = prepare_windows_batch_command(&emsdk);
    #[cfg(not(windows))]
    let mut install = std::process::Command::new(&emsdk);
    install.arg("install").arg("latest");
    run_checked_command(&mut install, "installing emsdk")?;

    #[cfg(windows)]
    let mut activate = prepare_windows_batch_command(&emsdk);
    #[cfg(not(windows))]
    let mut activate = std::process::Command::new(&emsdk);
    activate.arg("activate").arg("latest");
    run_checked_command(&mut activate, "activating emsdk")?;

    if !emcc.is_file() {
        return Err(format!(
            "emsdk installation completed, but emcc was not found at {}",
            emcc.display()
        ));
    }

    Ok(root)
}

fn recreate_dir(path: &Path) -> Result<(), String> {
    if path.exists() {
        fs::remove_dir_all(path)
            .map_err(|e| format!("failed to clear directory {}: {e}", path.display()))?;
    }
    fs::create_dir_all(path)
        .map_err(|e| format!("failed to create directory {}: {e}", path.display()))
}

fn stage_web_project(project_root: &Path, stage_dir: &Path) -> Result<(), String> {
    recreate_dir(stage_dir)?;

    let mut files = Vec::new();
    collect_project_files(project_root, project_root, &mut files)?;
    files.sort();

    for source in files {
        let relative = source
            .strip_prefix(project_root)
            .map_err(|e| format!("failed to strip staged project prefix: {e}"))?;
        let destination = stage_dir.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create staged directory {}: {e}", parent.display()))?;
        }
        fs::copy(&source, &destination).map_err(|e| {
            format!(
                "failed to stage webasm project file {} -> {}: {e}",
                source.display(),
                destination.display()
            )
        })?;
    }

    Ok(())
}

fn collect_bundle_files(root: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(root)
        .map_err(|e| format!("failed to read bundle directory {}: {e}", root.display()))?;
    let mut children = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("failed to read bundle directory entry: {e}"))?;
        children.push(entry.path());
    }
    children.sort();

    for child in children {
        let file_type = fs::metadata(&child)
            .map_err(|e| format!("failed to stat {}: {e}", child.display()))?;
        if file_type.is_dir() {
            collect_bundle_files(&child, out)?;
        } else if file_type.is_file() {
            out.push(child);
        }
    }
    Ok(())
}

fn create_webasm_zip(bundle_dir: &Path, zip_path: &Path) -> Result<(), String> {
    let file = File::create(zip_path)
        .map_err(|e| format!("failed to create webasm package {}: {e}", zip_path.display()))?;
    let mut archive = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o644);

    let mut files = Vec::new();
    collect_bundle_files(bundle_dir, &mut files)?;

    for path in files {
        let relative = path
            .strip_prefix(bundle_dir)
            .map_err(|e| format!("failed to strip bundle prefix: {e}"))?
            .to_string_lossy()
            .replace('\\', "/");

        archive
            .start_file(&relative, options)
            .map_err(|e| format!("failed to add {} to webasm package: {e}", relative))?;

        let mut source = File::open(&path)
            .map_err(|e| format!("failed to open bundle file {}: {e}", path.display()))?;
        std::io::copy(&mut source, &mut archive).map_err(|e| {
            format!(
                "failed to write bundle file {} into {}: {e}",
                path.display(),
                zip_path.display()
            )
        })?;
    }

    archive
        .finish()
        .map_err(|e| format!("failed to finalize webasm package {}: {e}", zip_path.display()))?;

    Ok(())
}

fn webasm_index_html(project_root: &Path) -> String {
    let settings = parse_project_settings(project_root);
    let title = settings
        .window_title
        .or(settings.package_name)
        .unwrap_or_else(|| project_output_stem(project_root));

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{title}</title>
  <link rel="icon" href="data:,">
  <style>
    :root {{
      color-scheme: dark;
      --bg: #0b0b0b;
      --track: #242424;
      --fill: #f2f2f2;
      --text: #f5f5f5;
      --muted: #9a9a9a;
      --danger: #ff8a8a;
    }}
    html, body {{
      margin: 0;
      width: 100%;
      height: 100%;
      overflow: hidden;
      background: var(--bg);
      color: var(--text);
      font: 400 14px/1.4 -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif;
    }}
    body {{
      position: relative;
    }}
    .shell {{
      position: fixed;
      inset: 0;
    }}
    canvas {{
      position: absolute;
      inset: 0;
      width: 100%;
      height: 100%;
      display: block;
      image-rendering: pixelated;
      image-rendering: crisp-edges;
      background: transparent;
    }}
    .overlay {{
      position: absolute;
      inset: 0;
      display: grid;
      place-items: center;
      transition:
        opacity 360ms ease,
        visibility 360ms ease;
      pointer-events: none;
    }}
    .overlay[data-state="ready"] {{
      opacity: 0;
      visibility: hidden;
    }}
    .panel {{
      width: min(320px, calc(100vw - 48px));
    }}
    h1 {{
      margin: 0 0 18px;
      text-align: center;
      font-size: clamp(20px, 3vw, 24px);
      line-height: 1.1;
      font-weight: 500;
    }}
    .meter {{
      position: relative;
      height: 4px;
      margin: 0;
      border-radius: 999px;
      overflow: hidden;
      background: var(--track);
    }}
    .meter-fill {{
      width: calc(var(--progress, 0) * 1%);
      height: 100%;
      border-radius: inherit;
      background: var(--fill);
      transition: width 220ms ease;
    }}
    .status {{
      display: none;
    }}
    .detail {{
      margin: 12px 0 0;
      min-height: 1.4em;
      color: var(--muted);
      text-align: center;
      font-size: 12px;
      white-space: pre-wrap;
    }}
    .overlay[data-state="error"] .detail,
    .overlay[data-state="file"] .detail,
    .overlay[data-state="error"] .hint,
    .overlay[data-state="file"] .hint {{
      color: var(--danger);
    }}
    .hint {{
      margin: 12px 0 0;
      color: var(--muted);
      text-align: center;
      font-size: 12px;
      line-height: 1.5;
    }}
    .hint code {{
      display: inline-block;
      margin-top: 8px;
      padding: 0;
      background: transparent;
      color: var(--text);
      font: 400 12px/1.4 ui-monospace, "SFMono-Regular", "Cascadia Code", "Source Code Pro", Consolas, monospace;
    }}
  </style>
</head>
<body>
  <div class="shell">
    <canvas id="canvas"></canvas>
    <div class="overlay" id="overlay" data-state="loading" style="--progress: 8">
      <section class="panel" aria-live="polite">
        <h1>Loading</h1>
        <p class="status" id="status">Starting loader...</p>
        <div class="meter" aria-hidden="true">
          <div class="meter-fill"></div>
        </div>
        <p class="detail" id="detail"></p>
        <p class="hint" id="hint" hidden></p>
      </section>
    </div>
  </div>
  <script>
    (() => {{
      const overlay = document.getElementById("overlay");
      const status = document.getElementById("status");
      const detail = document.getElementById("detail");
      const hint = document.getElementById("hint");

      function clampProgress(value) {{
        return Math.max(0, Math.min(100, Math.round(value)));
      }}

      function setOverlayState(nextState) {{
        if (overlay) {{
          overlay.dataset.state = nextState;
        }}
      }}

      function setProgress(value) {{
        if (!overlay) {{
          return;
        }}
        const safe = clampProgress(value);
        overlay.style.setProperty("--progress", String(safe));
      }}

      function setMessage(primary, secondary, state) {{
        if (state) {{
          setOverlayState(state);
        }}
        if (status) {{
          status.textContent = primary;
        }}
        if (detail) {{
          detail.textContent = secondary || "";
        }}
      }}

      function setHint(html) {{
        if (!hint) {{
          return;
        }}
        if (!html) {{
          hint.hidden = true;
          hint.innerHTML = "";
          return;
        }}
        hint.hidden = false;
        hint.innerHTML = html;
      }}

      function bindVisibleStatusTarget() {{
        if (!window.Module || !Module.neoloveState || !detail) {{
          return false;
        }}
        Module.neoloveState.overlayEl = overlay;
        Module.neoloveState.detailEl = detail;
        Module.neoloveState.statusEl = detail;
        return true;
      }}

      if (detail && overlay) {{
        const syncVisibleState = () => {{
          const state = detail.dataset.state;
          if (!state) {{
            return;
          }}
          overlay.dataset.state = state;
        }};
        new MutationObserver(syncVisibleState).observe(detail, {{
          attributes: true,
          attributeFilter: ["data-state"]
        }});
      }}

      function updateFromStatus(text) {{
        const message = String(text || "").trim();
        if (!message) {{
          setOverlayState("ready");
          setProgress(100);
          setHint("");
          return;
        }}

        const progressMatch = message.match(/\((\d+)\s*\/\s*(\d+)\)/);
        if (progressMatch) {{
          const loaded = Number(progressMatch[1]);
          const total = Number(progressMatch[2]);
          if (Number.isFinite(loaded) && Number.isFinite(total) && total > 0) {{
            const ratio = (loaded / total) * 100;
            setProgress(Math.max(12, ratio));
            setMessage("Streaming project data", "", "loading");
            return;
          }}
        }}

        if (message.includes("Running")) {{
          setProgress(96);
          setMessage("Launching game", "", "loading");
          return;
        }}

        if (message.includes("Loading")) {{
          setProgress(18);
          setMessage("Loading runtime", "", "loading");
          return;
        }}

        setMessage(message, "", "info");
      }}

      setMessage("Starting loader...", "", "loading");
      setProgress(8);

      if (window.location.protocol === "file:") {{
        setOverlayState("file");
        setProgress(100);
        setMessage(
          "This build cannot run from `file://`.",
          "Browsers block `neolove.wasm` and `neolove.data` when the page is opened directly from disk."
        );
        setHint("Serve this folder over HTTP, for example:<br><code>cd dist/webasm && python3 -m http.server 8000</code><br>Then open <code>http://localhost:8000</code>.");
        return;
      }}

      function isDevToolsShortcut(event) {{
        const key = typeof event.key === "string" ? event.key.toLowerCase() : "";
        if (key === "f12") {{
          return true;
        }}
        const primaryModifier = event.ctrlKey || event.metaKey;
        const secondaryModifier = event.shiftKey || event.altKey;
        if (!primaryModifier || !secondaryModifier) {{
          return false;
        }}
        return key === "i" || key === "j" || key === "c";
      }}

      window.addEventListener("keydown", (event) => {{
        if (!isDevToolsShortcut(event)) {{
          return;
        }}
        event.stopImmediatePropagation();
      }}, {{ capture: true }});

      window.addEventListener("keyup", (event) => {{
        if (!isDevToolsShortcut(event)) {{
          return;
        }}
        event.stopImmediatePropagation();
      }}, {{ capture: true }});

      document.addEventListener("mousedown", (event) => {{
        if (!(event.target instanceof HTMLCanvasElement)) {{
          return;
        }}
        if (event.button !== 2 || !event.shiftKey) {{
          return;
        }}
        event.stopImmediatePropagation();
      }}, {{ capture: true }});

      document.addEventListener("contextmenu", (event) => {{
        if (!(event.target instanceof HTMLCanvasElement)) {{
          return;
        }}
        if (!event.shiftKey) {{
          return;
        }}
        event.stopImmediatePropagation();
      }}, {{ capture: true }});

      window.addEventListener("error", (event) => {{
        console.warn("[NeoLOVE debug] window error", event.message || event.error || event);
      }});

      window.addEventListener("unhandledrejection", (event) => {{
        console.warn("[NeoLOVE debug] unhandled rejection", event.reason || event);
      }});

      window.Module = {{
        locateFile(path) {{
          return path;
        }},
        monitorRunDependencies(count) {{
          console.warn("[NeoLOVE debug] run dependencies", count);
        }},
        onRuntimeInitialized() {{
          console.warn("[NeoLOVE debug] runtime initialized");
        }},
        setStatus(text) {{
          updateFromStatus(text);
        }},
        print(text) {{
          console.log(text);
        }},
        printErr(text) {{
          const message = String(text);
          console.error(message);
          setOverlayState("error");
          setProgress(100);
          setMessage("Load failed", message, "error");
        }}
      }};

      const script = document.createElement("script");
      script.src = "neolove.js";
      script.async = true;
      script.onload = () => {{
        console.warn("[NeoLOVE debug] neolove.js loaded");
        if (bindVisibleStatusTarget()) {{
          return;
        }}
        const timer = window.setInterval(() => {{
          if (!bindVisibleStatusTarget()) {{
            return;
          }}
          window.clearInterval(timer);
        }}, 50);
      }};
      script.onerror = () => {{
        setOverlayState("error");
        setProgress(100);
        setMessage("Failed to load `neolove.js`.", "Check that the bundle files are being served from the same folder.", "error");
      }};
      document.body.appendChild(script);
    }})();
  </script>
</body>
</html>
"#
    )
}

fn build_webasm(project_root: &Path) -> Result<(PathBuf, PathBuf), String> {
    let output_stem = project_output_stem(project_root);
    let output_dir = project_root.join("dist");
    fs::create_dir_all(&output_dir).map_err(|e| {
        format!(
            "failed to create dist directory {}: {e}",
            output_dir.display()
        )
    })?;

    let bundle_dir = output_dir.join("webasm");
    recreate_dir(&bundle_dir)?;

    let stage_dir = output_dir.join(".webasm-stage");
    stage_web_project(project_root, &stage_dir)?;
    let staged_project = fs::canonicalize(&stage_dir)
        .map_err(|e| format!("failed to resolve staged webasm project {}: {e}", stage_dir.display()))?;

    println!("Ensuring emsdk is installed...");
    let emsdk = ensure_emsdk()?;

    println!("Ensuring wasm32-unknown-emscripten target is installed...");
    let mut rustup = std::process::Command::new("rustup");
    rustup.args(["target", "add", "wasm32-unknown-emscripten"]);
    run_checked_command(&mut rustup, "installing wasm32-unknown-emscripten target")?;

    let engine_root = engine_source_root()?;
    let cargo_target_dir = engine_root.join("target").join("webasm-emscripten-legacy-eh");
    println!("Building NeoLOVE webasm runtime...");
    let mut cargo = std::process::Command::new("cargo");
    apply_emsdk_env(&mut cargo, &emsdk)?;
    cargo.env("CXXFLAGS", "-fwasm-exceptions");
    cargo.env("CARGO_TARGET_DIR", &cargo_target_dir);
    cargo
        .arg("rustc")
        .arg("--release")
        .arg("--target")
        .arg("wasm32-unknown-emscripten")
        .arg("--bin")
        .arg(env!("CARGO_PKG_NAME"))
        .arg("--")
        .arg("-C")
        .arg("link-arg=--preload-file")
        .arg("-C")
        .arg(format!(
            "link-arg={}@/project",
            staged_project.to_string_lossy()
        ))
        .arg("-C")
        .arg("link-arg=-sFORCE_FILESYSTEM=1")
        .arg("-C")
        .arg("link-arg=-sALLOW_MEMORY_GROWTH=1")
        .current_dir(&engine_root);
    run_checked_command_quiet(&mut cargo, "building webasm runtime")?;

    let target_dir = cargo_target_dir
        .join("wasm32-unknown-emscripten")
        .join("release");
    let built_js = target_dir.join(format!("{}.js", env!("CARGO_PKG_NAME")));
    let built_wasm = target_dir.join(format!("{}.wasm", env!("CARGO_PKG_NAME")));
    let mut artifacts = vec![built_js, built_wasm];
    let built_data_candidates = [
        target_dir.join(format!("{}.data", env!("CARGO_PKG_NAME"))),
        target_dir
            .join("deps")
            .join(format!("{}.data", env!("CARGO_PKG_NAME"))),
    ];

    for artifact in &artifacts {
        if !artifact.is_file() {
            return Err(format!(
                "webasm build succeeded but expected output was not found: {}",
                artifact.display()
            ));
        }
    }

    if let Some(data_file) = built_data_candidates.iter().find(|path| path.is_file()) {
        artifacts.push(data_file.clone());
    }

    for artifact in &artifacts {
        let file_name = artifact
            .file_name()
            .ok_or_else(|| format!("failed to resolve artifact file name for {}", artifact.display()))?;
        let destination = bundle_dir.join(file_name);
        fs::copy(artifact, &destination).map_err(|e| {
            format!(
                "failed to copy webasm artifact {} -> {}: {e}",
                artifact.display(),
                destination.display()
            )
        })?;
    }

    fs::write(bundle_dir.join("index.html"), webasm_index_html(project_root)).map_err(|e| {
        format!(
            "failed to write webasm loader {}: {e}",
            bundle_dir.join("index.html").display()
        )
    })?;

    if stage_dir.exists() {
        fs::remove_dir_all(&stage_dir)
            .map_err(|e| format!("failed to clean staged webasm files {}: {e}", stage_dir.display()))?;
    }

    let zip_output = output_dir.join(format!("{output_stem}-webasm.zip"));
    create_webasm_zip(&bundle_dir, &zip_output)?;

    Ok((bundle_dir, zip_output))
}

fn virtual_key_name(key: VirtualKeyCode) -> Option<&'static str> {
    Some(match key {
        VirtualKeyCode::A => "a",
        VirtualKeyCode::B => "b",
        VirtualKeyCode::C => "c",
        VirtualKeyCode::D => "d",
        VirtualKeyCode::E => "e",
        VirtualKeyCode::F => "f",
        VirtualKeyCode::G => "g",
        VirtualKeyCode::H => "h",
        VirtualKeyCode::I => "i",
        VirtualKeyCode::J => "j",
        VirtualKeyCode::K => "k",
        VirtualKeyCode::L => "l",
        VirtualKeyCode::M => "m",
        VirtualKeyCode::N => "n",
        VirtualKeyCode::O => "o",
        VirtualKeyCode::P => "p",
        VirtualKeyCode::Q => "q",
        VirtualKeyCode::R => "r",
        VirtualKeyCode::S => "s",
        VirtualKeyCode::T => "t",
        VirtualKeyCode::U => "u",
        VirtualKeyCode::V => "v",
        VirtualKeyCode::W => "w",
        VirtualKeyCode::X => "x",
        VirtualKeyCode::Y => "y",
        VirtualKeyCode::Z => "z",
        VirtualKeyCode::Key0 => "0",
        VirtualKeyCode::Key1 => "1",
        VirtualKeyCode::Key2 => "2",
        VirtualKeyCode::Key3 => "3",
        VirtualKeyCode::Key4 => "4",
        VirtualKeyCode::Key5 => "5",
        VirtualKeyCode::Key6 => "6",
        VirtualKeyCode::Key7 => "7",
        VirtualKeyCode::Key8 => "8",
        VirtualKeyCode::Key9 => "9",
        VirtualKeyCode::Space => "space",
        VirtualKeyCode::Escape => "escape",
        VirtualKeyCode::Return => "enter",
        VirtualKeyCode::Tab => "tab",
        VirtualKeyCode::Back => "backspace",
        VirtualKeyCode::Left => "left",
        VirtualKeyCode::Right => "right",
        VirtualKeyCode::Up => "up",
        VirtualKeyCode::Down => "down",
        VirtualKeyCode::LShift => "leftshift",
        VirtualKeyCode::RShift => "rightshift",
        VirtualKeyCode::LControl => "leftcontrol",
        VirtualKeyCode::RControl => "rightcontrol",
        VirtualKeyCode::LAlt => "leftalt",
        VirtualKeyCode::RAlt => "rightalt",
        VirtualKeyCode::LWin => "leftsuper",
        VirtualKeyCode::RWin => "rightsuper",
        VirtualKeyCode::F1 => "f1",
        VirtualKeyCode::F2 => "f2",
        VirtualKeyCode::F3 => "f3",
        VirtualKeyCode::F4 => "f4",
        VirtualKeyCode::F5 => "f5",
        VirtualKeyCode::F6 => "f6",
        VirtualKeyCode::F7 => "f7",
        VirtualKeyCode::F8 => "f8",
        VirtualKeyCode::F9 => "f9",
        VirtualKeyCode::F10 => "f10",
        VirtualKeyCode::F11 => "f11",
        VirtualKeyCode::F12 => "f12",
        _ => return None,
    })
}

fn mouse_button_name(button: MouseButton) -> &'static str {
    match button {
        MouseButton::Left => "left",
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
        MouseButton::Other(_) => "other",
    }
}

fn normalize_mouse_wheel_delta(delta: MouseScrollDelta) -> (f32, f32) {
    const PIXELS_PER_LINE: f32 = 40.0;

    match delta {
        MouseScrollDelta::LineDelta(x, y) => (x, y),
        MouseScrollDelta::PixelDelta(pos) => (
            pos.x as f32 / PIXELS_PER_LINE,
            pos.y as f32 / PIXELS_PER_LINE,
        ),
    }
}

fn with_platform_state<R>(
    platform_state: &SharedPlatformState,
    _context: &str,
    f: impl FnOnce(&mut crate::platform::PlatformState) -> R,
) -> Result<R, String> {
    let mut platform = crate::platform::lock_platform_state(platform_state);
    Ok(f(&mut platform))
}

/// Set once we launch a game compiled with `neolove build`. Such games run
/// without an attached terminal on Windows (the executable is linked against the
/// GUI subsystem), so fatal errors have to surface through a native dialog
/// instead of stderr.
#[cfg(windows)]
static GAME_ERROR_DIALOGS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn enable_native_error_dialogs() {
    #[cfg(windows)]
    GAME_ERROR_DIALOGS.store(true, std::sync::atomic::Ordering::Relaxed);
}

fn native_error_dialogs_enabled() -> bool {
    #[cfg(windows)]
    {
        GAME_ERROR_DIALOGS.load(std::sync::atomic::Ordering::Relaxed)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Show `message` in a native OS error dialog. Only does anything on Windows,
/// where a compiled game has no console to print to.
#[cfg(windows)]
fn show_native_error(title: &str, message: &str) {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(
            hwnd: *mut std::ffi::c_void,
            text: *const u16,
            caption: *const u16,
            utype: u32,
        ) -> i32;
    }

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    const MB_OK: u32 = 0x0000_0000;
    const MB_ICONERROR: u32 = 0x0000_0010;

    let caption = title.trim_end_matches(':').trim();
    let caption = if caption.is_empty() { "NeoLOVE" } else { caption };
    let caption = to_wide(caption);
    let body = to_wide(message);

    // SAFETY: both pointers reference NUL-terminated UTF-16 buffers that outlive
    // the modal call, and a null owner HWND is valid for a top-level dialog.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(not(windows))]
fn show_native_error(_title: &str, _message: &str) {}

fn report_runtime_failure(title: &str, message: &str) {
    eprintln!("\x1b[31m{title}\x1b[0m\n{message}");
    if native_error_dialogs_enabled() {
        show_native_error(title, message);
    }
}

fn exit_runtime_failure(_control_flow: &mut ControlFlow, title: &str, message: &str) -> ! {
    report_runtime_failure(title, message);
    // EventLoop::run terminates with status 0 when ControlFlow::Exit is used,
    // which makes the editor treat a fatal runtime error as a normal close and
    // discard stderr. A fatal frame/render error must be observable by the
    // parent process.
    std::process::exit(1);
}

fn desktop_panic_hint(message: &str) -> Option<&'static str> {
    if message.contains("Failed to initialize any backend!")
        || message.contains("NoCompositorListening")
        || message.contains("XOpenDisplayFailed")
    {
        return Some(
            "NeoLOVE could not connect to a graphical desktop session. Start it from an X11 or Wayland session, and if you are inside a sandbox make sure DISPLAY or WAYLAND_DISPLAY and the matching socket are exposed.",
        );
    }

    None
}

fn describe_desktop_panic(context: &str, payload: &(dyn std::any::Any + Send)) -> String {
    let panic_message = lua_error::describe_panic(payload);
    let mut rendered = format!("{context}\nPanic: {panic_message}");
    if let Some(hint) = desktop_panic_hint(&panic_message) {
        rendered.push_str("\nHint: ");
        rendered.push_str(hint);
    }
    rendered
}

fn install_desktop_panic_hook() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            let thread = std::thread::current();
            let thread_name = thread.name().unwrap_or("unnamed");
            let location = info
                .location()
                .map(|location| {
                    format!(
                        "{}:{}:{}",
                        location.file(),
                        location.line(),
                        location.column()
                    )
                })
                .unwrap_or_else(|| "unknown location".to_string());
            let payload = if let Some(message) = info.payload().downcast_ref::<&str>() {
                (*message).to_string()
            } else if let Some(message) = info.payload().downcast_ref::<String>() {
                message.clone()
            } else {
                "non-string panic payload".to_string()
            };
            let message = format!(
                "NeoLOVE internal panic on thread '{thread_name}' at {location}:\n{payload}\nBacktrace:\n{}",
                std::backtrace::Backtrace::force_capture()
            );
            // A panic hook must not cause a second panic if stderr has already
            // been closed by a launcher or pipe consumer.
            let _ = writeln!(std::io::stderr().lock(), "{message}");
        }));
    });
}

fn catch_desktop_panic<T>(context: &str, f: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .map_err(|payload| describe_desktop_panic(context, payload.as_ref()))
}

fn run_project_window(project_root: PathBuf, data_root: Option<PathBuf>) -> Result<(), String> {
    env::set_current_dir(&project_root).map_err(|error| {
        format!(
            "failed to set current directory to {}: {error}",
            project_root.display()
        )
    })?;
    let (title, icon) = window_options_for_project(&project_root);
    let mut runtime = match data_root {
        Some(data_root) => window::Runtime::with_data_root(project_root, data_root),
        None => window::Runtime::new(project_root),
    };
    runtime.set_platform_window_state(DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT);

    // When launched by the editor, stream logs and live scene snapshots back to
    // its logger window over loopback IPC. Absent the env var this is a no-op.
    let ipc_client = env::var("NEOLOVE_EDITOR_IPC")
        .ok()
        .and_then(|addr| editor_ipc::IpcClient::connect(&addr));
    let (log_tx, log_rx) = std::sync::mpsc::channel();
    if ipc_client.is_some() {
        runtime.set_log_sink(log_tx);
    }

    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.start())) {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            return Err(format!(
                "failed to start runtime:\n{}",
                lua_error::describe_lua_error(&error)
            ));
        }
        Err(payload) => {
            return Err(format!(
                "runtime panicked during startup\nPanic: {}",
                lua_error::describe_panic(payload.as_ref())
            ));
        }
    }

    let event_loop =
        catch_desktop_panic("failed to initialize the window event loop", EventLoop::new)?;
    let mut builder = WindowBuilder::new()
        .with_title(title)
        .with_inner_size(LogicalSize::new(
            DEFAULT_WINDOW_WIDTH as f64,
            DEFAULT_WINDOW_HEIGHT as f64,
        ));
    if let Some(icon) = icon {
        builder = builder.with_window_icon(Some(icon));
    }
    let window = builder
        .build(&event_loop)
        .map(std::sync::Arc::new)
        .map_err(|error| format!("failed to create window: {error}"))?;
    let size = window.inner_size();
    runtime.set_platform_window_state(size.width as f32, size.height as f32);

    let platform_state = runtime.platform_state();
    let render_state = runtime.render_state();
    let mut presenter = DesktopPresenter::new(&event_loop, &window)?;

    let mut last_update = Instant::now();
    let mut last_snapshot = Instant::now();
    let mut cursor_grab_warning_logged = false;
    event_loop.run(move |event, _target, control_flow| {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            *control_flow = ControlFlow::Poll;

            match event {
                Event::WindowEvent { event, .. } => match event {
                    WindowEvent::CloseRequested => *control_flow = ControlFlow::Exit,
                    WindowEvent::Resized(size) => {
                        runtime.set_platform_window_state(size.width as f32, size.height as f32);
                        presenter.request_resize();
                    }
                    WindowEvent::CursorMoved { position, .. } => {
                        runtime.set_platform_mouse_state(position.x as f32, position.y as f32);
                    }
                    WindowEvent::MouseInput { state, button, .. } => {
                        if let Err(error) = with_platform_state(
                            &platform_state,
                            "updating mouse button state",
                            |platform| {
                                let name = mouse_button_name(button).to_string();
                                match state {
                                    ElementState::Pressed => {
                                        if platform.input_mut().mouse_down.insert(name.clone()) {
                                            platform.input_mut().mouse_pressed.insert(name);
                                        }
                                    }
                                    ElementState::Released => {
                                        platform.input_mut().mouse_down.remove(name.as_str());
                                        platform.input_mut().mouse_released.insert(name);
                                    }
                                }
                            },
                        ) {
                            exit_runtime_failure(control_flow, "Fatal Runtime Error:", &error);
                        }
                    }
                    WindowEvent::MouseWheel { delta, .. } => {
                        if let Err(error) = with_platform_state(
                            &platform_state,
                            "updating mouse wheel state",
                            |platform| {
                                let (x, y) = normalize_mouse_wheel_delta(delta);
                                platform.input_mut().wheel_x += x;
                                platform.input_mut().wheel_y += y;
                            },
                        ) {
                            exit_runtime_failure(control_flow, "Fatal Runtime Error:", &error);
                        }
                    }
                    WindowEvent::ReceivedCharacter(ch) => {
                        if !ch.is_control() {
                            if let Err(error) = with_platform_state(
                                &platform_state,
                                "recording text input",
                                |platform| {
                                    platform.input_mut().char_pressed = Some(ch.to_string());
                                },
                            ) {
                                exit_runtime_failure(control_flow, "Fatal Runtime Error:", &error);
                            }
                        }
                    }
                    WindowEvent::KeyboardInput {
                        input:
                            KeyboardInput {
                                virtual_keycode: Some(key),
                                state,
                                ..
                            },
                        ..
                    } => {
                        if let Some(name) = virtual_key_name(key) {
                            if let Err(error) = with_platform_state(
                                &platform_state,
                                "updating keyboard state",
                                |platform| {
                                    let name = name.to_string();
                                    match state {
                                        ElementState::Pressed => {
                                            if platform.input_mut().keys_down.insert(name.clone()) {
                                                platform.input_mut().keys_pressed.insert(name.clone());
                                            }
                                            platform.input_mut().last_key_pressed = Some(name);
                                        }
                                        ElementState::Released => {
                                            platform.input_mut().keys_down.remove(name.as_str());
                                            platform.input_mut().keys_released.insert(name);
                                        }
                                    }
                                },
                            ) {
                                exit_runtime_failure(control_flow, "Fatal Runtime Error:", &error);
                            }
                        }
                    }
                    _ => {}
                },
                Event::MainEventsCleared => {
                    let update_start = Instant::now();
                    let dt = update_start.duration_since(last_update).as_secs_f32();
                    last_update = update_start;

                    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.update(dt)))
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            exit_runtime_failure(control_flow, "Fatal Runtime Error:", &error);
                        }
                        Err(payload) => {
                            exit_runtime_failure(
                                control_flow,
                                "Rust Panic:",
                                &format!(
                                    "Runtime panicked during frame update\nPanic: {}",
                                    lua_error::describe_panic(payload.as_ref())
                                ),
                            );
                        }
                    }

                    // Stream output and a throttled live snapshot to the editor.
                    if let Some(ipc) = ipc_client.as_ref() {
                        while let Ok(line) = log_rx.try_recv() {
                            ipc.send(&editor_ipc::IpcMessage::Log(line));
                        }
                        let now = Instant::now();
                        if now.duration_since(last_snapshot) >= Duration::from_millis(100) {
                            last_snapshot = now;
                            ipc.send(&editor_ipc::IpcMessage::Scene {
                                entities: runtime.snapshot_entities(),
                            });
                        }
                    }

                    if runtime.exit_requested() {
                        *control_flow = ControlFlow::Exit;
                        return;
                    }

                    if let Err(error) = with_platform_state(
                        &platform_state,
                        "finalizing frame input state",
                        |platform| {
                            let mouse_locked = platform.input().mouse_locked;
                            let grab_mode = if mouse_locked {
                                CursorGrabMode::Locked
                            } else {
                                CursorGrabMode::None
                            };
                            if let Err(error) = window.set_cursor_grab(grab_mode) {
                                if !cursor_grab_warning_logged {
                                    let action = if mouse_locked { "lock" } else { "release" };
                                    eprintln!("cursor grab warning: failed to {action} cursor: {error}");
                                    cursor_grab_warning_logged = true;
                                }
                            } else {
                                cursor_grab_warning_logged = false;
                            }
                            window.set_cursor_visible(!mouse_locked);
                            platform.begin_frame();
                        },
                    ) {
                        exit_runtime_failure(control_flow, "Fatal Runtime Error:", &error);
                    }

                    if let Some(max_fps) = runtime.max_fps() {
                        let target = Duration::from_secs_f32(1.0 / max_fps.max(1.0));
                        let elapsed = update_start.elapsed();
                        if elapsed < target {
                            std::thread::sleep(target - elapsed);
                        }
                    }

                    window.request_redraw();
                }
                Event::RedrawRequested(_) => {
                    if let Err(error) = presenter.render(&window, &platform_state, &render_state) {
                        exit_runtime_failure(
                            control_flow,
                            "Fatal Render Error:",
                            &format!("desktop presenter failed: {error}"),
                        );
                    }
                }
                _ => {}
            }
        }));

        if let Err(payload) = result {
            exit_runtime_failure(
                control_flow,
                "Rust Panic:",
                &describe_desktop_panic("runtime panicked while processing window events", payload.as_ref()),
            );
        }
    });
}

fn handle_new_command(project_name: &str) -> Result<PathBuf, String> {
    let project_path = resolve_from_cwd(project_name)
        .map_err(|error| format!("failed to resolve project path '{project_name}': {error}"))?;
    fs::create_dir(&project_path).map_err(|error| {
        format!(
            "failed to create project directory {}: {error}",
            project_path.display()
        )
    })?;

    let toml_path = project_path.join("neolove.toml");
    let contents = format!(
        "\
[package]
name = \"{}\"
version = \"0.1.0\"

[window]
title = \"{}\"
icon = \"assets/icon.png\"

[dependencies]
",
        project_name, project_name
    );
    fs::write(&toml_path, contents)
        .map_err(|error| format!("failed to write {}: {error}", toml_path.display()))?;

    let entry_path = project_path.join("main.luau");
    fs::write(&entry_path, format!("print(\"Hello, {}!\")", project_name))
        .map_err(|error| format!("failed to write {}: {error}", entry_path.display()))?;

    let assets_path = project_path.join("assets");
    fs::create_dir(&assets_path)
        .map_err(|error| format!("failed to create {}: {error}", assets_path.display()))?;

    let luaurc_path = project_path.join(".luaurc");
    fs::write(&luaurc_path, TEMPLATE_LUAURC)
        .map_err(|error| format!("failed to write {}: {error}", luaurc_path.display()))?;

    let vscode_dir = project_path.join(".vscode");
    fs::create_dir_all(&vscode_dir)
        .map_err(|error| format!("failed to create {}: {error}", vscode_dir.display()))?;
    let vscode_settings = vscode_dir.join("settings.json");
    fs::write(&vscode_settings, TEMPLATE_VSCODE_SETTINGS)
        .map_err(|error| format!("failed to write {}: {error}", vscode_settings.display()))?;

    let types_dir = project_path.join("types");
    fs::create_dir_all(&types_dir)
        .map_err(|error| format!("failed to create {}: {error}", types_dir.display()))?;
    let api_path = types_dir.join("neolove_engine_api.d.luau");
    fs::write(&api_path, TEMPLATE_NEOLOVE_ENGINE_API)
        .map_err(|error| format!("failed to write {}: {error}", api_path.display()))?;

    Ok(project_path)
}

fn handle_api_command(project_dir: Option<&str>) -> Result<Vec<PathBuf>, String> {
    let project_root = resolve_target_project_root(project_dir)?;
    if !project_root.exists() || !project_root.is_dir() {
        return Err(format!(
            "project path is not a valid directory: {}",
            project_root.display()
        ));
    }

    let types_dir = project_root.join("types");
    fs::create_dir_all(&types_dir)
        .map_err(|error| format!("failed to create {}: {error}", types_dir.display()))?;

    let api_path = types_dir.join("neolove_engine_api.d.luau");
    fs::write(&api_path, TEMPLATE_NEOLOVE_ENGINE_API)
        .map_err(|error| format!("failed to write {}: {error}", api_path.display()))?;

    let root_api_path = project_root.join("neolove_engine_api.d.luau");
    if root_api_path.exists() {
        fs::write(&root_api_path, TEMPLATE_NEOLOVE_ENGINE_API)
            .map_err(|error| format!("failed to write {}: {error}", root_api_path.display()))?;
        Ok(vec![api_path, root_api_path])
    } else {
        Ok(vec![api_path])
    }
}

fn print_usage() {
    println!("NeoLOVE CLI");
    println!("Usage:");
    println!("  neolove new <project-name>");
    println!("  neolove run [project-dir]");
    println!("  neolove editor [project-dir]");
    println!("  neolove build [project-dir] [--webasm]");
    println!("  neolove api [project-dir]");
    println!("  neolove update");
    println!("  neolove setup-path");
    println!("  neolove --help");
    println!("  neolove --version");
}

fn validate_project_root(project_root: &Path) -> Result<(), String> {
    if !project_root.exists() {
        return Err(format!(
            "project directory does not exist: {}",
            project_root.display()
        ));
    }
    if !project_root.is_dir() {
        return Err(format!(
            "project path is not a directory: {}",
            project_root.display()
        ));
    }

    let entry = project_root.join("main.luau");
    if !entry.exists() {
        return Err(format!(
            "missing main.luau in project root: {}",
            project_root.display()
        ));
    }
    if !entry.is_file() {
        return Err(format!(
            "main.luau exists but is not a file: {}",
            entry.display()
        ));
    }
    Ok(())
}

fn resolve_target_project_root(project_dir: Option<&str>) -> Result<PathBuf, String> {
    match project_dir {
        Some(dir) => resolve_from_cwd(dir)
            .map_err(|error| format!("failed to resolve project path '{dir}': {error}")),
        None => env::current_dir().map_err(|error| format!("failed to get current directory: {error}")),
    }
}

fn embedded_data_root(executable: &Path) -> Result<PathBuf, String> {
    let parent = executable
        .parent()
        .ok_or_else(|| "embedded executable has no parent directory".to_string())?;
    let stem = executable
        .file_stem()
        .and_then(|value| value.to_str())
        .map(sanitize_executable_name)
        .unwrap_or_else(|| "game".to_string());
    Ok(parent.join(format!("{stem}_data")))
}

fn run_cli() -> Result<(), String> {
    let args: Vec<String> = env::args().collect();

    let current_exe =
        env::current_exe().map_err(|error| format!("failed to resolve executable path: {error}"))?;

    let embedded_payload = read_embedded_payload(&current_exe)
        .map_err(|error| format!("failed to read embedded payload: {error}"))?;

    if let Some(payload) = embedded_payload {
        if args.len() == 1 {
            // A compiled game runs without a console on Windows, so route fatal
            // errors to a native dialog instead of stderr.
            enable_native_error_dialogs();
            let project_root = extract_embedded_project(&payload)
                .map_err(|error| format!("failed to extract embedded project: {error}"))?;
            let data_root = embedded_data_root(&current_exe)?;
            return run_project_window(project_root, Some(data_root));
        }
    }

    match setup_path_for_neolove() {
        Ok(true) => {
            eprintln!("Added Neolove to PATH. Open a new terminal to use `neolove` globally.");
        }
        Ok(false) => {}
        Err(e) => {
            eprintln!("PATH setup warning: {}", e);
        }
    }

    if args.len() <= 1 {
        print_usage();
        return Ok(());
    }

    match args[1].as_str() {
        "--help" | "-h" | "help" => {
            print_usage();
        }
        "--version" | "-V" | "version" => {
            println!("{}", env!("CARGO_PKG_VERSION"));
        }
        "setup-path" => match setup_path_for_neolove() {
            Ok(true) => println!("PATH updated. Restart your terminal."),
            Ok(false) => println!("PATH already contains Neolove."),
            Err(error) => return Err(format!("failed to set PATH: {error}")),
        },
        "update" => {
            if args.len() != 2 {
                return Err(format!(
                    "update failed: expected no arguments, got {}",
                    args.len().saturating_sub(2)
                ));
            }
            let outcome = update::update_engine()
                .map_err(|error| format!("update failed: {error}"))?;
            println!("{outcome}");
        }
        "new" => {
            if args.len() != 3 {
                return Err(format!(
                    "new failed: expected 1 project name argument, got {}",
                    args.len().saturating_sub(2)
                ));
            }
            let project_path = handle_new_command(&args[2])?;
            println!(
                "Created project \"{}\" at {}.",
                args[2],
                project_path.display()
            );
            println!("Set [window].title and [window].icon in neolove.toml to customize the game window.");
            println!("To run, execute in the project directory the command `neolove run`");
            println!("To build a standalone executable, run `neolove build`");
            println!("To build the webasm package, run `neolove build --webasm`");
        }
        "run" => {
            let project_root = resolve_target_project_root(args.get(2).map(String::as_str))?;
            validate_project_root(&project_root).map_err(|error| format!("run failed: {error}"))?;
            run_project_window(project_root, None).map_err(|error| format!("run failed: {error}"))?;
        }
        "editor" => {
            if args.len() > 3 {
                return Err(format!(
                    "editor failed: expected at most one project directory, got {}",
                    args.len().saturating_sub(2)
                ));
            }
            let project_root = resolve_target_project_root(args.get(2).map(String::as_str))?;
            if !project_root.is_dir() {
                return Err(format!(
                    "editor failed: project directory does not exist: {}",
                    project_root.display()
                ));
            }
            editor::run_editor(project_root).map_err(|error| format!("editor failed: {error}"))?;
        }
        "build" => {
            let mut project_arg: Option<&str> = None;
            let mut webasm = false;
            for arg in &args[2..] {
                if arg == "--webasm" {
                    webasm = true;
                } else if arg.starts_with('-') {
                    return Err(format!("build failed: unrecognized option: {arg}"));
                } else if project_arg.is_none() {
                    project_arg = Some(arg);
                } else {
                    return Err("build failed: expected at most one project directory".to_string());
                }
            }

            let project_root = resolve_target_project_root(project_arg)?;
            validate_project_root(&project_root)
                .map_err(|error| format!("build failed: {error}"))?;

            if webasm {
                let (bundle_output, zip_output) =
                    build_webasm(&project_root).map_err(|error| format!("build failed: {error}"))?;
                println!("Built webasm bundle: {}", bundle_output.display());
                println!("Built itch.io package: {}", zip_output.display());
            } else {
                let output = build_executable(&project_root)
                    .map_err(|error| format!("build failed: {error}"))?;
                println!("Built executable: {}", output.display());
            }
        }
        "api" => {
            if args.len() > 3 {
                return Err(format!(
                    "api failed: expected at most one project directory, got {}",
                    args.len().saturating_sub(2)
                ));
            }
            let paths = handle_api_command(args.get(2).map(String::as_str))?;
            if paths.len() == 2 {
                println!(
                    "Updated API definitions at {} and {}.",
                    paths[0].display(),
                    paths[1].display()
                );
            } else if let Some(path) = paths.first() {
                println!("Updated API definitions at {}.", path.display());
            }
        }
        _ => {
            print_usage();
            return Err(format!("unrecognized command: {}", args[1]));
        }
    }

    Ok(())
}

fn main() -> ExitCode {
    install_desktop_panic_hook();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run_cli)) {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err(error)) => {
            eprintln!("{error}");
            if native_error_dialogs_enabled() {
                show_native_error("NeoLOVE", &error);
            }
            ExitCode::FAILURE
        }
        Err(payload) => {
            let message =
                describe_desktop_panic("neolove encountered an internal panic", payload.as_ref());
            eprintln!("{message}");
            if native_error_dialogs_enabled() {
                show_native_error("NeoLOVE", &message);
            }
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod build_compression_tests {
    use super::*;

    #[test]
    fn compressed_payload_round_trips_asset_bytes() {
        let data = vec![42u8; 32 * 1024];
        let path = "assets/ambience.wav";
        let mut raw = Vec::new();
        raw.extend_from_slice(PAYLOAD_MAGIC);
        write_u32(&mut raw, 1);
        write_u16(&mut raw, path.len() as u16);
        raw.extend_from_slice(path.as_bytes());
        write_u64(&mut raw, data.len() as u64);
        raw.extend_from_slice(&data);

        let compressed = compress_build_payload(&raw).expect("compress payload");
        assert!(compressed.len() < raw.len());
        let output = std::env::temp_dir().join(format!(
            "neolove_compression_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&output);
        std::fs::create_dir_all(&output).expect("create temp dir");
        unpack_payload(&compressed, &output).expect("unpack payload");
        assert_eq!(std::fs::read(output.join(path)).expect("read unpacked file"), data);
        let _ = std::fs::remove_dir_all(output);
    }
}
