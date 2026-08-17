use std::collections::HashMap;
use std::sync::Arc;

use skrifa::metrics::Metrics;
use skrifa::prelude::Size;
use skrifa::MetadataProvider;

/// `FontId` identifies a registered font within a [`FaceSet`] or [`FontCollection`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FontId(
    /// The zero-based registration index.
    pub u32,
);

/// `FontData` is shared, immutable font-file storage supplied by the host.
///
/// It may wrap owned bytes or memory-mapped data and must remain readable for
/// the lifetime of every [`Font`] created from it.
pub type FontData = Arc<dyn AsRef<[u8]> + Send + Sync>;

/// `FontAttrs` describes a face's position within a font family.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontAttrs {
    /// `weight` is the CSS font weight, conventionally from 100 to 900.
    pub weight: u16,
    /// `italic` indicates whether the face uses an italic or oblique style.
    pub italic: bool,
    /// `stretch` is the CSS font-width percentage, where 100 is normal.
    pub stretch: f32,
}

/// `NORMAL_STRETCH` is the CSS normal font-width percentage.
pub const NORMAL_STRETCH: f32 = 100.0;

impl Default for FontAttrs {
    fn default() -> Self {
        Self {
            weight: 400,
            italic: false,
            stretch: NORMAL_STRETCH,
        }
    }
}

/// The instance-independent half of a parsed face: bytes, coverage, and
/// the compiled shaping caches are IDENTICAL across a variable font's
/// named instances, so every instance shares one of these (DirectWrite's
/// model: instances enumerate separately, file state is shared).
struct SharedFace {
    data: FontData,
    /// Which face inside `data` — TrueType collections (.ttc) pack
    /// several; 0 for single-face files.
    face_index: u32,
    /// cmap materialized ONCE — fallback resolution is per-character and
    /// must never re-parse the font (Skia caches per-typeface the same way).
    charmap: HashMap<u32, u32>,
    /// HarfBuzz's compiled shaping caches (GSUB/GDEF/cmap), built once —
    /// the FontRef itself is cheap to reconstruct per shape (Skia's analog:
    /// HB faces cached per typeface).
    shaper_data: harfrust::ShaperData,
}

/// `FontUid` is the process-unique identity of one font instance.
///
/// Glyph caches may use it as a stable key because equal identifiers imply
/// equal outlines and variation coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FontUid(
    /// The process-unique numeric identity.
    pub u64,
);

