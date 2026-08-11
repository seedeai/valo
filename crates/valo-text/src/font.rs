use std::collections::HashMap;
use std::sync::Arc;

use skrifa::metrics::Metrics;
use skrifa::prelude::Size;
use skrifa::MetadataProvider;

/// Index into a [`FontCollection`] — stable for the collection's lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FontId(pub u32);

/// Font file bytes however the host owns them: an owned buffer, or a
/// memory-mapped file from a system-font source — faces only ever read.
pub type FontData = Arc<dyn AsRef<[u8]> + Send + Sync>;

/// One registered font: immutable bytes + the metrics every layout needs.
/// Shaping (harfrust), outlines (skrifa), and raster (swash) all re-read
/// the same bytes — no parsed state is shared across those seams.
/// A variant's place in its family — CSS-style matching picks by these.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontAttrs {
    /// CSS weight, 100–900.
    pub weight: u16,
    pub italic: bool,
}

impl Default for FontAttrs {
    fn default() -> Self {
        Self {
            weight: 400,
            italic: false,
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

/// Stable raster identity of one font INSTANCE (glyph caches key on it:
/// same uid = same outlines). Assigned at instance construction from a
/// process counter — Skia's typeface uniqueID role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FontUid(pub u64);

impl FontUid {
    fn next() -> FontUid {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        FontUid(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

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

    /// The raw file bytes — possibly a whole .ttc collection; pair every
    /// face view with [`Self::face_index`].
    pub fn data(&self) -> &[u8] {
        (*self.shared.data).as_ref()
    }

    /// Which face of [`Self::data`] this font is.
    pub fn face_index(&self) -> u32 {
        self.shared.face_index
    }

    /// User-space variation coordinates ((axis tag, value)); empty for the
    /// default instance and for static fonts.
    pub fn variation_coordinates(&self) -> &[([u8; 4], f32)] {
        &self.variation_coordinates
    }

    pub(crate) fn variation_location(&self) -> &skrifa::instance::Location {
        &self.variation_location
    }

    pub(crate) fn shaper_instance(&self) -> Option<&harfrust::ShaperInstance> {
        self.shaper_instance.as_ref()
    }

    /// The instance's stable raster identity.
    pub fn uid(&self) -> FontUid {
        self.uid
    }

    pub fn family(&self) -> &str {
        &self.family
    }

    pub fn aliases(&self) -> &[String] {
        &self.aliases
    }

    /// Does this face answer to `name`? (its family or any alias —
    /// ASCII-case-insensitively, the CSS and platform-manager behavior)
    pub fn matches(&self, name: &str) -> bool {
        self.family.eq_ignore_ascii_case(name)
            || self.aliases.iter().any(|a| a.eq_ignore_ascii_case(name))
    }

    /// Register one more name for this face (no-op if already answered).
    pub fn add_alias(&mut self, name: &str) {
        if !self.matches(name) {
            self.aliases.push(name.to_owned());
        }
    }

    pub fn attrs(&self) -> FontAttrs {
        self.attrs
    }

    /// Ascent in px at `size` (positive, above the baseline).
    pub fn ascent_px(&self, size: f32) -> f32 {
        self.ascent * size / self.units_per_em
    }

    /// Descent in px at `size` (positive, below the baseline).
    pub fn descent_px(&self, size: f32) -> f32 {
        -self.descent * size / self.units_per_em
    }

    /// Default line height in px at `size`.
    pub fn line_height_px(&self, size: f32) -> f32 {
        (self.ascent - self.descent + self.line_gap) * size / self.units_per_em
    }

    pub fn units_per_em(&self) -> f32 {
        self.units_per_em
    }

    /// The font-wide ink box at `size`, y-UP around the glyph origin:
    /// (x_min, y_min, x_max, y_max). Any glyph's ink fits inside — the
    /// cheap way to bound italic overhang and mark excursions per run.
    pub fn ink_box_px(&self, size: f32) -> Option<(f32, f32, f32, f32)> {
        let k = size / self.units_per_em;
        self.bounds
            .map(|(x0, y0, x1, y1)| (x0 * k, y0 * k, x1 * k, y1 * k))
    }

    pub fn covers(&self, ch: char) -> bool {
        self.shared.charmap.contains_key(&(ch as u32))
    }

    pub fn glyph_for(&self, ch: char) -> Option<u32> {
        self.shared.charmap.get(&(ch as u32)).copied()
    }

    pub(crate) fn shaper_data(&self) -> &harfrust::ShaperData {
        &self.shared.shaper_data
    }

    /// Underline (offset below baseline, thickness) in px at `size`, with
    /// conventional fallbacks when the font omits the post-table values.
    pub fn underline_px(&self, size: f32) -> (f32, f32) {
        self.decoration_px(self.underline, size, -0.1, 0.05)
    }

    /// Strikeout (offset ABOVE baseline, thickness) in px at `size`.
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

/// What shaping could not resolve — the host's font-loading demand signal
/// (Flutter web's missing-font detection reshaped as an API).
/// `families`: requested names the collection has NO face for at all.
/// `codepoints`: chars NO present face covers, each with the attrs of the
/// span that wanted it — a bold span's missing glyph should be answered
/// with a bold face (for subset-chunk families this doubles as "which
/// chunk is missing"). valo detects; the host owns the loading policy —
/// Google, a mirror, bundled files, the OS, anything.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FontDemand {
    pub families: Vec<String>,
    pub codepoints: Vec<(char, FontAttrs)>,
}

impl FontDemand {
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

/// Somewhere fonts can come FROM when the collection lacks them — the
/// pluggable half of the demand loop. Implementations only
/// locate and parse (an installed-fonts scan today; a platform-native
/// CoreText/DirectWrite lookup can replace it without touching policy);
/// the growth policy itself lives in [`FontCollection::grown_by`].
pub trait FontSource {
    /// Every face answering to `name` (all weights and styles — the
    /// collection's nearest-variant matching picks per span).
    fn family(&mut self, name: &str) -> Vec<Font>;

    /// One face covering `codepoint`, nearest to `attrs`.
    fn face_for_codepoint(&mut self, codepoint: char, attrs: FontAttrs) -> Option<Font>;
}

/// The host's registered fonts: families for styles to name, plus a global
/// fallback chain consulted per character. Immutable once built —
/// register everything, then `Arc` it for builders and the renderer.
#[derive(Default, Clone)]
pub struct FaceSet {
    /// `Arc` per face: adding a font clones N pointers, never re-parses
    /// (Skia registers typefaces incrementally).
    fonts: Vec<Arc<Font>>,
    fallbacks: Vec<FontId>,
}

impl FaceSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register font bytes under a family name (regular weight/style).
    /// Returns `None` when the bytes don't parse as a font.
    pub fn register(&mut self, family: &str, bytes: Vec<u8>) -> Option<FontId> {
        self.register_with(family, FontAttrs::default(), bytes)
    }

    /// Register one variant of a family — `resolve` picks the nearest
    /// weight with a matching style (CSS §5.2, simplified: style first,
    /// then minimal weight distance, ties to the first registered).
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

    /// Add an already-parsed [`Font`] under ITS OWN name/attrs — the
    /// `SkTypeface` → `registerTypeface` shape.
    pub fn add(&mut self, font: Font) -> FontId {
        self.fonts.push(Arc::new(font));
        FontId(self.fonts.len() as u32 - 1)
    }

    /// A new collection = this one + `font`. Faces are shared by `Arc`, so
    /// this is O(faces) pointer clones — no re-parsing, and every existing
    /// holder of the old collection is untouched.
    pub fn with_font(&self, font: Font) -> (FaceSet, FontId) {
        let mut next = self.clone();
        let id = next.add(font);
        (next, id)
    }

    /// A new collection with the fallback chain REPLACED (order matters).
    pub fn with_fallbacks(&self, fallbacks: Vec<FontId>) -> FaceSet {
        let mut next = self.clone();
        next.fallbacks = fallbacks;
        next
    }

    /// Append to the global fallback chain (consulted after a style's own
    /// families: nearest attrs among the faces covering the character,
    /// ties in chain order).
    pub fn add_fallback(&mut self, id: FontId) {
        self.fallbacks.push(id);
    }

    /// Grow this collection to answer `demand` from `source`: demanded
    /// families register under their own names PLUS the demanded name as
    /// an alias (a localized or differently-spelled request must match on
    /// the next layout, or a loop around this call could demand forever);
    /// codepoints still uncovered afterwards extend the fallback chain
    /// with a face matching the demanding span's attrs. `Some(grown)`
    /// only when something new was found — the caller's signal to
    /// re-register the collection and lay out again.
    /// Grow a COPY of this face set to answer `demand` from `source` — the
    /// out-of-band path (a host that already knows what it wants). Live
    /// resolution goes through [`FontCollection`], which owns its sources.
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

    /// True until the first `add`/`register` — building paragraphs against
    /// an empty collection is a contract violation (`resolve` asserts).
    pub fn is_empty(&self) -> bool {
        self.fonts.is_empty()
    }

    /// Faces registered so far. Ids are append-only, so a holder of an older
    /// collection can name the faces added since: `old.len()..new.len()`.
    pub fn len(&self) -> usize {
        self.fonts.len()
    }

    /// The shared instance behind `id` — what glyph runs carry to the
    /// renderer (Skia: blobs hold `sk_sp<SkTypeface>`).
    pub fn get_arc(&self, id: FontId) -> Arc<Font> {
        self.fonts[id.0 as usize].clone()
    }

    pub fn get(&self, id: FontId) -> &Font {
        &self.fonts[id.0 as usize]
    }

    pub fn family(&self, name: &str) -> Option<FontId> {
        let at = self.fonts.iter().position(|f| f.matches(name))?;
        Some(FontId(at as u32))
    }

    /// Ids of EVERY face answering to `name`, in registration order. Subset
    /// families (css2/cn-font-split unicode-range chunks) register many
    /// faces under one name with disjoint coverage — a fallback chain built
    /// from [`Self::family`] alone reaches only the first-loaded chunk, so
    /// hosts expand fallback names with this.
    pub fn faces<'a>(&'a self, name: &'a str) -> impl Iterator<Item = FontId> + 'a {
        self.variants(name).map(|(id, _)| id)
    }

    /// The family variant nearest `attrs`: matching style wins, then the
    /// smallest weight distance (ties to the first registered).
    pub fn family_variant(&self, name: &str, attrs: FontAttrs) -> Option<FontId> {
        self.nearest(self.variants(name), attrs)
    }

    /// The font that renders `ch` for a style: per requested family, the
    /// nearest variant that COVERS `ch` — subset families (cn-font-split
    /// chunks) carry one unicode range per face, so coverage must look past
    /// the best-attrs face. Then the fallback chain, else the first
    /// candidate.
    pub fn resolve(&self, families: &[String], attrs: FontAttrs, ch: char) -> FontId {
        self.resolve_covered(families, attrs, ch).0
    }

    /// [`Self::resolve`] plus whether ANYTHING actually covers `ch` — false
    /// means the returned face will shape `.notdef`. The demand signal:
    /// callers report uncovered chars to the host, which
    /// decides where fonts come from — valo only detects.
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

    /// CSS-style nearest among `faces`: matching style first, then smallest
    /// weight distance, ties to the first registered.
    fn nearest<'a>(
        &self,
        faces: impl Iterator<Item = (FontId, &'a Font)>,
        attrs: FontAttrs,
    ) -> Option<FontId> {
        faces
            .min_by_key(|(_, f)| {
                (
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
    /// Parse a font file into a queryable object: family and weight/style
    /// come from the file's own tables (Skia's `SkTypeface::MakeFromData`
    /// shape — parse once, inspect, then `FontCollection::add`). Every
    /// localized family name becomes an alias (fontdb keeps them all —
    /// documents reference `优设标题黑` as readily as `YouSheBiaoTiHei`).
    pub fn from_bytes(bytes: Vec<u8>) -> Option<Font> {
        Self::from_data(Arc::new(bytes), 0)
    }

    /// [`Self::from_bytes`] for shared or memory-mapped bytes and for
    /// collection files: `face_index` picks the face inside a .ttc (0 for
    /// single-face files).
    pub fn from_data(data: FontData, face_index: u32) -> Option<Font> {
        let data = unwrapped(data)?;
        Self::instance(data, face_index, Vec::new())
    }

    /// Every face a file offers for registration: a static font is itself;
    /// a variable font is its NAMED INSTANCES (fvar), each a [`Font`] with
    /// the instance's attrs and coordinates — nearest-variant matching
    /// then picks weights exactly like a static multi-weight family.
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

/// A named instance's place in its family: the weight/italic axes override
/// what the file's default-instance OS/2 table says.
fn instance_attrs(base: FontAttrs, coordinates: &[([u8; 4], f32)]) -> FontAttrs {
    let mut attrs = base;
    for (tag, value) in coordinates {
        match tag {
            b"wght" => attrs.weight = value.clamp(1.0, 1000.0) as u16,
            b"ital" => attrs.italic = *value >= 0.5,
            b"slnt" => attrs.italic = attrs.italic || *value < 0.0,
            _ => {}
        }
    }
    attrs
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

/// OS/2 weight + style flags via swash attributes.
fn embedded_attrs(data: &[u8], face_index: u32) -> FontAttrs {
    let Some(font) = swash::FontRef::from_index(data, face_index as usize) else {
        return FontAttrs::default();
    };
    let attrs = font.attributes();
    FontAttrs {
        weight: attrs.weight().0,
        italic: attrs.style() != swash::Style::Normal,
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

/// Faces plus the sources that can find more — Skia's `FontCollection`
/// (skparagraph FontCollection.h: the asset/dynamic/default `SkFontMgr`s
/// live INSIDE the collection, and `findTypefaces`/`defaultFallback` are
/// its methods). Shaping consults this at every miss; what no source can
/// answer accumulates as the [`demand`](Self::take_unanswered) a host
/// fetches asynchronously (Flutter web's `_unprocessedCodePoints`).
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
    pub fn new() -> FontCollection {
        FontCollection::default()
    }

    /// The faces resolved so far — what a built paragraph snapshots.
    pub fn faces(&self) -> &FaceSet {
        &self.faces
    }

    /// Registers bytes under a family (Skia `registerTypeface`; Flutter
    /// `FontLoader.load`). Host-facing: the other half of the async loop.
    pub fn register(&mut self, family: &str, bytes: Vec<u8>) -> Option<FontId> {
        self.faces.register(family, bytes)
    }

    pub fn add(&mut self, font: Font) -> FontId {
        self.faces.add(font)
    }

    pub fn add_fallback(&mut self, id: FontId) {
        self.faces.add_fallback(id);
    }

    pub fn get(&self, id: FontId) -> &Font {
        self.faces.get(id)
    }

    pub fn len(&self) -> usize {
        self.faces.len()
    }

    pub fn family(&self, name: &str) -> Option<FontId> {
        self.faces.family(name)
    }

    /// Adds a source consulted on a miss: the OS database, a downloader's
    /// already-fetched cache, anything.
    pub fn add_source(&mut self, source: impl FontSource + 'static) {
        self.sources.push(Box::new(source));
    }

    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }

    /// Replaces the faces with a set grown out of band (a host that
    /// answered a demand itself — [`FaceSet::grown_by`]).
    pub fn adopt_faces(&mut self, faces: FaceSet) {
        self.faces = faces;
    }

    /// Takes the misses no source could answer — the host's cue to fetch
    /// (and later [`register`](Self::register), which invalidates the text
    /// that wanted them). Draining is the caller's; nothing here is async.
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
