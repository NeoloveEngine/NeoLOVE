<!-- page: overview | Overview -->
# Overview

NeoLOVE is a Rust game engine for Luau projects. A project is a directory with a `main.luau` file and, optionally, a `neolove.toml`, assets, components, modules, and generated build output. Runtime APIs are exposed as Luau globals such as `ecs`, `core`, `assets`, `input`, `audio`, `fs`, `android`, `mobile`, `servers`, `shaders`, `tweening`, `animation`, and `async`.

The generated type surface is also available in `neolove_engine_api.d.luau`. New projects receive a copy from `src/project_template/neolove_engine_api.d.luau`.

<!-- page: cli | CLI -->
# CLI

```bash
neolove new <project-name>
neolove run [project-dir] [--mobile] [--portrait|--landscape] [--wifi|--cellular|--offline]
neolove editor [project-dir]
neolove build [project-dir] [--webasm|--android|--ios]
neolove api [project-dir]
neolove update
neolove setup-path
neolove --help
neolove --version
```

`run` and `build` require the target project to contain `main.luau`.

`neolove update` fast-forwards the engine's tracked Git branch, rebuilds the
same renderer feature set in release mode, and replaces the current executable.
The engine source checkout must be clean. The editor checks for updates without
blocking startup and asks before launching the updater.

`neolove build --webasm` creates an HTML5 bundle in `dist/webasm/` and a zip at `dist/<project-name>-webasm.zip`. Serve web builds over `http://` or `https://`; browsers will not reliably load the bundle from `file://`.

`neolove build --windows` builds a Windows desktop executable from Linux when the `x86_64-pc-windows-gnu` Rust target and MinGW-w64 linker are available. The MinGW C/C++ runtimes are linked statically so the `.exe` does not need a separately distributed `libstdc++-6.dll`. `neolove build --linux` builds a Linux desktop executable from Windows when a Linux GNU cross linker is available.

`neolove build --android` creates a signed arm64 APK at `dist/<project-name>-android-arm64.apk`. `--apk` is accepted as an alias. The first Android build may install the Android Rust target plus a local JDK, SDK, build-tools, and NDK under `~/.neolove/toolchains/`.

`neolove build --ios` creates an iOS simulator `.app` at `dist/<project-name>-ios-simulator.app`. It requires macOS with Xcode installed.

`neolove run --mobile` starts the desktop mobile emulator. The window is locked
to the emulated device size; use `--portrait` or `--landscape` to rotate it.
The emulator disables keyboard events and exposes mobile/network state through
the `mobile` global.

Release builds are size-oriented by default:

```bash
cargo build --release
cargo build --release --features vulkan
```

The default desktop binary uses the software renderer and omits Vulkan to keep
setup simple. Use `--features vulkan` for GPU acceleration and custom shader
rendering. Desktop game exports rebuild a compact packaged runtime before
appending the compressed project payload. Image codecs include PNG, JPEG, GIF,
BMP, TGA, TIFF, PNM, WebP, HDR, and DDS. Native audio supports WAV, MP3,
OGG/Vorbis, and FLAC; web builds can also play browser-decodable AAC/M4A and
AIFF.

<!-- page: project-model | Project Model -->
# Project Model

`main.luau` is loaded as the entry point. Relative bundled asset, command, font,
shader, and module paths are resolved from the project root.

Writable filesystem paths and image/sound exports use a separate data root. In
development this is the project directory. A packaged desktop game uses a
`<game-name>_data` directory beside the executable, so it does not require a
writable or separately distributed project folder.

Explicit absolute and parent-relative filesystem or export paths may target
any location permitted by the operating system.

Common project layout:

```text
my-game/
  main.luau
  scene.neoscene
  neolove.toml
  assets/
  components/
  shaders/
  neolove_engine_api.d.luau
```

`neolove.toml` can set package and desktop window defaults:

```toml
[package]
name = "my-game"

[window]
title = "My Game"
icon = "assets/icon.png"
width = 1280
height = 720
fullscreen = false
resizable = true
```

`width` and `height` are the logical starting resolution and are clamped to
`1..16384`. `fullscreen` and `resizable` accept boolean-style values such as
`true`, `false`, `on`, `off`, `1`, and `0`. The visual editor uses the same
width and height for the default window-bounds overlay.

<!-- page: visual-editor | Visual Editor -->
# Visual Editor

The visual editor is launched from a project directory:

```bash
neolove editor
neolove editor path/to/my-game
```

The editor opens a native window with a toolbar, document tabs, a dockable or
detachable Hierarchy, a 2D Viewport, an Inspector, and a bottom Project
browser. It does not require a
`main.luau` file to open; it loads `scene.neoscene` when present and creates a
starter scene otherwise.

Editor files:

- `scene.neoscene`: JSON scene data authored by the editor.
- Global `editor.json`: editor-wide theme, font, tooltip, overlay, autosave,
  dock layout, grid, snapping, and panel-size settings. NeoLOVE stores this in
  the operating system user config directory and reads older project-local
  `editor.json` files as a fallback.
- `main.luau`: generated by **Export** or **Run**. It is a small entry point
  that loads the configured start scene at runtime via `ecs.loadScene(...)`.
- `*.neoprefab`: JSON prefab files saved from editor entities.
- `*.neoanim`: JSON animation clips authored by the animation editor.

The top bar keeps frequent actions visible: active-scene rename, Save, Run,
Add Entity, transform tools, snap/grid controls, and panel/view menus. The
folder button opens the compact scene menu for lower-frequency actions:

- **New Scene** creates a new scene in its own tab.
- **Save** writes the current scene to its `.neoscene` file.
- **Reload Scene** reloads the current `.neoscene` file.
- **Export** saves the start scene's `.neoscene` and writes a runnable
  `main.luau` that loads it with `ecs.loadScene(...)`.
- **Run** exports `main.luau` and launches a live preview. If the preview exits
  with an error, the editor shows a dismissible **Runtime Error** dialog with the
  captured output and a copy button.
- **Mobile** opens the mobile emulator settings. Mobile mode locks the preview
  to a phone-sized portrait or landscape resolution, disables keyboard input in
  the launched game, and exposes Wi-Fi/cellular/low-power toggles through the
  runtime `mobile` global.
- **Build** exports `main.luau`, asks which platform to build, and packages the
  project into `dist/` without blocking the editor UI.
- **Add Entity** remains directly available in the top bar.

Editor Settings includes named presets, an editable custom palette with live
preview, a **Browse** button for `.ttf`/`.otf` editor fonts, and workflow
preferences. Custom colors are retained when switching to a preset, and font
changes apply without restarting the editor.

## Editing Scenes

The Hierarchy contains editor entities, shown as a tree with a search box that
filters by name. Drag rows to reparent entities, use the per-row eye toggle to
exclude an entity and its descendants from export, and use right-click menus for
common actions such as add child, duplicate, copy, paste, unparent, reset
transform, frame selected, rename, activate/deactivate, and delete.

The toolbar's three-dot menu contains Unity-style selection, hierarchy,
alignment, and Scene-view tools. Hierarchy eye and lock controls affect only
the editor view: they never disable or remove an entity from the exported game.
Branches can be folded individually or collapsed and expanded in bulk.

The Inspector edits entity transform data (`x`, `y`, `z`, size, rotation,
scale, and anchors), scene background color, attached components, and script
public variables. The editor's built-in component menu is backed by the real
engine component names:

- Common: `Rect2D`, `Shape2D`, `ParticleSystem2D`, `AnimationController`,
  `SpatialSound2D`, `TextBox`, `TextLabel`, `TextInput`, `Sprite2D`, `SpriteSheet2D`, `Image2D`,
  `NineSliceSprite2D`, `Tilemap2D`, `TileTexture2D`, `EntityScaler`,
  `Collider2D`, and `Rigidbody2D`.
- Advanced: `Spritebox2D`, `Bolt2D`, `Rope2D`, `LegacyBolt2D`, `String2D`,
  and `RudimentaryTextLabel`.
- Drag a `.luau` or `.lua` component script from the Project browser onto an
  entity in the Hierarchy or Viewport to attach it.
- Script variables wrapped in `Inspector(...)` are derived from their defaults
  and exported after `AddComponent(require("..."))`:

```luau
local Component = {
    speed = Inspector(100),                    -- number field
    lives = Inspector(1, 10),                  -- whole-number slider
    opacity = Inspector(0, 1, true),           -- fractional slider
    tint = Inspector(Color4(255, 120, 80)),    -- colour picker
    inventory = Inspector({ "sword", "key" }), -- editable list
    stats = Inspector({ health = 100, mana = 40 }), -- dictionary
    target = Inspector(IEntity),               -- scene entity reference
    renderer = Inspector(IComponent),          -- scene component reference
    sprite = Inspector(IImage),                -- image asset
    sound = Inspector(IAudio),                 -- sound asset
    material = Inspector(IShader),             -- fragment shader asset
    clip = Inspector(IAnimation),              -- animation clip asset
}
```