impl FontUid {
    fn next() -> FontUid {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        FontUid(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// `Font` is one parsed font face or named variable-font instance.
///
/// It retains immutable source bytes and exposes the names, attributes,
/// coverage, and metrics needed for shaping and rendering.
pub struct Font {
    uid: FontUid,
    shared: Arc<SharedFace>,
    /// User-space variation coordinates when this font is a named instance
    /// of a variable font ((axis tag, value) per axis); empty = the
    /// file's default instance.
    variation_coordinates: Vec<([u8; 4], f32)>,
    /// The same coordinates in normalized variation space, for the skrifa
    /// views (metrics, COLR paint graphs).
    variation_location: skrifa::instance::Location,
    /// HarfBuzz's compiled per-instance data (None = default instance).
    shaper_instance: Option<harfrust::ShaperInstance>,
    family: String,
    /// Other names this face answers to: the file's localized family names
    /// (fontdb keeps all of them) plus any host-registered alias (Skia's
    /// `registerTypeface(typeface, familyName)`).
    aliases: Vec<String>,
    attrs: FontAttrs,
    units_per_em: f32,
    /// Font-unit metrics, y-up (ascent positive, descent negative).
    ascent: f32,
    descent: f32,
    line_gap: f32,
    /// Union of all glyph ink, font units y-up — Skia's fXMin/fXMax family.
    bounds: Option<(f32, f32, f32, f32)>,
    /// (offset from baseline, thickness) in font units, when the font says.
    underline: Option<(f32, f32)>,
    strikeout: Option<(f32, f32)>,
}

impl std::fmt::Debug for Font {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Font")
            .field("uid", &self.uid)
            .field("family", &self.family)
            .field("attrs", &self.attrs)
            .finish_non_exhaustive()
    }
}

impl Font {
    fn parse(
        family: &str,
        attrs: FontAttrs,
        data: FontData,
        face_index: u32,
        variation_coordinates: Vec<([u8; 4], f32)>,
    ) -> Option<Self> {
        let shared = SharedFace::parse(data, face_index)?;
        Self::at_coordinates(shared, family, attrs, variation_coordinates)
    }

    /// One instance over already-parsed shared state — metrics and shaping
    /// instance data are the only per-instance work.
    fn at_coordinates(
        shared: Arc<SharedFace>,
        family: &str,
        attrs: FontAttrs,
        variation_coordinates: Vec<([u8; 4], f32)>,
    ) -> Option<Self> {
        let bytes: &[u8] = (*shared.data).as_ref();
        let font = skrifa::FontRef::from_index(bytes, shared.face_index).ok()?;
        let variation_location = font.axes().location(
            variation_coordinates
                .iter()
                .map(|(tag, value)| (skrifa::Tag::new(tag), *value)),
        );
        let metrics = Metrics::new(&font, Size::unscaled(), &variation_location);
        let shaper_instance = (!variation_coordinates.is_empty()).then(|| {
            let harf = harfrust::FontRef::from_index(bytes, shared.face_index).ok();
            harf.map(|harf| {
                harfrust::ShaperInstance::from_variations(
                    &harf,
                    variation_coordinates
                        .iter()
                        .map(|(tag, value)| harfrust::Variation {
                            tag: harfrust::Tag::new(tag),
                            value: *value,
                        }),
                )
            })
        });
        Some(Self {
            uid: FontUid::next(),
            family: family.to_owned(),
            aliases: Vec::new(),
            attrs,
            units_per_em: metrics.units_per_em as f32,
            ascent: metrics.ascent,
            descent: metrics.descent,
            line_gap: metrics.leading,
            bounds: metrics.bounds.map(|b| (b.x_min, b.y_min, b.x_max, b.y_max)),
            underline: metrics.underline.map(|d| (d.offset, d.thickness)),
            strikeout: metrics.strikeout.map(|d| (d.offset, d.thickness)),
            shared,
            variation_coordinates,
            variation_location,
            shaper_instance: shaper_instance.flatten(),
        })
    }

    /// `data` returns the source font-file bytes.
    ///
    /// For a font collection file, use [`Self::face_index`] to select this face.
    pub fn data(&self) -> &[u8] {
        (*self.shared.data).as_ref()
    }

    /// `face_index` returns this face's index within [`Self::data`].
    pub fn face_index(&self) -> u32 {
        self.shared.face_index
    }

    /// `variation_coordinates` returns user-space axis tags and values.
    ///
    /// It is empty for static fonts and default variable-font instances.
    pub fn variation_coordinates(&self) -> &[([u8; 4], f32)] {
        &self.variation_coordinates
    }

    pub(crate) fn variation_location(&self) -> &skrifa::instance::Location {
        &self.variation_location
    }

    pub(crate) fn shaper_instance(&self) -> Option<&harfrust::ShaperInstance> {
        self.shaper_instance.as_ref()
    }

    /// `uid` returns the process-unique identity of this font instance.
    pub fn uid(&self) -> FontUid {
        self.uid
    }

    /// `family` returns the font's primary family name.
    pub fn family(&self) -> &str {
        &self.family
    }

    /// `aliases` returns additional family names recognized for this font.
    pub fn aliases(&self) -> &[String] {
        &self.aliases
    }

    /// `matches` reports whether `name` matches the family or an alias.
    ///
    /// Matching is ASCII case-insensitive.
    pub fn matches(&self, name: &str) -> bool {
        self.family.eq_ignore_ascii_case(name)
            || self.aliases.iter().any(|a| a.eq_ignore_ascii_case(name))
    }

    /// `add_alias` registers another family name if it is not already recognized.
    pub fn add_alias(&mut self, name: &str) {
        if !self.matches(name) {
            self.aliases.push(name.to_owned());
        }
    }

    /// `attrs` returns the face-selection attributes of this font.
    pub fn attrs(&self) -> FontAttrs {
        self.attrs
    }

    /// `ascent_px` returns the positive distance above the baseline at `size`.
    pub fn ascent_px(&self, size: f32) -> f32 {
        self.ascent * size / self.units_per_em
    }

    /// `descent_px` returns the positive distance below the baseline at `size`.
    pub fn descent_px(&self, size: f32) -> f32 {
        -self.descent * size / self.units_per_em
    }

    /// `line_height_px` returns the font's default line height at `size`.
    pub fn line_height_px(&self, size: f32) -> f32 {
        (self.ascent - self.descent + self.line_gap) * size / self.units_per_em
    }

    /// `units_per_em` returns the font's design-space units per em.
    pub fn units_per_em(&self) -> f32 {
        self.units_per_em
    }

    /// `ink_box_px` returns the font-wide ink bounds at `size`.
    ///
    /// The tuple is `(x_min, y_min, x_max, y_max)` in y-up coordinates around
    /// the glyph origin. It returns `None` when the font provides no bounds.
    pub fn ink_box_px(&self, size: f32) -> Option<(f32, f32, f32, f32)> {
        let k = size / self.units_per_em;
        self.bounds
            .map(|(x0, y0, x1, y1)| (x0 * k, y0 * k, x1 * k, y1 * k))
    }

    /// `covers` reports whether the font maps a character to a glyph.
    pub fn covers(&self, ch: char) -> bool {
        self.shared.charmap.contains_key(&(ch as u32))
    }

    /// `glyph_for` returns the glyph identifier mapped from a character.
    pub fn glyph_for(&self, ch: char) -> Option<u32> {
        self.shared.charmap.get(&(ch as u32)).copied()
    }

    pub(crate) fn shaper_data(&self) -> &harfrust::ShaperData {
        &self.shared.shaper_data
    }

    /// `underline_px` returns underline offset and thickness at `size`.
    ///
    /// The offset is positive below the baseline. Conventional values are used
    /// when the font omits underline metrics.
    pub fn underline_px(&self, size: f32) -> (f32, f32) {
        self.decoration_px(self.underline, size, -0.1, 0.05)
    }

    /// `strikeout_px` returns strikeout offset and thickness at `size`.
    ///
    /// The offset is positive above the baseline. Conventional values are used
    /// when the font omits strikeout metrics.
    pub fn strikeout_px(&self, size: f32) -> (f32, f32) {
        self.decoration_px(self.strikeout, size, 0.3, 0.05)
    }

    fn decoration_px(
        &self,
        metric: Option<(f32, f32)>,
        size: f32,
        default_offset: f32,
        default_thickness: f32,
    ) -> (f32, f32) {
        match metric {
            // Font units are y-up: positive offsets sit above the baseline.
            Some((offset, thickness)) => (
                offset * size / self.units_per_em,
                (thickness * size / self.units_per_em).max(0.5),
            ),
            None => (size * default_offset, (size * default_thickness).max(0.5)),
        }
    }
}

/// `FontDemand` describes font requests that no registered face could satisfy.
///
/// Hosts can use it to load additional fonts and lay out affected text again.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FontDemand {
    /// `families` lists requested family names with no registered match.
    pub families: Vec<String>,
    /// `codepoints` lists uncovered characters and their requested attributes.
    pub codepoints: Vec<(char, FontAttrs)>,
}

impl FontDemand {
    /// `is_empty` reports whether every font request was satisfied.
    pub fn is_empty(&self) -> bool {
        self.families.is_empty() && self.codepoints.is_empty()
    }

