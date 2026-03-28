#[cfg(not(target_arch = "wasm32"))]
include!("main_desktop.rs");

#[cfg(target_arch = "wasm32")]
fn main() {
    panic!(
        "NeoLOVE currently supports WebAssembly builds as a compile target bootstrap only; the desktop runtime is not available on wasm yet."
    );
}
