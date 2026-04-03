<h1 align="center">Neolove</h1>
a game engine written in Rust that allows
you to easily make performant games in the language luau

<sub>[you can find docs here!](https://github.com/NeoloveEngine/NeoLOVE/wiki)</sub>

### CLI

```bash
neolove new <project-name>
neolove run [project-dir]
neolove build [project-dir] [--webasm]
neolove setup-path
neolove --help
neolove --version
```

`run` and `build` now validate that the target project has a `main.luau` entry file before starting.

### Production Defaults

- `cargo build --release` now uses thin LTO, single codegen unit, stripped binaries, and `panic = "abort"`.
- CI is configured in `.github/workflows/ci.yml` to run `fmt`, `clippy`, and `test` on push/PR.
- Lua-exposed `fs` and command `cwd` paths are restricted to the project root.

### WebAssembly

- `neolove build --webasm` now builds an itch.io-ready HTML5 bundle into `dist/webasm/` and also creates a single upload zip in `dist/<project-name>-webasm.zip`.
- The web bundle includes `index.html`, `neolove.js`, `neolove.wasm`, and `neolove.data` when the project payload is preloaded into the browser filesystem.
- Web builds now support the existing `audio.play`, `audio.playOnce`, `audio.stop`, and `audio.setVolume` API through the browser Web Audio backend.
- `cargo build --target wasm32-unknown-unknown` is still supported directly as the lower-level bootstrap build target.
- The first `--webasm` build may install `wasm32-unknown-emscripten` and bootstrap a local Emscripten toolchain under `~/.neolove/toolchains/emsdk`.

### Roadmap

<!-- plans for a PM were removed because pesde would be sufficient -->

- [X] luau processing
- [X] hierachy
- [X] entities
- [X] components
- [X] systems
- [X] rendering
- [X] drawable components
- [X] texture loading & rendering
- [X] reading/writing files
- [X] audio manager
- [ ] commands
- [X] network requests
- [ ] physics?
- [ ] gui
- [X] documentation
- [ ] compiling
- [ ] release

<sub>anything ending in `?` means that it may not be implemented</sub>