    pub(crate) fn add_family(&mut self, name: &str) {
        if !self.families.iter().any(|f| f == name) {
            self.families.push(name.to_owned());
        }
    }

    pub(crate) fn add_codepoint(&mut self, ch: char, attrs: FontAttrs) {
        if !self.codepoints.contains(&(ch, attrs)) {
            self.codepoints.push((ch, attrs));
        }
    }
}

/// `FontSource` locates fonts that are not already registered.
///
/// A source may consult installed fonts, downloaded assets, or another
/// host-owned repository. [`FontCollection`] consults sources in registration
/// order.
pub trait FontSource {
    /// `family` returns every available face matching a family name.
    fn family(&mut self, name: &str) -> Vec<Font>;

    /// `face_for_codepoint` returns a covering face nearest to `attrs`.
    fn face_for_codepoint(&mut self, codepoint: char, attrs: FontAttrs) -> Option<Font>;
}

/// `FaceSet` stores registered fonts and their global fallback order.
///
/// Cloning a face set shares parsed fonts, allowing callers to grow a snapshot
/// without modifying existing holders.
#[derive(Default, Clone)]
pub struct FaceSet {
    /// `Arc` per face: adding a font clones N pointers, never re-parses
    /// (Skia registers typefaces incrementally).
    fonts: Vec<Arc<Font>>,
    fallbacks: Vec<FontId>,
}

impl FaceSet {
    /// `new` creates an empty face set.
    pub fn new() -> Self {
        Self::default()
    }

