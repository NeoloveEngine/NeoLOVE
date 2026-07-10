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
- the complete generated Luau declaration file in the final API appendix.

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
| Assets and sound | `assets`, `audio` |
| Files and processes | `fs`, `commands`, `command` |
| Networking | `http`, `servers` |
| Gameplay helpers | `async`, `prefabs`, `prefab`, `tweening`, `tween`, `animation`, `animations` |
| Rendering | `shaders`, `lighting` |
| Global helpers | `Color4`, `Inspector`, `die`, `softrequire`, `Rng` |
| Editor declaration names | `IComponentPicker`, `IEntity`, `IComponent`, `IImage`, `IAudio`, `IShader`, `IAnimation` |

NeoLOVE also installs project-relative `require`, and replaces `print` with a
tab-separated logger that writes to stdout and mirrors output to the visual
editor logger when the game is launched from the editor.

<!-- page: install | Installation -->
# Installation

## Requirements

- A current stable Rust toolchain for manual builds.
- Linux: ALSA and `pkg-config` development packages.
- Vulkan is optional. The default desktop build uses the software renderer.

On Debian or Ubuntu:

```sh
sudo apt-get install pkg-config libasound2-dev
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

Desktop export first builds a compact packaged runtime, then appends a
compressed copy of the project. At launch the runtime extracts resources to a
temporary resource directory and uses `<game>_data` beside the executable for
writes.

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

## Viewport tools

- Move, scale, rotate, and combined transform gizmos.
- Grid display and snapping.
- Mouse-wheel zoom and panning.
- Frame selected, frame all, reset view, and 100% zoom.
- Multi-selection, grouping, visibility isolation, locking, alignment, z-order,
  size normalization, and window-fit operations.
- Tile painting while a `Tilemap2D` is selected.

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
transform fields, optional parent id, active state, and components.

`.neoanim` is JSON and uses the same `AnimationClip` shape documented in the
animation API.

At runtime, scene loading generates Luau from the document. It omits inactive
entities and every descendant of an inactive entity, creates parents before
children, requires each unique script component module once, and reuses each
unique image path. Core properties and Inspector values are assigned to the
new component instances; entity/component references are resolved after their
targets exist. Scene background, image filtering, and anti-aliasing are written
to `app`.

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
| `autosave_before_run` | boolean | `true` |
| `autosave_before_build` | boolean | `true` |
| `mobile_emulator` | boolean | `false` |
| `mobile_orientation` | `portrait` or `landscape` | `portrait` |
| `mobile_wifi` | boolean | `true` |
| `mobile_cellular` | boolean | `false` |
| `mobile_low_power` | boolean | `false` |

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
3. deliver completed HTTP and server work;
4. resume each unfinished `async` task once;
5. run pending custom-component `awake` callbacks;
6. advance tween and animation players;
7. dispatch entity listeners;
8. resolve `app.bg` and anti-aliasing;
9. run system `update` callbacks in registration order;
10. run newly queued component `awake` callbacks;
11. run non-rendering component `update` callbacks in entity/component order;
12. simulate rigidbodies, colliders, bolts, and ropes;
13. run rendering-component updates in stable draw order; and
14. submit queued draw commands to the selected presenter.

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

Each channel is clamped to `0..255` and converted to a byte. Alpha defaults to
`255`.

## `die`

```luau
die(reason: string?) -> ()
```

Requests a clean runtime exit. An absent or blank reason becomes
`"die() called"`.

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

## `print` and `require`

```luau
print(...any) -> ()
require(modulePath: string) -> any
```

`print` applies `tostring`, joins arguments with tabs, writes one line to
stdout, and forwards it to the editor logger. `require` is the mlua text-module
loader rooted at the project.

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
| `antiAliasing` | `high` | Global geometry and default text quality. The runtime also reads lowercase `antialiasing` as a fallback. |

## Functions

| Function | Behavior |
| --- | --- |
| `setMaxFps(fps?)` | Stores a positive finite cap. `nil`, non-positive, NaN, or infinite input clears it. |
| `getMaxFps()` | Returns the valid current cap or `nil`. |
| `setShowFps(enabled?)` | Enables or disables the counter; omitted input means `true`. |
| `getShowFps()` | Returns the current value, defaulting to `true`. |
| `setNearestNeighborScaling(enabled?)` | Sets filtering; omitted input means `true`. |
| `getNearestNeighborScaling()` | Returns the current value, defaulting to `true`. |
| `setAntiAliasing(mode?)` | Sets `off`, `standard`, or `high`; omitted/unknown input becomes `high`. |
| `getAntiAliasing()` | Returns the stored mode, defaulting to `high`. |

Replacing the global `app` table is supported: the functions look up the
current table when called. Desktop frame pacing also reads the current table.
The parser maps `none`, `disabled`, and `pixel` to `off`; `fast`, `normal`, and
`on` to `standard`; and every other string to `high`.

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

## State functions

| Function | Meaning |
| --- | --- |
| `isKeyDown(key)` | Key is currently held. |
| `isKeyPressed(key)` | Key transitioned down during this frame. |
| `isKeyReleased(key)` | Key transitioned up during this frame. |
| `isMouseDown(button?)` | Mouse button is held; omitted button is `left`. |
| `isMousePressed(button?)` | Mouse button transitioned down this frame. |
| `isMouseReleased(button?)` | Mouse button transitioned up this frame. |
| `getMouseWheel()` | Returns horizontal and vertical frame deltas. |
| `isScrollingIn()` | Vertical wheel delta is positive. |
| `isScrollingOut()` | Vertical wheel delta is negative. |
| `getScrollInAmount()` | Returns the signed vertical wheel delta despite the historical name. |
| `getMouseDelta()` | Returns cursor movement `dx, dy` for this frame. |
| `setMouseLocked(locked)` | Requests locked/grabbed cursor mode. |
| `isMouseLocked()` | Returns the requested lock state. |
| `getLastKeyPressed()` | Last mapped key pressed this frame, or `nil`. |
| `getCharPressed()` | Text character received this frame, or `nil`. |

Key and button strings are normalized by removing non-alphanumeric characters
and lowercasing, so `"Left Shift"`, `"left_shift"`, and `"leftshift"` match.

Supported cross-platform key names are `a` through `z`, `0` through `9`,
`space`, `escape`, `enter`, `tab`, `backspace`, `left`, `right`, `up`, `down`,
`leftshift`, `rightshift`, `leftcontrol`, `rightcontrol`, `leftalt`, `rightalt`,
`leftsuper`, `rightsuper`, and `f1` through `f12`. Mouse names are `left`,
`middle`, `right`, and on web `other`.

## On-screen keyboard

`showKeyboard` and `openKeyboard` are aliases. They request the Android soft
keyboard and default `implicit` to `true`. `hideKeyboard` and `closeKeyboard`
are aliases; `implicitOnly` defaults to `false`. All four return `true` when an
Android activity handled the request and `false` on unsupported platforms.

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

## Module functions

| Function | Result |
| --- | --- |
| `async(callback)` | Queues a coroutine and returns its handle. The callback starts next update. |
| `async.yield(...values)` | Yields the current coroutine until a later update. Yielded values follow normal coroutine semantics. |
| `async.count()` | Count of queued or suspended unfinished tasks. |
| `async.cancelAll()` | Marks every unfinished task cancelled and returns the number changed. |

## Handle fields and methods

`result` is the first return value; `results` stores all return values as a
1-based table; `getResult()` returns them as multiple values. `cancel()` returns
`true` only when it changed an unfinished task. Errors set `status = "error"`,
`done = true`, and `error`, and are also printed. Completed and cancelled tasks
cannot be resumed.

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

| Function | Behavior |
| --- | --- |
| `loadImage(pathOrBase64Png)` | Loads/caches a file or accepts raw PNG base64, `base64:...`, or a `data:image/png;base64,...` URI. |
| `loadImageBase64(base64Png)` | Explicit raw/base64 PNG loader. |
| `snapPhoto(x,y,x2,y2)` | Copies a clipped top-left/bottom-right rectangle from the most recently rendered frame. |
| `newImage(width,height,color?)` | Creates a mutable RGBA image; dimensions clamp to at most 65535 and color defaults opaque white. |
| `unloadImage(pathOrHandle)` | Unloads a cached path or handle; returns whether an asset changed. |

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
`r,g,b[,a]` channels, clamped to bytes. `setPixel` and `fill` modify the CPU copy; call `upload`
before expecting an already uploaded texture to reflect changes. `save` and
`export` are aliases and write PNG, adding `.png` when no extension is present.
A different extension raises an error.

## Sound functions

| Function | Behavior |
| --- | --- |
| `loadSound(path)` | Loads/caches decoded interleaved floating-point samples. |
| `newSound(sampleRate,channels,len,fill?)` | Creates at least `len` interleaved samples initialized to `fill` (default `0`), padding to a complete channel frame. |
| `unloadSound(pathOrHandle)` | Unloads a cached path or handle; returns whether an asset changed. |

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

On web, encoded browser-audio loads retain their encoded bytes for playback but
do not expose decoded editable samples: `sampleRate()`, `channels()`, and
`len()` report zero. Newly generated sounds still use the editable WAV path.

## Cache collection and path rules

`assets.gc()` removes cache entries whose weak handles no longer have any live
references and returns the numbers of image entries and sound entries removed.
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
export type AudioModule = {
    play: (sound: SoundHandle, looped: boolean?, volume: number?) -> (),
    playOnce: (sound: SoundHandle, volume: number?) -> (),
    stop: (sound: SoundHandle) -> (),
    setVolume: (sound: SoundHandle, volume: number) -> (),
    playSpatial: (sound: SoundHandle, x: number, y: number, looped: boolean?, volume: number?) -> (),
    setPosition: (sound: SoundHandle, x: number, y: number) -> boolean,
    setListenerPosition: (x: number, y: number) -> (),
}
```

