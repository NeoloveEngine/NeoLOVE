# NeoLOVE Engine Documentation (Single-File Reference)

This document consolidates the engine documentation from the current repository source code.

- Repository: `NeoLOVE`
- Generated from source on: 2026-03-08
- Primary sources: `README.md`, `src/*.rs`, `neolove_engine_api.d.luau`, sample projects

## 1. What NeoLOVE Is

NeoLOVE is a Rust game engine/runtime that executes Luau game code (`main.luau`) with:

- ECS-style entities/components/systems
- 2D rendering components
- 2D physics (Rapier-based)
- Asset management (images + WAV audio)
- Input, filesystem, HTTP, commands, shader modules
- CLI for project scaffolding, running, API generation, and packaging

## 2. CLI

From `README.md` + `src/main.rs`:

```bash
neolove new <project-name>
neolove run [project-dir]
neolove build [project-dir]
neolove api [project-dir]
neolove setup-path
neolove --help
neolove --version
```

### Command behavior

- `new <project-name>`
  - Creates project directory with:
    - `main.luau`
    - `neolove.toml`
    - `assets/`
    - `.luaurc`
    - `.vscode/settings.json`
    - `types/neolove_engine_api.d.luau`
- `run [project-dir]`
  - Runs project after validating `<project>/main.luau` exists and is a file.
- `build [project-dir]`
  - Builds a standalone executable in `<project>/dist/` by embedding project files into the current engine binary.
  - `.luau`/`.lua` files are compiled to Luau bytecode for the embedded payload.
- `api [project-dir]`
  - Writes/updates API type definitions to `types/neolove_engine_api.d.luau` (and root copy if present).
- `setup-path`
  - Adds the engine binary directory to user PATH (platform-specific behavior).

## 3. Project Configuration (`neolove.toml`)

Parsed keys currently used by engine:

- `[package] name = "..."`
- `[window] title = "..."`
- `[window] icon = "..."`

### Effects

- Window title resolution priority:
  1. `[window].title`
  2. `[package].name`
  3. fallback: `NeoLOVE`
- Window icon:
  - If `[window].icon` points to a readable image, engine generates 16x16/32x32/64x64 icon variants via nearest-neighbor resize.

## 4. Runtime Model

## Startup sequence (high level)

1. Initialize Luau compiler settings for runtime.
2. Install `require` and `softrequire`.
3. Install globals/modules (`app`, `input`, `assets`, `audio`, `fs`, `http`, `commands`, `shaders`, `ecs`, `transform`/`transforms`, `core`, etc.).
4. Load and execute project entry: `main.luau`.

## Per-frame update order

1. Refresh `mouse` and `window` globals.
2. Poll pending HTTP callbacks (`http._poll()`).
3. Clear screen with `app.bg`.
4. Run all system `update(system, dt)` callbacks.
5. Iterate entities by `z` order and run non-rendering component updates.
6. Run Rapier physics step and synchronization.
7. Run rendering component updates.

FPS display/cap behavior:

- `app.showFps` defaults to enabled (`true`) and draws FPS text each frame.
- `app.maxFps` defaults to `60` and sleeps to cap frame rate.
- `app.setMaxFps(nil)` or invalid/non-positive values disables FPS cap.

## 5. Global Tables and Functions

Globals exposed to Luau include:

- `app`
- `input` and alias `userInput`
- `assets`
- `audio`
- `fs`
- `http`
- `commands` and alias `command`
- `shaders`
- `ecs`
- `transform` and alias `transforms`
- `core`
- `mouse` (table: `x`, `y`)
- `window` (table: `x`, `y`)
- `Color4(r,g,b,a?)`
- `die()`
- `softrequire(modulePath, allowed?)`

## 6. Module Semantics and Details

## 6.1 `app`

Fields and controls:

