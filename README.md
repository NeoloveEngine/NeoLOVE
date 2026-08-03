<h1 align="center">NeoLOVE</h1>

<p align="center">
  A Rust game engine for building 2D games and early 3D projects with Luau.
</p>

NeoLOVE combines a Luau scripting runtime with an entity-component-system,
2D/3D rendering, physics, audio, input, networking, and native, Android, iOS
simulator, or WebAssembly packaging. A game is a directory containing a `main.luau` entry point and an
optional `neolove.toml` configuration file.

> [!NOTE]
> NeoLOVE is in early development. APIs and project formats may change before
> a stable release.

## Features

- Luau scripting with generated type definitions
- Entities, hierarchy, components, systems, linked prefabs, tweening, and keyframe animation controllers
- Shapes, text, sprites, nine-slice sprites, particle images, tilemaps, tile textures, and custom shaders
- Deterministic topmost widget routing, drag capture, exclusive focus, Tab traversal,
  and keyboard-operable buttons, sliders, dropdowns, scrolling lists, and text inputs
- Rigidbody, collider, rope, raycasting, and pixel-shaped sprite queries
- Euler-authored 3D meshes and primitives, cameras, configurable environments,
  skinned animation, 3D particles, point/spot/directional lights, depth-tested
  rendering, portable fragment material shaders, configurable antialiasing,
  live mesh edits, and exact triangle-mesh raycasts
- An ordered post-process stack with bloom, pixelation, chromatic aberration,
  motion blur, quantization/dithering, vignette, color adjustment, and tonemapping
- High-DPI logical rendering, bounded high-resolution light maps, lazy 3D depth
  allocation, copy-on-write texture snapshots, and configurable frame pacing
- Keyboard, mouse, microphone, camera, audio, image, file system, HTTP, and server APIs
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
- Linux builds require ALSA development files, `pkg-config`, Clang/libclang,
  and Linux V4L2 headers for microphone and camera capture

On Debian or Ubuntu:

```bash
sudo apt-get install pkg-config libasound2-dev clang libclang-dev linux-libc-dev
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
neolove new --2d my-game # --2d is optional and remains the default
cd my-game
neolove run
```

Create a 3D project from either the CLI or the Hub:

```bash
neolove new --3d my-3d-game
```

