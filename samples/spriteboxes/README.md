# Spriteboxes Demo

Run from the repository root:

```bash
cargo run -- run samples/spriteboxes
```

This demo generates all images at runtime. It shows:

- `core.Sprite2D` rendering with transparent padding.
- `core.Spritebox2D:ComputeSpritebox()` creating pixel-shaped hit regions.
- `spritebox:IsInside(mouse.x, mouse.y)` for hover and click checks.
- `spritebox:IsIntersecting(other)` with a mouse-following probe.
- `core.NineSliceSprite2D` and `core["9SliceSprite2D"]` panels.

Transparent sprite padding is intentionally visible in the layout. Hover and click only trigger on opaque pixels.
