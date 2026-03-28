# Vulkan Backend Notes

`macroquad` has been removed from the engine runtime.

The current backend split is:

- [`src/main.rs`](/home/khakixd/Documents/apps/NeoLOVE/src/main.rs): `winit` event loop and Vulkan swapchain presentation.
- [`src/platform.rs`](/home/khakixd/Documents/apps/NeoLOVE/src/platform.rs): engine-owned window, mouse, input, and frame state.
- [`src/renderer.rs`](/home/khakixd/Documents/apps/NeoLOVE/src/renderer.rs): draw queue plus software rasterization for 2D primitives, images, and text.
- [`src/assets.rs`](/home/khakixd/Documents/apps/NeoLOVE/src/assets.rs): CPU-side image/audio assets.
- [`src/audio_system.rs`](/home/khakixd/Documents/apps/NeoLOVE/src/audio_system.rs): `rodio` playback backend.

Current rendering model:

1. Lua components enqueue draw commands into the engine render state.
2. The software renderer rasterizes the frame into an RGBA buffer.
3. Vulkan presents that buffer by copying it into the swapchain image.

This removes the runtime dependency on `macroquad`, but it is not a feature-identical renderer yet. In particular:

- custom shader handles are preserved at the API layer but are not executed by the new renderer yet
- the Vulkan path currently acts as the presenter for the engine framebuffer rather than replacing all 2D rasterization with GPU pipelines

The important constraint for this port was satisfied: the engine no longer depends on `macroquad` for windowing, input, rendering, textures, or audio.