The choice is stored as `kind = "2d"` or `kind = "3d"` under `[project]` in
`neolove.toml`; legacy projects without the field remain 2D.

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
neolove hub             # open the project Hub
neolove editor          # edit the project in the current directory
neolove editor my-game  # edit a specific project
```

The Hub is the GUI launcher for creating a project, loading a project folder,
or reopening recent projects. Running the desktop NeoLOVE executable also
refreshes a user application-launcher entry that opens this Hub directly
(Start Menu on Windows, `~/Applications` on macOS, and the XDG application menu
on Linux desktop environments).

The editor opens a window with a dockable or detachable **Hierarchy**, a
**Viewport**, an **Inspector**, and a bottom **Project** file browser:

- Build scenes from entities and the real engine components — `Rect2D`,
  `Shape2D`, `ParticleSystem2D`, `AnimationController`, `SpatialSound2D`,
  `TextBox`, `TextInput`, `Panel`, `Button`, `Slider`, `Dropdown`, `ScrollList`,
  `Sprite2D`, `SpriteSheet2D`,
  `NineSliceSprite2D`, `Tilemap2D`, `TileTexture2D`, `Collider2D`,
  `Rigidbody2D`, `Bolt2D`, `Rope2D`, `Light2D`, `LightOccluder2D`, and
  `Camera`, plus `MeshRenderer3D`, `Camera3D`, `Environment3D`/`Skybox3D`,
  `ParticleSystem3D`, `Light3D`, `Rigidbody3D`, and `Collider3D` in 3D scenes —
  added from a dropdown, each with its
  inspector-editable properties (advanced fields collapse away).
- A 3D scene exposes X/Y/Z position, XYZ Euler rotation in degrees, and
  per-axis scale in the Inspector. Scene save/export preserves whether it is a
  2D or 3D scene.
- In the 3D viewport, Move provides X/Y/Z axis handles plus a free center drag,
  Scale provides X/Y/Z handles plus uniform center scaling, and Rotate provides
  X/Y/Z rings that edit the matching Euler angles. Grid snapping applies to
  viewport transforms when enabled.
- Nest entities into a hierarchy by dragging rows; set per-entity `z` order and
  `scale`; reorder, duplicate, copy/paste and rename via right-click menus.
- Attach a `Script` component to expose **public variables** edited in the
  inspector — including `IImage`, `IAudio`, `IShader`, and `IAnimation` asset
  handles for custom scripts.
- Add arbitrary typed values directly to an entity—numbers, strings, booleans,
  colors, nested lists/tables, entities, components, images, sounds, shaders,
  and animations—so editor-authored fields are read as `entity.foo` in code.
- Add, rename, reorder, or remove `Dropdown` options directly in the Inspector.
- Edit the scene background (`app.bg`) with a color picker; it previews live in
  the viewport. Scene lighting also previews live and can be disabled only for
  the editor viewport in Editor Settings. The viewport shows the configured
  default game-window bounds.
- Use move, scale, and rotate scene tools with explicit handles. Holding `Ctrl`
  while moving a parent keeps descendants in their world positions.
- Dock, undock, close, and restore the Hierarchy, Inspector, and Project
  browser from the Window menu; resize panels with draggable splitters.
- Browse, create, and open project files from the bottom Project panel; reveal
  folders in your OS file manager. Create shader and animation assets from the
  editor, open `.neoanim` clips in the Bezier animation editor, and toggle the
  grid overlay and grid snapping.
- Image components (`Sprite2D`, `SpriteSheet2D`, `Image2D`, `NineSliceSprite2D`, `TileTexture2D`)
  and `ParticleSystem2D` emitters load and preview their real assets in the
  viewport (with true 9-slice, tiling, and particle sprites). Paint `Tilemap2D`
  tiles directly inside selected tilemap entities. Copy/paste components
  between entities. Save a prefab by dragging an entity onto the Project panel,
  and drag a `.neoprefab` back into the viewport to instantiate it.
- Right-click almost anything—including empty viewport space and 3D entities—
  for a context menu; RMB fly-look remains available after the pointer crosses
  the click/drag threshold. Hover any control for a tooltip; unsaved changes
  prompt before New/Load/Quit.
- Unity-style quality-of-life: undo/redo (Ctrl+Z / Ctrl+Y), duplicate (Ctrl+D),
  frame-selected (F), reset view (0), rename (F2), arrow-key nudge (Shift =
  grid step), scroll-wheel zoom, a hierarchy search box, per-entity active
  toggles (excluded from export), Reset-Transform, and a live transform/zoom
  overlay in the viewport.

Scenes can also be loaded at runtime from Luau:

```luau
ecs.loadScene("scene.neoscene")
```

Scenes are saved as compressed `scene.neoscene` documents (legacy JSON remains
readable). **Export main.luau** generates a runnable entry point from the scene,
**Run** launches a live preview, and
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
Linux). The compact scene menu opens editor-wide settings with live theme
previews, an editable persistent custom palette, and a native font-file picker;
font changes apply immediately. Tooltip, overlay, lighting-preview, and autosave
preferences live there too. Viewport camera sensitivity, movement speed, field
of view, and inverted vertical mouse look are persisted there. Sensitivity
affects 2D middle-button panning and 3D mouse look/panning; speed controls 3D
fly and dolly movement, and FOV is applied directly to the perspective preview.
Older project-local `editor.json` files are still read as a fallback.

## CLI

| Command | Description |
| --- | --- |
| `neolove hub` | Open the project Hub |
| `neolove new [--2d\|--3d] <project-name>` | Create a 2D (default) or 3D project |
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
| `neolove setup-start-menu` | Refresh the user application-launcher entry |
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
| Assets, audio, and capture | `assets`, `audio`, `media`, `microphone` |
| Files, platform, and processes | `fs`, `android`, `mobile`, `commands`, `command` |
| Networking | `http`, `servers` |
| Gameplay helpers | `prefabs`, `prefab`, `tweening`, `tween`, `animation`, `animations` |
| Rendering | `shaders`, `lighting`, `postprocess`, `postProcess`, `environment3d`, `environment3D`, `skybox` |
| 3D physics queries | `physics3d`, `physics3D` |

### 3D meshes, lighting, and cameras

Entities reuse `x` and `y` for the first two position axes and add
`position_z`. The 3D transform fields are `rotation_x`, `rotation_y`, and
`rotation_z` (XYZ Euler angles in degrees), plus `scale_x`, `scale_y`, and
`scale_z`. They are independent of the legacy 2D `rotation`, uniform `scale`,
and draw-order `z` fields. World-space values are available through
`entity:GetWorldPosition3D()`, `entity:GetWorldRotation3D()`, and the matching
functions on `transform`.

```luau
local cameraEntity = ecs.newEntity("Camera", ecs.root, 0, 0)
cameraEntity.position_z = 5
local camera = cameraEntity:AddComponent(core.Camera3D)
camera.fov = 70

