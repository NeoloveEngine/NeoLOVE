<!-- page: overview | Overview -->
# NeoLOVE Documentation

NeoLOVE is a 2D game engine written in Rust and scripted with Luau. It combines
an entity-component runtime, rendering, physics, audio, input, networking,
filesystem access, prefab and animation helpers, a visual scene editor, and
packaging for desktop, WebAssembly, Android, and the iOS simulator.

This manual documents the complete user-facing surface:

- every CLI command and option;
- every `neolove.toml` setting;
- the project, scene, prefab, animation, and editor files;
- runtime order and path-resolution rules;
- every NeoLOVE Luau global, module, handle, function, field, callback, alias,
  and core component;
- platform differences, build outputs, and supported asset formats; and
- task-oriented, per-function reference entries with parameters, results,
  failure modes, edge cases, and runnable examples.

::: info
The API reference describes the implemented runtime. Names beginning with `_`
or `__` are also identified where they are visible, but they are engine-managed
implementation hooks and are not stable gameplay APIs unless a section says
otherwise. Internal Rust functions and structs are not part of the Luau or CLI
contract.
:::

## Runtime globals at a glance

| Area | Globals |
| --- | --- |
| Application | `app`, `window`, `mouse` |
| Input and platform | `input`, `userInput`, `android`, `mobile` |
| Entities and transforms | `ecs`, `core`, `transform`, `transforms` |
| Assets, sound, and capture | `assets`, `audio`, `media`, `microphone` |
| Files and processes | `fs`, `commands`, `command` |
| Networking | `http`, `servers` |
| Gameplay helpers | `async`, `prefabs`, `prefab`, `tweening`, `tween`, `animation`, `animations` |
| Rendering | `shaders`, `lighting`, `postprocess`, `postProcess`, `environment3d`, `environment3D`, `skybox` |
| 3D physics | `physics3d`, `physics3D` |
| Global helpers | `Color4`, `Inspector`, `die`, `softrequire`, `Rng` |
| Editor declaration names | `IComponentPicker`, `IEntity`, `IComponent`, `IImage`, `IAudio`, `IShader`, `IAnimation` |

NeoLOVE also installs project-relative `require`, and replaces `print` with a
tab-separated logger that writes to stdout and mirrors output to the visual
editor logger when the game is launched from the editor.

<!-- page: install | Installation -->
# Installation

## Requirements

- A current stable Rust toolchain for manual builds.
- Linux: ALSA development files, `pkg-config`, Clang/libclang, and Linux V4L2
  headers. These cover native audio plus microphone/camera capture.
- Vulkan is optional. The default desktop build uses the software renderer.

On Debian or Ubuntu:

```sh
sudo apt-get install pkg-config libasound2-dev clang libclang-dev linux-libc-dev
```

## Automated installers

::: tabs
== Linux and macOS
```sh
./install.sh
```

Set `NEOLOVE_VULKAN=1` to force Vulkan or `NEOLOVE_VULKAN=0` to force the
software renderer.

== Windows PowerShell
```powershell
.\install.ps1
```

Pass `-Vulkan On` or `-Vulkan Off` to override automatic Vulkan detection. The
installer also installs the Visual Studio 2022 Desktop C++ workload.
:::

The installers provision Git, Rust, native dependencies, and NeoLOVE in a
user-local application-data directory. They are safe to re-run. Existing clean
installations are updated, interrupted staging directories are cleaned up, and
a recognizable incomplete checkout is preserved as a timestamped backup before
a new clone replaces it.

## Manual installation

```sh
git clone https://github.com/NeoloveEngine/NeoLOVE.git
cd NeoLOVE
cargo install --path .
neolove --version
```

Use `cargo install --path . --features vulkan` to compile the Vulkan presenter
and enable custom desktop shaders. The software renderer remains available as a
fallback if Vulkan initialization fails.

## Release profiles

```sh
cargo build --release
cargo build --release --features vulkan
```

Release builds use size optimization, fat LTO, one codegen unit, stripped
symbols, and unwind-enabled panic handling so runtime failures can become
actionable dialogs instead of unexplained process aborts.

<!-- page: quick-start | Quick Start -->
# Quick Start

Create, enter, and run a project:

```sh
neolove new my-game
cd my-game
neolove run
```

Replace `main.luau` with:

```luau
app.bg = Color4(24, 26, 32)

local box = ecs.newEntity("box", ecs.root, 100, 100)
box.size_x = 160
box.size_y = 90

local rectangle = box:AddComponent(core.Rect2D)
rectangle.color = Color4(80, 140, 255)
```

The runtime loads `main.luau` once, then enters the frame loop. `run` and every
build command require `main.luau` at the project root.

## Create the same scene visually

```sh
neolove editor
```

Use **Add Entity**, add a `Rect2D` component in the Inspector, save the scene,
then choose **Run**. The editor writes a small `main.luau` which calls
`ecs.loadScene` for the configured start scene.

::: tip
Run `neolove api` after upgrading NeoLOVE to refresh
`types/neolove_engine_api.d.luau` in an existing project.
:::

<!-- page: project-layout | Project Layout -->
# Project Layout

A project is a directory containing `main.luau`. A typical generated project is:

```text
my-game/
├── .luaurc
├── .vscode/
│   └── settings.json
├── assets/
├── main.luau
├── neolove.toml
└── types/
    └── neolove_engine_api.d.luau
```

Common authored files include:

| Path or extension | Purpose |
| --- | --- |
| `main.luau` | Required runtime entry point. |
| `neolove.toml` | Project, package, and window settings. |
| `*.luau`, `*.lua` | Required modules or attachable behaviour components. |
| `*.neoscene` | Visual-editor scene document. |
| `*.neoprefab` | Visual-editor prefab subtree. |
| `*.neoanim` | Numeric keyframe animation clip. |
| `assets/` | Images, sounds, fonts, shaders, and other project resources. |
| `types/neolove_engine_api.d.luau` | Generated Luau declarations for tooling. |
| `dist/` | Generated build output. Do not author source here. |

## Resource and data roots

NeoLOVE distinguishes two roots:

- The **resource root** is the project directory or an extracted packaged
  payload. `main.luau`, modules, scenes, and bundled assets are read here.
- The **data root** is the default writable location. During development it is
  the project directory. A packaged desktop game uses
  `<executable-name>_data` beside the executable.

Relative reads check data first and bundled resources second. Relative writes
go to data. This permits packaged defaults to be overridden by saved files.
Absolute paths are used directly. Normalized parent-relative paths may leave
the data or project directory where the operating system permits it.

Commands are the deliberate exception: their `cwd` must remain inside the
project root.

<!-- page: project-config | Project Configuration -->
# Project Configuration

`neolove.toml` is intentionally a small TOML-like settings file. Strings must
be double quoted. `#` starts a comment. Unknown sections and keys are ignored.

```toml
[package]
name = "com.example.my_game"

[project]
start_scene = "scenes/title.neoscene"

[window]
title = "My Game"
icon = "assets/icon.png"
width = 1280
height = 720
fullscreen = false
resizable = true
```

## Complete settings reference

| Key | Type | Default | Effect |
| --- | --- | --- | --- |
| `package.name` | quoted string | project folder name | Package/display fallback, desktop output stem, Android application-id candidate, and iOS bundle-id candidate. Invalid platform identifiers are sanitized. |
| `project.start_scene` | quoted project-relative path | `scene.neoscene` | Scene exported and run by the visual editor. It must stay inside the project and end in `.neoscene`. |
| `window.title` | quoted string | package name, then folder name | Native game-window title. |
| `window.icon` | quoted project-relative path | none | Image decoded and resized to a 64×64 native window icon. Invalid images are ignored. |
| `window.width` | finite number | `1280` | Logical starting width, clamped to `1..16384`. |
| `window.height` | finite number | `720` | Logical starting height, clamped to `1..16384`. |
| `window.fullscreen` | boolean-like | `false` | Starts in borderless fullscreen. |
| `window.resizable` | boolean-like | `true` | Allows desktop window resizing. |

Boolean-like values are case-insensitive `true`, `false`, `yes`, `no`, `on`,
`off`, `1`, or `0`.

The mobile emulator overrides window width, height, fullscreen, and resizable
for that run. The visual editor uses the configured size for its game-window
bounds overlay.

<!-- page: cli | CLI Reference -->
# CLI Reference

```text
neolove
neolove hub
neolove new <project-name>
neolove run [project-dir] [run options]
neolove validate-3d [project-dir] --baseline <png> [validation options]
neolove editor [project-dir]
neolove build [project-dir] [target option]
neolove api [project-dir]
neolove update
neolove setup-path
neolove setup-start-menu
neolove --help
neolove --version
```

With no arguments, NeoLOVE opens the Hub when a graphical desktop is available;
otherwise it prints usage. Relative project paths resolve from the current
working directory.

On every non-packaged CLI startup, NeoLOVE also makes best-effort checks that
its PATH and application-launcher entries exist. Failures are warnings unless
the corresponding explicit setup command was requested.

## Commands

| Command | Arguments | Behavior |
| --- | --- | --- |
| `hub` | none | Opens the graphical project launcher. |
| `new` | exactly one project name | Creates the project, template settings, API declarations, and starter files. |
| `run` | zero or one project path, then run options in any order | Validates `main.luau` and starts the game. |
| `validate-3d` | zero or one project path, required baseline path, then validation options | Runs the isolated real 3D runtime headlessly, captures one lossless frame, writes a structured report, and exits nonzero on runtime or visual failure. |
| `editor` | zero or one project path | Opens a directory in the visual editor; `main.luau` is not required. |
| `build` | zero or one project path plus at most one target | Validates `main.luau` and writes a package under `dist/`. |
| `api` | zero or one project path | Rewrites `types/neolove_engine_api.d.luau`; also rewrites a root copy if one already exists. |
| `update` | none | Fast-forwards the tracked engine branch, rebuilds the same feature set, and replaces the executable. The source checkout must be clean. |
| `setup-path` | none | Adds NeoLOVE to the user PATH if needed. |
| `setup-start-menu` | none | Refreshes the per-user application-launcher entry. |
| `--help`, `-h`, `help` | none | Prints usage. |
| `--version`, `-V`, `version` | none | Prints the package version. |

Unknown commands print usage and fail. Extra positional arguments and unknown
options fail with a command-specific message.

## Complete `run` option reference

| Option | Alias | Effect |
| --- | --- | --- |
| `--mobile` | `--emulate-mobile` | Enables the desktop mobile emulator. |
| `--portrait` | none | Enables emulation and uses portrait orientation. |
| `--landscape` | none | Enables emulation and swaps the logical device axes. |
| `--wifi` | none | Enables Wi-Fi and disables cellular. |
| `--cellular` | none | Enables cellular and disables Wi-Fi. |
| `--offline` | `--no-wifi` | Disables both network flags. |
| `--low-power` | none | Enables emulated low-power mode. |
| `--no-low-power` | none | Disables emulated low-power mode. |
| `--mobile-size=WIDTHxHEIGHT` | none | Sets positive base device dimensions; default is `390x844`. |

All mobile-state options except `--no-low-power` also enable emulation. The
emulated window is non-resizable and not fullscreen. Keyboard events are
suppressed, although the mouse remains available for touch-style testing.

## Complete `validate-3d` option reference

| Option | Default | Effect |
| --- | --- | --- |
| `--baseline PATH` | required | PNG to create or compare. `--baseline=PATH` is also accepted. |
| `--backend auto\|software\|vulkan` | `auto` | Selects native Vulkan with fallback, forces software, or forces Vulkan. Forced Vulkan fails actionably when unavailable. |
| `--width N` | `960` | Capture width in `64..8192`. |
| `--height N` | `540` | Capture height in `64..8192`. |
| `--write-baseline` | off | Writes the captured PNG and its `*-baseline.json` backend/dimension sidecar instead of comparing. `--set-baseline` is an alias. |
| `--report PATH` | beside baseline | JSON report path. The default suffix is `-latest-report.json`. |
| `--diff PATH` | beside baseline | Highlighted failure-diff path. The default suffix is `-latest-diff.png`. |
| `--timeout-ms N` | `30000` | Bounded frame-capture timeout in `100..300000` milliseconds. |

The command uses the same isolated runtime, native Vulkan readback/software
fallback, and tolerance profiles as editor Game View. It fails for a child
runtime error, startup/timeout failure, dimension mismatch, changed-pixel
threshold breach, or mean-RGB-error breach. It always writes JSON metrics after
a completed comparison and writes the highlighted PNG on failure.

```sh
# Create the canonical software reference once.
neolove validate-3d . --baseline test-artifacts/pbr.png \
  --backend software --width 320 --height 180 --write-baseline

# CI jobs can validate each supported presenter. A failing comparison exits 1.
neolove validate-3d . --baseline test-artifacts/pbr.png \
  --backend software --width 320 --height 180
neolove validate-3d . --baseline test-artifacts/pbr.png \
  --backend vulkan --width 320 --height 180
```

### Mobile environment variables

The run options write these variables for the process. They may also be set
directly when launching a runtime:

| Variable | Default | Parsing |
| --- | --- | --- |
| `NEOLOVE_MOBILE_EMULATOR` | false | `1`, `true`, `yes`, or `on` enable. |
| `NEOLOVE_MOBILE_WIDTH` | `390` | Positive integer, runtime-clamped `120..4096`. |
| `NEOLOVE_MOBILE_HEIGHT` | `844` | Positive integer, runtime-clamped `120..4096`. |
| `NEOLOVE_MOBILE_ORIENTATION` | `portrait` | `landscape`, `wide`, or `horizontal` select landscape; all else is portrait. |
| `NEOLOVE_MOBILE_WIFI` | true | Same true-value parsing as enabled. |
| `NEOLOVE_MOBILE_CELLULAR` | false | Same true-value parsing as enabled. |
| `NEOLOVE_MOBILE_LOW_POWER` | false | Same true-value parsing as enabled. |

## Complete `build` target reference

| Option | Aliases | Target |
| --- | --- | --- |
| no option | none | Host desktop executable. |
| `--windows` | `--win`, `--exe` | Windows x86-64 GNU executable. |
| `--linux` | none | Linux x86-64 GNU executable. |
| `--webasm` | none | Emscripten HTML5 bundle and upload zip. |
| `--android` | `--apk` | Signed Android arm64 APK. |
| `--ios` | none | iOS Simulator `.app`. |

Only one target option may be selected.

Build discovery honors `JAVA_HOME`, then PATH for JDK 17+; `ANDROID_HOME` or
`ANDROID_SDK_ROOT` for an existing Android SDK; and Cargo's
`CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER` or
`CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER` cross-linker variables. The
install scripts honor `NEOLOVE_VULKAN` as documented under Installation.

<!-- page: builds | Packaging and Builds -->
# Packaging and Builds

## Desktop

Desktop export first builds a compact packaged runtime and appends a compressed
copy of the project. The complete native image is then Deflate-compressed inside
a small launcher. On first launch it is atomically cached in the operating
system's user cache directory; later launches reuse the cache. Project resources
use a separate temporary resource directory and writes still use `<game>_data`
beside the distributed executable. A project marked `kind = "2d"` omits 3D
component and material registration from its specialized runtime; change the
kind to `3d` before building a project that consumes those APIs.

The output is `dist/<project-output-name>` on Unix-like targets and
`dist/<project-output-name>.exe` on Windows. Exported Windows executables use
the GUI subsystem and do not open a separate console; fatal errors are shown in
a native dialog.

Cross-building Windows from Linux requires the
`x86_64-pc-windows-gnu` Rust target and MinGW-w64. The MinGW C/C++ runtimes are
linked statically. Cross-building Linux from Windows requires a Linux GNU cross
linker.

## WebAssembly

`neolove build --webasm` writes:

- the browser bundle under `dist/webasm/`; and
- `dist/<project-name>-webasm.zip`, ready for itch.io upload.

Serve the bundle through `http://` or `https://`; browsers do not reliably load
the Emscripten files from `file://`. Browser CORS, autoplay, filesystem, and
process restrictions apply.

## Android

`neolove build --android` writes
`dist/<project-name>-android-arm64.apk`. The APK targets API 35, uses minimum
SDK 24, and contains the arm64-v8a runtime plus the compressed project payload.
The first build may install a local JDK, Android SDK/build-tools 35.0.0, NDK
27.2.12479018, and the `aarch64-linux-android` Rust target beneath
`~/.neolove/toolchains/`.

## iOS Simulator

`neolove build --ios` writes
`dist/<project-name>-ios-simulator.app`. It is available only on macOS with
Xcode and targets the simulator, not a signed physical device or App Store
archive.

## Embedded asset formats

| Kind | Native support | Web notes |
| --- | --- | --- |
| Images | PNG, JPEG, GIF, BMP, TGA, TIFF, PNM, WebP, HDR, DDS | Decoded by the engine image path. |
| Audio | WAV, MP3, OGG/Vorbis, FLAC | Browser decoding additionally permits common AAC/M4A and AIFF files. |
| Fonts | TTF and OTF project files, plus built-in Open Sans | Web fonts are loaded through the browser `FontFace` path. |
| Shaders | GLSL vertex/fragment source | Desktop custom shaders require Vulkan; web uses the WebGL path. |

<!-- page: editor | Visual Editor -->
# Visual Editor

Launch the editor with `neolove editor [project-dir]`. It loads the configured
start scene when present and creates a starter scene otherwise. Unlike `run`,
opening the editor does not require `main.luau`.

The main window contains document tabs, a Hierarchy, a 2D Viewport, an
Inspector, and a bottom Project browser. Hierarchy, Inspector, and Project can
be hidden, docked, resized, or detached into native windows.

## Scene workflow

- **New Scene** opens an independent scene tab.
- **Save** writes the current `.neoscene` document.
- **Reload Scene** discards the in-memory version and reads the scene again.
- **Export** saves the configured start scene and writes a `main.luau` that
  calls `ecs.loadScene(...)`.
- **Run** exports and launches a preview. Runtime stdout is mirrored into the
  editor logger; a failed preview produces a copyable error dialog.
- **Mobile** configures portrait/landscape, Wi-Fi, cellular, and low-power
  emulation for previews.
- **Build** exports, asks for a platform, and packages without blocking the
  editor UI.

## Editing entities

Hierarchy rows form a parent/child tree. Drag to reparent, drag a row to the
Project panel to save a prefab, and drag `.neoprefab`, `.luau`, or `.lua` files
back into the Viewport or Hierarchy to instantiate or attach them.

The per-entity active checkbox controls scene export. An inactive entity and
all descendants are omitted from runtime output. The eye and lock controls are
editor-only state: they do not disable or remove an entity from the exported
game.

The Inspector edits name, position, z, size, rotation, scale, anchors, pivots,
core component properties, and behaviour-script variables. Selecting a
`Collider2D` draws its effective collision shape. Image, nine-slice, tiled,
tilemap, sprite-sheet, and particle components use real asset previews.

## Attached entity values

The Inspector's **Attached Values** section authors fields directly on the
runtime entity, without requiring a behaviour component. Click **Add Value**,
name the field, choose its type, and edit it in place. This is the visual-editor
equivalent of writing `entity.foo = "bar"` in Luau.

Available value types are numbers, strings, booleans, colors, entity
references, component references, images, sounds, shaders, animations, lists,
and tables. Lists and table values can contain any of those types recursively;
table keys can be strings, numbers, or booleans. Asset values use the same
searchable project picker as component fields. Assign entity references by
dragging a Hierarchy row, and component references by dragging a component
header. Deleting the referenced entity/component clears matching references,
including references nested inside lists and tables.

For example, an editor-authored entity with `health`, `portrait`, `target`, and
`stats` values is available directly to gameplay code:

```luau
print(enemy.health)
enemy.stats.hits += 1
enemy.target = nil
sprite.image = enemy.portrait
```

Scene export uses bracket assignment, such as
`enemy["display name"] = "Knight"`, so spaces and punctuation in names are
preserved. Empty names are retained in the editor but skipped at runtime.
Duplicate names are assigned in order, so the final value wins. Because these
are real entity-table fields, avoid engine-managed names such as `id`, `parent`,
`children`, and `components`, and avoid replacing transform fields or methods
unless that override is deliberate. Internal entity/component references are
remapped when a prefab is instantiated or an entity subtree is duplicated.

## Viewport tools

- Move, scale, rotate, and combined transform gizmos.
- Grid display and snapping.
- Mouse-wheel zoom and panning.
- Frame selected, frame all, reset view, and 100% zoom.
- Multi-selection, grouping, visibility isolation, locking, alignment, z-order,
  size normalization, and window-fit operations.
- Tile painting while a `Tilemap2D` is selected.
- In 3D scenes, Alt+LMB orbits around the active selection (or the last framed
  target), RMB drag looks around, WASD/QE flies, Shift boosts, and MMB pans. A
  short RMB click opens the selected-entity or empty-viewport context menu;
  crossing the drag threshold suppresses the menu. Mouse look and orbit are
  display-scale independent.
- The 3D Move gizmo has X/Y/Z axis handles, selectable XY/XZ/YZ plane handles,
  and a camera-facing free center drag. Its Local/World toolbar control changes
  the movement basis explicitly. Multi-selected objects convert the shared
  world displacement through their own parent transforms, so nested negative
  and non-uniform scales remain stable. Scale has X/Y/Z handles and a uniform
  center handle; Rotate has X/Y/Z rings that edit the corresponding Euler
  angles. The grid-snapping toggle applies to both dimensions, while the
  adjacent numeric field stores an independent 3D world-unit increment instead
  of reusing the 2D pixel grid. Ctrl-drag a selected move handle or selected
  object to duplicate its subtree and place the copy; duplication and movement
  undo together as one command.
- **Tools → Scene View** exposes Surface/Pivot Snap, Vertex Snap, and Align to
  Surface Normal for 3D scenes. Surface placement uses the visible transformed
  mesh triangle under the cursor with perspective-correct world interpolation;
  Vertex Snap refines it to a nearby visible vertex. Transformed box, sphere,
  capsule, and mesh colliders are collision-aware placement surfaces even
  without a mesh renderer. Collider surface preparation shares the viewport's
  bounded triangle budget.
  Locked meshes remain valid placement targets without becoming selectable.
  Multi-selection keeps its world offsets while the active pivot lands on the
  target, and optional normal alignment rotates each object's local +Y through
  its parent hierarchy.
- **Tools → Scene View** also switches perspective/orthographic projection,
  selects Top/Front/Right orthographic views, and stores or recalls four camera
  bookmarks. The viewport orientation widget exposes the same axis views and a
  clickable PERSP/ORTHO badge. Orthographic wheel zoom changes the persisted
  world-space half-height rather than dollying the camera.
- The same menu independently toggles editor-only wireframe, surface-normal,
  tangent, UV-seam, transformed mesh-bound, entity-pivot, world-origin/axis,
  collider-shape, rigid-body, trigger, authored-raycast, particle-bound,
  camera-frustum, light-range, spot-cone, runtime-shadow-frustum, and viewport-
  statistics overlays. Shadow frustums reuse the native renderer's exact
  camera-fitted directional and spot projection calculation.
  Normal and tangent drawing are each bounded to 2,048 sampled surfaces per
  frame; UV seam matching and output are independently bounded. The statistics strip reports
  Scene View CPU time, mesh draws, projected triangles, lights, and prepared
  snap surfaces; it does not claim unavailable runtime GPU or residency data.
- Dragging from empty 3D viewport space performs a visible marquee selection
  over projected mesh bounds and component proxies. Ctrl or Shift makes the
  marquee additive; modifier-click toggles an entity. Hidden and locked objects
  remain excluded, and meshes with many projected triangles are deduplicated.
- The adaptive 3D ground grid follows the camera with bounded fine/coarse line
  budgets, producing an effectively infinite view without unbounded draw work.
  Its lines are clipped at the camera near plane before projection.
- Camera sensitivity, movement speed, FOV, and inverted vertical mouse look
  are configurable under global Editor Settings.

The current lightweight 3D Scene view is an authoring aid, not the final visual
authority for textures, custom shaders, shadows, or post-processing that it does
not draw. For a 3D document, **Run** writes the current unsaved scene to an
isolated project-local preview cache, starts the real runtime in a hidden child
process, and switches the viewport to **Game View**. Vulkan builds use native
HDR/tonemap rendering plus GPU readback when available and otherwise use the
same real software renderer as a normal software run. The authored
scene and entry point are not overwritten. Game View receives lossless PNG
frames, forwards focused mouse/keyboard/text input, and reports runtime update
and render time. Its toolbar controls pause/resume, one deterministic 1/60-second
update, restart, and stop. When an entity with `Camera3D` is selected, the camera
button beside Run starts the staged scene with only that camera enabled.

Stopping discards the isolated runtime and removes its staged files; runtime
mutations are never copied into the authored scene. Live hierarchy, component
snapshots, and logs still travel over the same localhost session. The live pane
uses explicit authored source ids (not runtime allocation order), links structured
diagnostics to entities/components/scripts, and can validate the current document
against the runtime's immutable post-load/pre-update state. **Set Base** stores a
canonical Game View PNG; **Compare** applies explicit pixel tolerances and writes
JSON metrics plus a highlighted diff on failure. A JSON sidecar records the
baseline backend: same-backend comparisons allow at most 1% pixels over an
8-channel delta, while cross-backend comparisons allow 3% for measured
MSAA/software edge-coverage differences; both retain the 1.5 mean-RGB-error
limit. Set
`NEOLOVE_EDITOR_EMBEDDED_BACKEND=vulkan` or `software` to force a validation
backend; the default is Vulkan with software fallback. The validator exposed
and drove a native front-face winding repair; the representative corrected PBR
frame now measures 0.991 mean RGB error between software and Vulkan. Scene View
capture, complete profiler streams, and a broader representative-scene matrix
remain incomplete. Linux CI runs the repository PBR fixture through software
and Mesa/Lavapipe Vulkan with the headless `validate-3d` gate. Non-3D projects
retain their established Run behavior.

Holding `Ctrl` while moving a parent preserves descendant world positions.
Holding `Ctrl` during corner resize preserves aspect ratio. Arrow-key nudging
uses one pixel, or the grid step while `Shift` is held.

## Keyboard shortcuts

`Ctrl` means Command on macOS.

| Shortcut | Action |
| --- | --- |
| `Ctrl+S` | Save scene. |
| `Ctrl+Z` | Undo. |
| `Ctrl+Y` or `Ctrl+Shift+Z` | Redo. |
| `Ctrl+D` | Duplicate selection. |
| `Ctrl+C` / `Ctrl+V` | Copy / paste the selected entity or active text selection. |
| `Ctrl+A` | Select all. |
| `Ctrl+Shift+A` | Invert selection. |
| `Ctrl+G` | Group selection. |
| `Ctrl+Shift+G` | Unparent selection. |
| `H` / `Shift+H` | Hide selection / show all. |
| `L` / `Shift+L` | Lock selection / unlock all. |
| `G` | Toggle grid. |
| `Shift+S` | Toggle snapping. |
| `Shift+Space` | Maximize or restore the Viewport. |
| `F` | Frame selected. |
| `Home` | Frame all visible entities. |
| `0` | Reset view. |
| `F2` | Rename. |
| Arrow keys | Nudge selected entities; `Shift` uses the grid step. |
| `Delete` | Delete the active selection when no text field owns the key. |

## Hierarchy and component workflow

The Hierarchy search filters by entity name without changing the scene. Branches
may be folded individually or collapsed/expanded in bulk. Right-click actions
include add child, duplicate, copy, paste, unparent, reset transform, frame,
rename, activate/deactivate, and delete. The three-dot menus add selection,
hierarchy, arrangement, visibility, locking, alignment, z-order, and scene-view
operations.

The component picker focuses its search field when opened; typing filters and
Enter adds the top result. Core components are split into common and advanced
groups, while correctly guarded `IComponentPicker` scripts appear alongside
them. A component header can be copied and the picker can paste that component
onto another entity. Removing and re-adding changes component execution/draw
order.

Entity reference fields accept a dragged Hierarchy row. For a component
reference, drag a component header, hover the destination entity to inspect it,
then drop on the destination field without releasing the pointer.

For a `Dropdown`, expand **Options** in the Inspector to add, rename, move up,
move down, or delete entries. The editor preserves order, duplicates, empty
strings, and escaped characters when saving and exporting the scene. Newly
added rows receive keyboard focus. The visual editor authors string options;
use Luau when an option needs the runtime's richer table form with a separate
value or icon.

Color properties expose inline R, G, B, and A fields; alpha `0` is transparent
and `255` opaque. A swatch opens either an HSV square/hue strip or RGBA sliders,
both with alpha. The chosen mode is saved globally. Interactive widget state
colors are under **Advanced**.

Particle color/transparency sequence strips open a keypoint editor. The
viewport uses a deterministic representative particle preview so editing the
scene does not mutate saved runtime particle state.

## Project browser and asset editors

The Project browser can create folders, Luau component templates, fragment
shader templates, and animation clips. It opens project files through the OS,
reveals folders in the file manager, refreshes the tree, and can open the
project in VS Code.

- Drag a Hierarchy entity to the Project browser to save its subtree as
  `.neoprefab`.
- Drag a prefab into the Viewport to create fresh ids and a source link at the
  drop position.
- Double-click `.neoscene` or `.neoprefab` to open a document tab. A prefab tab
  edits only the prefab. Saving it refreshes linked instances in open and
  on-disk scenes while retaining each instance root's placement.
- Double-click `.neoanim` to open the Bezier animation editor.
- In a 3D project, **New 3D Material** and **New Physics Material** create the
  runtime's versioned `.neomaterial` and `.neophysicsmaterial` assets.
  Double-click either format to edit it with validation and a visible dirty
  state. Collider physics-material fields use a typed searchable picker.
- A selected `Tilemap2D` enters Paint mode from the Inspector; dragging writes
  the selected tile id, and `-1` erases.
- Image, font, sound, shader, and animation Inspector fields use searchable
  project asset pickers. Sound selection includes a downsampled waveform.

Most controls have tooltips and contextual right-click menus. Unsaved changes
prompt before destructive New, Load, document close, or quit operations.

## Hub

`neolove hub` creates projects, opens a selected project directory, and keeps up
to 12 recent projects. Launching the desktop NeoLOVE executable with no command
also opens the Hub and refreshes the per-user application-launcher entry when a
graphical session exists.

<!-- page: editor-files | Editor Files and Settings -->
# Editor Files and Settings

## Scene, prefab, and animation documents

New editor saves use compressed MessagePack documents with format headers:

- `.neoscene`: `NEOLSCN1` header;
- `.neoprefab`: `NEOLPFB1` header.

The loaders retain JSON compatibility for older or hand-authored files. A scene
stores `name`, RGBA `background`, `nearest_neighbor_scaling`, `antialiasing`,
and `entities`. Each entity stores its id, name, optional linked prefab source,
transform fields, optional parent id, active state, attached values, and
components.

`.neoanim` is JSON and uses the same `AnimationClip` shape documented in the
animation API.

At runtime, scene loading generates Luau from the document. It omits inactive
entities and every descendant of an inactive entity, creates parents before
children, requires each unique script component module once, and reuses each
unique image path. Core properties and Inspector values are assigned to new
component instances, and Attached Values are assigned directly to their entity
tables; entity/component references are resolved after their targets exist.
Scene background, image filtering, and anti-aliasing are written to `app`.

::: warning
Scene and prefab serialization is an editor interchange format, not a stable
gameplay API. Prefer editor operations, `ecs.loadScene`, and `prefabs.load`
over editing binary files directly.
:::

## Global editor configuration

`editor.json` is stored in the operating-system user config directory:

::: tabs
== Windows
`%APPDATA%\NeoLOVE\editor.json`

== macOS
`~/Library/Application Support/NeoLOVE/editor.json`

== Linux
`$XDG_CONFIG_HOME/neolove/editor.json`, or `~/.config/neolove/editor.json`
:::

An older project-local `editor.json` is read as a fallback. The global file has
four top-level objects: `theme`, `custom_theme`, `layout`, and `settings`
(`theme` and `custom_theme` have the same schema).

`recent_projects.json` is stored beside the global config and contains the
Hub's at-most-12 recent project records. It is managed by the Hub/editor rather
than intended for manual editing.

### `settings` fields

| Field | Type | Default |
| --- | --- | --- |
| `theme_name` | string preset id | `dark_plus` |
| `font_path` | string | empty, meaning built-in font |
| `show_tooltips` | boolean | `true` |
| `show_window_bounds` | boolean | `true` |
| `show_transform_hud` | boolean | `true` |
| `preview_lighting` | boolean | `true` |
| `autosave_before_run` | boolean | `true` |
| `autosave_before_build` | boolean | `true` |
| `mobile_emulator` | boolean | `false` |
| `mobile_orientation` | `portrait` or `landscape` | `portrait` |
| `mobile_wifi` | boolean | `true` |
| `mobile_cellular` | boolean | `false` |
| `mobile_low_power` | boolean | `false` |
| `viewport_camera_sensitivity` | number (`0.05..8`) | `1` |
| `viewport_camera_speed` | number (`0.1..1000`) | `10` |
| `viewport_camera_fov` | degrees (`20..140`) | `60` |
| `viewport_invert_mouse_look` | boolean | `false` |

Named `theme_name` values are `dark_plus`, `gruvbox_dark`, `dracula`,
`monokai`, `solarized_dark`, `light_plus`, and `custom`. Changing presets does
not overwrite `custom_theme`; choosing `custom` copies it into active `theme`.

### `layout` fields

| Field | Type | Default |
| --- | --- | --- |
| `left_w`, `right_w` | number | `240`, `330` |
| `hierarchy_side`, `inspector_side` | `Left` or `Right` | `Left`, `Right` |
| `left_split`, `right_split` | number | `0.5`, `0.5` |
| `snap` | boolean | `true` |
| `grid` | positive number | `32` |
| `show_grid` | boolean | `true` |
| `bin_h` | number | `170` |
| `hsv_picker` | boolean | `true` |
| `show_project`, `show_hierarchy`, `show_inspector` | boolean | `true` |
| `undock_hierarchy`, `undock_inspector`, `undock_project` | boolean | `false` |
| `view_tool` | `Move`, `Scale`, `Rotate`, or `Transform` | `Move` |

### Theme fields

RGBA values are arrays `[r, g, b, a]`. Theme objects contain `panel`,
`panel_alt`, `toolbar`, `viewport_bg`, `border`, `text`, `text_dim`, `button`,
`button_hover`, `button_active`, `field`, `field_focus`, `accent`, `selection`,
`danger`, `splitter`, `splitter_hover`, `header`, and `grid`, plus numeric
`corner_radius`. Missing fields receive defaults.

Dark+ defaults are:

| Field | Default |
| --- | --- |
| `panel` | `[37,37,38,255]` |
| `panel_alt` | `[45,45,45,255]` |
| `toolbar` | `[60,60,60,255]` |
| `viewport_bg` | `[30,30,30,255]` |
| `border` | `[69,69,69,255]` |
| `text` | `[212,212,212,255]` |
| `text_dim` | `[133,133,133,255]` |
| `button` | `[14,99,156,255]` |
| `button_hover` | `[17,119,187,255]` |
| `button_active` | `[9,71,113,255]` |
| `field` | `[60,60,60,255]` |
| `field_focus` | `[45,45,45,255]` |
| `accent` | `[0,122,204,255]` |
| `selection` | `[255,199,89,255]` |
| `danger` | `[241,76,76,255]` |
| `splitter` | `[51,51,51,255]` |
| `splitter_hover` | `[0,122,204,255]` |
| `header` | `[51,51,51,255]` |
| `grid` | `[255,255,255,16]` |
| `corner_radius` | `4` |

