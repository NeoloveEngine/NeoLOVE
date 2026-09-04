mod android_module;
mod animation;
mod assets;
mod audio_system;
mod commands;
mod core;
#[cfg(not(neolove_packaged))]
mod editor;
#[cfg(not(neolove_packaged))]
mod editor_ipc;
mod environment3d;
mod fs_module;
#[cfg(feature = "vulkan")]
mod gpu_renderer;
pub mod hierarchy;
mod http;
mod lighting;
mod lua_error;
mod media;
mod mesh;
mod mobile_emulation;
mod mobile_module;
mod physics3d;
mod particles3d;
mod platform;
mod post_process;
mod prefabs;
mod render3d;
mod renderer;
mod rng;
#[path = "editor/scene.rs"]
mod scene;
mod servers;
mod shader;
mod tweening;
#[cfg(not(neolove_packaged))]
mod update;
mod user_input;
mod widget_interaction;
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

#[cfg(not(neolove_packaged))]
use base64::Engine as _;
use image::imageops::FilterType;
#[cfg(not(neolove_packaged))]
use image::ImageEncoder as _;
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
use winit::window::{CursorGrabMode, Fullscreen, Icon, WindowBuilder};
use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

#[cfg(feature = "vulkan")]
use crate::gpu_renderer::VulkanPresenter;
use crate::platform::{SharedPlatformState, lock_platform_state};
use crate::renderer::SoftwareRenderer;

const EMBED_TRAILER_MAGIC: &[u8; 16] = b"NEOLOVE_EMBED_V1";
const PAYLOAD_MAGIC: &[u8; 8] = b"NLPKGv1\0";
const COMPRESSED_PAYLOAD_MAGIC: &[u8; 8] = b"NLPKGv2\0";
const WRAPPER_MAGIC: &[u8; 16] = b"NEOLOVE_WRAPPED1";
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
    /// The real software runtime renderer without an OS presentation surface.
    /// Editor Game View uses this variant inside its isolated child process.
    EmbeddedSoftware {
        renderer: SoftwareRenderer,
    },
}

/// Convert the physical swapchain/surface extent into the logical coordinate
/// space used by scripts. Rendering the software path at logical resolution
/// avoids doing 4x the lighting, post-process, and raster work on a 2x Retina
/// display, while the final blit still covers every physical output pixel.
fn logical_dimensions(width: u32, height: u32, scale_factor: f64) -> (u32, u32) {
    let scale_factor = if scale_factor.is_finite() {
        scale_factor.max(1.0)
    } else {
        1.0
    };
    (
        ((width.max(1) as f64 / scale_factor).round() as u32).max(1),
        ((height.max(1) as f64 / scale_factor).round() as u32).max(1),
    )
}

fn pack_softbuffer_pixel(rgba: &[u8]) -> u32 {
    (rgba[2] as u32) | ((rgba[1] as u32) << 8) | ((rgba[0] as u32) << 16)
}

fn sample_bilinear_channel(
    pixels: &[u8],
    width: usize,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    tx: f32,
    ty: f32,
    channel: usize,
) -> u8 {
    let top = pixels[(y0 * width + x0) * 4 + channel] as f32 * (1.0 - tx)
        + pixels[(y0 * width + x1) * 4 + channel] as f32 * tx;
    let bottom = pixels[(y1 * width + x0) * 4 + channel] as f32 * (1.0 - tx)
        + pixels[(y1 * width + x1) * 4 + channel] as f32 * tx;
    (top * (1.0 - ty) + bottom * ty).round().clamp(0.0, 255.0) as u8
}

fn blit_software_pixels(
    source: &[u8],
    source_width: u32,
    source_height: u32,
    destination: &mut [u32],
    destination_width: u32,
    destination_height: u32,
    nearest: bool,
) {
    let source_width = source_width.max(1) as usize;
    let source_height = source_height.max(1) as usize;
    let destination_width = destination_width.max(1) as usize;
    let destination_height = destination_height.max(1) as usize;
    if source_width == destination_width && source_height == destination_height {
        for (dst, rgba) in destination.iter_mut().zip(source.chunks_exact(4)) {
            *dst = pack_softbuffer_pixel(rgba);
        }
        return;
    }

    if nearest {
        if destination_width % source_width == 0 && destination_height % source_height == 0 {
            let scale_x = destination_width / source_width;
            let scale_y = destination_height / source_height;
            for source_y in 0..source_height {
                let destination_y = source_y * scale_y;
                let row_start = destination_y * destination_width;
                for source_x in 0..source_width {
                    let offset = (source_y * source_width + source_x) * 4;
                    let packed = pack_softbuffer_pixel(&source[offset..offset + 4]);
                    let start = row_start + source_x * scale_x;
                    destination[start..start + scale_x].fill(packed);
                }
                let row_end = row_start + destination_width;
                for duplicate in 1..scale_y {
                    destination.copy_within(
                        row_start..row_end,
                        row_start + duplicate * destination_width,
                    );
                }
            }
            return;
        }
        for y in 0..destination_height {
            let source_y = y * source_height / destination_height;
            let destination_row =
                &mut destination[y * destination_width..(y + 1).saturating_mul(destination_width)];
            for (x, dst) in destination_row.iter_mut().enumerate() {
                let source_x = x * source_width / destination_width;
                let offset = (source_y * source_width + source_x) * 4;
                *dst = pack_softbuffer_pixel(&source[offset..offset + 4]);
            }
        }
        return;
    }

    let scale_x = source_width as f32 / destination_width as f32;
    let scale_y = source_height as f32 / destination_height as f32;
    for y in 0..destination_height {
        let source_y = ((y as f32 + 0.5) * scale_y - 0.5).clamp(0.0, source_height as f32 - 1.0);
        let y0 = source_y.floor() as usize;
        let y1 = (y0 + 1).min(source_height - 1);
        let ty = source_y - y0 as f32;
        let destination_row = &mut destination[y * destination_width..(y + 1) * destination_width];
        for (x, dst) in destination_row.iter_mut().enumerate() {
            let source_x = ((x as f32 + 0.5) * scale_x - 0.5).clamp(0.0, source_width as f32 - 1.0);
            let x0 = source_x.floor() as usize;
            let x1 = (x0 + 1).min(source_width - 1);
            let tx = source_x - x0 as f32;
            let r = sample_bilinear_channel(source, source_width, x0, y0, x1, y1, tx, ty, 0);
            let g = sample_bilinear_channel(source, source_width, x0, y0, x1, y1, tx, ty, 1);
            let b = sample_bilinear_channel(source, source_width, x0, y0, x1, y1, tx, ty, 2);
            *dst = (b as u32) | ((g as u32) << 8) | ((r as u32) << 16);
        }
    }
}