- `app.bg`: clear color table (`r,g,b,a`)
- `app.nearestNeighborScaling`: boolean (default `true`)
- `app.setMaxFps(number?)`
- `app.getMaxFps()`
- `app.setShowFps(boolean?)`
- `app.getShowFps()`
- `app.setNearestNeighborScaling(boolean?)`
- `app.getNearestNeighborScaling()`

Texture rendering components consult `app.nearestNeighborScaling` to choose nearest vs linear filtering.

## 6.2 `input` / `userInput`

Supports keyboard/mouse state and text input:

- `isKeyDown`, `isKeyPressed`, `isKeyReleased`
- `isMouseDown`, `isMousePressed`, `isMouseReleased`
- `getMouseWheel`, `isScrollingIn`, `isScrollingOut`, `getScrollInAmount`
- `getMouseDelta`
- `setMouseLocked`, `isMouseLocked`
- `getLastKeyPressed`, `getCharPressed`

Mouse button names support aliases like `left/lmb`, `right/rmb`, `middle/mmb/wheel`.

Key names are normalized case-insensitively and non-alphanumeric characters are ignored; many aliases are accepted (letters, digits, function keys, arrows, numpad, modifiers, etc.).

## 6.3 `assets`

Image and sound handles are userdata objects with explicit upload/unload control.

### Paths and caching

- Relative paths are resolved against project root.
- For non-prefixed asset paths, engine also resolves under `assets/`.
- Path-keyed weak-cache exists for loaded files.
- `assets.gc()` removes stale weak cache entries (returns removed image/sound counts).

### Image support

- `assets.loadImage(path)`
- `assets.newImage(width, height, color?)`
- Handle methods: `width`, `height`, `size`, `getPixel`, `setPixel`, `fill`, `upload`, `unload`, `isUnloaded`

### Sound support

- WAV loading via `assets.loadSound(path)`.
- Generated sound buffers via `assets.newSound(sampleRate, channels, len, fill?)`.
- Handle methods: `sampleRate`, `channels`, `len`, `getSample`, `setSample`, `upload`, `unload`, `isUnloaded`.

### Unload helpers

- `assets.unloadImage(value)` accepts image handle or path string.
- `assets.unloadSound(value)` accepts sound handle or path string.

## 6.4 `audio`

Playback control for `SoundHandle`:

- `play(sound, looped?, volume?)`
- `playOnce(sound, volume?)`
- `stop(sound)`
- `setVolume(sound, volume)`

Volumes are clamped to `[0.0, 1.0]`.

## 6.5 `fs`

File API is sandboxed to project root:

- `readFile(path)`
- `writeFile(path, content)`
- `appendFile(path, content)`
- `exists(path)`
- `isFile(path)`
- `isDir(path)`
- `createDir(path)`
- `walk(path?, recursive?)`
- `rename(from, to)`
- `copy(from, to)`
- `removeFile(path)` -> `true/false`

Path traversal outside project root is rejected.

## 6.6 `http`

Asynchronous callback-based HTTP client with polling model:

- `request(url, callback)`
- `get(url, callback)` (alias)
- internal: `_poll()` (called each frame by engine)

Important constraints:

- Both `http://` and `https://` URLs are supported.
- GET-only request behavior.
- Response payload includes `ok`, `url`, `status`, `body`, `error`, `headers`.

## 6.7 `commands` / `command`

Process execution module (cwd sandboxed to project root):

- `run(command, args?, cwd?)` -> `{ ok, status_code, stdout, stderr, error? }`
- `runDetached(command, args?, cwd?)` -> `{ ok, pid, error? }`

`cwd` that escapes project root is rejected.

## 6.8 `shaders`

Shader/material management:

- `DEFAULT_VERTEX_SHADER`
- `load(vertexPath, fragmentPath, options?)`
- `loadFragment(fragmentPath, options?)`
- `fromSource(vertexSource, fragmentSource, options?)`
- `fromFragmentSource(fragmentSource, options?)`

Shader handle methods:

- `setUniform1f`, `setUniform2f`, `setUniform3f`, `setUniform4f`
- `setUniformColor`
- `setTexture`