local lightEntity = ecs.newEntity("Key light", ecs.root, 3, 4)
lightEntity.position_z = 5
local light = lightEntity:AddComponent(core.Light3D)
light.kind = "point"
light.intensity = 2
light.range = 20

local modelEntity = ecs.newEntity("Model", ecs.root, 0, 0)
local renderer = modelEntity:AddComponent(core.MeshRenderer3D)
renderer.mesh = assets.loadMesh("assets/model.glb")
renderer.texture = assets.loadImage("assets/albedo.png")
if shaders.supports3DShaders() then
    renderer.shader = shaders.load3DFragment("assets/model.frag") -- optional
end
app.setAntiAliasing("high")
```

`Camera3D` supports perspective and orthographic projections, near/far clips,
and explicit `SetActive()` selection. `Light3D` supports point, spot, and
directional diffuse lighting. `MeshRenderer3D` accepts an imported or
script-created mesh, tint, base texture, custom mesh shader, and double-sided
rendering. A readable headlight is used when a scene has no `Light3D`.
Mesh fragment shaders share `uv`, lit/tinted `color`, the base `Texture`
sampler, and the normal shader-uniform methods across Vulkan and WebGL. Native
software-only builds retain unshaded 3D and report unsupported GLSL explicitly.
The global `off`/`standard`/`high` antialiasing setting controls software 3D
edge smoothing, Vulkan MSAA, and WebGL shader-surface quality without blurring
the 2D overlay stream. See `docs.md` for the portable shader contract and exact
backend table.
The focused [`3d-shaders-aa`](../samples/3d-shaders-aa) project demonstrates
the portable material and live quality switching end to end.

### Primitive meshes and skeletal animation

The editor's `MeshRenderer3D` Inspector can create a cube, sphere, plane,
cylinder, capsule, or cone without an external model. The same cached meshes
are available to scripts:

```luau
local sphere = assets.primitiveMesh("sphere", {
    radius = 0.75,
    segments = 32,
    rings = 16,
})
renderer.mesh = sphere
```

Editor-authored mesh components default to a cube. Script-created components
default to `primitive = "none"`, preserving direct `renderer.mesh = ...`
assignment; set `primitive` explicitly when a script wants generated geometry.

Imported glTF/GLB skins and animation clips can be played directly. Each
`MeshRenderer3D` automatically takes a detached pose copy when its `animation`
field is set, so instances can animate independently:

```luau
renderer.mesh = assets.loadMesh("assets/character.glb")
renderer.animation = "Walk"
renderer.animation_looping = true
renderer.animation_speed = 1.0
renderer:PlayAnimation()

for _, name in ipairs(renderer.mesh:animationNames()) do
    print(name, renderer.mesh:animationDuration(name))
end
```

Scripts that drive a mesh handle manually can use `cloneDetached()`,
`sampleAnimation`, `playAnimation`, `updateAnimation`, `pauseAnimation`, and
`stopAnimation`. Skinning currently runs on the CPU and commits a revisioned
vertex snapshot that all rendering backends observe.

### Live mesh editing

`assets.loadMesh` caches a shared, revisioned `MeshHandle`; edits made through
one handle are visible to every renderer and mesh collider using that identity.
`assets.newMesh(vertices, indices)` creates geometry from one-based Luau index
tables. Omitting indices treats every three vertices as a triangle.

```luau
local mesh = assets.loadMesh("assets/terrain.obj")

-- Vertex and index APIs are one-based. Successful edits return the new revision.
mesh:setPosition(1, -1, 0.5, 0, true)
mesh:setIndex(1, 3)
mesh:recomputeNormals()

local vertex = mesh:getVertex(1) -- x/y/z, nx/ny/nz, u/v, tx/ty/tz/tw
local bounds = mesh:bounds()