<!-- page: inspector | Inspector and Scripts -->
# Inspector and Behaviour Scripts

Any `.luau` or `.lua` module may be attached as a custom component. The module
returns an ordinary component prototype. Public variables wrapped in
`Inspector(...)` become serializable controls:

```luau
local Behaviour = {
    speed = Inspector(100),
    lives = Inspector(1, 10),
    opacity = Inspector(0, 1, true),
    tint = Inspector(Color4(255, 120, 80)),
    inventory = Inspector({ "sword", "key" }),
    stats = Inspector({ health = 100, mana = 40 }),
    target = Inspector(IEntity),
    renderer = Inspector(IComponent),
    sprite = Inspector(IImage),
    sound = Inspector(IAudio),
    material = Inspector(IShader),
    clip = Inspector(IAnimation),
}

function Behaviour.awake(entity, self)
    print(self.speed)
end

function Behaviour.update(entity, self, dt)
end

return Behaviour
```

## `Inspector<T>(defaultValue, max?, fractional?) -> T`

At runtime, `Inspector` returns its first argument unchanged. In the editor:

- one number creates a numeric field;
- two numbers create a slider whose first value is both the initial value and
  one endpoint;
- `fractional = true`, or fractional bounds, allows fractional slider values;
- strings, booleans, and `Color4` create their corresponding controls;
- consecutive integer-key tables starting at 1 are editable lists;
- other tables are editable dictionaries;
- lists and dictionaries may nest any inspector-supported value;
- concrete runtime entities/components and the sentinel globals create drag
  targets for scene references;
- image, sound, shader, and animation placeholders create project asset fields.

When a script changes, the editor reparses its declaration in a bounded Luau
sandbox and retains compatible values by variable name.

## Reference placeholders

| Global | Inspector field | Exported runtime value |
| --- | --- | --- |
| `IEntity` | scene entity | entity table or `nil` |
| `IComponent` | scene component | component instance or `nil` |
| `IImage` | image asset | `assets.loadImage(path)` or `nil` |
| `IAudio` | sound asset | `assets.loadSound(path)` or `nil` |
| `IShader` | fragment shader | `shaders.loadFragment(path)` or `nil` |
| `IAnimation` | animation clip | `animation.load(path)` or `nil` |

These names are editor declaration sentinels. Do not use their placeholder
values as gameplay objects.

## Custom component picker

```luau
-- The editor parser defines IComponentPicker. The game runtime currently does not.
if IComponentPicker then
    IComponentPicker(Behaviour)
end
```

Calling `IComponentPicker` while the editor parses the module registers the
behaviour in **Add Component** search results. The current game runtime does not
install this function, despite its generated declaration, so guard the call as
shown. The reference sentinels are likewise editor parser values; unassigned
runtime references should be expected to become `nil` through `Inspector`.

<!-- page: runtime-model | Runtime Model -->
# Runtime Model

## Startup

1. The Luau VM and optimized compiler are configured.
2. Project-relative `require`, forwarded `print`, platform tables, and all
   NeoLOVE globals are installed.
3. `ecs.root` is created at id `0` with the current logical window size.
4. `main.luau` executes.
5. Deferred custom-component `awake` callbacks run at the start of a frame.

`require` uses the project filesystem and normal Luau module caching. Paths in
all NeoLOVE loaders are project-relative unless their section specifies the
writable data root.

## Frame order

Each update performs:

1. reset per-frame UI popup state;
2. synchronize `mouse`, `window`, and `ecs.root` size;
3. deliver completed HTTP, media-device, and server work;
4. resume each unfinished `async` task once;
5. run pending custom-component `awake` callbacks;
6. advance tween and animation players;
7. dispatch entity listeners;
8. resolve `app.bg` and anti-aliasing;
9. run system `update` callbacks in registration order;
10. run newly queued component `awake` callbacks;
11. run non-rendering component `update` callbacks in entity/component order;
12. simulate rigidbodies, colliders, bolts, and ropes;
13. resolve enabled Camera components in a dedicated camera pre-pass;
14. run drawable rendering-component updates in stable draw order; and
15. translate the scene and lighting by the resolved camera before submitting
    commands to the selected presenter.

`NEOLOVE_RENDERING = true` delays a component's update until the render pass.
Draw order is ascending `z`, then ascending entity id, with component order
stable within an entity. Front-to-back queries reverse that order.

::: warning
`System.lateUpdate` and `System.fixedUpdate` exist in the generated declaration
shape for compatibility, but this runtime does not schedule them. Only
`awake(self)` and `update(self, dt)` are invoked.
:::

<!-- page: api-conventions | API Conventions -->
# API Conventions

The reference uses Luau types:

- `T?` means `T | nil`.
- `{ T }` is an array-like table.
- `{ [K]: V }` is a keyed table.
- `() -> (A, B)` returns multiple values.
- `...any` is a variable-length value pack.
- Method signatures include an explicit `self`; call them with `:` unless a
  section says dot calls are supported.

## Aliases

Aliases reference the same module or operation unless noted:

| Canonical | Alias |
| --- | --- |
| `input` | `userInput` |
| `commands` | `command` |
| `prefabs` | `prefab` |
| `tweening` | `tween` |
| `animation` | `animations` |
| `transform` | `transforms` |
| `postprocess` | `postProcess` |
| `physics3d` | `physics3D` |
| `environment3d` | `environment3D`, `skybox` |
| `microphone` | `media.microphone` |
| `microphone.listDevices` | `microphone.enumerateDevices` |
| `core.Panel` | `core.Frame` |
| `core.TextBox` | `core.TextLabel`, `core.RudimentaryTextLabel` |
| `core.NineSliceSprite2D` | `core["9SliceSprite2D"]` |
| `core.Rope2D` | `core.String2D` |

Many entity, connection, particle, animation-controller, spatial-audio, and
handle methods provide both `camelCase` and `PascalCase` spellings. Every alias
is shown in its type definition.

## Mutable tables

Entities, component instances, systems, app state, clips, prefab templates,
and most response records are ordinary Luau tables. Unless a field is described
as derived/read-only, assignment takes effect on the next relevant update.
Handle userdata (`ImageHandle`, `SoundHandle`, `ShaderHandle`) expose methods
but not mutable table fields.

## Errors

Invalid argument types raise Luau errors. Synchronous filesystem, shader,
asset, scene, prefab, and animation loads also raise on failure. APIs which
represent expected operational failure return `false`, `nil`, or a result table
with `ok = false` as documented.

In the function-reference tables, **Parameters** names every argument other
than an implicit method `self`. **Returns** lists every normal result; `()`
means the call has no return value. Unless an entry explicitly says otherwise,
passing a value of the wrong Luau type raises an argument error before the
operation starts. Canonical spellings head each entry and exact aliases are
listed in that same entry rather than documented as separate operations.

<!-- page: global-helpers | Globals and Helpers -->
# Globals and Helpers

## Foundational values

```luau
export type Color4Value = {
    r: number,
    g: number,
    b: number,
    a: number,
}

export type Vec2 = {
    x: number,
    y: number,
}
```

`mouse: Vec2` is the current logical cursor position. `window: Vec2` is the
current logical width (`x`) and height (`y`). The runtime updates the existing
tables so references remain valid. In WebAssembly the tables are read-only
proxies.

## `Color4`

```luau
Color4(r: number, g: number, b: number, a: number?) -> Color4Value
```

| Parameters | Returns | Behavior and edge cases |
| --- | --- | --- |
| `r`, `g`, `b`: numeric byte-channel values. `a`: optional alpha, default `255`. | A new `{ r, g, b, a }` table whose fields are integer bytes. | Each channel is clamped to `0..255` and converted to a byte. Values outside that range do not wrap. The returned table is mutable and is not shared with another color. |

```luau
local translucentOrange = Color4(300, 128, -20, 96)
assert(translucentOrange.r == 255 and translucentOrange.b == 0)
```

## `die`

```luau
die(reason: string?) -> ()
```

| Parameters | Returns | Behavior and edge cases |
| --- | --- | --- |
| `reason`: optional diagnostic string. | `()`; control does not continue through a normal game frame. | Requests a clean runtime exit. `nil`, `""`, or whitespace-only text becomes `"die() called"`. The reason is reported through the runtime's normal shutdown/error channel; this is not a recoverable `false` result. |

## `softrequire`

```luau
softrequire(
    modulePathOrSource: string,
    allowedModules: { [string]: any } | { string }?
) -> any
```

If the first argument resolves to an existing project module, its source runs
in a restricted environment and is cached by resolved path. Otherwise the
argument is compiled as inline Luau and cached by source hash. `allowedModules`
may be a list of permitted global names or a map of names to explicit values.
The sandbox has its own `_G`; unapproved globals are unavailable.

The base sandbox includes `assert`, `error`, `getmetatable`, `ipairs`, `next`,
`pairs`, `pcall`, `rawequal`, `rawget`, `rawlen`, `rawset`, `select`,
`setmetatable`, `tonumber`, `tostring`, `type`, `unpack`, `xpcall`, and the
`math`, `string`, `table`, and `utf8` libraries. A list entry copies that named
runtime global when it exists; a string-keyed map installs the supplied value.

Path inputs may omit `.luau`/`.lua`, and directories resolve to `init.luau`.
Resolved module files must remain inside the project. Inline-source compilation
is attempted only when no file resolves.
Cache keys do not include `allowedModules`; the first successful load for a
given resolved path or source determines the cached result for later calls.

**Parameters.** `modulePathOrSource` is either a project-contained module path
or Luau source text. `allowedModules`, when present, is either an array of
runtime-global names to copy or a string-keyed table whose values are installed
under those keys. A numeric list entry whose global does not exist is simply
not installed.

**Returns and failures.** The function returns every normal value produced by
the loaded chunk according to the loader's module semantics. Resolution,
syntax, runtime, project-boundary, and invalid-allowlist errors raise. A module
which legitimately returns `nil` therefore has the same visible result as any
other nil-returning module; failures never use `nil` as a sentinel.

```luau
-- File form: only expose the globals this utility actually needs.
local parser = softrequire("scripts/parser", { "Color4", "Rng" })

-- Source form: inject an explicit dependency without widening the sandbox.
local double = softrequire("return function(n) return dep(n) * 2 end", {
    dep = function(n) return n + 1 end,
})
assert(double(4) == 10)
```

## `print` and `require`

```luau
print(...any) -> ()
require(modulePath: string) -> any
```

`print` accepts any number of values, returns nothing, applies `tostring`, joins
arguments with tabs, writes one line to stdout, and forwards it to the editor
logger. With no arguments it writes a blank line. A failing custom `__tostring`
metamethod propagates as an error.

`require` accepts one project module path and returns the value exported by the
module. It is the mlua text-module loader rooted at the project. Paths may omit
the script suffix and may resolve a directory's `init` file; missing files,
syntax/runtime errors, and paths escaping the project raise. Successfully
loaded modules are cached, so later calls return the cached export rather than
running top-level code again.

```luau
local inventory = require("scripts/inventory")
print("slots", #inventory.slots)
```

## Legacy `bg`

`app.bg` is canonical. If that field is absent, the runtime accepts a global
`bg` color table for older projects.
That legacy path also accepts uppercase `R`, `G`, `B`, and `A` channels;
component colors require lowercase fields as produced by `Color4`.

<!-- page: app | Application API -->
# Application API

Global: `app`

```luau
export type AppModule = {
    bg: Color4Value,
    maxFps: number?,
    showFps: boolean,
    nearestNeighborScaling: boolean,
    antiAliasing: "off" | "standard" | "high",

    setMaxFps: (fps: number?) -> (),
    getMaxFps: () -> number?,
    setShowFps: (enabled: boolean?) -> (),
    getShowFps: () -> boolean,
    setNearestNeighborScaling: (enabled: boolean?) -> (),
    getNearestNeighborScaling: () -> boolean,
    setAntiAliasing: (mode: ("off" | "standard" | "high")?) -> (),
    getAntiAliasing: () -> "off" | "standard" | "high",
}
```

## Fields

| Field | Default | Meaning |
| --- | --- | --- |
| `bg` | `Color4(255,255,255,255)` | Window clear color, read every frame. |
| `maxFps` | `nil` | Positive finite desktop/web frame cap. `nil` is uncapped. Prefer the setter. |
| `showFps` | `true` | Whether the presenter draws its FPS indicator. Prefer the setter. |
| `nearestNeighborScaling` | `true` | `true` selects nearest-neighbor image filtering; `false` selects linear filtering. |
| `antiAliasing` | `high` | Global 2D geometry, 3D mesh/particle, custom-shader, and default text quality. The runtime also reads lowercase `antialiasing` as a fallback. |

## Function reference

| Function | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `setMaxFps(fps?)` | `fps`: optional numeric frames-per-second cap. | `()` | Stores a positive finite cap. `nil`, `0`, negative, NaN, or infinite input clears the cap. It affects frame pacing, not simulation `dt` clamping. |
| `getMaxFps()` | None. | Positive finite `number`, or `nil` when uncapped/invalid. | Reads and validates the current `app.maxFps`, including direct field assignments. |
| `setShowFps(enabled?)` | `enabled`: optional boolean; default `true`. | `()` | Controls the presenter's built-in FPS counter. |
| `getShowFps()` | None. | `boolean`. | Returns `app.showFps`; a missing field defaults to `true`. |
| `setNearestNeighborScaling(enabled?)` | `enabled`: optional boolean; default `true`. | `()` | `true` selects nearest-neighbor sampling and `false` selects linear sampling for images. Existing image handles need not be reloaded. |
| `getNearestNeighborScaling()` | None. | `boolean`. | Returns the current filtering preference; a missing field defaults to `true`. |
| `setAntiAliasing(mode?)` | `mode`: optional quality name. | `()` | Stores normalized `"off"`, `"standard"`, or `"high"`. `nil` and unknown strings become `"high"`; compatibility spellings are listed below. |
| `getAntiAliasing()` | None. | `"off"`, `"standard"`, or `"high"`. | Normalizes the current field on read, so a legacy or unknown directly assigned value still has a defined result. |

Replacing the global `app` table is supported: the functions look up the
current table when called. Desktop frame pacing also reads the current table.
The parser maps `none`, `disabled`, and `pixel` to `off`; `fast`, `normal`, and
`on` to `standard`; and every other string to `high`.

