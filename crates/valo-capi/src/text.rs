//! Text: the font collection, paragraph building (one style per span —
//! embedders with style STACKS resolve them before calling), layout, and
//! the caret/selection query surface. Byte offsets index the paragraph's
//! UTF-8 text.

use std::sync::Arc;

use valo::{
    Decoration, DecorationKind, Font, FontCollection, FontId, Paragraph, ParagraphBuilder,
    ParagraphStyle, TextAlign, TextStyle,
};

use crate::{borrow, borrow_mut, dispose_handle, into_handle, ValoColor, ValoRect};

pub struct ValoFontCollection {
    pub(crate) collection: FontCollection,
}

/// The builder's INPUTS, accumulated across C calls. The real
/// `ParagraphBuilder` borrows a collection, which a C handle cannot hold,
/// so the borrow is taken for the duration of `build` instead.
pub struct ValoParagraphBuilder {
    style: valo::ParagraphStyle,
    spans: Vec<(String, valo::TextStyle)>,
}

pub struct ValoParagraph {
    pub(crate) paragraph: Paragraph,
}

/// One span's style, by value. `families_utf8` is a NEWLINE-separated
/// family list, tried in order per glyph (newlines can't occur in family
/// names). `line_height <= 0` uses the font's own metrics.
/// `decoration_kind`: -1 none, 0 underline, 1 line-through, 2 overline;
/// `decoration_color.alpha <= 0` inherits the text color.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ValoTextStyle {
    pub families_utf8: *const u8,
    pub families_length: usize,
    pub size: f32,
    /// CSS weight, 100–900.
    pub weight: u32,
    pub italic: bool,
    pub color: ValoColor,
    pub letter_spacing: f32,
    pub word_spacing: f32,
    pub line_height: f32,
    pub decoration_kind: i32,
    pub decoration_color: ValoColor,
    pub decoration_thickness: f32,
}

/// `align`: 0 left, 1 center, 2 right, 3 justify. `max_lines` 0 =
/// unlimited. A null/empty ellipsis truncates without one.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ValoParagraphStyle {
    pub align: i32,
    pub max_lines: u32,
    pub ellipsis_utf8: *const u8,
    pub ellipsis_length: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ValoTextRange {
    pub start: usize,
    pub end: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ValoLineMetrics {
    pub start: usize,
    pub end: usize,
    pub baseline: f32,
    pub ascent: f32,
    pub descent: f32,
    pub left: f32,
    pub width: f32,
}

// ── fonts ───────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn valo_fonts_new() -> *mut ValoFontCollection {
    into_handle(ValoFontCollection {
        collection: FontCollection::default(),
    })
}

/// # Safety
/// `fonts` must be a live [`valo_fonts_new`] handle (or null).
#[no_mangle]
pub unsafe extern "C" fn valo_fonts_dispose(fonts: *mut ValoFontCollection) {
    unsafe { dispose_handle(fonts) }
}

/// Register a face from font bytes (TTF/OTF). Returns the face id, or -1
/// when the bytes don't parse. Paragraphs built from earlier snapshots of
/// the collection are unaffected — rebuild them to see new faces.
///
/// # Safety
/// `fonts` must be a live handle; `bytes` must point to `length` readable
/// bytes.
#[no_mangle]
pub unsafe extern "C" fn valo_fonts_add(
    fonts: *mut ValoFontCollection,
    bytes: *const u8,
    length: usize,
) -> i64 {
    let Some(handle) = (unsafe { borrow_mut(fonts) }) else {
        return -1;
    };
    if bytes.is_null() || length == 0 {
        return -1;
    }
    let data = unsafe { std::slice::from_raw_parts(bytes, length) }.to_vec();
    let Some(font) = valo::Font::from_bytes(data) else {
        return -1;
    };
    let id = handle.collection.add(font);
    id.0 as i64
}

