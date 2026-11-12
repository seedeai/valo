use valo_dl::Paint;
use valo_geometry::{Color, Point};
use valo_text::FontAttrs;

/// `TextStyle` controls font selection and painting for a span of text.
///
/// Families are tried in order for each character before the collection's
/// fallback fonts.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    /// `families` lists preferred font families in fallback order.
    pub families: Vec<String>,
    /// `weight` selects a CSS font weight, conventionally from 100 to 900.
    ///
    /// A family without a face this heavy fakes bold on its nearest one, as
    /// [`valo_text::FontSynthesis::needed`] decides.
    pub weight: u16,
    /// `italic` selects an italic face when available, and slants an upright
    /// one when not.
    pub italic: bool,
    /// `stretch` selects CSS font width as a percentage, where 100 is normal.
    ///
    /// Valo selects a registered width and does not synthesize one.
    pub stretch: f32,
    /// `kerning` enables the font's kerning adjustments.
    pub kerning: bool,
    /// `variant_caps` selects OpenType capital-letter forms.
    pub variant_caps: VariantCaps,
    /// `size` is the font size in logical pixels.
    pub size: f32,
    /// `color` is the text fill color, used when `foreground` is `None`.
    ///
    /// A decoration without a color of its own takes this color even when
    /// `foreground` is set, as Skia's does.
    pub color: Color,
    /// `foreground` paints the glyphs in place of `color` when set.
    ///
    /// A stroke style outlines the glyphs, and a shader fills them with a
    /// gradient or an image. Shadows keep their own colors.
    pub foreground: Option<Paint>,
    /// `background` fills a box behind the span's characters when set.
    ///
    /// The box spans the characters' advances across, and the font's ascent
    /// and descent down, scaled by `height` when it is set: Skia's run box.
    /// Each line's backgrounds are painted before its shadows and glyphs, as
    /// Skia's are (`TextLine::paint`), so a background lies beneath every
    /// glyph of its line.
    pub background: Option<Paint>,
    /// `letter_spacing` adds logical pixels after each grapheme cluster.
    pub letter_spacing: f32,
    /// `word_spacing` adds logical pixels after each space, in addition to letter spacing.
    pub word_spacing: f32,
    /// `height` overrides line height as a multiple of `size`.
    ///
    /// `None` uses the font's metrics.
    pub height: Option<f32>,
    /// `leading_distribution` is how a `height` other than the font's own shares
    /// the space it adds or takes above and below the text.
    ///
    /// It changes nothing while `height` is `None`. Pick
    /// [`LeadingDistribution::Even`] for text that should sit where CSS puts it.
    pub leading_distribution: LeadingDistribution,
    /// `decoration` optionally adds an underline, overline, or strike-through.
    pub decoration: Option<Decoration>,
    /// `shadows` are painted back-to-front beneath the text.
    pub shadows: Vec<Shadow>,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            families: Vec::new(),
            weight: 400,
            italic: false,
            stretch: FontAttrs::default().stretch,
            kerning: true,
            variant_caps: VariantCaps::Normal,
            size: 14.0,
            color: Color::BLACK,
            foreground: None,
            background: None,
            letter_spacing: 0.0,
            word_spacing: 0.0,
            height: None,
            leading_distribution: LeadingDistribution::Proportional,
            decoration: None,
            shadows: Vec::new(),
        }
    }
}

impl TextStyle {
    /// `new` creates a style with one preferred family, size, and color.
    pub fn new(family: &str, size: f32, color: Color) -> Self {
        Self {
            families: vec![family.to_owned()],
            size,
            color,
            ..Default::default()
        }
    }

    /// `font_attrs` returns the attributes used to select a face within a family.
    pub fn font_attrs(&self) -> FontAttrs {
        FontAttrs {
            weight: self.weight,
            italic: self.italic,
            stretch: self.stretch,
        }
    }
}

/// `VariantCaps` selects an OpenType capitalization variant.
///
/// If a font lacks the requested feature, text remains unchanged; Valo does not
/// synthesize capital forms.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VariantCaps {
    /// `Normal` leaves capitalization features disabled.
    #[default]
    Normal,
    /// `SmallCaps` renders lowercase letters as small capitals.
    SmallCaps,
    /// `AllSmallCaps` renders both lowercase and uppercase letters as small capitals.
    AllSmallCaps,
    /// `PetiteCaps` renders lowercase letters as petite capitals.
    PetiteCaps,
    /// `AllPetiteCaps` renders both lowercase and uppercase letters as petite capitals.
    AllPetiteCaps,
    /// `Unicase` uses a mixture of uppercase and lowercase-sized capitals.
    Unicase,
    /// `TitlingCaps` uses capitals designed for display text.
    TitlingCaps,
}