impl DesktopPresenter {
    fn new_embedded(
        _event_loop: &EventLoop<()>,
        _window: &std::sync::Arc<winit::window::Window>,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        let requested = env::var("NEOLOVE_EDITOR_EMBEDDED_BACKEND")
            .unwrap_or_else(|_| "auto".to_string())
            .to_ascii_lowercase();
        #[cfg(feature = "vulkan")]
        if requested != "software" {
            match catch_desktop_panic("failed while initializing embedded Vulkan", || {
                VulkanPresenter::new(_event_loop, _window.clone())
            })? {
                Ok((mut presenter, _surface)) => {
                    presenter.set_frame_capture_enabled(true);
                    return Ok(Self::Vulkan(presenter));
                }
                Err(error) if requested == "vulkan" => {
                    return Err(format!(
                        "embedded Vulkan was explicitly requested but initialization failed: {error}"
                    ));
                }
                Err(error) => eprintln!(
                    "render warning: embedded Vulkan capture unavailable, using software: {error}"
                ),
            }
        }

        #[cfg(not(feature = "vulkan"))]
        if requested == "vulkan" {
            return Err(
                "embedded Vulkan was explicitly requested, but this build has no Vulkan feature"
                    .to_string(),
            );
        }

        Ok(Self::EmbeddedSoftware {
            renderer: SoftwareRenderer::new(width, height),
        })
    }

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
        let (width, height) = logical_dimensions(size.width, size.height, window.scale_factor());
        Ok(Self::Software {
            _context: context,
            surface,
            renderer: SoftwareRenderer::new(width, height),
            vulkan_error,
        })
    }

    fn request_resize(&mut self) {
        #[cfg(feature = "vulkan")]
        if let Self::Vulkan(presenter) = self {
            presenter.request_swapchain_recreate();
        }
    }

    fn backend_name(&self) -> &'static str {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(presenter) if presenter.captured_pixels().is_some() => "vulkan-embedded",
            #[cfg(feature = "vulkan")]
            Self::Vulkan(_) => "vulkan",
            Self::Software { .. } => "software",
            Self::EmbeddedSoftware { .. } => "software-embedded",
        }
    }

    fn embedded_pixels(&self) -> Option<(u32, u32, &[u8])> {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(presenter) => presenter.captured_pixels(),
            Self::EmbeddedSoftware { renderer } => {
                let (width, height) = renderer.dimensions();
                Some((width, height, renderer.pixels()))
            }
            _ => None,
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
                let (logical_width, logical_height) =
                    logical_dimensions(size.width, size.height, window.scale_factor());
                presenter.render(
                    platform_state,
                    render_state,
                    size.width,
                    size.height,
                    logical_width,
                    logical_height,
                )
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
                let (logical_width, logical_height) =
                    logical_dimensions(width, height, window.scale_factor());
                renderer.resize(logical_width, logical_height);
                renderer
                    .render(platform_state, render_state)
                    .map_err(|error| {
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
                let mut buffer = surface.buffer_mut().map_err(|error| {
                    format!("failed to acquire software surface buffer: {error}")
                })?;
                let nearest = lock_platform_state(platform_state).nearest_neighbor_scaling();
                blit_software_pixels(
                    renderer.pixels(),
                    logical_width,
                    logical_height,
                    &mut buffer,
                    width,
                    height,
                    nearest,
                );
                buffer
                    .present()
                    .map_err(|error| format!("failed to present software surface: {error}"))?;
                Ok(())
            }
            Self::EmbeddedSoftware { renderer } => {
                let size = window.inner_size();
                let (logical_width, logical_height) =
                    logical_dimensions(size.width, size.height, window.scale_factor());
                renderer.resize(logical_width, logical_height);
                renderer
                    .render(platform_state, render_state)
                    .map_err(|error| format!("embedded software renderer failed: {error}"))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ProjectKind {
    #[default]
    TwoD,
    ThreeD,
}

impl ProjectKind {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "2d" => Some(Self::TwoD),
            "3d" => Some(Self::ThreeD),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::TwoD => "2d",
            Self::ThreeD => "3d",
        }
    }
}

#[derive(Default, Clone)]
struct ProjectSettings {
    kind: ProjectKind,
    package_name: Option<String>,
    start_scene: Option<String>,
    window_title: Option<String>,
    window_icon: Option<String>,
    window_width: Option<f32>,
    window_height: Option<f32>,
    window_fullscreen: Option<bool>,
    window_resizable: Option<bool>,
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

fn write_text_if_changed(path: &Path, contents: &str) -> std::io::Result<bool> {
    let existing = fs::read_to_string(path).ok();
    if existing.as_deref() == Some(contents) {
        return Ok(false);
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(true)
}

#[cfg(windows)]
fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(windows)]
fn ensure_start_menu_entry(exe: &Path) -> Result<bool, String> {
    let work_dir = exe
        .parent()
        .ok_or_else(|| "executable has no parent directory".to_string())?;
    let exe = powershell_quote(&exe.to_string_lossy());
    let work_dir = powershell_quote(&work_dir.to_string_lossy());
    let script = format!(
        "$exe={exe}; \
         $args='hub'; \
         $work={work_dir}; \
         $programs=[Environment]::GetFolderPath('Programs'); \
         if(-not $programs){{ $programs=Join-Path $env:APPDATA 'Microsoft\\Windows\\Start Menu\\Programs' }}; \
         New-Item -ItemType Directory -Force -Path $programs | Out-Null; \
         $shortcut=Join-Path $programs 'NeoLOVE.lnk'; \
         $shell=New-Object -ComObject WScript.Shell; \
         $link=$shell.CreateShortcut($shortcut); \
         $changed=(-not (Test-Path $shortcut)) -or ($link.TargetPath -ne $exe) -or ($link.Arguments -ne $args) -or ($link.WorkingDirectory -ne $work); \
         $link.TargetPath=$exe; \
         $link.Arguments=$args; \
         $link.WorkingDirectory=$work; \
         $link.IconLocation=\"$exe,0\"; \
         $link.Description='Open the NeoLOVE Hub'; \
         $link.Save(); \
         if($changed){{ Write-Output 'updated' }}else{{ Write-Output 'exists' }}"
    );

    let output = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .map_err(|e| format!("failed to run powershell for Start Menu setup: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "failed to update Start Menu shortcut: {}",
            stderr.trim()
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).contains("updated"))
}

#[cfg(target_os = "macos")]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(target_os = "macos")]
fn ensure_start_menu_entry(exe: &Path) -> Result<bool, String> {
    let home = user_home_dir().ok_or_else(|| "could not resolve home directory".to_string())?;
    let app_root = home
        .join("Applications")
        .join("NeoLOVE Hub.app")
        .join("Contents");
    let macos_dir = app_root.join("MacOS");
    let script_path = macos_dir.join("NeoLOVE Hub");
    let plist_path = app_root.join("Info.plist");

    let script = format!(
        "#!/bin/sh\nexec {} hub \"$@\"\n",
        shell_quote(&exe.to_string_lossy())
    );
    let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>
  <string>NeoLOVE Hub</string>
  <key>CFBundleIdentifier</key>
  <string>org.neolove.hub</string>
  <key>CFBundleName</key>
  <string>NeoLOVE Hub</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>LSMinimumSystemVersion</key>
  <string>10.13</string>
  <key>NSCameraUsageDescription</key>
  <string>NeoLOVE games can request camera access for developer-defined gameplay features.</string>
  <key>NSMicrophoneUsageDescription</key>
  <string>NeoLOVE games can request microphone access for developer-defined gameplay features.</string>
</dict>
</plist>
"#;

    let mut changed = write_text_if_changed(&script_path, &script).map_err(|e| {
        format!(
            "failed to write launcher script {}: {e}",
            script_path.display()
        )
    })?;
    changed |= write_text_if_changed(&plist_path, plist).map_err(|e| {
        format!(
            "failed to write launcher plist {}: {e}",
            plist_path.display()
        )
    })?;

    let metadata = fs::metadata(&script_path).map_err(|e| {
        format!(
            "failed to stat launcher script {}: {e}",
            script_path.display()
        )
    })?;
    let mut perms = metadata.permissions();
    let mode = perms.mode();
    if mode & 0o111 == 0 {
        perms.set_mode(mode | 0o755);
        fs::set_permissions(&script_path, perms).map_err(|e| {
            format!(
                "failed to make launcher script executable {}: {e}",
                script_path.display()
            )
        })?;
        changed = true;
    }

    Ok(changed)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn desktop_exec_quote(path: &Path) -> String {
    let value = path.to_string_lossy();
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn ensure_start_menu_entry(exe: &Path) -> Result<bool, String> {
    let home = user_home_dir().ok_or_else(|| "could not resolve home directory".to_string())?;
    let data_home = env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local").join("share"));
    let applications = data_home.join("applications");
    let desktop_file = applications.join("neolove-hub.desktop");
    let icon_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("logo.png");
    let icon_line = if icon_path.is_file() {
        format!("Icon={}\n", icon_path.to_string_lossy())
    } else {
        String::new()
    };
    let contents = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.0\n\
         Name=NeoLOVE\n\
         GenericName=Game Engine\n\
         Comment=Open the NeoLOVE Hub\n\
         Exec={} hub\n\
         Terminal=false\n\
         Categories=Development;IDE;Game;\n\
         StartupNotify=true\n\
         {icon_line}",
        desktop_exec_quote(exe)
    );

    write_text_if_changed(&desktop_file, &contents).map_err(|e| {
        format!(
            "failed to write app launcher {}: {e}",
            desktop_file.display()
        )
    })
}

fn setup_start_menu_for_neolove() -> Result<bool, String> {
    let exe = env::current_exe().map_err(|e| format!("could not resolve executable path: {e}"))?;
    ensure_start_menu_entry(&exe)
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

fn parse_number(input: &str) -> Option<f32> {
    input
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
}

fn parse_bool(input: &str) -> Option<bool> {
    match input.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
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
        match section.as_str() {
            "package" if key == "name" => {
                if let Some(value) = parse_quoted(value_raw) {
                    settings.package_name = Some(value);
                }
            }
            "project" if key == "kind" => {
                if let Some(value) =
                    parse_quoted(value_raw).and_then(|value| ProjectKind::parse(&value))
                {
                    settings.kind = value;
                }
            }
            "project" if key == "start_scene" => {
                if let Some(value) = parse_quoted(value_raw) {
                    settings.start_scene = Some(value);
                }
            }
            "window" if key == "title" => {
                if let Some(value) = parse_quoted(value_raw) {
                    settings.window_title = Some(value);
                }
            }
            "window" if key == "icon" => {
                if let Some(value) = parse_quoted(value_raw) {
                    settings.window_icon = Some(value);
                }
            }
            "window" if key == "width" => {
                settings.window_width =
                    parse_number(value_raw).map(|value| value.clamp(1.0, 16384.0));
            }
            "window" if key == "height" => {
                settings.window_height =
                    parse_number(value_raw).map(|value| value.clamp(1.0, 16384.0));
            }
            "window" if key == "fullscreen" => {
                settings.window_fullscreen = parse_bool(value_raw);
            }
            "window" if key == "resizable" => {
                settings.window_resizable = parse_bool(value_raw);
            }
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

fn window_options_for_project(project_root: &Path) -> (String, Option<Icon>, f32, f32, bool, bool) {
    let settings = parse_project_settings(project_root);
    let title = settings
        .window_title
        .clone()
        .or(settings.package_name.clone())
        .unwrap_or_else(|| "NeoLOVE".to_string());
    let width = settings.window_width.unwrap_or(DEFAULT_WINDOW_WIDTH);
    let height = settings.window_height.unwrap_or(DEFAULT_WINDOW_HEIGHT);
    let fullscreen = settings.window_fullscreen.unwrap_or(false);
    let resizable = settings.window_resizable.unwrap_or(true);

    let icon = settings
        .window_icon
        .as_ref()
        .and_then(|path| try_load_window_icon(project_root, path));

    (title, icon, width, height, fullscreen, resizable)
}

fn should_skip_in_build(path: &Path) -> bool {
    if path.components().any(|component| {
        let name = component.as_os_str();
        name == OsStr::new(".git")
            || name == OsStr::new(".vscode")
            || name == OsStr::new(".idea")
            || name == OsStr::new(".neolove")
            || name == OsStr::new("target")
            || name == OsStr::new("dist")
    }) {
        return true;
    }

    let file_name = path.file_name().unwrap_or_default();
    file_name == OsStr::new(".gitignore")
        || file_name == OsStr::new(".luaurc")
        || is_lua_declaration_file(path)
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

    let total_steps = files.len() + 2;
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
    let mut compressed =
        Vec::with_capacity(COMPRESSED_PAYLOAD_MAGIC.len() + cursor.get_ref().len());
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DesktopPackageTarget {
    Host,
    Windows,
    Linux,
}

impl DesktopPackageTarget {
    fn label(self) -> &'static str {
        match self {
            Self::Host => "desktop",
            Self::Windows => "Windows desktop",
            Self::Linux => "Linux desktop",
        }
    }

    fn target_triple(self) -> Option<&'static str> {
        match self {
            Self::Host => None,
            Self::Windows if cfg!(windows) => None,
            Self::Windows => Some("x86_64-pc-windows-gnu"),
            Self::Linux if cfg!(target_os = "linux") => None,
            Self::Linux => Some("x86_64-unknown-linux-gnu"),
        }
    }

    fn target_dir_name(self, project_kind: ProjectKind) -> String {
        let platform = match self.target_triple() {
            Some("x86_64-pc-windows-gnu") => "windows-x86_64",
            Some("x86_64-unknown-linux-gnu") => "linux-x86_64",
            _ => "host",
        };
        format!(
            "neolove-packaged-runtime-{platform}-{}",
            project_kind.as_str()
        )
    }

    fn is_windows(self) -> bool {
        match self {
            Self::Windows => true,
            Self::Linux => false,
            Self::Host => cfg!(windows),
        }
    }
}

/// MinGW can otherwise leave a cross-built game dependent on
/// `libstdc++-6.dll` from the build machine. Native Windows builds typically
/// find that runtime through their toolchain installation, but a distributed
/// single-file game will fail at process startup. Link the small GNU C/C++
/// runtime portions into Linux-to-Windows artifacts instead.
fn cross_target_rustflags_config(
    target: DesktopPackageTarget,
    cpp_runtime_dir: Option<&Path>,
) -> Option<String> {
    match target.target_triple() {
        Some("x86_64-pc-windows-gnu") => {
            let mut flags = Vec::new();
            if let Some(dir) = cpp_runtime_dir {
                flags.push("-L".to_string());
                flags.push(format!("native={}", dir.display()));
            }
            flags.extend([
                "-C".to_string(),
                "link-arg=-static-libgcc".to_string(),
                "-C".to_string(),
                "link-arg=-static-libstdc++".to_string(),
            ]);
            let flags = serde_json::to_string(&flags).ok()?;
            Some(format!("target.x86_64-pc-windows-gnu.rustflags={flags}"))
        }
        _ => None,
    }
}

fn cross_target_cpp_stdlib(target: DesktopPackageTarget) -> Option<(&'static str, &'static str)> {
    match target.target_triple() {
        // luau0-src honors this target-qualified variable and forwards its
        // value as a Cargo link kind. Without `static=`, mlua-sys emits a
        // dynamic `stdc++` dependency even when GCC's own runtime flags are
        // static.
        Some("x86_64-pc-windows-gnu") => Some(("CXXSTDLIB_x86_64_pc_windows_gnu", "static=stdc++")),
        _ => None,
    }
}

fn mingw_cpp_runtime_dir(target: DesktopPackageTarget) -> Result<Option<PathBuf>, String> {
    if target.target_triple() != Some("x86_64-pc-windows-gnu") {
        return Ok(None);
    }
    let compiler = "x86_64-w64-mingw32-gcc";
    let output = std::process::Command::new(compiler)
        .arg("-print-file-name=libstdc++.a")
        .output()
        .map_err(|error| format!("failed to locate the MinGW static C++ runtime: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to locate the MinGW static C++ runtime with {compiler}"
        ));
    }
    let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    if !path.is_file() {
        return Err(format!(
            "MinGW static C++ runtime was not found (expected libstdc++.a, got {})",
            path.display()
        ));
    }
    Ok(path.parent().map(Path::to_path_buf))
}

/// Rewrite the PE `Subsystem` field of a Windows executable image in place,
/// switching it from the console subsystem (used by the `neolove` CLI) to the
/// GUI subsystem so a compiled game launches without spawning a terminal window.
///
/// The dev `neolove.exe` stays a console application; only the copy we hand to
/// players is patched. Field offsets follow the PE/COFF specification and are
/// identical for PE32 and PE32+ images.
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

fn executable_file_name(output_stem: &str, target: DesktopPackageTarget) -> String {
    let mut output_name = output_stem.to_string();
    if target.is_windows() && !output_name.to_ascii_lowercase().ends_with(".exe") {
        output_name.push_str(".exe");
    }
    output_name
}

fn runtime_executable_file_name(target: DesktopPackageTarget) -> String {
    executable_file_name(env!("CARGO_PKG_NAME"), target)
}

fn find_on_path(program: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|path| {
        env::split_paths(&path)
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
    })
}

fn ensure_cross_desktop_linker(
    target: DesktopPackageTarget,
) -> Result<Option<(&'static str, &'static str)>, String> {
    match target.target_triple() {
        Some("x86_64-pc-windows-gnu") => {
            let linker = "x86_64-w64-mingw32-gcc";
            if find_on_path(linker).is_none() {
                return Err(
                    "Windows desktop builds from this host need the MinGW-w64 cross linker \
                     `x86_64-w64-mingw32-gcc` on PATH. Install MinGW-w64, then run \
                     `neolove build --windows` again."
                        .to_string(),
                );
            }
            Ok(Some(("CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER", linker)))
        }
        Some("x86_64-unknown-linux-gnu") => {
            let linker = "x86_64-linux-gnu-gcc";
            if find_on_path(linker).is_none() {
                return Err("Linux desktop builds from this host need the cross linker \
                     `x86_64-linux-gnu-gcc` on PATH. Install a Linux GNU cross toolchain \
                     or build from Linux/WSL, then run `neolove build --linux` again."
                    .to_string());
            }
            Ok(Some((
                "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER",
                linker,
            )))
        }
        Some(target_triple) => Err(format!("unsupported desktop cross target: {target_triple}")),
        None => Ok(None),
    }
}

fn ensure_rust_target_installed(target_triple: &str) -> Result<(), String> {
    println!("Ensuring Rust target {target_triple} is installed...");
    let mut rustup = std::process::Command::new("rustup");
    rustup.args(["target", "add", target_triple]);
    run_checked_command(&mut rustup, "installing desktop Rust target")
}

fn build_packaged_runtime(
    target: DesktopPackageTarget,
    project_kind: ProjectKind,
) -> Result<(PathBuf, PathBuf), String> {
    let engine_root = engine_source_root()?;
    let cargo_target_dir = engine_root
        .join("target")
        .join(target.target_dir_name(project_kind));
    let rust_target = target.target_triple();
    if let Some(target_triple) = rust_target {
        ensure_rust_target_installed(target_triple)?;
    }
    let linker_env = ensure_cross_desktop_linker(target)?;
    let cpp_runtime_dir = mingw_cpp_runtime_dir(target)?;

    println!("Building compact {} runtime...", target.label());
    println!("This can take a few minutes the first time; Cargo output will be shown below.");
    let mut cargo = std::process::Command::new("cargo");
    apply_size_optimized_release_env(&mut cargo);
    if let Some(config) = cross_target_rustflags_config(target, cpp_runtime_dir.as_deref()) {
        cargo.arg("--config").arg(config);
    }
    if let Some((key, value)) = cross_target_cpp_stdlib(target) {
        cargo.env(key, value);
    }
    cargo
        .env("NEOLOVE_PACKAGED_RUNTIME", "1")
        .env("NEOLOVE_PACKAGED_PROJECT_KIND", project_kind.as_str())
        .env("CARGO_TARGET_DIR", &cargo_target_dir)
        .arg("build")
        .arg("--release")
        .arg("--bin")
        .arg(env!("CARGO_PKG_NAME"))
        .arg("--bin")
        .arg("neolove-launcher")
        .args(["--features", "packaged-launcher"]);
    if let Some(target_triple) = rust_target {
        cargo.arg("--target").arg(target_triple);
    }
    if let Some((env_key, linker)) = linker_env {
        cargo.env(env_key, linker);
    }
    if cfg!(feature = "vulkan") {
        cargo.args(["--features", "vulkan"]);
    }
    cargo.current_dir(&engine_root);
    run_checked_command(&mut cargo, "building compact packaged runtime")?;

    let artifact = if rust_target.is_some() {
        cargo_target_dir
            .join(rust_target.expect("checked above"))
            .join("release")
            .join(runtime_executable_file_name(target))
    } else {
        cargo_target_dir
            .join("release")
            .join(runtime_executable_file_name(target))
    };
    if !artifact.is_file() {
        return Err(format!(
            "packaged runtime build succeeded but output was not found: {}",
            artifact.display()
        ));
    }
    let launcher = if rust_target.is_some() {
        cargo_target_dir
            .join(rust_target.expect("checked above"))
            .join("release")
            .join(executable_file_name("neolove-launcher", target))
    } else {
        cargo_target_dir
            .join("release")
            .join(executable_file_name("neolove-launcher", target))
    };
    if !launcher.is_file() {
        return Err(format!(
            "packaged launcher build succeeded but output was not found: {}",
            launcher.display()
        ));
    }
    Ok((artifact, launcher))
}

fn build_executable(project_root: &Path, target: DesktopPackageTarget) -> Result<PathBuf, String> {
    let project_kind = parse_project_settings(project_root).kind;
    let output_stem = project_output_stem(project_root);
    let output_name = executable_file_name(&output_stem, target);

    let payload = build_payload(project_root)?;

    let (packaged_runtime, packaged_launcher) = build_packaged_runtime(target, project_kind)?;
    #[allow(unused_mut)]
    let mut engine_bytes = fs::read(&packaged_runtime).map_err(|e| {
        format!(
            "failed to read packaged runtime {}: {e}",
            packaged_runtime.display()
        )
    })?;

    // Compiled Windows games should not pop up a console window; flip the copied
    // image to the GUI subsystem before embedding the payload, even when the
    // host building it is not Windows.
    if target.is_windows() {
        patch_subsystem_to_gui(&mut engine_bytes)
            .map_err(|e| format!("failed to prepare windowed game executable: {e}"))?;
    }

    let output_dir = project_root.join("dist");
    fs::create_dir_all(&output_dir).map_err(|e| {
        format!(
            "failed to create dist directory {}: {e}",
            output_dir.display()
        )
    })?;
    let output_path = output_dir.join(output_name);

    let mut encoder = flate2::write::DeflateEncoder::new(
        Vec::new(),
        flate2::Compression::best(),
    );
    encoder
        .write_all(&engine_bytes)
        .map_err(|error| format!("failed to compress packaged runtime: {error}"))?;
    let compressed_runtime = encoder
        .finish()
        .map_err(|error| format!("failed to finalize packaged runtime: {error}"))?;

    let mut launcher_bytes = fs::read(&packaged_launcher).map_err(|error| {
        format!(
            "failed to read packaged launcher {}: {error}",
            packaged_launcher.display()
        )
    })?;
    if target.is_windows() {
        patch_subsystem_to_gui(&mut launcher_bytes)
            .map_err(|error| format!("failed to prepare game launcher: {error}"))?;
    }

    let total_steps = 3usize;
    progress_bar(1, total_steps, "Writing compressed game launcher");

    let mut out_file = File::create(&output_path).map_err(|e| {
        format!(
            "failed to create output executable {}: {e}",
            output_path.display()
        )
    })?;
    out_file
        .write_all(&launcher_bytes)
        .map_err(|e| format!("failed to write launcher bytes: {e}"))?;

    progress_bar(2, total_steps, "Embedding compressed runtime and game");
    out_file
        .write_all(&compressed_runtime)
        .and_then(|_| out_file.write_all(&(compressed_runtime.len() as u64).to_le_bytes()))
        .and_then(|_| out_file.write_all(WRAPPER_MAGIC))
        .map_err(|e| format!("failed to write compressed runtime: {e}"))?;
    out_file
        .write_all(&payload)
        .and_then(|_| out_file.write_all(&(payload.len() as u64).to_le_bytes()))
        .and_then(|_| out_file.write_all(EMBED_TRAILER_MAGIC))
        .map_err(|e| format!("failed to write game payload: {e}"))?;
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
            "build requires engine source files; expected Cargo.toml at {}",
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

fn apply_size_optimized_release_env(command: &mut std::process::Command) {
    command
        .env("CARGO_PROFILE_RELEASE_OPT_LEVEL", "z")
        .env("CARGO_PROFILE_RELEASE_LTO", "fat")
        .env("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", "1")
        .env("CARGO_PROFILE_RELEASE_STRIP", "symbols")
        .env("CARGO_PROFILE_RELEASE_DEBUG", "false")
        .env("CARGO_PROFILE_RELEASE_INCREMENTAL", "false");
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
    let entries = fs::read_dir(&node_root).map_err(|e| {
        format!(
            "failed to read emsdk node directory {}: {e}",
            node_root.display()
        )
    })?;

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
    let joined =
        env::join_paths(paths).map_err(|e| format!("failed to construct PATH for emsdk: {e}"))?;

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
        fs::remove_dir_all(&root).map_err(|e| {
            format!(
                "failed to clean incomplete emsdk install {}: {e}",
                root.display()
            )
        })?;
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

fn copy_dir_recursive(source: &Path, destination: &Path) -> Result<(), String> {
    if !source.is_dir() {
        return Err(format!(
            "source directory does not exist: {}",
            source.display()
        ));
    }
    fs::create_dir_all(destination)
        .map_err(|e| format!("failed to create directory {}: {e}", destination.display()))?;
    for entry in
        fs::read_dir(source).map_err(|e| format!("failed to read {}: {e}", source.display()))?
    {
        let entry =
            entry.map_err(|e| format!("failed to read entry in {}: {e}", source.display()))?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let kind = entry
            .file_type()
            .map_err(|e| format!("failed to inspect {}: {e}", source_path.display()))?;
        if kind.is_dir() {
            copy_dir_recursive(&source_path, &destination_path)?;
        } else if kind.is_file() {
            fs::copy(&source_path, &destination_path).map_err(|e| {
                format!(
                    "failed to copy {} -> {}: {e}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }
    Ok(())
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
            fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "failed to create staged directory {}: {e}",
                    parent.display()
                )
            })?;
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
        let file_type =
            fs::metadata(&child).map_err(|e| format!("failed to stat {}: {e}", child.display()))?;
        if file_type.is_dir() {
            collect_bundle_files(&child, out)?;
        } else if file_type.is_file() {
            out.push(child);
        }
    }
    Ok(())
}

fn create_webasm_zip(bundle_dir: &Path, zip_path: &Path) -> Result<(), String> {
    let file = File::create(zip_path).map_err(|e| {
        format!(
            "failed to create webasm package {}: {e}",
            zip_path.display()
        )
    })?;
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

    archive.finish().map_err(|e| {
        format!(
            "failed to finalize webasm package {}: {e}",
            zip_path.display()
        )
    })?;

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
    let staged_project = fs::canonicalize(&stage_dir).map_err(|e| {
        format!(
            "failed to resolve staged webasm project {}: {e}",
            stage_dir.display()
        )
    })?;

    println!("Ensuring emsdk is installed...");
    let emsdk = ensure_emsdk()?;

    println!("Ensuring wasm32-unknown-emscripten target is installed...");
    let mut rustup = std::process::Command::new("rustup");
    rustup.args(["target", "add", "wasm32-unknown-emscripten"]);
    run_checked_command(&mut rustup, "installing wasm32-unknown-emscripten target")?;

    let engine_root = engine_source_root()?;
    let cargo_target_dir = engine_root
        .join("target")
        .join("webasm-emscripten-legacy-eh");
    println!("Building NeoLOVE webasm runtime...");
    let mut cargo = std::process::Command::new("cargo");
    apply_emsdk_env(&mut cargo, &emsdk)?;
    apply_size_optimized_release_env(&mut cargo);
    cargo.env("CXXFLAGS", "-Oz -fwasm-exceptions");
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
        .arg("link-arg=-Oz")
        .arg("-C")
        .arg("link-arg=--strip-debug")
        .arg("-C")
        .arg("link-arg=-sASSERTIONS=0")
        .arg("-C")
        .arg("link-arg=-sMALLOC=emmalloc")
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
        let file_name = artifact.file_name().ok_or_else(|| {
            format!(
                "failed to resolve artifact file name for {}",
                artifact.display()
            )
        })?;
        let destination = bundle_dir.join(file_name);
        fs::copy(artifact, &destination).map_err(|e| {
            format!(
                "failed to copy webasm artifact {} -> {}: {e}",
                artifact.display(),
                destination.display()
            )
        })?;
    }

    fs::write(
        bundle_dir.join("index.html"),
        webasm_index_html(project_root),
    )
    .map_err(|e| {
        format!(
            "failed to write webasm loader {}: {e}",
            bundle_dir.join("index.html").display()
        )
    })?;

    if stage_dir.exists() {
        fs::remove_dir_all(&stage_dir).map_err(|e| {
            format!(
                "failed to clean staged webasm files {}: {e}",
                stage_dir.display()
            )
        })?;
    }

    let zip_output = output_dir.join(format!("{output_stem}-webasm.zip"));
    create_webasm_zip(&bundle_dir, &zip_output)?;

    Ok((bundle_dir, zip_output))
}

const ANDROID_API_LEVEL: &str = "35";
const ANDROID_BUILD_TOOLS_VERSION: &str = "35.0.0";
const ANDROID_NDK_VERSION: &str = "27.2.12479018";
const ANDROID_MIN_SDK: &str = "24";
const ANDROID_RUST_TARGET: &str = "aarch64-linux-android";
const ANDROID_ABI: &str = "arm64-v8a";
const ANDROID_PAYLOAD_ASSET: &str = "neolove_project.payload";
const ANDROID_CMDLINE_TOOLS_URL: &str =
    "https://dl.google.com/android/repository/commandlinetools-linux-14742923_latest.zip";
const JDK_LINUX_X64_URL: &str =
    "https://api.adoptium.net/v3/binary/latest/21/ga/linux/x64/jdk/hotspot/normal/eclipse";

struct AndroidToolchain {
    java_home: PathBuf,
    aapt2: PathBuf,
    zipalign: PathBuf,
    apksigner: PathBuf,
    android_jar: PathBuf,
    clang: PathBuf,
    clangxx: PathBuf,
    llvm_ar: PathBuf,
    llvm_strip: PathBuf,
    keytool: PathBuf,
}

fn executable_name(name: &str) -> String {
    #[cfg(windows)]
    {
        format!("{name}.exe")
    }
    #[cfg(not(windows))]
    {
        name.to_string()
    }
}

fn script_name(name: &str) -> String {
    #[cfg(windows)]
    {
        format!("{name}.bat")
    }
    #[cfg(not(windows))]
    {
        name.to_string()
    }
}

fn find_program_on_path(name: &str) -> Option<PathBuf> {
    let name = executable_name(name);
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|dir| dir.join(&name))
        .find(|candidate| candidate.is_file())
}

fn java_home_from_executable(java: &Path) -> Option<PathBuf> {
    java.parent()?.parent().map(Path::to_path_buf)
}

fn find_java_home() -> Option<PathBuf> {
    env::var_os("JAVA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|home| home.join("bin").join(executable_name("java")).is_file())
        .or_else(|| find_program_on_path("java").and_then(|java| java_home_from_executable(&java)))
}

fn find_java_home_under(root: &Path) -> Option<PathBuf> {
    if root.join("bin").join(executable_name("java")).is_file() {
        return Some(root.to_path_buf());
    }
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.join("bin").join(executable_name("java")).is_file() {
            return Some(path);
        }
    }
    None
}

fn toolchains_root() -> Result<PathBuf, String> {
    let home = user_home_dir().ok_or_else(|| "could not resolve home directory".to_string())?;
    Ok(home.join(".neolove").join("toolchains"))
}

fn download_file(url: &str, output: &Path) -> Result<(), String> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }

    if let Some(curl) = find_program_on_path("curl") {
        let mut command = std::process::Command::new(curl);
        command
            .arg("--fail")
            .arg("--location")
            .arg("--show-error")
            .arg("--output")
            .arg(output)
            .arg(url);
        return run_checked_command(&mut command, "downloading toolchain file");
    }

    if let Some(wget) = find_program_on_path("wget") {
        let mut command = std::process::Command::new(wget);
        command.arg("-O").arg(output).arg(url);
        return run_checked_command(&mut command, "downloading toolchain file");
    }

    Err("Android build bootstrap requires curl or wget on PATH".to_string())
}

fn prepend_path(command: &mut std::process::Command, extra: &Path) -> Result<(), String> {
    let mut paths = vec![extra.to_path_buf()];
    if let Some(existing) = env::var_os("PATH") {
        paths.extend(env::split_paths(&existing));
    }
    let joined =
        env::join_paths(paths).map_err(|error| format!("failed to construct PATH: {error}"))?;
    command.env("PATH", joined);
    Ok(())
}

fn apply_java_env(command: &mut std::process::Command, java_home: &Path) -> Result<(), String> {
    command.env("JAVA_HOME", java_home);
    prepend_path(command, &java_home.join("bin"))
}

fn ensure_jdk() -> Result<PathBuf, String> {
    if let Some(java_home) = find_java_home() {
        return Ok(java_home);
    }

    let root = toolchains_root()?.join("jdk");
    if let Some(java_home) = find_java_home_under(&root) {
        return Ok(java_home);
    }

    if cfg!(not(all(target_os = "linux", target_arch = "x86_64"))) {
        return Err(
            "Java was not found. Set JAVA_HOME to a JDK 17+ installation before building Android APKs."
                .to_string(),
        );
    }

    fs::create_dir_all(&root)
        .map_err(|error| format!("failed to create JDK directory {}: {error}", root.display()))?;
    let archive = std::env::temp_dir().join("neolove-jdk-linux-x64.tar.gz");
    println!("Installing user-local JDK for Android APK signing...");
    download_file(JDK_LINUX_X64_URL, &archive)?;
    let mut tar = std::process::Command::new("tar");
    tar.arg("-xzf").arg(&archive).arg("-C").arg(&root);
    run_checked_command(&mut tar, "extracting JDK")?;

    find_java_home_under(&root).ok_or_else(|| {
        format!(
            "JDK archive was extracted, but no bin/java was found under {}",
            root.display()
        )
    })
}

fn configured_android_sdk_root() -> Result<PathBuf, String> {
    if let Some(root) = env::var_os("ANDROID_HOME")
        .or_else(|| env::var_os("ANDROID_SDK_ROOT"))
        .filter(|value| !value.is_empty())
    {
        return Ok(PathBuf::from(root));
    }
    Ok(toolchains_root()?.join("android-sdk"))
}

fn sdkmanager_path(sdk_root: &Path) -> PathBuf {
    sdk_root
        .join("cmdline-tools")
        .join("latest")
        .join("bin")
        .join(script_name("sdkmanager"))
}

fn ensure_android_cmdline_tools(sdk_root: &Path) -> Result<PathBuf, String> {
    let sdkmanager = sdkmanager_path(sdk_root);
    if sdkmanager.is_file() {
        return Ok(sdkmanager);
    }

    if cfg!(not(all(target_os = "linux", target_arch = "x86_64"))) {
        return Err(format!(
            "Android command-line tools were not found at {}. Install them or set ANDROID_HOME.",
            sdkmanager.display()
        ));
    }

    println!("Installing Android command-line tools...");
    let tools_root = sdk_root.join("cmdline-tools");
    let latest = tools_root.join("latest");
    let staging = tools_root.join(".latest-staging");
    recreate_dir(&staging)?;
    fs::create_dir_all(&tools_root).map_err(|error| {
        format!(
            "failed to create Android cmdline-tools directory {}: {error}",
            tools_root.display()
        )
    })?;

    let archive = std::env::temp_dir().join("neolove-android-cmdline-tools.zip");
    download_file(ANDROID_CMDLINE_TOOLS_URL, &archive)?;
    let unzip = find_program_on_path("unzip")
        .ok_or_else(|| "Android build bootstrap requires unzip on PATH".to_string())?;
    let mut command = std::process::Command::new(unzip);
    command
        .arg("-q")
        .arg("-o")
        .arg(&archive)
        .arg("-d")
        .arg(&staging);
    run_checked_command(&mut command, "extracting Android command-line tools")?;

    if latest.exists() {
        fs::remove_dir_all(&latest)
            .map_err(|error| format!("failed to clean {}: {error}", latest.display()))?;
    }
    let extracted = staging.join("cmdline-tools");
    fs::rename(&extracted, &latest).map_err(|error| {
        format!(
            "failed to move Android command-line tools {} -> {}: {error}",
            extracted.display(),
            latest.display()
        )
    })?;
    fs::remove_dir_all(&staging)
        .map_err(|error| format!("failed to clean {}: {error}", staging.display()))?;

    if !sdkmanager.is_file() {
        return Err(format!(
            "Android command-line tools install completed, but sdkmanager was not found at {}",
            sdkmanager.display()
        ));
    }
    Ok(sdkmanager)
}

fn accept_android_licenses(
    sdkmanager: &Path,
    sdk_root: &Path,
    java_home: &Path,
) -> Result<(), String> {
    let mut command = std::process::Command::new(sdkmanager);
    apply_java_env(&mut command, java_home)?;
    command
        .arg(format!("--sdk_root={}", sdk_root.display()))
        .arg("--licenses")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null());

    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to launch sdkmanager --licenses: {error}"))?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all("y\n".repeat(64).as_bytes())
            .map_err(|error| format!("failed to accept Android SDK licenses: {error}"))?;
    }
    let status = child
        .wait()
        .map_err(|error| format!("failed while accepting Android SDK licenses: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("sdkmanager --licenses failed with status {status}"))
    }
}

fn run_sdkmanager_install(
    sdkmanager: &Path,
    sdk_root: &Path,
    java_home: &Path,
    packages: &[&str],
) -> Result<(), String> {
    let mut command = std::process::Command::new(sdkmanager);
    apply_java_env(&mut command, java_home)?;
    command
        .arg(format!("--sdk_root={}", sdk_root.display()))
        .arg("--install");
    command.args(packages);
    run_checked_command(&mut command, "installing Android SDK packages")
}

fn android_host_tag() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows-x86_64"
    } else if cfg!(target_os = "macos") {
        "darwin-x86_64"
    } else {
        "linux-x86_64"
    }
}

