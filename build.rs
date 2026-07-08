fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/web_bridge.c");
    println!("cargo:rerun-if-env-changed=CC_wasm32-unknown-emscripten");
    println!("cargo:rerun-if-env-changed=CC_wasm32_unknown_emscripten");
    println!("cargo:rerun-if-env-changed=TARGET_CC");
    println!("cargo:rerun-if-env-changed=CC");
    println!("cargo:rerun-if-env-changed=AR_wasm32-unknown-emscripten");
    println!("cargo:rerun-if-env-changed=AR_wasm32_unknown_emscripten");
    println!("cargo:rerun-if-env-changed=TARGET_AR");
    println!("cargo:rerun-if-env-changed=AR");
    println!("cargo:rerun-if-env-changed=PATH");
    println!("cargo:rerun-if-env-changed=HOME");
    println!("cargo:rerun-if-env-changed=USERPROFILE");
    println!("cargo:rerun-if-env-changed=NEOLOVE_PACKAGED_RUNTIME");
    println!("cargo:rustc-check-cfg=cfg(neolove_packaged)");

    if std::env::var_os("NEOLOVE_PACKAGED_RUNTIME").is_some_and(|value| !value.is_empty()) {
        println!("cargo:rustc-cfg=neolove_packaged");
    }

    emit_git_revision();

    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if target_arch == "wasm32" && target_os == "emscripten" {
        let compiler = emscripten_compiler().unwrap_or_else(|| {
            panic!(
                "wasm32-unknown-emscripten builds require `emcc` on PATH or a configured \
                 CC_wasm32-unknown-emscripten/CC_wasm32_unknown_emscripten/TARGET_CC/CC"
            )
        });
        let archiver = emscripten_archiver().unwrap_or_else(|| {
            panic!(
                "wasm32-unknown-emscripten builds require `emar` on PATH or a configured \
                 AR_wasm32-unknown-emscripten/AR_wasm32_unknown_emscripten/TARGET_AR/AR"
            )
        });

        cc::Build::new()
            .compiler(compiler)
            .archiver(archiver)
            .file("src/web_bridge.c")
            .compile("neolove_web_bridge");

        let out_dir = std::path::PathBuf::from(
            std::env::var_os("OUT_DIR").expect("Cargo should set OUT_DIR for build scripts"),
        );
        println!(
            "cargo:rustc-link-arg-bin=neolove={}",
            out_dir.join("libneolove_web_bridge.a").display()
        );
        println!("cargo:rustc-link-lib=static=neolove_web_bridge");
    }
}

fn emit_git_revision() {
    let manifest_dir =
        std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let git_dir = resolve_git_dir(&manifest_dir);
    let head_path = git_dir.join("HEAD");
    println!("cargo:rerun-if-changed={}", head_path.display());
    if let Ok(head) = std::fs::read_to_string(&head_path)
        && let Some(reference) = head.trim().strip_prefix("ref: ")
    {
        println!(
            "cargo:rerun-if-changed={}",
            git_dir.join(reference).display()
        );
    }
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&manifest_dir)
        .output();
    let revision = output
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=NEOLOVE_GIT_REVISION={revision}");
}

fn resolve_git_dir(manifest_dir: &std::path::Path) -> std::path::PathBuf {
    let dot_git = manifest_dir.join(".git");
    if dot_git.is_dir() {
        return dot_git;
    }
    let Ok(contents) = std::fs::read_to_string(&dot_git) else {
        return dot_git;
    };
    let Some(path) = contents.trim().strip_prefix("gitdir: ") else {
        return dot_git;
    };
    let path = std::path::PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        manifest_dir.join(path)
    }
}

fn emscripten_compiler() -> Option<std::path::PathBuf> {
    configured_tool([
        "CC_wasm32-unknown-emscripten",
        "CC_wasm32_unknown_emscripten",
        "TARGET_CC",
        "CC",
    ])
    .or_else(|| find_tool_on_path(emcc_binary_name()))
    .or_else(|| find_local_emsdk_tool(emcc_binary_name()))
}

fn emscripten_archiver() -> Option<std::path::PathBuf> {
    configured_tool([
        "AR_wasm32-unknown-emscripten",
        "AR_wasm32_unknown_emscripten",
        "TARGET_AR",
        "AR",
    ])
    .or_else(|| find_tool_on_path(emar_binary_name()))
    .or_else(|| find_local_emsdk_tool(emar_binary_name()))
}

fn configured_tool<const N: usize>(variables: [&str; N]) -> Option<std::path::PathBuf> {
    variables
        .into_iter()
        .filter_map(std::env::var_os)
        .find(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
}

fn find_tool_on_path(tool_name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(tool_name))
            .find(|candidate| candidate.is_file())
    })
}

fn find_local_emsdk_tool(tool_name: &str) -> Option<std::path::PathBuf> {
    home_dir()
        .map(|home| {
            home.join(".neolove")
                .join("toolchains")
                .join("emsdk")
                .join("upstream")
                .join("emscripten")
                .join(tool_name)
        })
        .filter(|candidate| candidate.is_file())
}

fn home_dir() -> Option<std::path::PathBuf> {
    ["HOME", "USERPROFILE"]
        .into_iter()
        .filter_map(std::env::var_os)
        .find(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
}

fn emcc_binary_name() -> &'static str {
    #[cfg(windows)]
    {
        "emcc.bat"
    }

    #[cfg(not(windows))]
    {
        "emcc"
    }
}

fn emar_binary_name() -> &'static str {
    #[cfg(windows)]
    {
        "emar.bat"
    }

    #[cfg(not(windows))]
    {
        "emar"
    }
}
