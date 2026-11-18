//! The strut a paragraph's style asks for, resolved against the fonts it was
//! built with: skparagraph's `ParagraphImpl::resolveStrut` and its
//! `InternalLineMetrics::updateLineMetrics`.
//!
//! A strut is an invisible run on every line, so a paragraph resolves it once,
//! when it is built, and hands its metrics to every line the wrapper and the
//! placer make. It draws nothing.

use valo_text::{FaceSet, Font};

use crate::style::StrutStyle;
use crate::wrap::LineHeights;

/// `Strut` is a paragraph's strut measured against one font: the room every
/// line must have, and whether it replaces each line's room rather than being
/// a floor under it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Strut {
    heights: LineHeights,
    /// `strut_box` is the room the strut itself takes above and below the
    /// baseline, its leading left out, when its size gives a box at all: what
    /// skparagraph's `kStrut` height style draws for a selection.
    strut_box: Option<StrutBox>,
    force: bool,
}

/// `StrutBox` is a strut's own ascent and descent about the baseline, its
/// leading left out: skparagraph's `strutMetrics()` as `TextLine`'s `kStrut`
/// case reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StrutBox {
    pub ascent: f32,
    pub descent: f32,
}

impl Strut {
    /// `resolve` measures `style` against `faces`: skparagraph's `resolveStrut`,
    /// which finds the strut font itself and keeps no strut when it is off or
    /// resolves to nothing.
    pub(crate) fn resolve(faces: &FaceSet, style: &StrutStyle) -> Option<Self> {
        if !style.enabled || style.size < 0.0 || faces.is_empty() {
            return None;
        }
        let id = faces.resolve(&style.families, style.font_attrs(), ' ');
        let heights = strut_heights(faces.get(id), style);
        // A size of zero asks for no box, as skparagraph's `kStrut` case
        // leaves the measured run box unless the strut's size is above zero.
        let strut_box = (style.size > 0.0).then(|| StrutBox {
            ascent: heights.ascent - heights.leading / 2.0,
            descent: heights.descent - heights.leading / 2.0,
        });
        Some(Self {
            heights,
            strut_box,
            force: style.force_height,
        })
    }

    /// `heights` is the room the strut asks for: every line's when it is
    /// forced, and what a paragraph with no text has to measure by.
    pub(crate) fn heights(&self) -> LineHeights {
        self.heights
    }

    /// `strut_box` is the box the strut's own metrics make for a selection,
    /// when its size is above zero.
    pub(crate) fn strut_box(&self) -> Option<StrutBox> {
        self.strut_box
    }

    /// `apply` is a line's room after this strut: skparagraph's
    /// `InternalLineMetrics::updateLineMetrics`.
    ///
    /// Forced, the line takes the strut's room whole, its raw metrics included,
    /// so every line is the same height. Otherwise the line keeps the room it
    /// has wherever that is more than the strut's on a side, and the strut's
    /// gap counts on top of the line's own half leading, as skparagraph's
    /// comparison of the two adds it.
    pub(crate) fn apply(&self, line: LineHeights) -> LineHeights {
        if self.force {
            return self.heights;
        }
        let half_leading = line.leading / 2.0;
        LineHeights {
            ascent: line.ascent.max(self.heights.ascent + half_leading),
            descent: line.descent.max(self.heights.descent + half_leading),
            leading: line.leading,
            raw_ascent: line.raw_ascent.max(self.heights.raw_ascent),
            raw_descent: line.raw_descent.max(self.heights.raw_descent),
        }
    }
}