fn android_build_tools_tool(sdk_root: &Path, tool: &str, script: bool) -> PathBuf {
    sdk_root
        .join("build-tools")
        .join(ANDROID_BUILD_TOOLS_VERSION)
        .join(if script {
            script_name(tool)
        } else {
            executable_name(tool)
        })
}

fn ensure_android_toolchain() -> Result<AndroidToolchain, String> {
    let java_home = ensure_jdk()?;
    let sdk_root = configured_android_sdk_root()?;
    let sdkmanager = ensure_android_cmdline_tools(&sdk_root)?;
    accept_android_licenses(&sdkmanager, &sdk_root, &java_home)?;
    run_sdkmanager_install(
        &sdkmanager,
        &sdk_root,
        &java_home,
        &[
            "platform-tools",
            &format!("platforms;android-{ANDROID_API_LEVEL}"),
            &format!("build-tools;{ANDROID_BUILD_TOOLS_VERSION}"),
            &format!("ndk;{ANDROID_NDK_VERSION}"),
        ],
    )?;

    let ndk_root = sdk_root.join("ndk").join(ANDROID_NDK_VERSION);
    let llvm_bin = ndk_root
        .join("toolchains")
        .join("llvm")
        .join("prebuilt")
        .join(android_host_tag())
        .join("bin");
    let clang = llvm_bin.join(if cfg!(windows) {
        format!("{ANDROID_RUST_TARGET}{ANDROID_MIN_SDK}-clang.cmd")
    } else {
        format!("{ANDROID_RUST_TARGET}{ANDROID_MIN_SDK}-clang")
    });
    let clangxx = llvm_bin.join(if cfg!(windows) {
        format!("{ANDROID_RUST_TARGET}{ANDROID_MIN_SDK}-clang++.cmd")
    } else {
        format!("{ANDROID_RUST_TARGET}{ANDROID_MIN_SDK}-clang++")
    });
    let llvm_ar = llvm_bin.join(executable_name("llvm-ar"));
    let llvm_strip = llvm_bin.join(executable_name("llvm-strip"));

    let toolchain = AndroidToolchain {
        java_home: java_home.clone(),
        aapt2: android_build_tools_tool(&sdk_root, "aapt2", false),
        zipalign: android_build_tools_tool(&sdk_root, "zipalign", false),
        apksigner: android_build_tools_tool(&sdk_root, "apksigner", true),
        android_jar: sdk_root
            .join("platforms")
            .join(format!("android-{ANDROID_API_LEVEL}"))
            .join("android.jar"),
        clang,
        clangxx,
        llvm_ar,
        llvm_strip,
        keytool: java_home.join("bin").join(executable_name("keytool")),
    };

    for path in [
        &toolchain.aapt2,
        &toolchain.zipalign,
        &toolchain.apksigner,
        &toolchain.android_jar,
        &toolchain.clang,
        &toolchain.clangxx,
        &toolchain.llvm_ar,
        &toolchain.llvm_strip,
        &toolchain.keytool,
    ] {
        if !path.is_file() {
            return Err(format!(
                "Android toolchain setup completed, but required tool is missing: {}",
                path.display()
            ));
        }
    }

    Ok(toolchain)
}

