<h1 align="center">Neolove</h1>
a game engine written in Rust that allows
you to easily make performant games in the language luau

<sub>[you can find docs here!](https://github.com/NeoloveEngine/NeoLOVE/wiki)</sub>

### CLI

```bash
neolove new <project-name>
neolove run [project-dir]
neolove build [project-dir]
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

- `cargo build --target wasm32-unknown-unknown` is now supported as a bootstrap build target.
- The current wasm target is build-only; the desktop runtime still requires native windowing/Vulkan and is not yet runnable in the browser.

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