    /// `register` adds font bytes under a family using default attributes.
    ///
    /// It returns `None` when face zero cannot be parsed.
    pub fn register(&mut self, family: &str, bytes: Vec<u8>) -> Option<FontId> {
        self.register_with(family, FontAttrs::default(), bytes)
    }

    /// `register_with` adds font bytes under a family with explicit attributes.
    ///
    /// It returns `None` when face zero cannot be parsed.
    pub fn register_with(
        &mut self,
        family: &str,
        attrs: FontAttrs,
        bytes: Vec<u8>,
    ) -> Option<FontId> {
        let data = unwrapped(Arc::new(bytes))?;
        let font = Font::parse(family, attrs, data, 0, Vec::new())?;
        Some(self.add(font))
    }

    /// `add` registers an already parsed font under its embedded names and attributes.
    pub fn add(&mut self, font: Font) -> FontId {
        self.fonts.push(Arc::new(font));
        FontId(self.fonts.len() as u32 - 1)
    }

    /// `with_font` returns a cloned face set containing one additional font.
    ///
    /// Existing fonts remain shared and are not reparsed.
    pub fn with_font(&self, font: Font) -> (FaceSet, FontId) {
        let mut next = self.clone();
        let id = next.add(font);
        (next, id)
    }

    /// `with_fallbacks` returns a clone with its global fallback order replaced.
    ///
    /// Every identifier must belong to this face set.
    pub fn with_fallbacks(&self, fallbacks: Vec<FontId>) -> FaceSet {
        let mut next = self.clone();
        next.fallbacks = fallbacks;
        next
    }

    /// `add_fallback` appends a registered font to the global fallback order.
    ///
    /// Requested families are searched before this chain. `id` must belong to
    /// this face set.
    pub fn add_fallback(&mut self, id: FontId) {
        self.fallbacks.push(id);
    }

    /// `grown_by` returns a clone extended with answers from a font source.
    ///
    /// Requested families are also registered under the requested name.
    /// Uncovered codepoints add matching faces to the fallback chain. It returns
    /// `None` when the source supplies nothing new.
    pub fn grown_by(&self, source: &mut dyn FontSource, demand: &FontDemand) -> Option<FaceSet> {
        let mut next = self.clone();
        let mut grew = false;
        for name in &demand.families {
            if next.family(name).is_some() {
                // Answered already (a stale demand) — resolution matches
                // it now; asking the source again would duplicate faces.
                continue;
            }
            grew |= next.register_answers(source.family(name), name);
        }
        for &(codepoint, attrs) in &demand.codepoints {
            grew |= next.register_fallback_answer(source, codepoint, attrs);
        }
        grew.then_some(next)
    }

    fn register_answers(&mut self, faces: Vec<Font>, requested_name: &str) -> bool {
        let mut added = false;
        for mut font in faces {
            font.add_alias(requested_name);
            self.add(font);
            added = true;
        }
        added
    }

    fn register_fallback_answer(
        &mut self,
        source: &mut dyn FontSource,
        codepoint: char,
        attrs: FontAttrs,
    ) -> bool {
        if is_private_use(codepoint) {
            // Icon fonts resolve by their REGISTERED family name; a
            // generic source "covering" the Private Use Area paints some
            // vendor's glyphs where tofu is the honest render.
            return false;
        }
        if self.covers_anywhere(codepoint) {
            // A family registered moments ago (or the host) already covers
            // it — resolution will find that face without a new fallback.
            return false;
        }
        let Some(font) = source.face_for_codepoint(codepoint, attrs) else {
            return false;
        };
        let id = self.add(font);
        self.add_fallback(id);
        true
    }

    fn covers_anywhere(&self, codepoint: char) -> bool {
        self.fonts.iter().any(|font| font.covers(codepoint))
    }