fn valid_android_package_name(value: &str) -> bool {
    let parts: Vec<&str> = value.split('.').collect();
    parts.len() >= 2
        && parts.iter().all(|part| {
            let mut chars = part.chars();
            matches!(chars.next(), Some(ch) if ch.is_ascii_alphabetic() || ch == '_')
                && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        })
}

fn android_identifier_segment(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if ch == '_' || ch == '-' || ch.is_whitespace() {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push_str("game");
    }
    if out.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        out.insert(0, 'g');
    }
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    out.trim_matches('_').to_string().if_empty("game")
}

trait IfEmpty {
    fn if_empty(self, fallback: &str) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_string()
        } else {
            self
        }
    }
}

fn android_package_name(project_root: &Path) -> String {
    let settings = parse_project_settings(project_root);
    if let Some(name) = settings.package_name.as_deref() {
        let lower = name.to_ascii_lowercase();
        if valid_android_package_name(&lower) {
            return lower;
        }
    }
    format!(
        "com.neolove.{}",
        android_identifier_segment(&project_output_stem(project_root))
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn android_manifest(project_root: &Path) -> String {
    let settings = parse_project_settings(project_root);
    let package_name = android_package_name(project_root);
    let label = settings
        .window_title
        .or(settings.package_name)
        .unwrap_or_else(|| project_output_stem(project_root));
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android"
    package="{package_name}"
    android:versionCode="1"
    android:versionName="0.1.0">
    <uses-sdk android:minSdkVersion="{ANDROID_MIN_SDK}" android:targetSdkVersion="{ANDROID_API_LEVEL}" />
    <uses-permission android:name="android.permission.INTERNET" />
    <application
        android:label="{label}"
        android:allowBackup="false"
        android:hasCode="false"
        android:extractNativeLibs="true"
        android:theme="@android:style/Theme.NoTitleBar.Fullscreen">
        <activity
            android:name="android.app.NativeActivity"
            android:exported="true"
            android:configChanges="keyboard|keyboardHidden|orientation|screenLayout|screenSize|smallestScreenSize|uiMode"
            android:screenOrientation="sensorLandscape">
            <meta-data android:name="android.app.lib_name" android:value="neolove" />
            <intent-filter>
                <action android:name="android.intent.action.MAIN" />
                <category android:name="android.intent.category.LAUNCHER" />
            </intent-filter>
        </activity>
    </application>
</manifest>
"#,
        package_name = xml_escape(&package_name),
        label = xml_escape(&label),
    )
}

fn ensure_android_debug_keystore(toolchain: &AndroidToolchain) -> Result<PathBuf, String> {
    let keystore = toolchains_root()?.join("android").join("debug.keystore");
    if keystore.is_file() {
        return Ok(keystore);
    }
    if let Some(parent) = keystore.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    let mut command = std::process::Command::new(&toolchain.keytool);
    apply_java_env(&mut command, &toolchain.java_home)?;
    command
        .arg("-genkeypair")
        .arg("-keystore")
        .arg(&keystore)
        .arg("-storepass")
        .arg("android")
        .arg("-keypass")
        .arg("android")
        .arg("-alias")
        .arg("androiddebugkey")
        .arg("-keyalg")
        .arg("RSA")
        .arg("-keysize")
        .arg("2048")
        .arg("-validity")
        .arg("10000")
        .arg("-dname")
        .arg("CN=Android Debug,O=NeoLOVE,C=US");
    run_checked_command(&mut command, "creating Android debug keystore")?;
    Ok(keystore)
}

fn build_android(project_root: &Path) -> Result<PathBuf, String> {
    let output_stem = project_output_stem(project_root);
    let output_dir = project_root.join("dist");
    fs::create_dir_all(&output_dir).map_err(|error| {
        format!(
            "failed to create dist directory {}: {error}",
            output_dir.display()
        )
    })?;

    let toolchain = ensure_android_toolchain()?;
    println!("Ensuring Rust Android target is installed...");
    let mut rustup = std::process::Command::new("rustup");
    rustup.args(["target", "add", ANDROID_RUST_TARGET]);
    run_checked_command(&mut rustup, "installing Android Rust target")?;

    let payload = build_payload(project_root)?;
    let engine_root = engine_source_root()?;
    let cargo_target_dir = engine_root.join("target").join("android-aarch64");

    println!("Building NeoLOVE Android runtime...");
    let mut cargo = std::process::Command::new("cargo");
    apply_size_optimized_release_env(&mut cargo);
    cargo
        .env("CARGO_TARGET_DIR", &cargo_target_dir)
        .env(
            "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER",
            &toolchain.clang,
        )
        .env("CC_aarch64_linux_android", &toolchain.clang)
        .env("CXX_aarch64_linux_android", &toolchain.clangxx)
        .env("AR_aarch64_linux_android", &toolchain.llvm_ar)
        .arg("build")
        .arg("--release")
        .arg("--lib")
        .arg("--target")
        .arg(ANDROID_RUST_TARGET)
        .current_dir(&engine_root);
    run_checked_command_quiet(&mut cargo, "building Android runtime")?;

    let built_library = cargo_target_dir
        .join(ANDROID_RUST_TARGET)
        .join("release")
        .join(format!("lib{}.so", env!("CARGO_PKG_NAME")));
    if !built_library.is_file() {
        return Err(format!(
            "Android runtime build succeeded but output was not found: {}",
            built_library.display()
        ));
    }

    let stage_dir = output_dir.join(".android-stage");
    recreate_dir(&stage_dir)?;
    let lib_dir = stage_dir.join("lib").join(ANDROID_ABI);
    let assets_dir = stage_dir.join("assets");
    fs::create_dir_all(&lib_dir)
        .map_err(|error| format!("failed to create {}: {error}", lib_dir.display()))?;
    fs::create_dir_all(&assets_dir)
        .map_err(|error| format!("failed to create {}: {error}", assets_dir.display()))?;

    let staged_library = lib_dir.join("libneolove.so");
    fs::copy(&built_library, &staged_library).map_err(|error| {
        format!(
            "failed to stage Android runtime {} -> {}: {error}",
            built_library.display(),
            staged_library.display()
        )
    })?;
    let mut strip = std::process::Command::new(&toolchain.llvm_strip);
    strip.arg("--strip-unneeded").arg(&staged_library);
    run_checked_command(&mut strip, "stripping Android runtime")?;

    fs::write(assets_dir.join(ANDROID_PAYLOAD_ASSET), payload)
        .map_err(|error| format!("failed to stage Android project payload: {error}"))?;
    let manifest_path = stage_dir.join("AndroidManifest.xml");
    fs::write(&manifest_path, android_manifest(project_root))
        .map_err(|error| format!("failed to write Android manifest: {error}"))?;

    let linked_apk = stage_dir.join("linked.apk");
    let mut aapt2 = std::process::Command::new(&toolchain.aapt2);
    aapt2
        .arg("link")
        .arg("--manifest")
        .arg(&manifest_path)
        .arg("-I")
        .arg(&toolchain.android_jar)
        .arg("--min-sdk-version")
        .arg(ANDROID_MIN_SDK)
        .arg("--target-sdk-version")
        .arg(ANDROID_API_LEVEL)
        .arg("-o")
        .arg(&linked_apk);
    run_checked_command_quiet(&mut aapt2, "linking Android APK manifest")?;

    let zip = find_program_on_path("zip")
        .ok_or_else(|| "Android APK packaging requires zip on PATH".to_string())?;
    let mut zip_command = std::process::Command::new(zip);
    zip_command
        .current_dir(&stage_dir)
        .arg("-q")
        .arg("-0")
        .arg(&linked_apk)
        .arg(format!("assets/{ANDROID_PAYLOAD_ASSET}"))
        .arg(format!("lib/{ANDROID_ABI}/libneolove.so"));
    run_checked_command(&mut zip_command, "adding Android assets to APK")?;

    let aligned_apk = stage_dir.join("aligned.apk");
    let mut zipalign = std::process::Command::new(&toolchain.zipalign);
    zipalign
        .arg("-f")
        .arg("-p")
        .arg("4")
        .arg(&linked_apk)
        .arg(&aligned_apk);
    run_checked_command(&mut zipalign, "aligning Android APK")?;

    let final_apk = output_dir.join(format!("{output_stem}-android-arm64.apk"));
    let keystore = ensure_android_debug_keystore(&toolchain)?;
    let mut apksigner = std::process::Command::new(&toolchain.apksigner);
    apply_java_env(&mut apksigner, &toolchain.java_home)?;
    apksigner
        .arg("sign")
        .arg("--ks")
        .arg(&keystore)
        .arg("--ks-pass")
        .arg("pass:android")
        .arg("--key-pass")
        .arg("pass:android")
        .arg("--out")
        .arg(&final_apk)
        .arg(&aligned_apk);
    run_checked_command_quiet(&mut apksigner, "signing Android APK")?;

    let _ = fs::remove_dir_all(&stage_dir);
    Ok(final_apk)
}

fn ios_product_name(project_root: &Path) -> String {
    let stem = project_output_stem(project_root);
    let mut out = String::new();
    for ch in stem.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if ch == '_' || ch == '-' || ch.is_whitespace() {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push_str("NeoLOVEGame");
    }
    if out.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        out.insert_str(0, "NeoLOVE");
    }
    out
}

fn ios_info_plist(project_root: &Path, product_name: &str, bundle_id: &str) -> String {
    let settings = parse_project_settings(project_root);
    let label = settings
        .window_title
        .or(settings.package_name)
        .unwrap_or_else(|| project_output_stem(project_root));
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "https://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>$(DEVELOPMENT_LANGUAGE)</string>
    <key>CFBundleDisplayName</key>
    <string>{}</string>
    <key>CFBundleExecutable</key>
    <string>$(EXECUTABLE_NAME)</string>
    <key>CFBundleIdentifier</key>
    <string>{}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>{}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSRequiresIPhoneOS</key>
    <true/>
    <key>NSAppTransportSecurity</key>
    <dict>
        <key>NSAllowsLocalNetworking</key>
        <true/>
    </dict>
    <key>NSCameraUsageDescription</key>
    <string>This game can request camera access for gameplay features.</string>
    <key>NSMicrophoneUsageDescription</key>
    <string>This game can request microphone access for gameplay features.</string>
    <key>UIRequiresFullScreen</key>
    <true/>
    <key>UISupportedInterfaceOrientations</key>
    <array>
        <string>UIInterfaceOrientationPortrait</string>
        <string>UIInterfaceOrientationLandscapeLeft</string>
        <string>UIInterfaceOrientationLandscapeRight</string>
    </array>
</dict>
</plist>
"#,
        xml_escape(&label),
        xml_escape(bundle_id),
        xml_escape(product_name)
    )
}

fn ios_app_delegate_source() -> &'static str {
    r#"import UIKit

@main
final class AppDelegate: UIResponder, UIApplicationDelegate {
    var window: UIWindow?

    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
    ) -> Bool {
        let window = UIWindow(frame: UIScreen.main.bounds)
        window.rootViewController = ViewController()
        window.makeKeyAndVisible()
        self.window = window
        return true
    }
}
"#
}

fn ios_view_controller_source() -> &'static str {
    r#"import UIKit
import WebKit

final class ViewController: UIViewController, WKUIDelegate {
    private var webView: WKWebView!
    private var server: LocalWebServer?

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black

        let configuration = WKWebViewConfiguration()
        configuration.allowsInlineMediaPlayback = true
        if #available(iOS 10.0, *) {
            configuration.mediaTypesRequiringUserActionForPlayback = []
        }

        let webView = WKWebView(frame: view.bounds, configuration: configuration)
        webView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        webView.isOpaque = false
        webView.backgroundColor = .black
        view.addSubview(webView)
        self.webView = webView
        webView.uiDelegate = self

        do {
            guard let root = Bundle.main.resourceURL?.appendingPathComponent("webasm", isDirectory: true) else {
                throw LocalWebServer.Error.missingBundle
            }
            let server = try LocalWebServer(root: root)
            self.server = server
            let url = try server.start()
            webView.load(URLRequest(url: url.appendingPathComponent("index.html")))
        } catch {
            let escaped = String(describing: error)
                .replacingOccurrences(of: "&", with: "&amp;")
                .replacingOccurrences(of: "<", with: "&lt;")
                .replacingOccurrences(of: ">", with: "&gt;")
            let message = "<html><body style='font: -apple-system-body; padding: 24px'><h1>NeoLOVE failed to start</h1><p>\(escaped)</p></body></html>"
            webView.loadHTMLString(message, baseURL: nil)
        }
    }

    @available(iOS 15.0, *)
    func webView(
        _ webView: WKWebView,
        requestMediaCapturePermissionFor origin: WKSecurityOrigin,
        initiatedByFrame frame: WKFrameInfo,
        type: WKMediaCaptureType,
        decisionHandler: @escaping (WKPermissionDecision) -> Void
    ) {
        // Keep the OS/browser prompt in control; never silently grant access.
        decisionHandler(.prompt)
    }
}
"#
}

fn ios_local_web_server_source() -> &'static str {
    r#"import Foundation
import Network

final class LocalWebServer {
    enum Error: Swift.Error {
        case missingBundle
        case missingPort
    }

    private let root: URL
    private let queue = DispatchQueue(label: "NeoLOVE.LocalWebServer")
    private var listener: NWListener?

    init(root: URL) throws {
        self.root = root
        self.listener = try NWListener(using: .tcp, on: .any)
    }

    func start() throws -> URL {
        guard let listener = listener else { throw Error.missingPort }
        listener.newConnectionHandler = { [weak self] connection in
            self?.handle(connection)
        }
        listener.start(queue: queue)
        guard let port = listener.port else { throw Error.missingPort }
        return URL(string: "http://127.0.0.1:\(port.rawValue)")!
    }

    private func handle(_ connection: NWConnection) {
        connection.start(queue: queue)
        connection.receive(minimumIncompleteLength: 1, maximumLength: 65536) { [weak self] data, _, _, _ in
            guard let self = self else { return }
            let response = self.response(for: data ?? Data())
            connection.send(content: response, completion: .contentProcessed { _ in
                connection.cancel()
            })
        }
    }

    private func response(for requestData: Data) -> Data {
        let request = String(decoding: requestData, as: UTF8.self)
        let firstLine = request.split(separator: "\r\n", maxSplits: 1).first ?? ""
        let parts = firstLine.split(separator: " ")
        let rawPath = parts.count >= 2 ? String(parts[1]) : "/index.html"
        let cleanPath = sanitize(rawPath)
        let fileURL = root.appendingPathComponent(cleanPath)
        guard fileURL.path.hasPrefix(root.path), let body = try? Data(contentsOf: fileURL) else {
            return http(status: "404 Not Found", mime: "text/plain; charset=utf-8", body: Data("Not Found".utf8))
        }
        return http(status: "200 OK", mime: mime(for: fileURL), body: body)
    }

    private func sanitize(_ rawPath: String) -> String {
        let path = rawPath.split(separator: "?", maxSplits: 1).first.map(String.init) ?? rawPath
        let decoded = path.removingPercentEncoding ?? path
        let trimmed = decoded.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        let parts = trimmed.split(separator: "/").filter { $0 != "." && $0 != ".." }
        return parts.isEmpty ? "index.html" : parts.joined(separator: "/")
    }

    private func http(status: String, mime: String, body: Data) -> Data {
        var header = "HTTP/1.1 \(status)\r\n"
        header += "Content-Type: \(mime)\r\n"
        header += "Content-Length: \(body.count)\r\n"
        header += "Cache-Control: no-store\r\n"
        header += "Connection: close\r\n\r\n"
        var data = Data(header.utf8)
        data.append(body)
        return data
    }

    private func mime(for url: URL) -> String {
        switch url.pathExtension.lowercased() {
        case "html": return "text/html; charset=utf-8"
        case "js": return "application/javascript; charset=utf-8"
        case "wasm": return "application/wasm"
        case "data": return "application/octet-stream"
        case "png": return "image/png"
        case "jpg", "jpeg": return "image/jpeg"
        case "gif": return "image/gif"
        case "svg": return "image/svg+xml"
        case "css": return "text/css; charset=utf-8"
        default: return "application/octet-stream"
        }
    }
}
"#
}

