# NeoLOVE Engine Documentation

NeoLOVE is a Rust game engine for Luau projects. A project is a directory with a `main.luau` file and, optionally, a `neolove.toml`, assets, components, modules, and generated web output. Runtime APIs are exposed as Luau globals such as `ecs`, `core`, `assets`, `input`, `audio`, `fs`, `servers`, `shaders`, and `tweening`.

The generated type surface is also available in `neolove_engine_api.d.luau`. New projects receive a copy from `src/project_template/neolove_engine_api.d.luau`.

## CLI

```bash
neolove new <project-name>
neolove run [project-dir]
neolove build [project-dir] [--webasm]
neolove setup-path
neolove --help
neolove --version
```

`run` and `build` require the target project to contain `main.luau`.

`neolove build --webasm` creates an HTML5 bundle in `dist/webasm/` and a zip at `dist/<project-name>-webasm.zip`. Serve web builds over `http://` or `https://`; browsers will not reliably load the bundle from `file://`.

Release builds are size-oriented by default:

```bash
cargo build --release
cargo build --release --features vulkan
```

The default desktop binary uses the software renderer and omits Vulkan to reduce executable size. Use `--features vulkan` for GPU acceleration and custom shader rendering. The default asset codecs are PNG images and WAV audio.

## Project Model

`main.luau` is loaded as the entry point. Relative asset, file, command, font, and shader paths are resolved inside the project root. Engine file APIs reject paths that escape the project root.

Common project layout:

```text
my-game/
  main.luau
  neolove.toml
  assets/
  components/
  shaders/
  neolove_engine_api.d.luau
```

## Runtime Order

Each frame:

1. Input state is refreshed.
2. Entity listeners are dispatched.
3. Non-rendering component `update(entity, component, dt)` callbacks run.
4. Physics and rope constraints are simulated.
5. Rendering component updates run in stable draw order.
6. Queued draw commands are rendered.
7. Internal async modules such as HTTP and servers are polled.

Rendering components have `NEOLOVE_RENDERING = true`. They still use an `update` callback, but the runtime delays those callbacks until the rendering pass so `z` order is stable.

## Global Helpers

### `Color4(r, g, b, a?)`

Creates a color table:

```luau
local white = Color4(255, 255, 255)
local translucent = Color4(255, 255, 255, 128)
```

Fields are `r`, `g`, `b`, and `a`. Values are clamped to `0..255`. Omitted alpha defaults to `255`.

### `die(reason?)`

Requests runtime exit. If `reason` is omitted, the engine records a default reason.

### `softrequire(modulePathOrSource, allowedModules?)`

Loads Luau source in a sandbox. `allowedModules` may be a table of global names or a map of explicit values. It is useful for plugins, user-authored scripts, or controlled module loading.

## App Settings

Global: `app`

Fields and functions:

- `app.bg`: clear color.
- `app.nearestNeighborScaling`: texture filtering default. `true` means nearest-neighbor.
- `app.setNearestNeighborScaling(enabled?)`
- `app.getNearestNeighborScaling()`
- `app.setMaxFps(fps?)`
- `app.getMaxFps()`
- `app.setShowFps(enabled?)`
- `app.getShowFps()`

Edges:

- `setMaxFps(nil)` clears the cap.
- Non-positive or non-finite FPS values are ignored.
- Replacing the global `app` table is supported; getters read the current table.

## Input

Globals: `input`, `userInput`

Keyboard:

```luau
input.isKeyDown("space")
input.isKeyPressed("a")
input.isKeyReleased("escape")
input.getLastKeyPressed()
input.getCharPressed()
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
- Mouse positions are exposed through global `mouse.x` and `mouse.y`.
- `window.x` and `window.y` contain the current logical window size.

## Assets

Global: `assets`

Images:

```luau
local image = assets.loadImage("assets/player.png")
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