/// Append a registered face to the fallback chain (codepoints no styled
/// family covers try the chain in order).
///
/// # Safety
/// `fonts` must be a live handle (or null, a no-op).
#[no_mangle]
pub unsafe extern "C" fn valo_fonts_add_fallback(fonts: *mut ValoFontCollection, face_id: i64) {
    let Some(handle) = (unsafe { borrow_mut(fonts) }) else {
        return;
    };
    if face_id < 0 {
        return;
    }

    handle.collection.add_fallback(FontId(face_id as u32));
}

/// Register every face a font file offers: a static font is one face; a
/// variable font is its NAMED INSTANCES, so weights and styles select
/// like a static multi-weight family. `add_as_fallbacks` also appends
/// each face to the fallback chain (default UI fonts). Returns the number
/// of faces added; 0 when the bytes don't parse.
///
/// # Safety
/// `fonts` must be a live handle; `bytes` must point to `length` readable
/// bytes.
#[no_mangle]
pub unsafe extern "C" fn valo_fonts_add_instances(
    fonts: *mut ValoFontCollection,
    bytes: *const u8,
    length: usize,
    add_as_fallbacks: bool,
) -> i32 {
    let Some(handle) = (unsafe { borrow_mut(fonts) }) else {
        return 0;
    };
    if bytes.is_null() || length == 0 {
        return 0;
    }
    let data = unsafe { std::slice::from_raw_parts(bytes, length) }.to_vec();
    let instances = Font::instances_from_data(Arc::new(data), 0);
    if instances.is_empty() {
        return 0;
    }
    let count = instances.len() as i32;

    for font in instances {
        let id = handle.collection.add(font);
        if add_as_fallbacks {
            handle.collection.add_fallback(id);
        }
    }

    count
}

/// The family name of a registered face: writes up to `capacity` UTF-8
/// bytes and returns the TOTAL length (call with capacity 0 to size);
/// 0 for an unknown id.
///
/// # Safety
/// `fonts` must be a live handle; `out_utf8` must point to `capacity`
/// writable bytes (or be null with capacity 0).
#[no_mangle]
pub unsafe extern "C" fn valo_fonts_family_name(
    fonts: *const ValoFontCollection,
    face_id: i64,
    out_utf8: *mut u8,
    capacity: usize,
) -> usize {
    let Some(handle) = (unsafe { borrow(fonts) }) else {
        return 0;
    };
    if face_id < 0 || face_id as usize >= handle.collection.len() {
        return 0;
    }
    let family = handle.collection.get(FontId(face_id as u32)).family();
    let bytes = family.as_bytes();
    if !out_utf8.is_null() {
        let written = bytes.len().min(capacity);
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out_utf8, written) };
    }
    bytes.len()
}

// ── paragraph building ──────────────────────────────────────────────────

/// # Safety
/// `fonts` must be a live handle (or null → null).
#[no_mangle]
pub unsafe extern "C" fn valo_paragraph_builder_new(
    fonts: *const ValoFontCollection,
    style: ValoParagraphStyle,
) -> *mut ValoParagraphBuilder {
    let Some(fonts) = (unsafe { borrow(fonts) }) else {
        return std::ptr::null_mut();
    };
    let _ = fonts;
    into_handle(ValoParagraphBuilder {
        style: paragraph_style(&style),
        spans: Vec::new(),
    })
}

/// # Safety
/// `builder` must be a live handle not yet built (or null).
#[no_mangle]
pub unsafe extern "C" fn valo_paragraph_builder_dispose(builder: *mut ValoParagraphBuilder) {
    unsafe { dispose_handle(builder) }
}

/// Append one styled span of UTF-8 text.
///
/// # Safety
/// `builder` must be a live handle; `text_utf8` must point to
/// `text_length` readable bytes of UTF-8.
#[no_mangle]
pub unsafe extern "C" fn valo_paragraph_builder_add_text(
    builder: *mut ValoParagraphBuilder,
    text_utf8: *const u8,
    text_length: usize,
    style: ValoTextStyle,
) {
    let Some(handle) = (unsafe { borrow_mut(builder) }) else {
        return;
    };
    let Some(text) = (unsafe { utf8(text_utf8, text_length) }) else {
        return;
    };
    handle.spans.push((text.to_owned(), text_style(&style)));
}