fn ios_pbxproj(product_name: &str, bundle_id: &str) -> String {
    r#"// !$*UTF8*$!
{
    archiveVersion = 1;
    classes = {};
    objectVersion = 56;
    objects = {
        A00000000000000000000001 /* AppDelegate.swift in Sources */ = {isa = PBXBuildFile; fileRef = A00000000000000000000011 /* AppDelegate.swift */; };
        A00000000000000000000002 /* ViewController.swift in Sources */ = {isa = PBXBuildFile; fileRef = A00000000000000000000012 /* ViewController.swift */; };
        A00000000000000000000003 /* LocalWebServer.swift in Sources */ = {isa = PBXBuildFile; fileRef = A00000000000000000000013 /* LocalWebServer.swift */; };
        A00000000000000000000004 /* webasm in Resources */ = {isa = PBXBuildFile; fileRef = A00000000000000000000015 /* webasm */; };
        A00000000000000000000010 /* __PRODUCT_NAME__.app */ = {isa = PBXFileReference; explicitFileType = wrapper.application; includeInIndex = 0; path = "__PRODUCT_NAME__.app"; sourceTree = BUILT_PRODUCTS_DIR; };
        A00000000000000000000011 /* AppDelegate.swift */ = {isa = PBXFileReference; lastKnownFileType = sourcecode.swift; path = AppDelegate.swift; sourceTree = "<group>"; };
        A00000000000000000000012 /* ViewController.swift */ = {isa = PBXFileReference; lastKnownFileType = sourcecode.swift; path = ViewController.swift; sourceTree = "<group>"; };
        A00000000000000000000013 /* LocalWebServer.swift */ = {isa = PBXFileReference; lastKnownFileType = sourcecode.swift; path = LocalWebServer.swift; sourceTree = "<group>"; };
        A00000000000000000000014 /* Info.plist */ = {isa = PBXFileReference; lastKnownFileType = text.plist.xml; path = Info.plist; sourceTree = "<group>"; };
        A00000000000000000000015 /* webasm */ = {isa = PBXFileReference; lastKnownFileType = folder; path = webasm; sourceTree = "<group>"; };
        A00000000000000000000020 /* Frameworks */ = {isa = PBXFrameworksBuildPhase; buildActionMask = 2147483647; files = (); runOnlyForDeploymentPostprocessing = 0; };
        A00000000000000000000030 = {isa = PBXGroup; children = (A00000000000000000000031 /* __PRODUCT_NAME__ */, A00000000000000000000032 /* Products */); sourceTree = "<group>"; };
        A00000000000000000000031 /* __PRODUCT_NAME__ */ = {isa = PBXGroup; children = (A00000000000000000000011, A00000000000000000000012, A00000000000000000000013, A00000000000000000000014, A00000000000000000000015); path = __PRODUCT_NAME__; sourceTree = "<group>"; };
        A00000000000000000000032 /* Products */ = {isa = PBXGroup; children = (A00000000000000000000010); name = Products; sourceTree = "<group>"; };
        A00000000000000000000040 /* __PRODUCT_NAME__ */ = {isa = PBXNativeTarget; buildConfigurationList = A00000000000000000000070; buildPhases = (A00000000000000000000050, A00000000000000000000060, A00000000000000000000020); buildRules = (); dependencies = (); name = __PRODUCT_NAME__; productName = __PRODUCT_NAME__; productReference = A00000000000000000000010 /* __PRODUCT_NAME__.app */; productType = "com.apple.product-type.application"; };
        A00000000000000000000041 /* Project object */ = {isa = PBXProject; attributes = {BuildIndependentTargetsInParallel = 1; LastSwiftUpdateCheck = 1600; LastUpgradeCheck = 1600; TargetAttributes = {A00000000000000000000040 = {CreatedOnToolsVersion = 16.0; }; }; }; buildConfigurationList = A00000000000000000000080; compatibilityVersion = "Xcode 14.0"; developmentRegion = en; hasScannedForEncodings = 0; knownRegions = (en, Base); mainGroup = A00000000000000000000030; productRefGroup = A00000000000000000000032; projectDirPath = ""; projectRoot = ""; targets = (A00000000000000000000040); };
        A00000000000000000000050 /* Sources */ = {isa = PBXSourcesBuildPhase; buildActionMask = 2147483647; files = (A00000000000000000000001, A00000000000000000000002, A00000000000000000000003); runOnlyForDeploymentPostprocessing = 0; };
        A00000000000000000000060 /* Resources */ = {isa = PBXResourcesBuildPhase; buildActionMask = 2147483647; files = (A00000000000000000000004); runOnlyForDeploymentPostprocessing = 0; };
        A00000000000000000000070 = {isa = XCConfigurationList; buildConfigurations = (A00000000000000000000071, A00000000000000000000072); defaultConfigurationIsVisible = 0; defaultConfigurationName = Release; };
        A00000000000000000000071 /* Debug */ = {isa = XCBuildConfiguration; buildSettings = {INFOPLIST_FILE = "__PRODUCT_NAME__/Info.plist"; PRODUCT_BUNDLE_IDENTIFIER = __BUNDLE_ID__; PRODUCT_NAME = "$(TARGET_NAME)"; SWIFT_VERSION = 5.0; IPHONEOS_DEPLOYMENT_TARGET = 14.0; TARGETED_DEVICE_FAMILY = "1,2"; CODE_SIGN_STYLE = Automatic; }; name = Debug; };
        A00000000000000000000072 /* Release */ = {isa = XCBuildConfiguration; buildSettings = {INFOPLIST_FILE = "__PRODUCT_NAME__/Info.plist"; PRODUCT_BUNDLE_IDENTIFIER = __BUNDLE_ID__; PRODUCT_NAME = "$(TARGET_NAME)"; SWIFT_VERSION = 5.0; IPHONEOS_DEPLOYMENT_TARGET = 14.0; TARGETED_DEVICE_FAMILY = "1,2"; CODE_SIGN_STYLE = Automatic; }; name = Release; };
        A00000000000000000000080 = {isa = XCConfigurationList; buildConfigurations = (A00000000000000000000081, A00000000000000000000082); defaultConfigurationIsVisible = 0; defaultConfigurationName = Release; };
        A00000000000000000000081 /* Debug */ = {isa = XCBuildConfiguration; buildSettings = {ALWAYS_SEARCH_USER_PATHS = NO; CLANG_ENABLE_MODULES = YES; CLANG_ENABLE_OBJC_ARC = YES; DEBUG_INFORMATION_FORMAT = dwarf; GCC_C_LANGUAGE_STANDARD = gnu17; GCC_OPTIMIZATION_LEVEL = 0; SDKROOT = iphoneos; SWIFT_OPTIMIZATION_LEVEL = "-Onone"; }; name = Debug; };
        A00000000000000000000082 /* Release */ = {isa = XCBuildConfiguration; buildSettings = {ALWAYS_SEARCH_USER_PATHS = NO; CLANG_ENABLE_MODULES = YES; CLANG_ENABLE_OBJC_ARC = YES; DEBUG_INFORMATION_FORMAT = "dwarf-with-dsym"; ENABLE_NS_ASSERTIONS = NO; GCC_C_LANGUAGE_STANDARD = gnu17; SDKROOT = iphoneos; SWIFT_COMPILATION_MODE = wholemodule; SWIFT_OPTIMIZATION_LEVEL = "-O"; VALIDATE_PRODUCT = YES; }; name = Release; };
    };
    rootObject = A00000000000000000000041 /* Project object */;
}
"#
    .replace("__PRODUCT_NAME__", product_name)
    .replace("__BUNDLE_ID__", bundle_id)
}