/// `strut_heights` is the room a strut's font and style ask for, as
/// skparagraph's `resolveStrut` works it from the font's metrics.
fn strut_heights(font: &Font, style: &StrutStyle) -> LineHeights {
    let size = style.size;
    let (ascent, descent) = (font.ascent_px(size), font.descent_px(size));
    let gap = font.line_height_px(size) - ascent - descent;
    let leading = style.leading.map_or(0.0, |multiple| multiple * size);
    let half_leading = leading / 2.0;
    if !style.height_override {
        return LineHeights {
            ascent: ascent + half_leading,
            descent: descent + half_leading,
            leading,
            raw_ascent: ascent + half_leading,
            raw_descent: descent + half_leading,
        };
    }
    let (strut_ascent, strut_descent) = if style.half_leading {
        // Half leading's occupied height is the font's ascent and descent
        // alone; the gap is not part of the room it shares.
        let flexible = (style.height * size - (ascent + descent)) / 2.0;
        (ascent + flexible, descent + flexible)
    } else {
        // A font whose ascent, descent and gap are all zero has no height to
        // scale, so the multiplier stands in for one, as skparagraph does.
        // The gap counts in the divisor while the strut's leading stays the
        // explicit one, so a font with a gap lands short of `height × size`.
        let metrics_height = ascent + descent + gap;
        let multiplier = if metrics_height == 0.0 {
            style.height
        } else {
            style.height * size / metrics_height
        };
        (ascent * multiplier, descent * multiplier)
    };
    LineHeights {
        ascent: strut_ascent + half_leading,
        descent: strut_descent + half_leading,
        leading,
        raw_ascent: ascent + gap / 2.0,
        raw_descent: descent + gap / 2.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use valo_text::FontCollection;

    /// `line_gap_faces` is the test font whose ascent, descent and line gap are
    /// 0.935, 0.265 and 0.2 of its size: 18.7, 5.3 and 4 px at 20 px.
    fn line_gap_faces() -> FaceSet {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
        let mut collection = FontCollection::new();
        collection
            .register(
                "Line Gap Test",
                std::fs::read(format!("{dir}/line_gap_test.ttf")).unwrap(),
            )
            .unwrap();
        collection.faces().clone()
    }

    fn line_gap_strut(style: StrutStyle) -> Strut {
        Strut::resolve(&line_gap_faces(), &style).expect("an enabled strut with a font")
    }

    fn strut_style() -> StrutStyle {
        StrutStyle {
            enabled: true,
            families: vec!["Line Gap Test".to_owned()],
            size: 20.0,
            ..StrutStyle::default()
        }
    }

    /// Half leading shares `height × size` evenly above and below the font's
    /// own room, the gap aside: 8 px over 18.7 and 8 px under 5.3 at 40 px.
    #[test]
    fn a_half_leading_strut_is_its_height() {
        let strut = line_gap_strut(StrutStyle {
            height: 2.0,
            height_override: true,
            half_leading: true,
            ..strut_style()
        });
        assert!((strut.heights.ascent - 26.7).abs() < 1e-4, "{strut:?}");
        assert!((strut.heights.descent - 13.3).abs() < 1e-4, "{strut:?}");
        assert!((strut.heights.height() - 40.0).abs() < 1e-4);
    }

    /// A proportional height override scales the font's ascent and descent by
    /// `height × size` over its whole line height, gap included, while the
    /// strut's leading stays the explicit one: 18.7 and 5.3 scaled by 40/28,
    /// as skparagraph's `resolveStrut` works it.
    #[test]
    fn a_proportional_strut_scales_the_font_without_its_gap() {
        let strut = line_gap_strut(StrutStyle {
            height: 2.0,
            height_override: true,
            ..strut_style()
        });
        assert!((strut.heights.ascent - 18.7 * 40.0 / 28.0).abs() < 1e-4);
        assert!((strut.heights.descent - 5.3 * 40.0 / 28.0).abs() < 1e-4);
        assert_eq!(strut.heights.leading, 0.0);
    }

    /// A leading adds the same room above and below whatever the height, half
    /// of `leading × size` on each side.
    #[test]
    fn a_leading_lies_half_above_and_half_below() {
        let plain = line_gap_strut(strut_style());
        let led = line_gap_strut(StrutStyle {
            leading: Some(0.75),
            ..strut_style()
        });
        assert!((led.heights.ascent - plain.heights.ascent - 7.5).abs() < 1e-4);
        assert!((led.heights.descent - plain.heights.descent - 7.5).abs() < 1e-4);
        assert_eq!(led.heights.leading, 15.0);
    }

    /// A strut is off unless asked for, and never negative.
    #[test]
    fn a_disabled_strut_resolves_to_nothing() {
        assert!(Strut::resolve(&line_gap_faces(), &strut_style()).is_some());
        assert!(Strut::resolve(
            &line_gap_faces(),
            &StrutStyle {
                enabled: false,
                ..strut_style()
            }
        )
        .is_none());
        assert!(Strut::resolve(
            &line_gap_faces(),
            &StrutStyle {
                size: -1.0,
                ..strut_style()
            }
        )
        .is_none());
    }

    /// A forced strut replaces the line whole; a plain one only raises a side
    /// the line is short on, the line's own half leading counting against the
    /// strut's gap, as skparagraph's comparison adds it.
    #[test]
    fn a_strut_either_replaces_a_line_or_raises_it() {
        let line = LineHeights {
            ascent: 40.0,
            descent: 4.0,
            leading: 6.0,
            raw_ascent: 37.0,
            raw_descent: 3.0,
        };
        let strut = Strut {
            heights: LineHeights {
                ascent: 25.0,
                descent: 5.0,
                leading: 0.0,
                raw_ascent: 25.0,
                raw_descent: 5.0,
            },
            strut_box: None,
            force: false,
        };
        let raised = strut.apply(line);
        assert_eq!(raised.ascent, 40.0);
        assert_eq!(raised.descent, 8.0);
        assert_eq!(raised.raw_descent, 5.0);
        assert_eq!(raised.leading, 6.0);

        let forced = Strut {
            force: true,
            ..strut
        };
        assert_eq!(forced.apply(line), forced.heights);
    }
}