| Function | Behavior |
| --- | --- |
| `play(sound,looped?,volume?)` | Starts or restarts non-spatial playback. Defaults: not looped, volume `1`. |
| `playOnce(sound,volume?)` | Equivalent to `play(sound,false,volume)`. |
| `stop(sound)` | Stops active playback associated with the handle. |
| `setVolume(sound,volume)` | Changes current playback volume. |
| `playSpatial(sound,x,y,looped?,volume?)` | Starts a 2D emitter at world coordinates. |
| `setPosition(sound,x,y)` | Moves an existing spatial emitter; returns `false` if none is active. |
| `setListenerPosition(x,y)` | Moves the 2D listener. |

Volume is clamped to `0..1`. Browser playback is subject to user-gesture
autoplay restrictions. `SpatialSound2D` is preferable when the emitter should
follow an entity automatically.

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

| Function | Result and path behavior |
| --- | --- |
| `isWebasm()` | `true` in the Emscripten browser target. |
| `isWebAssembly()` | Exact alias of `isWebasm`. |
| `isMobile()` | `true` on Android/iOS or in mobile emulation. |
| `isAndroid()` | `true` only on Android. |
| `openFilePicker()` | Native desktop file path, or `nil` when cancelled/unavailable. |
| `openFolderPicker()` | Native desktop folder path, or `nil` when cancelled/unavailable. |
| `getDataDirectory()` | Absolute/default writable root string. |
| `dataPath(path)` | Resolves `path` against data using normal absolute/relative rules. |
| `readFile(path)` | Reads UTF-8 text from data, then bundled resources. |
| `readBytes(path)` | Reads arbitrary bytes into a Luau string. |
| `writeFile(path,content)` | Replaces a data-root-relative file and creates parents. |
| `appendFile(path,content)` | Creates/appends and creates parents. |
| `exists(path)` | Tests the resolved read path. |
| `isFile(path)` | Tests whether the resolved read path is a file. |
| `isDir(path)` | Tests whether the resolved read path is a directory. |
| `createDir(path)` | Recursively creates a directory. |
| `walk(path?,recursive?)` | Lists path entries; defaults to data root and recursive `true`. |
| `rename(from,to)` | Renames between writable paths and creates destination parents. |
| `copy(from,to)` | Copies a file or directory tree from read resolution to writable resolution. |
| `removeFile(path)` | Removes one file; returns `false` when absent. |

Pickers return `nil` on web and Android. `removeFile` does not remove
directories. I/O errors otherwise raise with the operation and resolved path.

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

All metadata getters return `nil` outside Android or when the platform did not
provide the property. `getApiLevel` aliases `getSdkInt`. Keyboard functions
match the input-module aliases and defaults.

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

The network and low-power values are simulation state, not a live native
connectivity probe.

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

`run` waits, captures stdout/stderr, and uses status `-1` when the process could
not start or had no exit code. `runDetached` returns after spawning with stdio
disconnected; `pid` is `0` on failure. Empty command strings return an error
record instead of raising.

`cwd` defaults to the project root. Relative values resolve beneath it;
normalized values which escape the project raise an error. The command is
executed directly, not through a shell, so pass arguments as separate strings.

Web builds expose the same functions but always return `ok = false` with
`"commands are not available in web builds"`. Their shared unsupported record
also includes `statusCode = status_code = -1`, `pid = 0`, and empty
`stdout`/`stderr`, regardless of which function was called.

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

`request` and `get` return monotonically increasing request ids. They are the
same function, so both accept all three overloads. A bare URL uses GET. An
options table defaults `method` to GET. In the three-argument form, the first
URL overrides `options.url`; `options.method` still controls the method.
Callbacks are delivered at the beginning of a later frame. `ok` means no
transport error; inspect `status` for HTTP success. A failed transport may have
`status = nil`, empty body, and populated `error`.

Native builds support `http://` and `https://` with bundled WebPKI roots. Web
builds use browser `fetch` and obey CORS. `http._poll()` drains completed work
and is called automatically; calling it from gameplay can change callback
timing and is not recommended.

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