fn build_ios(project_root: &Path) -> Result<PathBuf, String> {
    if !cfg!(target_os = "macos") {
        return Err(
            "iOS builds require macOS with Xcode installed. Use `neolove build --ios` on a Mac."
                .to_string(),
        );
    }

    let output_stem = project_output_stem(project_root);
    let output_dir = project_root.join("dist");
    fs::create_dir_all(&output_dir).map_err(|error| {
        format!(
            "failed to create dist directory {}: {error}",
            output_dir.display()
        )
    })?;

    let (web_bundle, _) = build_webasm(project_root)?;
    let product_name = ios_product_name(project_root);
    let bundle_id = android_package_name(project_root);
    let ios_dir = output_dir.join("ios");
    recreate_dir(&ios_dir)?;
    let source_dir = ios_dir.join(&product_name);
    fs::create_dir_all(&source_dir)
        .map_err(|error| format!("failed to create iOS source directory: {error}"))?;
    copy_dir_recursive(&web_bundle, &source_dir.join("webasm"))?;
    fs::write(
        source_dir.join("AppDelegate.swift"),
        ios_app_delegate_source(),
    )
    .map_err(|error| format!("failed to write iOS AppDelegate.swift: {error}"))?;
    fs::write(
        source_dir.join("ViewController.swift"),
        ios_view_controller_source(),
    )
    .map_err(|error| format!("failed to write iOS ViewController.swift: {error}"))?;
    fs::write(
        source_dir.join("LocalWebServer.swift"),
        ios_local_web_server_source(),
    )
    .map_err(|error| format!("failed to write iOS LocalWebServer.swift: {error}"))?;
    fs::write(
        source_dir.join("Info.plist"),
        ios_info_plist(project_root, &product_name, &bundle_id),
    )
    .map_err(|error| format!("failed to write iOS Info.plist: {error}"))?;

    let xcodeproj = ios_dir.join(format!("{product_name}.xcodeproj"));
    fs::create_dir_all(&xcodeproj)
        .map_err(|error| format!("failed to create {}: {error}", xcodeproj.display()))?;
    fs::write(
        xcodeproj.join("project.pbxproj"),
        ios_pbxproj(&product_name, &bundle_id),
    )
    .map_err(|error| format!("failed to write iOS Xcode project: {error}"))?;

    let xcodebuild = find_program_on_path("xcodebuild")
        .ok_or_else(|| "iOS builds require xcodebuild on PATH".to_string())?;
    let derived_data = ios_dir.join("DerivedData");
    let mut command = std::process::Command::new(xcodebuild);
    command
        .arg("-project")
        .arg(&xcodeproj)
        .arg("-target")
        .arg(&product_name)
        .arg("-configuration")
        .arg("Release")
        .arg("-sdk")
        .arg("iphonesimulator")
        .arg("-derivedDataPath")
        .arg(&derived_data)
        .arg("CODE_SIGNING_ALLOWED=NO")
        .arg("build");
    run_checked_command_quiet(&mut command, "building iOS simulator app with Xcode")?;

    let built_app = derived_data
        .join("Build")
        .join("Products")
        .join("Release-iphonesimulator")
        .join(format!("{product_name}.app"));
    if !built_app.is_dir() {
        return Err(format!(
            "iOS build succeeded but app bundle was not found: {}",
            built_app.display()
        ));
    }

    let output_app = output_dir.join(format!("{output_stem}-ios-simulator.app"));
    if output_app.exists() {
        fs::remove_dir_all(&output_app)
            .map_err(|error| format!("failed to replace {}: {error}", output_app.display()))?;
    }
    copy_dir_recursive(&built_app, &output_app)?;
    Ok(output_app)
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

fn frame_deadline(frame_started: Instant, now: Instant, max_fps: Option<f32>) -> Option<Instant> {
    let fps = max_fps.filter(|fps| fps.is_finite() && *fps > 0.0)?;
    let target = Duration::from_secs_f32(1.0 / fps.max(1.0));
    let deadline = frame_started + target;
    (deadline > now).then_some(deadline)
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
    let caption = if caption.is_empty() {
        "NeoLOVE"
    } else {
        caption
    };
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

#[cfg(not(neolove_packaged))]
fn encode_editor_runtime_frame(
    serial: u64,
    width: u32,
    height: u32,
    pixels: &[u8],
    backend: &str,
    fps: f32,
    update_ms: f32,
    render_ms: f32,
    draw_calls: u32,
    triangles: u64,
) -> Result<editor_ipc::RuntimeFrame, String> {
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(pixels, width, height, image::ColorType::Rgba8)
        .map_err(|error| format!("encode embedded runtime frame: {error}"))?;
    Ok(editor_ipc::RuntimeFrame {
        serial,
        width,
        height,
        png_base64: base64::engine::general_purpose::STANDARD.encode(png),
        backend: backend.to_string(),
        fps,
        update_ms,
        render_ms,
        draw_calls,
        triangles,
    })
}

#[cfg(not(neolove_packaged))]
fn runtime_frame_draw_stats(
    render_state: &crate::renderer::SharedRenderState,
) -> (u32, u64) {
    let commands = crate::renderer::last_frame_commands(render_state)
        .ok()
        .flatten();
    let Some(commands) = commands else {
        return (0, 0);
    };
    let draw_calls = u32::try_from(commands.len()).unwrap_or(u32::MAX);
    let triangles = commands
        .iter()
        .filter_map(|command| match command {
            crate::renderer::DrawCommand::Mesh3D(command) => command
                .mesh
                .with_read(|mesh, _| mesh.indices.len() as u64 / 3)
                .ok(),
            crate::renderer::DrawCommand::Triangle { .. } => Some(1),
            crate::renderer::DrawCommand::Rect { .. }
            | crate::renderer::DrawCommand::Image { .. } => Some(2),
            _ => None,
        })
        .sum();
    (draw_calls, triangles)
}

#[cfg(not(neolove_packaged))]
fn runtime_error_log(message: String) -> crate::window::RuntimeLogLine {
    let marker = |name: &str| {
        let start = message.find(&format!("{name}="))? + name.len() + 1;
        let digits = message[start..]
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .collect::<String>();
        (!digits.is_empty()).then(|| digits.parse::<usize>().ok()).flatten()
    };
    let component = message
        .find("component=")
        .map(|start| start + "component=".len())
        .and_then(|start| {
            let value = message[start..]
                .chars()
                .take_while(|character| {
                    !character.is_whitespace() && !matches!(character, ']' | ':' | ',')
                })
                .collect::<String>();
            (!value.is_empty()).then_some(value)
        })
        .or_else(|| {
            let start = message.find("component '")? + "component '".len();
            let end = message[start..].find('\'')? + start;
            Some(message[start..end].to_string())
        });
    let (script, line) = message
        .split_whitespace()
        .filter_map(|token| {
            let token = token.trim_matches(|character: char| {
                matches!(character, '[' | ']' | '(' | ')' | '\'' | '"' | ',')
            });
            let source = token.strip_prefix('@')?.trim_end_matches(':');
            let (path, line) = source.rsplit_once(':')?;
            let line = line
                .trim_matches(|character: char| !character.is_ascii_digit())
                .parse::<u32>()
                .ok()?;
            Some((path.to_string(), line))
        })
        .next()
        .map(|(script, line)| (Some(script), Some(line)))
        .unwrap_or_default();
    let entity_id = marker("entity_id");
    let component_index = marker("component_index");
    crate::window::RuntimeLogLine {
        level: "error".into(),
        message,
        entity_id,
        component_index,
        component,
        script,
        line,
        ..crate::window::RuntimeLogLine::default()
    }
}

#[cfg(not(neolove_packaged))]
fn apply_editor_runtime_input(
    platform_state: &SharedPlatformState,
    snapshot: editor_ipc::RuntimeInputSnapshot,
) {
    use std::collections::BTreeSet;

    let mut platform = lock_platform_state(platform_state);
    let next_keys = snapshot.keys.into_iter().collect::<BTreeSet<_>>();
    let previous_keys = platform.input().keys_down.clone();
    for key in next_keys.difference(&previous_keys) {
        platform.input_mut().keys_pressed.insert(key.clone());
        platform.input_mut().last_key_pressed = Some(key.clone());
    }
    for key in previous_keys.difference(&next_keys) {
        platform.input_mut().keys_released.insert(key.clone());
    }
    platform.input_mut().keys_down = next_keys;

    let next_buttons = snapshot
        .mouse_buttons
        .into_iter()
        .collect::<BTreeSet<_>>();
    let previous_buttons = platform.input().mouse_down.clone();
    for button in next_buttons.difference(&previous_buttons) {
        platform.input_mut().mouse_pressed.insert(button.clone());
    }
    for button in previous_buttons.difference(&next_buttons) {
        platform.input_mut().mouse_released.insert(button.clone());
    }
    platform.input_mut().mouse_down = next_buttons;
    platform.set_mouse_position(snapshot.mouse_x, snapshot.mouse_y);
    platform.input_mut().wheel_x += snapshot.wheel_x;
    platform.input_mut().wheel_y += snapshot.wheel_y;
    if !snapshot.text.is_empty() {
        platform.input_mut().char_pressed = Some(snapshot.text);
    }
}

fn run_project_window(project_root: PathBuf, data_root: Option<PathBuf>) -> Result<(), String> {
    env::set_current_dir(&project_root).map_err(|error| {
        format!(
            "failed to set current directory to {}: {error}",
            project_root.display()
        )
    })?;
    let (title, icon, mut window_width, mut window_height, fullscreen, resizable) =
        window_options_for_project(&project_root);
    #[cfg(not(neolove_packaged))]
    let editor_embedded = env::var("NEOLOVE_EDITOR_EMBEDDED").as_deref() == Ok("1");
    #[cfg(neolove_packaged)]
    let editor_embedded = false;
    if editor_embedded {
        window_width = env::var("NEOLOVE_EDITOR_EMBEDDED_WIDTH")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|value| value.is_finite() && *value >= 64.0)
            .unwrap_or(960.0);
        window_height = env::var("NEOLOVE_EDITOR_EMBEDDED_HEIGHT")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|value| value.is_finite() && *value >= 64.0)
            .unwrap_or(540.0);
    }
    let mobile_profile = mobile_emulation::MobileEmulation::from_env();
    let (window_width, window_height, fullscreen, resizable) = if mobile_profile.enabled {
        let (mobile_width, mobile_height) = mobile_profile.oriented_size();
        (mobile_width as f32, mobile_height as f32, false, false)
    } else {
        (window_width, window_height, fullscreen, resizable)
    };
    let mut runtime = match data_root {
        Some(data_root) => window::Runtime::with_data_root(project_root, data_root),
        None => window::Runtime::new(project_root),
    };
    runtime.set_platform_window_state(window_width, window_height);

    // When launched by the editor, stream logs and live scene snapshots back to
    // its logger window over loopback IPC. Absent the env var this is a no-op.
    #[cfg(not(neolove_packaged))]
    let ipc_client = env::var("NEOLOVE_EDITOR_IPC")
        .ok()
        .and_then(|addr| editor_ipc::IpcClient::connect(&addr));
    #[cfg(not(neolove_packaged))]
    let (log_tx, log_rx) = std::sync::mpsc::channel();
    #[cfg(not(neolove_packaged))]
    if ipc_client.is_some() {
        runtime.set_log_sink(log_tx);
    }

    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.start())) {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            let message = format!(
                "failed to start runtime:\n{}",
                lua_error::describe_lua_error(&error)
            );
            #[cfg(not(neolove_packaged))]
            if let Some(ipc) = ipc_client.as_ref() {
                ipc.send(&editor_ipc::IpcMessage::Log(runtime_error_log(
                    message.clone(),
                )));
            }
            return Err(message);
        }
        Err(payload) => {
            let message = format!(
                "runtime panicked during startup\nPanic: {}",
                lua_error::describe_panic(payload.as_ref())
            );
            #[cfg(not(neolove_packaged))]
            if let Some(ipc) = ipc_client.as_ref() {
                ipc.send(&editor_ipc::IpcMessage::Log(runtime_error_log(
                    message.clone(),
                )));
            }
            return Err(message);
        }
    }

    // Capture the exact post-load, pre-update state once. Live snapshots below
    // continue to reflect scripts and simulation, while this immutable sample
    // lets the editor distinguish serialization parity from intentional play.
    #[cfg(not(neolove_packaged))]
    if let Some(ipc) = ipc_client.as_ref() {
        ipc.send(&editor_ipc::IpcMessage::InitialScene {
            entities: runtime.snapshot_entities(),
        });
    }

    let event_loop =
        catch_desktop_panic("failed to initialize the window event loop", EventLoop::new)?;
    let title = if mobile_profile.enabled {
        format!(
            "{} - Mobile Emulator ({})",
            title,
            mobile_profile.orientation.as_str()
        )
    } else {
        title
    };
    let mut builder = WindowBuilder::new()
        .with_title(title)
        .with_inner_size(LogicalSize::new(window_width as f64, window_height as f64))
        .with_resizable(resizable && !editor_embedded)
        .with_visible(!editor_embedded);
    if fullscreen && !editor_embedded {
        builder = builder.with_fullscreen(Some(Fullscreen::Borderless(None)));
    }
    if let Some(icon) = icon {
        builder = builder.with_window_icon(Some(icon));
    }
    let window = builder
        .build(&event_loop)
        .map(std::sync::Arc::new)
        .map_err(|error| format!("failed to create window: {error}"))?;
    let size = window.inner_size();
    let (logical_width, logical_height) =
        logical_dimensions(size.width, size.height, window.scale_factor());
    runtime.set_platform_window_state(logical_width as f32, logical_height as f32);

    let platform_state = runtime.platform_state();
    let render_state = runtime.render_state();
    let mut presenter = if editor_embedded {
        DesktopPresenter::new_embedded(
            &event_loop,
            &window,
            logical_width,
            logical_height,
        )?
    } else {
        DesktopPresenter::new(&event_loop, &window)?
    };

    let mut last_update = Instant::now();
    let mut next_update_deadline = None;
    let mut last_snapshot = Instant::now();
    #[cfg(not(neolove_packaged))]
    let mut last_frame_stream = Instant::now() - Duration::from_secs(1);
    #[cfg(not(neolove_packaged))]
    let mut frame_serial = 0_u64;
    #[cfg(not(neolove_packaged))]
    let mut last_update_ms = 0.0_f32;
    #[cfg(not(neolove_packaged))]
    let mut runtime_fps = 60.0_f32;
    let mut cursor_grab_warning_logged = false;
    #[cfg(not(neolove_packaged))]
    let mut editor_paused = false;
    event_loop.run(move |event, _target, control_flow| {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let now = Instant::now();
            *control_flow = next_update_deadline
                .filter(|deadline| *deadline > now)
                .map(ControlFlow::WaitUntil)
                .unwrap_or(ControlFlow::Poll);

            match event {
                Event::WindowEvent { event, .. } => match event {
                    WindowEvent::CloseRequested => *control_flow = ControlFlow::Exit,
                    WindowEvent::Resized(size) => {
                        if mobile_profile.enabled {
                            let (mobile_width, mobile_height) = mobile_profile.oriented_size();
                            let requested =
                                LogicalSize::new(mobile_width as f64, mobile_height as f64);
                            window.set_inner_size(requested);
                            runtime.set_platform_window_state(
                                mobile_width as f32,
                                mobile_height as f32,
                            );
                        } else {
                            let (width, height) =
                                logical_dimensions(size.width, size.height, window.scale_factor());
                            runtime.set_platform_window_state(width as f32, height as f32);
                        }
                        presenter.request_resize();
                    }
                    WindowEvent::ScaleFactorChanged { new_inner_size, .. } => {
                        let (width, height) = logical_dimensions(
                            new_inner_size.width,
                            new_inner_size.height,
                            window.scale_factor(),
                        );
                        runtime.set_platform_window_state(width as f32, height as f32);
                        presenter.request_resize();
                    }
                    WindowEvent::CursorMoved { position, .. } => {
                        let scale_factor = window.scale_factor().max(1.0);
                        runtime.set_platform_mouse_state(
                            (position.x / scale_factor) as f32,
                            (position.y / scale_factor) as f32,
                        );
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
                        if !mobile_profile.enabled && !ch.is_control() {
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
                        if mobile_profile.enabled {
                            return;
                        }
                        if let Some(name) = virtual_key_name(key) {
                            if let Err(error) = with_platform_state(
                                &platform_state,
                                "updating keyboard state",
                                |platform| {
                                    let name = name.to_string();
                                    match state {
                                        ElementState::Pressed => {
                                            if platform.input_mut().keys_down.insert(name.clone()) {
                                                platform
                                                    .input_mut()
                                                    .keys_pressed
                                                    .insert(name.clone());
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
                    #[cfg(not(neolove_packaged))]
                    let mut editor_step_once = false;
                    #[cfg(not(neolove_packaged))]
                    if let Some(ipc) = ipc_client.as_ref() {
                        for command in ipc.drain_commands() {
                            match command {
                                editor_ipc::IpcCommand::Pause => editor_paused = true,
                                editor_ipc::IpcCommand::Resume => editor_paused = false,
                                editor_ipc::IpcCommand::Step => {
                                    editor_paused = true;
                                    editor_step_once = true;
                                }
                                editor_ipc::IpcCommand::Stop => {
                                    *control_flow = ControlFlow::Exit;
                                    return;
                                }
                                editor_ipc::IpcCommand::Input { snapshot } => {
                                    apply_editor_runtime_input(&platform_state, snapshot);
                                }
                                editor_ipc::IpcCommand::Resize { width, height } => {
                                    let width = width.clamp(64, 4096);
                                    let height = height.clamp(64, 4096);
                                    window.set_inner_size(LogicalSize::new(
                                        width as f64,
                                        height as f64,
                                    ));
                                    runtime.set_platform_window_state(
                                        width as f32,
                                        height as f32,
                                    );
                                    presenter.request_resize();
                                }
                            }
                        }
                    }

                    #[cfg(not(neolove_packaged))]
                    if editor_paused && !editor_step_once {
                        last_update = Instant::now();
                        let _ = with_platform_state(
                            &platform_state,
                            "finalizing paused frame input state",
                            |platform| platform.begin_frame(),
                        );
                        next_update_deadline =
                            Some(last_update + Duration::from_millis(16));
                        window.request_redraw();
                        return;
                    }

                    if let Some(deadline) = next_update_deadline
                        && Instant::now() < deadline
                    {
                        *control_flow = ControlFlow::WaitUntil(deadline);
                        return;
                    }
                    next_update_deadline = None;
                    let update_start = Instant::now();
                    let mut dt = update_start.duration_since(last_update).as_secs_f32();
                    #[cfg(not(neolove_packaged))]
                    if editor_step_once {
                        dt = 1.0 / 60.0;
                    }
                    last_update = update_start;
                    #[cfg(not(neolove_packaged))]
                    if (0.001..=0.25).contains(&dt) {
                        runtime_fps = runtime_fps * 0.85 + dt.recip() * 0.15;
                    }
                    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        runtime.update(dt)
                    })) {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            #[cfg(not(neolove_packaged))]
                            if let Some(ipc) = ipc_client.as_ref() {
                                ipc.send(&editor_ipc::IpcMessage::Log(runtime_error_log(
                                    error.clone(),
                                )));
                            }
                            exit_runtime_failure(control_flow, "Fatal Runtime Error:", &error);
                        }
                        Err(payload) => {
                            let panic_message = format!(
                                "Runtime panicked during frame update\nPanic: {}",
                                lua_error::describe_panic(payload.as_ref())
                            );
                            #[cfg(not(neolove_packaged))]
                            if let Some(ipc) = ipc_client.as_ref() {
                                ipc.send(&editor_ipc::IpcMessage::Log(runtime_error_log(
                                    panic_message.clone(),
                                )));
                            }
                            exit_runtime_failure(
                                control_flow,
                                "Rust Panic:",
                                &panic_message,
                            );
                        }
                    }
                    #[cfg(not(neolove_packaged))]
                    {
                        last_update_ms = update_start.elapsed().as_secs_f32() * 1000.0;
                    }

                    // Stream output and a throttled live snapshot to the editor.
                    #[cfg(not(neolove_packaged))]
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
                                    eprintln!(
                                        "cursor grab warning: failed to {action} cursor: {error}"
                                    );
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

                    window.request_redraw();
                }
                Event::RedrawRequested(_) => {
                    let render_start = Instant::now();
                    if let Err(error) = presenter.render(&window, &platform_state, &render_state) {
                        #[cfg(not(neolove_packaged))]
                        if let Some(ipc) = ipc_client.as_ref() {
                            ipc.send(&editor_ipc::IpcMessage::Log(runtime_error_log(format!(
                                "desktop presenter failed: {error}"
                            ))));
                        }
                        exit_runtime_failure(
                            control_flow,
                            "Fatal Render Error:",
                            &format!("desktop presenter failed: {error}"),
                        );
                    }
                    #[cfg(not(neolove_packaged))]
                    if editor_embedded
                        && last_frame_stream.elapsed() >= Duration::from_millis(66)
                        && let Some(ipc) = ipc_client.as_ref()
                        && let Some((width, height, pixels)) = presenter.embedded_pixels()
                    {
                        frame_serial = frame_serial.wrapping_add(1);
                        let render_ms = render_start.elapsed().as_secs_f32() * 1000.0;
                        let (draw_calls, triangles) = runtime_frame_draw_stats(&render_state);
                        match encode_editor_runtime_frame(
                            frame_serial,
                            width,
                            height,
                            pixels,
                            presenter.backend_name(),
                            runtime_fps,
                            last_update_ms,
                            render_ms,
                            draw_calls,
                            triangles,
                        ) {
                            Ok(frame) => ipc.send(&editor_ipc::IpcMessage::Frame(frame)),
                            Err(error) => eprintln!("runtime frame stream warning: {error}"),
                        }
                        last_frame_stream = Instant::now();
                    }
                    next_update_deadline =
                        frame_deadline(last_update, Instant::now(), runtime.max_fps());
                    if let Some(deadline) = next_update_deadline {
                        *control_flow = ControlFlow::WaitUntil(deadline);
                    }
                }
                _ => {}
            }
        }));

        if let Err(payload) = result {
            exit_runtime_failure(
                control_flow,
                "Rust Panic:",
                &describe_desktop_panic(
                    "runtime panicked while processing window events",
                    payload.as_ref(),
                ),
            );
        }
    });
}

fn create_project_at(
    project_path: &Path,
    project_name: &str,
    project_kind: ProjectKind,
) -> Result<PathBuf, String> {
    if let Some(parent) = project_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::create_dir(project_path).map_err(|error| {
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

[project]
kind = \"{}\"
start_scene = \"scene.neoscene\"

[window]
title = \"{}\"
icon = \"assets/icon.png\"
width = 1280
height = 720
fullscreen = false
resizable = true

[dependencies]
",
        project_name,
        project_kind.as_str(),
        project_name
    );
    fs::write(&toml_path, contents)
        .map_err(|error| format!("failed to write {}: {error}", toml_path.display()))?;

    let entry_path = project_path.join("main.luau");
    fs::write(
        &entry_path,
        format!(
            "-- Generated by the NeoLOVE visual editor. Edits may be overwritten.\nprint(\"Hello, {}!\")",
            project_name
        ),
    )
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

    Ok(project_path.to_path_buf())
}

fn handle_new_command(project_name: &str, project_kind: ProjectKind) -> Result<PathBuf, String> {
    let project_path = resolve_from_cwd(project_name)
        .map_err(|error| format!("failed to resolve project path '{project_name}': {error}"))?;
    let display_name = project_path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(project_name);
    create_project_at(&project_path, display_name, project_kind)
}

fn parse_new_options(args: &[String]) -> Result<(ProjectKind, &str), String> {
    let mut kind = ProjectKind::TwoD;
    let mut explicit_kind = None;
    let mut project_name = None;

    for arg in args {
        let candidate_kind = match arg.as_str() {
            "--2d" => Some(ProjectKind::TwoD),
            "--3d" => Some(ProjectKind::ThreeD),
            _ => None,
        };
        if let Some(candidate_kind) = candidate_kind {
            if explicit_kind.replace(candidate_kind).is_some() {
                return Err("new failed: specify only one of --2d or --3d".to_string());
            }
            kind = candidate_kind;
            continue;
        }

        if arg.starts_with('-') {
            return Err(format!("new failed: unknown option '{arg}'"));
        }
        if project_name.replace(arg.as_str()).is_some() {
            return Err("new failed: expected exactly one project name".to_string());
        }
    }

    let project_name = project_name
        .ok_or_else(|| "new failed: expected a project name after `neolove new`".to_string())?;
    Ok((kind, project_name))
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
    println!("  neolove hub");
    println!("  neolove new [--2d|--3d] <project-name>");
    println!(
        "  neolove run [project-dir] [--mobile] [--portrait|--landscape] [--wifi|--cellular|--offline]"
    );
    println!(
        "  neolove validate-3d [project-dir] --baseline <png> [--backend auto|software|vulkan] [--width N --height N] [--write-baseline]"
    );
    println!("  neolove editor [project-dir]");
    println!("  neolove build [project-dir] [--windows|--linux|--webasm|--android|--apk|--ios]");
    println!("  neolove api [project-dir]");
    println!("  neolove update");
    println!("  neolove setup-path");
    println!("  neolove setup-start-menu");
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
        None => {
            env::current_dir().map_err(|error| format!("failed to get current directory: {error}"))
        }
    }
}