    /// `is_empty` reports whether no fonts are registered.
    pub fn is_empty(&self) -> bool {
        self.fonts.is_empty()
    }

    /// `len` returns the number of registered fonts.
    pub fn len(&self) -> usize {
        self.fonts.len()
    }

    /// `get_arc` returns a shared handle to a registered font.
    ///
    /// # Panics
    ///
    /// Panics if `id` does not belong to this face set.
    pub fn get_arc(&self, id: FontId) -> Arc<Font> {
        self.fonts[id.0 as usize].clone()
    }

    /// `get` returns a registered font by identifier.
    ///
    /// # Panics
    ///
    /// Panics if `id` does not belong to this face set.
    pub fn get(&self, id: FontId) -> &Font {
        &self.fonts[id.0 as usize]
    }

    /// `family` returns the first registered font matching a family name or alias.
    pub fn family(&self, name: &str) -> Option<FontId> {
        let at = self.fonts.iter().position(|f| f.matches(name))?;
        Some(FontId(at as u32))
    }

    /// `faces` iterates over every font matching a family name or alias.
    ///
    /// Results follow registration order and include separate subset faces.
    pub fn faces<'a>(&'a self, name: &'a str) -> impl Iterator<Item = FontId> + 'a {
        self.variants(name).map(|(id, _)| id)
    }

    /// `family_variant` returns the family face nearest to requested attributes.
    ///
    /// Width is matched first, then italic style and weight. Registration order
    /// breaks ties.
    pub fn family_variant(&self, name: &str, attrs: FontAttrs) -> Option<FontId> {
        self.nearest(self.variants(name), attrs)
    }

    /// `resolve` selects a font for a character and requested style.
    ///
    /// It searches requested families in order, then global fallbacks. If no
    /// font covers the character, it returns a font suitable for rendering
    /// `.notdef`.
    ///
    /// # Panics
    ///
    /// Panics when the face set is empty.
    pub fn resolve(&self, families: &[String], attrs: FontAttrs, ch: char) -> FontId {
        self.resolve_covered(families, attrs, ch).0
    }

    /// `resolve_covered` selects a font and reports whether it covers the character.
    ///
    /// A `false` flag means the returned font will render `.notdef`.
    ///
    /// # Panics
    ///
    /// Panics when the face set is empty.
    pub fn resolve_covered(
        &self,
        families: &[String],
        attrs: FontAttrs,
        ch: char,
    ) -> (FontId, bool) {
        for name in families {
            let covering = self.variants(name).filter(|(_, f)| f.covers(ch));
            if let Some(id) = self.nearest(covering, attrs) {
                return (id, true);
            }
        }
        let covering_fallbacks = self
            .fallbacks
            .iter()
            .map(|id| (*id, self.get(*id)))
            .filter(|(_, f)| f.covers(ch));
        if let Some(id) = self.nearest(covering_fallbacks, attrs) {
            return (id, true);
        }
        (self.tofu_face(families, attrs), false)
    }

    /// Every face answering to `name`, with its id.
    fn variants<'a>(&'a self, name: &'a str) -> impl Iterator<Item = (FontId, &'a Font)> {
        self.fonts
            .iter()
            .enumerate()
            .filter(move |(_, f)| f.matches(name))
            .map(|(at, f)| (FontId(at as u32), f.as_ref()))
    }

    /// CSS-style nearest among `faces`: width first, then matching style,
    /// then smallest weight distance, ties to the first registered. That is
    /// the CSS font-matching precedence; the distances themselves stay plain
    /// absolute differences rather than CSS's directional walk.
    fn nearest<'a>(
        &self,
        faces: impl Iterator<Item = (FontId, &'a Font)>,
        attrs: FontAttrs,
    ) -> Option<FontId> {
        faces
            .min_by_key(|(_, f)| {
                (
                    stretch_distance(f.attrs.stretch, attrs.stretch),
                    f.attrs.italic != attrs.italic,
                    f.attrs.weight.abs_diff(attrs.weight),
                )
            })
            .map(|(id, _)| id)
    }

    /// Nothing covers `ch`: the style's best variant, else the first
    /// fallback, else the first face — the tofu renders in SOMETHING.
    fn tofu_face(&self, families: &[String], attrs: FontAttrs) -> FontId {
        self.tofu_face_opt(families, attrs).unwrap_or_else(|| {
            panic!(
                "FontCollection has no fonts registered — register() one before building paragraphs"
            );
        })
    }

