# vello

GPU rendering. `vello` 0.8, on `wgpu`.

New to Rust syntax? [RUST.md](../RUST.md) first.

---

## The idea

Most 2D renderers rasterise on the CPU. vello does it in **compute shaders**:
you describe shapes, it uploads them, and the GPU works out every pixel in
parallel. Anti-aliasing, curves and clipping all happen there.

That is why there is no software fallback — the algorithm *is* the GPU. A
machine without Vulkan, Metal or DX12 cannot run this app.

`wgpu` underneath is the portability layer: one API over Vulkan, Metal, DX12 and
WebGPU. It is where the long first compile comes from — wgpu and its shader
compiler `naga` are most of the dependency tree.

## Scene: a display list, rebuilt every frame

```rust
// crates/koru-editor/src/draw.rs
scene.reset();
scene.fill(Fill::NonZero, root, colour, None, &rect);
scene.stroke(&Stroke::new(1.0), root, colour, None, &shape);
```

A `Scene` is a list of drawing commands, not a retained tree of objects. Koru
clears and refills it each frame. That sounds wasteful and is not: the work is
appending to a buffer, and the GPU does the rest.

`Fill::NonZero` is the winding rule — how to decide "inside" for a
self-intersecting path. `NonZero` is the usual answer.

Shapes come from `kurbo`, the geometry crate: `Rect`, `RoundedRect`, `Circle`,
`Line`, `Affine`.

## `Affine`: DPI applied exactly once

Every draw call takes a transform. Koru uses that for the scale factor and
nothing else:

```rust
let root = Affine::scale(dpi);
```

Everything downstream is in logical pixels, and no constant in `theme.rs` knows
what DPI the screen is. The same mechanism does the italic skew for fonts
without a real oblique ([PARLEY.md](PARLEY.md#synthesis-faux-bold-and-faux-italic)).

## Glyphs

parley produces positioned glyphs; vello draws them:

```rust
// crates/koru-editor/src/draw.rs
scene.draw_glyphs(font)
     .brush(colour)
     .transform(base)
     .draw(style, glyphs);
```

No conversion between the two: `parley::FontData` *is* `peniko::FontData`, since
both re-export `linebender_resource_handle`. `peniko` is the shared vocabulary —
`Color`, `Brush`, `Blob` — rather than a third rendering library.

## The one API surprise

**vello 0.8 has no `render_to_surface`.** You render into an intermediate
texture, and getting that onto the screen is your job:

```rust
// apps/koru/src/main.rs
renderer.render_to_texture(device, queue, &scene, &surface.target_view, &params)?;

let frame = surface.surface.get_current_texture()?;
surface.blitter.copy(device, &mut encoder, &surface.target_view, &frame_view);
queue.submit([encoder.finish()]);
frame.present();
```

`RenderSurface` carries a blitter for exactly this. Reasonable once you know;
baffling for an afternoon if you do not.

## Surfaces and devices

`RenderContext` owns the GPU devices and hands out surfaces:

```rust
let surface = pollster::block_on(context.create_surface(
    window.clone(), width, height, wgpu::PresentMode::AutoVsync,
))?;
```

`create_surface` is the **only** `async` call in the app. Rather than pull an
async runtime through the whole binary for one `await` at startup,
`pollster::block_on` runs it to completion on the spot.

`PresentMode::AutoVsync` waits for the display refresh — no tearing, no frames
rendered that nobody sees.

Each window gets its own `Renderer` here. Two surfaces usually resolve to the
same device and could share one, but a `Renderer` is cheap next to a window and
tying it to the surface removes the question.

## Resizing

A surface has a fixed size and must be told when the window changes:

```rust
context.resize_surface(&mut pane.surface, size.width, size.height);
```

In physical pixels, straight from winit — this is one of the few places that is
not logical.

## Colour

`peniko::Color`, constructed from 8-bit components:

```rust
Color::from_rgba8(0xFF, 0xE0, 0x66, 0x8C)      // the yellow highlight
```

All of it lives in `crates/koru-draw/src/theme.rs`; nothing else in the app constructs
a color.

Worth knowing: **brushes are baked into parley layouts**, so a theme change is
not a redraw — it invalidates every layout in every window.
`App::invalidate_everywhere` exists for that.

## The debug build trap

```toml
[profile.dev]
opt-level = 1
[profile.dev.package."*"]
opt-level = 3
```

An unoptimised vello is unusably slow at runtime. Dependencies are built at full
optimisation even in debug; they rarely recompile, so it costs almost nothing in
iteration time and makes `./koru.sh run` actually usable.

## Where to look

- `crates/koru-editor/src/draw.rs` — every scene command in the project
- `apps/koru/src/main.rs` — surface creation, the render-and-blit sequence
- [vello](https://github.com/linebender/vello) · [wgpu](https://wgpu.rs) — upstream
