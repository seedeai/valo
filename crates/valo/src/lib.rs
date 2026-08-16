//! valo — a standalone 2D render engine on wgpu. The facade crate: everything a
//! host needs, one import.
//!
//! ```no_run
//! # fn demo(device: wgpu::Device, queue: wgpu::Queue, view: &wgpu::TextureView, texture: &wgpu::Texture) {
//! use valo::{Color, Context, DisplayListBuilder, Paint, Rect, RenderTarget};
//!
//! let mut ctx = Context::new(device, queue);
//! let mut b = DisplayListBuilder::new();
//! b.draw_rect(Rect::new(10.0, 10.0, 100.0, 80.0), &Paint::from_color(Color::rgb(0.9, 0.3, 0.2)));
//! let dl = b.build();
//! ctx.render(&dl, &RenderTarget {
//!     view,
//!     texture,
//!     format: wgpu::TextureFormat::Rgba8Unorm,
//!     size: [800, 600],
//!     clear: Some(Color::WHITE),
//! });
//! # }
//! ```
//!
//! Design principles and the memory model: see CLAUDE.md.

#![warn(missing_docs)]

mod context;
mod export;
mod hud;
mod surface;

pub use context::Context;
pub use export::unpremultiply;
#[cfg(target_os = "macos")]
pub use surface::{metal_device_of, wrap_metal_texture, ExternalMetalTexture};
pub use surface::{Offscreen, PersistentCanvas, Surface, SurfaceFrame};

pub use valo_dl::{
    BackdropGroup, BlendMode, BlurStyle, ClipOp, ColorFilter, DisplayList, DisplayListBuilder,
    Filter, FocalCircle, GradientStop, Image, ImageFilter, MaskBlur, MaskKind, MipmapMode, Op,
    Paint, PaintStyle, Sampling, Shader, SpreadMode, TileMode,
};
pub use valo_geometry::{
    constrain_radii, constrain_radii_elliptical, dash_contours, local_tolerance, stroke_contains,
    Cap, Color, ContourMeasure, Dash, FillRule, Join, Matrix, Path, PathBuilder, PathSample, Point,
    Rect, Size, Stroke, Winding,
};
pub use valo_renderer::{
    AtlasReport, ImageDesc, MemoryReport, PoolReport, RenderStats, RenderTarget, TextTiers,
    WgpuCounters,
};
pub use valo_text::{
    Decoration, DecorationKind, FaceSet, Font, FontAttrs, FontCollection, FontData, FontDemand,
    FontId, FontSource, Line, LineMetrics, Paragraph, ParagraphBuilder, ParagraphStyle, PlacedRun,
    PositionWithAffinity, Rasterizer, Shadow, TextAlign, TextDirection, TextStyle, VariantCaps,
};

mod text;
pub use hud::Hud;
pub use text::{DrawGlyphRunExt, DrawParagraphExt};
