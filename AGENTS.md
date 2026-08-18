# valo — how this project works

valo is a 2D render engine in Rust on wgpu. It records drawing commands into a display list, plans them into GPU passes, and draws them alongside a Skia-shaped text stack. The architecture follows Flutter's Impeller; the text stack follows Skia. The host application owns all IO and presentation — windows, files, the network — and the renderer holds no application content of its own, only caches.

## The frame

```
record    DisplayListBuilder → Arc<DisplayList>      CPU only, no GPU device, any thread
plan      cull → depth assign → reorder → segment    pure CPU pass over the recorded ops
encode    render passes, breaking only where recorded (backdrop filters, advanced blends)
submit    stats: cpu/plan/encode ms, draws, culled, passes, atlas churn, GPU timestamps
```

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
| `valo-web` | wasm-bindgen bindings: the raw API, canvas attach, image upload. Ships to npm as `valo-web`; the `webgl` feature builds the WebGL2-fallback compat artifact |
| `valo-web-demo` | dev only: the browser playground chapters (`npm run dev:web`) |
| `valo-harness` | dev only: headless GPU, golden compare, example runner. Never a dependency of a shipping crate |

## Rules

Each rule exists for a reason; the reason is stated so you can tell when a rule genuinely does not apply, rather than following it blindly.

1. **Use wgpu directly — never wrap it in another GPU abstraction.** WebGPU is already a portable command encoder; a second portability layer would cost performance and clarity while buying nothing.

2. **Let specialist libraries do specialist jobs; valo owns the coordination between them.** Text shaping is harfrust, font parsing skrifa, glyph rasterising swash, line breaks and bidi the unicode crates, atlas packing etagere. valo owns recording, render planning, clipping, blur, blending, paragraph layout, and every cache. Why: those libraries each encode years of edge cases we should not re-learn — and the coordination between them is exactly the part nobody else provides.

3. **Compute drawing facts at record time, not at replay time.** Bounds, clip lifetimes, depth slots and layer bounds are stored on each recorded operation; replay just reads them. Why: recording sees the whole command stream with its clip stack live, so it can know these things cheaply — replay would have to re-derive them per frame, every frame, from less context. If replay is calculating something the recorder could have known, move the calculation.

4. **`cargo test` must stay headless — no browser, no display server.** Browser suites exist (Canvas2D conformance, site smoke, WebGL-fallback smoke) but are separate npm suites. Why: the Rust suite is the inner loop for engine work; if it ever needs a browser or a window, iteration speed and CI portability both die.

5. **The host supplies every external resource and render target.** Fonts arrive as registered bytes, images arrive uploaded, the frame target arrives from the host; valo never opens font files, touches the network, or owns a window. Why: every embedder — a game, an editor, a browser page, a C host — already has its own IO and window stack, and an engine that reaches around it cannot be embedded cleanly.

## Design facts

Not rules — facts about how the engine works that are cheap to know and expensive to rediscover. The detailed mechanics live as comments beside the code they describe.

- Coordinates start at the top-left and y grows downward, in logical pixels until a transform says otherwise (the Canvas2D/Skia convention).
- The public transform is a full 4×4, but matrix z never controls draw order. Depth is internal: the renderer assigns z to make clips depth-tested, and callers never see it. (The formula lives beside `slot_z` in `planner/replay.rs`.)
- Draws render into 4-sample scratch textures that hardware-resolve into single-sample persistent targets. The scratch is kept only when a later render section resumes the same target; the final section discards it, which lets tiled GPUs (phones, Apple Silicon) skip writing it to memory.
- Recording may hold existing GPU texture handles, but never needs a GPU device — display lists and text stay recordable from any thread. Hosts create and upload GPU resources before recording refers to them.
- When one paint carries both a colour filter and a blur, the order applied depends on what the filter is attached to — the reasoning lives beside `LayerEffects` in `planner/filters.rs`.

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

### Comments

- Comments must carry information the code does not: contracts, constraints, rationale, surprising behavior, or external design references.
- Public rustdoc starts with the exact identifier and a complete sentence: `Context is ...`, `` `render` draws ... ``. Go style: the first line alone states WHAT it is; a blank line, then one concise paragraph for why it exists and what else is worth knowing. Describe correct use, ownership, side effects, defaults, edge cases, and when an option should be changed. Per-field facts go on the fields, not in the type's paragraph.
- Introduce public concepts in ordinary language before specialized terms: say what the API represents, why a caller uses it, and define terms such as shaping or caret affinity. Add a short example when prose alone would still leave correct use unclear.
- Link public documentation through facade types such as `crate::TextTiers`, not internal implementation crates.
- Never cite an internal document, plan number, or chapter that a reader cannot follow. Delete stale or misplaced comments instead of adapting them to the wrong item.

## Pitfalls that cost time

Issue records, not rules — each of these produced wrong pixels or lost hours once already.

- **Images rendering solid black:** every image-drawing step must tint with `alpha_tint(...)`, never the raw paint colour. The image shader multiplies samples by its tint and the default paint is black. Shipped twice; the `m3_images` golden now catches it.
- **Gradient gaps rendering solid black:** never treat a paint as fully covering unless it is guaranteed to touch every pixel in its bounds. Some gradients have opaque stops but paint nothing outside their valid region; promoting them to the no-blend pipeline replaces those untouched pixels with black. `shader_opaque` in `planner/emit.rs` is deliberately conservative about this.
- **GPU resources not being freed:** one non-blocking `poll` per paced frame is enough for the driver to reclaim finished resources. Blocking waits belong only on pixel-readback paths; an unthrottled submit loop is the one case needing an explicit wait, and hosts do not run that way.