-- Material slots are created on demand. Texture sources are editable metadata;
-- assign an ImageHandle to MeshRenderer3D.texture for the rendered base texture.
mesh:setMaterialColor(1, Color4(255, 180, 120))
mesh:setMaterialTexture(1, "base_color", "assets/albedo.png")
```

`replaceGeometry`, `setVertex`, `setPosition`, and `setIndex` validate a new
snapshot before committing it, so a failed edit does not corrupt the live mesh.
`assets.unloadMesh(path)` evicts the cache entry but does not invalidate handles already
held by scripts or components.

Mesh textures are live handles too: `ImageHandle:setPixel(x, y, color)` and
`ImageHandle:fill(color)` increment the image revision. Software rendering sees
the new pixels immediately, while Vulkan and WebGL refresh their cached texture
the next time the mesh is drawn without replacing the handle.

### Environments and 3D particles

New 3D scenes include an editable `Environment3D` component. It supports a
solid color, vertical gradient, or camera-rotated equirectangular panorama:

```luau
skybox.setEquirectangular(assets.loadImage("assets/studio-panorama.png"), 25)
skybox.setIntensity(1.2)
-- `environment3d` and `environment3D` are aliases of `skybox`.
```

`ParticleSystem3D` uses a bounded native particle pool and submits one batched
billboard command per emitter. Point, box, sphere, and cone emission shapes,
lifetime/speed ranges, gravity, drag, size/color fades, rotation, deterministic
seeds, textures, looping, and manual `Emit(count)` are editable in the
Inspector and available in scripts.

### 3D colliders and queries

`Collider3D` supports box, sphere, capsule, and triangle-mesh colliders. Mesh
colliders keep a BVH and refresh it when their `MeshHandle` revision changes.
Filters use integer `layer` and `mask` fields. Raycasts test the exact triangle
surface rather than the mesh bounds; mesh-to-body contacts remain bounds-only
as described below.

```luau
local collider = modelEntity:AddComponent(core.Collider3D)
collider.shape = "mesh"
collider.mesh = renderer.mesh

local hit = physics3d.raycast(0, 2, 5, 0, -0.25, -1, 100, {
    layer = 1,
    mask = 0xFFFFFFFF,
    include_triggers = false,
})
if hit then
    print(hit.entity_id, hit.distance, hit.triangle_index, hit.barycentric)
end

for _, pair in ipairs(physics3d.broadphasePairs()) do
    print(pair.first_id, pair.second_id, pair.has_trigger)
end

for _, contact in ipairs(physics3d.contacts({ include_bounds = true })) do
    print(contact.first_id, contact.second_id, contact.quality, contact.penetration)