| Function | Behavior |
| --- | --- |
| `host(scriptPath,port,options?)` | Starts a separate low-level Luau server runtime from a project-contained script. Port `0` asks the OS for a free port. |
| `connect(url)` | Connects a low-level client; class-service `connect` wraps this with named events. |
| `define(definition)` | Decorates a class-like service table in place. The other three service constructor names are aliases. |
| `serializeTable(value)` | MessagePack-serializes supported Luau tables and buffers. Snake-case alias included. |
| `deserializeTable(payload)` | Restores a serialized table or raises when the root is not a table. |
| UUID helpers | Return lowercase standard UUID strings; v7 is time ordered. |
| `sha256(value)` | Returns a 64-character lowercase hex digest. |
| `sha128(value)` | Returns the first 128 bits as 32 lowercase hex characters. |
| `_poll()` | Delivers host/client work; engine-managed. |

Serialization accepts table roots containing nils, booleans, integers,
numbers, UTF-8 strings, buffers, and nested tables. Consecutive 1-based keys
round-trip as arrays; other key/value pairs round-trip as maps. Functions,
threads, userdata, and cyclic tables raise errors.

`host` binds `127.0.0.1` by default. Use `{ host = "0.0.0.0" }` for LAN
access, then clients connect to the machine's actual address. TLS requires both
certificate and private-key paths; camel and snake spellings are accepted, and
both files must stay inside the project.

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

The raw callbacks registered through `addCallback` receive buffers. Class
listeners receive decoded data. `on`/`once`/`onAny` return the registered
function; pass it to `off`. `disconnect` returns `false` if already disconnected.
`getKickReason` includes server kicks, remote closure messages, or `nil`.

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

`broadcast` excludes the internal host client and returns successful sends.
`getClients` likewise returns only connected non-host keys. `stop` is idempotent
and returns whether it stopped an active host. `emit` and `sendEvent` are added
only to class-service handles; those handles also receive `service`, pointing
to the decorated service definition.

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
    fromSource: (vertexSource: string, fragmentSource: string, options: ShaderLoadOptions?) -> ShaderHandle,
    fromFragmentSource: (fragmentSource: string, options: ShaderLoadOptions?) -> ShaderHandle,
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

`load` reads both project files; `loadFragment` supplies the built-in vertex
source; the `from...Source` variants compile provided strings. The current
runtime accepts the options shape for compatibility but does not need it to
create uniform slots. `DEFAULT_VERTEX_SHADER` exposes the built-in GLSL.

Float uniform storage is bounded to 16 named entries and texture uniforms to 4.
`setUniformColor` converts byte channels to normalized floats. `setTexture`
requires a live uploaded image.

Custom shaders require a Vulkan-feature desktop build. WebAssembly supports
fragment shaders for rectangles, primitive shapes, and images through WebGL;
the built-in sampler is named `Texture`.

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

| Function | Behavior |
| --- | --- |
| `setEnabled(enabled?)` | Turns the system on or off; omitted input means `true`. |
| `enable()` / `disable()` | Convenience wrappers for `setEnabled`. |
| `isEnabled()` | Returns the current toggle state. |
| `setAmbient(color, intensity?)` | Sets the base light color and, when given, its intensity (clamped to `>= 0`). |
| `setAmbientIntensity(intensity)` | Sets only the ambient intensity. |
| `getAmbient()` | Returns the ambient `Color4` and its intensity. |
| `setAmbientOcclusion(enabled?, radius?, intensity?, samples?)` | Toggles AO and optionally sets its radius (pixels), strength (`0..1`), and per-pixel sample count (`1..64`). |
| `setShadows(enabled?, softness?)` | Toggles occluder shadows; `softness` is the penumbra size in pixels (`0` is a hard shadow). |
| `setBloom(amount)` | Sets extra glow added where light exceeds full brightness. |
| `setExposure(value)` | Sets the output multiplier applied after lighting. |
| `setQuality(quality)` | Sets the light-map resolution: `low` (quarter), `medium` (half), `high` (full), `ultra` (full plus extra shadow/AO samples). |
| `getQuality()` | Returns the current quality string. |
| `sample(x, y)` | Returns the light color reaching a screen-space position as a `Color4`, or `nil` when the point is off-screen. `getAt` and `sampleAt` are aliases. Reads the **last completed frame's** lights/occluders, so it is safe to call from `update` while this frame's lights are still being queued. Returns opaque white when lighting is disabled (everything is effectively fully lit). |
| `reset()` | Restores every lighting setting to its default (which includes disabling the system). |

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
and mood.

## Performance notes

The software light pass is built to stay cheap: the light map is computed on
worker threads (one band of rows each), the final composite is likewise
parallelized, occluder rotations and bounds are resolved once per frame, ambient
occlusion is evaluated only near occluders, and **soft shadows are produced by
blurring the light map** rather than casting many penumbra rays per pixel — so
softness is nearly free.

To tune further: lower `setQuality` (`low` is quarter-resolution and much
cheaper; the blur keeps it smooth), reduce light `radius`, disable
`castsShadows` on fill lights, and prefer fewer shadow-casting **directional**
lights (they cover the whole screen). Ambient occlusion is the other main cost;
fewer `samples` or a smaller `radius` both help.

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

| Function | Result |
| --- | --- |
| `Rng.new(seed?)` | A generator seeded with `seed`, or entropy-seeded when omitted. |
| `Rng.fromString(text)` | A generator seeded from a stable hash of `text`. |
| `Rng(seed?)` | Callable shorthand for `Rng.new`. |

## Instance methods

| Method | Result |
| --- | --- |
| `next()` | Float in `[0, 1)`. |
| `number(min?, max?)` | `[0,1)` with no args, `[0,max)` with one, `[min,max)` with two. `float` and `range` are aliases. |
| `integer(min, max?)` | Inclusive integer; `integer(max)` means `[1, max]`. `int` is an alias. |
| `boolean(p?)` | `true` with probability `p` (default `0.5`). `bool` is an alias. |
| `sign()` | `-1` or `1`. |
| `angle()` | Radians in `[0, 2π)`. |
| `unit()` | A random unit vector `x, y`. |
| `pick(list)` | A uniformly random element of an array-like table, or `nil` when empty. |
| `shuffle(list)` | Shuffles the table in place (Fisher-Yates) and returns it. |
| `seed(n)` | Reseeds this generator in place. |
| `clone()` | An independent copy at the same position in the stream. |