fn graphical_desktop_available() -> bool {
    #[cfg(any(windows, target_os = "macos"))]
    {
        true
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        env::var_os("WAYLAND_DISPLAY").is_some() || env::var_os("DISPLAY").is_some()
    }
}

fn embedded_data_root(executable: &Path) -> Result<PathBuf, String> {
    // Compressed desktop builds run their cached native runtime from the temp
    // directory. Keep user saves beside the distributed launcher, exactly as
    // they were for the former uncompressed single-file executable.
    let launcher_path = env::var_os("NEOLOVE_LAUNCHER_PATH")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let executable = launcher_path.as_deref().unwrap_or(executable);
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

fn parse_run_options<'a>(
    args: &'a [String],
) -> Result<(Option<&'a str>, mobile_emulation::MobileEmulation), String> {
    let mut project_arg: Option<&str> = None;
    let mut mobile = mobile_emulation::MobileEmulation::from_env();
    for arg in args {
        match arg.as_str() {
            "--mobile" | "--emulate-mobile" => mobile.enabled = true,
            "--portrait" => {
                mobile.enabled = true;
                mobile.orientation = mobile_emulation::MobileOrientation::Portrait;
            }
            "--landscape" => {
                mobile.enabled = true;
                mobile.orientation = mobile_emulation::MobileOrientation::Landscape;
            }
            "--wifi" => {
                mobile.enabled = true;
                mobile.wifi = true;
                mobile.cellular = false;
            }
            "--cellular" => {
                mobile.enabled = true;
                mobile.wifi = false;
                mobile.cellular = true;
            }
            "--offline" | "--no-wifi" => {
                mobile.enabled = true;
                mobile.wifi = false;
                mobile.cellular = false;
            }
            "--low-power" => {
                mobile.enabled = true;
                mobile.low_power = true;
            }
            "--no-low-power" => mobile.low_power = false,
            _ if arg.starts_with("--mobile-size=") => {
                mobile.enabled = true;
                let value = arg.trim_start_matches("--mobile-size=");
                let Some((width, height)) = value.split_once('x') else {
                    return Err("run failed: --mobile-size expects WIDTHxHEIGHT".to_string());
                };
                mobile.width = width
                    .parse::<u32>()
                    .ok()
                    .filter(|value| *value > 0)
                    .ok_or_else(|| "run failed: invalid --mobile-size width".to_string())?;
                mobile.height = height
                    .parse::<u32>()
                    .ok()
                    .filter(|value| *value > 0)
                    .ok_or_else(|| "run failed: invalid --mobile-size height".to_string())?;
            }
            _ if arg.starts_with('-') => {
                return Err(format!("run failed: unrecognized option: {arg}"));
            }
            _ if project_arg.is_none() => project_arg = Some(arg),
            _ => return Err("run failed: expected at most one project directory".to_string()),
        }
    }
    Ok((project_arg, mobile))
}

#[cfg(not(neolove_packaged))]
#[derive(Clone, Debug, PartialEq, Eq)]
struct Validate3dOptions<'a> {
    project_arg: Option<&'a str>,
    baseline: PathBuf,
    backend: &'a str,
    width: u32,
    height: u32,
    write_baseline: bool,
    report: Option<PathBuf>,
    diff: Option<PathBuf>,
    timeout: Duration,
}

#[cfg(not(neolove_packaged))]
fn parse_validate_3d_options(args: &[String]) -> Result<Validate3dOptions<'_>, String> {
    let mut project_arg = None;
    let mut baseline = None;
    let mut backend = "auto";
    let mut width = 960_u32;
    let mut height = 540_u32;
    let mut write_baseline = false;
    let mut report = None;
    let mut diff = None;
    let mut timeout = Duration::from_secs(30);
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_str();
        let mut next_value = |name: &str| -> Result<&str, String> {
            index += 1;
            args.get(index)
                .map(String::as_str)
                .ok_or_else(|| format!("validate-3d failed: {name} requires a value"))
        };
        match argument {
            "--baseline" => baseline = Some(PathBuf::from(next_value("--baseline")?)),
            "--backend" => backend = next_value("--backend")?,
            "--width" => {
                width = next_value("--width")?.parse().map_err(|_| {
                    "validate-3d failed: --width expects an integer".to_string()
                })?;
            }
            "--height" => {
                height = next_value("--height")?.parse().map_err(|_| {
                    "validate-3d failed: --height expects an integer".to_string()
                })?;
            }
            "--write-baseline" | "--set-baseline" => write_baseline = true,
            "--report" => report = Some(PathBuf::from(next_value("--report")?)),
            "--diff" => diff = Some(PathBuf::from(next_value("--diff")?)),
            "--timeout-ms" => {
                let milliseconds = next_value("--timeout-ms")?.parse::<u64>().map_err(|_| {
                    "validate-3d failed: --timeout-ms expects an integer".to_string()
                })?;
                timeout = Duration::from_millis(milliseconds);
            }
            _ if argument.starts_with("--baseline=") => {
                baseline = Some(PathBuf::from(argument.trim_start_matches("--baseline=")));
            }
            _ if argument.starts_with("--backend=") => {
                backend = argument.trim_start_matches("--backend=");
            }
            _ if argument.starts_with("--width=") => {
                width = argument
                    .trim_start_matches("--width=")
                    .parse()
                    .map_err(|_| {
                        "validate-3d failed: --width expects an integer".to_string()
                    })?;
            }
            _ if argument.starts_with("--height=") => {
                height = argument
                    .trim_start_matches("--height=")
                    .parse()
                    .map_err(|_| {
                        "validate-3d failed: --height expects an integer".to_string()
                    })?;
            }
            _ if argument.starts_with("--report=") => {
                report = Some(PathBuf::from(argument.trim_start_matches("--report=")));
            }
            _ if argument.starts_with("--diff=") => {
                diff = Some(PathBuf::from(argument.trim_start_matches("--diff=")));
            }
            _ if argument.starts_with("--timeout-ms=") => {
                let milliseconds = argument
                    .trim_start_matches("--timeout-ms=")
                    .parse::<u64>()
                    .map_err(|_| {
                        "validate-3d failed: --timeout-ms expects an integer".to_string()
                    })?;
                timeout = Duration::from_millis(milliseconds);
            }
            _ if argument.starts_with('-') => {
                return Err(format!(
                    "validate-3d failed: unrecognized option: {argument}"
                ));
            }
            _ if project_arg.is_none() => project_arg = Some(argument),
            _ => {
                return Err(
                    "validate-3d failed: expected at most one project directory".to_string(),
                );
            }
        }
        index += 1;
    }
    if !matches!(backend, "auto" | "software" | "vulkan") {
        return Err(
            "validate-3d failed: --backend expects auto, software, or vulkan".to_string(),
        );
    }
    if !(64..=8192).contains(&width) || !(64..=8192).contains(&height) {
        return Err(
            "validate-3d failed: width and height must each be between 64 and 8192".to_string(),
        );
    }
    if timeout < Duration::from_millis(100) || timeout > Duration::from_secs(300) {
        return Err(
            "validate-3d failed: --timeout-ms must be between 100 and 300000".to_string(),
        );
    }
    let baseline = baseline.ok_or_else(|| {
        "validate-3d failed: --baseline PATH is required (add --write-baseline to create it)"
            .to_string()
    })?;
    Ok(Validate3dOptions {
        project_arg,
        baseline,
        backend,
        width,
        height,
        write_baseline,
        report,
        diff,
        timeout,
    })
}

#[cfg(not(neolove_packaged))]
fn visual_baseline_metadata_path(baseline: &Path) -> PathBuf {
    let stem = baseline
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("baseline");
    baseline.with_file_name(format!("{stem}-baseline.json"))
}

#[cfg(not(neolove_packaged))]
fn visual_validation_artifact_path(baseline: &Path, suffix: &str) -> PathBuf {
    let stem = baseline
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("baseline");
    baseline.with_file_name(format!("{stem}-{suffix}"))
}

#[cfg(not(neolove_packaged))]
fn stop_validation_child(
    command_tx: &std::sync::mpsc::Sender<editor_ipc::IpcCommand>,
    child: &mut std::process::Child,
) -> Result<std::process::ExitStatus, String> {
    let _ = command_tx.send(editor_ipc::IpcCommand::Stop);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("query validation runtime: {error}"))?
        {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            child
                .kill()
                .map_err(|error| format!("terminate validation runtime: {error}"))?;
            return child
                .wait()
                .map_err(|error| format!("wait for terminated validation runtime: {error}"));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(not(neolove_packaged))]
