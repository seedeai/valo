//! The wgpu core: replays a recorded `DisplayList` into render passes. Directly on
//! wgpu — WebGPU is already a portable encoder, so there is no HAL layer here.
//!
//! Memory model: the renderer is **stateless with respect to content** —
//! everything it holds is a content-keyed or frame-scoped cache:
//! - [`HostBuffer`]: per-frame bump arena for transient uniforms, a 3-frame ring of
//!   persistent buffers + bind groups (creates happen only on first growth; warm
//!   frames create nothing — on wasm every create is a JS-boundary hop).
//! - [`PipelineCache`]: grow-only map of pipeline variants (format × blend × samples).

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
