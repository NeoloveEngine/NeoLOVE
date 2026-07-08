<h1 align="center">NeoLOVE</h1>

<p align="center">
  A Rust game engine for building 2D games with Luau.
</p>

NeoLOVE combines a Luau scripting runtime with an entity-component-system,
2D rendering, physics, audio, input, networking, and native, Android, iOS
simulator, or WebAssembly packaging. A game is a directory containing a `main.luau` entry point and an
optional `neolove.toml` configuration file.

> [!NOTE]
> NeoLOVE is in early development. APIs and project formats may change before
> a stable release.

## Features

- Luau scripting with generated type definitions
- Entities, hierarchy, components, systems, linked prefabs, tweening, and keyframe animation controllers
- Shapes, text, sprites, nine-slice sprites, particle images, tilemaps, tile textures, and custom shaders
- Rigidbody, collider, rope, raycasting, and pixel-shaped sprite queries
- Keyboard, mouse, audio, image, file system, HTTP, and server APIs
- A built-in visual scene editor inspired by Unity and Godot
- Standalone desktop executables with embedded game assets
- Signed Android APK builds with embedded game assets
- iOS simulator app builds for macOS/Xcode users
- Itch.io-ready WebAssembly bundles
- Mobile emulator mode for desktop testing of locked phone/tablet layouts
- External writable data directories for packaged desktop games
- Project-root restrictions for command working-directory paths

## Requirements

