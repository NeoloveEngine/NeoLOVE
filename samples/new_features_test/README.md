# NeoLOVE New Features Test

This sample tests the newly added features:

- Window title config from `neolove.toml` (`[window].title`)
- Window icon config from `neolove.toml` (`[window].icon`)
- `TextBox` auto-fit scaling and bounds-based alignment
- Custom text fonts (`TextBox.font`)
- Wrapped text inside entity bounds (`TextBox.wrap` / `TextBox.size_mode`)
- FPS counter toggle (`app.setShowFps` / `app.getShowFps`)
- Build pipeline (`neolove build`) with bytecode + embedding

## Run

```bash
cd samples/new_features_test
neolove run
```

## Build + Run Built Executable

```bash
cd samples/new_features_test
neolove build
./dist/new_features_test
```

(Use `dist\\new_features_test.exe` on Windows.)

## Runtime Controls

- `F`: toggle FPS counter visibility
- `R`: toggle max FPS cap (`60` / uncapped)
