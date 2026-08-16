# Valo

Valo is a WebGPU-native 2D rendering engine built with Rust and [wgpu](https://wgpu.rs/). It follows the architecture of Flutter's [Impeller](https://github.com/flutter/flutter/tree/master/engine/src/flutter/impeller) with no shader compilation at draw time — behind a user-facing API in the shape of Canvas2D and Skia.

## What WebGPU brings

- **On browsers**
  - Modern-GPU rendering without WebGL's overhead
  - One shared context drives every `<canvas>` on the page, no per-canvas resource allocation
  - Advanced rendering features without hacks needed

- **On native platforms**
  - Small binary, small memory footprint, fast rendering
  - The same code on Windows, macOS, Linux, Android and iOS

**Playground** [valo.im/playground](https://valo.im/playground)

## Install

For browsers:
```sh
npm install valo-web
```

For native platforms:
```sh
cargo add valo
```

## What it looks like

Recording touches no GPU: a `DisplayListBuilder` is pure CPU work you can run on any thread, keep, nest inside other lists, and replay every frame.

```rust
use valo::{Color, DisplayListBuilder, Paint, Rect};

let mut builder = DisplayListBuilder::new();
builder.draw_rrect_radii(
    Rect::new(40.0, 40.0, 400.0, 240.0),
    [24.0; 4],
    &Paint::from_color(Color::rgb(0.13, 0.15, 0.20)),
);
builder.draw_circle(
    (330.0, 180.0),
    70.0,
    &Paint::from_color(Color::rgba(0.30, 0.75, 0.95, 0.85)),
);
// Frosted glass over whatever is already recorded beneath.
builder.backdrop_blur(Rect::new(64.0, 200.0, 352.0, 56.0), 12.0);
let display_list = builder.build();
```

Rendering is where the GPU comes in — a `Context` wraps the wgpu device you own, and a `Surface` wraps your window's swapchain:

```rust
let mut context = valo::Context::new(device, queue);

// Each frame: acquire, render, present.
if let Some(frame) = surface.acquire() {
    context.render(&display_list, &frame.target(Some(Color::WHITE)));
    context.present(frame);
}
```

Gradients, image filters, blend modes, clips, layers and shaped paragraphs are all paints and draws on the same builder — the [examples](#examples) walk through each.

## From TypeScript

The raw API is the engine's own vocabulary — display lists, paints, shaders, paragraphs — with explicit resource lifetimes:

```ts
import { initializeValo, createRenderer, DisplayListBuilder, Paint } from "valo-web/raw";

await initializeValo();
const renderer = await createRenderer(document.querySelector("canvas")!);

const builder = new DisplayListBuilder();
const paint = new Paint(0.78, 1, 0.24, 1);
builder.drawRoundedRect(24, 24, 240, 140, new Float32Array([28, 8, 28, 8]), paint);
const list = builder.build();
renderer.render(list, true, 1, 1, 1, 1);

list.free(); builder.free(); paint.free();
```

The Canvas2D adapter runs existing drawing code on the same renderer:

```ts
import { createValoCanvas } from "valo-web";

const context = await createValoCanvas(document.querySelector("canvas")!);
context.fillStyle = "#c8ff3d";
context.roundRect(24, 24, 240, 140, [28, 8, 28, 8]);
context.fill();
context.backdropBlur(48, 72, 192, 56, 12); // Valo extension
```

It covers Canvas2D state, paths, transforms, clips, gradients, images, shadows, blends and text, and is checked against Chrome's own Canvas2D by a [differential conformance suite](packages/valo-conformance/README.md). Browsers without WebGPU can use `valo-web/compat`, which falls back to WebGL2 (raw API, one canvas per renderer).

## From C

Valo ships a C API — every function null-safe, every object an opaque handle with a matching `dispose`. The header is [`crates/valo-capi/include/valo.h`](crates/valo-capi/include/valo.h):

```c
ValoContext *context = valo_context_new();
ValoDisplayListBuilder *builder = valo_builder_new();

ValoPaint paint = {0};
paint.color = (ValoColor){0.96f, 0.35f, 0.25f, 1.0f};
valo_builder_draw_rect(builder, (ValoRect){40, 40, 400, 240}, paint);

ValoDisplayList *list = valo_builder_build(builder);
valo_context_render_to_pixels(context, list, (ValoColor){1, 1, 1, 1}, 480, 320, pixels);

valo_display_list_dispose(list);
valo_context_dispose(context);
```

The complete version — build, run, write an image — is [`examples/c/hello.c`](examples/c/hello.c), run by `./examples/c/run.sh`.

## Quick start (Rust)

```sh
cargo add valo wgpu pollster
```

Valo draws onto a device you own — a surface you configured, or a headless one:

```rust
use valo::{Color, Context, DisplayListBuilder, Paint, Rect};

fn main() {
    let instance = wgpu::Instance::default();
    let adapter =
        pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) =
        pollster::block_on(adapter.request_device(&Default::default())).unwrap();

    let mut builder = DisplayListBuilder::new();
    builder.draw_rect(
        Rect::new(40.0, 40.0, 160.0, 90.0),
        &Paint::from_color(Color::rgb(0.96, 0.35, 0.25)),
    );
    let display_list = builder.build();

    let mut context = Context::new(device, queue);
    let pixels = context.render_to_rgba(&display_list, [480, 320], Some(Color::WHITE));
    // 480×320 straight-alpha RGBA8 — hand it to any PNG encoder.
    println!("rendered {} bytes", pixels.len());
}
```

Or run the shipped version straight to a PNG:

```sh
cargo run -p valo --example hello       # → target/examples/hello.png
```

### In a window

With winit, valo's `Surface` wraps the window's swapchain and the frame loop is acquire → render → present:

```rust
use std::sync::Arc;
use valo::{Color, Context, DisplayListBuilder, Paint, Surface};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

#[derive(Default)]
struct App(Option<(Arc<Window>, Surface, Context)>);

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.0.is_some() {
            return;
        }
        let window = Arc::new(event_loop.create_window(Window::default_attributes()).unwrap());
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let size = window.inner_size();
        let surface = Surface::new(&instance, &adapter, &device, window.clone(), [size.width, size.height]).unwrap();
        self.0 = Some((window, surface, Context::new(device, queue)));
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some((window, surface, context)) = self.0.as_mut() else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => surface.resize([size.width, size.height]),
            WindowEvent::RedrawRequested => {
                let mut builder = DisplayListBuilder::new();
                builder.draw_circle(
                    (200.0, 150.0),
                    80.0,
                    &Paint::from_color(Color::rgb(0.78, 1.0, 0.24)),
                );
                let list = builder.build();
                if let Some(frame) = surface.acquire() {
                    context.render(&list, &frame.target(Some(Color::rgb(0.04, 0.04, 0.06))));
                    context.present(frame);
                }
                window.request_redraw();
            }
            _ => {}
        }
    }
}

fn main() {
    EventLoop::new().unwrap().run_app(&mut App::default()).unwrap();
}
```

The animated version with a retained list and a live HUD title is [`crates/valo/examples/window.rs`](crates/valo/examples/window.rs).

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
npm run dev:web                          # five raw/Canvas API chapters
```

## Testing

```sh
cargo test                   # unit tests + golden pixel tests on a headless device
VALO_BLESS=1 cargo test      # accept new goldens after an intended visual change
cargo bench -p valo          # criterion: record, frame, text, geometry
npm run test:conformance     # the same scenes through Valo and Chrome's Canvas2D, compared as pixels
npm run test:compat          # the WebGL2 fallback build, in a browser with no WebGPU
```

Failed conformance comparisons write both renders, the diff and the exact scene to `packages/valo-conformance/artifacts/`.

## License

MIT. See [LICENSE](LICENSE).

The fonts under `assets/` are third-party test and example assets under their own licenses — see [assets/fonts/README.md](assets/fonts/README.md). No crate embeds or ships a font.