impl VariantCaps {
    /// `feature_tags` returns the enabled OpenType tags in application order.
    pub fn feature_tags(self) -> &'static [&'static [u8; 4]] {
        match self {
            Self::Normal => &[],
            Self::SmallCaps => &[b"smcp"],
            Self::AllSmallCaps => &[b"c2sc", b"smcp"],
            Self::PetiteCaps => &[b"pcap"],
            Self::AllPetiteCaps => &[b"c2pc", b"pcap"],
            Self::Unicase => &[b"unic"],
            Self::TitlingCaps => &[b"titl"],
        }
    }
}

/// `Decoration` describes a line drawn relative to styled text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decoration {
    /// `kind` selects the decoration's position.
    pub kind: DecorationKind,
    /// `color` overrides the text color when set.
    pub color: Option<Color>,
    /// `thickness` multiplies the font's suggested decoration thickness.
    pub thickness: f32,
}

impl Decoration {
    /// `new` creates a text-colored decoration at the font's suggested thickness.
    pub fn new(kind: DecorationKind) -> Self {
        Self {
            kind,
            color: None,
            thickness: 1.0,
        }
    }
}

/// `DecorationKind` selects where a text decoration is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecorationKind {
    /// `Underline` draws below the baseline using the font's underline metrics.
    Underline,
    /// `LineThrough` draws through the text using the font's strikeout metrics.
    LineThrough,
    /// `Overline` draws above the text.
    Overline,
}

/// `Shadow` describes a colored, offset copy painted beneath text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    /// `color` is the shadow color.
    pub color: Color,
    /// `offset` moves the shadow in logical pixels.
    pub offset: Point,
    /// `blur` is the Gaussian sigma; zero produces a sharp copy.
    pub blur: f32,
}

/// `LeadingDistribution` is how a line height other than the font's own shares
/// its extra space, the leading, above and below the text: Flutter's
/// `TextLeadingDistribution`, Skia's half leading (`TextStyle::setHalfLeading`).
///
/// The leading is `height × size` less the font's ascent and descent. A height
/// below the font's own makes it negative, and it is taken away in the same
/// way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LeadingDistribution {
    /// `Proportional` shares the leading in proportion to the font's ascent and
    /// descent, so the text keeps its place relative to the line's top and
    /// bottom: Flutter's default.
    #[default]
    Proportional,
    /// `Even` puts half the leading above the text and half below, as CSS's
    /// `line-height` does: skparagraph's `Run::calculateMetrics` with half
    /// leading, where the font's line gap cancels out.
    Even,
}

/// `TextAlign` controls horizontal line placement within the layout width.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAlign {
    /// `Left` aligns lines with the left edge.
    #[default]
    Left,
    /// `Center` centers each line.
    Center,
    /// `Right` aligns lines with the right edge.
    Right,
    /// `Justify` expands word spacing to fill eligible lines.
    ///
    /// Final lines and lines ending in a hard break remain unexpanded.
    Justify,
}

impl From<TextAlign> for ParagraphStyle {
    fn from(align: TextAlign) -> Self {
        Self {
            align,
            ..Default::default()
        }
    }
}

/// `TextDirection` selects a paragraph's base writing direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextDirection {
    /// `Ltr` sets a left-to-right base direction.
    Ltr,
    /// `Rtl` sets a right-to-left base direction.
    Rtl,
}

/// `PlaceholderAlignment` says where a placeholder's box sits against the text of its line.
///
/// The `Top`, `Bottom` and `Middle` alignments measure against the font of the
/// [`TextStyle`] the placeholder was added with, as Skia's do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlaceholderAlignment {
    /// `Baseline` puts the point `baseline_offset` below the box's top on the baseline.
    #[default]
    Baseline,
    /// `AboveBaseline` puts the box's bottom on the baseline.
    AboveBaseline,
    /// `BelowBaseline` puts the box's top on the baseline.
    BelowBaseline,
    /// `Top` lines the box's top up with the top of the font's ascent.
    Top,
    /// `Bottom` lines the box's bottom up with the bottom of the font's descent.
    Bottom,
    /// `Middle` centres the box on the middle of the font's ascent and descent.
    Middle,
}

