# Valo

Valo is a WebGPU-native 2D rendering engine built with Rust and [wgpu](https://wgpu.rs/). It follows the architecture of Flutter's [Impeller](https://github.com/flutter/flutter/tree/master/engine/src/flutter/impeller) with no shader compilation at draw time — behind a user-facing API in the shape of Canvas2D and Skia.

**Web playground** — `npm install && npm run dev:web`

Valo runs in the browser on WebGPU and natively on desktop through wgpu, from the same code, with performance comparable to Skia and Impeller.

The browser package exposes both the retained engine directly and a typed,
Canvas-shaped adapter. The adapter keeps familiar application code while adding
Valo's layers, color matrices, backdrop blur, and explicit frame control.

## What it looks like

```rust
use std::sync::Arc;
use valo::{
    Color, DisplayListBuilder, DrawParagraphExt, GradientStop, Paint, ParagraphBuilder,
    Point, Rect, Shader, SpreadMode, TextStyle,
};

let mut builder = DisplayListBuilder::new();

// A card: rounded rect, per-corner radii, soft shadow underneath.
builder.draw_rrect_radii(
    Rect::new(40.0, 40.0, 400.0, 240.0),
    [24.0, 24.0, 24.0, 24.0],
    &Paint {
        color: Color::rgba(0.0, 0.0, 0.0, 0.35),
        mask_blur: Some(valo::MaskBlur::new(18.0)),
        ..Default::default()
    },
);

// Gradients are a paint, so they fill rects, paths, and text alike.
builder.draw_rrect_radii(
    Rect::new(40.0, 40.0, 400.0, 240.0),
    [24.0, 24.0, 24.0, 24.0],
    &Paint::from_shader(Shader::Linear {
        start: Point::new(40.0, 40.0),
        end: Point::new(440.0, 280.0),
        stops: vec![
            GradientStop { offset: 0.0, color: Color::rgb(0.16, 0.18, 0.28) },
            GradientStop { offset: 1.0, color: Color::rgb(0.35, 0.16, 0.40) },
        ],
        spread: SpreadMode::Pad,
        local: valo::Matrix::IDENTITY,
    }),
);

// Frosted glass over whatever is already there.
builder.backdrop_blur(Rect::new(64.0, 200.0, 352.0, 56.0), 12.0);

// Text is laid out once and re-usable; wrapping, bidi and fallback included.
let mut paragraph = ParagraphBuilder::new(&mut fonts)
    .add_text("Valo renders this", &TextStyle::new("Fira Sans", 28.0, Color::WHITE))
    .build();
paragraph.layout(360.0);
builder.draw_paragraph(&paragraph, (64.0, 96.0));

let display_list = Arc::new(builder.build());
```

Recording touches no GPU and needs no `Context` — it is pure CPU work you can do on any thread, keep, nest inside other lists, and replay every frame.

## From TypeScript

```sh
npm install @valo/web
```

Use the Canvas-shaped API for existing drawing code:

```ts
import { createValoCanvas } from "@valo/web";

const canvas = document.querySelector("canvas")!;
const context = await createValoCanvas(canvas);

context.fillStyle = "#c8ff3d";
context.roundRect(24, 24, 240, 140, [28, 8, 28, 8]);
context.fill();
context.backdropBlur(48, 72, 192, 56, 12); // Valo extension
```

Or import `@valo/web/raw` for retained `DisplayListBuilder`, `Path`, `Paint`,
`Shader`, `Paragraph`, image upload, render statistics, and explicit resource
lifetimes. Both layers use the same WebAssembly renderer.

The adapter covers common Canvas2D state, paths (including `arc`, `arcTo`, and
`ellipse`), transforms, clips, gradients, images, shadows, blends, and filled or
stroked fixed-font text. Deliberate gaps currently throw `NotSupportedError`:
synchronous `getImageData`/`putImageData`, `isPointInStroke`, and non-repeating
pattern modes. Canvas `filter` supports ordered `blur`, `brightness`,
`contrast`, `grayscale`, `hue-rotate`, `invert`, `opacity`, `saturate`, and
`sepia` chains; `drop-shadow()` and `url()` are not yet supported. Valo's typed
`setColorMatrix` extension accepts a direct 4×5 matrix. Web fonts are explicitly registered from bytes;
Valo does not silently depend on browser font fallback. For animation, call
`beginFrame()` and `present()` to bound retained history.

## From C

Valo also ships a C API, so it embeds into anything that speaks C. The header is [`crates/valo-capi/include/valo.h`](crates/valo-capi/include/valo.h), every function is null-safe, and every object is an opaque handle with a matching `dispose`.

```c
#include "valo.h"

ValoContext *context = valo_context_new();
ValoDisplayListBuilder *builder = valo_builder_new();

ValoPaint paint = {0};
paint.color = (ValoColor){0.96f, 0.35f, 0.25f, 1.0f};
paint.blend_mode = 3; /* srcOver */

ValoCornerRadii radii = {24, 24, 24, 24, 24, 24, 24, 24};
valo_builder_draw_rounded_rect(builder, (ValoRect){40, 40, 400, 240}, radii, paint);

ValoDisplayList *list = valo_builder_build(builder);

uint8_t *pixels = malloc(480 * 320 * 4);
valo_context_render_to_pixels(context, list, (ValoColor){1, 1, 1, 1}, 480, 320, pixels);

valo_display_list_dispose(list);
valo_context_dispose(context);
```

The complete version — build, run, write an image — is [`examples/c/hello.c`](examples/c/hello.c):

```sh
./examples/c/run.sh
```

## Quick start

```toml
[dependencies]
valo = { git = "https://github.com/tyxu/valo" }
wgpu = "29"
```

Valo draws onto a device you own, so the host keeps control of the window, the swapchain, and presentation:

```rust
use valo::{Color, Context, DisplayListBuilder, Offscreen, Paint, Rect};

// Any wgpu device works — a surface you configured, or a headless one.
let mut context = Context::new(device.clone(), queue);
let target = Offscreen::new(&device, [480, 320]);

let mut builder = DisplayListBuilder::new();
builder.draw_rect(
    Rect::new(40.0, 40.0, 160.0, 90.0),
    &Paint::from_color(Color::rgb(0.96, 0.35, 0.25)),
);
let display_list = builder.build();

let stats = context.render(&display_list, &target.target(Some(Color::WHITE)));
println!("{} draws, {} culled, {:.2}ms", stats.draws, stats.culled, stats.cpu_ms);
```

Run the same thing end to end, straight to a PNG:

```sh
cargo run -p valo --example hello       # → target/examples/hello.png
```

## Testing Canvas compatibility

The private `@valo/conformance` package replays the same scenes into browser
Canvas2D and Valo in Chromium. Curated cases protect named behavior; constrained,
seeded fuzzing explores command combinations; the benchmark reports JavaScript
recording and submission cost without hiding a GPU wait in the result.

```sh
npm run build:web
npm run test:conformance
npm run fuzz:canvas
npm run benchmark:canvas
```

Failed comparisons write the Canvas2D image, Valo image, visual diff, and exact
scene to `packages/valo-conformance/artifacts/`.

## API Catalog

### Recording

| Canvas state | Transforms | Clips | Draws |
|---|---|---|---|
| `DisplayListBuilder::new` | `translate` | `clip_rect` | `draw_rect` |
| `save` | `scale` | `clip_rrect` | `draw_rrect` |
| `restore` | `rotate` | `clip_rrect_radii` | `draw_rrect_radii` |
| `save_layer` | `concat` | `clip_rrect_radii_elliptical` | `draw_rrect_radii_elliptical` |
| `save_layer_mask` | | `clip_path` | `draw_circle` |
| `build` | | | `draw_path` |
| `draw_display_list` | | | `draw_image` |
| `draw_display_list_cached` | | | `draw_image_rect` |
| | | | `draw_paragraph` |
| | | | `draw_paragraph_with` |
| | | | `backdrop_blur` |
| | | | `backdrop_blur_shared` |

### Paint and geometry

| Paint | Shaders | Paths | Types |
|---|---|---|---|
| `Paint::from_color` | `Shader::Linear` | `PathBuilder::new` | `Color` |
| `Paint::from_shader` | `Shader::Radial` | `move_to` | `Rect` |
| `PaintStyle::Fill` | `Shader::Sweep` | `line_to` | `Point` |
| `PaintStyle::Stroke` | `GradientStop` | `quad_to` | `Size` |
| `Stroke` · `Cap` · `Join` | `SpreadMode` | `cubic_to` | `Matrix` |
| `Dash` | `TileMode` | `close` | `FillRule` |
| `MaskBlur::new` | `Filter` | `rect` · `circle` | `ClipOp` |
| `MaskBlur::solid` | `MaskKind` | `rrect` · `rrect_radii` | `BlendMode` |
| `MaskBlur::inner` · `outer` | `Shader::Image` | `rrect_radii_elliptical` | `constrain_radii` |
| | `FocalCircle` | | |
| `ColorFilter::Matrix` | | `arc` · `ellipse` | `constrain_radii_elliptical` |
| `ColorFilter::Blend` | | `arc_to` | |
| `ImageFilter::blur` | | | |
| `ImageFilter::color` | | | |
| `ImageFilter::compose` | | | |
| `BlurStyle` | | `contains` | |
| | | `build` | |

### Text

| Fonts | Building | Layout | Editing |
|---|---|---|---|
| `FontCollection::register` | `ParagraphBuilder::new` | `layout` | `caret_for_offset` |
| `register_with` | `add_text` | `lines` | `glyph_position_at` |
| `add_fallback` | `style` | `line_metrics` | `rects_for_range` |
| `add_alias` | `build` | `width` · `height` | `word_boundary` |
| `family` | `TextStyle::new` | `bounds` | `update_color` |
| `family_variant` | `ParagraphStyle` | `min_intrinsic_width` | `truncated` |
| `FaceSet::grown_by` | `Shadow` | `max_intrinsic_width` | `text` |
| `FontSource` | `Decoration` | `longest_line` | `demand` |
| `FontAttrs` | `TextAlign` | `Line` · `LineMetrics` | `PositionWithAffinity` |

### Device and output

| Context | Targets | Images | Diagnostics |
|---|---|---|---|
| `Context::new` | `RenderTarget` | `upload_image` | `RenderStats` |
| `render` | `Surface::new` | `import_image` | `memory_report` |
| `set_text_tiers` | `acquire` · `present` | `upload_image_bitmap` | `MemoryReport` |
| `set_text_raster_hold` | `resize` | `ImageDesc` | `AtlasReport` |
| `set_raster_hold` | `Offscreen::new` | `Sampling` | `PoolReport` |
| `set_hide_missing_glyphs` | `target` | `unpremultiply` | `WgpuCounters` |
| | `ExternalMetalTexture` | | `Hud` |

## Examples

Every example renders headless to `target/examples/<name>.png`:

```sh
cargo run -p valo --example hello        # start here: three shapes, ~20 lines
cargo run -p valo --example rects        # blends, transforms, retained lists, culling
cargo run -p valo --example paths        # stencil-then-cover fills, both fill rules
cargo run -p valo --example clips        # intersect/difference/nested, auto-expiry
cargo run -p valo --example images       # upload, mips, sampling, tiling, opacity
cargo run -p valo --example gradients    # linear/radial/sweep, gradients on paths
cargo run -p valo --example layers       # save layers, group opacity, elision
cargo run -p valo --example blends       # 13 advanced (dst-reading) blend modes
cargo run -p valo --example shadows      # analytic (r)rect shadows + mask blur
cargo run -p valo --example backdrop     # frosted glass, shared backdrop blurs
cargo run -p valo --example mask_styles  # blur styles + per-corner rrect radii
cargo run -p valo --example strokes      # caps/joins/miter/dash/hairline/gradient
cargo run -p valo --example text         # paragraphs: wrap, fallback, bidi, SDF
cargo run -p valo --example text_zoom    # text tiers staying crisp across zooms
```

Two run interactively in a native window, and the browser playground builds from the same engine:

```sh
cargo run -p valo --example window       # animated transforms + retained lists
cargo run -p valo --example board        # Figma-style pan/zoom board, ~3.3k draws, live HUD
npm install
npm run dev:web                         # five raw/Canvas API chapters
```

## Testing

```sh
cargo test                   # unit tests + 34 golden pixel tests on a headless device
VALO_BLESS=1 cargo test      # accept new goldens after an intended visual change
cargo bench -p valo          # criterion: record, frame, text, geometry
```

## License

MIT. See [LICENSE](LICENSE).

The fonts under `assets/` are third-party test and example assets under their own licenses — see [assets/fonts/README.md](assets/fonts/README.md). No crate embeds or ships a font.