Two generators created with the same seed produce identical sequences, which is
what makes seeded worlds and deterministic tests reproducible.

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
    root: Entity,
    addComponent: (entity: Entity, component: Component) -> ComponentInstance,
    removeComponent: (entity: Entity, target: number | ComponentInstance) -> boolean,
    loadScene: (path: string) -> (),
}
```

## Function behavior

| Function or variable | Behavior |
| --- | --- |
| `root` | Id `0` entity. Its `size_x`/`size_y` track the logical window. |
| `newEntity(name,parent?,x?,y?)` | Creates an entity. Omitted position is `0,0`; omitted parent leaves it unparented. |
| `deleteEntity(entity)` | Recursively removes the entity/descendants, detaches from its parent, and disconnects listeners. |
| `duplicateEntity(entity,parent)` | Captures and instantiates a deep prefab-style copy under `parent`. |
| `findFirstChild(parent,name)` | Returns the first direct child with an exact name. It is not recursive. |
| `addComponent(entity,prototype)` | Deep-copies a table prototype, attaches instance methods, runs core setup, and queues custom `awake`. |
| `removeComponent(entity,target)` | Removes by 1-based component index or exact instance; returns `false` if absent. |
| `addSystem(system)` | Immediately calls optional `awake`, then registers the table. |
| `loadScene(path)` | Reads a project `.neoscene`, clears all existing non-root entities/listeners, then instantiates the scene. |

`loadScene` preserves `ecs.root` but replaces its children. If parsing or
generated Luau execution fails, it raises a path-rich error.

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

Transform reads also accept camel aliases `anchorX/Y`, `pivotX/Y`,
`positionPivot`, `positionPivotX/Y`, and `rotationPivot/X/Y`. The older
`position_pivot_x/y` and boolean `rotation_pivot_middle` are accepted too.
Snake case is canonical. Named `topright` aliases `top_right`; unknown position
pivots fall back to top-left.

`Duplicate()` without a parent uses the current parent, falling back to
`ecs.root`. `IsInside` tests transformed bounds including hierarchy scale,
rotation, anchors, and pivots; boundary points count as inside.

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

`awake` runs synchronously in `addSystem`; `update` runs each frame. As noted in
Runtime Model, the other two declared callbacks are not currently scheduled.

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

| Function | Semantics |
| --- | --- |
| `getWorldPosition(entity)` | Returns transformed top-left position after parent scale/rotation, anchors, and pivots. |
| `getWorldRotation(entity)` | Sum of local rotations through the parent chain. |
| `lookAt(fromX,fromY,toX,toY)` | Radians facing the target; zero faces positive X and positive angles turn toward positive Y. `look_at` aliases it. |
| `GetEntitiesInFront(x,y,minimumZ?)` | All non-root transformed bounds containing the point, sorted descending z then descending id. Lower-camel alias included. |
| `doTheyOverlap(entities)` | `true` when any pair of global axis-aligned entity bounds overlaps. It ignores Spritebox masks and rotation polygon detail. |
| `raycast(...)` | Nearest AABB hit along a normalized direction. |

`raycast` returns `nil` for a zero/non-finite direction. Distance defaults to
infinity, is clamped to `0..1,000,000`, and negative becomes zero. Both ignore
fields accept one entity or an array and are combined. Entities with explicit
`raycastable = false`, non-positive global size, or id `0` are skipped. Normal
aliases carry identical numbers.

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

| Function | Behavior |
| --- | --- |
| `capture(entity)` | Deep-captures the entity subtree and internal references. |
| `component(source,overrides?)` | Deep-copies a component prototype and overlays keyed values. |
| `load(path)` | Parses a `.neoprefab` from data/resource resolution without instantiating it. |
| `register(name,source)` | Captures/loads a source and stores the template under an exact name. |
| `get(name)` | Returns the registered template or `nil`. |
| `remove(name)` | Removes a registration and reports whether it existed. |
| `instantiate(source,parent?)` | Resolves registered name/entity/template and creates a fresh subtree; parent defaults to `ecs.root`. |
| `duplicate(...)` | Exact alias of `instantiate`. |

Instantiation remaps entity/component references within each copy and preserves
shared table identity, cycles, and metatables. It builds the complete tree
before calling custom `awake`, in parent-to-descendant and component-list order.
Prefab-authored values survive core initialization. Script paths in editor
prefabs stay project-relative.

`prefabs.ui` provides immutable source templates for a label, panel, dialog,
and status chip. Instantiate or register/capture before customization. The
module also exposes engine-managed `_registry`; do not mutate it directly.

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

`to`, `new`, and `create` are identical. The target's current and destination
values must be numeric; duration must be finite and non-negative. Defaults are
`linear` and `out`. Progress is clamped to `0..1`; zero duration completes on the
next positive update. Completion writes the exact destination, calls the
callback once, and releases registry references.

Accepted style aliases are `sin`, `quadratic`, `quartic`, `quintic`,
`exponential`, and `circular`. Direction parsing ignores `_` and `-`, so
`inOut`, `in_out`, and `in-out` are equivalent. Unknown names raise.

`cancelAll`/`cancel_all` return the number newly cancelled. `count` counts live
tweens. `ease` evaluates without creating a tween. `update` and `_update` are
the same function; the engine calls `_update` automatically, so gameplay should
not call either unless it deliberately wants an extra advance.

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

`load`/`Load` read `.neoanim` JSON from the project path. `new`/`create` create
a paused player; `play` creates a playing one. `duration` defaults to the last
key time when absent and is raised to at least that time when supplied shorter.
Looping defaults false; `looped` is a compatibility alias of `looping`.

Tracks write numeric target properties by exact string key. Keys are sampled in
time order. `step` and `hold` retain the earlier value. `cubic` and `ease` alias
`bezier`. Bezier x handles clamp to `0..1`; defaults are outgoing `(0.333, 0)`
and incoming `(0.667, 1)`. Linear is the track default.

`seek` clamps to clip duration and clears finished state. `stop` pauses and
rewinds to zero. `setSpeed` accepts finite values at least zero. Players remain
registered after finishing and can be played again. `update`/`_update` are the
same engine-managed advance function; avoid an accidental double update.

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
    Shape2D: Shape2D,
    ParticleSystem2D: ParticleSystem2D,
    AnimationController: AnimationController,
    SpatialSound2D: SpatialSound2D,
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

::: details Engine-managed component fields
Core prototypes and instances contain lifecycle functions such as `awake` and
`update`, plus `__neolove_core_component` and `__neolove_component` tags. Some
components also keep `__...` caches, timers, particle arrays, or player handles.
They are visible because components are Luau tables, but gameplay must not call,
replace, serialize independently, or depend on those fields.
:::

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

`play` resumes automatic emission. `pause` keeps existing particles alive but
stops automatic emission. `stop` pauses and clears particles/timers. `emit`
queues a manual burst, defaulting to one particle. Non-looping emission stops
after `duration`, while existing particles finish.

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
`play` sets desired playback; `pause` pauses; `stop` pauses and rewinds. A
negative speed is rejected by the underlying handle.

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

Defaults: sound `nil`, volume `1`, looping/autoplay `false`. `play` returns
`false` without a sound; otherwise it starts spatial playback at the current
world transform and returns `true`. The component moves the emitter every
frame while active. Removal stops its sound.

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

Ranges use zero-based, end-exclusive character indexes. Formatting ranges may
overlap and are retained when new text still intersects them. `setSize` is
relative to component scale. `setOffset` and `setPixelOffset` are aliases and
do not change character advance. `setCharacterOffset` formats one character.
Calling `clearFormatting()` without a complete range clears all formatting.

Letter query indexes are zero-based. Position returns `x,y`; bounds returns
`x,y,w,h`; invalid/unlaid-out input returns nil values. Closest-index queries
accept world coordinates and return the nearest cursor/insertion index.
`getClosestCharacterIndex` aliases `getClosestLetterIndex`.

The three `core` names reference the same prototype behavior.

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
engine-updated. `focus` succeeds only while enabled and unlocked; `blur` always
clears focus. Password mode masks display but retains real `text`. The same rich
formatting methods as TextBox are supported.

::: warning
UI callbacks receive the owning `entity` first and the component instance
second. The current generated declaration file omits that second parameter;
the implemented callback signatures above are authoritative.
:::

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
top. `setValue`/`SetValue` clamp and recompute fraction without firing
`onChanged`; dragging fires only when the numeric value changes.

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

`pause` retains frame/time. `stop` pauses, resets frame and accumulated time.
`setFrame`/`set_frame` clamp negative indexes to zero and reset time. Pascal
aliases exist for play/pause/stop even though older declarations omit them.
Non-looping playback stops on the last valid frame.

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

Defaults: not computed, threshold `0`, zero rectangles and bounds. Computing
reads the source image and source rectangle, clamps threshold to `0..255`, scans
alpha, and merges opaque pixels into normalized rectangles. It raises when no
supported source component/image exists and otherwise returns `true`, including
an all-transparent result.

`IsInside` accepts world coordinates. `IsIntersecting` accepts another
Spritebox instance or an entity containing one. Both follow live world size,
scale, hierarchy, and rotation. Intersection uses an AABB broad phase and SAT
on cached rectangles.

Recompute after changing the image pixels, source rectangle, alpha threshold,
nine-slice settings, or destination size when exact nine-slice shape matters.
This is a gameplay query shape: it is not used by `transform.raycast`,
`doTheyOverlap`, or Rigidbody physics.

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

| Mode | Software geometry | Text |
| --- | --- | --- |
| `off` | hard single-sample edges | hard masks |
| `standard` | 2× edge coverage | normal grayscale glyph rasterization |
| `high` | 4× geometry edge coverage | 2× supersampled glyphs with premultiplied downsampling |

Vulkan selects the best supported device MSAA level for geometry. Text still
uses the selected text path because glyphs are rasterized before upload.
Individual text components may override the global mode.

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
back to software, inspect the warning for driver/runtime details.

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
Every entry has a full definition on its linked conceptual page or in the
generated declaration appendix.

## Runtime and editor-declaration names

`Color4`, `Inspector`, `IComponentPicker`, `IEntity`, `IComponent`, `IImage`,
`IAudio`, `IShader`, `IAnimation`, `die`, `softrequire`, `print`, `require`,
`app`, `input`, `userInput`, `assets`, `audio`, `fs`, `android`, `mobile`,
`http`, `commands`, `command`, `servers`, `shaders`, `ecs`, `prefabs`, `prefab`,
`tweening`, `tween`, `animation`, `animations`, `transform`, `transforms`,
`core`, `async`, `mouse`, and `window`.

## Handle and record types

`Color4Value`, `Vec2`, `Entity`, `Connection`, `EntityListenInfo`, `System`,
`Component`, `ComponentInstance`, `ImageHandle`, `SoundHandle`, `ShaderHandle`,
`AsyncTask`, `HttpRequestOptions`, `HttpResponse`, `CommandRunResult`,
`CommandDetachedResult`, `ServerClientHandle`, `HostedServerHandle`,
`ServerPeer`, `ServerService`, `RaycastHit`, `RaycastOptions`, `PrefabTemplate`,
`TweenHandle`, `AnimationKeyframe`, `AnimationTrack`, `AnimationClip`, and
`AnimationHandle`.

## Core prototypes

`Rect2D`, `EntityScaler`, `Shape2D`, `ParticleSystem2D`, `AnimationController`,
`SpatialSound2D`, `TextBox`, `TextLabel`, `RudimentaryTextLabel`, `TextInput`,
`Panel`, `Frame`, `Button`, `Slider`, `Dropdown`, `Sprite2D`, `Image2D`,
`SpriteSheet2D`, `NineSliceSprite2D`, `9SliceSprite2D`, `TileTexture2D`,
`Tilemap2D`, `Spritebox2D`, `Collider2D`, `Rigidbody2D`, `Bolt2D`,
`LegacyBolt2D`, `Rope2D`, and `String2D`.

## Exposed engine-managed names

| Name | Owner | Purpose |
| --- | --- | --- |
| `_poll` | `http`, `servers` | Drains asynchronous callbacks; called by the runtime. |
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

<!-- page: declarations | Generated Luau Declarations -->
# Generated Luau Declarations

This is the complete declaration source currently installed by `neolove new`
and `neolove api`. It is reproduced verbatim so every declared type, function,
field, callback, alias, and global is available in one copyable reference.

::: warning
The generated file is a tooling snapshot. The implementation-focused component
pages in this manual include several runtime fields, aliases, and corrected UI
callback parameters which this snapshot does not yet express. Runtime behavior
described in those pages takes precedence.
:::

```luau
-- neolove engine api definitions

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

