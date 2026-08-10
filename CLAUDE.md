# valo — design principles

valo is a 2D render engine in Rust on wgpu. It takes Impeller's architecture (display list → record-time oracle → planned GPU passes) and Skia's text architecture (skparagraph's tiers over a shaping/raster/atlas stack), and owns the orchestration between them.

## The frame

```
record    DisplayListBuilder → Arc<DisplayList>      CPU only, no GPU device, any thread
plan      cull → depth assign → reorder → segment    pure CPU pass over the recorded ops
encode    render passes, breaking only where recorded (backdrop filters, advanced blends)
submit    stats: cpu/plan/encode ms, draws, culled, passes, atlas churn, GPU timestamps
```

Present and read-back belong to the host. valo never owns a window, a surface loop, or a font source.

## Crates

| crate | role |
|---|---|
| `valo-geometry` | pure math: points, rects, paths, 4×4 matrices, strokes, color. No GPU, no unicode, no deps beyond glam |
| `valo-dl` | recording: `DisplayListBuilder`, ops, paints, shaders. GPU-free and `Send + Sync` |
| `valo-text` | typographer: shaping, bidi, wrapping, glyph raster, SDF, COLR. GPU-free |
| `valo-renderer` | the wgpu core: planner, encoder, pipelines, atlases, pools, caches |
| `valo` | the facade hosts use, plus `Hud` |
| `valo-svg` | SVG → display list translation |
| `valo-system-fonts` | native OS font discovery behind `FontSource`. Never a wasm dependency |
| `valo-capi` | C ABI for non-Rust embedders; the committed header is `crates/valo-capi/include/valo.h` |
| `valo-harness` | dev only: headless GPU, golden compare, example runner. Never a dependency of a shipping crate |

## Principles

**wgpu is the HAL.** No abstraction layer over it. WebGPU is already a portable encoder; a second one earns nothing.

**Delegate at the real engines' seams, own the architecture between them.** Shaping is harfrust, font parsing skrifa, glyph raster swash, line breaks and bidi the unicode crates, atlas packing etagere. valo owns the display list, the record-time oracle, pass orchestration, stencil-then-cover, the blur and blend machinery, the paragraph pipeline, and every cache.

**The recorder knows everything, so it says so.** Device bounds intersected with the live clip stack, depth slots, clip expiry, and save-layer scope bounds are all computed at record time and carried inline on the ops. Replay reads them and never counts, never looks ahead, never patches at restore. If you find yourself deriving something during replay that the builder could have known, that is a bug in the oracle.

**The renderer is stateless with respect to content.** Everything it holds is a content-keyed or frame-scoped cache: pipelines, glyph atlas pages, flattened contours, pooled targets, the host buffer ring. Retained-mode performance comes from those caches, not from a scene graph. There is no scene graph.

**No browser needed.** Golden pixel tests run on a headless native device and examples render straight to PNG or into a winit window. Nothing in the test suite requires a browser or a display server.

**Host-agnostic.** valo takes bytes, handles, and display lists. It never reaches for a font file, a network, or an application model. Fonts arrive registered; images arrive uploaded; the frame target arrives from the host.

**No layer that doesn't pay rent.** Every abstraction should be traceable to a cost it removes.

## Invariants

- y-down, origin top-left, logical pixels until a transform says otherwise.
- `Color` is straight (unpremultiplied) sRGB; premultiplication happens at the GPU boundary and blending is in sRGB space (the CSS/Skia look). Linear-light and wide-gamut blending are deliberately deferred — the type survives that change.
- The public transform is a full 4×4. Depth is a renderer concern: clips consume z, the caller never sees it. Matrix z is ignored for ordering, as in Impeller.
- Depth clips: z = slot/(slots+1), depth clears to 0, draws test `GreaterEqual`, and ceilings are written at the expiry slot. The restore that closes a clip scope owns one slot, and that slot is every inner clip's expiry.
- MSAA is ×4 everywhere. msaa+depth are stored only for segments a later segment resumes; the final segment of every target discards, which skips the tile flush on tiled GPUs.
- Recording is GPU-free. If `valo-dl` or `valo-text` ever needs a device, the layering broke.
- A layer's post-effects run in Impeller's order (`Paint::WithFilters`): the colour filter recolours the layer's own pixels, then the blur spreads the filtered result. Colour matrices are defined on straight colour, so the filter pass unpremultiplies, applies, clamps, and premultiplies again.

## Working here

```sh
cargo test                      # unit + golden pixel tests (headless GPU)
VALO_BLESS=1 cargo test         # accept new goldens after an intended visual change
cargo run -p valo --example rects   # one example per feature; renders to target/examples/
cargo bench -p valo             # criterion: record, frame, text, geometry
cargo check --target wasm32-unknown-unknown -p valo -p valo-svg
```

Goldens compare within ±3 per channel. A golden that changes because you changed what the scene draws is fine — re-bless it. A golden that changes because you changed how it draws needs an explanation before it gets blessed.

## Style

Single Level of Abstraction per function: a function either orchestrates or does detail work, never both. One purpose per function, one responsibility per type.

No abbreviations in public names or fields. Full words — `composition`, not `comp`; `effect`, not `fx`.

Comments explain **why**, never what the code already says. A comment that names an external design (Impeller's `DrawOrderResolver`, Skia's `SubRunControl`, mapbox's TinySDF) is carrying real information — keep those. Never cite an internal document, plan number, or chapter: this repo has no such documents, and a reader cannot follow the pointer.

## Traps that have already cost time

**Never use encoder-level `write_timestamp` on Apple/Metal.** It hangs command buffers — the GPU never signals completion and every later `poll(wait)` wedges. Apple GPUs sample timestamps only at render-pass stage boundaries, so always use `RenderPassDescriptor::timestamp_writes`, even when the adapter advertises `TIMESTAMP_QUERY_INSIDE_ENCODERS`.

**Image-ish steps must tint with `alpha_tint(...)`, never the raw paint color.** `fs_image` multiplies by the paint color and `Paint::default()` is black, so a new step that forwards the paint straight through renders solid black. This bug shipped twice. The `m3_images` golden is the tripwire.

**A paint that can leave pixels uncovered must never be judged opaque.** The
opaque-promotion pass gives a draw the depth-writing pipeline with blending
off, which REPLACES the destination — so any gradient that paints nothing
somewhere (a two-point conical outside its cone, most visibly the strip case)
renders those pixels as solid black instead of leaving the background. Opaque
stops are not enough on their own; the shape of the coverage matters too.

**Winding flips in NDC** because the ortho matrix has a negative y-scale. Irrelevant to `!= 0` stencil tests, which are sign-agnostic, but check it before adding sign-sensitive stencil logic.

**A non-blocking `poll` per frame is enough to reclaim GPU resources** at paced frame rates. Blocking waits belong only on CPU read-back paths; an unthrottled submit loop is the one case that needs an explicit wait, and it is not how hosts run.
