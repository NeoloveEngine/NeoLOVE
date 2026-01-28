pub mod hierarchy;
pub mod window;

use std::env;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::time::SystemTime;
use macroquad::color::BLACK;
use macroquad::window::{clear_background, next_frame};

fn resolve_from_cwd(user_path: &str) -> std::io::Result<PathBuf> {
    let p = PathBuf::from(user_path);

    if p.is_absolute() {
        return Ok(p);
    }

    let cwd = env::current_dir()?;
    Ok(cwd.join(p))
}

#[macroquad::main("NeoLOVE")]
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
                let project_path = resolve_from_cwd(&project_name).unwrap();
                if fs::create_dir(&project_path).is_err()
                {
                    println!("error when creating project folder. does this folder already exist?");
                    return;
                }

                {
                    let f = File::create(&project_path.join("neolove.toml"));
                    let contents = format!("\
[package]
name = \"{}\"
version = \"0.1.0\"

[dependencies]
", project_name);
                    let mut file = f.expect("error when creating neolove.toml");
                    file.write_all(contents.as_bytes()).expect("could not write toml");
                }

                {
                    let f = File::create(&project_path.join("main.luau"));
                    let contents = format!("print(\"Hello, {}!\")", project_name);
                    let mut file = f.expect("error when creating main.luau");
                    file.write_all(contents.as_bytes()).expect("could not write main.luau");
                }

                {
                    fs::create_dir(&project_path.join("assets")).expect("could not create assets");
                }

                println!("Created project \"{project_name}\" at {}.", project_path.to_str().unwrap());
                println!("To run, execute in the project directory the command `Neolove run`")
            },
            "run" => {
                let mut runtime = window::Runtime::new(env::current_dir().unwrap());
                runtime.start();

                let mut ct = SystemTime::now() .duration_since(SystemTime::UNIX_EPOCH) .unwrap() .as_secs_f64();
                loop {
                    clear_background(BLACK);
                    let ct2 = SystemTime::now() .duration_since(SystemTime::UNIX_EPOCH) .unwrap() .as_secs_f64();
                    let dt = ct2 - ct;
                    ct = ct2;
                    runtime.update(dt as f32);
                    next_frame().await;
                }
            },
            _ => println!("unrecognized"),
        }
    } else {
        println!("no arguments provided");
    }
}