For numeric sliders, the first number is both the initial value and one range
endpoint. Passing `true` as the third argument enables fractional values;
fractional bounds enable them automatically. Tables with consecutive numeric
keys starting at `1` are lists. Tables with gaps or non-numeric keys are
dictionaries. Lists and dictionaries can contain nested inspector-supported
values. When the script changes on disk, the editor refreshes its declaration
schema and preserves compatible edited values by name.

Entity and component defaults may also be concrete runtime values instead of
`IEntity`/`IComponent`; the editor infers the same reference field. To assign an
entity, drag its Hierarchy row onto the field. To assign a component, drag its
Inspector header, hover the destination entity in the Hierarchy to inspect it,
then drop the component onto the destination field without releasing the mouse.

Components can be reordered by removing and re-adding them. Each component
header has a copy button, and the **Add Component** menu offers **Paste** to
apply a copied component to another entity. The **Add Component** picker opens
with a focused search box: typing filters the list, and pressing Enter adds the
top match. Behaviour scripts that call `IComponentPicker(Behaviour)` appear in
the picker too (see [Custom Picker Components](#custom-picker-components)). Color
properties show inline **R, G, B, and A** fields — the `A` (alpha) field sets
transparency, where `0` is fully transparent and `255` is opaque. The swatch
(and the scene background) opens a picker that toggles between an HSV square with
a hue strip and plain RGBA sliders — both include an alpha slider — and the
choice is remembered in the global editor config. Interactive UI components
(Button, TextInput, Slider, Dropdown) expose every state colour, including the
`hover` variants, under the inspector's **Advanced** section.
Selecting an entity with a `Collider2D` previews the collider's shape and size
as a green outline, since it can differ from the entity bounds.

Image properties use project-relative paths or base64 PNG data. Because
`main.luau` loads the start scene through `ecs.loadScene(...)`, the scene's
generated Luau is produced at load time — it requires each script component
module once and loads each image once through `assets.loadImage` (which caches
by path). The viewport previews image components with the real asset, including
nine-slice and tiled rendering. Script inspector asset handles export through
`assets.loadImage`, `assets.loadSound`, `shaders.loadFragment`, and
`animation.load`.

### Particle System 2D

`ParticleSystem2D` is a bounded particle emitter with point, box, and
circle emission shapes. The visual editor exposes emission rate, maximum
particles, duration/looping, lifetime, speed, direction/spread, start/end size,
an optional particle image, colour and transparency keypoints over normalized
lifetime, radius, and gravity. Clicking either sequence strip opens a
Roblox-style keypoint editor. Its deterministic editor preview shows a
representative spread without changing the saved scene.

```luau
local emitter = ecs.newEntity("Sparks", ecs.root, 320, 240)
local particles = emitter:AddComponent(core.ParticleSystem2D)
particles.image = assets.loadImage("assets/spark.png")
particles.emission_rate = 40
particles.lifetime = 0.8
particles.speed = 140
particles.spread = 55
particles.color_sequence = {
    { time = 0, color = Color4(255, 210, 90) },
    { time = 0.6, color = Color4(255, 120, 30) },
    { time = 1, color = Color4(255, 60, 20) },
}
particles.transparency_sequence = {
    { time = 0, value = 0 }, -- opaque
    { time = 1, value = 1 }, -- transparent
}

particles:pause()
particles:emit(12) -- one-shot burst
particles:play()
particles:stop()   -- stops and clears live particles
```

## Viewport And Project Browser

The Viewport supports entity selection, right-click creation, middle-mouse
panning, scroll-wheel zoom, grid display, grid snapping, a default window-bounds
overlay, and a live transform/zoom overlay. The toolbar grid field controls the
snap step.

Scene tools:

- Move uses a center handle and drag gestures for selected entities.
- Scale uses corner handles.
- Rotate uses a rotation knob.
- When a transform handle overlaps an entity, pressing the handle starts that
  handle operation instead of selecting the entity behind it.
- Holding `Ctrl` while moving an entity moves it independently of descendants by
  updating descendant local positions so their world positions stay stable.
- Holding `Ctrl` while dragging a scale handle preserves the entity aspect ratio.

The Window dropdown can close, restore, dock, or undock the Hierarchy,
Inspector, and Project browser. Undocked widgets are shown as separate native
editor windows. Their header button docks them back into the main editor.

The Project browser opens project files with the OS default handler, creates
folders, Luau script templates, fragment shader templates, and animation clips,
reveals folders in the OS file manager, and handles editor prefabs:

- Drag an entity from the Hierarchy to the Project browser to save a
  `.neoprefab` containing that entity and its descendants.
- Drag a `.neoprefab` from the Project browser into the Viewport to instantiate
  it at the drop position with fresh entity ids and a source link.
- Double-click a `.neoscene` or `.neoprefab` to open it in a tab. A prefab tab
  contains only that prefab. Saving it refreshes linked instances in open and
  on-disk scenes while preserving each instance root's placement.
- Double-click a `.neoanim` to open it in the animation editor.
- A selected `Tilemap2D` component can enter Paint mode from the Inspector.
  Drag over the entity grid to write the selected tile id; tile `-1` erases.

Useful shortcuts:

- `Ctrl+S`: save the scene.
- `Ctrl+Z`: undo.
- `Ctrl+Y` or `Ctrl+Shift+Z`: redo.
- `Ctrl+C` / `Ctrl+V`: copy and paste the selected entity.
- `Ctrl+D`: duplicate the selected entity.
- `Ctrl+A`: select all entities; `Ctrl+Shift+A` inverts the selection.
- `Ctrl+G`: group the selection; `Ctrl+Shift+G` unparents it.
- `H` / `Shift+H`: hide the selection in the Scene view / show all.
- `L` / `Shift+L`: lock selection from Scene picking / unlock all.
- `G`: toggle the grid; `Shift+S`: toggle snapping.
- `F`: frame the selected entity.
- `Home`: frame all visible entities.
- `Shift+Space`: maximize or restore the Scene view.
- `0`: reset the viewport camera.
- `F2`: rename the selected entity.
- Arrow keys: nudge the selected entity by one unit; hold `Shift` to nudge by
  the grid step.
- Hold `Ctrl` while dragging a resize handle to preserve the entity's aspect
  ratio; hold `Ctrl` while moving to keep descendants in place.

## Runtime Loading

Scenes can be exported to Luau or loaded directly at runtime:

```luau
ecs.loadScene("scene.neoscene")
```

`ecs.loadScene` parses the editor JSON and executes the same generated Luau
that **Export** writes. This instantiates the scene into the current world; it
does not clear existing entities first. Inactive entities and children of
inactive entities are omitted.

<!-- page: runtime-order | Runtime Order -->
# Runtime Order

Each frame:

1. Input state is refreshed.
2. HTTP and server callbacks are polled.
3. Luau `async` tasks are resumed once.
4. Tweening, animation players, and entity listeners are updated.
5. System and non-rendering component `update` callbacks run.
6. Physics and rope constraints are simulated.
7. Rendering component updates run in stable draw order.
8. Queued draw commands are rendered.

Rendering components have `NEOLOVE_RENDERING = true`. They still use an `update` callback, but the runtime delays those callbacks until the rendering pass so `z` order is stable.

<!-- page: global-helpers | Global Helpers -->
# Global Helpers

## `Color4(r, g, b, a?)`

Creates a color table:

```luau
local white = Color4(255, 255, 255)
local translucent = Color4(255, 255, 255, 128)
```

Fields are `r`, `g`, `b`, and `a`. Values are clamped to `0..255`. Omitted alpha defaults to `255`.

## `die(reason?)`

Requests runtime exit. If `reason` is omitted, the engine records a default reason.

## `softrequire(modulePathOrSource, allowedModules?)`

Loads Luau source in a sandbox. `allowedModules` may be a table of global names or a map of explicit values. It is useful for plugins, user-authored scripts, or controlled module loading.

<!-- page: async-tasks | Async Tasks -->
# Async Tasks

Global: `async`

`async(callback)` creates a Luau coroutine and queues it for the engine update
loop. The callback begins on the next update. Each call to `async.yield()`
suspends the task until the following update.

```luau
local task = async(function()
    for chunk = 1, 100 do
        generateMapChunk(chunk)
        async.yield()
    end

    return "finished", 100
end)
```

::: warning
Async tasks use cooperative scheduling inside the engine's Luau VM. They are
not operating-system threads. A callback that runs for a long time without
yielding still blocks that frame.
:::

Split map generation and other CPU-heavy work into bounded chunks and call
`async.yield()` regularly. Synchronous filesystem and command calls finish
before the coroutine can yield.

## Task handles

`async(callback)` returns a task handle:

```luau
task:isDone()
task:getStatus()
task:getError()
local result, count = task:getResult()
task:cancel()
```

Status values are `queued`, `running`, `suspended`, `completed`, `cancelled`,
and `error`.

PascalCase aliases are also available:

- `IsDone`
- `GetStatus`
- `GetError`
- `GetResult`
- `Cancel`

Public fields include `id`, `done`, `cancelled`, `status`, `error`, `result`,
and `results`.

## Module helpers

```luau
local activeTasks = async.count()
local cancelledTasks = async.cancelAll()
async.yield()
```

- `async.count()` returns the number of queued or suspended tasks.
- `async.cancelAll()` cancels all unfinished tasks and returns the number
  cancelled.
- `async.yield()` suspends the current async task until the next update.

<!-- page: app-settings | App Settings -->
# App Settings

Global: `app`

Fields and functions:

- `app.bg`: clear color.
- `app.nearestNeighborScaling`: texture filtering default. `true` means nearest-neighbor.
- `app.antiAliasing`: `off`, `standard`, or `high` (the default).
- `app.setNearestNeighborScaling(enabled?)`
- `app.getNearestNeighborScaling()`
- `app.setAntiAliasing(mode?)`
- `app.getAntiAliasing()`
- `app.setMaxFps(fps?)`
- `app.getMaxFps()`
- `app.setShowFps(enabled?)`
- `app.getShowFps()`

Edges:

- `setMaxFps(nil)` clears the cap.
- Non-positive or non-finite FPS values are ignored.
- Replacing the global `app` table is supported; getters read the current table.

<!-- page: input | Input -->
# Input

Globals: `input`, `userInput`

Keyboard:

```luau
input.isKeyDown("space")
input.isKeyPressed("a")
input.isKeyReleased("escape")
input.getLastKeyPressed()
input.getCharPressed()
input.showKeyboard()
input.hideKeyboard()
```

Mouse:

```luau
input.isMouseDown("left")
input.isMousePressed("right")
input.isMouseReleased("middle")
local wheelX, wheelY = input.getMouseWheel()
local dx, dy = input.getMouseDelta()
input.setMouseLocked(true)
```

Scroll helpers:

```luau
input.isScrollingIn()
input.isScrollingOut()
input.getScrollInAmount()
```

Edges:

- Pressed and released states are frame-local.
- `input.showKeyboard()` / `input.openKeyboard()` request the on-screen
  keyboard on supported mobile builds and return whether the platform handled
  the request. The optional boolean is passed as Android's implicit-show flag
  and defaults to `true`.
- `input.hideKeyboard()` / `input.closeKeyboard()` request that the on-screen
  keyboard close on supported mobile builds and return whether the platform
  handled the request. The optional boolean is passed as Android's
  implicit-only hide flag and defaults to `false`.
- Mouse positions are exposed through global `mouse.x` and `mouse.y`.
- `window.x` and `window.y` contain the current logical window size.

<!-- page: assets | Assets -->
# Assets

Global: `assets`

Images:

```luau
local image = assets.loadImage("assets/player.png")
local embedded = assets.loadImage("data:image/png;base64,iVBORw0KGgo...")
local embeddedRaw = assets.loadImageBase64("iVBORw0KGgo...")
local photo = assets.snapPhoto(100, 80, 420, 300)
local blank = assets.newImage(64, 64, Color4(0, 0, 0, 0))
local w = image:width()
local h = image:height()
local w2, h2 = image:size()
local pixel = image:getPixel(0, 0)
image:setPixel(0, 0, Color4(255, 0, 0))
image:fill(Color4(0, 0, 0, 0))
image:upload()
image:save("runtime/player_copy.png")
image:unload()
```

Sounds:

```luau
local sound = assets.loadSound("assets/sfx.wav")
local generated = assets.newSound(44100, 1, 44100, 0)
local sampleRate = sound:sampleRate()
local channels = sound:channels()
local sampleCount = sound:len()
local value = sound:getSample(0)
sound:setSample(0, 0.5)
sound:save("runtime/generated.wav")
sound:unload()
```

Asset management:

```luau
assets.unloadImage("assets/player.png")
assets.unloadSound("assets/sfx.wav")
local imageCount, soundCount = assets.gc()
```

Edges:

- Images can be loaded from PNG, JPEG, GIF, BMP, TGA, TIFF, PNM, WebP, HDR, and DDS files.
- `loadImage` also accepts raw base64-encoded PNG data, `base64:` values, and
  `data:image/png;base64,...` URIs. `loadImageBase64` is the explicit raw-data API.
- `snapPhoto(x, y, x2, y2)` returns an `ImageHandle` containing that rectangle
  from the most recently rendered frame. Coordinates are top-left and
  bottom-right, and are clipped to the window. Call it after at least one frame.
- `getPixel` and `setPixel` use zero-based coordinates.
- Unloaded handles reject further reads, writes, uploads, and rendering.
- Relative `save` and `export` paths use the writable game data directory.
- Absolute and normalized parent-relative export paths may target any
  OS-permitted location. They are not restricted to the project, data, or
  executable directory.
- Image and sound exports receive `.png` or `.wav` extensions when omitted. A
  different extension is rejected.
- Relative asset loads check writable game data first, then fall back to
  bundled project resources.
- `assets.gc()` drops unloaded cache entries whose handles are no longer referenced.

::: tip
Use `fs.dataPath("generated/image.png")` when an export API needs the complete
path to the default writable data directory.
:::

<!-- page: audio | Audio -->
# Audio

Global: `audio`

```luau
audio.play(sound, true, 0.5)
audio.playOnce(sound)
audio.setVolume(sound, 0.25)
audio.stop(sound)
```

Edges:

- Native builds load WAV, MP3, OGG/Vorbis, and FLAC. Web builds also pass browser-decodable AAC/M4A and AIFF through WebAudio.
- Volume is clamped to `0..1`.
- Browser audio may not start until the user interacts with the page.
- `playOnce` is `play(sound, false, volume)`.

2D spatial playback uses world coordinates. Move the listener once per frame
when it follows a camera or player:

```luau
audio.setListenerPosition(camera.x, camera.y)
audio.playSpatial(sound, enemy.x, enemy.y, true, 0.8)
audio.setPosition(sound, enemy.x, enemy.y)
```

For entity-bound audio, `SpatialSound2D` owns the emitter position and follows
the entity automatically. Its sound can be selected from the editor Inspector:

```luau
local emitter = enemy:AddComponent(core.SpatialSound2D)
emitter.sound = assets.loadSound("assets/enemy.ogg")
emitter.volume = 0.8
emitter.looping = true
emitter:play()
```

<!-- page: file-system | File System -->
# File System

Global: `fs`

```luau
local runningOnWeb = fs.isWebasm()
local runningOnMobile = fs.isMobile()
local runningOnAndroid = fs.isAndroid()
local filePath = fs.openFilePicker()
local folderPath = fs.openFolderPicker()
local dataDirectory = fs.getDataDirectory()
local savePath = fs.dataPath("data/save.txt")
local text = fs.readFile("data/save.txt")
fs.writeFile("data/save.txt", "hello")
fs.appendFile("data/save.txt", "\nworld")
fs.exists("data/save.txt")
fs.isFile("data/save.txt")
fs.isDir("data")
fs.createDir("data")
local entries = fs.walk("data", true)
fs.rename("data/a.txt", "data/b.txt")
fs.copy("data/b.txt", "data/c.txt")
fs.removeFile("data/c.txt")
```

`fs.walk` entries include `path`, `name`, `kind`, `isFile`, `isDir`, `is_file`, and `is_dir`.

Edges:

- `fs.isWebasm()` and `fs.isWebAssembly()` return whether the game is running
  in the WebAssembly/browser build.
- `fs.isMobile()` returns whether the game is running on a mobile target or in
  the desktop mobile emulator.
- `fs.isAndroid()` returns whether the game is running on Android.
- `fs.openFilePicker()` and `fs.openFolderPicker()` return an absolute path
  string selected by the user, or `nil` when cancelled or unavailable. They use
  native desktop dialogs and return `nil` on WebAssembly and Android.
- Relative writes use the writable game data directory.
- Relative reads check writable game data first, then bundled project
  resources. Packaged games can therefore load embedded defaults and override
  them with saved data.
- `fs.getDataDirectory()` returns the default writable directory.
- `fs.dataPath(path)` resolves a relative path against that directory.
- Absolute paths are used directly.
- Parent-relative paths are normalized and may leave the data or project
  directory.
- Explicit paths are not restricted to the project, data, or executable
  directory. Operating-system permissions and platform sandboxes still apply.
- Directory creation creates parent directories as needed.
- `removeFile` returns `false` when the target is absent.

## Paths outside the data directory

```luau
fs.writeFile("/tmp/neolove/save.json", '{"level": 4}')
fs.appendFile("../shared/log.txt", "started\n")
fs.createDir("/tmp/neolove/maps")

local image = assets.newImage(64, 64, Color4(255, 0, 0))
image:export("/tmp/neolove/generated/icon.png")
```

::: info
The default data directory is still used for relative writes. To write
somewhere else, pass an absolute or parent-relative destination.
:::

<!-- page: mobile | Mobile -->
# Mobile

Global: `mobile`

```luau
if mobile.isMobile() then
    local width, height = mobile.getDeviceSize()
    local network = mobile.getNetworkType()
    local top, right, bottom, left = mobile.getSafeAreaInsets()
end
```

Functions:

- `mobile.isMobile()` returns `true` on mobile builds and in the desktop mobile
  emulator.
- `mobile.isEmulated()` returns whether the current run is the desktop mobile
  emulator.
- `mobile.isOnline()` returns whether the emulated or platform network is
  available.
- `mobile.isWifiEnabled()` and `mobile.isCellularEnabled()` expose emulator
  network toggles.
- `mobile.isLowPowerMode()` exposes the emulator low-power toggle.
- `mobile.getNetworkType()` returns `"wifi"`, `"cellular"`, or `"offline"`.
- `mobile.getOrientation()` returns `"portrait"` or `"landscape"`.
- `mobile.isLandscape()` returns whether the current mobile orientation is
  landscape.
- `mobile.getDeviceSize()` returns the locked logical mobile width and height.
- `mobile.getSafeAreaInsets()` returns top, right, bottom, and left safe-area
  insets.

Desktop emulator:

```bash
neolove run . --mobile --portrait --wifi
neolove run . --mobile --landscape --offline
neolove run . --mobile --mobile-size=430x932 --cellular --low-power
```

In emulator mode the game window is not resizable. Use portrait or landscape
rotation instead of arbitrary resizing. Keyboard events are suppressed so games
must use mouse/touch-style controls or request the on-screen keyboard through
`input.showKeyboard()` where the target platform supports it.

<!-- page: commands | Commands -->
# Commands

Globals: `commands`, `command`

```luau
local result = commands.run("echo", { "hello" }, ".")
local child = commands.runDetached("my-tool", { "--flag" }, ".")
```

`run` returns `ok`, `statusCode`, `status_code`, `stdout`, `stderr`, and `error`.

`runDetached` returns `ok`, `pid`, and `error`.

Edges:

- `cwd` is constrained to the project root.
- Prefer passing arguments separately instead of building shell command strings.

<!-- page: http | HTTP -->
# HTTP

Global: `http`

```luau
http.get("https://example.com", function(response)
    if response.ok then
        print(response.body)
    else
        print(response.error)
    end
end)

http.request({
    url = "https://example.com/api",
    method = "POST",
    headers = { ["Content-Type"] = "application/json" },
    body = "{\"hello\":true}",
}, function(response)
    print(response.status, response.body)
end)
```

`http.request(url, callback)`, `http.request(options, callback)`, and `http.get(url, callback)` return request ids. Options include `url`, `method`, `headers`, and string `body`. Responses include `ok`, `url`, `status`, `body`, `error`, and `headers`.

Edges:

- Requests are asynchronous.
- Native builds use a compact HTTP/HTTPS client; web builds use browser `fetch` and follow browser CORS rules.
- `_poll()` is internal and normally called by the engine.

<!-- page: servers | Servers -->
# Servers

Global: `servers`

```luau
local Chat = servers.define({
    onConnect = function(self, client)
        client:emit("welcome", { id = client.key })
    end,
    onMessage = function(self, client, event, data)
        if event == "chat" then
            self.host:emit("chat", { from = client.key, text = data.text })
        end
    end,
    onStart = function(self, host)
        self.host = host
    end,
})

local hosted = Chat:host(9000)
local client = Chat:connect(hosted.url)
client:on("welcome", function(data) print(data.id) end)
client:emit("chat", { text = "hello" })
```

`servers.define(table)` (also `service`, `createService`, and `create_service`)
turns an ordinary table into an in-process service class. It accepts optional
`onStart`, `onConnect`, `onMessage`, and `onDisconnect` methods. Messages use
named events and automatically serialize Luau data; a separate server Luau file
is not needed.

Server helpers:

- `servers.host(scriptPath, port, options?)`
- `servers.connect(url)`
- `servers.serializeTable(value)` / `servers.serialize_table(value)`
- `servers.deserializeTable(payload)` / `servers.deserialize_table(payload)`
- `servers.generateUuid4()` / `servers.generate_uuid4()`
- `servers.generateUuid7()` / `servers.generate_uuid7()`
- `servers.sha256(value)`
- `servers.sha128(value)`

Hosted server handles expose `client`, `port`, `url`, `stop()`, `getPort()`, and `getUrl()`.
They also expose `send(clientKey, payload)`, `broadcast(payload)`, `getClients()`,
`getClientCount()`, and, for class services, `emit(event, data)` and
`sendEvent(clientKey, event, data)`.

Client handles expose `key`, `is_host`, `send(payload)`, `addCallback(callback)`, `disconnect()`, `isConnected()`, `getKey()`, `isHost()`, and `getKickReason()`.

Edges:

- Payloads are Luau `buffer` values.
- The original `servers.host(scriptPath, port, options?)` buffer API remains
  available for low-level or isolated server runtimes.
- TLS host options support `certPath`/`keyPath` and `cert_path`/`key_path`.
- Hosts bind to `127.0.0.1` by default. For LAN clients, use
  `{ host = "0.0.0.0" }` and connect them to the machine's real IP address,
  for example `Chat:connect("http://192.168.1.20:9000")`.
- `_poll()` is internal and normally called by the engine.

<!-- page: shaders | Shaders -->
# Shaders

Global: `shaders`

```luau
local shader = shaders.loadFragment("shaders/pulse.glsl", {
    uniforms = { "time" },
    images = { "mask" },
})
shader:setUniform1f("time", 1.25)
shader:setUniformColor("tint", Color4(255, 128, 64))
```

Loading:

- `shaders.load(vertexPath, fragmentPath, options?)`
- `shaders.loadFragment(fragmentPath, options?)`
- `shaders.fromSource(vertexSource, fragmentSource, options?)`
- `shaders.fromFragmentSource(fragmentSource, options?)`

Uniform setters:

- `setUniform1f(name, x)`
- `setUniform2f(name, x, y)`
- `setUniform3f(name, x, y, z)`
- `setUniform4f(name, x, y, z, w)`
- `setUniformColor(name, color)`
- `setTexture(name, image)`

Edges:

- Custom shaders require the Vulkan renderer on desktop; build the engine with `--features vulkan`.
- Drawable components in the visual editor expose a Shader asset field under Advanced. Selecting a fragment shader exports a real `shaders.loadFragment(...)` handle.
- Web builds support fragment shaders on rectangles, triangles, circles, and images through the browser WebGL path, including float/vector uniforms. Shape commands use the built-in white `Texture` sampler, while image commands bind their source image to `Texture`.
- `shaders.DEFAULT_VERTEX_SHADER` contains the built-in vertex shader source.

<!-- page: tweening | Tweening -->
# Tweening

Globals: `tweening`, `tween`

```luau
local handle = tweening.to(entity, "x", 400, 1.0, "quad", "out", function()
    print("done")
end)
handle:cancel()
tweening.cancelAll()
tweening.update(dt)
```

Aliases: `to`, `new`, and `create`.

Easing styles: `linear`, `sine`, `quad`, `cubic`, `quart`, `quint`, `expo`, `circ`, `back`, `bounce`.

Directions: `in`, `out`, `inOut`, `in_out`.

Edges:

- Tweens target numeric fields.
- `cancelAll()` returns the number of cancelled tweens.
- The engine exposes `tweening.update(dt)` for explicit control.

<!-- page: animation | Animation -->
# Animation

Globals: `animation`, `animations`

Animation clips contain numeric property tracks and advance automatically
before game systems update. Tracks support linear, Bezier, and step/hold
interpolation. `.neoanim` files created by the editor can be loaded directly.

```luau
local clip = {
    duration = 1,
    looping = true,
    tracks = {
        { property = "x", keys = {
            { time = 0, value = 100 },
            { time = 1, value = 300 },
        }},
    },
}

local fileClip = animation.load("walk.neoanim")
local player = animation.play(entity, fileClip or clip)
player:pause()
player:seek(0.25)
player:setSpeed(2)
player:play()
```

Bezier keyframes use `interpolation = "bezier"` and optional handle fields
`out_x`, `out_y`, `in_x`, and `in_y`. Handles are normalized against the span
between adjacent keyframes.

`core.AnimationController` manages one clip on an entity:

```luau
local controller = entity:AddComponent(core.AnimationController)
controller.animation = animation.load("walk.neoanim")
controller.autoplay = true
controller.looping = true
controller.speed = 1

controller:pause()
controller:play()
controller:stop()
```

Controller fields are `animation`, `autoplay`, `looping`, `playing`, and
`speed`. `Play`, `Pause`, and `Stop` PascalCase aliases are also available.

<!-- page: ecs | ECS -->
# ECS

Global: `ecs`

Create entities:

```luau
local player = ecs.newEntity("player", ecs.root, 100, 100)
player.size_x = 64
player.size_y = 64
```

Entity fields:

- `id`
- `name`
- `x`, `y`
- `size_x`, `size_y`
- `scale`
- `rotation`
- `z`
- `anchor_x`, `anchor_y`
- `pivot_x`, `pivot_y`
- `position_pivot`
- `rotation_pivot`
- `rotation_pivot_x`, `rotation_pivot_y`
- `parent`
- `children`
- `components`
- `raycastable`

Entity methods:

```luau
entity:AddComponent(core.Rect2D)
entity:RemoveComponent(component)
entity:Duplicate(parent)
entity:Delete()
entity:FindFirstChild("name")
local wx, wy = entity:GetWorldPosition()
local rot = entity:GetWorldRotation()
local containsPoint = entity:IsInside(worldX, worldY)
```

`IsInside` returns whether a world-space point is within the entity's transformed
bounds, including parent transforms, scale, rotation, and pivots. Bounds edges
count as inside. `isInside` is a lower-camel alias.

ECS functions:

- `ecs.addSystem(system)`
- `ecs.newEntity(name, parent?, x?, y?)`
- `ecs.deleteEntity(entity)`
- `ecs.duplicateEntity(entity, parent)`
- `ecs.findFirstChild(parent, name)`
- `ecs.addComponent(entity, component)`
- `ecs.removeComponent(entity, indexOrComponent)`
- `ecs.loadScene(path)`
- `ecs.root`

Component shape:

```luau
local component = {
    awake = function(entity, component) end, -- optional
    update = function(entity, component, dt) end,
    destroy = function(entity, component) end,
}
```

Components receive instance methods after attachment:

```luau
component:Remove()
component:GetEntity()
```

Edges:

- `ecs.addComponent` deep-copies the component prototype.
- Custom component `awake` is optional. When present, it is queued for the first
  frame after attachment so scene-exported Inspector assignments are already on
  the component instance. Read Inspector-edited values from the `component`
  argument (often named `self`), not from the shared module prototype table.
- Core engine components may still run their internal setup immediately during
  attachment.
- Component `destroy` runs when removed; `onDestroy` is used as a fallback.
- Component prototypes must be tables.
- Runtime errors in callbacks are reported with component context.
- `ecs.loadScene(path)` loads a `.neoscene` file created by the visual editor
  and instantiates its active entities into the current world.

<!-- page: systems | Systems -->
# Systems

Systems are tables passed to `ecs.addSystem`.

```luau
ecs.addSystem({
    awake = function(self) end, -- optional
    update = function(self, dt) end,
    lateUpdate = function(self, dt) end,
    fixedUpdate = function(self, dt) end,
})
```

Use systems for global simulation, managers, spawning, and logic that does not naturally belong to one entity.
System `awake` is optional and runs once on registration when present.

<!-- page: transforms | Transforms -->
# Transforms

Globals: `transform`, `transforms`

```luau
local x, y = transform.getWorldPosition(entity)
local r = transform.getWorldRotation(entity)
local facing = transform.lookAt(x, y, targetX, targetY)
local entities = transform.GetEntitiesInFront(mouse.x, mouse.y, 0)
local hit = transform.raycast(0, 0, 1, 0, 500, { ignore = player })
local overlapping = transform.doTheyOverlap({ a, b, c })
```

## `GetEntitiesInFront(worldX, worldY, minimumZ?)`

Returns every non-root entity whose transformed bounds contain the world-space
point. Results are ordered frontmost-first by descending `z`. Entities with the
same `z` are ordered by descending entity id, matching the engine's stable
front-to-back ordering.

When `minimumZ` is provided, only entities with `z >= minimumZ` are returned.
The filter is omitted when `minimumZ` is `nil`.

```luau
local underMouse = transform.GetEntitiesInFront(mouse.x, mouse.y)
local foreground = transform.GetEntitiesInFront(mouse.x, mouse.y, 10)

local topEntity = underMouse[1]
```

`transform.getEntitiesInFront` is a lower-camel alias.

`transform.lookAt(fromX, fromY, toX, toY)` returns the world-space rotation in
radians required for the first position to face the second. Zero radians faces
right; positive rotation turns toward positive Y. `look_at` is an alias.

Transform rules:

- Entity `x` and `y` are local to the parent.
- Parent scale and rotation affect children.
- `size_x` and `size_y` are scaled by global scale.
- `anchor_x` and `anchor_y` offset against the parent bounds.
- `position_pivot = "center"` treats `x`, `y` as center position.
- `position_pivot = "top_right"` treats `x`, `y` as the top-right point.
- `pivot_x` and `pivot_y` override named position pivots and are fractions of the entity size.
- `rotation_pivot = "middle"` or `"center"` rotates around the center.
- `rotation_pivot_x` and `rotation_pivot_y` override the rotation pivot.

Raycast hit fields:

- `entity`
- `id`
- `distance`
- `x`, `y`
- `normalX`, `normalY`
- `normal_x`, `normal_y`

Edges:

- `raycast` direction is normalized internally.
- Zero-length ray directions return no hit.
- `max_distance` defaults to infinity and is clamped.
- `raycastable = false` excludes an entity.
- `ignore` and `ignoreEntity` accept one entity or an array of entities.
- `doTheyOverlap` uses entity bounds, not Spritebox2D pixel masks.

<!-- page: entity-listeners | Entity Listeners -->
# Entity Listeners

Supported events:

- `leftClick`
- `rightClick`
- `middleClick`
- `scrollUp`
- `scrollDown`
- `mouseEntered`
- `mouseExited`

Example:

```luau
local connection = button:Listen("leftClick", function(entity, event)
    print(event.x, event.y)
end)
connection:Disconnect()
```

Event fields include `kind`, `type`, `button`, `x`, `y`, `mouseX`, `mouseY`,
`localX`, `localY` (plus snake-case aliases), `wheelX`, `wheelY`, and `amount`.

Edges:

- Listener hit testing and local coordinates follow the complete world transform,
  including entity rotation, custom rotation pivots, parent rotation, and scale.
- Listener connections support `Disconnect`, `disconnect`, `IsConnected`, and `isConnected`.
- Deleting entities disconnects their listeners.

<!-- page: prefabs | Prefabs -->
# Prefabs

Globals: `prefabs`, `prefab`

```luau
local template = prefabs.capture(entity)
prefabs.register("enemy", template)
local enemy = prefabs.instantiate("enemy", ecs.root)

local fileTemplate = prefabs.load("prefabs/enemy.neoprefab")
local fileEnemy = prefabs.instantiate(fileTemplate, ecs.root)
```

Functions:

- `prefabs.capture(entity)`
- `prefabs.component(source, overrides?)`
- `prefabs.load(path)`
- `prefabs.register(name, source)`
- `prefabs.get(name)`
- `prefabs.remove(name)`
- `prefabs.instantiate(source, parent?)`
- `prefabs.duplicate(source, parent?)`

Built-in UI templates are under `prefabs.ui`: `label`, `panel`, `dialog`, `statusChip`, and `status_chip`.

Edges:

- Prefabs store entity fields, children, and components.
- `prefabs.component` is useful for making component prototypes with overrides.
- `instantiate` accepts a registered name, an entity, or a prefab template.
- The visual editor saves `.neoprefab` files from entity subtrees. Drag them
  into the editor viewport, or load them at runtime with `prefabs.load(path)`.
- Prefab paths use the same project/data resource resolution as `fs.readFile`.
- Script component paths stored in editor prefabs resolve from the project root,
  even when `prefabs.load` is called by a nested module. Entity and component
  Inspector references within the prefab are preserved and remapped per instance.
- Instantiation runs component `awake` callbacks once, in parent-to-descendant
  and component-list order, after the complete prefab tree and its references
  have been created. Prefab-authored component values are preserved across
  default initialization. Loading a prefab template does not run script `awake`.

<!-- page: rendering-components | Core Rendering Components -->
# Core Rendering Components

All drawable components share:

- `NEOLOVE_RENDERING = true`
- `color`
- `shader`
- `visible`

## Component Reference

Every built-in component available through the `core` module, and what it does.
Names joined by `/` are aliases for the same component. UI components (Panel,
Button, TextInput, Slider, Dropdown) default to a Visual Studio Code **Dark+**
colour scheme, and every state colour — including hover — is configurable.

| Component | Category | What it does |
| --- | --- | --- |
| `core.Rect2D` | Rendering | Draws a solid rectangle filling the entity bounds. |
| `core.Shape2D` | Rendering | Draws a primitive: box, circle, triangle, or right-triangle. |
| `core.ParticleSystem2D` | Rendering | Emits bounded particles as tinted circles or a sampled image. |
| `core.TextBox` / `core.TextLabel` / `core.RudimentaryTextLabel` | Text | Bounded or content-sized rich text with formatting and auto-fit. |
| `core.TextInput` | UI | Editable single-line field with caret, placeholder, focus, and submit/change callbacks. |
| `core.Panel` / `core.Frame` | UI | Container with background, border, rounded corners, and optional 9-slice image. |
| `core.Button` | UI | Clickable button with hover/pressed/disabled states, an optional icon, and click callbacks. |
| `core.Slider` | UI | Draggable value slider (horizontal or vertical) with a track, filled range, thumb, and `onChanged`. |
| `core.Dropdown` | UI | Selectable list with a scrollable popup menu and per-item styling. |
| `core.Sprite2D` / `core.Image2D` | Rendering | Draws an image scaled to the entity bounds, with an optional source rectangle. |
| `core.SpriteSheet2D` | Rendering | Draws and animates frames from a regular sprite atlas. |
| `core.NineSliceSprite2D` / `core["9SliceSprite2D"]` | Rendering | Nine-slice sprite: fixed corners and edges, stretched center. |
| `core.Spritebox2D` | Rendering | Sprite renderer that hit-tests against opaque pixels using `alpha_threshold`. |
| `core.TileTexture2D` | Rendering | Tiles an image across the entity bounds. |
| `core.Tilemap2D` | Rendering | Draws a grid of tiles sampled from an atlas. |
| `core.AnimationController` | Animation | Plays and blends a keyframe clip on the entity. |
| `core.EntityScaler` | Layout | Scales the entity to a target size, in pixels or as a fraction of the screen. |
| `core.Collider2D` | Physics | 2D collision shape for overlap and physics queries. |
| `core.Rigidbody2D` | Physics | Gives the entity velocity, forces, and physics integration. |
| `core.Bolt2D` / `core.LegacyBolt2D` | Physics | Rigid joint constraining two bodies together. |
| `core.Rope2D` / `core.String2D` | Physics | Distance joint with min/max length, stiffness, and damping. |
| `core.SpatialSound2D` | Audio | Positional audio emitter attached to the entity transform. |

Scripted behaviour modules can also appear in the editor's **Add Component**
picker — see [Custom Picker Components](#custom-picker-components).

## `core.Rect2D`

Draws a rectangle using the entity transform and size.

```luau
local box = ecs.addComponent(entity, core.Rect2D)
box.color = Color4(255, 0, 0)
```

## `core.Shape2D`

Draws primitive shapes.

Fields:

- `shape`: `box`, `circle`, `triangle`, `right_triangle`, `righttriangle`, or `rightangledtriangle`
- `triangle_corner`
- `offset_x`, `offset_y`
- `size_x`, `size_y`

If `size_x` or `size_y` is `0`, the entity size is used.

## `core.ParticleSystem2D`

Emits bounded particles from the entity transform. Without an image, particles
draw as tinted circles. When `image` is assigned, each particle draws the image
at its sampled particle size.

Common fields:

- `image`
- `playing`, `looping`, `visible`
- `duration`, `emission_rate`, `max_particles`
- `lifetime`, `speed`, `direction`, `spread`
- `start_size`, `end_size`
- `color_sequence`, `transparency_sequence`
- `shape`: `point`, `box`, or `circle`
- `radius`, `gravity_x`, `gravity_y`
- `shader`

Methods are `play`, `pause`, `stop`, and `emit`; PascalCase aliases are
available.

## `core.TextBox`

Bounded or content-sized text.

Fields:

- `text`
- `scale`
- `min_scale`
- `used_scale`
- `text_scale`: `none`, `fit`, `fit_width`, `fit_height`
- `align_x`: `left`, `center`, `right`
- `align_y`: `top`, `center`, `bottom`
- `wrap`: `none`, `word`, `char`, or boolean
- `size_mode`: `content`, `entity`, `box`, `bounds`
- `padding`, `padding_x`, `padding_y`
- `line_spacing`
- `letter_spacing`
- `tab_size`: number of spaces a tab character advances by, default `4`; `tab_width` is accepted as an alias
- `font`
- `antialiasing`: `inherit`, `off`, `standard`, or `high`. `inherit` uses `app.antiAliasing`.
- `scale_x`, `scale_y`, `dx`, `dy`, `line_count`

Rich text methods use zero-based, end-exclusive character ranges. Formatting is stored separately from `text`, so surviving ranges continue to apply after text reassignment when their indices still overlap the new string. Overlapping formatting is supported. `clearFormatting()` with no arguments clears the whole string; `clearAllFormatting()` is an explicit alias.

- `setBold(startIndex, endIndex)`
- `setItalic(startIndex, endIndex)`
- `setUnderline(startIndex, endIndex)`
- `setColor(startIndex, endIndex, Color4)`
- `setSize(startIndex, endIndex, scale)` relative to the component `scale`
- `setFont(startIndex, endIndex, fontPath)`
- `setOffset(startIndex, endIndex, x, y)` applies a pixel offset without changing character advance; `setPixelOffset` is an alias.
- `setCharacterOffset(charIndex, x, y)` offsets one character.
- `clearFormatting(startIndex?, endIndex?)`
- `clearAllFormatting()`
- `getLetterCount()`
- `getLetterPosition(charIndex)` returns `x, y` or nils when unavailable/out of range
- `getLetterBounds(charIndex)` returns `x, y, w, h` or nils when unavailable/out of range
- `getClosestLetterIndex(x, y)` returns the nearest zero-based cursor/insertion index for a world-space point, or nil when unavailable. `getClosestCharacterIndex` is an alias.

Aliases: `core.TextLabel`, `core.RudimentaryTextLabel`.

Edges:

- `font` may be `"default"` or a project-root path; web builds load project fonts from the bundled Emscripten filesystem through the browser FontFace API.
- Text can auto-fit within entity bounds using `text_scale`.
- Content-sized text is not culled before layout.

## `core.TextInput`

An editable `TextBox`-style field with a caret, placeholder, focus, locking,
font and alignment controls, rich-text formatting, password masking, and
submit/change callbacks.

```luau
local input = ecs.addComponent(entity, core.TextInput)
input.placeholder = "Player name"
input.font = "assets/fonts/ui.ttf"
input.align_x = "center"
input.onSubmit = function(entity, text) print(text) end
```

Set `locked = true` (or `enabled = false`) to prevent interaction. `focus()` and
`blur()` control focus from code. `onChanged`, `onSubmit`, `onFocus`, and
`onBlur` are optional callbacks. TextInput supports the same rich-text methods
as `TextBox`.

Colours default to the VS Code Dark+ input palette
(`background_color = Color4(60, 60, 60)`, `focus_border_color = Color4(0, 127, 212)`).

## `core.Panel` and `core.Frame`

A UI container: a filled rectangle with a border, rounded corners, and an
optional 9-slice background image. `core.Frame` is a backwards-compatible alias.

```luau
local panel = ecs.addComponent(entity, core.Panel)
panel.background_color = Color4(37, 37, 38) -- VS Code Dark+ sidebar
panel.border_color = Color4(69, 69, 69)
panel.corner_radius = 6
```

Fields: `background_color`, `border_color`, `border_width`, `corner_radius`,
`background_image`, and `slice_left`/`slice_right`/`slice_top`/`slice_bottom`.

## `core.Button`

An interactive button that renders a `Panel`-style background plus centered text
and an optional inline icon. It tracks hover and pressed state and exposes click
callbacks.

```luau
local button = ecs.addComponent(entity, core.Button)
button.text = "Play"
button.hover_background_color = Color4(17, 119, 187)
button.onClick = function(entity) print("clicked") end
```

Each state colour is configurable and defaults to the VS Code Dark+ button
palette (`background_color = Color4(14, 99, 156)`, hover `Color4(17, 119, 187)`):
`background_color`, `hover_background_color`, `pressed_background_color`,
`disabled_background_color`, and the matching `*_border_color` / `*_text_color`
fields. Callbacks: `onClick`, `onPress`, `onRelease`, `onHoverEnter`,
`onHoverLeave`. Set `enabled = false` to disable it.

## `core.Slider`

A draggable value slider with a track, a filled range, and a thumb. Works
horizontally or vertically and reports changes through `onChanged`.

```luau
local slider = ecs.addComponent(entity, core.Slider)
slider.min = 0
slider.max = 100
slider.value = 50
slider.onChanged = function(entity, value) print(value) end
```

Fields: `min`, `max`, `value`, `fraction` (read-only 0..1), `step` (0 = continuous),
`orientation` (`"horizontal"` or `"vertical"`), `thumb_size`, `track_thickness`,
`corner_radius`, and `thumb_corner_radius`. Colours default to the VS Code Dark+
palette and each has a configurable hover variant: `background_color` /
`hover_background_color` (track), `fill_color` / `hover_fill_color`, and
`thumb_color` / `hover_thumb_color`. `setValue(value)` sets the value from code
(clamped, without firing `onChanged`).

## `core.Dropdown`

A closed control that opens a scrollable popup menu of options. Each `options`
entry may be a string, or a table with `text`, `value`, and `image` fields.

```luau
local dropdown = ecs.addComponent(entity, core.Dropdown)
dropdown.options = { "Easy", "Normal", "Hard" }
dropdown.onChanged = function(entity, index, value) print(index, value) end
```

Fields: `options`, `selected_index` (1-based), `selected_text`, `selected_value`,
`placeholder`, `item_height`, `max_visible_items`, and `open_upwards`. Closed-state
colours (`background_color`, `hover_background_color`, `open_background_color`),
menu colours (`menu_background_color`, `menu_border_color`), and per-item colours
(`item_background_color`, `item_hover_background_color`,
`item_selected_background_color`, plus their `*_text_color` counterparts) are all
configurable and default to the VS Code Dark+ dropdown/list palette
(selection `Color4(9, 71, 113)`, hover `Color4(42, 45, 46)`).

## Custom Picker Components

Behaviour scripts can register themselves in the editor's **Add Component**
picker by calling `IComponentPicker(Behaviour)` at module scope. The script then
appears in the picker's search results alongside the core components and can be
attached to entities like any other component.

```luau
local Behaviour = {
    speed = Inspector(100),
}

function Behaviour.awake(entity, self) end
function Behaviour.update(entity, self, dt) end

IComponentPicker(Behaviour)
return Behaviour
```

The picker's search box is focused as soon as it opens; typing filters the list,
and pressing Enter adds the top match to the selected entity.

## `core.Sprite2D` and `core.Image2D`

Draws an image scaled to the entity bounds. `Image2D` is kept as an alias-compatible sprite renderer.

```luau
local sprite = ecs.addComponent(entity, core.Sprite2D)
sprite.image = assets.loadImage("assets/player.png")
```

Optional source rectangle fields:

- `source_x`
- `source_y`
- `source_w` or `source_width`
- `source_h` or `source_height`
- Camel-case aliases are also accepted: `sourceX`, `sourceY`, `sourceW`, `sourceH`, `sourceWidth`, `sourceHeight`

Edges:

- Source rectangles use image pixel coordinates.
- If no source rectangle is supplied, the whole image is drawn.
- Rendering is skipped when `image` is nil or unloaded.

## `core.SpriteSheet2D`

Draws and animates frames from a regular sprite atlas.

```luau
local sprite = ecs.addComponent(entity, core.SpriteSheet2D)
sprite.image = assets.loadImage("assets/player-sheet.png")
sprite.frame_width = 32
sprite.frame_height = 32
sprite.frame_count = 8
sprite.spacing = 1 -- optional atlas padding between frames
sprite.fps = 12
sprite:play()
```

`columns = 0` and `frame_count = 0` automatically derive both values from the
image dimensions. Frames are zero-based. Use `play()`, `pause()`, `stop()`, or
`setFrame(frame)`; `looping` controls whether playback wraps or stops on the
last frame. The visual editor previews the selected `frame` directly.

## `core.NineSliceSprite2D` and `core["9SliceSprite2D"]`

Draws a nine-sliced sprite: fixed corners, fixed edges, stretched center.

```luau
local panel = ecs.addComponent(entity, core.NineSliceSprite2D)
panel.image = assets.loadImage("assets/panel.png")
panel.slice_left = 8
panel.slice_right = 8
panel.slice_top = 8
panel.slice_bottom = 8
```

Fields:

- `image`
- `slice_left`, `slice_right`, `slice_top`, `slice_bottom`
- Camel-case aliases: `sliceLeft`, `sliceRight`, `sliceTop`, `sliceBottom`
- Optional source rectangle fields from `Sprite2D`

Edges:

- When all slice values are `0`, it draws as a normal sprite.
- If the entity is smaller than the fixed edges, edge sizes are scaled down to fit.
- `core["9SliceSprite2D"]` is available for code that wants the numeric name. Use bracket access because `core.9SliceSprite2D` is not valid Luau syntax.

## `core.TileTexture2D`

Repeats an image over the entity bounds.

Fields:

- `image`
- `tile_width`, `tile_height`
- `offset_x`, `offset_y`

Edges:

- `tile_width` and `tile_height` default to the image size when `0`.
- Tile layers are culled in local space before queueing tiles, including when
  the entity is rotated.

## `core.Tilemap2D`

Draws a finite grid from an atlas. Set `map_width`, `map_height`, `tile_width`,
and `tile_height`. `tiles` is a flat numeric array or a comma/whitespace-separated
string. Tile `0` is the first atlas cell and `-1` is empty. `spacing` and
`margin` support packed atlases. In the visual editor, select a `Tilemap2D`
component and use Paint mode in the Inspector to edit `tiles` directly inside
the entity bounds. Large maps cull off-screen cell ranges in tilemap-local
space, including rotated maps.

<!-- page: spritebox2d | Spritebox2D -->
# Spritebox2D

`core.Spritebox2D` computes a geometric hit shape from the opaque pixels of a sprite on the same entity. It is intended for click hit testing, custom overlap checks, and gameplay collision checks that need to ignore transparent sprite padding.

Basic usage:

```luau
local entity = ecs.newEntity("button", ecs.root, 100, 100)
entity.size_x = 128
entity.size_y = 64

local sprite = entity:AddComponent(core.Sprite2D)
sprite.image = assets.loadImage("assets/button.png")

local spritebox = entity:AddComponent(core.Spritebox2D)
spritebox:ComputeSpritebox()

if spritebox:IsInside(mouse.x, mouse.y) then
    print("pixel-shaped hit")
end
```

Methods:

- `spritebox:ComputeSpritebox() -> boolean`
- `spritebox:computeSpritebox() -> boolean`
- `spritebox:IsInside(x, y) -> boolean`
- `spritebox:isInside(x, y) -> boolean`
- `spritebox:IsIntersecting(otherEntityOrSpritebox) -> boolean`
- `spritebox:isIntersecting(otherEntityOrSpritebox) -> boolean`

Fields:

- `computed`
- `alpha_threshold`
- `rect_count`
- `bounds_x`, `bounds_y`, `bounds_w`, `bounds_h`

How it works:

- `ComputeSpritebox` searches the same entity for `Sprite2D`, `Image2D`, `NineSliceSprite2D`, or `core["9SliceSprite2D"]`.
- It reads that component's `image` and optional source rectangle.
- It scans alpha values and builds a merged rectangle cover of opaque pixels.
- Rectangles are stored in normalized sprite space so the shape follows entity scale and size.
- `IsInside` expects world coordinates.
- `IsIntersecting` accepts either an entity containing a Spritebox2D or another Spritebox2D component instance.
- Intersection uses a world-space AABB broad phase and per-rectangle SAT checks, so rotated entities are handled.

Edges:

- Call `ComputeSpritebox` after assigning or changing the sprite image.
- Recompute after changing `alpha_threshold`, image pixels, source rectangle, or nine-slice source/slice settings.
- For exact nine-slice hit shape after resizing, recompute after changing entity size.
- All-transparent sprites compute successfully but `rect_count` is `0`; hit checks return `false`.
- `alpha_threshold = 0` means any alpha greater than zero is inside. Higher values exclude soft or semi-transparent pixels.
- `transform.doTheyOverlap` does not use Spritebox2D; use `spritebox:IsIntersecting(...)` for pixel-shaped checks.
- Spritebox2D is not automatically wired into Rigidbody2D physics. It is a gameplay/query shape.

<!-- page: physics-components | Physics Components -->
# Physics Components

## `core.Collider2D`

Axis-aligned collision shape used by the Rigidbody2D solver.

Fields:

- `enabled`
- `is_trigger`
- `non_physics`
- `offset_x`, `offset_y`
- `size_x`, `size_y`
- `shape`
- `triangle_corner`
- `restitution`
- `friction`
- `touching`
- `last_hit_id`

Callbacks:

- `onCollisionEnter`
- `onCollisionStay`
- `onCollisionExit`
- `onTriggerEnter`
- `onTriggerStay`
- `onTriggerExit`

Setters:

```luau
collider:setOnCollisionEnter(function(selfEntity, selfCollider, otherEntity, otherCollider, otherId)
end)
```

Edges:

- Collider sizes default to the entity size when component sizes are `0`.
- Trigger callbacks do not apply physical response.
- `non_physics` colliders can be used for callbacks without participating as normal physics bodies.

## `core.Rigidbody2D`

Force-based body.

Fields:

- `velocity_x`, `velocity_y`
- `force_x`, `force_y`
- `acceleration_x`, `acceleration_y`
- `gravity_x`, `gravity_y`, `gravity_scale`
- `mass`
- `inertia`
- `linear_damping`, `angular_damping`
- `restitution`
- `friction`
- `sleep_epsilon`
- `bounds_mode`: `none` or `window`
- `freeze_x`, `freeze_y`, `freeze_rotation`
- `is_static`
- `collision_enabled`
- `grounded`
- `max_speed`
- `max_angular_speed`
- `angular_velocity`
- `torque`

Methods:

```luau
rb:addForce(0, -100)
rb:addImpulse(200, 0)
rb:addTorque(5)
rb:addAngularImpulse(1)
rb:setVelocity(100, 0)
local vx, vy = rb:getVelocity()
rb:setAngularVelocity(0)
local omega = rb:getAngularVelocity()
rb:setGravity(0, 980)
```

Edges:

- Static bodies force linear and angular velocity to zero.
- `collision_enabled = false` disables Rigidbody collision solving for that body.
- `bounds_mode = "window"` constrains bodies to the window bounds.

## `core.Bolt2D`

Pins the entity's rotation pivot to another entity. Low strength keeps the pivot attached while allowing the entity to rotate around the bolt point; high strength increasingly resists that rotation.

Fields:

- `enabled`
- `target_entity`, `target`
- `x`, `y`
- `offset_x`, `offset_y`
- `strength`
- `contacts_enabled`
- `current_force`, `force`

Method:

```luau
bolt:attach(targetEntity)
```

Edges:

- Add `Bolt2D` to the entity being pinned.
- `x` and `y` are target-local offsets from the target entity's rotation pivot. `offset_x` and `offset_y` are aliases.
- The bolted entity's own rotation pivot is the attached point.
- `strength` is clamped to `0..1`; `0` disables the bolt, low values allow free rotation around the attached point, and `1` locks rotation as well as position.
- `link(targetEntity)` is an alias for `attach(targetEntity)`.

## `core.LegacyBolt2D`

Previous Bolt2D behavior: soft spring-like positional following.

Fields and methods match `core.Bolt2D`.

Edges:

- `strength` is clamped to `0..1`; `0` disables the legacy bolt, `1` creates a hard positional pin, and values between use soft linear motors that can lag behind the target point.

## `core.Rope2D` and `core.String2D`

Distance constraint between two entities.

Fields:

- `enabled`
- `entity_a`, `entity_b`
- `min_length`
- `max_length`
- `stiffness`
- `damping`
- `break_force`
- `current_length`
- `tension`
- `snapped`

Method:

```luau
rope:link(entityA, entityB)
```

Edges:

- `String2D` is an alias of `Rope2D`.
- `break_force = 0` means no break threshold.
- Rope constraints are solved globally during the physics step.

<!-- page: rendering-details | Rendering Details -->
# Rendering Details

Draw order is sorted by `z`, then entity id and component order for stable output. Equal `z` values preserve deterministic order.

Texture filtering comes from `app.nearestNeighborScaling`:

- `true`: nearest neighbor.
- `false`: linear filtering.

Anti-aliasing comes from `app.antiAliasing`:

- `off`: hard one-sample edges and pixel-hard text masks.
- `standard`: 2× software edge coverage and normal grayscale glyph rasterization.
- `high`: 4× software geometry edge coverage and 2× supersampled text with
  premultiplied downsampling. Individual text components can override this with
  their `antialiasing` field.

Vulkan builds use the best supported device MSAA level for geometry; the text
quality modes still apply because glyphs are rasterized before GPU upload.

Rendering is skipped for components with `visible = false`, nil images, unloaded images, zero or negative sizes, or fully transparent colors.

<!-- page: webassembly | WebAssembly -->
# WebAssembly

`neolove build --webasm` emits:

- `dist/webasm/index.html`
- `dist/webasm/neolove.js`
- `dist/webasm/neolove.wasm`
- `dist/webasm/neolove.data`
- `dist/<project-name>-webasm.zip`

Local test command:

```bash
cd dist/webasm
python3 -m http.server 8000
```

Then open `http://localhost:8000`.

Edges:

- First web builds may install `wasm32-unknown-emscripten` and a local Emscripten toolchain under `~/.neolove/toolchains/emsdk`.
- Web audio requires browser permission or user gesture.
- Web shader effects are rendered through WebGL and composited with the software-rendered scene; unshaded software chunks are dirty-rect composited to avoid full-canvas copies around shader draws.

<!-- page: android | Android -->
# Android

`neolove build --android` emits a signed APK:

- `dist/<project-name>-android-arm64.apk`

The APK contains the optimized NeoLOVE Android runtime plus the compressed
project payload in the APK assets. On first use, the builder installs missing
toolchain pieces into `~/.neolove/toolchains/`: a JDK, Android command-line
tools, build-tools, platform SDK, NDK, and the `aarch64-linux-android` Rust
target.

Global: `android`

```luau
if fs.isAndroid() then
    local id = android.getDeviceId()
    local sdk = android.getSdkInt()
    local model = android.getModel()
    android.showKeyboard()
end
```

Functions:

- `android.isAndroid()` returns whether the game is running on Android.
- `android.getDeviceId()` returns Android's app-scoped secure device ID when available.
- `android.getSdkInt()` and `android.getApiLevel()` return the Android SDK/API level.
- `android.getBrand()`, `android.getManufacturer()`, `android.getModel()`,
  `android.getDevice()`, and `android.getProduct()` return Android build fields
  when available.
- `android.showKeyboard()` / `android.openKeyboard()` request the on-screen
  keyboard and return `true` when the Android runtime is available.
- `android.hideKeyboard()` / `android.closeKeyboard()` request that the
  on-screen keyboard close and return `true` when the Android runtime is
  available.

On non-Android targets, `android.isAndroid()` returns `false` and the data
getters return `nil`; keyboard functions return `false`.

<!-- page: ios | iOS -->
# iOS

`neolove build --ios` emits an iOS simulator app on macOS:

- `dist/<project-name>-ios-simulator.app`

The iOS builder wraps the WebAssembly output in a generated Xcode project and
uses `xcodebuild` with the `iphonesimulator` SDK. It requires macOS with Xcode
installed. Code signing is disabled for simulator builds.

On non-macOS hosts, the command fails immediately with a platform requirement
message instead of attempting a partial build.

<!-- page: performance | Performance Guidance -->
# Performance Guidance

- Prefer `Spritebox2D:ComputeSpritebox()` once after image setup instead of every frame.
- Reuse loaded image and sound handles.
- Use `TileTexture2D` for repeated backgrounds instead of manually creating many sprite entities.
- Keep `z` changes intentional; rendering order work is stable but still per-frame.
- For collision-heavy gameplay, use Collider2D/Rigidbody2D for broad physical simulation and Spritebox2D for precise final checks.
- Use `assets.gc()` after unloading large batches of assets.
- Keep web bundles lean by removing unused assets before building.
- Break CPU-heavy async tasks into bounded chunks and call `async.yield()`
  regularly. Scheduling a coroutine does not preempt long-running Luau code.
- On web, group shader-heavy entities where possible; the runtime avoids full-frame uploads for software-only frames and dirty-rect composites mixed software/shader chunks, but each WebGL shader switch still has browser overhead.

<!-- page: troubleshooting | Troubleshooting -->
# Troubleshooting

- `component prototype is nil`: the component table does not exist. Check spelling, especially `core["9SliceSprite2D"]` bracket access.
- `image is unloaded`: a handle was unloaded and then reused.
- Files fail to save beside packaged resources: use a relative `fs` path or
  `fs.dataPath()` so the writable game data directory is used.
- An async task freezes a frame: split the callback into bounded work and call
  `async.yield()` between chunks.
- An async task stops unexpectedly: inspect `task:getStatus()` and
  `task:getError()`.
- Sprite clicks hit transparent padding: use `core.Spritebox2D` and call `ComputeSpritebox`.
- Spritebox always returns false: verify the sprite component is attached before the Spritebox computes, the image has alpha greater than `alpha_threshold`, and world coordinates are being passed to `IsInside`.
- Web build is blank from `file://`: serve through HTTP.
- Custom shader errors in web builds: verify the browser supports WebGL and that the fragment shader is valid GLSL ES 1.00 (`#version 100`).