export type PositionPivot = "center" | "top_right"

export type EntityListenEvent = "leftClick" | "rightClick" | "middleClick" | "scrollUp" | "scrollDown" | "mouseEntered" | "mouseExited"

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

export type Entity = {
	id: number,
	name: string,
	x: number,
	y: number,
	anchor_x: number,
	anchor_y: number,
	pivot_x: number?,
	pivot_y: number?,
	rotation: number,
	rotation_pivot: string,
	rotation_pivot_x: number?,
	rotation_pivot_y: number?,
	position_pivot: PositionPivot?,
	z: number,
	size_x: number,
	size_y: number,
	scale: number,
	raycastable: boolean?,
	parent: Entity?,
	children: { Entity },
	components: { ComponentInstance },
	listen: (self: Entity, event: EntityListenEvent | string, callback: (entity: Entity, event: EntityListenInfo) -> ()) -> Connection,
	Listen: (self: Entity, event: EntityListenEvent | string, callback: (entity: Entity, event: EntityListenInfo) -> ()) -> Connection,
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
	getWorldPosition: (self: Entity) -> (number, number),
	GetWorldPosition: (self: Entity) -> (number, number),
	getWorldRotation: (self: Entity) -> number,
	GetWorldRotation: (self: Entity) -> number,
	isInside: (self: Entity, world_x: number, world_y: number) -> boolean,
	IsInside: (self: Entity, world_x: number, world_y: number) -> boolean,
	[string]: any,
}

export type System = {
	awake: ((self: System) -> ())?,
	update: ((self: System, dt: number) -> ())?,
	lateUpdate: ((self: System, dt: number) -> ())?,
	fixedUpdate: ((self: System, dt: number) -> ())?,
	[string]: any,
}

export type CollisionCallback = (
	selfEntity: Entity,
	selfCollider: Collider2D,
	otherEntity: Entity,
	otherCollider: Collider2D,
	otherId: number
) -> ()

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

export type ShaderHandle = {
	setUniform1f: (self: ShaderHandle, name: string, x: number) -> (),
	setUniform2f: (self: ShaderHandle, name: string, x: number, y: number) -> (),
	setUniform3f: (self: ShaderHandle, name: string, x: number, y: number, z: number) -> (),
	setUniform4f: (self: ShaderHandle, name: string, x: number, y: number, z: number, w: number) -> (),
	setUniformColor: (self: ShaderHandle, name: string, color: Color4Value) -> (),
	setTexture: (self: ShaderHandle, name: string, image: ImageHandle) -> (),
}

