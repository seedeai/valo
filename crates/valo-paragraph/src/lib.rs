//! valo-paragraph — styled paragraphs, Skia's `skparagraph`: it sits above
//! `valo-dl` so that a style can hold what the display list records.
//! The pipeline: style spans → per-character font fallback → bidi levels →
//! harfrust shaping (endless line) → greedy wrapping with UAX #14
//! opportunities → per-line UAX #9 visual reorder → placed glyph runs →
//! glyph runs, shadows and decorations recorded into a display list.
//! Fonts, the shaper and glyph raster are `valo-text`'s, beneath.

mod drawing;
mod paragraph;
mod shape;
mod style;
mod wrap;

pub use drawing::{DrawGlyphRunExt, DrawParagraphExt};
pub use paragraph::{
    Line, LineMetrics, Paragraph, ParagraphBuilder, PlacedGlyph, PlacedRun, PlaceholderBox,
    PositionWithAffinity,
};
pub use style::{
    Decoration, DecorationKind, ParagraphStyle, PlaceholderAlignment, PlaceholderStyle, Shadow,
    TextAlign, TextBaseline, TextDirection, TextStyle, VariantCaps,
};
