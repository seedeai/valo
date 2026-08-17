# Valo

Valo is a WebGPU-native 2D render engine built with Rust and [wgpu](https://wgpu.rs/). It follows the architecture of Flutter's [Impeller](https://github.com/flutter/flutter/tree/master/engine/src/flutter/impeller) that guarantees no shader compilation needed at draw time.

To use valo, you can use the `valo` crate directly from Rust on native platforms or in a browser through WebAssembly. For JavaScript applications running in browser, you can use `valo-web` through valo's native API or its Canvas2D-compatible adapter.

Valo is built on top of [wgpu](https://wgpu.rs/) and thus supports all platforms that wgpu supports, including Windows, macOS, Linux, iOS, Android and the web.

Resources:

- [Documentation](https://valo.im/docs)
- [Playground](https://valo.im/playground)
- [Examples](crates/valo/examples)

## Which package to use

| Interface | Package | Runs on | Use it when |
| --- | --- | --- | --- |
| Rust | `valo` | Native and browser | Your application is written in Rust |
| JavaScript | `valo-web/raw` | Browser | You want raw power and control over the rendering engine |
| Canvas2D adapter | `valo-web` | Browser | Bringing existing Canvas2D-shaped code |
| C API | `valo-capi` | Native | When C ABI required |

## Rust

```sh
cargo add valo wgpu
```

Drawing with valo is done in two steps: 

- First record a `DisplayList` which is a plain record of drawing commands and can be reused between frames.
- Then submit that list to a `valo::Context` to trigger real rendering on the GPU. The following example draws a green rectangle:

```rust
use valo::{Color, DisplayListBuilder, Paint, Rect};

let mut builder = DisplayListBuilder::new();
builder.draw_rect(
    Rect::new(40.0, 40.0, 240.0, 140.0),
    &Paint::from_color(Color::rgb(0.78, 1.0, 0.24)),
);
let display_list = builder.build();
```

### Render to the GPU

Rendering requires:

- A `valo::Context`, created from a wgpu device and queue. See the device setup for [native Rust](packages/valo-site/content/docs/guides/embedding-rust.mdx) or [Rust in the browser](packages/valo-site/content/docs/guides/rust-browser.mdx).
- A `RenderTarget` to draw into. Acquire one from a `Surface` backed by a [winit window](crates/valo/examples/window.rs) or an [HTML canvas](crates/valo-web-demo/src/lib.rs), or use `Offscreen` when no display is needed.

Render and present an acquired surface frame:

```rust
let mut context = valo::Context::new(device, queue);
if let Some(frame) = surface.acquire() {
    context.render(&display_list, &frame.target(Some(Color::BLACK)));
    context.present(frame);
}
```

## JavaScript and TypeScript

```sh
npm install valo-web
```

Valo's engine API exposes the same recording model to JavaScript:

```ts
import {
  DisplayListBuilder,
  Paint,
  createDevice,
  initializeValo,
} from 'valo-web/raw';

await initializeValo();
const device = await createDevice();
const renderer = device.attach(document.querySelector('canvas')!);

const builder = new DisplayListBuilder();
const paint = new Paint(0.78, 1, 0.24, 1);
builder.drawRect(40, 40, 240, 140, paint);
const list = builder.build();
const stats = renderer.render(list, true, 0.04, 0.04, 0.05, 1);

// Valo's resources are allocated in WebAssembly memory and need to be freed manually.
stats?.free();
list.free();
builder.free();
paint.free();
renderer.free();
device.free();
```

For Canvas2D-shaped code, use the adapter:

```ts
import { createValoCanvas } from 'valo-web';

const context = await createValoCanvas(document.querySelector('canvas')!);
context.fillStyle = '#c8ff3d';
context.fillRect(40, 40, 240, 140);
```

> The default WebAssembly build requires WebGPU. For older browsers, use `valo-web/compat`, which can fall back to WebGL2. See [Browser requirements](https://valo.im/docs/getting-started#browser-requirements).

## Development

```sh
cargo test                  # Rust unit and headless golden tests
npm run build:site          # documentation site
npm run test:conformance    # compare the Canvas2D adapter with Chrome
```

## Contributing and AI

Valo is developed with strong assistance from AI agents, with humans leading the design, reviewing changes, testing, and remain responsible for correctness.

AI-assisted contributions are welcome as long as the contributor fully reviews the work, understands how it fits the architecture, manually reviews code changes and tests the work. Code that the contributor cannot explain, verify, or maintain should not be submitted.

## License

MIT licensed. Third-party test fonts under `assets/` retain their own licenses.