- PNG is the default compiled image format for small release binaries.
- `getPixel` and `setPixel` use zero-based coordinates.
- Unloaded handles reject further reads, writes, uploads, and rendering.
- `save` and `export` paths stay inside the project root and receive `.png` or `.wav` extensions when omitted.
- `assets.gc()` drops unloaded cache entries whose handles are no longer referenced.

## Audio

Global: `audio`

```luau
audio.play(sound, true, 0.5)
audio.playOnce(sound)
audio.setVolume(sound, 0.25)
audio.stop(sound)
```

Edges:

- WAV is the default compiled audio format for small release binaries.
- Volume is clamped to `0..1`.
- Browser audio may not start until the user interacts with the page.
- `playOnce` is `play(sound, false, volume)`.

## File System

Global: `fs`

```luau
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

- All paths are project-root scoped.
- Directory creation creates parent directories as needed.
- `removeFile` returns `false` when the target is absent.

## Commands

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

## HTTP

Global: `http`

```luau
http.get("https://example.com", function(response)
    if response.ok then
        print(response.body)
    else
        print(response.error)
    end
end)
```

`http.request(url, callback)` and `http.get(url, callback)` return request ids. Responses include `ok`, `url`, `status`, `body`, `error`, and `headers`.

Edges:

- Requests are asynchronous.
- `_poll()` is internal and normally called by the engine.

## Servers

Global: `servers`

```luau
local hosted = servers.host("server.luau", 9000)
local client = servers.connect(hosted.url)
client:addCallback(function(payload)
    local message = servers.deserializeTable(payload)
end)
client:send(servers.serializeTable({ hello = "world" }))
```

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

Client handles expose `key`, `is_host`, `send(payload)`, `addCallback(callback)`, `disconnect()`, `isConnected()`, `getKey()`, `isHost()`, and `getKickReason()`.

Edges:

- Payloads are Luau `buffer` values.
- TLS host options support `certPath`/`keyPath` and `cert_path`/`key_path`.
- `_poll()` is internal and normally called by the engine.

## Shaders

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
- The current web runtime uses the software renderer and reports an error for custom shader draw commands.
- `shaders.DEFAULT_VERTEX_SHADER` contains the built-in vertex shader source.

## Tweening

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

## ECS

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
```

ECS functions:

- `ecs.addSystem(system)`
- `ecs.newEntity(name, parent?, x?, y?)`
- `ecs.deleteEntity(entity)`
- `ecs.duplicateEntity(entity, parent)`
- `ecs.findFirstChild(parent, name)`
- `ecs.addComponent(entity, component)`
- `ecs.removeComponent(entity, indexOrComponent)`
- `ecs.root`

Component shape:

```luau
local component = {
    awake = function(entity, component) end,
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
- Component `awake` runs before the component is pushed into `entity.components`.
- Component `destroy` runs when removed; `onDestroy` is used as a fallback.
- Component prototypes must be tables.
- Runtime errors in callbacks are reported with component context.

## Systems

Systems are tables passed to `ecs.addSystem`.

```luau
ecs.addSystem({
    awake = function(self) end,
    update = function(self, dt) end,
    lateUpdate = function(self, dt) end,
    fixedUpdate = function(self, dt) end,
})
```

Use systems for global simulation, managers, spawning, and logic that does not naturally belong to one entity.

## Transforms

Globals: `transform`, `transforms`

```luau
local x, y = transform.getWorldPosition(entity)
local r = transform.getWorldRotation(entity)
local hit = transform.raycast(0, 0, 1, 0, 500, { ignore = player })
local overlapping = transform.doTheyOverlap({ a, b, c })
```

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

## Entity Listeners

Supported events:

- `leftClick`
- `rightClick`
- `middleClick`
- `scrollUp`
- `scrollDown`

Example:

```luau
local connection = button:Listen("leftClick", function(entity, event)
    print(event.x, event.y)
end)
connection:Disconnect()
```

Event fields include `kind`, `type`, `button`, `x`, `y`, `mouseX`, `mouseY`, `wheelX`, `wheelY`, and `amount`.

Edges:

- Listener hit testing uses entity bounds.
- Listener connections support `Disconnect`, `disconnect`, `IsConnected`, and `isConnected`.
- Deleting entities disconnects their listeners.

## Prefabs

Globals: `prefabs`, `prefab`

```luau
local template = prefabs.capture(entity)
prefabs.register("enemy", template)
local enemy = prefabs.instantiate("enemy", ecs.root)
```

Functions:

- `prefabs.capture(entity)`
- `prefabs.component(source, overrides?)`
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

## Core Rendering Components

All drawable components share:

- `NEOLOVE_RENDERING = true`
- `color`
- `shader`
- `visible`

### `core.Rect2D`

Draws a rectangle using the entity transform and size.

```luau
local box = ecs.addComponent(entity, core.Rect2D)
box.color = Color4(255, 0, 0)
```

### `core.Shape2D`

Draws primitive shapes.

Fields:

- `shape`: `box`, `circle`, `triangle`, `right_triangle`, `righttriangle`, or `rightangledtriangle`
- `triangle_corner`
- `offset_x`, `offset_y`
- `size_x`, `size_y`

If `size_x` or `size_y` is `0`, the entity size is used.

### `core.TextBox`

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
- `font`
- `scale_x`, `scale_y`, `dx`, `dy`, `line_count`

Aliases: `core.TextLabel`, `core.RudimentaryTextLabel`.

Edges:

- `font` may be `"default"` or a project-root path.
- Text can auto-fit within entity bounds using `text_scale`.
- Content-sized text is not culled before layout.

### `core.Sprite2D` and `core.Image2D`

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

### `core.NineSliceSprite2D` and `core["9SliceSprite2D"]`

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

### `core.TileTexture2D`

Repeats an image over the entity bounds.

Fields:

- `image`
- `tile_width`, `tile_height`
- `offset_x`, `offset_y`

Edges:

- `tile_width` and `tile_height` default to the image size when `0`.
- Non-rotated tile layers are culled to the viewport before queueing tiles.
- Rotated tile layers preserve full-entity iteration for correctness.

## Spritebox2D

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

## Physics Components

### `core.Collider2D`

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

### `core.Rigidbody2D`

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

### `core.Rope2D` and `core.String2D`

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

## Rendering Details

Draw order is sorted by `z`, then entity id and component order for stable output. Equal `z` values preserve deterministic order.

Texture filtering comes from `app.nearestNeighborScaling`:

- `true`: nearest neighbor.
- `false`: linear filtering.

Rendering is skipped for components with `visible = false`, nil images, unloaded images, zero or negative sizes, or fully transparent colors.

## WebAssembly

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
- Web shader support is limited by the current software renderer.

## Performance Guidance

- Prefer `Spritebox2D:ComputeSpritebox()` once after image setup instead of every frame.
- Reuse loaded image and sound handles.
- Use `TileTexture2D` for repeated backgrounds instead of manually creating many sprite entities.
- Keep `z` changes intentional; rendering order work is stable but still per-frame.
- For collision-heavy gameplay, use Collider2D/Rigidbody2D for broad physical simulation and Spritebox2D for precise final checks.
- Use `assets.gc()` after unloading large batches of assets.
- Keep web bundles lean by removing unused assets before building.

## Troubleshooting

- `component prototype is nil`: the component table does not exist. Check spelling, especially `core["9SliceSprite2D"]` bracket access.
- `component has no awake function`: custom components need at least `awake`.
- `image is unloaded`: a handle was unloaded and then reused.
- Sprite clicks hit transparent padding: use `core.Spritebox2D` and call `ComputeSpritebox`.
- Spritebox always returns false: verify the sprite component is attached before the Spritebox computes, the image has alpha greater than `alpha_threshold`, and world coordinates are being passed to `IsInside`.
- Web build is blank from `file://`: serve through HTTP.
- Custom shader errors in web builds: the current web renderer does not support custom shader draw commands.