fn capture_validation_frame(
    executable: &Path,
    project_root: &Path,
    options: &Validate3dOptions<'_>,
) -> Result<editor_ipc::RuntimeFrame, String> {
    let session = editor_ipc::LoggerSession::start()
        .map_err(|error| format!("start validation IPC listener: {error}"))?;
    let command_tx = session.command_sender();
    let mut child = std::process::Command::new(executable)
        .arg("run")
        .arg(project_root)
        .env("NEOLOVE_EDITOR_IPC", &session.addr)
        .env("NEOLOVE_EDITOR_EMBEDDED", "1")
        .env("NEOLOVE_EDITOR_EMBEDDED_BACKEND", options.backend)
        .env("NEOLOVE_EDITOR_EMBEDDED_WIDTH", options.width.to_string())
        .env("NEOLOVE_EDITOR_EMBEDDED_HEIGHT", options.height.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("launch validation runtime: {error}"))?;
    let deadline = Instant::now() + options.timeout;
    let frame = loop {
        if let Ok(state) = session.state.lock() {
            if let Some(frame) = state.latest_frame.clone() {
                break frame;
            }
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("query validation runtime: {error}"))?
        {
            let mut stderr = String::new();
            if let Some(mut pipe) = child.stderr.take() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            return Err(format!(
                "validation runtime exited before producing a frame ({status}){}",
                if stderr.trim().is_empty() {
                    String::new()
                } else {
                    format!(": {}", stderr.trim())
                }
            ));
        }
        if Instant::now() >= deadline {
            let _ = stop_validation_child(&command_tx, &mut child);
            return Err(format!(
                "validation runtime did not produce a frame within {} ms",
                options.timeout.as_millis()
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let logs = session
        .state
        .lock()
        .map(|state| state.logs.iter().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let status = stop_validation_child(&command_tx, &mut child)?;
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    if !status.success() {
        return Err(format!(
            "validation runtime exited with {status}{}",
            if stderr.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", stderr.trim())
            }
        ));
    }
    if let Some(error) = logs.iter().find(|line| line.level == "error") {
        return Err(format!("validation runtime error: {}", error.message));
    }
    for warning in logs.iter().filter(|line| line.level == "warning") {
        eprintln!("validation runtime warning: {}", warning.message);
    }
    Ok(frame)
}

#[cfg(not(neolove_packaged))]
fn write_parent(path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("path has no parent: {}", path.display()))?;
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
    fs::create_dir_all(parent)
        .map_err(|error| format!("create directory {}: {error}", parent.display()))
}

#[cfg(not(neolove_packaged))]
fn validate_3d_project(
    executable: &Path,
    project_root: &Path,
    options: &Validate3dOptions<'_>,
) -> Result<(), String> {
    let frame = capture_validation_frame(executable, project_root, options)?;
    let png = base64::engine::general_purpose::STANDARD
        .decode(&frame.png_base64)
        .map_err(|error| format!("decode validation frame: {error}"))?;
    let current = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
        .map_err(|error| format!("decode validation PNG: {error}"))?
        .into_rgba8();
    let baseline_path = if options.baseline.is_absolute() {
        options.baseline.clone()
    } else {
        env::current_dir()
            .map_err(|error| format!("resolve baseline path: {error}"))?
            .join(&options.baseline)
    };
    let metadata_path = visual_baseline_metadata_path(&baseline_path);
    if options.write_baseline {
        write_parent(&baseline_path)?;
        current
            .save_with_format(&baseline_path, image::ImageFormat::Png)
            .map_err(|error| format!("save baseline {}: {error}", baseline_path.display()))?;
        let metadata = editor::visual_regression3d::VisualBaselineMetadata::new(
            frame.backend.clone(),
            current.width(),
            current.height(),
        );
        let json = serde_json::to_string_pretty(&metadata)
            .map_err(|error| format!("serialize baseline metadata: {error}"))?;
        fs::write(&metadata_path, json).map_err(|error| {
            format!(
                "save baseline metadata {}: {error}",
                metadata_path.display()
            )
        })?;
        println!(
            "Wrote 3D visual baseline {} ({} · {}x{})",
            baseline_path.display(),
            frame.backend,
            current.width(),
            current.height()
        );
        return Ok(());
    }
    let baseline = image::open(&baseline_path)
        .map_err(|error| format!("open baseline {}: {error}", baseline_path.display()))?
        .into_rgba8();
    let metadata = fs::read_to_string(&metadata_path)
        .ok()
        .and_then(|json| {
            serde_json::from_str::<editor::visual_regression3d::VisualBaselineMetadata>(&json).ok()
        })
        .filter(|metadata| metadata.matches(baseline.width(), baseline.height()));
    let baseline_backend = metadata
        .as_ref()
        .map(|metadata| metadata.backend.as_str())
        .unwrap_or("");
    if baseline_backend.is_empty() {
        eprintln!(
            "validation warning: no valid backend metadata at {}; using the strict same-backend profile",
            metadata_path.display()
        );
    }
    let (tolerance, profile) = editor::visual_regression3d::comparison_tolerance(
        baseline_backend,
        &frame.backend,
    );
    let (mut report, diff_image) =
        editor::visual_regression3d::compare(&baseline, &current, tolerance);
    report.comparison_profile = profile.to_string();
    report.baseline_backend = baseline_backend.to_string();
    report.current_backend = frame.backend.clone();
    let report_path = options.report.clone().unwrap_or_else(|| {
        visual_validation_artifact_path(&baseline_path, "latest-report.json")
    });
    let diff_path = options
        .diff
        .clone()
        .unwrap_or_else(|| visual_validation_artifact_path(&baseline_path, "latest-diff.png"));
    write_parent(&report_path)?;
    let report_json = serde_json::to_string_pretty(&report)
        .map_err(|error| format!("serialize validation report: {error}"))?;
    fs::write(&report_path, &report_json)
        .map_err(|error| format!("write report {}: {error}", report_path.display()))?;
    println!("{report_json}");
    println!("Report: {}", report_path.display());
    if !report.passed {
        write_parent(&diff_path)?;
        diff_image
            .save_with_format(&diff_path, image::ImageFormat::Png)
            .map_err(|error| format!("write diff {}: {error}", diff_path.display()))?;
        println!("Diff: {}", diff_path.display());
        return Err(format!("3D visual regression failed: {}", report.summary()));
    }
    Ok(())
}

fn run_cli() -> Result<(), String> {
    let args: Vec<String> = env::args().collect();

    let current_exe = env::current_exe()
        .map_err(|error| format!("failed to resolve executable path: {error}"))?;

    let payload_executable = env::var_os("NEOLOVE_LAUNCHER_PATH")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| current_exe.clone());
    let embedded_payload = read_embedded_payload(&payload_executable)
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

    #[cfg(neolove_packaged)]
    {
        Err("packaged NeoLOVE runtime can only launch its embedded game payload".to_string())
    }

    #[cfg(not(neolove_packaged))]
    {
        match setup_path_for_neolove() {
            Ok(true) => {
                eprintln!("Added Neolove to PATH. Open a new terminal to use `neolove` globally.");
            }
            Ok(false) => {}
            Err(e) => {
                eprintln!("PATH setup warning: {}", e);
            }
        }

        match setup_start_menu_for_neolove() {
            Ok(true) => {
                eprintln!("Added NeoLOVE to your application launcher.");
            }
            Ok(false) => {}
            Err(e) => {
                eprintln!("Start menu setup warning: {}", e);
            }
        }

        if args.len() <= 1 {
            if graphical_desktop_available() {
                editor::run_hub().map_err(|error| format!("hub failed: {error}"))?;
                return Ok(());
            }
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
            "setup-start-menu" => match setup_start_menu_for_neolove() {
                Ok(true) => println!("Application launcher entry updated."),
                Ok(false) => println!("Application launcher entry is already up to date."),
                Err(error) => {
                    return Err(format!("failed to set up application launcher: {error}"));
                }
            },
            "hub" => {
                if args.len() != 2 {
                    return Err(format!(
                        "hub failed: expected no arguments, got {}",
                        args.len().saturating_sub(2)
                    ));
                }
                editor::run_hub().map_err(|error| format!("hub failed: {error}"))?;
            }
            "update" => {
                if args.len() != 2 {
                    return Err(format!(
                        "update failed: expected no arguments, got {}",
                        args.len().saturating_sub(2)
                    ));
                }
                let outcome =
                    update::update_engine().map_err(|error| format!("update failed: {error}"))?;
                println!("{outcome}");
            }
            "new" => {
                let (project_kind, project_name) = parse_new_options(&args[2..])?;
                let project_path = handle_new_command(project_name, project_kind)?;
                println!(
                    "Created project \"{}\" at {}.",
                    project_name,
                    project_path.display()
                );
                println!("Set [window] fields in neolove.toml to customize the game window.");
                println!("To run, execute in the project directory the command `neolove run`");
                println!("To build a standalone executable, run `neolove build`");
                println!("To build the webasm package, run `neolove build --webasm`");
                println!("To build an Android APK, run `neolove build --android`");
                println!("To build an iOS simulator app on macOS, run `neolove build --ios`");
            }
            "run" => {
                let (project_arg, mobile_profile) = parse_run_options(&args[2..])?;
                mobile_emulation::set_current_process_env(&mobile_profile);
                let project_root = resolve_target_project_root(project_arg)?;
                validate_project_root(&project_root)
                    .map_err(|error| format!("run failed: {error}"))?;
                run_project_window(project_root, None)
                    .map_err(|error| format!("run failed: {error}"))?;
            }
            "validate-3d" => {
                let options = parse_validate_3d_options(&args[2..])?;
                let project_root = resolve_target_project_root(options.project_arg)?;
                validate_project_root(&project_root)
                    .map_err(|error| format!("validate-3d failed: {error}"))?;
                validate_3d_project(&current_exe, &project_root, &options)?;
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
                editor::run_editor(project_root)
                    .map_err(|error| format!("editor failed: {error}"))?;
            }
            "build" => {
                let mut project_arg: Option<&str> = None;
                let mut desktop_target = DesktopPackageTarget::Host;
                let mut webasm = false;
                let mut android = false;
                let mut ios = false;
                for arg in &args[2..] {
                    if arg == "--webasm" {
                        webasm = true;
                    } else if arg == "--android" || arg == "--apk" {
                        android = true;
                    } else if arg == "--ios" {
                        ios = true;
                    } else if arg == "--windows" || arg == "--win" || arg == "--exe" {
                        if desktop_target != DesktopPackageTarget::Host {
                            return Err("build failed: choose only one desktop target".to_string());
                        }
                        desktop_target = DesktopPackageTarget::Windows;
                    } else if arg == "--linux" {
                        if desktop_target != DesktopPackageTarget::Host {
                            return Err("build failed: choose only one desktop target".to_string());
                        }
                        desktop_target = DesktopPackageTarget::Linux;
                    } else if arg.starts_with('-') {
                        return Err(format!("build failed: unrecognized option: {arg}"));
                    } else if project_arg.is_none() {
                        project_arg = Some(arg);
                    } else {
                        return Err(
                            "build failed: expected at most one project directory".to_string()
                        );
                    }
                }

                let project_root = resolve_target_project_root(project_arg)?;
                validate_project_root(&project_root)
                    .map_err(|error| format!("build failed: {error}"))?;

                if [
                    webasm,
                    android,
                    ios,
                    desktop_target != DesktopPackageTarget::Host,
                ]
                .into_iter()
                .filter(|enabled| *enabled)
                .count()
                    > 1
                {
                    return Err("build failed: choose only one target option".to_string());
                }

                if webasm {
                    let (bundle_output, zip_output) = build_webasm(&project_root)
                        .map_err(|error| format!("build failed: {error}"))?;
                    println!("Built webasm bundle: {}", bundle_output.display());
                    println!("Built itch.io package: {}", zip_output.display());
                } else if android {
                    let output = build_android(&project_root)
                        .map_err(|error| format!("build failed: {error}"))?;
                    println!("Built Android APK: {}", output.display());
                } else if ios {
                    let output = build_ios(&project_root)
                        .map_err(|error| format!("build failed: {error}"))?;
                    println!("Built iOS simulator app: {}", output.display());
                } else {
                    let output = build_executable(&project_root, desktop_target)
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
    fn hidpi_dimensions_use_logical_pixels_and_reject_invalid_scales() {
        assert_eq!(logical_dimensions(3840, 2160, 2.0), (1920, 1080));
        assert_eq!(logical_dimensions(3000, 2000, 1.5), (2000, 1333));
        assert_eq!(logical_dimensions(640, 480, f64::NAN), (640, 480));
        assert_eq!(logical_dimensions(0, 0, 2.0), (1, 1));
    }

    #[test]
    fn fps_deadline_accounts_for_update_and_render_time() {
        let start = Instant::now();
        let midway = start + Duration::from_millis(5);
        let deadline = frame_deadline(start, midway, Some(100.0)).expect("remaining frame time");
        assert_eq!(deadline, start + Duration::from_millis(10));
        assert!(frame_deadline(start, start + Duration::from_millis(12), Some(100.0)).is_none());
        assert!(frame_deadline(start, midway, None).is_none());
    }

    #[test]
    fn software_hidpi_blit_supports_nearest_and_linear_filtering() {
        let source = [255, 0, 0, 255, 0, 255, 0, 255];
        let mut nearest = vec![0; 8];
        blit_software_pixels(&source, 2, 1, &mut nearest, 4, 2, true);
        assert_eq!(nearest[0], 0x00ff0000);
        assert_eq!(nearest[1], 0x00ff0000);
        assert_eq!(nearest[2], 0x0000ff00);
        assert_eq!(nearest[3], 0x0000ff00);
        assert_eq!(&nearest[..4], &nearest[4..]);

        let mut linear = vec![0; 4];
        blit_software_pixels(&source, 2, 1, &mut linear, 4, 1, false);
        assert_eq!(linear[0], 0x00ff0000);
        assert_eq!(linear[3], 0x0000ff00);
        assert_ne!(linear[1], linear[0]);
        assert_ne!(linear[2], linear[3]);
    }

    #[test]
    fn linux_to_windows_builds_link_mingw_runtimes_statically() {
        let config = cross_target_rustflags_config(
            DesktopPackageTarget::Windows,
            Some(Path::new("/mingw/lib")),
        )
        .expect("cross Windows target should add linker flags");
        assert!(config.contains("native=/mingw/lib"));
        assert!(config.contains("link-arg=-static-libgcc"));
        assert!(config.contains("link-arg=-static-libstdc++"));
        assert_eq!(
            cross_target_cpp_stdlib(DesktopPackageTarget::Windows),
            Some(("CXXSTDLIB_x86_64_pc_windows_gnu", "static=stdc++"))
        );
        assert_eq!(
            cross_target_rustflags_config(DesktopPackageTarget::Linux, None),
            None
        );
    }

    #[test]
    fn project_settings_parse_resizable_window_flag() {
        let root = std::env::temp_dir().join(format!(
            "neolove_window_settings_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create temp project");
        std::fs::write(
            root.join("neolove.toml"),
            "[project]\nstart_scene = \"levels/title.neoscene\"\n\n[window]\nwidth = 800\nheight = 600\nfullscreen = false\nresizable = false\n",
        )
        .expect("write settings");

        let settings = parse_project_settings(&root);
        assert_eq!(settings.kind, ProjectKind::TwoD);
        assert_eq!(
            settings.start_scene.as_deref(),
            Some("levels/title.neoscene")
        );
        assert_eq!(settings.window_width, Some(800.0));
        assert_eq!(settings.window_height, Some(600.0));
        assert_eq!(settings.window_fullscreen, Some(false));
        assert_eq!(settings.window_resizable, Some(false));

        let (_, _, _, _, _, resizable) = window_options_for_project(&root);
        assert!(!resizable);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn desktop_payload_excludes_editor_only_project_files() {
        for path in [
            ".git/config",
            ".vscode/settings.json",
            ".idea/workspace.xml",
            ".neolove/recovery/scene.json",
            "target/debug/game",
            "dist/game",
            ".gitignore",
            ".luaurc",
            "types/neolove_engine_api.d.luau",
        ] {
            assert!(should_skip_in_build(Path::new(path)), "{path}");
        }
        for path in [
            "main.luau",
            "neolove.toml",
            "scenes/level.neoscene",
            "assets/sprite.png",
        ] {
            assert!(!should_skip_in_build(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn packaged_runtime_caches_are_separate_by_project_kind() {
        let target = DesktopPackageTarget::Host;
        assert_ne!(
            target.target_dir_name(ProjectKind::TwoD),
            target.target_dir_name(ProjectKind::ThreeD)
        );
        assert!(target.target_dir_name(ProjectKind::TwoD).ends_with("-2d"));
        assert!(target.target_dir_name(ProjectKind::ThreeD).ends_with("-3d"));
    }

    #[test]
    fn new_command_project_kind_options_are_backward_compatible() {
        let args = ["my-game".to_string()];
        assert_eq!(
            parse_new_options(&args).expect("legacy new command"),
            (ProjectKind::TwoD, "my-game")
        );

        let args = ["--2d".to_string(), "my-game".to_string()];
        assert_eq!(
            parse_new_options(&args).expect("explicit 2D project"),
            (ProjectKind::TwoD, "my-game")
        );

        let args = ["--3d".to_string(), "my-game".to_string()];
        assert_eq!(
            parse_new_options(&args).expect("3D project"),
            (ProjectKind::ThreeD, "my-game")
        );

        let args = ["my-game".to_string(), "--3d".to_string()];
        assert_eq!(
            parse_new_options(&args).expect("kind option after project name"),
            (ProjectKind::ThreeD, "my-game")
        );

        let args = [
            "--2d".to_string(),
            "--3d".to_string(),
            "my-game".to_string(),
        ];
        assert!(parse_new_options(&args).is_err());
        let args = ["--unknown".to_string(), "my-game".to_string()];
        assert!(parse_new_options(&args).is_err());
        assert!(parse_new_options(&[]).is_err());
    }

    #[test]
    fn validate_3d_cli_options_are_bounded_and_ci_friendly() {
        let args = [
            "sample".to_string(),
            "--baseline".to_string(),
            "artifacts/reference.png".to_string(),
            "--backend=vulkan".to_string(),
            "--width".to_string(),
            "320".to_string(),
            "--height=180".to_string(),
            "--timeout-ms=12000".to_string(),
            "--report=artifacts/report.json".to_string(),
            "--diff".to_string(),
            "artifacts/diff.png".to_string(),
        ];
        let parsed = parse_validate_3d_options(&args).expect("validation options");
        assert_eq!(parsed.project_arg, Some("sample"));
        assert_eq!(parsed.baseline, PathBuf::from("artifacts/reference.png"));
        assert_eq!(parsed.backend, "vulkan");
        assert_eq!((parsed.width, parsed.height), (320, 180));
        assert_eq!(parsed.timeout, Duration::from_secs(12));
        assert_eq!(parsed.report, Some(PathBuf::from("artifacts/report.json")));
        assert_eq!(parsed.diff, Some(PathBuf::from("artifacts/diff.png")));
        assert_eq!(
            visual_baseline_metadata_path(Path::new("artifacts/reference.png")),
            PathBuf::from("artifacts/reference-baseline.json")
        );

        assert!(parse_validate_3d_options(&["--backend=metal".into()]).is_err());
        assert!(
            parse_validate_3d_options(&[
                "--baseline=base.png".into(),
                "--width=32".into()
            ])
            .is_err()
        );
        assert!(parse_validate_3d_options(&[]).is_err());
    }

    #[test]
    fn project_templates_write_and_parse_the_selected_kind() {
        let root = std::env::temp_dir().join(format!(
            "neolove_project_kind_template_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);

        for (directory, kind, expected) in [
            ("two-d", ProjectKind::TwoD, "kind = \"2d\""),
            ("three-d", ProjectKind::ThreeD, "kind = \"3d\""),
        ] {
            let project = root.join(directory);
            create_project_at(&project, "Kind Test", kind).expect("create project template");
            let toml = std::fs::read_to_string(project.join("neolove.toml"))
                .expect("read generated project settings");
            assert!(toml.contains(expected), "generated settings: {toml}");
            assert_eq!(parse_project_settings(&project).kind, kind);
            let entry = std::fs::read_to_string(project.join("main.luau"))
                .expect("read generated entry point");
            assert!(
                entry.starts_with("-- Generated by the NeoLOVE visual editor"),
                "new projects must mark main.luau as editor-owned"
            );
        }

        let _ = std::fs::remove_dir_all(root);
    }

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
        let output =
            std::env::temp_dir().join(format!("neolove_compression_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&output);
        std::fs::create_dir_all(&output).expect("create temp dir");
        unpack_payload(&compressed, &output).expect("unpack payload");
        assert_eq!(
            std::fs::read(output.join(path)).expect("read unpacked file"),
            data
        );
        let _ = std::fs::remove_dir_all(output);
    }

    #[test]
    fn embedded_input_snapshots_preserve_runtime_pressed_held_and_released_edges() {
        let platform = crate::platform::new_shared_platform_state();
        apply_editor_runtime_input(
            &platform,
            editor_ipc::RuntimeInputSnapshot {
                mouse_x: 120.0,
                mouse_y: 45.0,
                mouse_buttons: vec!["left".into()],
                keys: vec!["w".into(), "leftshift".into()],
                wheel_x: -1.0,
                wheel_y: 2.0,
                text: "é".into(),
            },
        );
        {
            let state = crate::platform::lock_platform_state(&platform);
            assert_eq!((state.mouse().x, state.mouse().y), (120.0, 45.0));
            assert!(state.input().keys_down.contains("w"));
            assert!(state.input().keys_pressed.contains("w"));
            assert!(state.input().mouse_down.contains("left"));
            assert!(state.input().mouse_pressed.contains("left"));
            assert_eq!((state.input().wheel_x, state.input().wheel_y), (-1.0, 2.0));
            assert_eq!(state.input().char_pressed.as_deref(), Some("é"));
        }
        crate::platform::lock_platform_state(&platform).begin_frame();
        apply_editor_runtime_input(
            &platform,
            editor_ipc::RuntimeInputSnapshot {
                mouse_x: 125.0,
                mouse_y: 40.0,
                ..Default::default()
            },
        );
        let state = crate::platform::lock_platform_state(&platform);
        assert!(state.input().keys_down.is_empty());
        assert!(state.input().keys_released.contains("w"));
        assert!(state.input().mouse_down.is_empty());
        assert!(state.input().mouse_released.contains("left"));
        assert_eq!((state.mouse().delta_x, state.mouse().delta_y), (5.0, -5.0));
    }

    #[test]
    fn runtime_error_logs_extract_entity_component_and_script_links() {
        let diagnostic = runtime_error_log(
            "rendering failed [entity_id=27 component_index=3 component=MeshRenderer3D]: @scripts/spinner.luau:42: bad material"
                .to_string(),
        );
        assert_eq!(diagnostic.level, "error");
        assert_eq!(diagnostic.entity_id, Some(27));
        assert_eq!(diagnostic.component_index, Some(3));
        assert_eq!(diagnostic.component.as_deref(), Some("MeshRenderer3D"));
        assert_eq!(diagnostic.script.as_deref(), Some("scripts/spinner.luau"));
        assert_eq!(diagnostic.line, Some(42));
    }
}