/// Finish building: consumes the builder handle; the paragraph needs
/// [`valo_paragraph_layout`] before drawing or querying.
///
/// # Safety
/// `builder` must be a live handle (or null → null).
#[no_mangle]
pub unsafe extern "C" fn valo_paragraph_builder_build(
    builder: *mut ValoParagraphBuilder,
    fonts: *mut ValoFontCollection,
) -> *mut ValoParagraph {
    if builder.is_null() {
        return std::ptr::null_mut();
    }
    let Some(fonts) = (unsafe { borrow_mut(fonts) }) else {
        return std::ptr::null_mut();
    };
    let boxed = unsafe { Box::from_raw(builder) };
    // The collection answers misses itself during this build (its own
    // sources); what nothing answered waits in `take_unanswered`.
    let mut real = ParagraphBuilder::new(&mut fonts.collection);
    real.style(boxed.style);
    for (text, style) in &boxed.spans {
        real.add_text(text, style);
    }
    into_handle(ValoParagraph {
        paragraph: real.build(),
    })
}

/// # Safety
/// `paragraph` must be a live handle (or null).
#[no_mangle]
pub unsafe extern "C" fn valo_paragraph_dispose(paragraph: *mut ValoParagraph) {
    unsafe { dispose_handle(paragraph) }
}

// ── layout + metrics ────────────────────────────────────────────────────

/// Lay out to `max_width` (pass INFINITY for unconstrained).
///
/// # Safety
/// `paragraph` must be a live handle (or null, a no-op).
#[no_mangle]
pub unsafe extern "C" fn valo_paragraph_layout(paragraph: *mut ValoParagraph, max_width: f32) {
    if let Some(p) = unsafe { borrow_mut(paragraph) } {
        p.paragraph.layout(max_width);
    }
}

