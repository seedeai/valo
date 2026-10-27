use crate::font::FontAttrs;

/// `FontSynthesis` is the bold and italic a run of text fakes because its face lacks them.
///
/// A family registered without a bold or an italic face still draws bold and italic text:
/// the glyphs of the face it has are thickened or slanted, as Skia's paragraph fakes missing
/// font styles (`fakeMissingFontStyles`, on by default) and browsers synthesize them.
/// Advances and metrics stay the face's own, so faking a style never moves text.
/// [`FontSynthesis::needed`] decides it for a run; [`FontSynthesis::NONE`] draws the face as
/// it is. Colour glyphs, such as emoji, are drawn unchanged.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FontSynthesis {
    /// `embolden` is how much fake bold thickens a glyph, as a fraction of the font size: the
    /// width of the stroke Skia strokes and fills the outline with, so each edge moves out by
    /// half of it. 0 is no fake bold.
    pub embolden: f32,
    /// `skew` is the horizontal shear fake italic slants a glyph by, as Skia's
    /// `SkFont::setSkewX`: each point's x moves by `skew` times its y, in valo's y-down
    /// coordinates around the glyph's baseline origin, so a negative skew leans the top of a
    /// glyph right. 0 is upright.
    pub skew: f32,
}

/// Fake italic's shear: Skia's paragraph sets `SkFont::setSkewX(-1/4)`.
const FAKE_ITALIC_SKEW: f32 = -0.25;

/// The lightest requested weight fake bold answers: Skia's `SkFontStyle::kSemiBold_Weight`.
const FAKE_BOLD_MIN_WEIGHT: u16 = 600;

/// How much heavier than the face a request must be for fake bold.
const FAKE_BOLD_MIN_GAIN: u16 = 200;

/// Fake bold's stroke width over the text size, as (size, fraction) keys: the first fraction
/// at and below the first size, the last at and above the last, eased linearly between.
/// Skia's `kStdFakeBoldInterpKeys` and `kStdFakeBoldInterpValues` (`SkTextFormatParams.h`).
const FAKE_BOLD_WIDTHS: [(f32, f32); 2] = [(9.0, 1.0 / 24.0), (36.0, 1.0 / 32.0)];

/// How far, in strengths, swash's embolden can move a corner along its bisector: as
/// FreeType's `FT_Outline_EmboldenXY`, it stops shifting corners sharper than a turn whose
/// cosine is −0.9396, where the shift is √(2 / (1 − 0.9396)) ≈ 5.75 strengths.
const EMBOLDEN_CORNER_REACH: f32 = 5.76;

impl FontSynthesis {
    /// `NONE` draws a face as it is.
    pub const NONE: FontSynthesis = FontSynthesis {
        embolden: 0.0,
        skew: 0.0,
    };

    /// `needed` returns what `face` fakes to stand in for `wanted` at `size`, by Skia's rule.
    ///
    /// Fake bold when the requested weight is semibold (600) or heavier and at least 200 above
    /// the face's; fake italic when italic is requested and the face is upright. The rule is
    /// SkParagraph's `OneLineShaper`, except that an oblique face counts as italic here, since
    /// [`FontAttrs`] does not tell the two apart.
    pub fn needed(wanted: FontAttrs, face: FontAttrs, size: f32) -> FontSynthesis {
        let fake_bold = wanted.weight >= FAKE_BOLD_MIN_WEIGHT
            && wanted.weight.saturating_sub(face.weight) >= FAKE_BOLD_MIN_GAIN;
        let fake_italic = wanted.italic && !face.italic;
        FontSynthesis {
            embolden: if fake_bold {
                fake_bold_width(size)
            } else {
                0.0
            },
            skew: if fake_italic { FAKE_ITALIC_SKEW } else { 0.0 },
        }
    }

    /// `is_none` reports whether the face draws as it is.
    pub fn is_none(&self) -> bool {
        self.embolden == 0.0 && self.skew == 0.0
    }

    /// How far fake bold moves each edge of a glyph drawn at `size`.
    pub(crate) fn edge_growth(&self, size: f32) -> f32 {
        self.embolden * size / 2.0
    }

    /// A glyph ink box `(x_min, y_min, x_max, y_max)`, y-up at `size`, grown to hold the same
    /// glyphs synthesized: outset by the furthest fake bold can move a corner, then sheared.
    pub(crate) fn ink_box(
        &self,
        (x_min, y_min, x_max, y_max): (f32, f32, f32, f32),
        size: f32,
    ) -> (f32, f32, f32, f32) {
        let reach = self.edge_growth(size) * EMBOLDEN_CORNER_REACH;
        let (x_min, y_min, x_max, y_max) =
            (x_min - reach, y_min - reach, x_max + reach, y_max + reach);
        // y-up, so the shear is `-skew`: x moves by `-skew × y`.
        let (low, high) = (-self.skew * y_min, -self.skew * y_max);
        (x_min + low.min(high), y_min, x_max + low.max(high), y_max)
    }
}

/// Fake bold's stroke width over `size`, Skia's `SkFloatInterpFunc` over [`FAKE_BOLD_WIDTHS`].
fn fake_bold_width(size: f32) -> f32 {
    let [(small_size, small_width), (large_size, large_width)] = FAKE_BOLD_WIDTHS;
    let along = ((size - small_size) / (large_size - small_size)).clamp(0.0, 1.0);
    small_width + (large_width - small_width) * along
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs(weight: u16, italic: bool) -> FontAttrs {
        FontAttrs {
            weight,
            italic,
            ..FontAttrs::default()
        }
    }

    /// Skia's thresholds, at their edges: bold from 600 and 200 above the face, italic only
    /// over an upright face.
    #[test]
    fn fakes_only_what_skia_fakes() {
        let regular = attrs(400, false);
        let at = |wanted: FontAttrs, face: FontAttrs| FontSynthesis::needed(wanted, face, 16.0);
        assert!(at(attrs(600, false), regular).embolden > 0.0);
        assert!(at(attrs(700, false), attrs(500, false)).embolden > 0.0);
        assert_eq!(
            at(attrs(500, false), attrs(300, false)),
            FontSynthesis::NONE
        );
        assert_eq!(
            at(attrs(700, false), attrs(600, false)),
            FontSynthesis::NONE
        );
        assert_eq!(
            at(attrs(400, false), attrs(700, false)),
            FontSynthesis::NONE
        );
        assert_eq!(at(attrs(400, true), regular).skew, -0.25);
        assert_eq!(at(attrs(400, true), attrs(400, true)), FontSynthesis::NONE);
        assert_eq!(at(attrs(400, false), attrs(400, true)), FontSynthesis::NONE);
    }

    /// The stroke is 1/24 of the size up to 9, 1/32 from 36, and eases between.
    #[test]
    fn fake_bold_width_eases_from_a_24th_to_a_32nd() {
        assert_eq!(fake_bold_width(6.0), 1.0 / 24.0);
        assert_eq!(fake_bold_width(9.0), 1.0 / 24.0);
        assert_eq!(fake_bold_width(36.0), 1.0 / 32.0);
        assert_eq!(fake_bold_width(72.0), 1.0 / 32.0);
        let middle = fake_bold_width(22.5);
        assert!((middle - (1.0 / 24.0 + 1.0 / 32.0) / 2.0).abs() < 1e-6);
    }
}
