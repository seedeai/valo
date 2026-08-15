//! The recording layer: `DisplayListBuilder` records draw commands into an immutable,
//! retained [`DisplayList`], computing the **record-time oracle** as it goes — per-op
//! device bounds, list bounds, depth-slot counts. The renderer replays lists without
//! re-deriving any of it — the recorder knows everything, so it says so at record
//! time rather than making the renderer rediscover it.
//!
//! `DisplayList` is a first-class retained object:
//! immutable, `Arc`-shared, **nestable** via [`DisplayListBuilder::draw_display_list`],
//! with a stable [`DisplayList::id`] — hosts keep one list per layer and recompose a
//! cheap top-level list each frame; the id + bounds invariants are the future hooks for
//! raster caching and damage rects (deliberately not built yet).
//!
//! GPU-free and `Send + Sync`: record on any thread, no `Context` required.

mod builder;
mod color_filter;
mod list;
mod paint;
mod resources;
mod shader;

pub use builder::DisplayListBuilder;
pub use list::{BackdropGroup, ClipOp, DisplayList, GlyphPos, MaskKind, Op};
pub use paint::{BlendMode, BlurStyle, ColorFilter, ImageFilter, MaskBlur, Paint, PaintStyle};
pub use resources::{Filter, Image, ImageInner, MipmapMode, Sampling, TileMode};
pub use shader::{FocalCircle, GradientStop, Shader, SpreadMode, MAX_GRADIENT_STOPS};