export type ImageHandle = {
	width: (self: ImageHandle) -> number,
	height: (self: ImageHandle) -> number,
	size: (self: ImageHandle) -> (number, number),
	getPixel: (self: ImageHandle, x: number, y: number) -> Color4Value,
	setPixel: (self: ImageHandle, x: number, y: number, color: Color4Value) -> (),
	fill: (self: ImageHandle, color: Color4Value) -> (),
	upload: (self: ImageHandle) -> (),
	export: (self: ImageHandle, path: string) -> (),
	save: (self: ImageHandle, path: string) -> (),
	unload: (self: ImageHandle) -> (),
	isUnloaded: (self: ImageHandle) -> boolean,
}

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

export type AppModule = {
	bg: Color4Value,
	antiAliasing: "off" | "standard" | "high",
	setMaxFps: (fps: number?) -> (),
	getMaxFps: () -> number?,
	setShowFps: (enabled: boolean?) -> (),
	getShowFps: () -> boolean,
	nearestNeighborScaling: boolean,
	setNearestNeighborScaling: (enabled: boolean?) -> (),
	getNearestNeighborScaling: () -> boolean,
	setAntiAliasing: (mode: ("off" | "standard" | "high")?) -> (),
	getAntiAliasing: () -> "off" | "standard" | "high",
}

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

export type AudioModule = {
	play: (sound: SoundHandle, looped: boolean?, volume: number?) -> (),
	playOnce: (sound: SoundHandle, volume: number?) -> (),
	stop: (sound: SoundHandle) -> (),
	setVolume: (sound: SoundHandle, volume: number) -> (),
	playSpatial: (sound: SoundHandle, x: number, y: number, looped: boolean?, volume: number?) -> (),
	setPosition: (sound: SoundHandle, x: number, y: number) -> boolean,
	setListenerPosition: (x: number, y: number) -> (),
}

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

export type AsyncModule = {
	yield: (...any) -> ...any,
	count: () -> number,
	cancelAll: () -> number,
} & ((callback: () -> ...any) -> AsyncTask)

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
	request: ((url: string, callback: (response: HttpResponse) -> ()) -> number) & ((options: HttpRequestOptions, callback: (response: HttpResponse) -> ()) -> number),
	get: (url: string, callback: (response: HttpResponse) -> ()) -> number,
	_poll: () -> (),
}

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

export type ServerHostOptions = {
	host: string?,
	certPath: string?,
	keyPath: string?,
	cert_path: string?,
	key_path: string?,
}

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
	on: ((self: ServerClientHandle, eventName: string, callback: (data: any, eventName: string, client: ServerClientHandle) -> ()) -> ((data: any, eventName: string, client: ServerClientHandle) -> ()))?,
	once: ((self: ServerClientHandle, eventName: string, callback: (data: any, eventName: string, client: ServerClientHandle) -> ()) -> ((data: any, eventName: string, client: ServerClientHandle) -> ()))?,
	off: ((self: ServerClientHandle, eventName: string, callback: (...any) -> ()) -> boolean)?,
	onAny: ((self: ServerClientHandle, callback: (eventName: string, data: any, client: ServerClientHandle) -> ()) -> ((eventName: string, data: any, client: ServerClientHandle) -> ()))?,
	emit: ((self: ServerClientHandle, eventName: string, data: any) -> boolean)?,
}

export type HostedServerHandle = {
	client: ServerClientHandle,
	port: number,
	url: string,
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
	kick: (self: ServerPeer, reason: string?) -> (),
	isConnected: (self: ServerPeer) -> boolean,
}

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
	fromSource: (vertexSource: string, fragmentSource: string, options: ShaderLoadOptions?) -> ShaderHandle,
	fromFragmentSource: (fragmentSource: string, options: ShaderLoadOptions?) -> ShaderHandle,
}

export type TransformModule = {
	getWorldPosition: (entity: Entity) -> (number, number),
	getWorldRotation: (entity: Entity) -> number,
	lookAt: (from_x: number, from_y: number, to_x: number, to_y: number) -> number,
	look_at: (from_x: number, from_y: number, to_x: number, to_y: number) -> number,
	GetEntitiesInFront: (world_x: number, world_y: number, minimum_z: number?) -> { Entity },
	getEntitiesInFront: (world_x: number, world_y: number, minimum_z: number?) -> { Entity },
	doTheyOverlap: (entities: { Entity }) -> boolean,
	raycast: (
		origin_x: number,
		origin_y: number,
		dir_x: number,
		dir_y: number,
		max_distance: number?,
		options: RaycastOptions?
	) -> RaycastHit?,
}

export type EcsModule = {
	addSystem: (system: System) -> (),
	newEntity: (name: string, parent: Entity?, x: number?, y: number?) -> Entity,
	deleteEntity: (entity: Entity) -> (),
	duplicateEntity: (targetEntity: Entity, parent: Entity) -> Entity,
	findFirstChild: (parent: Entity, name: string) -> Entity?,
	root: Entity,
	addComponent: (entity: Entity, component: Component) -> ComponentInstance,
	removeComponent: (entity: Entity, target: number | ComponentInstance) -> boolean,
	loadScene: (path: string) -> (),
}

