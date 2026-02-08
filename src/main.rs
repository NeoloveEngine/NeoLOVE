pub mod hierarchy;
pub mod window;
mod core;
mod assets;
mod audio_system;
mod user_input;

use std::env;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use macroquad::color::BLACK;
use macroquad::prelude::draw_text;
use macroquad::window::{clear_background, next_frame, Conf, miniquad};
#[cfg(windows)]
use std::process::Command;

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
        env::var_os("USERPROFILE").map(PathBuf::from)
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
    let escaped_dir = binary_dir
        .to_string_lossy()
        .replace('\'', "''");

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

fn window_conf() -> Conf {
    Conf {
        window_title: "NeoLOVE".to_owned(),
        platform: miniquad::conf::Platform {
            // disable vsync so app.setMaxFps can raise the cap above monitor refresh
            // users can still cap fps in code (default is 60)
            swap_interval: Some(0),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    match setup_path_for_neolove() {
        Ok(true) => {
            eprintln!("Added Neolove to PATH. Open a new terminal to use `neolove` globally.");
        }
        Ok(false) => {}
        Err(e) => {
            eprintln!("PATH setup warning: {}", e);
        }
    }

    let args: Vec<String> = env::args().collect();
    if args.len() > 1 {
        match args[1].as_str() {
            "setup-path" => {
                match setup_path_for_neolove() {
                    Ok(true) => println!("PATH updated. Restart your terminal."),
                    Ok(false) => println!("PATH already contains Neolove."),
                    Err(e) => eprintln!("failed to set PATH: {}", e),
                }
            }
            "new" => {
                if args.len() != 3 {
                    println!("expected {} arguments, got {}", 3, args.len());
                    return;
                }
                let project_name = args[2].clone();
                let project_path = match resolve_from_cwd(&project_name) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("failed to resolve project path: {}", e);
                        return;
                    }
                };
                if let Err(e) = fs::create_dir(&project_path) {
                    eprintln!(
                        "error when creating project folder (does it already exist?): {}",
                        e
                    );
                    return;
                }

                {
                    let f = match File::create(&project_path.join("neolove.toml")) {
                        Ok(f) => f,
                        Err(e) => {
                            eprintln!("error when creating neolove.toml: {}", e);
                            return;
                        }
                    };
                    let contents = format!(
                        "\
[package]
name = \"{}\"
version = \"0.1.0\"

[dependencies]
",
                        project_name
                    );
                    let mut file = f;
                    if let Err(e) = file.write_all(contents.as_bytes()) {
                        eprintln!("could not write neolove.toml: {}", e);
                        return;
                    }
                }

                {
                    let f = match File::create(&project_path.join("main.luau")) {
                        Ok(f) => f,
                        Err(e) => {
                            eprintln!("error when creating main.luau: {}", e);
                            return;
                        }
                    };
                    let contents = format!("print(\"Hello, {}!\")", project_name);
                    let mut file = f;
                    if let Err(e) = file.write_all(contents.as_bytes()) {
                        eprintln!("could not write main.luau: {}", e);
                        return;
                    }
                }

                {
                    if let Err(e) = fs::create_dir(&project_path.join("assets")) {
                        eprintln!("could not create assets folder: {}", e);
                        return;
                    }
                }

                println!("Created project \"{project_name}\" at {}.", project_path.display());
                println!("To run, execute in the project directory the command `neolove run`")
            },
            "run" => {
                let cwd = match env::current_dir() {
                    Ok(cwd) => cwd,
                    Err(e) => {
                        eprintln!("failed to get current directory: {}", e);
                        return;
                    }
                };
                let mut runtime = window::Runtime::new(cwd);
                if let Err(e) = runtime.start() {
                    eprintln!("\x1b[31mLua Error:\x1b[0m {}", e);
                    return;
                }

                let mut last_frame = Instant::now();
                loop {
                    let frame_start = Instant::now();
                    clear_background(BLACK);
                    let dt = frame_start.duration_since(last_frame).as_secs_f32();
                    last_frame = frame_start;
                    runtime.update(dt);
                    if dt > 0.0 {
                        draw_text((1.0 / dt).round().to_string().as_str(), 10f32, 30f32, 32f32, BLACK);
                    }

                    if let Some(max_fps) = runtime.max_fps() {
                        let max_fps = max_fps.max(1.0);
                        let target = Duration::from_secs_f32(1.0 / max_fps);
                        let elapsed = frame_start.elapsed();
                        if elapsed < target {
                            std::thread::sleep(target - elapsed);
                        }
                    }
                    next_frame().await;
                }
            },
            _ => println!("unrecognized"),
        }
    } else {
        println!("no arguments provided");
    }
}