    /// [`Self::tofu_face`] that reports an EMPTY collection instead of
    /// panicking — the `build_with` path, where an empty start is valid
    /// (the chain is asked first; a char no source can render is skipped
    /// and reported through the demand).
    pub(crate) fn tofu_face_opt(&self, families: &[String], attrs: FontAttrs) -> Option<FontId> {
        families
            .iter()
            .find_map(|name| self.family_variant(name, attrs))
            .or_else(|| self.fallbacks.first().copied())
            .or_else(|| (!self.fonts.is_empty()).then_some(FontId(0)))
    }

    /// [`Self::resolve_covered`] that can also report "no face exists at
    /// all" (empty collection on the `build_with` path).
    pub(crate) fn resolve_covered_opt(
        &self,
        families: &[String],
        attrs: FontAttrs,
        ch: char,
    ) -> Option<(FontId, bool)> {
        if self.fonts.is_empty() {
            return None;
        }
        Some(self.resolve_covered(families, attrs, ch))
    }
}

impl Font {
    /// `from_bytes` parses face zero from owned font-file bytes.
    ///
    /// Family names and attributes come from the font. Localized family names
    /// become aliases. It returns `None` when the bytes cannot be parsed.
    pub fn from_bytes(bytes: Vec<u8>) -> Option<Font> {
        Self::from_data(Arc::new(bytes), 0)
    }

    /// `from_data` parses one face from shared font-file storage.
    ///
    /// Use face index zero for a single-face file. It returns `None` when the
    /// index or font data is invalid.
    pub fn from_data(data: FontData, face_index: u32) -> Option<Font> {
        let data = unwrapped(data)?;
        Self::instance(data, face_index, Vec::new())
    }

    /// `instances_from_data` parses the registrable instances of one face.
    ///
    /// Static fonts produce one item. Variable fonts with named instances
    /// produce one [`Font`] per instance. Invalid data returns an empty vector.
    pub fn instances_from_data(data: FontData, face_index: u32) -> Vec<Font> {
        let Some(data) = unwrapped(data) else {
            return Vec::new();
        };
        let instances = named_instance_coordinates((*data).as_ref(), face_index);
        if instances.is_empty() {
            return Self::instance(data, face_index, Vec::new())
                .into_iter()
                .collect();
        }
        // The expensive halves (cmap, shaping caches) parse ONCE; each
        // instance adds only its metrics and axis data.
        let Some(shared) = SharedFace::parse(data, face_index) else {
            return Vec::new();
        };
        instances
            .into_iter()
            .filter_map(|coordinates| Self::shared_instance(shared.clone(), coordinates))
            .collect()
    }

    /// One face at one set of coordinates, self-described from its tables
    /// (attrs overridden by the coordinates' weight/italic axes).
    fn instance(data: FontData, face_index: u32, coordinates: Vec<([u8; 4], f32)>) -> Option<Font> {
        let shared = SharedFace::parse(data, face_index)?;
        Self::shared_instance(shared, coordinates)
    }

    /// [`Self::instance`] over already-shared file state.
    fn shared_instance(shared: Arc<SharedFace>, coordinates: Vec<([u8; 4], f32)>) -> Option<Font> {
        let bytes: &[u8] = (*shared.data).as_ref();
        let (family, aliases) = embedded_names(bytes, shared.face_index)?;
        let attrs = instance_attrs(embedded_attrs(bytes, shared.face_index), &coordinates);
        let mut font = Font::at_coordinates(shared, &family, attrs, coordinates)?;
        for name in &aliases {
            font.add_alias(name);
        }
        Some(font)
    }
}

impl SharedFace {
    fn parse(data: FontData, face_index: u32) -> Option<Arc<Self>> {
        let bytes: &[u8] = (*data).as_ref();
        let font = skrifa::FontRef::from_index(bytes, face_index).ok()?;
        let charmap = font
            .charmap()
            .mappings()
            .map(|(code, glyph)| (code, glyph.to_u32()))
            .collect();
        let harf = harfrust::FontRef::from_index(bytes, face_index).ok()?;
        let shaper_data = harfrust::ShaperData::new(&harf);
        Some(Arc::new(Self {
            data,
            face_index,
            charmap,
            shaper_data,
        }))
    }
}