macro_rules! paragraph_metric {
    ($(#[$doc:meta])* $name:ident -> $ty:ty, |$p:ident| $body:expr) => {
        $(#[$doc])*
        /// # Safety
        /// `paragraph` must be a live handle (or null → zero).
        #[no_mangle]
        pub unsafe extern "C" fn $name(paragraph: *const ValoParagraph) -> $ty {
            match unsafe { borrow(paragraph) } {
                Some($p) => $body,
                None => Default::default(),
            }
        }
    };
}

paragraph_metric!(valo_paragraph_width -> f32, |p| p.paragraph.width());
paragraph_metric!(valo_paragraph_height -> f32, |p| p.paragraph.height());
paragraph_metric!(valo_paragraph_longest_line -> f32, |p| p.paragraph.longest_line());
paragraph_metric!(valo_paragraph_min_intrinsic_width -> f32, |p| p
    .paragraph
    .min_intrinsic_width());
paragraph_metric!(valo_paragraph_max_intrinsic_width -> f32, |p| p
    .paragraph
    .max_intrinsic_width());
paragraph_metric!(valo_paragraph_line_count -> usize, |p| p.paragraph.lines().len());
paragraph_metric!(
    /// True when `max_lines` truncated the content.
    valo_paragraph_did_exceed_max_lines -> bool,
    |p| p.paragraph.truncated()
);

// ── caret / selection queries ───────────────────────────────────────────

/// The caret rectangle for a byte offset.
///
/// # Safety
/// `paragraph` must be a live handle (or null → zero rect).
#[no_mangle]
pub unsafe extern "C" fn valo_paragraph_caret_for_offset(
    paragraph: *const ValoParagraph,
    byte_offset: usize,
) -> ValoRect {
    let Some(p) = (unsafe { borrow(paragraph) }) else {
        return zero_rect();
    };
    rect_out(p.paragraph.caret_for_offset(byte_offset))
}

/// The byte offset nearest a paragraph-local point; `out_downstream`
/// (nullable) receives the caret affinity.
///
/// # Safety
/// `paragraph` must be a live handle; `out_downstream` null or writable.
#[no_mangle]
pub unsafe extern "C" fn valo_paragraph_byte_offset_at(
    paragraph: *const ValoParagraph,
    x: f32,
    y: f32,
    out_downstream: *mut bool,
) -> usize {
    let Some(p) = (unsafe { borrow(paragraph) }) else {
        return 0;
    };
    let position = p.paragraph.glyph_position_at(valo::Point::new(x, y));
    if let Some(out) = unsafe { out_downstream.as_mut() } {
        *out = position.downstream;
    }
    position.offset
}

/// Selection rectangles for a byte range: writes up to `capacity` rects
/// and returns the TOTAL count (call once with capacity 0 to size).
///
/// # Safety
/// `paragraph` must be a live handle; `out_rects` must point to
/// `capacity` writable rects (or be null with capacity 0).
#[no_mangle]
pub unsafe extern "C" fn valo_paragraph_rects_for_range(
    paragraph: *const ValoParagraph,
    start: usize,
    end: usize,
    out_rects: *mut ValoRect,
    capacity: usize,
) -> usize {
    let Some(p) = (unsafe { borrow(paragraph) }) else {
        return 0;
    };
    let rects = p.paragraph.rects_for_range(start..end);
    if !out_rects.is_null() {
        for (index, rect) in rects.iter().take(capacity).enumerate() {
            unsafe { out_rects.add(index).write(rect_out(*rect)) };
        }
    }
    rects.len()
}

/// The word range around a byte offset (double-click selection).
///
/// # Safety
/// `paragraph` must be a live handle (or null → empty range).
#[no_mangle]
pub unsafe extern "C" fn valo_paragraph_word_boundary(
    paragraph: *const ValoParagraph,
    byte_offset: usize,
) -> ValoTextRange {
    let Some(p) = (unsafe { borrow(paragraph) }) else {
        return ValoTextRange { start: 0, end: 0 };
    };
    let range = p.paragraph.word_boundary(byte_offset);
    ValoTextRange {
        start: range.start,
        end: range.end,
    }
}

/// Metrics of line `index`; false past the last line.
///
/// # Safety
/// `paragraph` must be a live handle; `out_metrics` must be writable.
#[no_mangle]
pub unsafe extern "C" fn valo_paragraph_line_metrics(
    paragraph: *const ValoParagraph,
    index: usize,
    out_metrics: *mut ValoLineMetrics,
) -> bool {
    let (Some(p), Some(out)) = (unsafe { borrow(paragraph) }, unsafe {
        out_metrics.as_mut()
    }) else {
        return false;
    };
    let metrics = p.paragraph.line_metrics();
    let Some(line) = metrics.get(index) else {
        return false;
    };
    *out = ValoLineMetrics {
        start: line.range.start,
        end: line.range.end,
        baseline: line.baseline,
        ascent: line.ascent,
        descent: line.descent,
        left: line.left,
        width: line.width,
    };
    true
}

// ── conversions ─────────────────────────────────────────────────────────

fn text_style(style: &ValoTextStyle) -> TextStyle {
    let families = unsafe { utf8(style.families_utf8, style.families_length) }.unwrap_or("");
    let mut out = TextStyle::new("", style.size, style.color.into());
    out.families = families
        .split('\n')
        .filter(|family| !family.is_empty())
        .map(str::to_owned)
        .collect();
    out.weight = style.weight.clamp(1, 1000) as u16;
    out.italic = style.italic;
    out.letter_spacing = style.letter_spacing;
    out.word_spacing = style.word_spacing;
    out.height = (style.line_height > 0.0).then_some(style.line_height);
    out.decoration = decoration(style);
    out
}

fn decoration(style: &ValoTextStyle) -> Option<Decoration> {
    let kind = match style.decoration_kind {
        0 => DecorationKind::Underline,
        1 => DecorationKind::LineThrough,
        2 => DecorationKind::Overline,
        _ => return None,
    };
    Some(Decoration {
        kind,
        color: (style.decoration_color.alpha > 0.0).then(|| style.decoration_color.into()),
        thickness: if style.decoration_thickness > 0.0 {
            style.decoration_thickness
        } else {
            1.0
        },
    })
}

fn paragraph_style(style: &ValoParagraphStyle) -> ParagraphStyle {
    ParagraphStyle {
        align: match style.align {
            1 => TextAlign::Center,
            2 => TextAlign::Right,
            3 => TextAlign::Justify,
            _ => TextAlign::Left,
        },
        // The C struct is a committed ABI; a base direction would have to be
        // added to it, so this surface still infers from content.
        direction: None,
        preserve_trailing_whitespace: false,
        max_lines: (style.max_lines > 0).then_some(style.max_lines),
        ellipsis: unsafe { utf8(style.ellipsis_utf8, style.ellipsis_length) }
            .filter(|e| !e.is_empty())
            .map(str::to_owned),
    }
}

// ── font demand ───────────────────────────────────────────────────────────────

/// The paragraph's unanswered font demand, families half: names the
/// collection has no face for at all, NEWLINE-joined UTF-8 (family names
/// cannot contain newlines). Two-call: pass null/0 to size, then a
/// buffer; returns the TOTAL byte length.
///
/// # Safety
/// `paragraph` must be a live handle; `out_utf8` must point to `capacity`
/// writable bytes (or be null with capacity 0).
#[no_mangle]
pub unsafe extern "C" fn valo_paragraph_demand_families(
    paragraph: *const ValoParagraph,
    out_utf8: *mut u8,
    capacity: usize,
) -> usize {
    let Some(handle) = (unsafe { borrow(paragraph) }) else {
        return 0;
    };
    let joined = handle.paragraph.demand().families.join("\n");
    let bytes = joined.as_bytes();
    if !out_utf8.is_null() {
        let written = bytes.len().min(capacity);
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out_utf8, written) };
    }
    bytes.len()
}

