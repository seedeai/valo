//! valo-text — fonts, shaping and glyph raster, no GPU: what Skia keeps in
//! its core and `skshaper`, beneath the paints the display list records.
//! Fonts register into collections that resolve families, styles and
//! per-character fallback; the shaper turns one string in one font into
//! glyphs with harfrust; glyph raster (swash alpha masks, SDF distance
//! fields, skrifa outline paths, COLR) is what the renderer's atlas calls in.
//! The styled paragraph (spans, bidi, wrapping, painting) is `valo-paragraph`,
//! above `valo-dl`, so its styles can hold paints.

mod colr;
mod font;
mod raster;
mod sdf;
mod shaper;
mod synthesis;
#[cfg(feature = "woff2")]
mod woff2;

#[cfg(feature = "woff2")]
#[doc(hidden)]
pub fn woff2_to_sfnt_for_probe(bytes: &[u8]) -> Option<Vec<u8>> {
    woff2::to_sfnt(bytes, 0)
}

pub use font::{
    match_style, FaceSet, Font, FontAttrs, FontCollection, FontData, FontDemand, FontId,
    FontSource, FontUid,
};
pub use raster::{glyph_path, GlyphImage, GlyphRaster, GlyphStroke, Rasterizer, SDF_PAD};
pub use shaper::{shape, FontFeature, ShapedGlyph};
pub use synthesis::FontSynthesis;