- A current stable [Rust toolchain](https://www.rust-lang.org/tools/install)
- Linux builds require ALSA and `pkg-config` development packages

On Debian or Ubuntu:

```bash
sudo apt-get install pkg-config libasound2-dev
```

## Install

Automated setup scripts install Git, native build dependencies, the stable Rust
toolchain, and NeoLOVE in a user-local application-data directory. They then
compile and launch the editor using the optimized release profile. Vulkan is
enabled automatically when a working runtime is detected.

On Linux or macOS:

```bash
./install.sh
```

Set `NEOLOVE_VULKAN=1` or `NEOLOVE_VULKAN=0` to override Vulkan detection.

On Windows PowerShell:

```powershell
.\install.ps1
```

Pass `-Vulkan On` or `-Vulkan Off` to override Vulkan detection. The Windows
installer also installs the Visual Studio 2022 Desktop C++ build workload.

Both installers are safe to re-run. Existing installations are updated without
overwriting local changes, interrupted installer staging is cleaned up, and a
recognizable incomplete NeoLOVE checkout is preserved as a timestamped backup
before a fresh clone is created.

### Manual install

Build and install the CLI from source:

```bash
git clone https://github.com/NeoloveEngine/NeoLOVE.git
cd NeoLOVE
cargo install --path .
neolove --version
```

Use `cargo install --path . --features vulkan` if you need Vulkan rendering and
custom desktop shaders. The smaller software renderer is used by default.

## Quick Start

Create and run a project:

```bash
neolove new my-game
cd my-game
neolove run
```

New projects include this structure:

```text
my-game/
|-- .luaurc
|-- .vscode/
|   `-- settings.json
|-- assets/
|-- main.luau
|-- neolove.toml
`-- types/
    `-- neolove_engine_api.d.luau
```

Replace `main.luau` with a simple scene:

```luau
app.bg = Color4(24, 26, 32)

local box = ecs.newEntity("box", ecs.root, 100, 100)
box.size_x = 160
box.size_y = 90

local rectangle = box:AddComponent(core.Rect2D)
rectangle.color = Color4(80, 140, 255)
```

`run` and `build` require `main.luau` at the project root.

## Visual Editor

NeoLOVE ships with a built-in visual scene editor, similar in spirit to the
Unity or Godot editors:

```bash
neolove editor          # edit the project in the current directory
neolove editor my-game  # edit a specific project
```

The editor opens a window with a dockable or detachable **Hierarchy**, a 2D
**Viewport**, an **Inspector**, and a bottom **Project** file browser:

- Build scenes from entities and the real engine components — `Rect2D`,
  `Shape2D`, `ParticleSystem2D`, `AnimationController`, `SpatialSound2D`,
  `TextBox`, `Sprite2D`, `NineSliceSprite2D`, `Tilemap2D`, `TileTexture2D`,
  `Collider2D`, `Rigidbody2D`, `Bolt2D`, `Rope2D` — added from a dropdown,
  each with its inspector-editable properties (advanced fields collapse away).
- Nest entities into a hierarchy by dragging rows; set per-entity `z` order and
  `scale`; reorder, duplicate, copy/paste and rename via right-click menus.
- Attach a `Script` component to expose **public variables** edited in the
  inspector — including `IImage`, `IAudio`, `IShader`, and `IAnimation` asset
  handles for custom scripts.
- Edit the scene background (`app.bg`) with a color picker; it previews live in
  the viewport. The viewport shows the configured default game-window bounds.
- Use move, scale, and rotate scene tools with explicit handles. Holding `Ctrl`
  while moving a parent keeps descendants in their world positions.
- Dock, undock, close, and restore the Hierarchy, Inspector, and Project
  browser from the Window menu; resize panels with draggable splitters.
- Browse, create, and open project files from the bottom Project panel; reveal
  folders in your OS file manager. Create shader and animation assets from the
  editor, open `.neoanim` clips in the Bezier animation editor, and toggle the
  grid overlay and grid snapping.
- Image components (`Sprite2D`, `Image2D`, `NineSliceSprite2D`, `TileTexture2D`)
  and `ParticleSystem2D` emitters load and preview their real assets in the
  viewport (with true 9-slice, tiling, and particle sprites). Paint `Tilemap2D`
  tiles directly inside selected tilemap entities. Copy/paste components
  between entities. Save a prefab by dragging an entity onto the Project panel,
  and drag a `.neoprefab` back into the viewport to instantiate it.
- Right-click almost anything for a context menu; hover any control for a
  tooltip; unsaved changes prompt before New/Load/Quit.
- Unity-style quality-of-life: undo/redo (Ctrl+Z / Ctrl+Y), duplicate (Ctrl+D),
  frame-selected (F), reset view (0), rename (F2), arrow-key nudge (Shift =
  grid step), scroll-wheel zoom, a hierarchy search box, per-entity active
  toggles (excluded from export), Reset-Transform, and a live transform/zoom
  overlay in the viewport.

Scenes can also be loaded at runtime from Luau:

```luau
ecs.loadScene("scene.neoscene")
```

Scenes are saved as `scene.neoscene` (JSON). **Export main.luau** generates a
runnable entry point from the scene, **Run** launches a live preview, and
**Build** exports then asks whether to package for desktop, WebAssembly,
Android, or iOS. The **Mobile** control runs previews in a locked phone-sized
viewport with portrait/landscape rotation and Wi-Fi/cellular/low-power toggles.

Project window defaults live in `neolove.toml` under `[window]`:

```toml
[window]
title = "My Game"
width = 1280
height = 720
fullscreen = false
resizable = true
```

Global editor preferences are stored in your user config directory
(`%APPDATA%\NeoLOVE\editor.json` on Windows,
`~/Library/Application Support/NeoLOVE/editor.json` on macOS, or
`$XDG_CONFIG_HOME/neolove/editor.json` / `~/.config/neolove/editor.json` on
Linux). The Settings button opens editor-wide theme, font, tooltip, overlay,
and autosave options. Older project-local `editor.json` files are still read as
a fallback.

## CLI

| Command | Description |
| --- | --- |
| `neolove new <project-name>` | Create a new project |
| `neolove run [project-dir]` | Run a project |
| `neolove run [project-dir] --mobile` | Run with the locked mobile emulator |
| `neolove editor [project-dir]` | Open the visual scene editor |
| `neolove build [project-dir]` | Build a standalone desktop executable |
| `neolove build [project-dir] --webasm` | Build an HTML5 bundle and upload zip |
| `neolove build [project-dir] --android` | Build a signed Android APK |
| `neolove build [project-dir] --ios` | Build an iOS simulator app on macOS |
| `neolove api [project-dir]` | Refresh the Luau API type definitions |
| `neolove update` | Pull, rebuild, and install the latest engine revision |
| `neolove setup-path` | Add NeoLOVE to the user PATH |
| `neolove --help` | Show CLI usage |
| `neolove --version` | Print the installed version |

The visual editor checks its tracked Git branch for updates in the background
when it opens. If a newer revision is available, it offers to run
`neolove update` and restart. Updates require a clean engine source checkout;
local engine changes must be committed or stashed first.

## Runtime API

NeoLOVE exposes its APIs as Luau globals:

| Area | Globals |
| --- | --- |
| Application and input | `app`, `input`, `userInput`, `mouse`, `window` |
| Entities and transforms | `ecs`, `core`, `transform`, `transforms` |
| Assets and audio | `assets`, `audio` |
| Files, platform, and processes | `fs`, `android`, `mobile`, `commands`, `command` |
| Networking | `http`, `servers` |
| Gameplay helpers | `prefabs`, `prefab`, `tweening`, `tween`, `animation`, `animations` |
| Rendering | `shaders` |

The complete typed API is defined in
[`neolove_engine_api.d.luau`](neolove_engine_api.d.luau). Running
`neolove api` copies the current definitions into a project's `types/`
directory for Luau language-server support.

## Building Games

Build a standalone executable:

```bash
neolove build
```

The executable is written to `dist/<project-name>` (`.exe` on Windows) and
contains the game files and assets.

Build for the web:

```bash
neolove build --webasm
```

This creates:

```text
dist/
|-- <project-name>-webasm.zip
`-- webasm/
    |-- index.html
    |-- neolove.data
    |-- neolove.js
    `-- neolove.wasm
```

Serve the web bundle over HTTP for local testing:

```bash
cd dist/webasm
python3 -m http.server 8000
```

Then open `http://localhost:8000`. Browsers will not reliably run the bundle
from a `file://` URL. The first web build may install the Emscripten Rust target
and a local toolchain under `~/.neolove/toolchains/emsdk`.

Build an Android APK:

```bash
neolove build --android
```

This creates `dist/<project-name>-android-arm64.apk`. The first Android build
may install the Android Rust target plus a local JDK, SDK, build-tools, and NDK
under `~/.neolove/toolchains/`. `--apk` is accepted as an alias for `--android`.

Build an iOS simulator app on macOS:

```bash
neolove build --ios
```

This creates `dist/<project-name>-ios-simulator.app` using Xcode's
`iphonesimulator` SDK. The command is only available on macOS with Xcode
installed.

## Asset Support

- Images: PNG, JPEG, GIF, BMP, TGA, TIFF, PNM, WebP, HDR, and DDS
- Audio: WAV, MP3, OGG/Vorbis, and FLAC natively, plus browser codecs on web
- Native audio: WAV
- Browser audio: WAV and browser-decodable MP3, OGG, FLAC, AAC/M4A, and AIFF

Browser audio may require a user interaction before playback begins.

## Writable Game Data

During development, relative `fs` paths and image/sound exports use the project
directory. A packaged desktop executable instead creates a writable
`<game-name>_data` directory beside the executable. Relative writes and exports
go there, while reads fall back to bundled project resources.

Use `fs.getDataDirectory()` to inspect that location or `fs.dataPath("save.json")`
to build a path for APIs such as image and sound export. Absolute paths and
normalized parent-relative paths are also supported by `fs` and asset export.
They are not restricted to the project, data, or executable directory; normal
operating-system permissions still apply. See [`docs.md`](docs.md) for path and
async task examples.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features
cargo test --all-targets --all-features
cargo check --target wasm32-unknown-unknown
```

Release builds use size-oriented optimization, fat LTO, a single codegen unit,
stripped symbols, and a deflated embedded project payload so image and audio
assets are compressed in standalone builds. Desktop game exports rebuild a
compact packaged runtime before appending the payload. Web upload ZIPs use
deflate compression as well.

## License

NeoLOVE is licensed under the [GNU AGPL v3](LICENSE).
