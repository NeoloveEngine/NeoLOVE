# 3D shaders and antialiasing validation

Date: 2026-08-03 (Australia/Sydney)

## Automated checks

| Check | Result |
| --- | --- |
| `cargo test --all-targets` | PASS — 419 passed, 5 intentionally ignored, 0 failed. |
| `cargo test --all-targets --all-features` | PASS — 421 passed, 5 intentionally ignored, 0 failed. Includes Vulkan shader parsing and 1×/2×/4× MSAA selection. |
| `cargo check --target wasm32-unknown-emscripten` | PASS. Validates the Rust/C bridge signature and Emscripten build. |
| Extracted WebGL shader bridge parsed with Node `new Function` | PASS. |
| Root/template Luau definitions compared with `cmp` | PASS — byte-identical. |
| `git diff --check` | PASS. |

Focused tests cover portable WebGL source normalization, explicit 3D shader
constructors/capability aliases, normalized color uniforms, portable uniform
limits, software 3D edge coverage, the 3D-before-2D overlay boundary, 2D-only
scratch allocation behavior, and Vulkan sample-count fallback.

## Runtime sample

Project: `/home/khakixd/Documents/apps/samples/3d-shaders-aa`

- Software-only run: PASS; capability query leaves the sample unshaded rather
  than submitting unsupported GLSL.
- Vulkan-feature run: PASS; the portable fragment compiled lazily and rendered
  on both a rotating sphere and cube with high MSAA selected.
- Live uniform updates and the `off` / `standard` / `high` input controls ran
  without a runtime or renderer diagnostic.

## Visual artifact

`test-artifacts/screenshots/3d-shaders-aa-high.png`

The 1330×792 active-window capture was produced with Spectacle from the live
Vulkan sample. Visual inspection confirms the procedural cyan grid/rim fragment
material on both meshes, depth-correct overlap with the floor, readable 2D
overlay text, and smooth high-quality silhouettes. The screenshot is evidence
for this focused milestone, not yet a tolerance-based visual-regression suite.

## Existing 3D preparation benchmark

The release-only shared-index diagnostic remained available and previously
reported 48 entities / 221,184 triangles in 10.289 ms versus 42.566 ms for the
expanded reference path (4.14× preparation speedup). This is a focused local
diagnostic, not a universal engine comparison.
