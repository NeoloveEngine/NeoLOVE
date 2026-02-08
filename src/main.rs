pub mod hierarchy;
pub mod window;
mod core;
mod assets;

use std::env;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use macroquad::color::BLACK;
use macroquad::prelude::draw_text;
use macroquad::window::{clear_background, next_frame, Conf, miniquad};

fn resolve_from_cwd(user_path: &str) -> std::io::Result<PathBuf> {
    let p = PathBuf::from(user_path);

    if p.is_absolute() {
        return Ok(p);
    }

    let cwd = env::current_dir()?;
    Ok(cwd.join(p))
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
    let args: Vec<String> = env::args().collect();
    if args.len() > 1 {
        match args[1].as_str() {
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
                println!("To run, execute in the project directory the command `Neolove run`")
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