export type PrefabTemplate = {
	name: string?,
	x: number?,
	y: number?,
	anchor_x: number?,
	anchor_y: number?,
	pivot_x: number?,
	pivot_y: number?,
	rotation: number?,
	rotation_pivot: string?,
	rotation_pivot_x: number?,
	rotation_pivot_y: number?,
	position_pivot: PositionPivot?,
	z: number?,
	size_x: number?,
	size_y: number?,
	scale: number?,
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

export type EasingStyle =
	"linear"
	| "sine"
	| "quad"
	| "cubic"
	| "quart"
	| "quint"
	| "expo"
	| "circ"
	| "back"
	| "bounce"

export type EasingDirection = "in" | "out" | "inOut" | "in_out"

export type TweenHandle = {
	id: number,
	cancel: (self: TweenHandle) -> boolean,
	Cancel: (self: TweenHandle) -> boolean,
	isDone: (self: TweenHandle) -> boolean,
	IsDone: (self: TweenHandle) -> boolean,
}

export type TweeningModule = {
	to: (
		target: { [any]: any },
		key: any,
		value: number,
		duration: number,
		style: EasingStyle?,
		direction: EasingDirection?,
		onComplete: (() -> ())?
	) -> TweenHandle,
	new: (
		target: { [any]: any },
		key: any,
		value: number,
		duration: number,
		style: EasingStyle?,
		direction: EasingDirection?,
		onComplete: (() -> ())?
	) -> TweenHandle,
	create: (
		target: { [any]: any },
		key: any,
		value: number,
		duration: number,
		style: EasingStyle?,
		direction: EasingDirection?,
		onComplete: (() -> ())?
	) -> TweenHandle,
	cancelAll: () -> number,
	cancel_all: () -> number,
	count: () -> number,
	ease: (t: number, style: EasingStyle?, direction: EasingDirection?) -> number,
	update: (dt: number) -> (),
}

export type AnimationKeyframe = {
	time: number,
	value: number,
	out_x: number?,
	out_y: number?,
	in_x: number?,
	in_y: number?,
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
}

export type BaseDrawableComponent = ComponentInstance & {
	NEOLOVE_RENDERING: boolean,
	color: Color4Value,
	shader: ShaderHandle?,
	visible: boolean,
}

export type Rect2D = BaseDrawableComponent

export type EntityScaler = ComponentInstance & {
	__neolove_component: "EntityScaler",
	enabled: boolean,
	edit_with_percent: boolean,
	editWithPercent: boolean?,
	x_percent: number,
	y_percent: number,
	size_x_percent: number,
	size_y_percent: number,
	xPercent: number?,
	yPercent: number?,
	sizeXPercent: number?,
	sizeYPercent: number?,
	percent_x: number?,
	percent_y: number?,
	percentX: number?,
	percentY: number?,
	offset_x: number,
	offset_y: number,
	offsetX: number?,
	offsetY: number?,
	pivot_x: number,
	pivot_y: number,
	pivotX: number?,
	pivotY: number?,
}

export type Shape2DShape = "box" | "circle" | "triangle" | "right_triangle" | "righttriangle" | "rightangledtriangle"
export type TriangleCorner = "bl" | "br" | "tl" | "tr" | "bottomright" | "rightbottom" | "topleft" | "lefttop" | "topright" | "righttop"

export type Shape2D = BaseDrawableComponent & {
	shape: Shape2DShape,
	triangle_corner: TriangleCorner,
	offset_x: number,
	offset_y: number,
	size_x: number,
	size_y: number,
}

export type ParticleEmitterShape = "point" | "box" | "circle"
export type ParticleColorKeypoint = { time: number, color: Color4Value }
export type ParticleNumberKeypoint = { time: number, value: number }

export type ParticleSystem2D = BaseDrawableComponent & {
	__neolove_component: "ParticleSystem2D",
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

export type AnimationController = ComponentInstance & {
	__neolove_component: "AnimationController",
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

export type SpatialSound2D = ComponentInstance & {
	__neolove_component: "SpatialSound2D",
	sound: SoundHandle?,
	volume: number,
	looping: boolean,
	autoplay: boolean,
	play: (self: SpatialSound2D) -> boolean,
	Play: (self: SpatialSound2D) -> boolean,
	stop: (self: SpatialSound2D) -> (),
	Stop: (self: SpatialSound2D) -> (),
}

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
}

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

export type TextLabel = TextBox
export type RudimentaryTextLabel = TextBox

export type TextInput = BaseDrawableComponent & UiTextStyle & {
	__neolove_component: "TextInput",
	text: string,
	placeholder: string,
	enabled: boolean,
	locked: boolean,
	focused: boolean,
	password: boolean,
	max_length: number,
	submit_on_enter: boolean,
	clear_on_submit: boolean,
	blur_on_submit: boolean,
	cursor_index: number,
	text_color: Color4Value,
	placeholder_color: Color4Value,
	caret_color: Color4Value,
	background_color: Color4Value,
	border_color: Color4Value,
	border_width: number,
	corner_radius: number,
	caret_width: number,
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
	blur: (self: TextInput) -> (),
	onChanged: ((self: Entity, text: string) -> ())?,
	onSubmit: ((self: Entity, text: string) -> ())?,
	onFocus: ((self: Entity) -> ())?,
	onBlur: ((self: Entity) -> ())?,
}

-- A customizable UI container. Defaults match Visual Studio Code's Dark+ theme.
export type Panel = BaseDrawableComponent & {
	__neolove_component: "Panel",
	background_color: Color4Value,
	border_color: Color4Value,
	border_width: number,
	corner_radius: number,
	background_image: ImageHandle?,
	slice_left: number,
	slice_right: number,
	slice_top: number,
	slice_bottom: number,
}
export type Frame = Panel

-- An interactive button. Every state colour (including hover) is configurable.
export type Button = BaseDrawableComponent & UiTextStyle & {
	__neolove_component: "Button",
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
	icon_image: ImageHandle?,
	icon_color: Color4Value,
	icon_size: number,
	icon_gap: number,
	icon_side: "left" | "right",
	onClick: ((self: Entity) -> ())?,
	onPress: ((self: Entity) -> ())?,
	onRelease: ((self: Entity) -> ())?,
	onHoverEnter: ((self: Entity) -> ())?,
	onHoverLeave: ((self: Entity) -> ())?,
}

-- A draggable value slider. Hover colours for the track, fill, and thumb are
-- each configurable.
export type Slider = BaseDrawableComponent & {
	__neolove_component: "Slider",
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
	background_color: Color4Value,
	hover_background_color: Color4Value,
	disabled_background_color: Color4Value,
	fill_color: Color4Value,
	hover_fill_color: Color4Value,
	disabled_fill_color: Color4Value,
	thumb_color: Color4Value,
	hover_thumb_color: Color4Value,
	disabled_thumb_color: Color4Value,
	setValue: (self: Slider, value: number) -> (),
	onChanged: ((self: Entity, value: number) -> ())?,
}

-- A selectable dropdown with a scrollable popup menu.
export type Dropdown = BaseDrawableComponent & UiTextStyle & {
	__neolove_component: "Dropdown",
	enabled: boolean,
	open: boolean,
	hovered: boolean,
	selected_index: number,
	selected_text: string,
	selected_value: string,
	placeholder: string,
	options: { any },
	item_height: number,
	max_visible_items: number,
	open_upwards: boolean,
	background_color: Color4Value,
	hover_background_color: Color4Value,
	open_background_color: Color4Value,
	disabled_background_color: Color4Value,
	border_color: Color4Value,
	hover_border_color: Color4Value,
	open_border_color: Color4Value,
	text_color: Color4Value,
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
	onChanged: ((self: Entity, index: number, value: any) -> ())?,
}

export type Sprite2D = BaseDrawableComponent & {
	__neolove_component: "Sprite2D" | "Image2D",
	image: ImageHandle?,
	source_x: number?,
	source_y: number?,
	source_w: number?,
	source_h: number?,
	source_width: number?,
	source_height: number?,
	sourceX: number?,
	sourceY: number?,
	sourceW: number?,
	sourceH: number?,
	sourceWidth: number?,
	sourceHeight: number?,
}

export type Image2D = Sprite2D

export type SpriteSheet2D = BaseDrawableComponent & {
	__neolove_component: "SpriteSheet2D",
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
	pause: (self: SpriteSheet2D) -> (),
	stop: (self: SpriteSheet2D) -> (),
	setFrame: (self: SpriteSheet2D, frame: number) -> (),
	set_frame: (self: SpriteSheet2D, frame: number) -> (),
}

export type NineSliceSprite2D = BaseDrawableComponent & {
	__neolove_component: "NineSliceSprite2D",
	image: ImageHandle?,
	source_x: number?,
	source_y: number?,
	source_w: number?,
	source_h: number?,
	source_width: number?,
	source_height: number?,
	sourceX: number?,
	sourceY: number?,
	sourceW: number?,
	sourceH: number?,
	sourceWidth: number?,
	sourceHeight: number?,
	slice_left: number,
	slice_right: number,
	slice_top: number,
	slice_bottom: number,
	sliceLeft: number?,
	sliceRight: number?,
	sliceTop: number?,
	sliceBottom: number?,
}

export type TileTexture2D = BaseDrawableComponent & {
	image: ImageHandle?,
	tile_width: number,
	tile_height: number,
	offset_x: number,
	offset_y: number,
}

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

export type Spritebox2D = ComponentInstance & {
	__neolove_component: "Spritebox2D",
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

export type Collider2D = ComponentInstance & {
	__neolove_component: "Collider2D",
	enabled: boolean,
	is_trigger: boolean,
	non_physics: boolean,
	offset_x: number,
	offset_y: number,
	size_x: number,
	size_y: number,
	shape: string,
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
	setOnCollisionEnter: (self: Collider2D, callback: CollisionCallback?) -> (),
	setOnCollisionStay: (self: Collider2D, callback: CollisionCallback?) -> (),
	setOnCollisionExit: (self: Collider2D, callback: CollisionCallback?) -> (),
	setOnTriggerEnter: (self: Collider2D, callback: CollisionCallback?) -> (),
	setOnTriggerStay: (self: Collider2D, callback: CollisionCallback?) -> (),
	setOnTriggerExit: (self: Collider2D, callback: CollisionCallback?) -> (),
}

export type RigidbodyBoundsMode = "none" | "window"

export type Rigidbody2D = ComponentInstance & {
	__neolove_component: "Rigidbody2D",
	velocity_x: number,
	velocity_y: number,
	force_x: number,
	force_y: number,
	acceleration_x: number,
	acceleration_y: number,
	gravity_x: number,
	gravity_y: number,
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

export type Bolt2D = ComponentInstance & {
	__neolove_component: "Bolt2D",
	enabled: boolean,
	target_entity: Entity?,
	target: Entity?,
	x: number,
	y: number,
	offset_x: number,
	offset_y: number,
	strength: number,
	contacts_enabled: boolean,
	current_force: number,
	force: number,
	attach: (self: Bolt2D, targetEntity: Entity) -> (),
	link: (self: Bolt2D, targetEntity: Entity) -> (),
}

export type LegacyBolt2D = ComponentInstance & {
	__neolove_component: "LegacyBolt2D",
	enabled: boolean,
	target_entity: Entity?,
	target: Entity?,
	x: number,
	y: number,
	offset_x: number,
	offset_y: number,
	strength: number,
	contacts_enabled: boolean,
	current_force: number,
	force: number,
	attach: (self: LegacyBolt2D, targetEntity: Entity) -> (),
	link: (self: LegacyBolt2D, targetEntity: Entity) -> (),
}

export type Rope2D = ComponentInstance & {
	__neolove_component: "Rope2D",
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

export type Light2DKind = "point" | "spot" | "directional"

export type Light2D = ComponentInstance & {
	__neolove_component: "Light2D",
	NEOLOVE_RENDERING: boolean,
	kind: Light2DKind,
	color: Color4Value,
	intensity: number,
	radius: number,
	falloff: number,
	angleOffset: number,
	coneAngle: number,
	coneSoftness: number,
	castsShadows: boolean,
	shadowSoftness: number,
	visible: boolean,
}

export type LightOccluderShape = "box" | "circle"

export type LightOccluder2D = ComponentInstance & {
	__neolove_component: "LightOccluder2D",
	NEOLOVE_RENDERING: boolean,
	shape: LightOccluderShape,
	visible: boolean,
}

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

export type CoreModule = {
	Rect2D: Rect2D,
	Light2D: Light2D,
	LightOccluder2D: LightOccluder2D,
	EntityScaler: EntityScaler,
	Shape2D: Shape2D,
	ParticleSystem2D: ParticleSystem2D,
	AnimationController: AnimationController,
	SpatialSound2D: SpatialSound2D,
	TextBox: TextBox,
	TextLabel: TextLabel,
	RudimentaryTextLabel: RudimentaryTextLabel,
	TextInput: TextInput,
	Panel: Panel,
	Frame: Frame,
	Button: Button,
	Slider: Slider,
	Dropdown: Dropdown,
	Sprite2D: Sprite2D,
	SpriteSheet2D: SpriteSheet2D,
	Image2D: Image2D,
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

export type RngInstance = {
	next: (self: RngInstance) -> number,
	number: (self: RngInstance, min: number?, max: number?) -> number,
	float: (self: RngInstance, min: number?, max: number?) -> number,
	range: (self: RngInstance, min: number?, max: number?) -> number,
	integer: (self: RngInstance, min: number, max: number?) -> number,
	int: (self: RngInstance, min: number, max: number?) -> number,
	boolean: (self: RngInstance, p: number?) -> boolean,
	bool: (self: RngInstance, p: number?) -> boolean,
	sign: (self: RngInstance) -> number,
	angle: (self: RngInstance) -> number,
	unit: (self: RngInstance) -> (number, number),
	pick: (self: RngInstance, list: { any }) -> any,
	shuffle: (self: RngInstance, list: { any }) -> { any },
	seed: (self: RngInstance, seed: number) -> (),
	clone: (self: RngInstance) -> RngInstance,
	Clone: (self: RngInstance) -> RngInstance,
}

export type RngModule = {
	new: (seed: number?) -> RngInstance,
	fromString: (text: string) -> RngInstance,
} & ((seed: number?) -> RngInstance)

declare function Color4(r: number, g: number, b: number, a: number?): Color4Value
declare function Inspector<T>(defaultValue: T, max: number?, fractional: boolean?): T
-- Inspector reference placeholders. The visual editor replaces these with a
-- selected scene entity/component when it generates the scene.
declare IEntity: Entity
declare IComponent: ComponentInstance
declare IImage: ImageHandle
declare IAudio: SoundHandle
declare IShader: ShaderHandle
declare IAnimation: AnimationClip
-- Register the current behaviour module in the editor's "Add Component" picker,
-- so it can be attached to entities like a core component. Call it at module
-- scope, e.g. `IComponentPicker(Behaviour)`.
declare function IComponentPicker(behaviour: any): ()
declare function die(): ()
declare function softrequire(modulePathOrSource: string, allowedModules: { [string]: any } | { string }?): any

declare app: AppModule
declare input: InputModule
declare userInput: InputModule
declare assets: AssetsModule
declare audio: AudioModule
declare fs: FsModule
declare android: AndroidModule
declare mobile: MobileModule
declare http: HttpModule
declare commands: CommandsModule
declare command: CommandsModule
declare servers: ServersModule
declare shaders: ShadersModule
declare ecs: EcsModule
declare prefabs: PrefabsModule
declare prefab: PrefabsModule
declare tweening: TweeningModule
declare tween: TweeningModule
declare animation: AnimationModule
declare animations: AnimationModule
declare transform: TransformModule
declare transforms: TransformModule
declare core: CoreModule
declare lighting: LightingModule
declare Rng: RngModule
declare async: AsyncModule

declare mouse: Vec2
declare window: Vec2

return nil
```
