use valo_geometry::{Color, Point};

/// How one span of text looks (skparagraph's TextStyle, the subset valo implements).
/// Families are tried in order per character (then the collection's fallback
/// chain); `weight`/`italic` pick within a family's variants.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    pub families: Vec<String>,
    /// CSS weight, 100–900.
    pub weight: u16,
    pub italic: bool,
    /// CSS `font-width` (legacy `font-stretch`) percentage; 100 is normal.
    /// Selects among a family's registered widths — valo never synthesizes
    /// one, and neither do browsers.
    pub stretch: f32,
    /// Let the font kern (`kern`). Canvas2D's `fontKerning: "none"` clears it.
    pub kerning: bool,
    /// OpenType capital-letter forms — Canvas2D's `fontVariantCaps`.
    pub variant_caps: VariantCaps,
    pub size: f32,
    pub color: Color,
    /// Added after every grapheme cluster (px).
    pub letter_spacing: f32,
    /// Added after every U+0020 cluster (px), on top of `letter_spacing`.
    pub word_spacing: f32,
    /// Line-height multiplier: `Some(1.5)` = 1.5 × size, metrics scaled
    /// proportionally (skparagraph's setHeight + heightOverride). `None` =
    /// the font's own metrics.
    pub height: Option<f32>,
    pub decoration: Option<Decoration>,
    /// Painted back-to-front UNDER the text, each a blurred offset copy —
    /// Flutter's TextStyle.shadows lowering.
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
    pub fn new(family: &str, size: f32, color: Color) -> Self {
        Self {
            families: vec![family.to_owned()],
            size,
            color,
            ..Default::default()
        }
    }

    /// What this style asks of face selection — the CSS matching axes,
    /// separated from everything shaping and painting care about.
    pub fn font_attrs(&self) -> crate::font::FontAttrs {
        crate::font::FontAttrs {
            weight: self.weight,
            italic: self.italic,
            stretch: self.stretch,
        }
    }
}

/// CSS `font-variant-caps`, lowered to the OpenType features a shaper
/// understands. Every variant here is a font capability: a face without the
/// feature renders unchanged rather than synthesizing small capitals, which
/// is what browsers do for `font-synthesis: none` and what valo always does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VariantCaps {
    #[default]
    Normal,
    /// `smcp` — lowercase becomes small capitals.
    SmallCaps,
    /// `c2sc` + `smcp` — capitals shrink to small capitals too.
    AllSmallCaps,
    /// `pcap` — the lighter-weight petite variant.
    PetiteCaps,
    /// `c2pc` + `pcap`.
    AllPetiteCaps,
    /// `unic` — lowercase-looking capitals.
    Unicase,
    /// `titl` — capitals cut for all-caps display sizes.
    TitlingCaps,
}

impl VariantCaps {
    /// The OpenType tags this variant turns ON, in application order.
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

/// An underline / strike-through / overline, drawn from the font's own
/// decoration metrics (post/OS2 tables; skparagraph's Decorations.cpp).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decoration {
    pub kind: DecorationKind,
    /// `None` = the text color.
    pub color: Option<Color>,
    /// Multiplier over the font's suggested thickness.
    pub thickness: f32,
}

impl Decoration {
    pub fn new(kind: DecorationKind) -> Self {
        Self {
            kind,
            color: None,
            thickness: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecorationKind {
    Underline,
    LineThrough,
    Overline,
}

/// One text shadow: an offset, optionally blurred copy in `color`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub color: Color,
    pub offset: Point,
    /// Gaussian σ; 0 = a hard offset copy.
    pub blur: f32,
}

/// Paragraph-level horizontal alignment (needs a finite layout width).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
    /// Word gaps stretch to fill the width; a paragraph's last line (and
    /// lines ending in a hard break) stay ragged, CSS-style.
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

/// Which way the paragraph reads before its content gets a say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextDirection {
    Ltr,
    Rtl,
}

/// Paragraph-level knobs, fixed at `build` (Flutter's ParagraphStyle).
#[derive(Clone, Debug, Default)]
pub struct ParagraphStyle {
    pub align: TextAlign,
    /// The bidi base level. `None` infers it from the first strong character
    /// (UAX #9 rules P2/P3); `Some(..)` forces it, which is what CSS
    /// `direction` and Canvas2D's `direction` ask for. Forcing matters for
    /// text that is entirely neutral — digits and punctuation carry no
    /// direction of their own, so only the base level orders them.
    pub direction: Option<TextDirection>,
    /// Include trailing whitespace in line advances. Canvas text enables this;
    /// paragraph layout defaults to trimmed line widths like SkParagraph.
    pub preserve_trailing_whitespace: bool,
    /// Stop wrapping after this many lines; content past them is dropped
    /// (see `ellipsis`).
    pub max_lines: Option<u32>,
    /// Spliced onto a truncated last line (shaped in that line's trailing
    /// style), on the visual end matching the paragraph's base direction.
    pub ellipsis: Option<String>,
}