/// The demand's codepoints half: chars no present face covers, as UTF-32.
/// Two-call: pass null/0 to size, then a buffer; returns the TOTAL count.
///
/// # Safety
/// `paragraph` must be a live handle; `out_codepoints` must point to
/// `capacity` writable u32 slots (or be null with capacity 0).
#[no_mangle]
pub unsafe extern "C" fn valo_paragraph_demand_codepoints(
    paragraph: *const ValoParagraph,
    out_codepoints: *mut u32,
    capacity: usize,
) -> usize {
    let Some(handle) = (unsafe { borrow(paragraph) }) else {
        return 0;
    };
    let mut codepoints: Vec<u32> = Vec::new();
    for &(codepoint, _) in &handle.paragraph.demand().codepoints {
        // The C surface reports CHARS (attrs stay internal to satisfy);
        // one char demanded under several styles reports once.
        if !codepoints.contains(&(codepoint as u32)) {
            codepoints.push(codepoint as u32);
        }
    }
    if !out_codepoints.is_null() {
        for (at, codepoint) in codepoints.iter().take(capacity).enumerate() {
            unsafe { *out_codepoints.add(at) = *codepoint };
        }
    }
    codepoints.len()
}

/// (pointer, length) → &str; None on null or invalid UTF-8.
pub(crate) unsafe fn utf8<'a>(bytes: *const u8, length: usize) -> Option<&'a str> {
    if bytes.is_null() {
        return None;
    }
    std::str::from_utf8(unsafe { std::slice::from_raw_parts(bytes, length) }).ok()
}

fn rect_out(rect: valo::Rect) -> ValoRect {
    ValoRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

fn zero_rect() -> ValoRect {
    ValoRect {
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
    }
}
