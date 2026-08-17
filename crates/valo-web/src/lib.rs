//! `valo-web` is the WebAssembly JavaScript and TypeScript API for Valo.
//!
//! Callers record drawing commands on the CPU, upload images and fonts from the
//! page, and render finished display lists into a canvas. Canvas2D policy lives
//! in TypeScript; this crate owns GPU resources and the typed recording surface.
//!
//! Coordinates start at the top-left and grow downward, in logical pixels until
//! a transform says otherwise. Colors are straight-alpha sRGB; components are
//! conventionally `0..=1` and are not clamped at this boundary.
//!
//! JavaScript class names drop the `Web` prefix: `WebDevice` is `Device`,
//! `WebRenderer` is `Renderer`, and so on. Integer mode arguments match the
//! named objects in `valo-web/raw` (`BlendMode.SrcOver`, `ClipOp.Difference`,
//! …). Out-of-range integers use the default noted on each method rather than
//! throwing.
//!
//! Typical call order: `createDevice`, then `attach` per canvas, then record a
//! `WebDisplayListBuilder`, then `WebRenderer::render`. Uploaded `WebImage`
//! values and registered fonts must exist before a display list refers to them.
//! Wasm objects own Rust memory and expose `free()`.
#![cfg(target_arch = "wasm32")]

mod path;
mod recording;
mod renderer;
mod style;
mod text;
mod types;

pub use path::WebPath;
pub use recording::{WebDisplayList, WebDisplayListBuilder};
pub use renderer::{create_renderer, WebImage, WebRenderStats, WebRenderer};
pub use style::{WebColorFilter, WebImageFilter, WebPaint, WebShader};
pub use text::{WebFontCollection, WebParagraph, WebParagraphBuilder, WebTextStyle};