The setting is live: changing it rebuilds Vulkan multisample attachments when
needed, selects the software 3D edge pass, and selects the browser shader
surface on the next frame. Hardware sample availability can make a backend
fall back to a lower level; the detailed backend table is in
[Rendering Details](#anti-aliasing).

```luau
app.setMaxFps(120)
app.setShowFps(false)
app.setNearestNeighborScaling(true)
app.setAntiAliasing("standard")
print(app.getMaxFps(), app.getAntiAliasing())
```

<!-- page: input | Input API -->
# Input API

Globals: `input`, `userInput` (same table)

```luau
export type InputModule = {
    isKeyDown: (key: string) -> boolean,
    isKeyPressed: (key: string) -> boolean,
    isKeyReleased: (key: string) -> boolean,
    isMouseDown: (button: string?) -> boolean,
    isMousePressed: (button: string?) -> boolean,
    isMouseReleased: (button: string?) -> boolean,
    getMouseWheel: () -> (number, number),
    isScrollingIn: () -> boolean,
    isScrollingOut: () -> boolean,
    getScrollInAmount: () -> number,
    getMouseDelta: () -> (number, number),
    setMouseLocked: (locked: boolean) -> (),
    isMouseLocked: () -> boolean,
    getLastKeyPressed: () -> string?,
    getCharPressed: () -> string?,
    showKeyboard: (implicit: boolean?) -> boolean,
    openKeyboard: (implicit: boolean?) -> boolean,
    hideKeyboard: (implicitOnly: boolean?) -> boolean,
    closeKeyboard: (implicitOnly: boolean?) -> boolean,
}
```

## State-function reference

| Function | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `isKeyDown(key)` | `key`: normalized key name. | `boolean`. | `true` for every frame the mapped key remains held. Unknown names return `false`. |
| `isKeyPressed(key)` | `key`: normalized key name. | `boolean`. | `true` only in the frame in which the mapped key transitioned up-to-down. Repeats do not create another transition unless the key was released. Unknown names return `false`. |
| `isKeyReleased(key)` | `key`: normalized key name. | `boolean`. | `true` only in the down-to-up transition frame. Unknown names return `false`. |
| `isMouseDown(button?)` | `button`: optional normalized button, default `"left"`. | `boolean`. | Tests held state. Unknown buttons return `false`. |
| `isMousePressed(button?)` | `button`: optional normalized button, default `"left"`. | `boolean`. | Tests this frame's up-to-down transition. |
| `isMouseReleased(button?)` | `button`: optional normalized button, default `"left"`. | `boolean`. | Tests this frame's down-to-up transition. |
| `getMouseWheel()` | None. | Horizontal `x` delta, then vertical `y` delta. | Both are signed numbers accumulated for the current frame and reset for the next. A frame with no wheel input returns `0, 0`. |
| `isScrollingIn()` | None. | `boolean`. | Equivalent to testing whether the current vertical wheel delta is positive. |
| `isScrollingOut()` | None. | `boolean`. | Equivalent to testing whether the current vertical wheel delta is negative. |
| `getScrollInAmount()` | None. | Signed vertical wheel delta. | Historical name notwithstanding, scrolling out returns a negative number and no scrolling returns `0`. |
| `getMouseDelta()` | None. | `dx, dy` movement for this frame. | Values are logical-window deltas and reset each frame; locked mode may report relative motion without moving `mouse`. |
| `setMouseLocked(locked)` | `locked`: requested lock/grab state. | `()` | Requests the platform cursor mode. Browser policy may defer/reject pointer lock until a user gesture; the stored request can still be read back. |
| `isMouseLocked()` | None. | `boolean`. | Returns the requested state, not a guarantee that the OS/browser currently owns the pointer. |
| `getLastKeyPressed()` | None. | Normalized key `string`, or `nil`. | Returns the last mapped key transition observed in the current frame. Multiple presses keep only the last one. |
| `getCharPressed()` | None. | Text `string`, or `nil`. | Returns the last text-input character received in the current frame. It is text input, not a physical-key name; multiple characters keep only the last. |

Key and button strings are normalized by removing non-alphanumeric characters
and lowercasing, so `"Left Shift"`, `"left_shift"`, and `"leftshift"` match.

Supported cross-platform key names are `a` through `z`, `0` through `9`,
`space`, `escape`, `enter`, `tab`, `backspace`, `left`, `right`, `up`, `down`,
`leftshift`, `rightshift`, `leftcontrol`, `rightcontrol`, `leftalt`, `rightalt`,
`leftsuper`, `rightsuper`, and `f1` through `f12`. Mouse names are `left`,
`middle`, `right`, and on web `other`.

## On-screen keyboard

| Canonical function | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `showKeyboard(implicit?)` (`openKeyboard` alias) | `implicit`: optional boolean, default `true`, forwarded to Android's show mode. | `true` when an Android activity accepted the request; `false` when unsupported or no activity was available. | This is a request, so `true` does not guarantee a particular IME visibly opened. Desktop, web, and iOS builds return `false`. |
| `hideKeyboard(implicitOnly?)` (`closeKeyboard` alias) | `implicitOnly`: optional boolean, default `false`, forwarded to Android's hide mode. | `true` when an Android activity accepted the request; otherwise `false`. | When `true`, an explicitly opened keyboard may remain visible according to Android IME rules. |

```luau
function update()
    if input.isKeyPressed("escape") then
        input.setMouseLocked(not input.isMouseLocked())
    end
    local wheelX, wheelY = input.getMouseWheel()
    if wheelY ~= 0 then print("wheel", wheelX, wheelY) end
end
```

Pressed, released, wheel, character, and delta values are frame-local. Mobile
emulation suppresses hardware keyboard events.

<!-- page: async | Async Tasks -->
# Async Tasks

Global: `async`

`async` is a callable module for cooperative Luau coroutines. It does not create
OS threads. Every unfinished task resumes at most once per engine frame.

```luau
export type AsyncModule = {
    yield: (...any) -> ...any,
    count: () -> number,
    cancelAll: () -> number,
} & ((callback: () -> ...any) -> AsyncTask)

export type AsyncTask = {
    id: number,
    done: boolean,
    cancelled: boolean,
    status: "queued" | "running" | "suspended" | "completed" | "cancelled" | "error",
    error: string?,
    result: any,
    results: { any },
    cancel: (self: AsyncTask) -> boolean,
    Cancel: (self: AsyncTask) -> boolean,
    isDone: (self: AsyncTask) -> boolean,
    IsDone: (self: AsyncTask) -> boolean,
    getStatus: (self: AsyncTask) -> string,
    GetStatus: (self: AsyncTask) -> string,
    getError: (self: AsyncTask) -> string?,
    GetError: (self: AsyncTask) -> string?,
    getResult: (self: AsyncTask) -> ...any,
    GetResult: (self: AsyncTask) -> ...any,
}
```

```luau
local task = async(function()
    for chunk = 1, 100 do
        generateChunk(chunk)
        async.yield()
    end
    return "finished", 100
end)
```

## Module-function reference

| Function | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `async(callback)` | `callback`: zero-argument function whose return values become the task result. | A new `AsyncTask` in `"queued"` state. | Queues but does not immediately invoke the callback; first execution is on the next engine update. A callback error is captured on the task and printed rather than returned by this call. |
| `async.yield(...values)` | Any values to yield through the Luau coroutine. | Values supplied when the coroutine is next resumed, following ordinary coroutine semantics. | Suspends only the calling coroutine until a later engine update. Calling outside a yieldable coroutine raises. Engine scheduling does not currently inject resume values, so normal task use receives no values. |
| `async.count()` | None. | Non-negative number of queued/running/suspended unfinished tasks. | Completed, cancelled, and errored handles are excluded even while user code still references them. |
| `async.cancelAll()` | None. | Number of tasks newly changed to cancelled. | Already terminal tasks are ignored. Cancellation is cooperative between resumes; it cannot interrupt a callback while that callback is currently executing. |

## Handle fields and method reference

`result` is the first return value and `results` stores all return values as a
1-based table. Errors set `status = "error"`, `done = true`, and `error`, and
are also printed. Completed and cancelled tasks cannot be resumed.

| Canonical method | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `task:cancel()` (`Cancel` alias) | None beyond `self`. | `true` if this call changed an unfinished task; otherwise `false`. | Marks a queued or suspended task cancelled. It is idempotent and cannot interrupt Lua code already running during the same resume. |
| `task:isDone()` (`IsDone` alias) | None. | `boolean`. | `true` for `completed`, `cancelled`, and `error`; `false` for queued/running/suspended. Equivalent to the current `done` field. |
| `task:getStatus()` (`GetStatus` alias) | None. | One status string from the declared status union. | Returns a snapshot; retaining the string does not track later state. |
| `task:getError()` (`GetError` alias) | None. | Error message `string`, or `nil`. | Only errored tasks normally have a message. A not-yet-finished, completed, or cancelled task returns `nil`. |
| `task:getResult()` (`GetResult` alias) | None. | Every callback return value as multiple results. | Before successful completion, or after cancellation/error, it returns no values. Nil holes follow normal Luau multiple-return/table limitations; use `status` to distinguish unfinished from a callback that intentionally returned nothing. |

```luau
local job = async(function()
    async.yield()
    return "ready", 42
end)

-- In a later frame:
if job:isDone() and job:getStatus() == "completed" then
    local label, value = job:getResult()
    print(label, value)
end
```

::: warning
Synchronous asset, filesystem, networking setup, and command calls still block
the frame in which they execute. Break CPU-heavy Luau work into bounded chunks
and yield regularly.
:::

<!-- page: assets | Assets API -->
# Assets API

Global: `assets`

```luau
export type AssetsModule = {
    loadImage: (pathOrBase64Png: string) -> ImageHandle,
    loadImageBase64: (base64Png: string) -> ImageHandle,
    snapPhoto: (x: number, y: number, x2: number, y2: number) -> ImageHandle,
    newImage: (width: number, height: number, color: Color4Value?) -> ImageHandle,
    loadSound: (path: string) -> SoundHandle,
    newSound: (sampleRate: number, channels: number, len: number, fill: number?) -> SoundHandle,
    unloadImage: (value: string | ImageHandle) -> boolean,
    unloadSound: (value: string | SoundHandle) -> boolean,
    gc: () -> (number, number),
}
```

## Image functions

| Function | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `loadImage(pathOrBase64Png)` | A project/data resource path, raw PNG base64, `base64:...`, or `data:image/png;base64,...`. | Live `ImageHandle`. | File loads are cached by resolved path. Base64 is decoded as PNG. Missing files, malformed base64, unsupported/corrupt image data, and path violations raise. An explicitly unloaded cached image is decoded again on the next load. |
| `loadImageBase64(base64Png)` | Raw, `base64:`-prefixed, or PNG data-URI text. | Live `ImageHandle`. | Bypasses path detection and decodes PNG bytes. Invalid encoding/data raises; equivalent text may share the base64 cache. |
| `snapPhoto(x, y, x2, y2)` | Opposite rectangle corners in logical screen pixels. | New mutable `ImageHandle` containing the clipped region. | Coordinates may be supplied in either order and are clipped to the last completed frame. It raises before a frame exists or if clipping leaves no positive-area region. The snapshot does not update with future frames. |
| `newImage(width, height, color?)` | Numeric dimensions and optional initial color, default opaque white. | New mutable `ImageHandle`. | Dimensions are converted to integer pixel counts and capped at `65535` each; non-positive/invalid dimensions raise. Every pixel receives a copy of the clamped color. |
| `unloadImage(value)` | Cached path/base64 key or an `ImageHandle`. | `true` if a live cache/handle was invalidated; otherwise `false`. | Unloading is idempotent. Other references to the same handle become unusable; a later path load creates a live replacement. |

```luau
export type ImageHandle = {
    width: (self: ImageHandle) -> number,
    height: (self: ImageHandle) -> number,
    size: (self: ImageHandle) -> (number, number),
    getPixel: (self: ImageHandle, x: number, y: number) -> Color4Value,
    setPixel: ((self: ImageHandle, x: number, y: number, color: Color4Value) -> ())
        & ((self: ImageHandle, x: number, y: number,
            r: number, g: number, b: number, a: number?) -> ()),
    fill: ((self: ImageHandle, color: Color4Value) -> ())
        & ((self: ImageHandle, r: number, g: number, b: number, a: number?) -> ()),
    upload: (self: ImageHandle) -> (),
    export: (self: ImageHandle, path: string) -> (),
    save: (self: ImageHandle, path: string) -> (),
    unload: (self: ImageHandle) -> (),
    isUnloaded: (self: ImageHandle) -> boolean,
}
```

Pixels are zero-based. Both mutation methods accept a `Color4` table or separate
`r,g,b[,a]` channels, clamped to bytes. `setPixel` and `fill` modify the CPU
copy; call `upload` before expecting an already uploaded texture to reflect
changes.

| Canonical method | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `image:width()` | None. | Pixel width as an integer. | Raises after unload. |
| `image:height()` | None. | Pixel height as an integer. | Raises after unload. |
| `image:size()` | None. | Width, then height. | Both results match the methods above; raises after unload. |
| `image:getPixel(x, y)` | Zero-based integer pixel coordinates. | New `Color4Value` table for that pixel. | Raises when either coordinate is outside the image or the handle is unloaded. Mutating the returned table does not edit the image. |
| `image:setPixel(x, y, color)` | Coordinates plus `Color4Value`; the runtime also accepts `x, y, r, g, b, a?`. | `()` | Updates the CPU copy at exactly one pixel. Channels clamp to bytes. Out-of-range coordinates/unloaded handles raise. Call `upload()` to refresh a texture already sent to the renderer. |
| `image:fill(color)` | `Color4Value`; the runtime also accepts `r, g, b, a?`. | `()` | Replaces every CPU-side pixel. It does not resize or automatically re-upload an existing GPU texture. |
| `image:upload()` | None. | `()` | Uploads or refreshes renderer texture data from the CPU copy. Repeated calls are allowed; unloaded handles raise. |
| `image:export(path)` (`save` alias) | Destination path. | `()` | Encodes the current CPU copy as PNG. Adds `.png` when no extension is present; any other extension raises. Relative paths use the writable data root and parent directories are created where supported. I/O/encoding failures raise. |
| `image:unload()` | None. | `()` | Invalidates the handle and associated cached/render resources. Repeating it is safe; all data methods subsequently raise. Use `isUnloaded()` when the distinction matters. |
| `image:isUnloaded()` | None. | `boolean`. | Never raises solely because the handle is unloaded. |

```luau
local checker = assets.newImage(16, 16, Color4(20, 20, 24))
for y = 0, 15 do
    for x = 0, 15 do
        if (x + y) % 2 == 0 then
            checker:setPixel(x, y, 240, 90, 160)
        end
    end
end
checker:upload()
checker:save("generated/checker.png")
```

## Sound functions

| Function | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `loadSound(path)` | Project/data resource path to supported encoded audio. | Live cached `SoundHandle`. | Resolves writable data before packaged resources. Missing, unreadable, corrupt, or unsupported audio raises. Browser encoded audio can be playable without exposing decoded samples, as described below. |
| `newSound(sampleRate, channels, len, fill?)` | Positive sample rate, channel count of at least `1`, requested interleaved sample count, and optional initial amplitude (default `0`). | New editable `SoundHandle`. | Values are converted to the runtime's integral counts; sample length is padded upward to a complete channel frame and fill clamps to `-1..1`. Invalid/non-positive rate, channels, or length raise. |
| `unloadSound(value)` | Cached path or `SoundHandle`. | `true` if a live sound/cache entry changed; otherwise `false`. | Idempotent. All references to an invalidated handle cease to be playable/editable; loading the path again creates a replacement. |

```luau
export type SoundHandle = {
    sampleRate: (self: SoundHandle) -> number,
    channels: (self: SoundHandle) -> number,
    len: (self: SoundHandle) -> number,
    getSample: (self: SoundHandle, index: number) -> number,
    setSample: (self: SoundHandle, index: number, value: number) -> (),
    upload: (self: SoundHandle) -> (),
    export: (self: SoundHandle, path: string) -> (),
    save: (self: SoundHandle, path: string) -> (),
    unload: (self: SoundHandle) -> (),
    isUnloaded: (self: SoundHandle) -> boolean,
}
```

Channels must be at least one. Fill and assigned sample values are clamped to
`-1..1`. Sample indexes are zero-based. `len()` returns total interleaved
samples after any padding. `upload`
refreshes the playable buffer. `save` and `export` write WAV and add `.wav` if
needed.

| Canonical method | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `sound:sampleRate()` | None. | Sample rate in Hz, or `0` for opaque browser-encoded audio. | Raises after unload. |
| `sound:channels()` | None. | Interleaved channel count, or `0` for opaque browser audio. | Raises after unload. |
| `sound:len()` | None. | Total interleaved sample count, or `0` when samples are unavailable. | This is not frames-per-channel; divide by `channels()` when nonzero. |
| `sound:getSample(index)` | Zero-based interleaved sample index. | Amplitude number in `-1..1`. | Out-of-range indexes, unavailable decoded samples, and unloaded handles raise. |
| `sound:setSample(index, value)` | Zero-based interleaved index and numeric amplitude. | `()` | Writes the CPU sample, clamping amplitude to `-1..1`; it does not automatically refresh current playback. Invalid indexes/unavailable data raise. |
| `sound:upload()` | None. | `()` | Rebuilds the playable buffer from editable samples. Opaque browser audio and unloaded handles reject sample upload. Existing playback may need to be restarted to use changed data. |
| `sound:export(path)` (`save` alias) | Destination path. | `()` | Writes editable samples as WAV, adding `.wav` when no extension is present. Other extensions, unavailable decoded samples, or I/O failures raise. |
| `sound:unload()` | None. | `()` | Invalidates samples and playback resources. Repeated calls are safe. |
| `sound:isUnloaded()` | None. | `boolean`. | Reports invalidation without accessing sample data. |

```luau
local tone = assets.newSound(48_000, 1, 4_800)
for i = 0, tone:len() - 1 do
    tone:setSample(i, math.sin(i * 2 * math.pi * 440 / tone:sampleRate()) * 0.2)
end
tone:upload()
audio.playOnce(tone)
```

On web, encoded browser-audio loads retain their encoded bytes for playback but
do not expose decoded editable samples: `sampleRate()`, `channels()`, and
`len()` report zero. Newly generated sounds still use the editable WAV path.

## Cache collection and path rules

`assets.gc()` takes no parameters, removes cache entries whose weak handles no
longer have any live references, and returns two non-negative integers: image
entries removed, then sound entries removed. Explicitly unloaded entries may
also disappear; still-referenced live handles are never collected.
Unloaded handles reject further reads, writes,
uploads, rendering, and playback.

Relative loads check writable data first, then packaged resources. Relative
exports use data. Absolute and normalized parent-relative export destinations
are accepted subject to OS permissions. `snapPhoto` requires at least one
completed frame.

<!-- page: audio | Audio API -->
# Audio API

Global: `audio`

```luau
export type SpatialAudio3DOptions = {
    voice_id: number?,
    looping: boolean?,
    volume: number?,
    min_distance: number?,
    max_distance: number?,
    rolloff: number?,
    distance_model: "inverse" | "linear" | "exponential"?,
}

export type AudioModule = {
    play: (sound: SoundHandle, looped: boolean?, volume: number?) -> (),
    playOnce: (sound: SoundHandle, volume: number?) -> (),
    stop: (sound: SoundHandle) -> (),
    setVolume: (sound: SoundHandle, volume: number) -> (),
    playSpatial: (sound: SoundHandle, x: number, y: number, looped: boolean?, volume: number?) -> (),
    setPosition: (sound: SoundHandle, x: number, y: number) -> boolean,
    setListenerPosition: (x: number, y: number) -> (),
    playSpatial3D: (sound: SoundHandle, x: number, y: number, z: number, options: SpatialAudio3DOptions?) -> number,
    updateSpatial3D: (voiceId: number, x: number, y: number, z: number, options: SpatialAudio3DOptions?) -> boolean,
    stopSpatial3D: (voiceId: number) -> (),
    setListener3D: (x: number, y: number, z: number, forwardX: number, forwardY: number, forwardZ: number, upX: number, upY: number, upZ: number, earDistance: number?) -> (),
}
```

| Function | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `play(sound, looped?, volume?)` | Live `SoundHandle`; optional loop flag (default `false`); optional gain (default `1`). | `()` | Starts or restarts non-spatial playback associated with this handle. Gain clamps to `0..1`. Invalid/unloaded sound data or an unavailable audio device raises/reports through the platform audio backend. |
| `playOnce(sound, volume?)` | Live sound and optional gain. | `()` | Exact convenience behavior of `play(sound, false, volume)`. A later `play` on the same handle replaces its associated playback. |
| `stop(sound)` | Sound whose associated playback should stop. | `()` | Stops both looping and one-shot playback tracked for the handle. Calling it when nothing is active is a no-op; an unloaded handle is not a valid sound argument. |
| `setVolume(sound, volume)` | Sound and numeric gain. | `()` | Changes the tracked active playback, clamping to `0..1`. If no playback is active there is nothing to modify; use the next `play` call's volume. |
| `playSpatial(sound, x, y, looped?, volume?)` | Live sound, world-space emitter position, optional loop flag and gain. | `()` | Starts/restarts tracked 2D spatial playback. Attenuation depends on distance from the current listener. Non-finite coordinates are invalid. |
| `setPosition(sound, x, y)` | Sound and new world-space emitter coordinates. | `true` when an active spatial emitter moved; `false` when none exists. | Non-spatial playback does not count as an emitter. |
| `setListenerPosition(x, y)` | World-space listener coordinates. | `()` | Affects current and future spatial playback. The listener is global; the Camera component does not move it automatically. |
| `playSpatial3D(sound, x, y, z, options?)` | Live sound, world-space position, and optional voice/loop/gain/distance settings. | Independent numeric voice id. | Starts a native 3D voice. Reusing an explicit `voice_id` replaces only that voice, so separate entities may share one sound. Distances sanitize to `min_distance >= 0.001`, `max_distance >= min_distance`; volume clamps to `0..1` and rolloff to non-negative. Snake-case fields and the declared camel-case aliases are accepted. |
| `updateSpatial3D(voiceId, x, y, z, options?)` | Existing voice, current position, and live settings. | `true` while the voice exists. | Moves the emitter and applies edited gain/attenuation without restarting playback. Returns `false` for a missing or completed native one-shot. |
| `stopSpatial3D(voiceId)` | Voice returned by `playSpatial3D`. | `()` | Stops only that 3D voice; it does not stop another source using the same sound asset. |
| `setListener3D(...)` | Position, forward/up vectors, and optional ear separation. | `()` | Updates current and future 3D voices. Native output uses oriented listener-relative stereo plus the same WebAudio distance equations used by the browser. Prefer `AudioListener3D` for automatic world-transform synchronization. |

Volume is clamped to `0..1`. Browser playback is subject to user-gesture
autoplay restrictions. `SpatialSound2D` is preferable when the emitter should
follow an entity automatically.

For 3D scenes, prefer `AudioSource3D` and `AudioListener3D`. Their component
updates resolve the same nested world transforms used by rendering and physics,
and each source owns an independent voice even when sounds are shared.

```luau
local bell = assets.loadSound("assets/bell.wav")
audio.setListenerPosition(player:GetWorldPosition())
local towerX, towerY = tower:GetWorldPosition()
audio.playSpatial(bell, towerX, towerY, false, 0.8)
```

<!-- page: media | Microphone and Camera API -->
# Microphone and Camera API

Globals: `media`, `microphone` (`media.microphone` is the same table)

NeoLOVE never opens a microphone/camera or displays a permission prompt merely
because the module exists. Device enumeration and permission-status reads do
not open hardware. Only an explicit `media.requestAccess(...)`,
`microphone.requestAccess(...)`, or `microphone.requestDevice(...)` call
requests capture permission and starts devices. Make that call in response to a
clear user action, explain what will be captured, retain the returned stream
only as long as needed, and stop it when the feature ends.

All discovery/access callbacks are asynchronous and are delivered at the start
of a later engine frame. Submission returns a request id, never a stream
directly. Callback errors in game code are logged and do not make the request
callback run again.

## Devices, permissions, and support

```luau
export type MediaPermissionStatus = "prompt" | "granted" | "denied" | "unavailable"
export type MediaDeviceKind = "microphone" | "camera"
export type MediaEnumerationKind = MediaDeviceKind | "all" | "both"

export type MediaDevice = {
    id: string,
    kind: MediaDeviceKind,
    label: string,
    isDefault: boolean,
}

export type MediaDeviceResult = {
    ok: boolean,
    devices: { MediaDevice }?,
    code: string?,
    error: string?,
}
```

Device ids are opaque selections for a later constraint's `deviceId`; do not
parse, persist as identity, or expose them unnecessarily. They can change after
reboot, reconnect, browser permission changes, or site-data clearing. Browser
labels/ids may be blank, generic, or anonymized until permission is granted.

| Function | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `media.enumerateDevices(callback)` / `media.enumerateDevices(kind, callback)` | Optional `"microphone"`, `"camera"`, `"all"`, or `"both"` filter; callback receives `MediaDeviceResult`. Omitted kind means both. | Monotonically increasing request-id number. | Enumerates asynchronously without opening a device. Success has `ok = true` and a `devices` array, which can legitimately be empty. Failure has `ok = false`, `code`, and `error`. When both kinds are requested, native enumeration succeeds with devices from the working kind if the other kind failed; it fails only when no devices were returned and at least one enumeration failed. Runtime compatibility spellings `mic`/`audio`/`audioinput` and `video`/`videoinput` are accepted, but canonical code should use the declared names. Invalid call shapes/kinds raise synchronously and do not schedule a callback. |
| `media.getPermissionStatus(kind)` | Exactly `"microphone"` or `"camera"`. | `"prompt"`, `"granted"`, `"denied"`, or `"unavailable"`. | Read-only snapshot; never prompts. `prompt` means access has not produced a known decision, not a guarantee the next request will show UI. `unavailable` means the backend/policy cannot request that kind. `"all"`/`"both"` and unknown kinds raise. Browser Permissions API updates can arrive asynchronously, so a later call can differ. |
| `media.permissions()` | None. | New `{ microphone = status, camera = status }` table. | Snapshot equivalent to two permission reads. Mutating it does not change permission state. A failed combined audio+video request does not mark both denied when the runtime cannot know which permission failed; query each status or request separately when the distinction matters. |
| `media.isSupported(kind)` | One device/enumeration kind. | `boolean`. | Reports backend capability, **not** attached hardware, current permission, or whether a requested format will work. `all`/`both` is true only when the backend supports both categories. Linux, Windows, and macOS can return true with no device connected. Web requires a secure context and `getUserMedia`; Android and native platforms without a capture backend return false. Invalid kinds raise. |

```luau
local statuses = media.permissions()
print("microphone", statuses.microphone, "camera", statuses.camera)

media.enumerateDevices("camera", function(result)
    if not result.ok then
        print("camera discovery failed", result.code, result.error)
        return
    end
    for _, device in ipairs(result.devices or {}) do
        print(device.id, device.label, device.isDefault)
    end
end)
```

## Focused `microphone` library

The focused library exposes the common list → choose → request flow without a
mixed audio/video options table. `microphone` and `media.microphone` refer to
the same object and use the same asynchronous backend, request ids, streams,
permission rules, and errors as `media`. Listing devices never opens a
microphone or prompts for permission.

```luau
export type MicrophoneModule = {
    listDevices: (callback: (MediaDeviceResult) -> ()) -> number,
    enumerateDevices: (callback: (MediaDeviceResult) -> ()) -> number,
    requestAccess: (((MediaAccessResult) -> ()) -> number)
        & ((MediaAudioConstraints, (MediaAccessResult) -> ()) -> number)
        & ((string, (MediaAccessResult) -> ()) -> number),
    requestDevice: (deviceId: string, callback: (MediaAccessResult) -> ()) -> number,
    cancelRequest: (requestId: number) -> boolean,
    getPermissionStatus: () -> MediaPermissionStatus,
    isSupported: () -> boolean,
}
```

| Function | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `microphone.listDevices(callback)` (`enumerateDevices` alias) | Callback receiving `MediaDeviceResult`. | Monotonically increasing request-id number. | Enumerates only microphone devices. A successful result can contain an empty `devices` array. Device ids and labels have the privacy/lifetime caveats above. Invalid callback types raise synchronously. |
| `microphone.requestAccess(callback)` | Access-result callback. | Request-id number. | Requests the default microphone with backend defaults. It is asynchronous and may display a permission prompt. |
| `microphone.requestAccess(constraints, callback)` | `MediaAudioConstraints` table and result callback. | Request-id number. | Requests one microphone with the supplied device/rate/channel/processing preferences. Actual negotiated format can differ; inspect `stream:getAudioFormat()`. Invalid constraints raise before a request is queued. |
| `microphone.requestAccess(deviceId, callback)` | Opaque enumeration id and result callback. | Request-id number. | Shorthand for `{ deviceId = deviceId }`. A stale/unknown id completes with `device_unavailable`; an empty id selects the default. |
| `microphone.requestDevice(deviceId, callback)` | Opaque enumeration id and result callback. | Request-id number. | Explicitly named alias of the string overload above, useful after a device picker. It does not validate that the id is still attached until asynchronous startup. |
| `microphone.cancelRequest(requestId)` | Pending enumeration/access request id. | `boolean`. | Same cancellation function as `media.cancelRequest`; `true` means this call first cancelled a pending request. The callback still runs once with code `cancelled`. |
| `microphone.getPermissionStatus()` | None. | `MediaPermissionStatus`. | Read-only microphone permission snapshot; never prompts. |
| `microphone.isSupported()` | None. | `boolean`. | Reports backend microphone capability, not whether a device is attached or permission is granted. |

```luau
local microphoneStream: MediaStream? = nil

microphone.listDevices(function(result)
    if not result.ok then
        print("Microphone discovery failed", result.code, result.error)
        return
    end

    local devices = result.devices or {}
    for index, device in ipairs(devices) do
        print(index, device.label, device.id, device.isDefault)
    end
    if #devices == 0 then
        print("No microphone is attached")
        return
    end

    -- In a real settings screen, pass the id chosen by the user.
    microphone.requestDevice(devices[1].id, function(access)
        if access.ok then
            microphoneStream = access.stream
            local format = microphoneStream:getAudioFormat()
            if format then
                print("Using microphone", devices[1].label, format.sampleRate, format.channels)
            end
        else
            print("Microphone could not start", access.code, access.error)
        end
    end)
end)

-- When voice mode ends:
-- if microphoneStream then microphoneStream:stop(); microphoneStream = nil end
```

## Requesting access

```luau
export type MediaAudioConstraints = {
    deviceId: string?,
    sampleRate: number?,
    channels: number?,
    echoCancellation: boolean?,
    noiseSuppression: boolean?,
    autoGainControl: boolean?,
}

export type MediaVideoConstraints = {
    deviceId: string?,
    width: number?,
    height: number?,
    frameRate: number?,
    facingMode: ("user" | "environment" | "left" | "right")?,
}

export type MediaRequestOptions = {
    audio: (boolean | MediaAudioConstraints)?,
    video: (boolean | MediaVideoConstraints)?,
    microphone: (boolean | MediaAudioConstraints)?,
    camera: (boolean | MediaVideoConstraints)?,
}

export type MediaAccessResult = {
    ok: boolean,
    stream: MediaStream?,
    code: string?,
    error: string?,
}
```

`audio` and `video` are canonical; `microphone` and `camera` are fallback
aliases used only when their canonical key is nil. Thus `audio = false` disables
audio even if `microphone` also contains a table. `true` requests backend
defaults, `false`/nil disables that track, and a table requests it with
constraints. At least one track must be enabled.

| Constraint | Accepted values | Negotiation and edge cases |
| --- | --- | --- |
| `audio.deviceId` | Id from microphone enumeration, or a platform id string. | Selects that input. Empty/default ids choose the default. A stale/unknown exact id fails with `device_unavailable`. |
| `audio.sampleRate` | Whole number `8000..384000`. | Native selects the closest supported rate/config; web sends an ideal constraint and reports the AudioContext's actual rate. Read `stream:getAudioFormat()` rather than assuming the request was exact. |
| `audio.channels` | Whole number `1..32`. | Native selects the closest supported channel configuration; browser support varies. Samples are always returned interleaved at the negotiated count. |
| `echoCancellation`, `noiseSuppression`, `autoGainControl` | Optional booleans. | Passed to browser media constraints. The current desktop backend accepts but does not implement these software-processing hints. |
| `video.deviceId` | Camera enumeration id/platform id. | Selects the device; stale ids fail. Empty/default uses the first/default camera. |
| `video.width`, `video.height` | Whole numbers `1..16384`. | Desired dimensions. Desktop selects the closest decodable format; browser treats them as ideal. Actual frame dimensions can differ and can change, so inspect each frame. |
| `video.frameRate` | Whole number `1..240`. | Desired FPS, not a delivery guarantee. Slow game reads drop intermediate frames rather than building an unbounded queue. |
| `video.facingMode` | `user`, `environment`, `left`, or `right`. | Browser ideal-facing hint. The current desktop backend accepts but ignores it; choose a desktop `deviceId` instead. Values are case-sensitive and unknown values raise synchronously. |

All numeric constraints must be finite integers in range. Wrong types,
fractional/out-of-range values, a non-table options argument, a non-function
callback, or enabling neither track raises synchronously; no request id is
returned and no callback runs.

### `media.requestAccess(options, callback) -> number`

Validates the options, registers the callback, starts an asynchronous
all-or-nothing capture request, and returns its id. Success invokes the callback
once with `{ ok = true, stream = MediaStream }`. Failure invokes it once with
`{ ok = false, code = ..., error = ... }`. A combined request succeeds only
when both requested tracks open; if either fails, partially opened tracks are
stopped. Native startup times out after about 60 seconds rather than waiting
forever for a device/permission response.

Keep a strong reference to `result.stream`. Dropping the last Luau stream
handle stops its devices automatically; saving only a frame image does not keep
capture alive.

### `media.cancelRequest(requestId) -> boolean`

Returns `true` only when this call first marks a still-pending enumeration or
access request cancelled. It returns `false` for an unknown, already completed,
or already cancelled id. A successful cancellation still invokes the original
callback exactly once on a later frame with `ok = false`, `code = "cancelled"`;
any stream that races to completion is immediately stopped. Cancellation
cannot retract a permission choice the user already made at the OS/browser
prompt.

### Result error codes

Every asynchronous failure includes a human-readable `error`; branch on the
stable `code` and show/log the message. Backends may add codes in future.

| Code | Meaning and likely recovery |
| --- | --- |
| `cancelled` | The matching pending request was cancelled. Usually no user-facing error is needed. |
| `permission_denied` | User, OS privacy settings, browser policy, insecure/security policy, or a permission timeout denied capture. Explain how to re-enable permission; do not immediately reprompt in a loop. |
| `device_unavailable` | No matching/default device exists, or a selected id went stale. Re-enumerate and let the user choose another device. |
| `device_busy` | Another application/session owns the device or it could not be read. Offer retry after the other use ends. |
| `constraints_unsatisfied` | Browser could not satisfy a requested constraint. Relax dimensions/rate/channels or request defaults. |
| `unsupported_format` | Native microphone hardware exposed a sample format this build cannot convert. Choose another device/config. |
| `unsupported` | Capture API/backend/context is unavailable (including current Android builds and insecure web capture). Hide/disable the feature or direct web users to HTTPS/localhost. |
| `invalid_options` | Backend rejected encoded options. Most invalid Luau options instead raise synchronously before submission. |
| `capture_failed` | Generic startup/runtime worker, decode, timeout, or browser failure not covered above. The accompanying message is diagnostic. |

## Stream methods and capture formats

```luau
export type MediaAudioFormat = { sampleRate: number, channels: number }
export type MediaVideoFormat = { width: number, height: number, frameRate: number }

export type MediaAudioSamples = MediaAudioFormat & {
    frameCount: number,
    droppedSamples: number,
    format: "f32le",
    samples: { number },
}

export type MediaAudioBytes = MediaAudioFormat & {
    frameCount: number,
    droppedSamples: number,
    format: "f32le",
    data: string,
}

export type MediaVideoFrame = {
    image: ImageHandle,
    width: number,
    height: number,
    timestamp: number,
    droppedFrames: number,
}
```

| Method | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `stream:stop()` | None. | `()` | Idempotently stops all tracks, clears buffered audio/video, releases device/backend nodes, and wipes the stream's privacy-sensitive live camera image (including retained clones) to one transparent pixel. The image handle remains safely renderable while a preview is removed. The stream remains queryable for state, formats, and its last runtime error, but read methods raise `media stream is stopped`. Always stop promptly when leaving voice/camera mode. |
| `stream:isActive()` | None. | `boolean`. | True while the backend has at least one live track and no stop/runtime-failure flag. A cable disconnect, browser track end, runtime capture error, explicit stop, or `stopAll` makes it false. |
| `stream:hasAudio()` | None. | `boolean`. | Reports whether the negotiated stream was created with an audio track. It remains a format/capability fact after stop; it is not active-state. |
| `stream:hasVideo()` | None. | `boolean`. | Same rule for the video track. |
| `stream:getAudioFormat()` | None. | New `{ sampleRate, channels }`, or `nil` without audio. | Actual negotiated values. Browser processing can update internal capture details, but returned chunks also carry their exact format. Mutating the table has no effect. |
| `stream:getVideoFormat()` | None. | New `{ width, height, frameRate }`, or `nil` without video. | Initial negotiated/settings values; web width/height/FPS can be `0` before metadata is ready. Each returned frame is authoritative if the camera changes resolution. |
| `stream:readAudio(maxFrames?)` | Optional whole frame limit, default `1024`, valid `1..16384`. | `MediaAudioSamples`, or `nil` when no complete frame is buffered. | Consumes oldest buffered complete channel frames. Raises if the stream has no microphone track, has stopped, or the limit is invalid. `samples` is 1-based, interleaved floating-point PCM (frame 1 channel 1, frame 1 channel 2, ...), normally `-1..1`. |
| `stream:readAudioBytes(maxFrames?)` | Same frame limit. | `MediaAudioBytes`, or `nil`. | Consumes from the **same FIFO** as `readAudio`; choose one representation per consumer. Raises without a microphone track, after stop, or for an invalid limit. `data` is a binary-safe Luau string of interleaved IEEE-754 32-bit little-endian floats. Its byte length is `frameCount * channels * 4`. Use this form for compact network/storage conversion. |
| `stream:readVideoFrame()` | None. | New frame record, or `nil` when no newer frame is available. | Consumes the newest available camera frame; intermediate frames are intentionally replaced/dropped. Raises if there is no camera track, the stream has stopped, or a frame cannot be copied/decoded. Updating the frame image is CPU/GPU revision-aware; do not call `image:upload()`. |
| `stream:getLastError()` | None. | Runtime error/track-ended message string, or `nil`. | Startup failures arrive through `MediaAccessResult`, not here. This reports an error after a previously successful stream; it may remain readable after activity ends. A normal explicit stop need not set an error. |

Audio capture retains at most approximately five seconds. If the game does not
read quickly enough, oldest interleaved samples are discarded.
`droppedSamples` reports the number of individual channel samples discarded
since the previous successful audio read (divide by `channels` for dropped
frames); it resets when a non-empty chunk is returned. `frameCount` is the
number of complete multi-channel frames actually returned, never the raw sample
array length.

Camera capture retains only its newest unread frame. `droppedFrames` reports
frames overwritten since the previous successful frame read. `timestamp` is a
monotonic seconds value useful for ordering within that stream/session, but its
origin differs by backend and must not be compared across machines/streams.

::: warning Live camera image
Each stream owns exactly one mutable camera `ImageHandle`. Every successful
`readVideoFrame()` updates that same image's pixels, dimensions, and renderer
revision. A previously retained `oldFrame.image` therefore shows the **newest
read frame**, not a snapshot. This avoids allocating a texture every frame and
lets a `Sprite2D` keep the first image handle. Copy/export pixels before the next
read when a snapshot is required. The live handle has no export destination of
its own: copy its pixels into an `assets.newImage(...)` handle, then export that
copy. Do not unload this live image while its
stream is active. Stopping the stream wipes the image and every retained clone
to one transparent pixel so captured pixels are not kept implicitly; explicitly
copy a frame first when the application has informed consent to retain it.
:::

### `media.stopAll() -> number`

Stops every still-referenced active media stream and backend capture session,
returning how many tracked streams were active immediately before stopping.
Repeated calls normally return `0`. It does not cancel pending access/device
requests; cancel those ids separately. Runtime shutdown/module teardown stops
pending work and active streams, and the web build also stops tracks on page
hide/unload.

### `media._poll() -> ()`

Drains device/access completions and invokes callbacks. The engine calls it at
the start of each update. Manual calls return nothing but can re-enter user code
and change callback ordering, so `_poll` is engine-managed.

## Camera preview example

Request access from a button/click handler on web. The stream and component are
retained outside the callback; the same sprite image then updates without
allocation on each successful read.

```luau
local cameraStream: MediaStream? = nil
local previewSprite: Sprite2D? = nil

local function startCamera()
    if not media.isSupported("camera") then
        print("Camera capture is unavailable")
        return
    end

    media.requestAccess({
        video = { width = 640, height = 480, frameRate = 30, facingMode = "user" },
    }, function(result)
        if not result.ok then
            print("Camera failed", result.code, result.error)
            return
        end
        cameraStream = result.stream -- retain it or capture stops
    end)
end

local previewEntity = ecs.newEntity("Camera preview", ecs.root, 32, 32)
previewEntity.size_x, previewEntity.size_y = 320, 240
previewSprite = previewEntity:AddComponent(core.Sprite2D)

function update(dt)
    if cameraStream and cameraStream:isActive() then
        local frame = cameraStream:readVideoFrame()
        if frame then
            previewSprite.image = frame.image -- same live handle after first frame
            previewEntity.size_y = previewEntity.size_x * frame.height / frame.width
        end
    end
end

-- Wire this to a visible opt-in button:
-- permissionButton.onClick = function() startCamera() end
-- On leaving the camera mode: cameraStream:stop(); cameraStream = nil
```

## Proximity-voice transport example

`readAudioBytes` supplies capture PCM; it does not provide a codec, jitter
buffer, echo mixer, encryption, or networking. This compact example sends
20-ms-ish PCM packets through a class-service client. A production game should
add an audio codec, sequence/timestamps, authentication/moderation, jitter
buffering, resampling, packet-loss handling, and bandwidth/rate limits.

```luau
local voiceStream: MediaStream? = nil

local function startVoiceAfterUserConsent()
    media.requestAccess({
        audio = {
            sampleRate = 48_000,
            channels = 1,
            echoCancellation = true,
            noiseSuppression = true,
        },
    }, function(result)
        if result.ok then
            voiceStream = result.stream
        else
            print("Voice unavailable", result.code, result.error)
        end
    end)
end

local function sendVoice(networkClient, player)
    if not voiceStream or not voiceStream:isActive() then return end
    local format = voiceStream:getAudioFormat()
    if not format then return end

    local framesPerPacket = math.max(1, math.floor(format.sampleRate * 0.02))
    local chunk = voiceStream:readAudioBytes(framesPerPacket)
    if not chunk then return end
    local x, y = player:GetWorldPosition()
    networkClient:emit("voice", {
        sampleRate = chunk.sampleRate,
        channels = chunk.channels,
        frameCount = chunk.frameCount,
        pcm = buffer.fromstring(chunk.data),
        x = x,
        y = y,
    })
end
```

A minimal receiver can turn one packet into a generated sound and place it in
world space. This is intentionally simple and allocates per packet; a real
voice player should reuse buffers and queue decoded frames smoothly.

```luau
networkClient:on("voice", function(packet)
    local sampleCount = packet.frameCount * packet.channels
    local sound = assets.newSound(packet.sampleRate, packet.channels, sampleCount)
    for index = 0, sampleCount - 1 do
        sound:setSample(index, buffer.readf32(packet.pcm, index * 4))
    end
    sound:upload()
    audio.playSpatial(sound, packet.x, packet.y, false, 1)
end)
```

## Platform and privacy notes

- Linux, Windows, and macOS desktop builds support microphone/camera discovery and capture. Requested
  formats are best-fit; `isSupported` does not promise hardware is attached.
  macOS can show an OS camera prompt; other desktop privacy controls can still
  deny or hide devices.
- Web builds require a secure context (HTTPS or browser-trusted localhost) and
  `navigator.mediaDevices.getUserMedia`. Permissions Policy, embedding iframe
  policy, browser settings, and OS privacy settings still apply. Start access
  from a click/tap/key gesture; the audio track also uses Web Audio and browsers
  can keep its context suspended until a gesture. Enumeration may conceal
  labels before permission.
- Native Android capture is not available in this build:
  `isSupported(...)` is false, permission statuses are `unavailable`, device
  enumeration succeeds with an empty list, and access callbacks fail with
  `code = "unsupported"`. The iOS simulator is not currently documented as a
  supported capture deployment target.
- Never treat permission as permanent. Provide an obvious mute/camera-off
  control, stop on disconnect/scene exit, avoid recording/transmitting without
  an active indicator, and protect captured/networked data according to the
  user's jurisdiction and your game's privacy policy.

<!-- page: filesystem | File System API -->
# File System API

Global: `fs`

```luau
export type FsWalkEntry = {
    path: string,
    name: string,
    kind: "file" | "directory",
    isFile: boolean,
    isDir: boolean,
    is_file: boolean,
    is_dir: boolean,
}

export type FsModule = {
    isWebasm: () -> boolean,
    isWebAssembly: () -> boolean,
    isMobile: () -> boolean,
    isAndroid: () -> boolean,
    openFilePicker: () -> string?,
    openFolderPicker: () -> string?,
    getDataDirectory: () -> string,
    dataPath: (path: string) -> string,
    readFile: (path: string) -> string,
    readBytes: (path: string) -> string,
    writeFile: (path: string, content: string) -> (),
    appendFile: (path: string, content: string) -> (),
    exists: (path: string) -> boolean,
    isFile: (path: string) -> boolean,
    isDir: (path: string) -> boolean,
    createDir: (path: string) -> (),
    walk: (path: string?, recursive: boolean?) -> { FsWalkEntry },
    rename: (from: string, to: string) -> (),
    copy: (from: string, to: string) -> (),
    removeFile: (path: string) -> boolean,
}
```

## Complete function reference

| Canonical function | Parameters | Returns | Result, path behavior, and edge cases |
| --- | --- | --- | --- |
| `isWebasm()` (`isWebAssembly` alias) | None. | `boolean`. | `true` only in the Emscripten browser target; mobile emulation does not affect it. |
| `isMobile()` | None. | `boolean`. | `true` on native Android/iOS and while desktop mobile emulation is active. |
| `isAndroid()` | None. | `boolean`. | `true` only on Android; Android-like emulation still returns `false`. |
| `openFilePicker()` | None. | Selected native path string, or `nil`. | Returns `nil` on cancel, picker failure/unavailability, web, and Android. It does not copy the selected file into the project/data root. |
| `openFolderPicker()` | None. | Selected native directory string, or `nil`. | Same cancellation/platform behavior as the file picker. |
| `getDataDirectory()` | None. | Absolute/default writable-root string. | The directory may not exist until the first write. The exact location is platform/build dependent. |
| `dataPath(path)` | Relative or absolute path string. | Resolved writable-path string. | Relative paths are joined to the data root; absolute paths stay absolute under normal platform rules. This resolves only—it does not create or validate the target. |
| `readFile(path)` | Read path. | UTF-8 text string. | Searches writable data before packaged resources. Missing files, directories, invalid UTF-8, and I/O failures raise. Use `readBytes` for arbitrary data. |
| `readBytes(path)` | Read path. | Byte-preserving Luau string. | Uses the same resolution order as `readFile`; missing/unreadable paths raise. Embedded NUL bytes are retained. |
| `writeFile(path, content)` | Writable destination and replacement string bytes. | `()` | Replaces/creates a file and creates parent directories. Relative destinations use data, never overwrite packaged resources. Permission/I/O failures raise. |
| `appendFile(path, content)` | Writable destination and bytes to append. | `()` | Creates the file/parents if absent; an empty string is a valid no-content append. I/O failures raise. |
| `exists(path)` | Read path. | `boolean`. | Tests data/resource resolution and returns `false` for missing/unresolvable targets rather than raising for ordinary absence. Either files or directories count. |
| `isFile(path)` | Read path. | `boolean`. | `true` only for a resolved regular file; missing paths/directories return `false`. |
| `isDir(path)` | Read path. | `boolean`. | `true` only for a resolved directory; missing paths/files return `false`. |
| `createDir(path)` | Writable directory path. | `()` | Recursively creates parents and succeeds when the directory already exists; a conflicting file or I/O failure raises. |
| `walk(path?, recursive?)` | Optional starting path (default data root) and optional recursion flag (default `true`). | Array of `FsWalkEntry` records. | Returns deterministic entries for the resolved directory. A file/nonexistent/unreadable start raises. Symlink handling follows the host filesystem and should not be used to assume sandboxing. |
| `rename(from, to)` | Two writable paths. | `()` | Renames and creates destination parents. Source absence, cross-device restrictions, destination conflicts, and I/O errors raise. Packaged-only resources cannot be renamed. |
| `copy(from, to)` | Read-resolved source and writable destination. | `()` | Copies one file or a directory tree and creates destination parents. It does not mutate the source. Missing source/conflicts/I/O errors raise. |
| `removeFile(path)` | Writable file path. | `true` if removed; `false` if absent. | Does not remove directories or packaged resources. Other failures, including a directory at the path, raise. |

Pickers return `nil` on web and Android. `removeFile` does not remove
directories. I/O errors otherwise raise with the operation and resolved path.

```luau
fs.createDir("saves")
fs.writeFile("saves/slot1.json", "{\"level\":3}")
if fs.isFile("saves/slot1.json") then
    local bytes = fs.readBytes("saves/slot1.json")
    fs.copy("saves/slot1.json", "saves/slot1.backup.json")
    print(#bytes)
end
```

<!-- page: platform | Android and Mobile APIs -->
# Android and Mobile APIs

## Android

Global: `android`

```luau
export type AndroidModule = {
    isAndroid: () -> boolean,
    getDeviceId: () -> string?,
    getSdkInt: () -> number?,
    getApiLevel: () -> number?,
    getBrand: () -> string?,
    getManufacturer: () -> string?,
    getModel: () -> string?,
    getDevice: () -> string?,
    getProduct: () -> string?,
    showKeyboard: (implicit: boolean?) -> boolean,
    openKeyboard: (implicit: boolean?) -> boolean,
    hideKeyboard: (implicitOnly: boolean?) -> boolean,
    closeKeyboard: (implicitOnly: boolean?) -> boolean,
}
```

| Canonical function | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `android.isAndroid()` | None. | `boolean`. | `true` only inside the Android runtime. Desktop mobile emulation does not change this result. |
| `android.getDeviceId()` | None. | Device-id string, or `nil`. | Returns the Android property supplied to the runtime. It is not promised to be globally unique, permanent, or suitable as an authentication secret. Returns `nil` outside Android or when unavailable. |
| `android.getSdkInt()` (`getApiLevel` alias) | None. | Android API-level integer, or `nil`. | Returns `nil` outside Android or when the platform omitted/failed to parse the property. |
| `android.getBrand()` | None. | Brand string, or `nil`. | Direct platform metadata; it can be blank/spoofed and is absent off Android. |
| `android.getManufacturer()` | None. | Manufacturer string, or `nil`. | Same availability caveats as `getBrand`. |
| `android.getModel()` | None. | Model string, or `nil`. | Same availability caveats as `getBrand`. |
| `android.getDevice()` | None. | Android device-code string, or `nil`. | This is the platform's device value, not a NeoLOVE entity/device handle. |
| `android.getProduct()` | None. | Product-code string, or `nil`. | Same availability caveats as the other metadata getters. |
| `android.showKeyboard(implicit?)` (`openKeyboard` alias) | Optional implicit-show flag, default `true`. | Whether the current Android activity accepted the request. | Returns `false` outside Android or without an activity; acceptance does not guarantee the IME becomes visible. |
| `android.hideKeyboard(implicitOnly?)` (`closeKeyboard` alias) | Optional implicit-only flag, default `false`. | Whether an Android activity accepted the request. | Returns `false` when unsupported/unavailable. |

```luau
if android.isAndroid() then
    print("Android API", android.getSdkInt() or "unknown")
    android.showKeyboard(false)
end
```

## Mobile state

Global: `mobile`

```luau
export type MobileModule = {
    isMobile: () -> boolean,
    isEmulated: () -> boolean,
    isOnline: () -> boolean,
    isWifiEnabled: () -> boolean,
    isCellularEnabled: () -> boolean,
    isLowPowerMode: () -> boolean,
    getNetworkType: () -> "wifi" | "cellular" | "offline",
    getOrientation: () -> "portrait" | "landscape",
    isLandscape: () -> boolean,
    getDeviceSize: () -> (number, number),
    getSafeAreaInsets: () -> (number, number, number, number),
}
```

`getNetworkType` prefers Wi-Fi over cellular. `getDeviceSize` returns the
oriented emulator size when enabled, otherwise the current window size.
`getSafeAreaInsets` returns `top, right, bottom, left`; the current runtime
models portrait mobile as `47, 0, 34, 0` and all other states as zeros.

| Function | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `mobile.isMobile()` | None. | `boolean`. | `true` on Android/iOS or when editor/desktop mobile emulation is enabled. |
| `mobile.isEmulated()` | None. | `boolean`. | `true` only for emulation; native mobile returns `false`. |
| `mobile.isOnline()` | None. | `boolean`. | Simulated connectivity: `true` when Wi-Fi or cellular state is enabled. It does not prove Internet reachability. |
| `mobile.isWifiEnabled()` | None. | `boolean`. | Current simulated Wi-Fi flag. |
| `mobile.isCellularEnabled()` | None. | `boolean`. | Current simulated cellular flag. |
| `mobile.isLowPowerMode()` | None. | `boolean`. | Current simulated low-power flag; not a live native battery query. |
| `mobile.getNetworkType()` | None. | `"wifi"`, `"cellular"`, or `"offline"`. | Prefers `"wifi"` when both flags are enabled, then cellular, otherwise offline. |
| `mobile.getOrientation()` | None. | `"portrait"` or `"landscape"`. | Derived from current/emulated dimensions; a square uses the runtime's portrait fallback. |
| `mobile.isLandscape()` | None. | `boolean`. | Convenience test equivalent to `getOrientation() == "landscape"`. |
| `mobile.getDeviceSize()` | None. | Width, then height as numbers. | Returns oriented emulator size when enabled, otherwise current logical window dimensions. Values can change after resize/orientation events. |
| `mobile.getSafeAreaInsets()` | None. | `top, right, bottom, left` numeric insets. | Current model returns `47, 0, 34, 0` for portrait mobile and zeros otherwise; callers should still read all four rather than hard-code them. |

The network and low-power values are simulation state, not a live native
connectivity probe.

```luau
local width, height = mobile.getDeviceSize()
local top, right, bottom, left = mobile.getSafeAreaInsets()
hud.x, hud.y = left, top
hud.size_x, hud.size_y = width - left - right, height - top - bottom
```

<!-- page: commands | Commands API -->
# Commands API

Globals: `commands`, `command` (same table)

```luau
export type CommandRunResult = {
    ok: boolean,
    statusCode: number,
    status_code: number,
    stdout: string,
    stderr: string,
    error: string?,
}

export type CommandDetachedResult = {
    ok: boolean,
    pid: number,
    error: string?,
}

export type CommandsModule = {
    run: (command: string, args: { string }?, cwd: string?) -> CommandRunResult,
    runDetached: (command: string, args: { string }?, cwd: string?) -> CommandDetachedResult,
}
```

| Function | Parameters | Returns | Behavior, outputs, and edge cases |
| --- | --- | --- | --- |
| `commands.run(command, args?, cwd?)` | Executable name/path; optional array of literal argument strings; optional working directory. | `CommandRunResult`: `ok` is true only for exit code `0`; `statusCode`/`status_code` are the exit status; captured `stdout` and `stderr`; `error` is a launch/wait diagnostic or `nil`. | Waits synchronously. A nonzero child exit is a completed result with `ok = false` but normally no launch `error`. Launch failure/no exit code uses status `-1`. Empty command returns an error record. Output is decoded to strings by the platform process layer. |
| `commands.runDetached(command, args?, cwd?)` | Same executable/argument/cwd contract. | `CommandDetachedResult`: `ok`, spawned `pid` (`0` on failure), and optional `error`. | Returns after spawn with stdio disconnected and does not report the later exit status. Empty command returns an error record. The process can outlive the game. |

`cwd` defaults to the project root. Relative values resolve beneath it;
normalized values which escape the project raise an error. The command is
executed directly, not through a shell, so pass arguments as separate strings.

Web builds expose the same functions but always return `ok = false` with
`"commands are not available in web builds"`. Their shared unsupported record
also includes `statusCode = status_code = -1`, `pid = 0`, and empty
`stdout`/`stderr`, regardless of which function was called.

```luau
local result = commands.run("git", { "rev-parse", "--short", "HEAD" })
if result.ok then
    print("revision", result.stdout)
else
    print("git failed", result.statusCode, result.error or result.stderr)
end
```

<!-- page: http | HTTP API -->
# HTTP API

Global: `http`

```luau
export type HttpHeaders = { [string]: string }

export type HttpResponse = {
    ok: boolean,
    url: string,
    status: number?,
    body: string,
    error: string?,
    headers: HttpHeaders,
}

export type HttpRequestOptions = {
    url: string,
    method: string?,
    headers: HttpHeaders?,
    body: string?,
}

export type HttpModule = {
    request: ((url: string, callback: (response: HttpResponse) -> ()) -> number)
        & ((options: HttpRequestOptions, callback: (response: HttpResponse) -> ()) -> number)
        & ((url: string, options: HttpRequestOptions,
            callback: (response: HttpResponse) -> ()) -> number),
    get: ((url: string, callback: (response: HttpResponse) -> ()) -> number)
        & ((options: HttpRequestOptions, callback: (response: HttpResponse) -> ()) -> number)
        & ((url: string, options: HttpRequestOptions,
            callback: (response: HttpResponse) -> ()) -> number),
    _poll: () -> (),
}
```

```luau
http.request({
    url = "https://example.com/api",
    method = "POST",
    headers = { ["Content-Type"] = "application/json" },
    body = "{\"hello\":true}",
}, function(response)
    print(response.status, response.body)
end)
```

### `http.request(...) -> number` (`http.get` alias)

Accepted forms are `(url, callback)`, `(options, callback)`, and
`(url, options, callback)`. `url`/`options.url` is the HTTP(S) destination;
`options.method` defaults to `"GET"`; `headers` maps header names to string
values; `body` is the request bytes in a Luau string. The callback receives one
`HttpResponse`. In the three-argument form, the positional URL overrides
`options.url`, while the options method/headers/body still apply. Despite its
name, `get` is an exact alias and does not override an explicitly supplied
method.

The function returns a monotonically increasing request id immediately. The id
is for correlation only—there is no public cancellation/query operation.
Callbacks run at the start of a later frame, exactly once for work accepted by
the runtime. `response.ok` means no transport error, not a `2xx` status; inspect
`response.status`. HTTP error responses can therefore have `ok = true`, a
status such as `404`, body bytes, and `error = nil`. DNS/TLS/connect/fetch
failure can produce `ok = false`, `status = nil`, an empty/partial body, and an
`error`. Response header keys follow the underlying platform and should be
treated case-insensitively.

Missing/blank URLs, malformed options, unsupported schemes, or a non-function
callback raise during submission. Native builds support `http://` and
`https://` with bundled WebPKI roots. Web builds use browser `fetch`, obey CORS,
and may reject headers/methods prohibited by browser policy.

### `http._poll() -> ()`

Takes no parameters and returns nothing. It drains completed work and invokes
ready callbacks. The engine calls it automatically; gameplay calls can move
callbacks earlier in the frame and re-enter user code unexpectedly, so the
underscore marks it engine-managed rather than a normal update primitive.

```luau
local requestId = http.get("https://example.com/health", function(response)
    if not response.ok then
        print("transport failed", response.error)
    elseif response.status and response.status >= 200 and response.status < 300 then
        print("healthy", response.body)
    else
        print("HTTP status", response.status)
    end
end)
print("queued request", requestId)
```

<!-- page: servers | Servers API -->
# Servers API

Global: `servers`

NeoLOVE offers two layers:

- class-like in-process services with named, serialized events; and
- a low-level buffer transport which may run a separate Luau server script.

Both use HTTP/HTTPS transport and work on native desktop targets. The web
runtime exposes matching names which raise an unsupported-platform error.

## Module definition

```luau
export type ServerHostOptions = {
    host: string?,
    certPath: string?,
    keyPath: string?,
    cert_path: string?,
    key_path: string?,
}

export type ServersModule = {
    host: (scriptPath: string, port: number, options: ServerHostOptions?) -> HostedServerHandle,
    connect: (url: string) -> ServerClientHandle,
    define: (definition: { [string]: any }) -> ServerService,
    service: (definition: { [string]: any }) -> ServerService,
    createService: (definition: { [string]: any }) -> ServerService,
    create_service: (definition: { [string]: any }) -> ServerService,
    serializeTable: (value: any) -> buffer,
    serialize_table: (value: any) -> buffer,
    deserializeTable: (payload: buffer) -> any,
    deserialize_table: (payload: buffer) -> any,
    generateUuid4: () -> string,
    generate_uuid4: () -> string,
    generateUuid7: () -> string,
    generate_uuid7: () -> string,
    sha256: (value: string | buffer) -> string,
    sha128: (value: string | buffer) -> string,
    _poll: () -> (),
}
```

| Canonical function | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `servers.host(scriptPath, port, options?)` | Project-contained server script; numeric port; optional bind/TLS options. | `HostedServerHandle`. | Starts a separate low-level Luau server runtime. Port `0` asks the OS for a free port. Bind, script load/compile, project-boundary, and incomplete/invalid TLS configuration raise. Startup of the script can report asynchronously through server logging. |
| `servers.connect(url)` | Complete `http://` or `https://` server URL. | `ServerClientHandle`. | Creates a low-level client and starts connection work; the returned handle can exist before the connection succeeds. Bad URL/TLS/transport ultimately leaves it disconnected and gives `getKickReason()` a diagnostic where available. Unsupported web targets raise. |
| `servers.define(definition)` (`service`, `createService`, `create_service` aliases) | Mutable service-definition table. | The same table, decorated as `ServerService`. | Mutates in place by installing class-service methods/metadata. Re-defining a decorated table is idempotent. Non-table definitions raise. User keys can be callbacks or arbitrary service state; avoid replacing installed methods. |
| `servers.serializeTable(value)` (`serialize_table` alias) | Supported Luau table root. | MessagePack `buffer`. | Preserves supported nested values and table shape as described below. Unsupported values, non-table roots where the implementation requires a table, excessive/cyclic structure, and serialization failures raise. |
| `servers.deserializeTable(payload)` (`deserialize_table` alias) | MessagePack `buffer` created by the compatible serializer. | Reconstructed Luau table. | Malformed/truncated payloads, unsupported encoded values, or a non-table root raise; failure is never returned as `nil`. |
| `servers.generateUuid4()` (`generate_uuid4` alias) | None. | Lowercase standard random UUID string. | Produces a new v4 value per call; randomness is suitable for identifiers, not a secret/authentication proof. |
| `servers.generateUuid7()` (`generate_uuid7` alias) | None. | Lowercase standard time-ordered UUID string. | Encodes current time ordering plus randomness. Clock rollback/concurrency can affect strict textual ordering; uniqueness does not imply authorization. |
| `servers.sha256(value)` | String bytes or `buffer`. | 64-character lowercase hexadecimal digest. | Hashes exact bytes, including embedded NULs; it is not keyed and should not be used as a password hash or MAC. |
| `servers.sha128(value)` | String bytes or `buffer`. | 32-character lowercase hexadecimal digest. | Returns the first 128 bits of the SHA-256 digest. The truncation has less collision resistance than `sha256`. |
| `servers._poll()` | None. | `()` | Delivers pending host/client work and callbacks. Engine-managed; manual calls can alter ordering/re-enter callbacks. |

Serialization accepts table roots containing nils, booleans, integers,
numbers, UTF-8 strings, buffers, and nested tables. Consecutive 1-based keys
round-trip as arrays; other key/value pairs round-trip as maps. Functions,
threads, userdata, and cyclic tables raise errors.

`host` binds `127.0.0.1` by default. Use `{ host = "0.0.0.0" }` for LAN
access, then clients connect to the machine's actual address. TLS requires both
certificate and private-key paths; camel and snake spellings are accepted, and
both files must stay inside the project.

```luau
local packet = servers.serializeTable({ kind = "ready", players = 4 })
local decoded = servers.deserializeTable(packet)
assert(decoded.kind == "ready")
print(servers.generateUuid7(), servers.sha256("lobby:" .. decoded.players))
```

## Class service

```luau
export type ServerService = {
    name: string?,
    onStart: ((self: ServerService, host: HostedServerHandle) -> ())?,
    onConnect: ((self: ServerService, client: ServerPeer) -> ())?,
    onMessage: ((self: ServerService, client: ServerPeer, eventName: string, data: any) -> ())?,
    onDisconnect: ((self: ServerService, client: ServerPeer) -> ())?,
    host: (self: ServerService, port: number, options: ServerHostOptions?) -> HostedServerHandle,
    connect: (self: ServerService, url: string) -> ServerClientHandle,
    [string]: any,
}
```

```luau
local Chat = servers.define({
    onStart = function(self, host)
        self.hostHandle = host
    end,
    onConnect = function(self, client)
        client:emit("welcome", { id = client.key })
    end,
    onMessage = function(self, client, event, data)
        if event == "chat" then
            self.hostHandle:emit("chat", {
                from = client.key,
                text = data.text,
            })
        end
    end,
    onDisconnect = function(self, client)
    end,
})

local hosted = Chat:host(9000)
local client = Chat:connect(hosted.url)
client:on("welcome", function(data) print(data.id) end)
client:emit("chat", { text = "hello" })
```

`define` marks and mutates the provided table. Re-defining an already decorated
table returns it. Named event packets are ordinary serialized tables; an
unwrapped low-level payload arrives to a class service as event `"message"`
with the buffer as data.

### Service callbacks and methods

| Member | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `service:host(port, options?)` | Port and the same bind/TLS options as `servers.host`; `self` is implicit. | Class-enabled `HostedServerHandle`. | Starts the service without a separate user server script, attaches `service`, calls `onStart(self, host)` after setup, and routes named events. Callback errors are logged and do not become a return value from later polling. |
| `service:connect(url)` | Server URL. | Class-enabled `ServerClientHandle`. | Creates a low-level connection and enables `on`/`once`/`emit` event wrappers for this service protocol. Connection establishment remains asynchronous. |
| `onStart(self, host)` | Decorated service and new host. | Return values ignored. | Optional lifecycle callback, once per successful host invocation. |
| `onConnect(self, client)` | Service and connected `ServerPeer`. | Return values ignored. | Optional callback for a non-host peer becoming available. The peer may disconnect before later sends. |
| `onMessage(self, client, eventName, data)` | Service, sending peer, decoded event name, decoded data. | Return values ignored. | Optional callback per event. Decode/callback failures are reported; malformed payloads do not produce a valid user event. |
| `onDisconnect(self, client)` | Service and the disconnected peer snapshot. | Return values ignored. | Optional final notification. `client:isConnected()` is false by this point and sends fail. |

## Client handle

```luau
export type ServerClientHandle = {
    key: string,
    is_host: boolean,
    send: (payload: buffer) -> boolean,
    addCallback: (callback: (payload: buffer) -> ()) -> (),
    addcallback: (callback: (payload: buffer) -> ()) -> (),
    disconnect: () -> boolean,
    isConnected: () -> boolean,
    getKey: () -> string,
    isHost: () -> boolean,
    getKickReason: () -> string?,
    on: (self: ServerClientHandle, eventName: string,
        callback: (data: any, eventName: string, client: ServerClientHandle) -> ())
        -> ((data: any, eventName: string, client: ServerClientHandle) -> ()),
    once: (self: ServerClientHandle, eventName: string,
        callback: (data: any, eventName: string, client: ServerClientHandle) -> ())
        -> ((data: any, eventName: string, client: ServerClientHandle) -> ()),
    off: (self: ServerClientHandle, eventName: string, callback: (...any) -> ()) -> boolean,
    onAny: (self: ServerClientHandle,
        callback: (eventName: string, data: any, client: ServerClientHandle) -> ())
        -> ((eventName: string, data: any, client: ServerClientHandle) -> ()),
    emit: (self: ServerClientHandle, eventName: string, data: any) -> boolean,
    sendEvent: (self: ServerClientHandle, eventName: string, data: any) -> boolean,
}
```

The raw callbacks registered through `addCallback` receive buffers; class
listeners receive decoded data.

| Canonical operation | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `client.send(payload)` | Raw `buffer`; this low-level operation uses dot syntax. | `true` if queued for an active connection, otherwise `false`. | Success means queued, not acknowledged or delivered. A disconnected/closing connection returns `false`; serialization is the caller's responsibility. |
| `client.addCallback(callback)` (`addcallback` alias) | Function receiving one raw buffer. | `()` | Registers an additional persistent low-level callback. There is no removal handle; avoid adding one each frame. Callback errors are logged during polling. |
| `client.disconnect()` | None. | `true` if this call began disconnection; `false` if already disconnected/closing. | Idempotent. Queued messages are not guaranteed to flush. |
| `client.isConnected()` | None. | `boolean`. | Snapshot of current transport state. A newly returned client can be false until connection setup completes. |
| `client.getKey()` | None. | Stable connection-key string. | Matches public `key`; it remains readable after disconnect for correlation. |
| `client.isHost()` | None. | `boolean`. | Reports whether this is the service's internal host client; normal remote clients are false. |
| `client.getKickReason()` | None. | Server/closure diagnostic string, or `nil`. | `nil` means no reason has been recorded, not necessarily that the connection is healthy. |
| `client:on(eventName, callback)` | Event name and `(data, eventName, client)` callback; colon syntax. | The same callback function. | Registers persistently. Duplicate registration can invoke the function multiple times. Return it (or retain the original) for `off`. Available on class-enabled clients. |
| `client:once(eventName, callback)` | Same arguments as `on`. | The same callback. | Removes this registration before/when its first matching event is delivered. It may never run if disconnected first. |
| `client:off(eventName, callback)` | Exact event name and exact previously registered function. | `true` if a registration was removed; otherwise `false`. | Does not remove a different closure with identical code. |
| `client:onAny(callback)` | `(eventName, data, client)` callback. | The same callback. | Observes all decoded named events. Register sparingly; no event-name filter is applied. |
| `client:emit(eventName, data)` (`sendEvent` alias) | Event name and serializable data. | `true` if the encoded event was queued; otherwise `false`. | Encoding errors raise; transport loss returns `false`. It provides no delivery/remote-handler acknowledgement. |

```luau
local listener
listener = client:on("score", function(data)
    print("score", data.value)
    client:off("score", listener)
end)
if not client:emit("ready", { at = os.clock() }) then
    print("not connected")
end
```

## Host and peer handles

```luau
export type HostedServerHandle = {
    client: ServerClientHandle,
    port: number,
    url: string,
    service: ServerService?,
    stop: () -> boolean,
    getPort: () -> number,
    getUrl: () -> string,
    send: (self: HostedServerHandle, clientKey: string, payload: buffer) -> boolean,
    broadcast: (self: HostedServerHandle, payload: buffer) -> number,
    getClients: (self: HostedServerHandle) -> { string },
    getClientCount: (self: HostedServerHandle) -> number,
    emit: ((self: HostedServerHandle, eventName: string, data: any) -> number)?,
    sendEvent: ((self: HostedServerHandle, clientKey: string, eventName: string, data: any) -> boolean)?,
}

export type ServerPeer = {
    key: string,
    is_host: boolean,
    tags: { string },
    send: (self: ServerPeer, payload: buffer) -> boolean,
    emit: (self: ServerPeer, eventName: string, data: any) -> boolean,
    sendEvent: (self: ServerPeer, eventName: string, data: any) -> boolean,
    kick: (self: ServerPeer, reason: string?) -> (),
    isConnected: (self: ServerPeer) -> boolean,
}
```

| Host operation | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `host.stop()` | None. | `true` if an active host was stopped; otherwise `false`. | Idempotent. Disconnects clients and prevents new sends; queued work may be discarded. |
| `host.getPort()` | None. | Bound port integer. | For requested port `0`, this is the actual assigned port. Remains readable after stop. |
| `host.getUrl()` | None. | Connection URL string. | Matches `url`; wildcard binds may still require clients to replace the host portion with a reachable address. |
| `host:send(clientKey, payload)` | Target key and raw buffer. | `true` when queued to that connected peer; otherwise `false`. | Unknown, host-internal, or disconnected keys fail without acknowledgement. |
| `host:broadcast(payload)` | Raw buffer. | Number of peers for which queuing succeeded. | Excludes the internal host client. Partial success is represented by a count smaller than `getClientCount()`. |
| `host:getClients()` | None. | Array of connected non-host key strings. | Snapshot only; a key can disconnect immediately after return. Ordering should not be treated as stable. |
| `host:getClientCount()` | None. | Non-negative number of connected non-host peers. | Equivalent to the current snapshot count, excluding internal host. |
| `host:emit(eventName, data)` | Event name and serializable value. | Number of peers successfully queued. | Class-service handles only; nil/absent on raw hosts. Encoding errors raise. |
| `host:sendEvent(clientKey, eventName, data)` | Peer key, event name, serializable value. | `true` if queued; otherwise `false`. | Class-service handles only. Unknown/disconnected clients return false; encoding errors raise. |

| Peer operation | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `peer:send(payload)` | Raw buffer. | `boolean` queued status. | Success is not delivery acknowledgement; disconnected peers return false. |
| `peer:emit(eventName, data)` (`sendEvent` alias) | Event name and serializable data. | `boolean` queued status. | Uses the class event envelope; encoding errors raise. |
| `peer:kick(reason?)` | Optional reason sent to the peer. | `()` | Begins disconnection. Omitted reason uses the runtime default. Repeated kicks have no additional effect; delivery of the textual reason is best-effort. |
| `peer:isConnected()` | None. | `boolean`. | Snapshot of the peer transport state. |

`broadcast` and `getClients` exclude the internal host client. `emit` and
`sendEvent` are added only to class-service handles; those handles also receive
`service`, pointing to the decorated service definition.

## Separate server-script environment

A script started by `servers.host(path,...)` receives `server`, `fs`, `http`,
`https` (alias of `http`), `commands`, `cli` (alias of `commands`), and
project-relative `require`:

```luau
export type ServerScriptModule = {
    addCallback: (callback: (clientKey: string, payload: buffer) -> ()) -> (),
    addcallback: (callback: (clientKey: string, payload: buffer) -> ()) -> (),
    send: (clientKey: string, payload: buffer) -> (),
    kick: (clientKey: string, reason: string?) -> (),
    isHost: (clientKey: string) -> boolean,
    getClientTags: (clientKey: string) -> { string },
    getHostClientKey: () -> string,
    serializeTable: (value: any) -> buffer,
    serialize_table: (value: any) -> buffer,
    deserializeTable: (payload: buffer) -> any,
    deserialize_table: (payload: buffer) -> any,
    generateUuid4: () -> string,
    generate_uuid4: () -> string,
    generateUuid7: () -> string,
    generate_uuid7: () -> string,
    sha256: (value: string | buffer) -> string,
    sha128: (value: string | buffer) -> string,
}
```

Lifecycle connect/disconnect notifications are not sent to low-level script
callbacks; callbacks receive client key and payload messages.

The server-script functions use dot syntax. `server.addCallback(callback)`
(`addcallback`) registers a persistent `(clientKey, payload)` listener and
returns nothing. `server.send(clientKey, payload)` queues raw bytes and returns
nothing (invalid/disconnected keys are reported by the server runtime rather
than a boolean). `server.kick(clientKey, reason?)` begins removal and returns
nothing. `server.isHost(clientKey)` returns a boolean; unknown keys are false.
`server.getClientTags(clientKey)` returns a new array (empty for no/unknown
tags), and `server.getHostClientKey()` returns the internal host key string.
The serialization, UUID, and hash helpers have exactly the parameter, result,
and failure contracts documented for the `servers` module above.

```luau
-- server_main.luau, started by servers.host(...)
server.addCallback(function(clientKey, payload)
    local message = server.deserializeTable(payload)
    if message.kind == "ping" then
        server.send(clientKey, server.serializeTable({ kind = "pong" }))
    end
end)
```

<!-- page: shaders | Shader API -->
# Shader API

Global: `shaders`

```luau
export type ShaderLoadOptions = {
    uniforms: { string }?,
    images: { string }?,
    textures: { string }?,
    pipelines: { string }?,
    [string]: any,
}

export type ShadersModule = {
    DEFAULT_VERTEX_SHADER: string,
    load: (vertexPath: string, fragmentPath: string, options: ShaderLoadOptions?) -> ShaderHandle,
    loadFragment: (fragmentPath: string, options: ShaderLoadOptions?) -> ShaderHandle,
    load3DFragment: (fragmentPath: string, options: ShaderLoadOptions?) -> ShaderHandle,
    fromSource: (vertexSource: string, fragmentSource: string, options: ShaderLoadOptions?) -> ShaderHandle,
    fromFragmentSource: (fragmentSource: string, options: ShaderLoadOptions?) -> ShaderHandle,
    from3DFragmentSource: (fragmentSource: string, options: ShaderLoadOptions?) -> ShaderHandle,
    supports3D: () -> boolean,
    supports3DShaders: () -> boolean,
}

export type ShaderHandle = {
    setUniform1f: (self: ShaderHandle, name: string, x: number) -> (),
    setUniform2f: (self: ShaderHandle, name: string, x: number, y: number) -> (),
    setUniform3f: (self: ShaderHandle, name: string, x: number, y: number, z: number) -> (),
    setUniform4f: (self: ShaderHandle, name: string, x: number, y: number, z: number, w: number) -> (),
    setUniformColor: (self: ShaderHandle, name: string, color: Color4Value) -> (),
    setTexture: (self: ShaderHandle, name: string, image: ImageHandle) -> (),
}
```

| Function | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `shaders.load(vertexPath, fragmentPath, options?)` | Two project resource paths and optional compatibility options. | `ShaderHandle`. | Reads both sources immediately. The current renderer retains the vertex source for compatibility but uses its fixed projected-vertex stage; use a fragment constructor for portable 2D/3D shaders. Missing/escaping paths and UTF-8/I/O failures raise immediately. |
| `shaders.loadFragment(fragmentPath, options?)` | Fragment path and optional options. | `ShaderHandle`. | Reads the fragment and pairs it with `DEFAULT_VERTEX_SHADER`. Backend compilation is lazy at the first draw. |
| `shaders.load3DFragment(fragmentPath, options?)` | Fragment path and optional options. | `ShaderHandle`. | Explicit 3D-material alias of `loadFragment`; assign the result to `MeshRenderer3D.shader`. The same handle can also be used by a 2D drawable. |
| `shaders.fromSource(vertexSource, fragmentSource, options?)` | Complete GLSL source strings and optional options. | `ShaderHandle`. | Stores both strings without filesystem resolution. The fixed projected-vertex limitation above still applies. |
| `shaders.fromFragmentSource(fragmentSource, options?)` | Fragment GLSL and optional options. | `ShaderHandle`. | In-memory counterpart of `loadFragment`. |
| `shaders.from3DFragmentSource(fragmentSource, options?)` | Fragment GLSL and optional options. | `ShaderHandle`. | Explicit 3D-material alias of `fromFragmentSource`. |
| `shaders.supports3D()` / `supports3DShaders()` | None. | `boolean`. | `true` for WebGL exports and binaries compiled with the `vulkan` feature; `false` for software-only native binaries. A Vulkan initialization failure can still force a capable binary onto the unsupported software fallback. |

Shader handles are lightweight source/uniform objects. File reads happen in
the `load...` call, while GLSL parsing, compilation, linking, and pipeline-cache
creation happen lazily when a compatible renderer first draws the handle.
Consequently, syntax and driver errors are reported by that draw rather than by
the constructor. Compiled Vulkan/WebGL programs are cached by normalized source
until the relevant render resources are rebuilt.

The current runtime accepts `options` for compatibility but does not require
its `uniforms`, `images`/`textures`, or `pipelines` lists to create uniform
slots. Unknown option keys are ignored. `DEFAULT_VERTEX_SHADER` is a readable
string containing the projected-vertex contract; assigning a new value does
not rewrite existing handles or the renderer's fixed vertex stage.

Float/vector uniform storage is bounded to 16 distinct names and extra texture
storage to 4 distinct names. Replacing an existing name does not consume a new
slot. `setUniformColor` converts `Color4` byte channels to normalized floats.
`setTexture` requires a live uploaded image.

| Handle method | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `shader:setUniform1f(name, x)` | Uniform name and one number. | `()` | Stores/replaces one float value. An absent/optimized-out shader uniform is backend-dependent; exceeding 16 distinct float-uniform names raises. Non-finite values can produce shader-defined output. |
| `shader:setUniform2f(name, x, y)` | Name and two numbers. | `()` | Stores a two-component float vector under the name, replacing another float arity for that name. |
| `shader:setUniform3f(name, x, y, z)` | Name and three numbers. | `()` | Same rules, three components. |
| `shader:setUniform4f(name, x, y, z, w)` | Name and four numbers. | `()` | Same rules, four components. |
| `shader:setUniformColor(name, color)` | Name and `Color4Value`. | `()` | Converts byte channels to `0..1` floats and stores a four-component uniform. Invalid/missing color channels raise. |
| `shader:setTexture(name, image)` | Sampler name and live `ImageHandle`. | `()` | Stores/replaces one extra texture binding. The image is uploaded as needed by rendering; unloaded images raise. More than four distinct names raises. Extra named samplers are currently Vulkan-only; `Texture` itself comes from the drawable/mesh `texture` field. |

## Portable fragment contract

NeoLOVE supplies `uv` (`vec2`) and `color` (`vec4`) to the fragment stage.
For a 3D mesh, `color` already contains the mesh tint, vertex color, and current
diffuse-light result; `uv` is perspective-correct. Declare the base sampler as
`uniform sampler2D Texture;`. Float, `vec2`, `vec3`, and `vec4` uniforms must be
declared one per line so the Vulkan binding rewriter can assign deterministic
slots. `gl_FragCoord.z` is available for screen/depth effects, although its
numeric mapping is backend-specific.

The portable asset dialect uses `#version 450`, `texture2D`, and
`gl_FragColor`. Vulkan rewrites those compatibility forms into its descriptor
layout. Web exports strip the desktop version/precision directives, inject the
matching WebGL 1 varyings, and normalize `texture` calls to `texture2D`. Do not
redeclare `uv` or `color` in portable assets.

```glsl
#version 450

uniform sampler2D Texture;
uniform vec4 Tint;
uniform float Pulse;

void main() {
    vec4 base = texture2D(Texture, uv) * color;
    float rim = smoothstep(0.15, 0.85, 1.0 - gl_FragCoord.z);
    gl_FragColor = vec4(base.rgb * Tint.rgb * (1.0 + rim * Pulse), base.a * Tint.a);
}
```

```luau
local tintShader = shaders.loadFragment("shaders/tint.frag")
tintShader:setUniformColor("Tint", Color4(255, 120, 160))
tintShader:setUniform1f("Strength", 0.75)

local sprite = ecs.newEntity("Tinted", ecs.root, 100, 100)
local image = sprite:AddComponent(core.Sprite2D)
image.image = assets.loadImage("assets/character.png")
image.shader = tintShader
```

Custom shaders require a Vulkan-feature desktop build. WebAssembly supports
the same portable fragment contract for rectangles, primitive shapes, images,
and `MeshRenderer3D` through WebGL. The native software renderer reports an
actionable error instead of silently drawing a shader incorrectly.

<!-- page: lighting | 2D Lighting -->
# 2D Lighting

Global: `lighting`

NeoLOVE has an optional 2D lighting system. When enabled it builds a light map
from the scene's lights and shadow occluders each frame and composites it over
the rendered image: ambient light, colored point/spot/directional lights,
distance falloff, spot cones, ray-cast shadows with an optional soft penumbra,
ambient occlusion, bloom, and exposure.

Lighting is a per-scene toggle and is **off by default**, so existing projects
render unchanged until they opt in. When enabled with full white ambient at
intensity `1` and no lights, the scene still looks unlit; you create darkness by
lowering the ambient and letting lights cut through it.

::: info
Both rendering paths composite a **per-pixel** light map over the frame. The
**software renderer** (the default desktop path, plus WebAssembly and Android)
multiplies it on the CPU. The optional **Vulkan** desktop build uploads the same
light map as a texture and multiplies it over the scene in a final GPU pass, so
gradients, colored lights, soft shadows, and ambient occlusion match. The Vulkan
pass is a plain multiply, so **bloom and over-bright (> 1) light are not
represented** there. Positions are ordinary on-screen (logical) pixel
coordinates, matching draw commands.
:::

## Enabling and configuring lighting

```luau
lighting.setEnabled(true)
lighting.setAmbient(Color4(20, 24, 40), 0.35) -- cool, dim base light
lighting.setAmbientOcclusion(true, 32, 0.6)   -- contact shadows near occluders
lighting.setShadows(true, 3)                   -- soft-edged shadows
lighting.setBloom(0.4)
lighting.setQuality("high")
```

```luau
export type LightQuality = "low" | "medium" | "high" | "ultra"

export type LightingModule = {
    setEnabled: (enabled: boolean?) -> (),
    enable: () -> (),
    disable: () -> (),
    isEnabled: () -> boolean,
    setAmbient: (color: Color4Value, intensity: number?) -> (),
    setAmbientIntensity: (intensity: number) -> (),
    getAmbient: () -> (Color4Value, number),
    setAmbientOcclusion: (enabled: boolean?, radius: number?, intensity: number?, samples: number?) -> (),
    setShadows: (enabled: boolean?, softness: number?) -> (),
    setBloom: (amount: number) -> (),
    setExposure: (value: number) -> (),
    setQuality: (quality: LightQuality) -> (),
    getQuality: () -> LightQuality,
    sample: (x: number, y: number) -> Color4Value?,
    getAt: (x: number, y: number) -> Color4Value?,
    sampleAt: (x: number, y: number) -> Color4Value?,
    reset: () -> (),
}
```

## `lighting` functions

| Canonical function | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `lighting.setEnabled(enabled?)` | Optional boolean, default `true`. | `()` | Turns light-map generation/compositing on or off. Disabling does not discard other settings, so re-enabling restores them. |
| `lighting.enable()` | None. | `()` | Convenience call equivalent to `setEnabled(true)`. |
| `lighting.disable()` | None. | `()` | Equivalent to `setEnabled(false)`. The already completed frame remains available to internal history, but sampling while disabled returns white. |
| `lighting.isEnabled()` | None. | `boolean`. | Returns the current global toggle immediately. |
| `lighting.setAmbient(color, intensity?)` | Ambient `Color4Value`; optional numeric intensity. | `()` | Copies/clamps the color. When intensity is present it is clamped to `>= 0`; omitting it preserves the previous intensity. Non-finite values are normalized/rejected according to numeric validation rather than allowed to poison the light map. |
| `lighting.setAmbientIntensity(intensity)` | Numeric ambient multiplier. | `()` | Changes only intensity, clamped to non-negative. Values above `1` intentionally permit over-bright light in the software path. |
| `lighting.getAmbient()` | None. | New ambient `Color4Value`, then numeric intensity. | Mutating the returned color does not change settings; call `setAmbient` to write it back. |
| `lighting.setAmbientOcclusion(enabled?, radius?, intensity?, samples?)` | Optional toggle (default `true` when supplied alone), pixel radius, strength, and sample count. | `()` | Only supplied numeric fields change. Radius is non-negative, intensity clamps to `0..1`, and samples to integer `1..64`. AO has no visible effect without occluders. |
| `lighting.setShadows(enabled?, softness?)` | Optional toggle and optional penumbra size in pixels. | `()` | Omitted toggle means enabled. Softness clamps to non-negative; `0` is hard-edged. Lights with `castsShadows = false` ignore occluders, and per-light non-negative softness overrides this global value. |
| `lighting.setBloom(amount)` | Numeric bloom strength. | `()` | Clamps to non-negative. Bloom only adds energy where computed light exceeds full brightness and is not represented by the Vulkan multiply-only composite. |
| `lighting.setExposure(value)` | Numeric output multiplier. | `()` | Clamps to non-negative. `0` produces black while lighting is enabled; it does not alter stored ambient/light intensities. |
| `lighting.setQuality(quality)` | `"low"`, `"medium"`, `"high"`, or `"ultra"`. | `()` | Low uses one light texel per 4×4 output pixels, medium 2×2, and high always 1×1. Ultra adds higher-detail sampling but adapts map resolution to roughly one million texels: 1280×720 remains 1×1, 1080p/1440p use 2×2, and 4K uses 3×3. Linear upscaling and independent shadow/AO fields keep the result smooth while preventing display resolution from multiplying CPU work without bound. Unknown names raise rather than silently selecting a quality. A resize/rebuild takes effect on the next rendered frame. |
| `lighting.getQuality()` | None. | One `LightQuality` string. | Returns the normalized current setting. |
| `lighting.sample(x, y)` (`getAt`, `sampleAt` aliases) | Logical screen-space pixel coordinates. | Opaque `Color4Value`, or `nil` when outside the completed frame. | Reads the **last completed frame**, so it is stable during `update`. When disabled, any on-screen point returns opaque white. Fractional points sample the containing light-map pixel; camera-aware gameplay should convert world coordinates to screen coordinates before sampling. Before a completed frame or outside its bounds, returns `nil`. |
| `lighting.reset()` | None. | `()` | Restores all defaults: disabled, white full ambient, default AO/shadow/bloom/exposure/quality settings. Components are not removed or edited. |

```luau
-- Dim an enemy that is standing in shadow.
local here = lighting.sample(enemy.x, enemy.y)
if here and here.r + here.g + here.b < 120 then
    enemy:AddComponent(core.Rect2D).color = Color4(60, 60, 70)
end
```

Settings persist across frames. `Light2D` and `LightOccluder2D` components,
by contrast, contribute their light and shadow every frame while active.

## `Light2D`

`core.Light2D` emits light. Its position comes from the entity transform, and
for spot and directional lights the aim follows the entity rotation plus
`angleOffset`.

| Field | Default | Meaning |
| --- | --- | --- |
| `kind` | `"point"` | `"point"`, `"spot"`, or `"directional"`. |
| `color` | `Color4(255,255,255)` | Light color. |
| `intensity` | `1.0` | Brightness multiplier. |
| `radius` | `256` | Reach in pixels for point/spot lights. |
| `falloff` | `2.0` | Distance-attenuation exponent (1 linear, 2 quadratic). |
| `angleOffset` | `0` | Degrees added to the entity rotation for the aim. |
| `coneAngle` | `60` | Full cone width in degrees for spot lights. |
| `coneSoftness` | `0.35` | `0` is a hard cone edge, `1` fades across the whole cone. |
| `castsShadows` | `true` | Whether occluders shadow this light. |
| `shadowSoftness` | `-1` | Per-light penumbra in pixels. Negative uses the global `lighting.setShadows` softness. |
| `visible` | `true` | When `false`, the light is skipped. |

```luau
local torch = ecs.newEntity("torch", ecs.root, 400, 300)
local light = torch:AddComponent(core.Light2D)
light.color = Color4(255, 170, 90)
light.radius = 220
light.intensity = 1.4
```

## `LightOccluder2D`

`core.LightOccluder2D` marks an entity as a shadow caster. It uses the entity's
bounds (position, size, and rotation) as an occluder that blocks lights and
contributes to ambient occlusion. It works on any entity, including one drawn
with a `Sprite2D`, `Shape2D`, or `Spritebox2D`.

| Field | Default | Meaning |
| --- | --- | --- |
| `shape` | `"box"` | `"box"` uses the rotated rectangle; `"circle"` treats the bounds as an ellipse. |
| `visible` | `true` | When `false`, the entity stops casting. |

```luau
local wall = ecs.newEntity("wall", ecs.root, 500, 250)
wall.size_x = 24
wall.size_y = 160
wall:AddComponent(core.Rect2D)     -- something to see
wall:AddComponent(core.LightOccluder2D) -- and something to cast shadows
```

## In the visual editor

Lighting is also a per-scene toggle in the editor. With nothing selected, the
Inspector's **Scene** panel has a **Lighting** section: enable it, set the
ambient color and intensity, ambient occlusion, shadows and softness, bloom,
exposure, and quality. These settings are saved in the `.neoscene` document and
exported to `main.luau` as `lighting.*` calls, so a Run matches the editor.

The viewport previews lighting by tinting each object by the light reaching its
center — a live, per-object approximation rather than a flat veil. The full
per-pixel light map (smooth gradients across a single sprite, in-shadow
softening) is produced by the runtime; the editor preview is a guide to placement
and mood. **Editor Settings → Preview scene lighting** controls this preview and
defaults to on. Turning it off can make a very dense scene easier to edit; it
does not change, disable, or save over the scene's runtime lighting settings.

## Performance notes

The light pass avoids repeating expensive geometry work at every output pixel:

- a BVH rejects most occluders during exact ray tests;
- screen tiles contain compact lists of only the lights that can affect them;
- each shadow-casting light builds a compact byte visibility field once, then
  all light-map texels bilinearly reuse it;
- soft-shadow blur runs on that small visibility field, not on the full RGB
  light map;
- ambient occlusion is sampled on a low-frequency byte field near occluders and
  interpolated across the map;
- light-map rows, shadow fields, AO rows, CPU compositing, and GPU upload
  encoding use bounded worker bands; and
- an unchanged light/config/occluder snapshot reuses its cached map.

The maintained release benchmark uses 1920×1080 Ultra, 16 moving lights, 40
occluders, 4px soft shadows, and 12-sample AO. On the development host, the
combined map-build result fell from about **179.7 ms/frame** before these
optimizations to about **10.9 ms/frame**. Run the same workload on a target
machine with:

```sh
cargo test --release benchmark_ultra_dynamic_light_maps -- --ignored --nocapture
```

That figure measures light-map construction, not the rest of a game's frame,
and hardware/scene layout still matter. To tune further, lower `setQuality`,
reduce light radius, disable `castsShadows` on fill lights, and avoid unnecessary
shadow-casting directional lights because each covers the whole screen. Fewer
AO samples or a smaller radius also lowers field construction cost.

<!-- page: rng | Random Numbers -->
# Random Numbers

Global: `Rng`

`math.random` shares one hidden global stream, which makes reproducible runs —
procedural generation, replays, deterministic tests — awkward. `Rng` hands out
independent, seedable generators instead. Each is a fast xoshiro256** stream.

```luau
local rng = Rng.new(1234)     -- seeded and reproducible
local noise = Rng.new()       -- entropy-seeded
local level = Rng.fromString("level-3") -- stable seed from a name

print(rng:integer(1, 6))      -- a dice roll, 1..6 inclusive
print(rng:number())           -- a float in [0, 1)
print(rng:number(10, 20))     -- a float in [10, 20)
rng:shuffle(deck)             -- in-place Fisher-Yates
local card = rng:pick(deck)   -- a random element
```

`Rng(seed)` is shorthand for `Rng.new(seed)`.

```luau
export type RngModule = {
    new: (seed: number?) -> RngInstance,
    fromString: (text: string) -> RngInstance,
} & ((seed: number?) -> RngInstance)
```

## Module functions

| Canonical function | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `Rng.new(seed?)` (`Rng(seed?)` callable shorthand) | Optional integer-compatible numeric seed. | New independent `RngInstance`. | With a seed, identical integer values produce identical streams. Without one, wall-clock entropy plus a process counter makes adjacent instances best-effort distinct. Numeric seeds are converted to the runtime's 64-bit integer representation; this is deterministic randomness, not cryptographic randomness. |
| `Rng.fromString(text)` | Arbitrary UTF-8 string. | New independently seeded `RngInstance`. | Hashes the exact string bytes with a stable built-in hash, so the same text reproduces across runs. Case, whitespace, and normalization differences produce different seeds. Hash collisions are possible. |

## Instance methods

| Canonical method | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `rng:next()` | None. | Float in `[0, 1)`. | Advances the stream once with 53 bits of floating-point resolution. |
| `rng:number(min?, max?)` (`float`, `range` aliases) | No bounds; one `max`; or `min, max`. | With no bounds `[0,1)`; one bound scales from zero toward `max`; two bounds interpolate from `min` toward but not including `max`. | Bounds are not reordered: reversed two-bound input travels downward from `min` toward `max`. Equal bounds always return that bound. NaN/infinite bounds propagate non-finite arithmetic and should be avoided. |
| `rng:integer(min, max?)` (`int` alias) | One integer `max`, or inclusive `min, max`. | Integer uniformly selected from the inclusive range. | One argument means `[1,max]`. Two bounds are order-insensitive. Equal bounds return that value. The runtime also accepts no arguments and returns a raw signed 64-bit stream value, but this compatibility behavior is intentionally absent from the typed API. |
| `rng:boolean(p?)` (`bool` alias) | Optional probability, default `0.5`. | `boolean`. | Advances once and compares with `p`: `p <= 0` is always false and `p >= 1` always true; NaN is false. Values are not otherwise clamped. |
| `rng:sign()` | None. | Exactly `-1` or `1`. | Each outcome is equally likely and advances the stream. |
| `rng:angle()` | None. | Radians in `[0, 2π)`. | Suitable for `math.cos`/`math.sin`; `2π` itself is excluded. |
| `rng:unit()` | None. | Unit-vector components `x, y`. | Samples an angle then returns cosine and sine. Floating-point rounding can make length differ microscopically from one. |
| `rng:pick(list)` | Array-like table. | One array element, or `nil` when raw length is zero. | Uses indices `1..#list`. Sparse holes can therefore return `nil`, which is indistinguishable from an empty list or an element intentionally containing nil. |
| `rng:shuffle(list)` | Mutable array-like table. | The exact same table. | Fisher–Yates shuffles indices `1..#list` in place. Sparse/non-array keys are not rearranged. Empty and one-item arrays are returned unchanged. |
| `rng:seed(seed)` | Integer-compatible numeric seed. | `()` | Replaces the instance state; the next result matches a fresh generator with the same seed. Clones remain independent and are not reseeded. |
| `rng:clone()` (`Clone` alias) | None. | New independent `RngInstance` at the same stream position. | The clone initially produces the same future sequence; advancing/reseeding either one does not affect the other. |

Two generators created with the same seed produce identical sequences, which is
what makes seeded worlds and deterministic tests reproducible.

```luau
local layout = Rng.fromString("world:desert:3")
local replay = layout:clone()
local x, y = layout:unit()
assert(replay:unit() == x) -- first return value is identical

local rewards = { "coin", "gem", "key" }
layout:shuffle(rewards)
print(layout:pick(rewards))
```

<!-- page: ecs | ECS API -->
# ECS API

Global: `ecs`

```luau
export type EcsModule = {
    addSystem: (system: System) -> (),
    newEntity: (name: string, parent: Entity?, x: number?, y: number?) -> Entity,
    deleteEntity: (entity: Entity) -> (),
    duplicateEntity: (targetEntity: Entity, parent: Entity) -> Entity,
    findFirstChild: (parent: Entity, name: string) -> Entity?,
    findByTag: (tag: string) -> { Entity },
    findByLayer: (layer: number) -> { Entity },
    root: Entity,
    addComponent: (entity: Entity, component: Component) -> ComponentInstance,
    removeComponent: (entity: Entity, target: number | ComponentInstance) -> boolean,
    loadScene: (path: string) -> (),
}
```

## Function behavior

`ecs.root` is the id-`0` root entity. Its `size_x`/`size_y` track the logical
window. Do not delete it or overwrite its engine-managed identity/collections.

| Function | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `ecs.newEntity(name, parent?, x?, y?)` | Name; optional parent; optional local coordinates defaulting to `0, 0`. | New attached/registered `Entity`. | A `nil` parent creates an unparented entity, not a root child. Names need not be unique. Parent must be a live entity; invalid arguments raise. Components are initially empty and size defaults are listed below. |
| `ecs.deleteEntity(entity)` | Live non-root entity. | `()` | Recursively unregisters descendants, detaches from its parent, and disconnects listeners. The entity table may still be referenced by Luau but is no longer live. Re-deleting/stale/root entities is invalid or a no-op according to registry state; component destroy callbacks are not guaranteed, as warned below. |
| `ecs.duplicateEntity(targetEntity, parent)` | Source entity and explicit destination parent. | Root of a fresh deep-copy subtree. | Uses prefab capture/instantiate semantics, remapping internal references and queueing custom `awake`. A source/destination that is stale or would violate hierarchy constraints raises. |
| `ecs.findFirstChild(parent, name)` | Parent and exact, case-sensitive direct-child name. | First matching `Entity`, or `nil`. | Does not recurse and makes no uniqueness guarantee. Current child-array order decides among duplicates. |
| `ecs.findByTag(tag)` (`FindByTag` alias) | Non-empty tag after trimming. | New id-sorted entity array. | Matches enabled `Tag` components by exact trimmed text. Disabled tags and empty queries do not match. |
| `ecs.findByLayer(layer)` (`FindByLayer` alias) | Integer logical layer. | New id-sorted entity array. | Matches enabled `Layer` components. It is gameplay metadata, not a render or collision filter. |
| `ecs.addComponent(entity, component)` | Live entity and component prototype table. | New `ComponentInstance`. | Deep-copies the prototype, attaches instance helpers/entity, runs core setup immediately, and queues custom `awake`. Prototype functions/tables follow the engine copy rules. Invalid core/custom definitions raise without returning a partial instance. |
| `ecs.removeComponent(entity, target)` | Live owner plus 1-based component index or exact attached instance. | `true` if removed; `false` if absent/invalid index. | Runs `destroy` or fallback `onDestroy`, removes it, and clears `instance.entity`. Callback errors can propagate/report while removal is underway. |
| `ecs.addSystem(system)` | Mutable system table. | `()` | Calls optional `system:awake()` synchronously, then registers it for per-frame `update`. If `awake` errors, registration does not complete normally. Adding the same table twice creates duplicate scheduling unless user code prevents it. |
| `ecs.loadScene(path)` | Project `.neoscene` path. | `()` | Reads and executes the generated scene representation, replacing all non-root entities/listeners only after entering the load workflow. Missing/invalid files, parsing, or generated Luau errors raise with path context. This is a destructive scene replacement; retain persistent data outside scene entities. |

`loadScene` preserves `ecs.root` but replaces its children. If parsing or
generated Luau execution fails, it raises a path-rich error.

```luau
local world = ecs.newEntity("World", ecs.root)
local player = ecs.newEntity("Player", world, 64, 96)
local body = player:AddComponent(core.Rigidbody2D)

local copy = ecs.duplicateEntity(player, world)
copy.name, copy.x = "Player ghost", 160
assert(world:FindFirstChild("Player") == player)
```

## Entity definition

```luau
export type PositionPivot = "center" | "top_right"

export type Entity = {
    id: number,
    name: string,
    x: number,
    y: number,
    anchor_x: number,
    anchor_y: number,
    anchorX: number?,
    anchorY: number?,
    pivot_x: number?,
    pivot_y: number?,
    pivotX: number?,
    pivotY: number?,
    position_pivot_x: number?,
    position_pivot_y: number?,
    positionPivotX: number?,
    positionPivotY: number?,
    rotation: number,
    rotation_pivot: string,
    rotation_pivot_x: number?,
    rotation_pivot_y: number?,
    rotationPivot: string?,
    rotationPivotX: number?,
    rotationPivotY: number?,
    rotation_pivot_middle: boolean?,
    position_pivot: PositionPivot | "topright" | string?,
    positionPivot: PositionPivot | "topright" | string?,
    z: number,
    size_x: number,
    size_y: number,
    scale: number,
    raycastable: boolean?,
    parent: Entity?,
    children: { Entity },
    components: { ComponentInstance },

    listen: (self: Entity, event: EntityListenEvent | string,
        callback: (entity: Entity, event: EntityListenInfo) -> ()) -> Connection,
    Listen: (self: Entity, event: EntityListenEvent | string,
        callback: (entity: Entity, event: EntityListenInfo) -> ()) -> Connection,
    delete: (self: Entity) -> (),
    Delete: (self: Entity) -> (),
    addComponent: (self: Entity, component: Component) -> ComponentInstance,
    AddComponent: (self: Entity, component: Component) -> ComponentInstance,
    removeComponent: (self: Entity, target: number | ComponentInstance) -> boolean,
    RemoveComponent: (self: Entity, target: number | ComponentInstance) -> boolean,
    duplicate: (self: Entity, parent: Entity?) -> Entity,
    Duplicate: (self: Entity, parent: Entity?) -> Entity,
    findFirstChild: (self: Entity, name: string) -> Entity?,
    FindFirstChild: (self: Entity, name: string) -> Entity?,
    hasTag: (self: Entity, tag: string) -> boolean,
    HasTag: (self: Entity, tag: string) -> boolean,
    isInLayer: (self: Entity, layer: number) -> boolean,
    IsInLayer: (self: Entity, layer: number) -> boolean,
    getWorldPosition: (self: Entity) -> (number, number),
    GetWorldPosition: (self: Entity) -> (number, number),
    getWorldRotation: (self: Entity) -> number,
    GetWorldRotation: (self: Entity) -> number,
    isInside: (self: Entity, worldX: number, worldY: number) -> boolean,
    IsInside: (self: Entity, worldX: number, worldY: number) -> boolean,
    [string]: any,
}
```

### Entity field defaults and semantics

| Field | New-entity default | Meaning |
| --- | --- | --- |
| `id` | allocated; root is `0` | Stable runtime identity. Do not modify. |
| `name` | constructor argument | User label. Names need not be unique. |
| `x`, `y` | supplied or `0` | Local position relative to parent transform and anchors. |
| `anchor_x`, `anchor_y` | `0` | Fractions of parent bounds added to local position. |
| `pivot_x`, `pivot_y` | `nil` | Numeric position pivot fractions; override `position_pivot`. |
| `rotation` | `0` | Local rotation in radians. |
| `rotation_pivot` | `topleft` | `middle`/`center` rotates around center; numeric fields override. |
| `rotation_pivot_x`, `rotation_pivot_y` | `nil` | Numeric rotation-pivot fractions. |
| `position_pivot` | `nil` | `center` or `top_right`; default means top-left. |
| `z` | `0` | Draw and front-query order. Parent z is not added. |
| `size_x`, `size_y` | `32`, `32` | Local unscaled bounds. Editor-created entities default to `100`, `100`. |
| `scale` | `1` | Uniform local scale inherited by descendants. |
| `raycastable` | `nil` | Only explicit `false` excludes `transform.raycast`. |
| `parent` | constructor argument or `nil` | Parent table. Runtime-created unparented entities are allowed. |
| `children` | `{}` | Direct child array, managed by ECS operations. |
| `components` | `{}` | 1-based attached component instances. |

Entities are ordinary extensible Luau tables, so game code may attach any
additional key/value (`entity.health = 100`, `entity.inventory = {}`). The
visual editor's **Attached Values** section serializes the supported authored
types into those same fields. ECS duplication and prefab instantiation
deep-copy ordinary tables and remap internal entity/component references.
Engine-managed fields and methods are not protected from assignment; replacing
them can invalidate hierarchy, rendering, or lifecycle behavior.

Transform reads also accept camel aliases `anchorX/Y`, `pivotX/Y`,
`positionPivot`, `positionPivotX/Y`, and `rotationPivot/X/Y`. The older
`position_pivot_x/y` and boolean `rotation_pivot_middle` are accepted too.
Snake case is canonical. Named `topright` aliases `top_right`; unknown position
pivots fall back to top-left.

`Duplicate()` without a parent uses the current parent, falling back to
`ecs.root`. `IsInside` tests transformed bounds including hierarchy scale,
rotation, anchors, and pivots; boundary points count as inside.

### Entity method reference

| Canonical method | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `entity:listen(event, callback)` (`Listen` alias) | Event name and `(entity, eventInfo)` function. | Live `Connection`. | Registers one pointer listener on this entity. Valid canonical event names and payloads are in Entity Listeners. Unknown event strings raise rather than silently inventing events. Deleting the entity disconnects it. |
| `entity:delete()` (`Delete` alias) | None. | `()` | Same recursive operation and caveats as `ecs.deleteEntity(entity)`. The table should be treated as stale after the call. |
| `entity:addComponent(prototype)` (`AddComponent` alias) | Component prototype. | New attached `ComponentInstance`. | Equivalent to `ecs.addComponent(entity, prototype)`. Always configures the copy, never the shared `core` prototype. |
| `entity:removeComponent(target)` (`RemoveComponent` alias) | 1-based index or exact component instance. | Removal `boolean`. | Equivalent to the ECS function. Indexes are evaluated against the current list, so earlier removals shift later indexes. |
| `entity:duplicate(parent?)` (`Duplicate` alias) | Optional destination parent. | New copied `Entity`. | Defaults to the current parent, then `ecs.root` when unparented. The new entity is independent but internal references within its copied subtree are remapped consistently. |
| `entity:findFirstChild(name)` (`FindFirstChild` alias) | Exact direct-child name. | Matching entity or `nil`. | Equivalent to `ecs.findFirstChild(self, name)`; not recursive. |
| `entity:hasTag(tag)` (`HasTag` alias) | Non-empty tag after trimming. | `boolean`. | Tests enabled dimension-independent `Tag` components on this entity. Comparison is exact and case-sensitive after trimming both values. |
| `entity:isInLayer(layer)` (`IsInLayer` alias) | Integer logical layer. | `boolean`. | Tests enabled dimension-independent `Layer` components on this entity. It does not inspect `Collider2D/3D.layer` or `RenderLayer3D.mask`. |
| `entity:getWorldPosition()` (`GetWorldPosition` alias) | None. | World top-left `x, y`. | Resolves anchors, pivots, parent scale/rotation, and hierarchy. Values are computed from current mutable fields; a cyclic/corrupt parent chain is invalid. |
| `entity:getWorldRotation()` (`GetWorldRotation` alias) | None. | World rotation in radians. | Sums local rotations through ancestors. It does not normalize to `0..2π`. |
| `entity:isInside(worldX, worldY)` (`IsInside` alias) | World-space point. | `boolean`. | Tests transformed bounds including rotation and scale; edges count as inside. Non-positive global size has no interior. Sprite transparency/masks are ignored—use `Spritebox2D:IsInside` for its pixel mask. |

## Component definition and lifecycle

```luau
export type Component = {
    name: string?,
    __neolove_component: string?,
    awake: ((entity: Entity, component: ComponentInstance) -> ())?,
    update: ((entity: Entity, component: ComponentInstance, dt: number) -> ())?,
    destroy: ((entity: Entity, component: ComponentInstance) -> ())?,
    onDestroy: ((entity: Entity, component: ComponentInstance) -> ())?,
    NEOLOVE_RENDERING: boolean?,
    [string]: any,
}

export type ComponentInstance = Component & {
    entity: Entity?,
    remove: (self: ComponentInstance) -> boolean,
    Remove: (self: ComponentInstance) -> boolean,
    getEntity: (self: ComponentInstance) -> Entity?,
    GetEntity: (self: ComponentInstance) -> Entity?,
    [string]: any,
}
```

Custom `awake` is deferred until all scene/prefab Inspector assignments exist.
Core setup runs immediately during attachment. `update` runs once per frame.
Removal calls `destroy`; if absent it calls `onDestroy`. It then clears
`component.entity`. Removing an already detached instance returns `false`.

| Component-instance method | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `component:remove()` (`Remove` alias) | None. | `true` if still attached and removed; otherwise `false`. | Delegates to the owning entity. During/after removal, `entity` becomes nil. A destroy callback should not recursively assume the same component remains in the list. |
| `component:getEntity()` (`GetEntity` alias) | None. | Owning `Entity`, or `nil`. | Returns nil after removal. The entity can still become stale after a later entity/scene deletion, so this is attachment state rather than an eternal liveness guarantee. |

::: warning
Current `deleteEntity` and scene replacement remove entity registries directly;
they do not run every attached component's `destroy`/`onDestroy`. Use
`removeComponent` for cleanup which must execute before deleting an entity.
:::

## Systems

```luau
export type System = {
    awake: ((self: System) -> ())?,
    update: ((self: System, dt: number) -> ())?,
    lateUpdate: ((self: System, dt: number) -> ())?,
    fixedUpdate: ((self: System, dt: number) -> ())?,
    [string]: any,
}
```

`awake(self)` receives the system itself, returns nothing meaningful, and runs
synchronously inside `addSystem`. `update(self, dt)` receives the same table and
elapsed frame seconds; its return values are ignored and it runs once per
engine frame in registration order. Errors propagate through the runtime's
frame error reporting and prevent the rest of that callback invocation from
finishing. As noted in Runtime Model, `lateUpdate(self, dt)` and
`fixedUpdate(self, dt)` are declared compatibility hooks but are not currently
scheduled, so assigning them alone has no effect.

<!-- page: transforms | Transform and Query API -->
# Transform and Query API

Globals: `transform`, `transforms` (same table)

```luau
export type RaycastHit = {
    entity: Entity,
    id: number,
    distance: number,
    x: number,
    y: number,
    normalX: number,
    normalY: number,
    normal_x: number,
    normal_y: number,
}

export type RaycastOptions = {
    ignore: Entity | { Entity }?,
    ignoreEntity: Entity | { Entity }?,
}

export type TransformModule = {
    getWorldPosition: (entity: Entity) -> (number, number),
    getWorldRotation: (entity: Entity) -> number,
    lookAt: (fromX: number, fromY: number, toX: number, toY: number) -> number,
    look_at: (fromX: number, fromY: number, toX: number, toY: number) -> number,
    GetEntitiesInFront: (worldX: number, worldY: number, minimumZ: number?) -> { Entity },
    getEntitiesInFront: (worldX: number, worldY: number, minimumZ: number?) -> { Entity },
    doTheyOverlap: (entities: { Entity }) -> boolean,
    raycast: (originX: number, originY: number, dirX: number, dirY: number,
        maxDistance: number?, options: RaycastOptions?) -> RaycastHit?,
}
```

| Canonical function | Parameters | Returns | Semantics and edge cases |
| --- | --- | --- | --- |
| `transform.getWorldPosition(entity)` | Entity to resolve. | World-space top-left `x, y`. | Applies the entity's pivot/anchor plus ancestor scale/rotation and current mutable fields. Stale/malformed entities or cyclic hierarchy data raise/fail rather than yielding a meaningful transform. |
| `transform.getWorldRotation(entity)` | Entity to resolve. | Rotation in radians. | Adds rotations through the parent chain without normalizing. Scale/pivots do not change this scalar result. |
| `transform.lookAt(fromX, fromY, toX, toY)` (`look_at` alias) | World-space start and target coordinates. | Facing angle in radians. | Uses `atan2(toY-fromY, toX-fromX)`: zero faces positive X and positive angles turn toward positive Y. Coincident points return the platform's defined zero-angle result. Non-finite input yields non-finite/platform math and should be avoided. |
| `transform.GetEntitiesInFront(worldX, worldY, minimumZ?)` (`getEntitiesInFront` alias) | World-space query point and optional inclusive minimum z. | New array of matching entities. | Tests all live non-root transformed bounds, sorted descending z then descending id (frontmost first). Omitted minimum accepts all z. Boundaries count; non-rendering/invisible entities can still match because this is a geometry query. |
| `transform.doTheyOverlap(entities)` | Array of entities (2D, 3D, or a mix). | `true` if any pair's world volumes intersect, else `false`. | Fewer than two valid entries returns false. Each entity resolves to one world-space oriented box using its own anchors, position/rotation pivots and the full rotation and scale of its ancestors, and pairs are compared with a separating-axis test, so rotated rectangles no longer report broad-phase false positives. An entity takes its 3D box from an enabled `Collider3D` or `Trigger3D` (box, sphere and capsule report their bounding box; `mesh` uses the mesh bounds), otherwise an enabled `MeshRenderer3D` (its generated mesh, or the authored primitive fields before the first frame builds one), otherwise an explicit `size_z` on the entity, which pairs with `size_x`/`size_y` as a centred volume. Everything else is measured as its 2D rectangle: two of them ignore depth, and one compared against a 3D box behaves like the flat quad it is, sitting in its accumulated `position_z` plane. Touching edges count as separated. Spritebox masks and component visibility are ignored. Duplicate references can overlap themselves according to the pair traversal and should be removed by callers. |
| `transform.raycast(originX, originY, dirX, dirY, maxDistance?, options?)` | Origin; direction (need not be normalized); optional distance; optional ignore record. | Nearest `RaycastHit`, or `nil`. | Normalizes direction, intersects entity AABBs, and returns the closest allowed hit including distance, point, and normal aliases. A zero/non-finite direction returns nil. Distance defaults to infinity, clamps to `0..1,000,000`, and negative becomes zero. Both ignore fields accept one entity or an array and are combined. Root, explicit `raycastable = false`, non-positive global bounds, and ignored entities are skipped. |

Normal aliases in `RaycastHit` carry identical numbers. A hit at the origin can
have distance `0`. When two candidates tie, stable engine entity ordering
decides which one is returned; do not use ties as an identity rule.

```luau
local angle = transform.lookAt(player.x, player.y, mouse.x, mouse.y)
player.rotation = angle

local hit = transform.raycast(player.x, player.y, math.cos(angle), math.sin(angle), 500, {
    ignore = player,
})
if hit then print("hit", hit.entity.name, hit.distance, hit.normalX, hit.normalY) end
```

<!-- page: listeners | Entity Listeners -->
# Entity Listeners

```luau
export type EntityListenEvent = "leftClick" | "rightClick" | "middleClick"
    | "scrollUp" | "scrollDown" | "mouseEntered" | "mouseExited"

export type EntityListenInfo = {
    kind: EntityListenEvent,
    type: EntityListenEvent,
    button: "left" | "right" | "middle"?,
    x: number,
    y: number,
    mouseX: number,
    mouseY: number,
    localX: number,
    localY: number,
    local_x: number,
    local_y: number,
    wheelX: number,
    wheelY: number,
    amount: number,
}

export type Connection = {
    Disconnect: (self: Connection) -> boolean,
    disconnect: (self: Connection) -> boolean,
    IsConnected: (self: Connection) -> boolean,
    isConnected: (self: Connection) -> boolean,
}
```

```luau
local connection = button:Listen("leftClick", function(entity, event)
    print(event.localX, event.localY)
end)
connection:Disconnect()
```

Click events fire on the relevant press inside transformed bounds. Enter/exit
track hover transitions. Scroll events target entities under the pointer.
`x/y` and `mouseX/mouseY` are world cursor coordinates; local aliases are
entity-local; wheel fields are signed raw deltas. `amount` is positive only for
the matching scroll direction and zero for all other events. `button` is `nil`
for hover/scroll.

Disconnect is idempotent and returns whether it removed a live registration.
Deleting an entity recursively disconnects its registrations.

| Connection method | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `connection:Disconnect()` (`disconnect` alias) | None. | `true` if a live registration was removed; otherwise `false`. | Idempotent. A callback may disconnect itself safely; it will not run on later events. Deleting its entity has the same terminal effect. |
| `connection:IsConnected()` (`isConnected` alias) | None. | `boolean`. | Reports whether the registration is currently eligible for future events. It becomes false immediately after disconnection/entity deletion, including while another callback is being dispatched. |

<!-- page: prefabs | Prefab API -->
# Prefab API

Globals: `prefabs`, `prefab` (same table)

```luau
export type PrefabTemplate = {
    name: string?,
    x: number?, y: number?, z: number?,
    anchor_x: number?, anchor_y: number?,
    pivot_x: number?, pivot_y: number?,
    rotation: number?,
    rotation_pivot: string?,
    rotation_pivot_x: number?, rotation_pivot_y: number?,
    position_pivot: PositionPivot?,
    size_x: number?, size_y: number?, scale: number?,
    parent: PrefabTemplate?,
    children: { PrefabTemplate }?,
    components: { Component }?,
    [string]: any,
}

export type PrefabUiModule = {
    label: PrefabTemplate,
    panel: PrefabTemplate,
    dialog: PrefabTemplate,
    statusChip: PrefabTemplate,
    status_chip: PrefabTemplate,
}

export type PrefabsModule = {
    capture: (entity: Entity) -> PrefabTemplate,
    component: <T>(source: T & Component, overrides: { [string]: any }?) -> T & Component,
    load: (path: string) -> PrefabTemplate,
    register: (name: string, source: string | Entity | PrefabTemplate) -> PrefabTemplate,
    get: (name: string) -> PrefabTemplate?,
    remove: (name: string) -> boolean,
    instantiate: (source: string | Entity | PrefabTemplate, parent: Entity?) -> Entity,
    duplicate: (source: string | Entity | PrefabTemplate, parent: Entity?) -> Entity,
    ui: PrefabUiModule,
}
```

| Canonical function | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `prefabs.capture(entity)` | Live source entity. | Detached `PrefabTemplate` snapshot. | Deep-captures the entity subtree, components, shared/cyclic table structure, and references whose targets are inside the subtree. Later source edits do not change the snapshot. Unsupported userdata/functions follow capture rules and invalid/stale entities raise. |
| `prefabs.component(source, overrides?)` | Component prototype/instance-like table and optional keyed override table. | Fresh component table of the source's generic type. | Deep-copies source then assigns override keys. Nested override values replace, rather than recursively merge, the copied key. It does not attach the result; use it in a prefab or `AddComponent`. |
| `prefabs.load(path)` | `.neoprefab` project/data/resource path. | Parsed detached `PrefabTemplate`. | Does not instantiate/register. Missing files, invalid JSON/document shape, path violations, or script/component resolution failures raise. |
| `prefabs.register(name, source)` | Exact registry name and a path, live entity, or template. | Captured/loaded stored `PrefabTemplate`. | Replaces an existing exact-name registration. The registry owns a safe snapshot rather than retaining a live entity. Empty/invalid names or invalid sources raise. |
| `prefabs.get(name)` | Exact registry name. | Registered `PrefabTemplate`, or `nil`. | Returns the stored template table; treat it as source data. Mutating it can affect later instantiations, so clone/register a separate value when isolation matters. |
| `prefabs.remove(name)` | Exact registry name. | `true` if removed; otherwise `false`. | Does not delete existing instances or invalidate template tables already held by user code. |
| `prefabs.instantiate(source, parent?)` (`duplicate` alias) | Registry name/path resolution as supported, live entity, or template; optional parent default `ecs.root`. | New entity-subtree root. | Creates a fresh hierarchy, remaps internal entity/component references, then queues custom `awake` after fields exist. Unknown registered names/invalid paths/templates/parents raise. Every invocation gets independent ordinary fields while intentionally shared references inside that one snapshot stay shared. |

Instantiation remaps entity/component references within each copy and preserves
shared table identity, cycles, and metatables. It builds the complete tree
before calling custom `awake`, in parent-to-descendant and component-list order.
Prefab-authored values survive core initialization. Script paths in editor
prefabs stay project-relative.

`prefabs.ui` provides immutable source templates for a label, panel, dialog,
and status chip. Instantiate or register/capture before customization. The
module also exposes engine-managed `_registry`; do not mutate it directly.

```luau
local enemyTemplate = prefabs.capture(enemy)
prefabs.register("enemy", enemyTemplate)

for i = 1, 3 do
    local copy = prefabs.instantiate("enemy", ecs.root)
    copy.x = 100 + i * 80
end

local redPanel = prefabs.component(core.Panel, {
    backgroundColor = Color4(120, 24, 32),
})
```

<!-- page: tweening | Tweening API -->
# Tweening API

Globals: `tweening`, `tween` (same table)

```luau
export type EasingStyle = "linear" | "sine" | "quad" | "cubic" | "quart"
    | "quint" | "expo" | "circ" | "back" | "bounce"
export type EasingDirection = "in" | "out" | "inOut" | "in_out"

export type TweenHandle = {
    id: number,
    cancel: (self: TweenHandle) -> boolean,
    Cancel: (self: TweenHandle) -> boolean,
    isDone: (self: TweenHandle) -> boolean,
    IsDone: (self: TweenHandle) -> boolean,
}

export type TweeningModule = {
    to: (target: { [any]: any }, key: any, value: number, duration: number,
        style: EasingStyle?, direction: EasingDirection?, onComplete: (() -> ())?) -> TweenHandle,
    new: (target: { [any]: any }, key: any, value: number, duration: number,
        style: EasingStyle?, direction: EasingDirection?, onComplete: (() -> ())?) -> TweenHandle,
    create: (target: { [any]: any }, key: any, value: number, duration: number,
        style: EasingStyle?, direction: EasingDirection?, onComplete: (() -> ())?) -> TweenHandle,
    cancelAll: () -> number,
    cancel_all: () -> number,
    count: () -> number,
    ease: (t: number, style: EasingStyle?, direction: EasingDirection?) -> number,
    update: (dt: number) -> (),
    _update: (dt: number) -> (),
}
```

### Module functions

| Canonical function | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `tweening.to(target, key, value, duration, style?, direction?, onComplete?)` (`new`, `create` aliases) | Mutable target table; exact key; numeric destination; finite non-negative seconds; optional easing style/direction; optional zero-argument completion callback. | New active `TweenHandle`. | Reads the target's numeric starting value immediately. Progress is clamped to `0..1`; zero duration reaches the destination on the next positive update. Completion writes the exact destination, invokes the callback once, and releases registry references. Missing/non-numeric start/destination, invalid duration/easing, or non-function callback raises without a useful handle. Concurrent tweens for the same key both write in update order; cancel the old one explicitly. |
| `tweening.cancelAll()` (`cancel_all` alias) | None. | Number of live tweens newly cancelled. | Already completed/cancelled entries are not counted. Completion callbacks do not run for cancellation. |
| `tweening.count()` | None. | Number of currently live tween entries. | Terminal tweens are excluded after registry cleanup even if handles remain referenced. |
| `tweening.ease(t, style?, direction?)` | Numeric progress and optional easing names. | Eased numeric progress. | Evaluates without creating state. Input progress is clamped to `0..1`; defaults are `linear` and `out`; unknown names raise. Some styles such as back/bounce can overshoot within their mathematical curve even though input is clamped. |
| `tweening.update(dt)` (`_update` alias) | Elapsed seconds. | `()` | Advances all live tweens. The engine calls `_update` once per frame. Manual use adds another advance; negative/non-finite `dt` is invalid or clamped by runtime validation and should not be supplied. |

Accepted style aliases are `sin`, `quadratic`, `quartic`, `quintic`,
`exponential`, and `circular`. Direction parsing ignores `_` and `-`, so
`inOut`, `in_out`, and `in-out` are equivalent. Unknown names raise.

`cancelAll`/`cancel_all` return the number newly cancelled. `count` counts live
tweens. `ease` evaluates without creating a tween. `update` and `_update` are
the same function; the engine calls `_update` automatically, so gameplay should
not call either unless it deliberately wants an extra advance.

### Handle methods

| Canonical method | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `handle:cancel()` (`Cancel` alias) | None. | `true` if a live tween was cancelled; otherwise `false`. | Idempotent. Leaves the target at its current interpolated value and suppresses `onComplete`. |
| `handle:isDone()` (`IsDone` alias) | None. | `boolean`. | True after normal completion or cancellation. Use your own flag inside `onComplete` if those outcomes must be distinguished. |

```luau
local fade = tweening.to(panel, "opacity", 0, 0.25, "sine", "out", function()
    panel.entity:delete()
end)

-- Later, if the panel must stay:
if not fade:isDone() then fade:cancel() end
```

<!-- page: animation | Animation API -->
# Animation API

Globals: `animation`, `animations` (same table)

```luau
export type AnimationKeyframe = {
    time: number,
    value: number,
    out_x: number?, out_y: number?,
    in_x: number?, in_y: number?,
}

export type AnimationTrack = {
    property: string,
    interpolation: "linear" | "step" | "hold" | "bezier"?,
    keys: { AnimationKeyframe },
}

export type AnimationClip = {
    duration: number?,
    looping: boolean?,
    looped: boolean?,
    tracks: { AnimationTrack },
}

export type AnimationHandle = {
    id: number,
    play: (self: AnimationHandle) -> (),
    pause: (self: AnimationHandle) -> (),
    stop: (self: AnimationHandle) -> (),
    seek: (self: AnimationHandle, time: number) -> (),
    setSpeed: (self: AnimationHandle, speed: number) -> (),
    isPlaying: (self: AnimationHandle) -> boolean,
}

export type AnimationModule = {
    load: (path: string) -> AnimationClip,
    Load: (path: string) -> AnimationClip,
    new: (target: { [any]: any }, clip: AnimationClip) -> AnimationHandle,
    create: (target: { [any]: any }, clip: AnimationClip) -> AnimationHandle,
    play: (target: { [any]: any }, clip: AnimationClip) -> AnimationHandle,
    update: (dt: number) -> (),
    _update: (dt: number) -> (),
}
```

### Module functions

| Canonical function | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `animation.load(path)` (`Load` alias) | Project/data/resource `.neoanim` path. | Parsed mutable `AnimationClip`. | Loads JSON without creating a player. Missing/invalid files, malformed tracks/keys, nonnumeric required fields, and path failures raise. `duration` defaults to the latest key time and is raised to at least that time if supplied shorter. Looping defaults false; `looped` aliases `looping`. |
| `animation.new(target, clip)` (`create` alias) | Mutable target table and clip table. | Paused `AnimationHandle`. | Validates/prepares tracks and registers a player at time zero. Target properties are not advanced until play/manual update. Invalid clip/target fields raise. The player retains references while registered. |
| `animation.play(target, clip)` | Mutable target and clip. | Playing `AnimationHandle`. | Convenience constructor equivalent to creating then calling `:play()`. Initial key values are applied according to player sampling/update timing. |
| `animation.update(dt)` (`_update` alias) | Elapsed seconds. | `()` | Advances registered players once. Engine-managed under `_update`; manual calls double-advance. Non-finite/negative `dt` should not be supplied. |

Tracks write numeric target properties by exact string key. Keys are sampled in
time order. `step` and `hold` retain the earlier value. `cubic` and `ease` alias
`bezier`. Bezier x handles clamp to `0..1`; defaults are outgoing `(0.333, 0)`
and incoming `(0.667, 1)`. Linear is the track default.

### Handle methods

| Method | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `player:play()` | None. | `()` | Sets playback active. A finished non-looping player can be played again according to its current/finished state; use `stop` or `seek(0)` when an explicit rewind is required. |
| `player:pause()` | None. | `()` | Stops time advancement while preserving current time and target values. Repeated calls are harmless. |
| `player:stop()` | None. | `()` | Pauses, rewinds to time zero, clears finished state, and applies/schedules the initial sample according to runtime player behavior. It does not unregister the handle. |
| `player:seek(time)` | Desired seconds. | `()` | Clamps to `0..clip.duration`, clears finished state, and samples all tracks at that time. Non-finite time is invalid. Seeking does not itself choose playing versus paused. |
| `player:setSpeed(speed)` | Finite non-negative multiplier. | `()` | `0` freezes advancement while preserving `isPlaying`; `1` is normal. Negative/non-finite values raise. Reverse playback is not supported. |
| `player:isPlaying()` | None. | `boolean`. | True only while active; paused/stopped/finished non-looping players return false. Looping players remain active until paused/stopped. |

Tracks write numeric target properties by exact string key. Keys are sampled in
time order. `step` and `hold` retain the earlier value. `cubic` and `ease` alias
`bezier`. Bezier x handles clamp to `0..1`; defaults are outgoing `(0.333, 0)`
and incoming `(0.667, 1)`. Linear is the track default. Players remain
registered after finishing and can be played again.

```luau
local clip = animation.load("animations/open.neoanim")
local player = animation.play(door, clip)

-- Scrubbing does not require a second clip or target.
player:pause()
player:seek((clip.duration or 0) * 0.5)
player:setSpeed(1.5)
player:play()
```

## `core.AnimationController`

The component wrapper is documented with the other non-rendering components.
It creates a player for `animation`, honors `autoplay`, overrides clip looping
with its own `looping`, and forwards its `speed` each frame.

<!-- page: core | Core Components -->
# Core Components

`core` contains component prototype tables. Attach a prototype with
`entity:AddComponent(core.Name)`; do not mutate the shared prototype when you
intend to configure only one entity.

```luau
export type CoreModule = {
    Rect2D: Rect2D,
    Light2D: Light2D,
    LightOccluder2D: LightOccluder2D,
    EntityScaler: EntityScaler,
    Camera: Camera,
    Tag: Tag,
    Layer: Layer,
    Shape2D: Shape2D,
    ParticleSystem2D: ParticleSystem2D,
    AnimationController: AnimationController,
    SpatialSound2D: SpatialSound2D,
    AudioSource3D: AudioSource3D,
    AudioListener3D: AudioListener3D,
    TextBox: TextBox,
    TextLabel: TextBox,
    RudimentaryTextLabel: TextBox,
    TextInput: TextInput,
    Panel: Panel,
    Frame: Panel,
    Button: Button,
    Slider: Slider,
    Dropdown: Dropdown,
    Sprite2D: Sprite2D,
    Image2D: Sprite2D,
    SpriteSheet2D: SpriteSheet2D,
    NineSliceSprite2D: NineSliceSprite2D,
    ["9SliceSprite2D"]: NineSliceSprite2D,
    TileTexture2D: TileTexture2D,
    Tilemap2D: Tilemap2D,
    Spritebox2D: Spritebox2D,
    Collider2D: Collider2D,
    Rigidbody2D: Rigidbody2D,
    Bolt2D: Bolt2D,
    LegacyBolt2D: LegacyBolt2D,
    Rope2D: Rope2D,
    String2D: Rope2D,
}
```

## Shared drawable fields

```luau
export type BaseDrawableComponent = ComponentInstance & {
    NEOLOVE_RENDERING: boolean,
    color: Color4Value,
    shader: ShaderHandle?,
    visible: boolean,
}
```

`color` tints the output and defaults opaque white. `visible = false` skips the
component. `shader` applies a custom shader where the renderer supports it.
Drawable output is also skipped for non-positive bounds, nil/unloaded images,
or fully transparent final colors.

## `core.Tag` and `core.Layer`

These components are dimension-independent metadata and are offered in both 2D
and 3D editor scenes. `Tag.tag` defaults to `"Untagged"`; logical `Layer.layer`
defaults to `0` and its optional display `name` defaults to `"Default"`.
Disabling either component removes it from entity/ECS queries. Neither changes
rendering, visibility, collision filters, or physics behavior. Use
`RenderLayer3D` for camera filtering and collider `layer`/`mask` fields for
physics filtering. The older `Tag3D`, `Layer3D`, and `*3D` query spellings are
compatibility aliases only.

::: details Engine-managed component fields
Core prototypes and instances contain lifecycle functions such as `awake` and
`update`, plus `__neolove_core_component` and `__neolove_component` tags. Some
components also keep `__...` caches, timers, particle arrays, or player handles.
They are visible because components are Luau tables, but gameplay must not call,
replace, serialize independently, or depend on those fields.
:::

<!-- page: rendering-3d | 3D Rendering, Animation, and Particles -->
# 3D Rendering, Animation, and Particles

3D entities reuse `x` and `y`, add `position_z`, and use XYZ Euler degrees in
`rotation_x`, `rotation_y`, and `rotation_z`. Per-axis scale is stored in
`scale_x`, `scale_y`, and `scale_z`. These fields are independent from the
legacy 2D `rotation`, uniform `scale`, and draw-order `z` fields.

## Cameras, lights, and mesh renderers

`core.Camera3D` supports perspective and orthographic projection, `fov`,
`orthographic_size`, near/far clipping, and a 31-bit `render_mask`. The first enabled camera is the
fallback; `camera:SetActive()` selects another camera explicitly. The editor
draws a camera-body, lens, viewfinder, and frustum proxy for camera entities.

`Visibility3D` supplies hierarchy-aware visual visibility. With
`inherit_parent = true`, a hidden ancestor suppresses this entity; setting it
false creates an explicit boundary. `RenderLayer3D.mask` is tested against the
active camera mask and passes when any bit overlaps. Meshes, lights,
environments, and particle drawing use the same policy in Scene View and the
runtime. Scripts and physics keep running, and hidden particle systems keep
simulating. The editor-only Render Layers and Entity Visibility diagnostics
show the resolved decision without mutating authored properties.

`core.Light3D` supports `point`, `spot`, and `directional` lights. Intensity,
range, spot angle/softness, color, and Euler-authored direction are evaluated
by the direct Cook-Torrance PBR lighting path.

On native Vulkan, the first enabled shadow-casting directional light is
preferred as the frame's shadow source; if none exists, the first
shadow-casting spot light is used. The renderer reuses a 2048×2048 depth image,
renders eligible native meshes (including GPU-skinned and camera-offscreen
casters), and applies bounded 3×3 PCF while evaluating that light. These
authoring fields are live:

```luau
light.casts_shadows = true
light.shadow_bias = 0.0005
renderer.casts_shadows = true
renderer.receives_shadows = true
```

Bias is a normalized light-depth offset and clamps to `0..0.1`. A frame without
an active shadow source does not redraw the map after its one-time layout
initialization. Point-light shadow cubemaps, multiple simultaneous shadow
lights, directional cascades, alpha-tested caster silhouettes, configurable
quality, and software/Web shadow parity are not yet implemented.

`core.MeshRenderer3D` accepts a `MeshHandle`, `mesh_path`, base texture, tint,
custom mesh shader, reusable `material`/`materials` overrides, and
`double_sided` rendering. An imported `mesh_path` takes
precedence over its generic primitive controls. glTF/GLB base-color images are
resolved from external relative URIs, embedded data URIs, and GLB buffer views,
then selected per material/submesh automatically. An explicit `texture` handle
overrides those imported base images for the complete mesh:

```luau
local model = ecs.newEntity("Model", ecs.root, 0, 0)
local renderer = model:AddComponent(core.MeshRenderer3D)
renderer.primitive = "sphere" -- cube, sphere, plane, cylinder, capsule, cone
renderer.primitive_radius = 0.75
renderer.primitive_segments = 32
renderer.primitive_rings = 16
```

New visual-editor mesh components default to a cube. A component created
directly with `AddComponent` defaults to `primitive = "none"` so the established
`renderer.mesh = assets.loadMesh(...)` script pattern remains an explicit
manual mesh and is not replaced on its first update.

Scripts can obtain the same cached geometry with
`assets.primitiveMesh(kind, options)`. Options include uniform/axis sizes,
radius, height, segments, and rings. Sizes must be positive; segments are
bounded to `3..1024`, rings to `1..512`, spheres require at least two rings,
and capsule height must be at least twice its radius.

## Custom 3D material shaders

Assign any fragment `ShaderHandle` to `MeshRenderer3D.shader`. The explicit
`load3DFragment` and `from3DFragmentSource` names are aliases of the shared
fragment constructors, so materials and 2D drawables use one uniform API and
pipeline cache:

```luau
local model = ecs.newEntity("Shader model", ecs.root, 0, 0)
local renderer = model:AddComponent(core.MeshRenderer3D)
renderer.mesh = assets.loadMesh("assets/model.glb")
renderer.texture = assets.loadImage("assets/model_albedo.png")

if shaders.supports3DShaders() then
    local material = shaders.load3DFragment("shaders/rim.frag")
    material:setUniformColor("Tint", Color4(160, 210, 255))
    material:setUniform1f("Pulse", 0.35)
    renderer.shader = material
else
    print("This native build uses the software-only shader fallback")
end

app.setAntiAliasing("high")
```

The sibling [`3d-shaders-aa`](../samples/3d-shaders-aa) sample is a runnable
version with a procedural material, capability fallback, live uniforms, and
keys `1`/`2`/`3` for the three AA modes.

When `texture` and an imported base-color image are both absent, the built-in
`Texture` sampler receives opaque white. Vulkan and the ordinary Web/software
path select imported base images per submesh. The custom WebGL shader bridge
currently uses only the explicit component texture.
Mesh UVs are perspective-correct and the supplied `color` includes current CPU
diffuse lighting, so a fragment shader can preserve it by multiplying its base
sample by `color`. See [Portable fragment contract](#portable-fragment-contract)
for the complete input, declaration, uniform-limit, and GLSL rules.

| Backend | 3D shader behavior |
| --- | --- |
| Vulkan-feature desktop | Fragment programs compile lazily, use the existing depth-tested mesh pipeline, and inherit the selected device MSAA sample count. |
| Web export | Portable fragments are normalized for WebGL 1. Projected depth is tested within each mesh command; separate WebGL mesh commands are composited in submission order and do not share depth with software-rendered meshes. |
| Native software-only/fallback | Unshaded meshes render normally, including software 3D antialiasing. A mesh with `shader` raises an actionable renderer error because arbitrary GLSL is not interpreted on the CPU. |

The current programmable stage is fragment-only. Mesh projection and lighting
are prepared before submission, and stored custom vertex sources do not replace
that stage. Use entity transforms, mesh editing, skinning, or particles to
change geometry; use the shader for material/color/texture/depth effects.

## 3D antialiasing

`app.setAntiAliasing("off" | "standard" | "high")` applies to meshes,
3D particles, and custom mesh shaders as well as existing 2D drawing. Vulkan
uses supported multisampling, ordinary web meshes use the software 3D path,
and custom web shaders select an antialiased/supersampled WebGL surface. The
software 3D filter uses luminance and depth discontinuities after opaque meshes
and transparent particle billboards, then the normal 2D stream draws on top;
it therefore cannot soften UI overlays. See the backend quality table under
[Anti-aliasing](#anti-aliasing).

## Imported and live-edited meshes

`assets.loadMesh` supports OBJ, glTF 2.0 (`.gltf`), binary glTF (`.glb`), and
ASCII/binary FBX geometry. `assets.newMesh(vertices, indices?)` creates a mesh
from one-based Luau tables. Mesh handles are shared, revisioned assets:

```luau
local mesh = assets.loadMesh("assets/terrain.glb")
mesh:setPosition(1, -1, 0.5, 0, true)
mesh:setMaterialColor(1, Color4(255, 180, 120))
mesh:setMaterialPbr(1, 0.2, 0.65)
mesh:setMaterialTexture(1, "base_color", assets.loadImage("assets/albedo.png"))
mesh:setMaterialTexture(1, "normal", assets.loadImage("assets/normal.png"))
mesh:setMaterialTexture(1, "metallic_roughness", assets.loadImage("assets/orm.png"))
mesh:setMaterialEmissive(1, Color4(20, 5, 0))
mesh:setMaterialAlpha(1, "mask", 0.5)
renderer.mesh = mesh
```

`getVertex`, `setVertex`, `getIndex`, `setIndex`, `replaceGeometry`, and
`recomputeNormals` update geometry atomically. A failed validation leaves the
previous snapshot intact. Mesh colliders and every renderer using the identity
observe successful revisions. Image handles are revisioned too, so editing the
assigned texture in a script is visible without replacing it.

For glTF/GLB assets, base-color, metallic/roughness, normal, and emissive image
dependencies are decoded once per glTF image and retained by the mesh handle.
OBJ `mtllib`/`usemtl` imports common MTL factors and maps; ASCII and binary FBX
imports common factors, external texture/video links, and ByPolygon/AllSame
material slots. Path-based import is required to resolve external files. The
default Vulkan and software/ordinary Web paths evaluate these bindings with
tangent-space normal mapping and direct-light metallic/roughness PBR shading.
`setMaterialTexture` accepts a live `ImageHandle`, a source-metadata string, or
`nil`; strings do not perform file I/O, so use `assets.loadImage(...)` for a
rendered runtime map. Custom shader paths remain author-controlled, and custom
WebGL currently receives only the explicit component texture.

## Reusable 3D materials

`Material3DHandle` separates appearance from geometry. Bind one handle to
`renderer.material` for slot 1, or populate `renderer.materials` by one-based
submesh material slot. A missing slot falls back to the material imported with
the mesh. Successful setters commit an atomic revision that all bound
renderers observe on the following frame:

```luau
local material = assets.newMaterial3D({
    name = "Painted metal",
    color = Color4(80, 130, 210),
    metallic = 0.75,
    roughness = 0.28,
    normal_texture = assets.loadImage("assets/paint-normal.png"),
    metallic_roughness_texture = assets.loadImage("assets/paint-orm.png"),
})
renderer.mesh = assets.primitiveMesh("sphere")
renderer.material = material

material:setColor(Color4(210, 70, 45))
material:setPbr(0.65, 0.4)
material:setTexture("emissive", assets.loadImage("assets/emission.png"))
material:setEmissive(Color4(30, 8, 2))
```

`setAlpha`, `setDoubleSided`, `revision`, `identity`, and `get` complete the
live API. `assets.saveMaterial3D(material, path)` writes version 1 JSON with a
`.neomaterial` extension. `assets.loadMaterial3D(path)` caches the handle and
loads texture sources relative to the material file; `loadMaterial`,
`saveMaterial`, and `newMaterial` are concise aliases. A runtime-only image has
no durable source, so export it and bind its path before saving. The scene
editor's MeshRenderer3D inspector includes a Material asset picker and exports
the selected handle assignment. In a 3D project, **Project → New 3D Material**
creates the same versioned contract, and double-clicking a `.neomaterial` opens
its dedicated PBR editor. The editor exposes base RGBA, metallic, roughness,
emissive RGB, alpha mode/cutoff, double-sided state, and base/normal/ORM/
emissive texture sources with UV-set indices. Slot labels make the runtime's
sRGB (base/emissive) versus linear (normal/ORM) interpretation explicit. Its
sphere preview is rendered by the real software PBR rasterizer after passing
through the runtime material loader; the last valid preview remains visible if
an in-progress edit becomes invalid. The same validation blocks a save and
reports corrupt JSON, unsupported values, or the resolved missing texture path.

## Armatures and imported animation

glTF/GLB imports one flattened skin per asset, joint weights, inverse-bind
matrices, and LINEAR/STEP translation, quaternion-rotation, and scale clips.
ASCII FBX supports a practical single-skinned-geometry subset built from
Model/Skin/Cluster objects and XYZ animation curves.

`MeshHandle` exposes `animationNames`, `animationDuration`, `jointCount`,
`sampleAnimation`, `playAnimation`, `updateAnimation`, `pauseAnimation`, and
`stopAnimation`. Cached handles share a pose; call `cloneDetached()` before
manually animating several instances independently.

The `MeshRenderer3D` fields `animation`, `animation_autoplay`,
`animation_looping`, `animation_playing`, and `animation_speed` provide
component-managed playback. Setting `animation` causes the component to take
an independent detached pose automatically. `PlayAnimation(clip?)`,
`PauseAnimation()`, and `StopAnimation()` have lowercase aliases.
Component-managed `animation_speed` must be finite and non-negative; reverse
initialization is not implemented. The lower-level `MeshHandle` methods remain
available when a script needs complete manual sampling control.

```luau
renderer.mesh = assets.loadMesh("assets/character.glb")
renderer.animation = "Walk"
renderer.animation_looping = true
renderer:PlayAnimation()
```

Pose sampling publishes an immutable CPU-deformed snapshot for bounds and
fallback renderers. The default Vulkan path additionally uploads bind-pose
joint/weight attributes once and applies palettes of up to 256 joints in its
vertex shader. Detached poses share the persistent bind/index buffers, so
animation revisions update uniforms rather than re-uploading geometry. Custom
mesh shaders, software/Web rendering, larger armatures, and skinned meshes whose
vertices were edited after import use the CPU snapshot. Multi-skin flattened
assets, morph targets, glTF CUBICSPLINE, compressed/sparse accessors, and
binary-FBX armature/animation data are not yet supported. Use ASCII FBX 7.x or
glTF 2.0 when animation is required.

## Environment and skybox control

`core.Environment3D` (`core.Skybox3D` alias) renders before depth-tested
geometry. Its modes are `solid`, `gradient`, `equirectangular`, and `cubemap`.
A panorama or cubemap follows camera rotation but ignores camera translation.
The component also owns runtime fog through `fog_enabled`, `fog_mode`,
`fog_color`, `fog_start`, `fog_end`, and `fog_density`. Linear fog uses the
authored start/end distances; `exponential` and `exponential_squared` use
density. All distances are measured from the active camera in world units.
Real-time 3D ambient occlusion is authored with `ao_enabled`, `ao_radius`,
`ao_intensity`, and `ao_bias`. Radius and bias are world-unit values; intensity
is clamped to 0–1.

Scripts without an environment entity can use the identical global aliases
`environment3d`, `environment3D`, and `skybox`:

```luau
skybox.setGradient(Color4(30, 47, 78), Color4(8, 10, 16))
skybox.setEquirectangular(assets.loadImage("assets/panorama.png"), 20)
skybox.setIntensity(1.2)
skybox.setFog(Color4(110, 125, 145), {
    mode = "linear",
    start_distance = 12,
    end_distance = 90,
})
skybox.setAmbientOcclusion({
    radius = 2.5,
    intensity = 0.65,
    bias = 0.025,
})
skybox.setEnabled(true)
-- skybox.clearFog() disables fog without changing the sky.
-- skybox.clearAmbientOcclusion() disables AO without changing the sky.
-- skybox.clear() removes the script-owned environment.
```

A six-face cubemap uses explicit axis names and can be assigned to the global
environment without flattening it into a panorama:

```luau
local studio = assets.loadCubemap({
    positive_x = "assets/studio/px.png",
    negative_x = "assets/studio/nx.png",
    positive_y = "assets/studio/py.png",
    negative_y = "assets/studio/ny.png",
    positive_z = "assets/studio/pz.png",
    negative_z = "assets/studio/nz.png",
})
skybox.setCubemap(studio, 15)
```

All six faces must be square and have identical dimensions. `newCubemap`
accepts live `ImageHandle` faces; `loadCubemap` resolves six image paths.
Cubemap face revisions propagate to the visible background and built-in PBR
lighting without replacing the `CubemapHandle`.

Fog is evaluated per pixel by the software/Web runtime and per fragment by the
native Vulkan PBR path. Scene View uses the same sanitized distance function;
particles and projected custom-mesh streams receive fogged vertex colors while
the embedded Game View remains the exact runtime authority.

Ambient occlusion uses conservative transformed world-space mesh bounds rather
than a screen-space depth approximation. For each receiving mesh, the renderer
chooses the nearest 32 eligible bounds, evaluates contact distance, surface
hemisphere, apparent extent, radius, bias, and authored intensity, and applies
the resulting visibility in linear light before emissive and fog. A renderer
with `casts_shadows = false` is not an AO occluder; one with
`receives_shadows = false` is not darkened by AO. Software and ordinary Web
meshes evaluate it per pixel, Vulkan built-in PBR evaluates it per fragment,
and projected custom meshes receive vertex-sampled AO. Scene View uses the same
policy with triangle-averaged preview values. This provides backend-stable
contact and crease shading, but it is not mesh-exact ray-traced AO or
depth-buffer SSAO; unusually interleaved custom WebGL/2D command chunks also do
not share occluders with separate software chunks.

The same equirectangular panorama or six-face cubemap supplies built-in PBR
environment lighting on Vulkan, software, and the ordinary Web mesh path. It
contributes bounded diffuse and roughness-aware specular samples using the
visible sky's image revisions, rotation, and intensity; the legacy synthetic
headlight is disabled while IBL is active. Software/Web representations and
native Vulkan cubemap uploads are revision-aware. Float-HDR texture uploads,
irradiance convolution, prefiltered specular mip chains, and a BRDF integration
LUT are not yet implemented.

## `core.ReflectionProbe3D`

`ReflectionProbe3D` supplies an authored local cubemap to built-in PBR meshes
inside a finite influence volume. Assign either a live `cubemap` handle in a
script or all six face properties (`positive_x`, `negative_x`, `positive_y`,
`negative_y`, `positive_z`, and `negative_z`) through the Inspector. Supplying
only some faces raises an actionable missing-face error; no faces leaves the
probe inactive.

```luau
local probe = room:AddComponent(core.ReflectionProbe3D)
probe.cubemap = studio
probe.size_x, probe.size_y, probe.size_z = 12, 5, 9
probe.blend_distance = 1.5
probe.intensity = 1.1
probe.rotation = 15
probe.priority = 10
```

The entity's hierarchy-resolved transform moves, rotates, and scales the
authored volume. Runtime selection tests the center of each receiving mesh's
transformed bounds against the conservative world AABB. Higher `priority`
wins overlapping volumes; ties prefer the greatest interior blend weight,
then nearest probe center and a stable source id. `blend_distance` fades local
lighting into the global `Environment3D` at volume edges. Visibility ancestry,
`visible`, `enabled`, `RenderLayer3D`, and the active camera mask are applied
before a probe is queued, so unloading or hiding it cannot leave stale light.

Software, ordinary Web, and native Vulkan built-in PBR paths share this
selection and blend policy. The Scene lighting panel can add/select a probe,
its Inspector persists all six image paths and settings, and the editor-only
**Reflection Probe Volumes** diagnostic shows the transformed influence box.
Embedded Game View remains the exact runtime visual authority; Scene View
currently visualizes the volume rather than duplicating the runtime IBL pass.

Probes currently consume assigned cubemaps. Runtime scene capture/baking,
irradiance/specular filtering, float-HDR faces, parallax-corrected box
projection, per-pixel volume selection, and built-in probe bindings for custom
shaders remain. A rotated probe's conservative AABB can influence corners
outside the authored oriented box.

## `core.ParticleSystem3D`

Each 3D emitter owns a fixed-capacity native pool (up to 100,000 particles)
and submits one camera-facing billboard batch. It supports point, box, sphere,
and cone emission; deterministic seeds; duration/looping; emission rate and
manual bursts; lifetime/speed ranges; gravity and drag; size/color fades;
rotation; and an optional texture.

```luau
local sparks = emitter:AddComponent(core.ParticleSystem3D)
sparks.shape = "cone"
sparks.max_particles = 4096
sparks.playing = false
sparks:Emit(256)
```

`Play`, `Pause`, `Stop`, and `Emit(count?)` have lowercase aliases.
`particle_count` is engine-derived. Simulation and per-emitter transparency
sorting are CPU-side; particles are not globally sorted between emitters.

## `core.Raycast3D`

`Raycast3D` is an authorable local-space query component backed by the same
native collider registry as `physics3d.raycast`. The Inspector exposes origin
offset, direction, maximum distance, layer/mask, trigger inclusion, and
self-exclusion. Each runtime update refreshes `hit`, `hit_entity_id`, hit
position, distance, and normal; `cast()` performs the same query immediately.

```luau
local sensor = actor:AddComponent(core.Raycast3D)
sensor.direction_y = -1
sensor.max_distance = 2.5
sensor.include_triggers = false
sensor:setOnHit(function(hit)
    print(hit.entity_id, hit.distance, hit.normal_y)
end)

local hit = sensor:cast()
```

The independently switchable Raycasts Scene View overlay draws the authored
world-space query without changing the component or simulating runtime state.

## `core.Collider3D`, `core.Trigger3D`, and physics materials

Both components author box, sphere, capsule, or triangle-mesh geometry with
the same world transform, mesh BVH, and reciprocal integer `layer`/`mask`
filtering used by `physics3d.raycast`, `contacts`, and capsule sweeps.
`Collider3D` can resolve exact primitive contacts. `Trigger3D` permanently
sets `is_trigger` and `non_physics`, so scripts cannot accidentally turn a
sensor into a resolving collider.

```luau
local rubber = assets.newPhysicsMaterial3D({
    name = "Rubber",
    friction = 0.8,
    restitution = 0.55,
})
assets.savePhysicsMaterial3D(rubber, "assets/materials/rubber")

local collider = actor:AddComponent(core.Collider3D)
collider.physics_material = rubber

local volume = checkpoint:AddComponent(core.Trigger3D)
volume.size_x, volume.size_y, volume.size_z = 4, 3, 2
volume:setOnEnter(function(hit) print("enter", hit.entity_id, hit.quality) end)
volume:setOnStay(function(hit) print("stay", hit.entity_id) end)
volume:setOnExit(function(hit) print("exit", hit.entity_id) end)
```

`PhysicsMaterial3DHandle` has shared identity plus `revision`, `get`,
`setFriction`, `setRestitution`, and `set`. Edits validate atomically: name
must not be empty and both factors must be finite within `0..1`. Bound
colliders read the current handle on their normal next update. With no handle,
the collider's inline `friction` and `restitution` remain the fallback.
`loadPhysicsMaterial3D` caches versioned `.neophysicsmaterial` files;
`newPhysicsMaterial`, `loadPhysicsMaterial`, `savePhysicsMaterial`, and
`unloadPhysicsMaterial` are aliases of their `*3D` forms.

Trigger overlap output is deterministic and deduplicated by other entity id.
`overlap_count` and sorted `overlapping_entity_ids` update every frame. Enter
fires for a new overlap, stay for every current overlap, and exit after
separation. Enter/stay callbacks receive `quality = "exact" | "bounds"` and
`exact`; exit omits quality because the pair is no longer touching. The Scene
View draws dedicated triggers in orange and uses their real transformed shapes
for collision-aware placement, without mutating the authored component.

## `core.CharacterController3D`

`CharacterController3D` is an upright kinematic capsule whose dimensions are
authored in world units (`height` includes both hemispheres). It shares the
native `Collider3D` registry and collision `layer`/`mask` contract. Calling
`Move(x, y, z)` treats its arguments as one world-space displacement and
returns the applied displacement, grounded state, support identity/normal, and
ordered collision hits.

Movement uses continuous capsule casts rather than end-position overlap. Box,
sphere, and capsule obstacles use exact primitive contact; mesh obstacles use
exact swept-sphere/triangle tests through the retained mesh BVH. After the
first hit, a bounded iterative solver removes only motion entering the surface,
preserving tangent motion for walls and walkable slopes. A step succeeds only
when the upward cast has headroom, the complete horizontal remainder clears,
and a downward cast finds a surface within `max_slope_degrees`. Ground snapping
keeps small descents stable. `skin_width` prevents persistent zero-distance
contacts.

```luau
local controller = player:AddComponent(core.CharacterController3D)
controller.radius = 0.45
controller.height = 1.8
controller.step_height = 0.3
controller.max_slope_degrees = 50
controller.velocity_x = moveX * 5
controller.velocity_z = moveZ * 5
controller:setOnCollision(function(hit)
    print(hit.entity_id, hit.normal_x, hit.normal_y, hit.normal_z)
end)
controller:setOnGrounded(function(ground)
    print("landed on", ground.entity_id)
end)

local jump = controller:Move(0, 0.75, 0)
```

When `use_gravity` is enabled, the component update integrates `velocity_y`
using `gravity`; horizontal velocity fields are also applied each frame.
Grounded controllers follow the supporting collider's translation before their
own velocity, supporting moving platforms. `onGrounded` fires on the airborne
to grounded transition; `onCollision` reports each blocking hit. The callbacks
do not change authored scene state. `physics3d.sweepCapsule` and its
snake-case alias expose the same continuous query for custom movement. The
Collider Shapes Scene View diagnostic renders the runtime-matching upright
capsule. Arbitrary capsule orientation and CCD for dynamic rigid bodies remain
outside this controller path.

## `core.LODGroup3D`

Attach `LODGroup3D` to an entity with `MeshRenderer3D` to select geometry using
distance from the active runtime camera:

```luau
local lod = model:AddComponent(core.LODGroup3D)
lod.lod0_mesh = "assets/tree-high.glb" -- empty inherits renderer.mesh_path
lod.lod1_mesh = "assets/tree-medium.glb"
lod.lod2_mesh = "assets/tree-low.glb"
lod.lod1_distance = 20
lod.lod2_distance = 50
lod.cull_distance = 100
lod.force_level = "automatic" -- lod0, lod1, lod2, or culled also accepted
```

The ranges are `[0, lod1)`, `[lod1, lod2)`, `[lod2, cull)`, then culled.
Non-finite values use defaults; negative and reversed thresholds are normalized
to a non-negative monotonic sequence. Empty mesh slots fall back toward LOD 0,
and an empty LOD 0 inherits `MeshRenderer3D.mesh_path`. `force_level` is a live
runtime override and bypasses distance selection, including forced culling.
After `MeshRenderer3D.update`, `active_level` reports the resolved populated
level (`-1` while culled) and `camera_distance` reports the measured distance.

The Scene View uses the same selector, fallback rules, entity world transform,
and camera distance as the runtime. Its editor-only **LOD State** diagnostic
draws the sanitized range spheres and resolved level. Preview rendering and the
diagnostic never serialize the runtime-observed fields or dirty the scene.
Because LOD selection is camera-dependent, an enabled group bypasses the
static-mesh preparation shortcut and is evaluated on every runtime frame.

## Backend and resolution behavior

The software renderer lazily allocates depth and 3D-AA scratch surfaces only
when needed and samples textures from immutable copy-on-write snapshots. Its
3D edge pass runs before 2D overlays. Vulkan uses a depth-tested, configurable
MSAA GPU raster path. Default-shader meshes retain revision-keyed indexed
device-local buffers, apply model/normal transforms and direct-light PBR on the
GPU, automatically instance compatible opaque submissions, and GPU-skin
supported armatures from per-draw palettes. Custom mesh shaders retain a
CPU-projected fallback. Vulkan also renders one 2048² directional/spot shadow
map before the main pass. The main pass writes and optionally MSAA-resolves to
a persistent linear RGBA16F image. A second fullscreen GPU pass applies the
last enabled exposure-tonemap configuration (None, Reinhard, or ACES plus
gamma) and writes the swapchain. Clear colors, ordinary RGBA images/UI,
equirectangular environments, and portable custom fragments are decoded into
linear space before blending; default PBR output remains unclamped until this
presentation pass. Default PBR meshes also bind an active equirectangular
environment and evaluate bounded diffuse/specular IBL in linear space; the
software and ordinary Web paths use the same orientation and lobe contract on
their RGBA8 reference framebuffer. Native bloom uses two reusable half-resolution RGBA16F
targets for threshold/downsample and bounded separable blur, then adds the
bloom image before tone mapping. Disabled or zero-radius/intensity bloom
performs no extra draws. Other ordered native effects still require GPU
ping-pong passes. WebAssembly composites ordinary meshes through the
antialiased software path and depth-tested custom-shader meshes through WebGL;
high-quality WebGL shader AA uses bounded 2× supersampling when device limits
permit it.

Native editor/runtime framebuffers use logical pixels at high-DPI scale factors
and expand once for presentation. Lighting quality tiers enforce bounded,
monotonic light-map texel budgets at 4K and 8K. These changes reduce resolution
scaling costs, but they are not a claim of universal performance superiority
over other engines; project-specific profiling is still required.

<!-- page: layout-effects | Layout, Effects, and Audio Components -->
# Layout, Effects, and Audio Components

## `core.Rect2D`

```luau
export type Rect2D = BaseDrawableComponent
```

Draws a rectangle over the entity's full transformed size. Defaults are the
shared drawable defaults.

## `core.EntityScaler`

```luau
export type EntityScaler = ComponentInstance & {
    enabled: boolean,
    edit_with_percent: boolean,
    editWithPercent: boolean?,
    x_percent: number,
    y_percent: number,
    size_x_percent: number,
    size_y_percent: number,
    xPercent: number?, yPercent: number?,
    sizeXPercent: number?, sizeYPercent: number?,
    percent_x: number?, percent_y: number?,
    percentX: number?, percentY: number?,
    offset_x: number, offset_y: number,
    offsetX: number?, offsetY: number?,
    pivot_x: number, pivot_y: number,
    pivotX: number?, pivotY: number?,
}
```

Every update, when enabled, this layout helper writes the owning entity's
anchors, position, pivots, and optional parent-relative size:

- x/y percentages and pivots are clamped to `0..1`;
- `x_percent` and `y_percent` become entity anchors;
- `offset_x` and `offset_y` become entity local position;
- a positive size percentage multiplies the direct parent's unscaled size;
- a size percentage of zero leaves that dimension unchanged.

Snake case is canonical. Camel aliases and `percent_x`/`percent_y` are read as
fallbacks. Defaults are enabled, percent editing enabled, and all numbers zero.
`edit_with_percent` is editor metadata and does not change the calculation.

## `core.Camera`

```luau
export type Camera = ComponentInstance & {
    enabled: boolean,
    SetActive: (self: Camera) -> boolean,
    setActive: (self: Camera) -> boolean,
    IsActive: (self: Camera) -> boolean,
    isActive: (self: Camera) -> boolean,
}
```

A Camera makes its owning entity's world position the center of the logical
window. For a `1280 × 720` window, a camera at `(400, 250)` maps that world
point to screen point `(640, 360)`. Camera translation is applied once to the
complete scene command list, lights, and light occluders. It does not change
physics coordinates, entity transforms, the clear color, or screen-space
editor/debug overlays. Camera rotation and zoom are not currently supported.

The global `mouse.x/y` and pointer event `x/y` remain logical screen
coordinates. Built-in UI controls and entity-listener hit tests automatically
apply the inverse camera translation, so they continue to line up with rendered
entities. When gameplay needs an explicit world point, use
`worldX = mouse.x - window.x / 2 + cameraEntityWorldX` and the corresponding
Y formula for the active camera (`window.x/y` are the logical width/height).

### `camera:SetActive() -> boolean`

`self` is the Camera component to select; colon syntax supplies it
automatically. The function returns `true` and makes the camera active when the
component is enabled and still attached to an entity. It returns `false`
without changing the selection when `enabled = false` or after the component
has been removed. `setActive` is an exact alias.

Selection is frame-atomic: even when `SetActive` is called from a system or a
component whose update occurs after a drawable, all scene commands in the
presented frame use the newly selected camera. With multiple cameras, the
explicit selection remains active while it is enabled and attached.

### `camera:IsActive() -> boolean`

Returns whether `self` is the selected camera at the instant of the call.
`isActive` is an exact alias. A newly added first camera becomes the automatic
fallback during the next camera pre-pass, so `IsActive()` can still be `false`
between `AddComponent` and the first update.

The first enabled Camera is selected automatically when a scene has no valid
active camera. If the active component is disabled, removed, or unloaded with
its scene, the next enabled camera becomes the fallback. When no enabled Camera
exists, translation is exactly `(0, 0)`, preserving the original behavior in
which entity coordinates are rendered directly as screen coordinates.

```luau
local player = ecs.newEntity("Player", ecs.root, 800, 450)
player.size_x, player.size_y = 32, 48
player:AddComponent(core.Sprite2D).image = assets.loadImage("assets/player.png")

local followRig = ecs.newEntity("Follow camera", ecs.root, 800, 450)
local followCamera = followRig:AddComponent(core.Camera)

local mapRig = ecs.newEntity("Map camera", ecs.root, 0, 0)
local mapCamera = mapRig:AddComponent(core.Camera)

-- Switch to the map for one mode, then back to the player-following rig.
assert(mapCamera:SetActive())
-- Later:
followRig.x, followRig.y = player:GetWorldPosition()
followCamera:SetActive()
```

Raw `mouse.x` and `mouse.y` remain screen coordinates. Built-in UI controls and
entity listener hit tests apply the inverse camera translation internally, so
their visible and clickable regions stay aligned. APIs explicitly documented
as accepting world coordinates, such as raycasts and `isInside`, continue to
take unshifted world values.

## `core.Shape2D`

```luau
export type Shape2DShape = "box" | "circle" | "triangle" | "right_triangle"
    | "righttriangle" | "rightangledtriangle"
export type TriangleCorner = "bl" | "br" | "tl" | "tr" | "bottomright"
    | "rightbottom" | "topleft" | "lefttop" | "topright" | "righttop"

export type Shape2D = BaseDrawableComponent & {
    shape: Shape2DShape,
    triangle_corner: TriangleCorner,
    offset_x: number,
    offset_y: number,
    size_x: number,
    size_y: number,
}
```

Defaults: box, bottom-left triangle corner, zero offset, and zero component
size. A non-positive component dimension falls back to the entity dimension.
Circle radius is half the smaller effective dimension. `triangle` and all
triangle aliases select a right triangle.

## `core.ParticleSystem2D`

```luau
export type ParticleEmitterShape = "point" | "box" | "circle"
export type ParticleColorKeypoint = { time: number, color: Color4Value }
export type ParticleNumberKeypoint = { time: number, value: number }

export type ParticleSystem2D = BaseDrawableComponent & {
    image: ImageHandle?,
    playing: boolean,
    looping: boolean,
    duration: number,
    emission_rate: number,
    max_particles: number,
    lifetime: number,
    speed: number,
    direction: number,
    spread: number,
    start_size: number,
    end_size: number,
    start_color: Color4Value,
    end_color: Color4Value,
    color_sequence: { ParticleColorKeypoint },
    transparency_sequence: { ParticleNumberKeypoint },
    shape: ParticleEmitterShape,
    radius: number,
    gravity_x: number,
    gravity_y: number,
    particle_count: number,
    play: (self: ParticleSystem2D) -> (),
    Play: (self: ParticleSystem2D) -> (),
    pause: (self: ParticleSystem2D) -> (),
    Pause: (self: ParticleSystem2D) -> (),
    stop: (self: ParticleSystem2D) -> (),
    Stop: (self: ParticleSystem2D) -> (),
    emit: (self: ParticleSystem2D, count: number?) -> (),
    Emit: (self: ParticleSystem2D, count: number?) -> (),
}
```

### Defaults

| Field | Default |
| --- | --- |
| `playing`, `looping` | `true`, `true` |
| `duration` | `5` seconds |
| `emission_rate`, `max_particles` | `12`, `256` |
| `lifetime`, `speed` | `1.5`, `80` |
| `direction`, `spread` | `-90`, `30` degrees |
| `start_size`, `end_size` | `8`, `2` |
| `start_color`, `end_color` | orange opaque, red-orange transparent |
| `color_sequence` | matching orange-to-red-orange keypoints at `0` and `1` |
| `transparency_sequence` | `0` to `1` at normalized times `0` and `1` |
| `shape`, `radius` | `point`, `32` |
| `gravity_x`, `gravity_y` | `0`, `60` |
| `image` | `nil` |
| `particle_count` | `0`, engine-derived |

Direction/spread use degrees. Color and transparency sequences are sampled over
normalized lifetime; keypoint times are clamped/sorted by the renderer. Without
an image, particles render as circles; with one, the image is tinted and scaled.
Emission is bounded by `max_particles`.

| Canonical method | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `particles:play()` (`Play` alias) | None. | `()` | Sets `playing = true` and resumes automatic emission from current timers. A non-looping emitter whose duration already elapsed may need `stop()` before replaying from time zero. Existing particles keep their current ages. |
| `particles:pause()` (`Pause` alias) | None. | `()` | Sets `playing = false`, stopping automatic emission while existing particles continue to simulate and expire. It does not clear queued/manual state. |
| `particles:stop()` (`Stop` alias) | None. | `()` | Pauses, clears all live particles and manual/emission timers, and resets derived `particle_count` to zero by the next component update. |
| `particles:emit(count?)` (`Emit` alias) | Optional numeric burst count, default `1`. | `()` | Queues a manual burst, still capped by `max_particles`. Counts are converted/clamped to a non-negative whole-particle quantity; zero/negative requests emit nothing. A paused emitter can still emit manually, while invisible rendering does not necessarily stop simulation. |

Non-looping automatic emission stops after `duration`, while particles already
alive finish their lifetimes.

```luau
local sparks = hitEffect:AddComponent(core.ParticleSystem2D)
sparks.looping = false
sparks.playing = false
sparks.max_particles = 64
sparks:emit(24)
```

## `core.AnimationController`

```luau
export type AnimationController = ComponentInstance & {
    animation: AnimationClip?,
    autoplay: boolean,
    looping: boolean,
    playing: boolean,
    speed: number,
    play: (self: AnimationController) -> (),
    Play: (self: AnimationController) -> (),
    pause: (self: AnimationController) -> (),
    Pause: (self: AnimationController) -> (),
    stop: (self: AnimationController) -> (),
    Stop: (self: AnimationController) -> (),
}
```

Defaults: no animation, autoplay and looping `true`, playing `false`, speed
`1`. Assigning an animation creates/replaces the internal player when needed.

| Canonical method | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `controller:play()` (`Play` alias) | None. | `()` | Sets desired `playing = true`. When `animation` is present, creates/reuses an internal player and starts it with component `looping`/`speed`. With no clip, the desired flag remains but there is nothing to sample until a clip is assigned. |
| `controller:pause()` (`Pause` alias) | None. | `()` | Sets desired playback false and pauses an existing player at its current time. Safe before a clip/player exists. |
| `controller:stop()` (`Stop` alias) | None. | `()` | Sets desired playback false and asks an existing player to pause/rewind to zero. Safe with no clip. |

Assigning a different animation creates/replaces the internal player when
needed. A negative/non-finite `speed` is rejected by the underlying handle
during synchronization.

```luau
local controller = door:AddComponent(core.AnimationController)
controller.animation = animation.load("animations/door.neoanim")
controller.looping = false
controller:play()
```

## `core.SpatialSound2D`

```luau
export type SpatialSound2D = ComponentInstance & {
    sound: SoundHandle?,
    volume: number,
    looping: boolean,
    autoplay: boolean,
    play: (self: SpatialSound2D) -> boolean,
    Play: (self: SpatialSound2D) -> boolean,
    stop: (self: SpatialSound2D) -> (),
    Stop: (self: SpatialSound2D) -> (),
}
```

Defaults: sound `nil`, volume `1`, looping/autoplay `false`. The component moves
the emitter every frame while active. Removal stops its sound.

| Canonical method | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `spatial:play()` (`Play` alias) | None. | `false` when `sound` is nil; `true` after requesting playback. | Starts/restarts spatial playback at the owning entity's current world position using `looping` and volume clamped to `0..1`. A detached component or unloaded/invalid sound cannot produce valid playback and may fail through the audio backend. `true` is a start request, not proof the browser allowed audible output. |
| `spatial:stop()` (`Stop` alias) | None. | `()` | Stops the sound handle associated with this component and clears its active state. Safe when idle or before a sound is assigned. |

```luau
local hum = machine:AddComponent(core.SpatialSound2D)
hum.sound = assets.loadSound("assets/hum.ogg")
hum.looping, hum.volume = true, 0.35
assert(hum:play())
```

## `core.AudioSource3D` and `core.AudioListener3D`

`AudioSource3D` is the authorable 3D emitter. It exposes `sound`, `enabled`,
`volume`, `looping`, `autoplay`, `min_distance`, `max_distance`, `rolloff`, and
the `inverse`, `linear`, or `exponential` `distance_model`. `play`/`Play`
returns `false` without an enabled source and valid sound; `stop`/`Stop` is safe
while idle. Position and attenuation edits propagate every runtime frame, and
removal or disabling stops the source's independent voice.

`AudioListener3D` follows its entity's world position and XYZ Euler
orientation. The first enabled listener becomes active; `SetActive`/`setActive`
selects another explicitly, while `IsActive`/`isActive` reports the selection.
`ear_distance` defaults to `0.2` world units. This explicit selection prevents
multiple listeners from overwriting one another according to update order.

```luau
local listener = player:AddComponent(core.AudioListener3D)
listener:SetActive()

local source = machine:AddComponent(core.AudioSource3D)
source.sound = assets.loadSound("assets/machine.ogg")
source.looping = true
source.autoplay = true
source.min_distance = 2
source.max_distance = 45
source.distance_model = "exponential"
```

<!-- page: text | Text Components -->
# Text Components

## Shared text style

```luau
export type TextScaleMode = "none" | "fit" | "fit_width" | "fit_height"
export type TextAlignX = "left" | "center" | "right"
export type TextAlignY = "top" | "center" | "bottom"
export type TextWrapMode = "none" | "word" | "char"
export type TextBoundsMode = "content" | "entity" | "box" | "bounds"

export type TextFontOptions = {
    path: string?,
    file: string?,
    source: string?,
    builtin: string?,
    name: string?,
}
export type TextFont = string | TextFontOptions

export type UiTextStyle = {
    scale: number,
    min_scale: number,
    align_x: TextAlignX,
    align_y: TextAlignY,
    text_scale: TextScaleMode,
    wrap: TextWrapMode | boolean,
    padding: number,
    padding_x: number,
    padding_y: number,
    line_spacing: number,
    letter_spacing: number,
    tab_size: number,
    tab_width: number?,
    font: TextFont?,
    antialiasing: "inherit" | "off" | "standard" | "high",
    alignX: TextAlignX?,
    alignY: TextAlignY?,
    vertical_align: TextAlignY?,
    verticalAlign: TextAlignY?,
    textScale: TextScaleMode?,
}
```

`font` may be `nil`/`"default"`, a project-relative font path, or an options
table. `path`, `file`, and `source` are path aliases; `builtin`/`name` select a
built-in name. `wrap = true` means word wrap and `false` means none.
`padding_x/y` override `padding`. `tab_width` aliases `tab_size`.
Compatibility fallbacks are `alignX`, `alignY`,
`vertical_align`/`verticalAlign`, and `textScale`; canonical snake-case fields
take precedence.

`text_scale = fit` scales both axes to fit, `fit_width` constrains width, and
`fit_height` constrains height, never below `min_scale`. `antialiasing = inherit`
uses `app.antiAliasing`.

## `core.TextBox`, `TextLabel`, and `RudimentaryTextLabel`

```luau
export type TextBox = BaseDrawableComponent & UiTextStyle & {
    text: string,
    used_scale: number,
    size_mode: TextBoundsMode,
    scale_x: number,
    scale_y: number,
    dx: number,
    dy: number,
    line_count: number,
    setBold: (self: TextBox, startIndex: number, endIndex: number) -> (),
    setItalic: (self: TextBox, startIndex: number, endIndex: number) -> (),
    setUnderline: (self: TextBox, startIndex: number, endIndex: number) -> (),
    setColor: (self: TextBox, startIndex: number, endIndex: number, color: Color4Value) -> (),
    setSize: (self: TextBox, startIndex: number, endIndex: number, scale: number) -> (),
    setFont: (self: TextBox, startIndex: number, endIndex: number, fontPath: string) -> (),
    setOffset: (self: TextBox, startIndex: number, endIndex: number, x: number, y: number) -> (),
    setPixelOffset: (self: TextBox, startIndex: number, endIndex: number, x: number, y: number) -> (),
    setCharacterOffset: (self: TextBox, charIndex: number, x: number, y: number) -> (),
    clearFormatting: (self: TextBox, startIndex: number?, endIndex: number?) -> (),
    clearAllFormatting: (self: TextBox) -> (),
    getLetterCount: (self: TextBox) -> number,
    getLetterPosition: (self: TextBox, charIndex: number) -> (number?, number?),
    getLetterBounds: (self: TextBox, charIndex: number) -> (number?, number?, number?, number?),
    getClosestLetterIndex: (self: TextBox, x: number, y: number) -> number?,
    getClosestCharacterIndex: (self: TextBox, x: number, y: number) -> number?,
}
```

Defaults: text `"Text Box"`, scale/used scale `32`, minimum `1`, no fitting,
left/top alignment, no wrap, content size mode, zero padding/letter spacing,
line spacing `1`, tab size `4`, inherited anti-aliasing, and default font.

`size_mode = content` uses measured content bounds; `entity`, `box`, and
`bounds` use the entity rectangle. `used_scale`, `dx`, `dy`, and `line_count`
are layout outputs. `scale_x`/`scale_y` are legacy layout fields and begin zero.

### Rich formatting and letter queries

Ranges use zero-based, end-exclusive Unicode-scalar indexes (not UTF-8 byte
offsets or grapheme-cluster indexes). Formatting ranges may overlap; later
range properties are composed by layout and remain while edited text still
intersects them.

| Method | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `text:setBold(startIndex, endIndex)` | Start inclusive and end exclusive. | `()` | Adds a bold range. Negative/fractional indexes are coerced to non-negative whole indexes. An empty/reversed/out-of-text range has no visible glyphs but can remain in range state. |
| `text:setItalic(startIndex, endIndex)` | Same range contract. | `()` | Adds italic formatting; overlaps combine with bold/other styles. |
| `text:setUnderline(startIndex, endIndex)` | Same range contract. | `()` | Adds underline formatting. |
| `text:setColor(startIndex, endIndex, color)` | Range and `Color4Value`. | `()` | Adds a foreground-color range. Color is read during layout; pass a complete valid color. |
| `text:setSize(startIndex, endIndex, scale)` | Range and numeric relative scale. | `()` | Multiplies glyph size relative to component `scale`; non-positive values can make text invisible/degenerate and should be avoided. |
| `text:setFont(startIndex, endIndex, fontPath)` | Range and project font path/name. | `()` | Selects a font for the range. The call stores formatting; missing/invalid font data can surface when layout/rendering resolves it. |
| `text:setOffset(startIndex, endIndex, x, y)` (`setPixelOffset` alias) | Range and finite pixel offsets. | `()` | Visually shifts glyphs without changing their normal advance. End is clamped to at least start in this operation. Offsets affect reported world letter bounds after layout. |
| `text:setCharacterOffset(charIndex, x, y)` | One index and pixel offsets. | `()` | Convenience range `[charIndex, charIndex + 1)`. An out-of-text index is retained but has no visible effect until text/range intersects it. |
| `text:clearFormatting(startIndex?, endIndex?)` | Either no complete range, or inclusive/exclusive bounds. | `()` | With either bound omitted, clears every rich range. With both, removes only overlap and splits preserved left/right parts as needed. Empty/reversed ranges remove nothing. |
| `text:clearAllFormatting()` | None. | `()` | Unconditionally replaces the rich-range list with an empty one. Plain component-wide style fields remain unchanged. |
| `text:getLetterCount()` | None. | Non-negative Unicode-scalar count. | Counts `text` characters, not bytes or visual grapheme clusters; combining marks can count separately. Does not require a rendered frame. |
| `text:getLetterPosition(charIndex)` | Zero-based glyph index. | World `x, y`, or `nil, nil`. | Refreshes layout when attached and returns the top-left of that glyph. Index `-1` addresses the start caret and index equal to letter count addresses the end caret; other invalid/non-integral values return nils. |
| `text:getLetterBounds(charIndex)` | Zero-based glyph/caret index. | World `x, y, width, height`, or four nils. | Same refresh/index rules as position. Newlines/whitespace can have layout-specific bounds. |
| `text:getClosestLetterIndex(x, y)` (`getClosestCharacterIndex` alias) | Finite world coordinates. | Nearest insertion index `0..letterCount`, or `nil` for invalid arguments. | Refreshes layout, considers both sides of every glyph, and returns `0` for a valid empty layout. Ties use the first candidate encountered. |

The three `core` names reference the same prototype behavior.

```luau
local title = label:AddComponent(core.TextBox)
title.text = "NeoLOVE editor"
title:setBold(0, 7)
title:setColor(0, 7, Color4(255, 90, 150))
title:setPixelOffset(8, 14, 0, 2)

local index = title:getClosestLetterIndex(mouse.x, mouse.y)
if index then
    local x, y, w, h = title:getLetterBounds(index)
    if x then print("caret", index, x, y, w, h) end
end
```

## `core.TextInput`

```luau
export type TextInput = BaseDrawableComponent & UiTextStyle & {
    text: string,
    placeholder: string,
    enabled: boolean,
    locked: boolean,
    hovered: boolean,
    focused: boolean,
    password: boolean,
    max_length: number,
    submit_on_enter: boolean,
    clear_on_submit: boolean,
    blur_on_submit: boolean,
    cursor_index: number,
    view_start: number,
    cursor_blink: number,
    caret_width: number,
    text_color: Color4Value,
    placeholder_color: Color4Value,
    disabled_text_color: Color4Value,
    caret_color: Color4Value,
    background_color: Color4Value,
    hover_background_color: Color4Value,
    focus_background_color: Color4Value,
    disabled_background_color: Color4Value,
    border_color: Color4Value,
    hover_border_color: Color4Value,
    focus_border_color: Color4Value,
    disabled_border_color: Color4Value,
    border_width: number,
    corner_radius: number,
    background_image: ImageHandle?,
    icon_image: ImageHandle?,
    icon_color: Color4Value,
    icon_size: number,
    icon_gap: number,
    icon_side: "left" | "right",
    slice_left: number, slice_right: number, slice_top: number, slice_bottom: number,

    setBold: (self: TextInput, startIndex: number, endIndex: number) -> (),
    setItalic: (self: TextInput, startIndex: number, endIndex: number) -> (),
    setUnderline: (self: TextInput, startIndex: number, endIndex: number) -> (),
    setColor: (self: TextInput, startIndex: number, endIndex: number, color: Color4Value) -> (),
    setSize: (self: TextInput, startIndex: number, endIndex: number, scale: number) -> (),
    setFont: (self: TextInput, startIndex: number, endIndex: number, fontPath: string) -> (),
    setOffset: (self: TextInput, startIndex: number, endIndex: number, x: number, y: number) -> (),
    setPixelOffset: (self: TextInput, startIndex: number, endIndex: number, x: number, y: number) -> (),
    setCharacterOffset: (self: TextInput, charIndex: number, x: number, y: number) -> (),
    clearFormatting: (self: TextInput, startIndex: number?, endIndex: number?) -> (),
    clearAllFormatting: (self: TextInput) -> (),
    focus: (self: TextInput) -> (),
    Focus: (self: TextInput) -> (),
    blur: (self: TextInput) -> (),
    Blur: (self: TextInput) -> (),
    onChanged: ((entity: Entity, component: TextInput, text: string) -> ())?,
    onSubmit: ((entity: Entity, component: TextInput, text: string) -> ())?,
    onFocus: ((entity: Entity, component: TextInput) -> ())?,
    onBlur: ((entity: Entity, component: TextInput) -> ())?,
}
```

TextInput is a single-line editable widget. Defaults are empty text,
placeholder `"Type here"`, enabled/unlocked, unfocused, non-password, unlimited
length (`max_length = 0`), submit on Enter, no clear/blur on submit, cursor zero,
scale `18`, minimum `12`, left/center alignment, no fitting/wrap, and 2-pixel
caret. It uses the VS Code Dark+ colors shown by the field names.

`hovered`, `focused`, `cursor_index`, `view_start`, and `cursor_blink` are
engine-updated. Password mode masks display but retains real `text`. The same
rich formatting methods as TextBox are supported with identical parameters,
no return values, Unicode-scalar range semantics, and edge cases.

| Input-specific method | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `inputComponent:focus()` (`Focus` alias) | None. | `()` | Sets `focused` true only if `enabled` and not `locked`; otherwise explicitly leaves/sets it false. Focus transitions discovered by the next update invoke `onFocus`/`onBlur` and request/hide the platform keyboard as appropriate. |
| `inputComponent:blur()` (`Blur` alias) | None. | `()` | Sets `focused` false unconditionally. Repeated calls are safe; callback delivery follows the component update's transition observation. |

Callbacks return nothing meaningful. `onChanged(entity, component, text)` runs
after a user edit that changes the string; `onSubmit(entity, component, text)`
runs on enabled Enter submission before optional clear/blur behavior;
`onFocus(entity, component)` and `onBlur(entity, component)` report focus
transitions. Programmatically assigning `text` does not necessarily synthesize
a user `onChanged` event. Callback errors are reported by the runtime and can
interrupt the remainder of that UI update.

```luau
local field = nameEntity:AddComponent(core.TextInput)
field.placeholder = "Display name"
field.max_length = 24
field.onSubmit = function(entity, component, value)
    print("submitted", value)
    component:blur()
end
field:focus()
```

The generated declaration file uses these same callback signatures, including
the owning `entity` first and the component instance second.

<!-- page: ui-components | UI Components -->
# UI Components

UI widgets use transformed entity bounds for drawing and pointer input. Popups
register an overlay region so controls behind an open dropdown do not receive
the same click. Every callback receives `(entity, component, ...)`.

## Panel image fields

Panel-style widgets accept `background_image` and
`slice_left`/`slice_right`/`slice_top`/`slice_bottom`. A live image with any
positive slice draws nine-sliced; otherwise the solid background and rounded
border are used. `border_width` and `corner_radius` are clamped non-negative.
All panel-style widgets also read `borderWidth`, `cornerRadius`, `sliceLeft`,
`sliceRight`, `sliceTop`, and `sliceBottom` as compatibility fallbacks.

## `core.Panel` and `core.Frame`

```luau
export type Panel = BaseDrawableComponent & {
    background_color: Color4Value,
    border_color: Color4Value,
    border_width: number,
    corner_radius: number,
    background_image: ImageHandle?,
    slice_left: number,
    slice_right: number,
    slice_top: number,
    slice_bottom: number,
    borderWidth: number?,
    cornerRadius: number?,
    sliceLeft: number?,
    sliceRight: number?,
    sliceTop: number?,
    sliceBottom: number?,
}
export type Frame = Panel
```

Defaults: background `[37,37,38,255]`, border `[69,69,69,255]`, border width
`1`, radius `4`, no image, and zero slices. `Frame` is the same prototype.

## `core.Button`

```luau
export type Button = BaseDrawableComponent & UiTextStyle & {
    text: string,
    enabled: boolean,
    hovered: boolean,
    pressed: boolean,
    background_color: Color4Value,
    hover_background_color: Color4Value,
    pressed_background_color: Color4Value,
    disabled_background_color: Color4Value,
    border_color: Color4Value,
    hover_border_color: Color4Value,
    pressed_border_color: Color4Value,
    disabled_border_color: Color4Value,
    text_color: Color4Value,
    hover_text_color: Color4Value,
    pressed_text_color: Color4Value,
    disabled_text_color: Color4Value,
    border_width: number,
    corner_radius: number,
    background_image: ImageHandle?,
    icon_image: ImageHandle?,
    icon_color: Color4Value,
    icon_size: number,
    icon_gap: number,
    icon_side: "left" | "right",
    slice_left: number, slice_right: number, slice_top: number, slice_bottom: number,
    onClick: ((entity: Entity, component: Button) -> ())?,
    onPress: ((entity: Entity, component: Button) -> ())?,
    onRelease: ((entity: Entity, component: Button) -> ())?,
    onHoverEnter: ((entity: Entity, component: Button) -> ())?,
    onHoverLeave: ((entity: Entity, component: Button) -> ())?,
}
```

Defaults: text `Button`, enabled, scale `18`/minimum `10`, centered fit text,
padding `8` (`12` horizontal), borderless radius `2`, no images, icon size `0`,
gap `10`, icon left, and zero slices. Colors cover normal, hover, pressed, and
disabled background/border/text states. `hovered` and `pressed` are derived.

Pressing inside calls `onPress`. Releasing a previously pressed button calls
`onRelease`, then `onClick` only if still hovered. Hover transitions call the
corresponding callbacks. Disabled buttons clear pressed state and do not emit.

Each optional callback receives `(entity, component)` and its return values are
ignored. `onPress` runs on a primary press inside; `onRelease` runs when that
captured press is released even if the pointer moved outside; `onClick` then
runs only if the release is still inside. `onHoverEnter`/`onHoverLeave` run once
per derived hover transition. Assigning `nil` disables a callback. Callback
errors are reported during the UI update and can prevent later callbacks in
that same interaction.

```luau
local button = buttonEntity:AddComponent(core.Button)
button.text = "Continue"
button.onClick = function(entity, component)
    component.enabled = false
    print(entity.name, "clicked")
end
```

## `core.Slider`

```luau
export type Slider = BaseDrawableComponent & {
    enabled: boolean,
    hovered: boolean,
    dragging: boolean,
    min: number,
    max: number,
    value: number,
    fraction: number,
    step: number,
    orientation: "horizontal" | "vertical",
    track_thickness: number,
    thumb_size: number,
    thumb_corner_radius: number,
    corner_radius: number,
    border_width: number,
    background_color: Color4Value,
    hover_background_color: Color4Value,
    disabled_background_color: Color4Value,
    border_color: Color4Value,
    hover_border_color: Color4Value,
    disabled_border_color: Color4Value,
    fill_color: Color4Value,
    hover_fill_color: Color4Value,
    disabled_fill_color: Color4Value,
    thumb_color: Color4Value,
    hover_thumb_color: Color4Value,
    disabled_thumb_color: Color4Value,
    background_image: ImageHandle?,
    slice_left: number, slice_right: number, slice_top: number, slice_bottom: number,
    setValue: (self: Slider, value: number) -> (),
    SetValue: (self: Slider, value: number) -> (),
    onChanged: ((entity: Entity, component: Slider, value: number) -> ())?,
}
```

Defaults: enabled, range `0..100`, value/fraction `0`, continuous step `0`,
horizontal, track `6`, thumb `16`, thumb radius `8`, track radius `3`, and no
border/background image. Hover and disabled palettes exist for track, border,
fill, and thumb.

The engine clamps `value` between the lower and upper of `min`/`max`, so
reversed ranges work. Fraction follows the directed range and remains `0..1`.
Positive `step` snaps relative to `min`. Vertical sliders place maximum at the
top.

### `slider:setValue(value) -> ()` (`SetValue` alias)

`value` is the desired numeric value. The method snaps/clamps it against the
current directed range, writes `value`, and recomputes derived `fraction`; it
returns nothing and deliberately does **not** invoke `onChanged`. Equal min/max
produces a stable zero fraction. Non-finite values are rejected/normalized by
numeric validation rather than becoming a useful slider state.

Dragging invokes `onChanged(entity, component, value)` only when the numeric
value actually changes. The callback's returns are ignored; disabled sliders
do not drag or fire. Use the callback for user intent and call your own handler
after `setValue` if programmatic changes should have equivalent effects.

```luau
local volume = sliderEntity:AddComponent(core.Slider)
volume.min, volume.max, volume.step = 0, 1, 0.05
volume:setValue(0.7)
volume.onChanged = function(entity, component, value)
    settings.volume = value
end
```

## `core.Dropdown`

```luau
export type DropdownOption = string | number | boolean | {
    text: (string | number | boolean)?,
    label: (string | number | boolean)?,
    name: (string | number | boolean)?,
    value: (string | number | boolean)?,
    id: (string | number | boolean)?,
    image: ImageHandle?,
    icon: ImageHandle?,
    image_color: Color4Value?,
    icon_color: Color4Value?,
    image_source_x: number?, image_source_y: number?,
    image_source_w: number?, image_source_h: number?,
    image_source_width: number?, image_source_height: number?,
    image_sourceX: number?, image_sourceY: number?,
    image_sourceW: number?, image_sourceH: number?,
    image_sourceWidth: number?, image_sourceHeight: number?,
    icon_source_x: number?, icon_source_y: number?,
    icon_source_w: number?, icon_source_h: number?,
    icon_source_width: number?, icon_source_height: number?,
    icon_sourceX: number?, icon_sourceY: number?,
    icon_sourceW: number?, icon_sourceH: number?,
    icon_sourceWidth: number?, icon_sourceHeight: number?,
    [string]: any,
}

export type Dropdown = BaseDrawableComponent & UiTextStyle & {
    enabled: boolean,
    open: boolean,
    hovered: boolean,
    hover_index: number,
    selected_index: number,
    selected_text: string,
    selected_value: string,
    scroll_index: number,
    wheel_scroll_accumulator: number,
    placeholder: string,
    options: { DropdownOption },
    item_height: number,
    item_corner_radius: number,
    item_icon_size: number,
    item_icon_gap: number,
    menu_gap: number,
    max_visible_items: number,
    open_upwards: boolean,
    background_color: Color4Value,
    hover_background_color: Color4Value,
    open_background_color: Color4Value,
    disabled_background_color: Color4Value,
    border_color: Color4Value,
    hover_border_color: Color4Value,
    open_border_color: Color4Value,
    disabled_border_color: Color4Value,
    text_color: Color4Value,
    disabled_text_color: Color4Value,
    menu_background_color: Color4Value,
    menu_border_color: Color4Value,
    item_background_color: Color4Value,
    item_hover_background_color: Color4Value,
    item_selected_background_color: Color4Value,
    item_text_color: Color4Value,
    item_hover_text_color: Color4Value,
    item_selected_text_color: Color4Value,
    border_width: number,
    corner_radius: number,
    background_image: ImageHandle?,
    icon_image: ImageHandle?,
    icon_color: Color4Value,
    icon_size: number,
    icon_gap: number,
    icon_side: "left" | "right",
    slice_left: number, slice_right: number, slice_top: number, slice_bottom: number,
    onChanged: ((entity: Entity, component: Dropdown, index: number, value: string) -> ())?,
}
```

### Options and selection

Primitive options are converted to text. For table options, display text uses
the first present value among `text`, `label`, `name`, and `value`. Stored value
uses `value`, then `id`, then text. Numbers and booleans become strings. Empty
items are skipped. `image`/`icon` are aliases; so are their tint fields.
Optional source rectangles use `image_source_x/y/w/h` or
`icon_source_x/y/w/h`, including `width`/`height` and camel-case spellings.

Indexes are 1-based, while `0` means no selection when the option list is
empty. With options present, the runtime clamps selection to a valid item.
`selected_text`, `selected_value`, `hover_index`, and `scroll_index` are updated
by the component. `wheel_scroll_accumulator` is engine-managed fractional wheel
state and starts at zero. Clicking a new item calls `onChanged` and closes the
menu.

### Defaults

The dropdown begins enabled/closed, placeholder `Select...`, item height `32`,
item radius `6`, automatic item icon sizing (`0`), icon gap `8`, menu gap `4`,
up to 8 visible items, and downward preference. Text is scale `18`, minimum
`12`, left/center, fit-width, padding `8`/horizontal `10`. Normal, hover, open,
disabled, menu, item hover, and selected colors are all independent.

Wheel scrolling changes the visible zero-based `scroll_index`. `open_upwards`
forces upward opening; otherwise the menu automatically flips when it would
overflow the bottom and space exists above.

`onChanged(entity, component, index, value)` receives a 1-based selected option
index and its normalized **string** value. Its return values are ignored. It
runs only for a user choice that changes selection, not when gameplay assigns
`selected_index`/`options`. Empty or disabled dropdowns do not emit. Callback
errors are reported during the UI update after selection state has changed.

```luau
local quality = dropdownEntity:AddComponent(core.Dropdown)
quality.options = {
    { text = "Low (fast)", value = "low" },
    { text = "High", value = "high" },
    { text = "Ultra", value = "ultra" },
}
quality.onChanged = function(entity, component, index, value)
    lighting.setQuality(value)
    print("choice", index, component.selected_text)
end
```

<!-- page: image-components | Image and Tile Components -->
# Image and Tile Components

## Source-rectangle convention

Image components accept a rectangle only when both width and height are
positive. These aliases are equivalent:

```luau
source_x / sourceX
source_y / sourceY
source_w / sourceW / source_width / sourceWidth
source_h / sourceH / source_height / sourceHeight
```

Coordinates are image pixels. The rectangle is clipped to the image bounds.
Without a complete valid rectangle, the whole image is used.

## `core.Sprite2D` and `core.Image2D`

```luau
export type Sprite2D = BaseDrawableComponent & {
    image: ImageHandle?,
    source_x: number?, source_y: number?,
    source_w: number?, source_h: number?,
    source_width: number?, source_height: number?,
    sourceX: number?, sourceY: number?,
    sourceW: number?, sourceH: number?,
    sourceWidth: number?, sourceHeight: number?,
}
export type Image2D = Sprite2D
```

Both scale an image/source rectangle to the entity bounds. They are separate
prototype tags with identical rendering behavior. Defaults: no image, opaque
white tint, visible, no shader/source rectangle.

## `core.SpriteSheet2D`

```luau
export type SpriteSheet2D = BaseDrawableComponent & {
    image: ImageHandle?,
    frame_width: number,
    frame_height: number,
    columns: number,
    frame_count: number,
    spacing: number,
    margin: number,
    frame: number,
    fps: number,
    playing: boolean,
    looping: boolean,
    play: (self: SpriteSheet2D) -> (),
    Play: (self: SpriteSheet2D) -> (),
    pause: (self: SpriteSheet2D) -> (),
    Pause: (self: SpriteSheet2D) -> (),
    stop: (self: SpriteSheet2D) -> (),
    Stop: (self: SpriteSheet2D) -> (),
    setFrame: (self: SpriteSheet2D, frame: number) -> (),
    set_frame: (self: SpriteSheet2D, frame: number) -> (),
}
```

Defaults: frame `32x32`, columns/frame count `0` (auto-derived), spacing/margin
`0`, frame `0`, 12 FPS, playing and looping. Frames are zero-based and clipped
to available atlas cells. `columns = 0` derives from image width;
`frame_count <= 0` derives the whole atlas. Positive margin surrounds the
atlas; spacing separates cells.

| Canonical method | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `sheet:play()` (`Play` alias) | None. | `()` | Sets `playing = true` and resumes from the current frame/accumulated time. With no valid image/cells there is nothing to advance, but desired state remains. |
| `sheet:pause()` (`Pause` alias) | None. | `()` | Sets `playing = false` while retaining frame and accumulated fractional-frame time. |
| `sheet:stop()` (`Stop` alias) | None. | `()` | Pauses, resets frame to zero, and clears accumulated time. Safe before an image is assigned. |
| `sheet:setFrame(frame)` (`set_frame` alias) | Desired zero-based numeric frame. | `()` | Converts/clamps negative input to zero and resets accumulated time. Effective rendering also clamps to the available/declared atlas range. Setting the final frame does not itself toggle `playing`. |

Non-looping playback stops on the last valid frame. Zero/non-positive FPS does
not produce forward automatic advancement.

```luau
local walk = actor:AddComponent(core.SpriteSheet2D)
walk.image = assets.loadImage("assets/walk.png")
walk.frame_width, walk.frame_height = 32, 48
walk.frame_count, walk.fps = 8, 10
walk:setFrame(3)
walk:play()
```

## `core.NineSliceSprite2D`

```luau
export type NineSliceSprite2D = BaseDrawableComponent & {
    image: ImageHandle?,
    source_x: number?, source_y: number?,
    source_w: number?, source_h: number?,
    source_width: number?, source_height: number?,
    sourceX: number?, sourceY: number?,
    sourceW: number?, sourceH: number?,
    sourceWidth: number?, sourceHeight: number?,
    slice_left: number,
    slice_right: number,
    slice_top: number,
    slice_bottom: number,
    sliceLeft: number?,
    sliceRight: number?,
    sliceTop: number?,
    sliceBottom: number?,
}
```

The four slice values are source-pixel edge widths. Corners remain fixed, edges
stretch along one axis, and the center stretches in both. When destination
bounds are too small, edge sizes scale down proportionally. All-zero slices
draw a normal sprite. Snake case is canonical with camel fallback.
`core["9SliceSprite2D"]` references the same prototype; dot syntax cannot start
an identifier with a digit.

## `core.TileTexture2D`

```luau
export type TileTexture2D = BaseDrawableComponent & {
    image: ImageHandle?,
    tile_width: number,
    tile_height: number,
    offset_x: number,
    offset_y: number,
}
```

Repeats the whole image across entity bounds. Width/height `0` use source image
dimensions; defaults are zero. Tile size and offset scale with the entity. The
phase is anchored to the parent world position when parented, allowing sibling
tiles to align. Partial edge tiles use clipped source rectangles. Iteration is
culled in layer-local space, including rotation.

## `core.Tilemap2D`

```luau
export type Tilemap2D = BaseDrawableComponent & {
    image: ImageHandle?,
    map_width: number,
    map_height: number,
    tile_width: number,
    tile_height: number,
    tiles: string | { number },
    spacing: number,
    margin: number,
}
```

Defaults: one-by-one map, 32×32 atlas tiles, tile string `"0"`, zero spacing
and margin, and no image. `tiles` is a row-major flat numeric array or a string
split on commas/whitespace. Tile id `0` is the first atlas cell; negative or
out-of-range ids draw nothing. Missing map entries behave as `-1`.

The map fills the entity bounds, so rendered cell size is entity width/height
divided by map dimensions; `tile_width`/`tile_height` describe source atlas
cells. Large maps calculate visible row/column ranges in local space. The
editor's tile paint mode edits the same flat `tiles` field.

<!-- page: spritebox | Spritebox2D -->
# Spritebox2D

`core.Spritebox2D` builds a cached geometric cover of opaque pixels from a
`Sprite2D`, `Image2D`, or `NineSliceSprite2D` on the same entity.

```luau
export type Spritebox2D = ComponentInstance & {
    computed: boolean,
    alpha_threshold: number,
    rect_count: number,
    bounds_x: number,
    bounds_y: number,
    bounds_w: number,
    bounds_h: number,
    ComputeSpritebox: (self: Spritebox2D) -> boolean,
    computeSpritebox: (self: Spritebox2D) -> boolean,
    IsInside: (self: Spritebox2D, x: number, y: number) -> boolean,
    isInside: (self: Spritebox2D, x: number, y: number) -> boolean,
    IsIntersecting: (self: Spritebox2D, other: Entity | Spritebox2D) -> boolean,
    isIntersecting: (self: Spritebox2D, other: Entity | Spritebox2D) -> boolean,
}
```

Defaults: not computed, threshold `0`, zero rectangles and bounds.

| Canonical method | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `spritebox:ComputeSpritebox()` (`computeSpritebox` alias) | None. | `true` after a successful scan. | Reads the supported sibling image/source rectangle, clamps threshold to `0..255`, scans alpha, and merges opaque pixels into normalized rectangles. It raises when detached, no supported source component/live image exists, or pixels cannot be read. An all-transparent image is still a successful empty result (`true`, `computed = true`, `rect_count = 0`). Replaces the previous cache atomically enough for later queries. |
| `spritebox:IsInside(x, y)` (`isInside` alias) | World-space point. | `boolean`. | Tests cached opaque rectangles after current entity hierarchy size/scale/rotation. Returns false when not successfully computed or when the cache is empty. Boundary handling follows polygon containment and can differ at floating-point edges. |
| `spritebox:IsIntersecting(other)` (`isIntersecting` alias) | Another `Spritebox2D` or entity containing one. | `boolean`. | Uses an AABB broad phase then SAT on both cached rectangle sets at their live transforms. Missing/uncomputed/empty other masks return false; malformed targets raise or fail lookup. A spritebox does not intersect itself meaningfully for gameplay—avoid self-pairs. |

Recompute after changing the image pixels, source rectangle, alpha threshold,
nine-slice settings, or destination size when exact nine-slice shape matters.
This is a gameplay query shape: it is not used by `transform.raycast`,
`doTheyOverlap`, or Rigidbody physics.

```luau
local mask = ship:AddComponent(core.Spritebox2D)
mask.alpha_threshold = 32
assert(mask:ComputeSpritebox())

if mask:IsInside(mouse.x, mouse.y) then
    print("opaque ship pixel")
end
if mask:IsIntersecting(asteroid) then
    print("pixel-shape overlap")
end
```

<!-- page: physics | Physics Components -->
# Physics Components

Physics is rebuilt from entity/component state when structural inputs change,
then stepped with `dt` clamped to at most `0.25` seconds. Entity position and
rotation are synchronized back from Rapier after each step.

## `core.Collider2D`

```luau
export type CollisionCallback = (
    selfEntity: Entity,
    selfCollider: Collider2D,
    otherEntity: Entity?,
    otherCollider: Collider2D?,
    otherId: number
) -> ()

export type Collider2D = ComponentInstance & {
    enabled: boolean,
    is_trigger: boolean,
    non_physics: boolean,
    offset_x: number,
    offset_y: number,
    size_x: number,
    size_y: number,
    shape: "box" | "circle" | "triangle" | "right_triangle" | "righttriangle" | "rightangledtriangle" | string,
    triangle_corner: TriangleCorner,
    restitution: number,
    friction: number,
    touching: boolean,
    last_hit_id: number,
    onCollisionEnter: CollisionCallback?,
    onCollisionStay: CollisionCallback?,
    onCollisionExit: CollisionCallback?,
    onTriggerEnter: CollisionCallback?,
    onTriggerStay: CollisionCallback?,
    onTriggerExit: CollisionCallback?,
    on_collision_enter: CollisionCallback?,
    on_collision_stay: CollisionCallback?,
    on_collision_exit: CollisionCallback?,
    on_trigger_enter: CollisionCallback?,
    on_trigger_stay: CollisionCallback?,
    on_trigger_exit: CollisionCallback?,
    setOnCollisionEnter: (self: Collider2D, callback: CollisionCallback?) -> (),
    setOnCollisionStay: (self: Collider2D, callback: CollisionCallback?) -> (),
    setOnCollisionExit: (self: Collider2D, callback: CollisionCallback?) -> (),
    setOnTriggerEnter: (self: Collider2D, callback: CollisionCallback?) -> (),
    setOnTriggerStay: (self: Collider2D, callback: CollisionCallback?) -> (),
    setOnTriggerExit: (self: Collider2D, callback: CollisionCallback?) -> (),
}
```

Defaults: enabled, non-trigger, physical, zero offset/size, box, bottom-left
corner, restitution `-1`, friction `0.45`, not touching, hit id `0`. A
non-positive component dimension uses entity size. Circle uses half the smaller
dimension. Unknown shapes fall back to box.

Restitution `-1` inherits the Rigidbody value; non-negative values clamp
`0..1`. Friction clamps at zero. `is_trigger` and `non_physics` both create a
sensor with no physical response; trigger-vs-collision callback choice follows
whether either collider is a trigger.

`touching` and `last_hit_id` reset each update and reflect the most recent
active pair. Enter fires for a new pair, stay for an existing pair, and exit
after separation. Camel callback fields take precedence; snake fields are
fallback aliases. The setter methods assign camel fields.

Each setter takes either a `CollisionCallback` or `nil`, returns nothing, and
replaces only its matching camel-case field:

| Method | Callback phase |
| --- | --- |
| `collider:setOnCollisionEnter(callback?)` | First frame of a non-trigger contact pair. |
| `collider:setOnCollisionStay(callback?)` | Later frames while that collision pair persists. |
| `collider:setOnCollisionExit(callback?)` | First update after that collision pair ends/disables. |
| `collider:setOnTriggerEnter(callback?)` | First frame where either member of a sensor/trigger pair overlaps. |
| `collider:setOnTriggerStay(callback?)` | Later overlapping trigger frames. |
| `collider:setOnTriggerExit(callback?)` | First update after trigger overlap ends/disables. |

Passing nil clears the callback. Every callback receives the owning entity and
collider, then the other entity/collider when still resolvable, plus `otherId`
which remains available for exit/stale-partner correlation. `otherEntity` and
`otherCollider` can be nil on an exit after deletion. Callback returns are
ignored; callback errors are reported during physics event delivery and may
interrupt later callbacks that frame.

```luau
local collider = player:AddComponent(core.Collider2D)
collider:setOnCollisionEnter(function(selfEntity, selfCollider, otherEntity, otherCollider, otherId)
    print("hit", otherEntity and otherEntity.name or otherId)
end)
collider:setOnCollisionExit(nil) -- explicitly no exit listener
```

## `core.Rigidbody2D`

```luau
export type RigidbodyBoundsMode = "none" | "window"

export type Rigidbody2D = ComponentInstance & {
    velocity_x: number, velocity_y: number,
    force_x: number, force_y: number,
    acceleration_x: number, acceleration_y: number,
    gravity_x: number, gravity_y: number,
    gravity_scale: number,
    mass: number,
    inertia: number,
    linear_damping: number,
    angular_damping: number,
    restitution: number,
    friction: number,
    sleep_epsilon: number,
    bounds_mode: RigidbodyBoundsMode | string,
    freeze_x: boolean,
    freeze_y: boolean,
    freeze_rotation: boolean,
    is_static: boolean,
    collision_enabled: boolean,
    grounded: boolean,
    max_speed: number,
    max_angular_speed: number,
    angular_velocity: number,
    torque: number,
    addForce: (self: Rigidbody2D, fx: number, fy: number) -> (),
    addImpulse: (self: Rigidbody2D, ix: number, iy: number) -> (),
    addTorque: (self: Rigidbody2D, torque: number) -> (),
    addAngularImpulse: (self: Rigidbody2D, impulse: number) -> (),
    setVelocity: (self: Rigidbody2D, vx: number, vy: number) -> (),
    getVelocity: (self: Rigidbody2D) -> (number, number),
    setAngularVelocity: (self: Rigidbody2D, omega: number) -> (),
    getAngularVelocity: (self: Rigidbody2D) -> number,
    setGravity: (self: Rigidbody2D, gx: number, gy: number) -> (),
}
```

### Defaults

| Group | Defaults |
| --- | --- |
| Linear state | velocity, force, acceleration `0,0` |
| Gravity | `0,980`, scale `1` |
| Mass/inertia | `1`, `0` (automatic inertia) |
| Damping | linear `0`, angular `0.5` |
| Material | restitution `0.25`, friction `0.45` |
| Sleep | epsilon `1` |
| Constraints | bounds `none`; all freeze flags `false` |
| Body mode | dynamic, collisions enabled, not grounded |
| Limits | max linear/angular speed `0` (unlimited) |
| Angular state | velocity and torque `0` |

Forces and torque accumulate until the physics step, then reset. Impulses change
velocity immediately using body mass/inertia. `set...` methods overwrite state.
Acceleration and scaled gravity are continuous. Static bodies force velocities
to zero. Freeze flags constrain axes. Positive speed limits clamp motion;
zero disables a limit. `grounded` is derived from contact normal. Window bounds
mode keeps the body's bounds inside the current logical window.

`collision_enabled = false` omits its collider from solving. A Collider without
a Rigidbody is represented as a static body. A Rigidbody may exist without a
Collider and still integrate motion.

### Rigidbody method reference

All force, impulse, velocity, gravity, torque, and angular values are in the
engine's world-unit/second convention and return no status unless stated.

| Method | Parameters | Returns | Behavior and edge cases |
| --- | --- | --- | --- |
| `body:addForce(fx, fy)` | Force components. | `()` | Adds to `force_x/y`; multiple calls accumulate until the physics step consumes/resets them. Static/frozen axes may prevent resulting movement, but the call still updates stored force before stepping. |
| `body:addImpulse(ix, iy)` | Linear impulse components. | `()` | Immediately adds impulse divided by `max(mass, 0.0001)` to velocity. Speed limits/freeze/static constraints are enforced by the physics update, not necessarily by this immediate write. |
| `body:addTorque(torque)` | Scalar torque. | `()` | Accumulates into `torque` until the next step. Positive direction follows entity rotation convention. |
| `body:addAngularImpulse(impulse)` | Scalar angular impulse. | `()` | Immediately adds impulse divided by inertia; non-positive automatic inertia falls back to mass, each floored at `0.0001`. Later freeze/static/speed limits can suppress it. |
| `body:setVelocity(vx, vy)` | Linear velocity components. | `()` | Directly overwrites both fields. Does not clear force/acceleration. Non-finite values are invalid physics input. |
| `body:getVelocity()` | None. | Current `vx, vy`; missing/malformed fields fall back to zero internally. | Snapshot before the next integration/contact correction. |
| `body:setAngularVelocity(omega)` | Angular radians-per-second value. | `()` | Overwrites angular velocity without clearing torque. |
| `body:getAngularVelocity()` | None. | Current angular velocity number, falling back to zero for a missing field. | Snapshot only. |
| `body:setGravity(gx, gy)` | Gravity acceleration components. | `()` | Overwrites per-body gravity before `gravity_scale`; does not modify other bodies or current velocity. |

```luau
local body = ball:AddComponent(core.Rigidbody2D)
body.mass = 2
body:addImpulse(240, -400)
body:addTorque(60)
local vx, vy = body:getVelocity()
print("launched at", vx, vy)
```

## `core.Bolt2D` and `core.LegacyBolt2D`

```luau
export type Bolt2D = ComponentInstance & {
    enabled: boolean,
    target_entity: Entity?,
    target: Entity?,
    x: number, y: number,
    offset_x: number, offset_y: number,
    strength: number,
    contacts_enabled: boolean,
    current_force: number,
    force: number,
    attach: (self: Bolt2D, targetEntity: Entity) -> (),
    link: (self: Bolt2D, targetEntity: Entity) -> (),
}
export type LegacyBolt2D = Bolt2D
```

Defaults: enabled, no target, zero target-local offset, strength `1`, contacts
enabled, and zero derived force. `target_entity` and `target` are aliases;
`x/y` and `offset_x/y` are aliases. `attach` and `link` set both target fields.
Strength clamps to `0..1`.

Bolt2D pins the owner's rotation pivot to an offset from the target rotation
pivot. Low non-zero strength preserves the point while allowing rotation;
higher strength increasingly resists relative rotation, and `1` locks it.
LegacyBolt2D instead uses the previous spring-like positional motor where
intermediate strength may lag. `current_force` and `force` are derived aliases.

### `bolt:attach(targetEntity) -> ()` (`link` alias)

`targetEntity` must be a live entity table. The method stores it in both
`target_entity` and compatibility field `target` and returns nothing. It does
not change offsets, strength, enabled state, or the target's hierarchy. A stale
target later makes the constraint ineffective; reattach to a live entity.

```luau
local bolt = wheel:AddComponent(core.Bolt2D)
bolt.offset_x, bolt.offset_y = 24, 0
bolt.strength = 0.8
bolt:attach(chassis)
```

## `core.Rope2D` and `core.String2D`

```luau
export type Rope2D = ComponentInstance & {
    enabled: boolean,
    entity_a: Entity?,
    entity_b: Entity?,
    min_length: number,
    max_length: number,
    stiffness: number,
    damping: number,
    break_force: number,
    current_length: number,
    tension: number,
    snapped: boolean,
    link: (self: Rope2D, entityA: Entity, entityB: Entity) -> (),
}
```

Defaults: enabled, no endpoints, minimum `0`, maximum `160`, stiffness `0.82`,
damping `0.08`, no break threshold, and zero derived state. `link` assigns
endpoints and clears `snapped`. The global physics step enforces the distance
range. `break_force = 0` means unbreakable; exceeding a positive threshold
disables and snaps the rope. `String2D` is the same prototype.

### `rope:link(entityA, entityB) -> ()`

Both parameters must be live endpoint entities. The method assigns
`entity_a`/`entity_b`, clears `snapped`, and returns nothing. It preserves the
current lengths, stiffness, damping, break threshold, and `enabled` flag—set
`enabled = true` yourself when relinking a rope that broke. Linking an entity to
itself yields a zero-distance constraint and is rarely useful. Deleting an
endpoint makes solving skip/invalidates that link until relinked.

```luau
local rope = ropeController:AddComponent(core.Rope2D)
rope.min_length, rope.max_length = 40, 180
rope.break_force = 900
rope:link(grapplingHook, player)
```

<!-- page: rendering | Rendering Details -->
# Rendering Details

## Software and Vulkan presenters

The default native build rasterizes into a CPU buffer and presents it through
`softbuffer`. A `vulkan` feature build first tries Vulkan and falls back to the
software presenter with a diagnostic if initialization fails.

Custom shader draw commands cannot run in the native software renderer. Other
geometry, images, text, nine-slice, tiles, and UI components work in both.

## Filtering

`app.nearestNeighborScaling = true` uses nearest-neighbor filtering for crisp
pixel art. `false` uses linear filtering. Panel backgrounds, UI icons, sprites,
tiles, particles, and custom shader textures use the same global choice.

## Anti-aliasing

| Mode | Software 2D | Software 3D | Vulkan geometry / 3D shaders | WebGL custom 3D shaders | Text |
| --- | --- | --- | --- | --- | --- |
| `off` | Hard single-sample edges. | Center-sampled triangle coverage; no 3D scratch surface. | 1× rasterization. | Context antialiasing disabled; 1× surface. | Hard masks. |
| `standard` | 2× edge coverage. | Light depth/luminance-aware edge smoothing. | Requests 2× MSAA, falling back to 1× if unsupported. | Requests browser WebGL multisampling at 1× surface resolution. | Normal grayscale glyph rasterization. |
| `high` | 4× geometry edge coverage. | Stronger depth/luminance-aware diagonal smoothing. | Requests 4× MSAA, then 2×/1× according to device support. | Browser multisampling plus a 2×-per-axis shader surface when renderbuffer limits allow; otherwise standard surface resolution. | 2× supersampled glyphs with premultiplied downsampling. |

Ordinary unshaded web meshes use the software 3D column. The software 3D pass
runs after meshes and 3D particles but before the established 2D command
stream, so a canvas, HUD, or editor overlay is not blurred by mesh AA. Vulkan
sample-count changes recreate the compatible render pass, depth/MSAA
attachments, built-in pipeline, and cached custom fragment pipelines. Text is
rasterized before upload and individual text components may override the global
mode.

## Transforms and ordering

Entity local position is first combined with parent anchors and position pivot,
then parent scale and rotation. Size uses cumulative scale. Rotation pivots
affect the entity and descendants.

Rendering updates sort by `z`, entity id, and component order. Equal z therefore
remains deterministic. UI popup menus are drawn into an overlay command list
after normal commands.

## Text caching

Text layout and raster sprites are cached by content, font, style, ranges, and
bounds. The sprite cache is bounded to 256 entries. Changing a relevant field
generates a new cache id; frequently changing large text can therefore be more
expensive than moving an already laid-out entity.

<!-- page: web | WebAssembly Runtime -->
# WebAssembly Runtime

Build with `neolove build --webasm`, then serve `dist/webasm` over HTTP(S).

## Browser differences

- The game runs through Emscripten and a browser animation loop.
- `window` and `mouse` are read-only proxy tables. Public `x/y` fields remain
  cross-platform; the web proxy additionally understands window
  `width/height` and mouse delta aliases as implementation conveniences.
- HTTP uses `fetch` and browser CORS/security policy.
- `commands.run` and `runDetached` return unsupported result records.
- Native file/folder pickers return `nil`.
- The browser's virtual filesystem backs bundled resources and writes.
- Audio decoding/playback uses browser facilities and may require a user
  gesture. AAC/M4A and AIFF work when the browser can decode them.
- Server hosting/connect helpers are unavailable in this build.
- Fragment shaders run through WebGL on supported draw types.
- Keyboard and mouse names match the Input reference.

Use relative URLs/assets and avoid assuming host filesystem paths. Persisted
browser storage depends on the host page and Emscripten filesystem setup.

<!-- page: android | Android Runtime -->
# Android Runtime

The Android package embeds resources in `neolove_project.payload` and enters
through a native activity. It is arm64-only and uses minimum SDK 24.

## Runtime behavior

- `android.isAndroid()` and `fs.isAndroid()` are true.
- `mobile.isMobile()` is true.
- `android` metadata getters expose values gathered from the Java/Android
  environment when available.
- Input and TextInput may request the system soft keyboard.
- File and folder pickers return `nil`.
- The native app sandbox still limits otherwise absolute filesystem paths.
- The current safe-area model returns portrait top/bottom insets documented by
  `mobile.getSafeAreaInsets`.

If the first build must provision its toolchain, ensure there is enough disk
space and network access for the JDK, SDK, NDK, build tools, and Rust target.

<!-- page: ios | iOS Simulator Runtime -->
# iOS Simulator Runtime

The iOS target is a simulator `.app` built on macOS with Xcode. The project
payload is bundled with the app. `mobile.isMobile()` is true and
`android.isAndroid()` is false.

This target is intended for simulator testing. It does not create a signed
device archive, provisioning profile, TestFlight upload, or App Store package.
iOS sandbox restrictions apply to filesystem paths, and platform features
which are implemented specifically through Android return unavailable values.

<!-- page: performance | Performance Guidance -->
# Performance Guidance

- Reuse `ImageHandle`, `SoundHandle`, fonts, shaders, prefab templates, and
  animation clips. Path loads already cache assets, but retaining handles makes
  lifetime explicit.
- Call `assets.gc()` after unloading a large batch, not every frame.
- Prefer sprite sheets and tilemaps over thousands of independent entities.
- Tilemap and TileTexture rendering already cull off-screen local ranges.
- Keep particle `max_particles` bounded and use an emission rate appropriate to
  particle lifetime.
- Avoid calling `snapPhoto`, pixel mutation/upload, filesystem functions,
  process functions, or sync asset decoders every frame.
- `async` helps distribute Luau work but does not parallelize it; yield between
  bounded chunks.
- Use `Spritebox2D` only where pixel-shaped queries matter. Recompute only after
  its source changes.
- Cache frequently used entity/component references instead of repeatedly
  searching hierarchy tables.
- Large amounts of changing rich text create layout/raster work; update text
  only when its content changes.
- `maxFps` can reduce CPU/GPU use; mobile low-power state is advisory simulation
  and does not automatically change engine quality.
- Keep physics collider shapes simple and use static bodies for immovable
  geometry.

<!-- page: troubleshooting | Troubleshooting -->
# Troubleshooting

## `neolove run` or `build` says `main.luau` is missing

Run from the project directory or pass it explicitly. The editor can open
without an entry point; use **Export** to generate one.

## The editor runs a different scene

Check `[project].start_scene` in `neolove.toml`. The path must remain inside the
project and use `.neoscene`. Saving another tab does not change the configured
start scene.

## Loading a scene deleted my entities

This is current behavior: `ecs.loadScene` clears every non-root runtime entity
before instantiating the requested scene. Use prefabs when you want to add a
subtree without replacing the scene.

## A custom component read prototype defaults in `awake`

Use the second callback argument (`component` or `self`), not the module table.
The engine deep-copies the prototype and the editor writes Inspector values onto
that instance before deferred custom `awake` runs.

## A UI callback's value argument is a component table

Widget callbacks receive `(entity, component, ...)`. For example:

```luau
slider.onChanged = function(entity, component, value)
    print(value)
end

dropdown.onChanged = function(entity, component, index, value)
    print(index, value)
end
```

## An image or sound cannot be found in a packaged game

Use project-relative paths and exact filename case. Reads check the writable
data override first, then embedded resources. Avoid constructing paths from the
temporary extracted resource location.

## Export wrote to an unexpected directory

Relative `fs`, image, and sound writes use `fs.getDataDirectory()`. Use
`fs.dataPath(relative)` to inspect the resolved destination. Absolute and
parent-relative paths intentionally follow OS path rules.

## A command working directory is rejected

Unlike filesystem writes, command `cwd` is confined to the project. Pass `.` or
a project-contained subdirectory and put each argument in the argument array.

## Browser requests fail but native requests work

Check CORS, mixed-content rules, HTTPS certificates, and the browser console.
NeoLOVE cannot bypass browser origin policy.

## Browser audio is silent

Start playback after a click/tap. Browser autoplay policy may suspend audio
created before user interaction.

## A custom shader fails on desktop

Install/build NeoLOVE with `--features vulkan`. If Vulkan initialization falls
back to software, inspect the warning for driver/runtime details. Check
`shaders.supports3DShaders()` before assigning an optional 3D material, and
remember that parsing/driver compilation is lazy: a syntax error is reported
when the shader is first drawn. Custom vertex stages are not currently applied;
use `load3DFragment` for the supported mesh-material path.

## A table or component field seems absent from autocomplete

Run `neolove api` after upgrading. The runtime has a few implemented fields and
aliases which are documented here before they appear in the generated file;
the component definitions in this manual are the behavior reference.

## The engine cannot open a window on Linux

Launch within an X11 or Wayland graphical session. Sandboxes must expose
`DISPLAY` or `WAYLAND_DISPLAY` and the corresponding socket.

## Android build setup fails

Verify free space, network access, archive tools, and write permission under
`~/.neolove/toolchains`. Re-run the build after correcting the reported failed
download or tool invocation.

<!-- page: api-index | Complete API Index -->
# Complete API Index

The following checklist provides a compact audit of the supported surface.
Every public entry has a full definition on its linked conceptual page; raw
tooling declarations are deliberately not reproduced in this manual.

## Runtime and editor-declaration names

`Color4`, `Inspector`, `IComponentPicker`, `IEntity`, `IComponent`, `IImage`,
`IAudio`, `IShader`, `IAnimation`, `die`, `softrequire`, `print`, `require`,
`app`, `input`, `userInput`, `assets`, `audio`, `media`, `fs`, `android`, `mobile`,
`http`, `commands`, `command`, `servers`, `shaders`, `ecs`, `prefabs`, `prefab`,
`tweening`, `tween`, `animation`, `animations`, `transform`, `transforms`,
`core`, `lighting`, `Rng`, `async`, `mouse`, and `window`.

## Handle and record types

`Color4Value`, `Vec2`, `Entity`, `Connection`, `EntityListenInfo`, `System`,
`Component`, `ComponentInstance`, `ImageHandle`, `SoundHandle`, `ShaderHandle`,
`MediaPermissionStatus`, `MediaDeviceKind`, `MediaEnumerationKind`, `MediaDevice`,
`MediaAudioConstraints`, `MediaVideoConstraints`, `MediaRequestOptions`,
`MediaAudioFormat`, `MediaVideoFormat`, `MediaAudioSamples`, `MediaAudioBytes`,
`MediaVideoFrame`, `MediaStream`, `MediaDeviceResult`, `MediaAccessResult`,
`RngInstance`, `AsyncTask`, `HttpRequestOptions`, `HttpResponse`, `CommandRunResult`,
`CommandDetachedResult`, `ServerClientHandle`, `HostedServerHandle`,
`ServerPeer`, `ServerService`, `RaycastHit`, `RaycastOptions`, `PrefabTemplate`,
`TweenHandle`, `AnimationKeyframe`, `AnimationTrack`, `AnimationClip`, and
`AnimationHandle`.

## Core prototypes

`Rect2D`, `Light2D`, `LightOccluder2D`, `EntityScaler`, `Camera`, `Shape2D`,
`ParticleSystem2D`, `AnimationController`,
`SpatialSound2D`, `TextBox`, `TextLabel`, `RudimentaryTextLabel`, `TextInput`,
`Panel`, `Frame`, `Button`, `Slider`, `Dropdown`, `Sprite2D`, `Image2D`,
`SpriteSheet2D`, `NineSliceSprite2D`, `9SliceSprite2D`, `TileTexture2D`,
`Tilemap2D`, `Spritebox2D`, `Collider2D`, `Rigidbody2D`, `Bolt2D`,
`LegacyBolt2D`, `Rope2D`, and `String2D`.

## Exposed engine-managed names

| Name | Owner | Purpose |
| --- | --- | --- |
| `_poll` | `http`, `media`, `servers` | Drains asynchronous callbacks; called by the runtime. |
| `_update` | `tweening`, `animation` | Automatic per-frame advance. |
| `_registry` | `prefabs` | Internal registered-template table. |
| `_hostClass` | `servers` | Raw inline class host constructor used by `servers.define`. |
| `__neolove_component` | component | Runtime/editor component kind tag. |
| `__neolove_core_component` | core component | Marks immediate core initialization. |
| `__neolove_entity_listen_impl` | global | Backend used by entity `listen`; not a user API. |

Additional visible engine state is exhaustive below:

| Owner | Engine-managed fields |
| --- | --- |
| Entity with physics | `__neolove_physics_component_count`, `__neolove_has_physics_components` |
| `SpatialSound2D` | `__autoplay_started`, `__playing` |
| `ParticleSystem2D` | `__particles`, `__emit_accumulator`, `__elapsed`, `__manual_emit`, `__rng` |
| `AnimationController` | `__player` |
| `TextBox` / `TextInput` | `__rich_text_ranges`, `__letter_bounds`, `__letter_caret_start`, `__letter_caret_end`, `__layout_cache_id` |
| `SpriteSheet2D` | `__frame_time` |
| `Spritebox2D` | `__spritebox_rects`, `__spritebox_shape`, `__spritebox_revision`, `__spritebox_world_shape`, `__spritebox_world_revision`, `__spritebox_world_x`, `__spritebox_world_y`, `__spritebox_world_rotation`, `__spritebox_world_w`, `__spritebox_world_h` |
| `Collider2D` | `__prev_collision_ids`, `__prev_trigger_ids` |
| Dropdown | `wheel_scroll_accumulator` |
| `async` / web proxy metatables | `__call` / `__index` |
| Server class/event wrapper | `__neolove_service`, `__neolove_event` packet marker |

Do not invoke or persist engine-managed names directly. They are listed so a
full table inspection is not mistaken for an undocumented gameplay feature.