/// `TextBaseline` names the baseline a placeholder's `baseline_offset` measures to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextBaseline {
    /// `Alphabetic` is the baseline Latin letters sit on.
    #[default]
    Alphabetic,
    /// `Ideographic` is the bottom of ideographic characters, taken as half the line's
    /// descent below the alphabetic baseline, as Skia takes it.
    Ideographic,
}

/// `PlaceholderStyle` is a box laid out inline with text, such as a widget a host draws
/// inside a paragraph.
///
/// The box takes up its width in the line, wraps like a word, and makes its line tall enough
/// to hold it. The paragraph draws nothing for it; the host reads where it landed from
/// [`crate::Paragraph::placeholder_boxes`] and draws the box's content there.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlaceholderStyle {
    /// `width` is the box's advance in logical pixels.
    pub width: f32,
    /// `height` is the box's height in logical pixels.
    pub height: f32,
    /// `alignment` places the box against the line's text.
    pub alignment: PlaceholderAlignment,
    /// `baseline` is the baseline `baseline_offset` measures to.
    pub baseline: TextBaseline,
    /// `baseline_offset` is the distance from the box's top to its baseline, used only by
    /// [`PlaceholderAlignment::Baseline`].
    pub baseline_offset: f32,
}

/// `ParagraphStyle` controls layout behavior for a complete paragraph.
#[derive(Clone, Debug, Default)]
pub struct ParagraphStyle {
    /// `align` controls horizontal line alignment.
    pub align: TextAlign,
    /// `direction` sets the bidirectional base direction.
    ///
    /// `None` infers it from the first strong character. Set it explicitly for
    /// neutral text such as digits and punctuation.
    pub direction: Option<TextDirection>,
    /// `preserve_trailing_whitespace` includes trailing spaces in line widths.
    pub preserve_trailing_whitespace: bool,
    /// `max_lines` limits the number of laid-out lines.
    pub max_lines: Option<u32>,
    /// `ellipsis` replaces omitted content at the visual end of a truncated paragraph.
    ///
    /// With `max_lines` set, it ends the last line when text is left over. Without it,
    /// the paragraph stops at the first line that overflows the layout width, and that
    /// line ends with the ellipsis; lines that end at a hard break before it are kept.
    /// Either way the paragraph reports itself [`crate::Paragraph::truncated`], and
    /// without `max_lines` its minimum intrinsic width is its maximum, as Flutter's
    /// `TextPainter.ellipsis` does. An empty string ellipsizes nothing.
    pub ellipsis: Option<String>,
    /// `text_height_behavior` says whether the first line's top and the last
    /// line's bottom take the room a height multiplier gives them, or only the
    /// fonts' own.
    pub text_height_behavior: TextHeightBehavior,
}

/// `TextHeightBehavior` is whether a paragraph's first and last lines take the
/// room a [`TextStyle::height`] gives above and below the text, or keep only the
/// fonts' own there: Flutter's `TextHeightBehavior`, Skia's `TextHeightBehavior`
/// flags (`kDisableFirstAscent`, `kDisableLastDescent`).
///
/// Turning one off lets a paragraph sit flush with what is above or below it
/// while keeping the spacing between its lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextHeightBehavior {
    /// `apply_height_to_first_ascent` gives the first line the room above its
    /// text that its height asks for; off, it has its fonts' own ascent there.
    pub apply_height_to_first_ascent: bool,
    /// `apply_height_to_last_descent` gives the last line the room below its
    /// text that its height asks for; off, it has its fonts' own descent there.
    pub apply_height_to_last_descent: bool,
}

impl Default for TextHeightBehavior {
    fn default() -> Self {
        Self {
            apply_height_to_first_ascent: true,
            apply_height_to_last_descent: true,
        }
    }
}

impl TextHeightBehavior {
    /// `applied` is a line's room after this behavior: the fonts' own ascent
    /// above a `first` line and descent below a `last` one, where they are off.
    pub(crate) fn applied(
        self,
        heights: crate::wrap::LineHeights,
        first: bool,
        last: bool,
    ) -> crate::wrap::LineHeights {
        crate::wrap::LineHeights {
            ascent: if first && !self.apply_height_to_first_ascent {
                heights.raw_ascent
            } else {
                heights.ascent
            },
            descent: if last && !self.apply_height_to_last_descent {
                heights.raw_descent
            } else {
                heights.descent
            },
            ..heights
        }
    }
}