`options.uniforms` supports string or table descriptors; `options.textures` declares texture samplers.

## 6.9 `softrequire`

`softrequire(modulePath, allowed?)` loads a module file in a restricted sandbox and caches by canonical file path.

- Auto extension resolution for `.luau` / `.lua`.
- Directory path resolves to `init.luau`.
- Module path is restricted to project root.
- Sandbox includes selected base functions/libs.
- `allowed` can expose additional globals/modules.

## 7. ECS and Transform System

## Entities

Entity table baseline fields:

- `id`, `name`
- `x`, `y`, `rotation`
- `anchor_x`, `anchor_y` normalized against the parent bounds (`0..1` typical range)
- `pivot_x`, `pivot_y` optional normalized position-pivot override for the entity bounds
- `rotation_pivot` (default `topleft`)
- `rotation_pivot_x`, `rotation_pivot_y` optional normalized rotation-pivot override
- `position_pivot` (supports `center`, `top_right`)
- `z`
- `size_x`, `size_y`
- `scale`
- `parent`, `children`
- `components`

Layout notes:

- Global origin resolution now applies `anchor_*` against the parent size before local `x/y`.
- Numeric `pivot_*` fields override legacy `position_pivot` helpers when present.
- Numeric `rotation_pivot_*` fields override `rotation_pivot`; if omitted they fall back to `pivot_*`.
- The root ECS entity size tracks the current window size, which makes anchor-based UI layout practical without a manual resize system.

## ECS operations

- `ecs.newEntity(name, parent?, x?, y?)`
- `ecs.deleteEntity(entity)` (recursive)
- `ecs.duplicateEntity(targetEntity, parent)`
- `ecs.findFirstChild(parent, name)`
- `ecs.addComponent(entity, componentPrototype)`
- `ecs.removeComponent(entity, indexOrComponent)`
- `ecs.root`

## Systems

- `ecs.addSystem(system)`
- System callbacks (if present): `awake`, `update`, `lateUpdate`, `fixedUpdate`
- Current engine frame loop invokes system `update` callback.

## Transform helpers

- `transform.getWorldPosition(entity)`
- `transform.getWorldRotation(entity)`
- `transform.doTheyOverlap(entities)` (AABB overlap across list)
- `transform.raycast(...)`

Raycast behavior:

- Normalizes direction.
- Optional `max_distance`.
- Optional ignore options (`ignore`, `ignoreEntity`) accept entity or list.
- Tests against global AABBs of raycastable entities (`raycastable=false` excludes).

## 8. Core Components (`core`)

Core component prototypes are added under `core` and cloned into entities via `ecs.addComponent`.

## 8.1 `Rect2D`

- Rendering rectangle with color, visibility, optional shader.
- Uses entity transform/size.

## 8.2 `Shape2D`

- Primitive rendering: box, circle, triangle/right-triangle.
- Fields include `shape`, `triangle_corner`, offsets and optional explicit size overrides.

## 8.3 `TextBox`

- Bounded text component with label-style defaults.
- Supports built-in/default font fallback, project-relative custom font loading, alignment, padding, wrapping, and auto-fit text scaling.
- Key fields: `text`, `scale`, `min_scale`, `used_scale`, `text_scale`, `align_x`, `align_y`, `wrap`, `size_mode`, `padding(_x/_y)`, `line_spacing`, `letter_spacing`, `font`, `dx`, `dy`, `line_count`.
- `size_mode = "content"` keeps old lightweight label behavior; `size_mode = "entity"` uses the entity bounds as the text box.
- `RudimentaryTextLabel` remains as a compatibility alias for `TextBox`.

## 8.4 `TextLabel`

- Alias of `TextBox` with the same configurable text layout/font-loading behavior.

## 8.5 `Image2D`

- Draws image handle tinted by component color.
- Scales to entity size.

## 8.6 `TileTexture2D`

