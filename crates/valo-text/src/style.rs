use valo_geometry::{Color, Point};

/// `TextStyle` controls font selection and painting for a span of text.
///
/// Families are tried in order for each character before the collection's
/// fallback fonts.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    /// `families` lists preferred font families in fallback order.
    pub families: Vec<String>,
    /// `weight` selects a CSS font weight, conventionally from 100 to 900.
    pub weight: u16,
    /// `italic` selects an italic face when available.
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
    /// `color` is the text fill color.
    pub color: Color,
    /// `letter_spacing` adds logical pixels after each grapheme cluster.
    pub letter_spacing: f32,
    /// `word_spacing` adds logical pixels after each space, in addition to letter spacing.
    pub word_spacing: f32,
    /// `height` overrides line height as a multiple of `size`.
    ///
    /// `None` uses the font's metrics.
    pub height: Option<f32>,
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
            stretch: crate::font::NORMAL_STRETCH,
            kerning: true,
            variant_caps: VariantCaps::Normal,
            size: 14.0,
            color: Color::BLACK,
            letter_spacing: 0.0,
            word_spacing: 0.0,
            height: None,
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
    pub fn font_attrs(&self) -> crate::font::FontAttrs {
        crate::font::FontAttrs {
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
    pub ellipsis: Option<String>,
}