/// The (tag, value) coordinate rows of every fvar named instance.
fn named_instance_coordinates(bytes: &[u8], face_index: u32) -> Vec<Vec<([u8; 4], f32)>> {
    let Ok(font) = skrifa::FontRef::from_index(bytes, face_index) else {
        return Vec::new();
    };
    let axis_tags: Vec<[u8; 4]> = font
        .axes()
        .iter()
        .map(|axis| axis.tag().to_be_bytes())
        .collect();
    font.named_instances()
        .iter()
        .map(|instance| {
            axis_tags
                .iter()
                .copied()
                .zip(instance.user_coords())
                .collect()
        })
        .collect()
}

/// A named instance's place in its family: the weight/italic/width axes
/// override what the file's default-instance OS/2 table says.
fn instance_attrs(base: FontAttrs, coordinates: &[([u8; 4], f32)]) -> FontAttrs {
    let mut attrs = base;
    for (tag, value) in coordinates {
        match tag {
            b"wght" => attrs.weight = value.clamp(1.0, 1000.0) as u16,
            b"ital" => attrs.italic = *value >= 0.5,
            b"slnt" => attrs.italic = attrs.italic || *value < 0.0,
            // `wdth` is already a percentage, which is what CSS asks for.
            b"wdth" => attrs.stretch = value.clamp(1.0, 1000.0),
            _ => {}
        }
    }
    attrs
}

/// Ordered width distance for variant matching. Quantized to 1/16 of a
/// percent so it can be an integer sort key without collapsing the named
/// widths (they are 12.5 apart at the closest).
fn stretch_distance(candidate: f32, wanted: f32) -> u32 {
    ((candidate - wanted).abs() * 16.0) as u32
}

/// WOFF2 arrives brotli-wrapped; faces parse the unwrapped TrueType bytes
/// (icon and web fonts ship compressed — the CoreText managers accept
/// them, so registration here does too). Identity for everything else.
#[cfg(feature = "woff2")]
fn unwrapped(data: FontData) -> Option<FontData> {
    let bytes: &[u8] = (*data).as_ref();
    if !woff2_patched::decode::is_woff2(bytes) {
        return Some(data);
    }
    let unpacked = woff2_patched::decode::convert_woff2_to_ttf(&mut &bytes[..]).ok()?;
    Some(Arc::new(unpacked))
}

#[cfg(not(feature = "woff2"))]
fn unwrapped(data: FontData) -> Option<FontData> {
    Some(data)
}

/// name table: the primary is the en typographic family (then en family,
/// then any-language); every other family/typographic-family string rides
/// along as an alias.
fn embedded_names(data: &[u8], face_index: u32) -> Option<(String, Vec<String>)> {
    use swash::StringId;
    let font = swash::FontRef::from_index(data, face_index as usize)?;
    let strings = font.localized_strings();
    let pick = |id: StringId| {
        strings
            .find_by_id(id, Some("en"))
            .or_else(|| strings.find_by_id(id, None))
            .map(|s| s.to_string())
    };
    let primary = pick(StringId::TypographicFamily).or_else(|| pick(StringId::Family))?;
    let aliases = strings
        .filter(|s| matches!(s.id(), StringId::Family | StringId::TypographicFamily))
        .map(|s| s.to_string())
        .filter(|name| *name != primary)
        .collect();
    Some((primary, aliases))
}

/// OS/2 weight + width + style flags via swash attributes.
fn embedded_attrs(data: &[u8], face_index: u32) -> FontAttrs {
    let Some(font) = swash::FontRef::from_index(data, face_index as usize) else {
        return FontAttrs::default();
    };
    let attrs = font.attributes();
    FontAttrs {
        weight: attrs.weight().0,
        italic: attrs.style() != swash::Style::Normal,
        stretch: attrs.stretch().to_percentage(),
    }
}

/// Unicode Private Use Areas — codepoints whose meaning belongs to a
/// specific registered font, never to a generic fallback source.
fn is_private_use(codepoint: char) -> bool {
    matches!(
        codepoint,
        '\u{E000}'..='\u{F8FF}' | '\u{F0000}'..='\u{FFFFD}' | '\u{100000}'..='\u{10FFFD}'
    )
}

