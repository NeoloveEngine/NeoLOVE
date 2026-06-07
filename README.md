<h1 align="center">NeoLOVE</h1>

<p align="center">
  A Rust game engine for building 2D games with Luau.
</p>

NeoLOVE combines a Luau scripting runtime with an entity-component-system,
2D rendering, physics, audio, input, networking, and native or WebAssembly
packaging. A game is a directory containing a `main.luau` entry point and an
optional `neolove.toml` configuration file.

> [!NOTE]
> NeoLOVE is in early development. APIs and project formats may change before
> a stable release.

## Features

- Luau scripting with generated type definitions
- Entities, hierarchy, components, systems, prefabs, and tweening
- Shapes, text, sprites, nine-slice sprites, tile textures, and custom shaders
- Rigidbody, collider, rope, raycasting, and pixel-shaped sprite queries
- Keyboard, mouse, audio, image, file system, HTTP, and server APIs
- Standalone desktop executables with embedded game assets
- Itch.io-ready WebAssembly bundles
- Project-root restrictions for file and command working-directory paths

## Requirements

- A current stable [Rust toolchain](https://www.rust-lang.org/tools/install)
- Linux builds require ALSA and `pkg-config` development packages

On Debian or Ubuntu:

```bash
sudo apt-get install pkg-config libasound2-dev
```

## Install

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

## CLI

| Command | Description |
| --- | --- |
| `neolove new <project-name>` | Create a new project |
| `neolove run [project-dir]` | Run a project |
| `neolove build [project-dir]` | Build a standalone desktop executable |
| `neolove build [project-dir] --webasm` | Build an HTML5 bundle and upload zip |
| `neolove api [project-dir]` | Refresh the Luau API type definitions |
| `neolove setup-path` | Add NeoLOVE to the user PATH |
| `neolove --help` | Show CLI usage |
| `neolove --version` | Print the installed version |

## Runtime API

NeoLOVE exposes its APIs as Luau globals:

| Area | Globals |
| --- | --- |
| Application and input | `app`, `input`, `userInput`, `mouse`, `window` |
| Entities and transforms | `ecs`, `core`, `transform`, `transforms` |
| Assets and audio | `assets`, `audio` |
| Files and processes | `fs`, `commands`, `command` |
| Networking | `http`, `servers` |
| Gameplay helpers | `prefabs`, `prefab`, `tweening`, `tween` |
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

## Asset Support

- Images: PNG, BMP, TGA, PNM, and WebP
- Native audio: WAV
- Browser audio: WAV and browser-decodable MP3, OGG, FLAC, AAC/M4A, and AIFF

Browser audio may require a user interaction before playback begins.

## Examples

The [`samples`](samples) directory includes projects covering:

- [`dodge`](samples/dodge) and [`blackjack`](samples/blackjack)
- [`rigidbody2d`](samples/rigidbody2d) and
  [`raycasting`](samples/raycasting)
- [`spriteboxes`](samples/spriteboxes) and [`shaders`](samples/shaders)
- [`tweening`](samples/tweening) and [`webasm_smoke`](samples/webasm_smoke)

Run any sample by passing its directory to the CLI:

```bash
neolove run samples/dodge
```

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features
cargo test --all-targets --all-features
cargo check --target wasm32-unknown-unknown
```

Release builds use size optimization, fat LTO, a single codegen unit, stripped
binaries, and abort-on-panic behavior.

## License

NeoLOVE is licensed under the [GNU AGPL v3](LICENSE).
