# NeoLOVE Complete Feature Lab

An interactive smoke test for every currently exposed NeoLOVE runtime area:

- Application settings, keyboard, mouse, wheel, text input, and mouse locking
- ECS systems, entities, hierarchy, instance methods, component lifecycle, and events
- Rectangles, shapes, text, sprites, source rectangles, nine-slice, tiles, and shaders
- Generated and reloaded images and sounds, audio playback, unload, and asset GC
- Rigidbody, collider callbacks, triggers, ropes, overlap checks, and raycasts
- Prefab capture/register/get/remove/instantiate/duplicate and built-in UI templates
- Tween creation, cancellation, easing, aliases, and completion callbacks
- Cooperative async task queuing, yielding, resuming, status, and results
- Filesystem operations, serialization, UUIDs, hashing, HTTP, commands, and servers
- `require`, `softrequire`, API aliases, window/mouse state, and project configuration

The lab deliberately does not auto-run external side effects. HTTP, process execution,
local server hosting, detached commands, mouse locking, and `die()` require a key press.

## Run

```bash
cargo run -- run samples/feature_lab
```

## Controls

| Key | Action |
| --- | --- |
| `1` | Play generated sound once |
| `2` | Start looped audio |
| `3` | Stop audio |
| `4` | Change loop volume |
| `T` | Launch three tweens |
| `G` | Export/reload/unload generated assets and run asset GC |
| `F` | Run the filesystem probe again |
| `H` | Make an HTTPS GET request to `https://example.com/` |
| `J` | Make an HTTPS request using the options-table overload |
| `C` | Run a foreground `printf` command (native only) |
| `D` | Run a detached `true` command (native only) |
| `N` | Host a local echo server, connect, serialize, send, and receive |
| `M` | Toggle mouse locking |
| `P` | Toggle FPS display |
| `R` | Reset the physics body |
| `K` | Disconnect/reconnect one entity event listener |
| `Delete` | Call `die()` and intentionally terminate the sample |

Move and click the mouse over the event pad. Use the arrow keys to apply physics
forces and impulses. The dashboard shows live input, physics, rendering, and probe
state. Shaders are most visible in Vulkan builds; software rendering still exercises
the shader API without applying GPU effects.

## Coverage Notes

Aliases such as `userInput`, `command`, `prefab`, `tween`, `transforms`,
`TextLabel`, `RudimentaryTextLabel`, `String2D`, and `core["9SliceSprite2D"]`
are checked as aliases rather than rendered twice. `Image2D` and `Sprite2D`
share rendering behavior but are distinct component prototypes, so both are
instantiated.

`die()` is only tested by the opt-in `Delete` key because success terminates the
process. Remote server transport is exercised against the locally hosted endpoint,
so no third-party server is required.
