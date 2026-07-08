#![cfg_attr(neolove_packaged, allow(dead_code, unused_mut, unused_variables))]

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "android")))]
include!("main_desktop.rs");

#[cfg(all(target_arch = "wasm32", target_os = "emscripten"))]
include!("main_web.rs");

#[cfg(target_os = "android")]
fn main() {}

#[cfg(all(target_arch = "wasm32", not(target_os = "emscripten")))]
fn main() {
    panic!(
        "NeoLOVE currently supports WebAssembly builds as a compile target bootstrap only; the desktop runtime is not available on wasm yet."
    );
}