- Repeats image tiles over entity area.
- Supports tile dimensions and offsets.
- Applies culling optimization for non-rotated cases.

## 8.12 `Collider2D`

- Collider fields: enabled, trigger flags, offsets/size, shape, friction/restitution, callbacks, runtime state.
- Callback helper methods: `setOnCollisionEnter`, `setOnCollisionStay`, `setOnCollisionExit`, `setOnTriggerEnter`, `setOnTriggerStay`, `setOnTriggerExit`.

## 8.13 `Rigidbody2D`

- Force/velocity/rotation properties and methods.
- Supports gravity, damping, constraints, static mode, window bounds mode, max speed caps.
- Methods: `addForce`, `addImpulse`, `addTorque`, `addAngularImpulse`, `setVelocity`, `getVelocity`, `setAngularVelocity`, `getAngularVelocity`, `setGravity`.

## 8.14 `Rope2D` / `String2D`

- Distance-constraint joint between two entities.
- Fields include min/max length, stiffness, damping, break force, tension, snapped state.
- `link(entityA, entityB)` helper.

## 9. Physics (Rapier2D) Behavior

Physics is rebuilt when topology/signature changes and stepped each frame with clamped dt.

Highlights:

- Supports collider shapes: box, circle, right triangle.
- Trigger/non-physics colliders are configured as sensors.
- Collision and trigger events track enter/stay/exit transitions.
- Both camelCase and snake_case callback names are honored internally:
  - `onCollisionEnter` / `on_collision_enter`
  - `onCollisionStay` / `on_collision_stay`
  - `onCollisionExit` / `on_collision_exit`
  - `onTriggerEnter` / `on_trigger_enter`
  - `onTriggerStay` / `on_trigger_stay`
  - `onTriggerExit` / `on_trigger_exit`
- Rigidbody values are synchronized back to entity transforms each frame.
- `bounds_mode = "window"` applies window edge collisions with restitution.
- `grounded` is inferred from contacts and bottom window collision.
- Rope joints update `current_length` and `tension`; if `break_force` exceeded, rope disables and marks `snapped=true`.

## 10. Build / Packaging Details

`neolove build` behavior:

- Recursively packages project files excluding `.git`, `target`, `dist`.
- Compiles Luau/Lua files to bytecode for embedded payload.
- Appends payload + trailer magic (`NEOLOVE_EMBED_V1`) to engine executable.
- Output executable written to `<project>/dist/<sanitized-name>`.
- On Unix, executable permissions are set.

Running built executable:

- If executable contains embedded payload and is launched without CLI args, payload is extracted to temp cache and run directly.

## 11. Safety / Sandboxing Summary

Current safeguards in code:

- `fs` module path resolution constrained to project root.
- `commands` cwd constrained to project root.
- `softrequire` canonical path constrained to project root.
- Embedded payload unpacking rejects unsafe relative paths (`..`, absolute paths).

## 12. Samples Included

Samples are under `samples/`:

- `asset_unload_gc`
- `blackjack`
- `dodge`
- `fs_http_lab`
- `new_features_test`
- `physics_repeatability`
- `raycasting`
- `rigidbody2d`
- `shaders`
- `text_layout_lab`
- `ui_showcase`
- `warp_rush`

Use these to see practical usage of rendering, shaders, physics, raycasts, assets, and input patterns.

## 13. Full Luau API Type Reference

The following is the repository type declaration file verbatim from `neolove_engine_api.d.luau`.

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

export type ComponentInstance = {
	entity: Entity?,
	awake: ((entity: Entity, component: ComponentInstance) -> ())?,
	update: ((entity: Entity, component: ComponentInstance, dt: number) -> ())?,
	destroy: ((entity: Entity, component: ComponentInstance) -> ())?,
	onDestroy: ((entity: Entity, component: ComponentInstance) -> ())?,
	NEOLOVE_RENDERING: boolean?,
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
	unload: (self: SoundHandle) -> (),
	isUnloaded: (self: SoundHandle) -> boolean,
}

