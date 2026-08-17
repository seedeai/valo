//! `valo-renderer` replays a recorded display list into wgpu render passes.
//!
//! Hosts normally use the `valo` crate's `Context`. This crate is the GPU core
//! that context wraps: it plans, encodes, and submits, and it owns only caches —
//! not application content. It talks to wgpu directly; there is no second GPU
//! abstraction.
//!
//! - [`HostBuffer`]: per-frame bump arena for transient uniforms and vertices.
//!   A 3-frame ring of persistent buffers means warm frames create nothing —
//!   the cost that matters most on wasm, where every create crosses into JS.
//! - [`PipelineCache`]: grow-only map of pipeline variants (format × blend × kind).

mod contours;
mod glyphs;
mod gpu_timer;
mod host_buffer;
mod images;
mod pipelines;
mod plan;
mod pool;
mod ramps;
mod raster;
mod renderer;
mod report;

pub use glyphs::TextTiers;
pub use host_buffer::HostBuffer;
pub use images::{ImageDesc, ImageStore, IMAGE_FORMAT};
pub use pipelines::{Frag, PipelineCache, PipelineKey, PipelineKind, DEPTH_FORMAT, SAMPLE_COUNT};
pub use pool::TargetPool;
pub use renderer::{RenderStats, RenderTarget, RendererCore};
pub use report::{AtlasReport, MemoryReport, PoolReport, WgpuCounters};