/// `FontCollection` owns registered faces and sources for resolving missing fonts.
///
/// Paragraph building consults sources in order and registers their answers.
/// Unanswered requests accumulate until [`Self::take_unanswered`] is called.
#[derive(Default)]
pub struct FontCollection {
    faces: FaceSet,
    /// Consulted in order, first answer wins (Skia's manager priority:
    /// registered bytes and downloaders before the platform database).
    sources: Vec<Box<dyn FontSource>>,
    /// Misses no source answered, since the last drain.
    unanswered: FontDemand,
}

impl FontCollection {
    /// `new` creates an empty collection with no font sources.
    pub fn new() -> FontCollection {
        FontCollection::default()
    }

    /// `faces` returns the faces currently registered in the collection.
    ///
    /// A built paragraph clones this set and remains independent of later changes.
    pub fn faces(&self) -> &FaceSet {
        &self.faces
    }

    /// `register` adds font bytes under a family using default attributes.
    ///
    /// It returns `None` when face zero cannot be parsed.
    pub fn register(&mut self, family: &str, bytes: Vec<u8>) -> Option<FontId> {
        self.faces.register(family, bytes)
    }

    /// `add` registers an already parsed font.
    pub fn add(&mut self, font: Font) -> FontId {
        self.faces.add(font)
    }

    /// `add_fallback` appends a registered font to the global fallback order.
    ///
    /// `id` must belong to this collection.
    pub fn add_fallback(&mut self, id: FontId) {
        self.faces.add_fallback(id);
    }

    /// `get` returns a registered font by identifier.
    ///
    /// # Panics
    ///
    /// Panics if `id` does not belong to this collection.
    pub fn get(&self, id: FontId) -> &Font {
        self.faces.get(id)
    }

    /// `len` returns the number of registered fonts.
    pub fn len(&self) -> usize {
        self.faces.len()
    }

    /// `family` returns the first font matching a family name or alias.
    pub fn family(&self, name: &str) -> Option<FontId> {
        self.faces.family(name)
    }

    /// `add_source` appends a source consulted when registered fonts cannot satisfy a request.
    pub fn add_source(&mut self, source: impl FontSource + 'static) {
        self.sources.push(Box::new(source));
    }

    /// `add_boxed_source` appends an already boxed font source.
    pub fn add_boxed_source(&mut self, source: Box<dyn FontSource>) {
        self.sources.push(source);
    }

    /// `is_empty` reports whether no fonts are currently registered.
    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }

    /// `adopt_faces` replaces the registered faces with a prepared set.
    ///
    /// This supports hosts that answer [`FontDemand`] using [`FaceSet::grown_by`].
    pub fn adopt_faces(&mut self, faces: FaceSet) {
        self.faces = faces;
    }

    /// `take_unanswered` drains font requests that no source could satisfy.
    ///
    /// Hosts may load and [`Self::register`] matching fonts before rebuilding
    /// affected paragraphs.
    pub fn take_unanswered(&mut self) -> FontDemand {
        std::mem::take(&mut self.unanswered)
    }

    /// Resolution with growth: look up, else ask the sources in order,
    /// registering what they answer (Skia's `findTypefaces` walking its
    /// managers). `false` = nobody had it; the miss is recorded.
    pub(crate) fn require_family(&mut self, name: &str) -> bool {
        if self.faces.family(name).is_some() {
            return true;
        }
        for source in &mut self.sources {
            let faces = source.family(name);
            if self.faces.register_answers(faces, name) {
                return true;
            }
        }
        self.unanswered.add_family(name);
        false
    }

    /// The per-codepoint half (Skia's `defaultFallback(unicode, ..)`).
    pub(crate) fn require_codepoint(&mut self, codepoint: char, attrs: FontAttrs) -> bool {
        if self.faces.covers_anywhere(codepoint) {
            return true;
        }
        for index in 0..self.sources.len() {
            let (head, tail) = self.sources.split_at_mut(index);
            let _ = head;
            let source = &mut tail[0];
            if self
                .faces
                .register_fallback_answer(source.as_mut(), codepoint, attrs)
            {
                return true;
            }
        }
        self.unanswered.add_codepoint(codepoint, attrs);
        false
    }
}