export type RaycastHit = {
	entity: Entity,
	id: number,
	distance: number,
	x: number,
	y: number,
	normal_x: number,
	normal_y: number,
}

export type RaycastOptions = {
	ignore: Entity | { Entity }?,
	ignoreEntity: Entity | { Entity }?,
}

export type AppModule = {
	bg: Color4Value,
	setMaxFps: (fps: number?) -> (),
	getMaxFps: () -> number?,
	setShowFps: (enabled: boolean?) -> (),
	getShowFps: () -> boolean,
	nearestNeighborScaling: boolean,
	setNearestNeighborScaling: (enabled: boolean?) -> (),
	getNearestNeighborScaling: () -> boolean,
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
}

export type AssetsModule = {
	loadImage: (path: string) -> ImageHandle,
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
}

export type FsWalkEntry = {
	path: string,
	name: string,
	kind: "file" | "directory",
	is_file: boolean,
	is_dir: boolean,
}

export type FsModule = {
	readFile: (path: string) -> string,
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

export type HttpHeaders = { [string]: string }

export type HttpResponse = {
	ok: boolean,
	url: string,
	status: number?,
	body: string,
	error: string?,
	headers: HttpHeaders,
}

export type HttpModule = {
	request: (url: string, callback: (response: HttpResponse) -> ()) -> number,
	get: (url: string, callback: (response: HttpResponse) -> ()) -> number,
	_poll: () -> (),
}

export type CommandRunResult = {
	ok: boolean,
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
	addComponent: <T>(entity: Entity, component: T) -> T,
	removeComponent: (entity: Entity, target: number | ComponentInstance) -> boolean,
}

export type BaseDrawableComponent = ComponentInstance & {
	NEOLOVE_RENDERING: boolean,
	color: Color4Value,
	shader: ShaderHandle?,
	visible: boolean,
}

export type Rect2D = BaseDrawableComponent

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
	font: TextFont?,
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
}

export type TextLabel = TextBox
export type RudimentaryTextLabel = TextBox

export type Image2D = BaseDrawableComponent & {
	image: ImageHandle?,
}

export type TileTexture2D = BaseDrawableComponent & {
	image: ImageHandle?,
	tile_width: number,
	tile_height: number,
	offset_x: number,
	offset_y: number,
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

export type CoreModule = {
	Rect2D: Rect2D,
	Shape2D: Shape2D,
	TextBox: TextBox,
	TextLabel: TextLabel,
	RudimentaryTextLabel: RudimentaryTextLabel,
	Image2D: Image2D,
	TileTexture2D: TileTexture2D,
	Collider2D: Collider2D,
	Rigidbody2D: Rigidbody2D,
	Rope2D: Rope2D,
	String2D: Rope2D,
}

declare function Color4(r: number, g: number, b: number, a: number?): Color4Value
declare function die(): ()
declare function softrequire(modulePath: string, allowedModules: { [string]: any } | { string }?): any

declare app: AppModule
declare input: InputModule
declare userInput: InputModule
declare assets: AssetsModule
declare audio: AudioModule
declare fs: FsModule
declare http: HttpModule
declare commands: CommandsModule
declare command: CommandsModule
declare shaders: ShadersModule
declare ecs: EcsModule
declare transform: TransformModule
declare transforms: TransformModule
declare core: CoreModule

declare mouse: Vec2
declare window: Vec2

return nil
```

## 14. Notes and Current Limitations

From code-level behavior at time of generation:

- HTTP module supports `http://` and `https://` GET requests, but still exposes no custom methods/headers/body API.
- System lifecycle callbacks beyond `update` exist in typings but only `update` is invoked in runtime loop.
- `TextBox` defaults to lightweight label-style rendering, but entity bounds unlock wrapping, alignment, padding, and auto-fit text scaling.
- UI input handling is stateful but still simple: there is no global z-ordered capture/focus manager yet, so overlapping interactive controls can still compete if you stack them in the same screen space.
