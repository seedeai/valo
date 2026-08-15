//! Raw, typed WebAssembly boundary for Valo.
//!
//! This crate deliberately mirrors Valo's retained objects. Canvas2D policy
//! lives in TypeScript; the Rust boundary only owns GPU resources and records
//! display-list operations.
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
pub use text::{WebFontCollection, WebParagraph};
