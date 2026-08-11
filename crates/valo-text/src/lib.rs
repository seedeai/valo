//! valo-text — typographer + paragraph layout, no GPU.
//! The pipeline: style spans → per-character font fallback →
//! bidi levels → harfrust shaping (endless line) → greedy wrapping with
//! UAX #14 opportunities → per-line UAX #9 visual reorder → placed glyph runs.
//! Glyph raster (swash alpha masks, SDF distance fields, skrifa outline
//! paths) also lives here — CPU-only; the renderer's atlas calls in.

mod colr;
mod font;
mod paragraph;
mod raster;
mod sdf;
mod shape;
mod style;
mod wrap;

pub use font::{
    FaceSet, Font, FontAttrs, FontCollection, FontData, FontDemand, FontId, FontSource,
};
pub use paragraph::{
    Line, LineMetrics, Paragraph, ParagraphBuilder, PlacedGlyph, PlacedRun, PositionWithAffinity,
};
pub use raster::{glyph_path, GlyphImage, Rasterizer, SDF_PAD};
pub use style::{Decoration, DecorationKind, ParagraphStyle, Shadow, TextAlign, TextStyle};