end
```

`Rigidbody3D` integrates linear/angular velocity, gravity, damping, axis locks,
and Euler rotation. Its lightweight contact layer performs deterministic
narrow-phase tests and automatic positional/velocity response (including
restitution and friction) for box-box, sphere-sphere, sphere-box,
capsule-sphere, and capsule-capsule contacts. `onContact` and `onTrigger`
callbacks receive the same contact records exposed by `physics3d.contacts()`.

Mesh contacts, capsule-box contacts, and non-uniformly scaled sphere/capsule
contacts are explicitly marked `quality = "bounds"`; they are useful for
triggers and diagnostics but are never auto-resolved. The solver does not yet
provide continuous collision detection, angular/contact manifolds, or full
mass-coupled multi-body impulse resolution.

### Post-processing

The ordered post-process stack uses one-based pass indices:

```luau
postprocess.clear()
postprocess.setEnabled(true)
local bloom = postprocess.add("bloom", {
    threshold = 0.75,
    intensity = 0.9,
    radius = 5,
})
postprocess.add("chromatic_aberration", {
    offset_pixels = 1.5,
    angle_degrees = 0,
})
postprocess.add("quantization", { levels = 16, dither_strength = 0.35 })
postprocess.move(bloom, postprocess.count())
postprocess.setPassEnabled(bloom, false)
```

Available effects are bloom, pixelation, chromatic aberration (the legacy
`chromatic_abberation` spelling is accepted), motion blur, quantization with
dithering, vignette, grayscale, invert, brightness/contrast/saturation, and
exposure tonemapping (`none`, `reinhard`, or `aces`). Motion blur history can be
discarded with `postprocess.resetHistory()`.

### Current 3D/backend limits

The 3D implementation is a usable foundation, but it should not yet be treated
as feature-parity with a mature 3D engine:

- OBJ imports triangle geometry, UVs/normals, groups, and `usemtl` names, but
  does not load `.mtl` libraries. glTF 2.0/GLB imports uncompressed triangle
  primitives and PBR material/texture metadata. ASCII and binary FBX import
  mesh control points and polygons; binary versions before and after FBX 7500
  accept raw or zlib-compressed numeric arrays.
- glTF/GLB imports one flattened skin per mesh asset, joint weights, inverse-bind
  matrices, and LINEAR/STEP translation, rotation, and scale clips. ASCII FBX
  supports a practical one-skinned-geometry subset of Model/Skin/Cluster data
  and XYZ animation curves. Multi-skin assets, glTF CUBICSPLINE animation,
  morph targets, compressed or sparse glTF accessors, FBX materials, and binary
  FBX armature/animation data are not yet supported. Imported texture paths are
  metadata and are not automatically loaded into `MeshRenderer3D.texture`.
- The software renderer has a per-pixel 3D depth buffer and a lazy depth-aware
  edge-AA pass. Vulkan uses a configurable MSAA-compatible D16 depth attachment
  and depth-tested GPU rasterization, including custom fragment materials, but
  transform/lighting remains CPU-side and projected triangles are streamed in
  non-indexed batches rather than cached indexed GPU mesh buffers. WebAssembly
  uses a depth-tested, antialiased WebGL bridge for custom-shader meshes;
  ordinary unshaded meshes use the software rasterizer before compositing.
- Environment textures are equirectangular panoramas rather than six-face
  cubemaps. Particle simulation and skeletal skinning are CPU-side; particles
  are sorted within each emitter, not globally between emitters.
- The post-process stack runs on the deterministic software framebuffer path.
  On web, 2D lighting or an active post-process pass conditionally captures the
  complete canvas—including Canvas, WebGL, software-mesh, and text output—then
  applies CPU lighting/effects and presents the result. Ordinary web frames
  avoid that readback. Native Vulkan currently does not apply post-process
  effects; full-frame GPU ping-pong passes are not wired yet. Likewise,
  `casts_shadows`, normal maps, metallic/roughness shading,
  and 3D shadow maps are present as authoring metadata but are not yet evaluated
  by the 3D renderer.
- The editor provides a perspective imported/primitive-mesh preview, live
  environment preview, point/spot/directional lighting, recognizable camera/
  light/collider proxies, depth-aware picking, model drag-and-drop, X/Y/Z move,
  scale, and Euler-rotation handles, and RMB fly navigation with WASD/QE. The
  camera-centered adaptive ground grid uses bounded fine/coarse line budgets
  to appear continuous while moving and clips lines at the camera near plane.
  Mesh textures, custom shaders, and post-processing are not yet previewed in
  the editor.

Servers can be declared in-process as class-like services; no separate server
script is needed:

```luau
local Chat = servers.define({
    onMessage = function(self, client, event, data)
        if event == "chat" then
            self.hostHandle:emit("chat", { from = client.key, text = data.text })
        end
    end,
    onStart = function(self, host)
        self.hostHandle = host
    end,
})

local host = Chat:host(4040)
local client = Chat:connect(host.url)
client:on("chat", function(message) print(message.text) end)
client:emit("chat", { text = "hello" })
```

The complete typed API is defined in
[`neolove_engine_api.d.luau`](src/project_template/neolove_engine_api.d.luau). Running
`neolove api` copies the current definitions into a project's `types/`
directory for Luau language-server support.

## Building Games

Build a standalone executable:

```bash
neolove build
```

The executable is written to `dist/<project-name>` (`.exe` on Windows) and
contains the game files and assets.

From Linux, `neolove build --windows` builds a Windows `.exe` when the
`x86_64-pc-windows-gnu` Rust target and MinGW-w64 linker are available. From
Windows, `neolove build --linux` builds a Linux executable when a Linux GNU
cross linker is available. Linux-to-Windows builds statically link the MinGW
C/C++ runtimes used by Luau, so the output remains a standalone executable and
does not require `libstdc++-6.dll` beside it.

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
- Meshes: OBJ, glTF 2.0 (`.gltf`), binary glTF (`.glb`), and ASCII/binary FBX
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

Release builds use performance-oriented optimization, fat LTO, a single codegen unit,
stripped symbols, and a deflated embedded project payload so image and audio
assets are compressed in standalone builds. Desktop game exports rebuild a
compact packaged runtime before appending the payload. Web upload ZIPs use
deflate compression as well.

## License

NeoLOVE is licensed under the [GNU AGPL v3](LICENSE).
