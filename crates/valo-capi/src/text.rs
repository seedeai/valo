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

/// `ValoFontCollection` holds registered faces used to shape paragraphs.
///
/// Create it with [`valo_fonts_new`], add faces, then pass it to
/// [`valo_paragraph_builder_new`] / [`valo_paragraph_builder_build`].
/// Rebuild paragraphs after adding faces. Dispose with [`valo_fonts_dispose`].
pub struct ValoFontCollection {
    pub(crate) collection: FontCollection,
}

/// `ValoParagraphBuilder` accumulates styled spans until [`valo_paragraph_builder_build`].
///
/// The real paragraph builder borrows a collection, which a C handle cannot
/// hold, so inputs are stored here and the borrow is taken only for the
/// duration of `build`.
pub struct ValoParagraphBuilder {
    style: valo::ParagraphStyle,
    spans: Vec<(String, valo::TextStyle)>,
}

/// `ValoParagraph` is a shaped paragraph ready for layout, drawing, and queries.
///
/// Produced by [`valo_paragraph_builder_build`]. Call [`valo_paragraph_layout`]
/// before drawing or reading metrics. Dispose with [`valo_paragraph_dispose`].
pub struct ValoParagraph {
    pub(crate) paragraph: Paragraph,
}

/// `ValoTextStyle` is one span's style, passed by value.
///
/// `families_utf8` is borrowed for the duration of the call that consumes
/// this struct. Unknown `decoration_kind` values mean no decoration.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ValoTextStyle {
    /// `families_utf8` is a newline-separated family list, tried in order per glyph.
    ///
    /// Newlines cannot occur in family names. Null/empty uses the collection's
    /// fallbacks only.
    pub families_utf8: *const u8,
    /// `families_length` is the byte length of `families_utf8`, not a NUL-terminated size.
    pub families_length: usize,
    /// `size` is the font size in logical pixels.
    pub size: f32,
    /// `weight` is a CSS weight, conventionally 100–900 and clamped to `1..=1000`.
    pub weight: u32,
    /// `italic` selects an italic face when available.
    pub italic: bool,
    /// `color` is the text fill color.
    pub color: ValoColor,
    /// `letter_spacing` adds logical pixels after each grapheme cluster.
    pub letter_spacing: f32,
    /// `word_spacing` adds logical pixels after each space, in addition to letter spacing.
    pub word_spacing: f32,
    /// `line_height` overrides line height as a multiple of `size`; `<= 0` uses the font's metrics.
    pub line_height: f32,
    /// `decoration_kind` is -1 none, 0 underline, 1 line-through, or 2 overline.
    pub decoration_kind: i32,
    /// `decoration_color` tints the decoration; `alpha <= 0` inherits the text color.
    pub decoration_color: ValoColor,
    /// `decoration_thickness` multiplies the font's suggested thickness; `<= 0` uses 1.
    pub decoration_thickness: f32,
}

/// `ValoParagraphStyle` controls layout for a complete paragraph, passed by value.
///
/// `ellipsis_utf8` is borrowed for the duration of [`valo_paragraph_builder_new`].
/// Unknown `align` values become left. Base writing direction is inferred
/// from content (the C struct has no direction field).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ValoParagraphStyle {
    /// `align` is 0 left, 1 center, 2 right, or 3 justify.
    pub align: i32,
    /// `max_lines` limits laid-out lines; 0 means unlimited.
    pub max_lines: u32,
    /// `ellipsis_utf8` replaces omitted content; null or empty truncates without an ellipsis.
    pub ellipsis_utf8: *const u8,
    /// `ellipsis_length` is the byte length of `ellipsis_utf8`.
    pub ellipsis_length: usize,
}

/// `ValoTextRange` is a UTF-8 byte range `[start, end)` in paragraph text.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ValoTextRange {
    /// `start` is the inclusive UTF-8 byte offset.
    pub start: usize,
    /// `end` is the exclusive UTF-8 byte offset.
    pub end: usize,
}

/// `ValoLineMetrics` measures one laid-out line in paragraph-local logical pixels.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ValoLineMetrics {
    /// `start` is the inclusive UTF-8 byte offset covered by the line.
    pub start: usize,
    /// `end` is the exclusive UTF-8 byte offset covered by the line.
    pub end: usize,
    /// `baseline` is the paragraph-local y coordinate of the baseline.
    pub baseline: f32,
    /// `ascent` is the logical-pixel distance above the baseline.
    pub ascent: f32,
    /// `descent` is the logical-pixel distance below the baseline.
    pub descent: f32,
    /// `left` is the paragraph-local x coordinate after alignment.
    pub left: f32,
    /// `width` is the line's signed content advance in logical pixels.
    pub width: f32,
}

// ── fonts ───────────────────────────────────────────────────────────────

/// `valo_fonts_new` creates an empty font collection.
#[no_mangle]
pub extern "C" fn valo_fonts_new() -> *mut ValoFontCollection {
    into_handle(ValoFontCollection {
        collection: FontCollection::default(),
    })
}

/// `valo_fonts_dispose` releases a font collection. Null is a no-op.
///
/// # Safety
/// `fonts` must be a live [`valo_fonts_new`] handle (or null).
#[no_mangle]
pub unsafe extern "C" fn valo_fonts_dispose(fonts: *mut ValoFontCollection) {
    unsafe { dispose_handle(fonts) }
}

/// `valo_fonts_add` registers a face from TTF/OTF bytes.
///
/// Returns the face id, or -1 when the bytes don't parse. The buffer is
/// copied and only has to outlive this call. Paragraphs built from earlier
/// snapshots of the collection are unaffected — rebuild them to see new
/// faces.
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

/// `valo_fonts_add_fallback` appends a registered face to the fallback chain.
///
/// Codepoints no styled family covers try the chain in order. Null handle
/// or a negative id is a no-op. A nonnegative id must have been returned by
/// this collection.
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

/// `valo_fonts_add_instances` registers every face a font file offers.
///
/// A static font is one face; a variable font is its named instances, so
/// weights and styles select like a static multi-weight family.
/// `add_as_fallbacks` also appends each face to the fallback chain (default
/// UI fonts). Returns the number of faces added; 0 when the bytes don't
/// parse.
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

/// `valo_fonts_family_name` writes the family name of a registered face.
///
/// Writes up to `capacity` UTF-8 bytes and returns the total length (call
/// with capacity 0 to size). Returns 0 for an unknown id or null handle.
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

/// `valo_paragraph_builder_new` creates a paragraph builder with `style`.
///
/// `fonts` must be non-null (returns null otherwise) but the collection is
/// used at [`valo_paragraph_builder_build`], not construction. Embedders
/// with style stacks resolve them before adding spans.
///
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

/// `valo_paragraph_builder_dispose` releases a builder that was not built. Null is a no-op.
///
/// # Safety
/// `builder` must be a live handle not yet built (or null).
#[no_mangle]
pub unsafe extern "C" fn valo_paragraph_builder_dispose(builder: *mut ValoParagraphBuilder) {
    unsafe { dispose_handle(builder) }
}

/// `valo_paragraph_builder_add_text` appends one styled span of UTF-8 text.
///
/// Embedders with style stacks resolve them first. Null builder, null text,
/// or invalid UTF-8 is a no-op. Style family pointers only have to outlive
/// this call.
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

/// `valo_paragraph_builder_build` consumes the builder and returns a shaped paragraph.
///
/// The collection resolves any missing families/codepoints from its own
/// sources during this call, and may grow. Call [`valo_paragraph_layout`]
/// before drawing or querying. Null builder or fonts returns null.
///
/// # Safety
/// `builder` must be a live handle (or null → null); `fonts` must be a live
/// handle (or null → null).
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

/// `valo_paragraph_dispose` releases a paragraph handle. Null is a no-op.
///
/// # Safety
/// `paragraph` must be a live handle (or null).
#[no_mangle]
pub unsafe extern "C" fn valo_paragraph_dispose(paragraph: *mut ValoParagraph) {
    unsafe { dispose_handle(paragraph) }
}

// ── layout + metrics ────────────────────────────────────────────────────

/// `valo_paragraph_layout` wraps the paragraph to `max_width`.
///
/// Pass infinity for unconstrained width. Call this before drawing or
/// reading metrics. Repeating the same width reuses the existing layout.
///
/// # Safety
/// `paragraph` must be a live handle (or null, a no-op).
#[no_mangle]
pub unsafe extern "C" fn valo_paragraph_layout(paragraph: *mut ValoParagraph, max_width: f32) {
    if let Some(p) = unsafe { borrow_mut(paragraph) } {
        p.paragraph.layout(max_width);
    }
}

/// Reads one paragraph metric as a `valo_paragraph_*` C function.
///
/// Invocation rustdoc covers units and the zero-before-layout default. This
/// expansion always appends the shared null-safety contract: a null
/// `paragraph` returns the type's zero value.
macro_rules! paragraph_metric {
    ($(#[$doc:meta])* $name:ident -> $ty:ty, |$p:ident| $body:expr) => {
        $(#[$doc])*
        ///
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

paragraph_metric!(
    /// `valo_paragraph_width` returns the nonnegative width of the widest laid-out line.
    ///
    /// It is zero before [`valo_paragraph_layout`].
    valo_paragraph_width -> f32,
    |p| p.paragraph.width()
);
paragraph_metric!(
    /// `valo_paragraph_height` returns the laid-out paragraph height in logical pixels.
    ///
    /// It is zero before [`valo_paragraph_layout`].
    valo_paragraph_height -> f32,
    |p| p.paragraph.height()
);
paragraph_metric!(
    /// `valo_paragraph_longest_line` returns the width of the widest laid-out line.
    ///
    /// It is zero before [`valo_paragraph_layout`].
    valo_paragraph_longest_line -> f32,
    |p| p.paragraph.longest_line()
);
paragraph_metric!(
    /// `valo_paragraph_min_intrinsic_width` returns the widest unbreakable segment.
    ///
    /// It is zero before [`valo_paragraph_layout`].
    valo_paragraph_min_intrinsic_width -> f32,
    |p| p.paragraph.min_intrinsic_width()
);
paragraph_metric!(
    /// `valo_paragraph_max_intrinsic_width` returns the width required to avoid soft wrapping.
    ///
    /// It is zero before [`valo_paragraph_layout`].
    valo_paragraph_max_intrinsic_width -> f32,
    |p| p.paragraph.max_intrinsic_width()
);
paragraph_metric!(
    /// `valo_paragraph_line_count` returns how many lines the last layout produced.
    ///
    /// It is zero before [`valo_paragraph_layout`].
    valo_paragraph_line_count -> usize,
    |p| p.paragraph.lines().len()
);
paragraph_metric!(
    /// `valo_paragraph_did_exceed_max_lines` is true when `max_lines` truncated the content.
    valo_paragraph_did_exceed_max_lines -> bool,
    |p| p.paragraph.truncated()
);

// ── caret / selection queries ───────────────────────────────────────────

/// `valo_paragraph_caret_for_offset` returns a zero-width caret rectangle for a UTF-8 byte offset.
///
/// Coordinates are paragraph-local logical pixels. Returns a zero rect when
/// there are no lines (including before layout) or the handle is null.
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

/// `valo_paragraph_byte_offset_at` returns the byte offset nearest a paragraph-local point.
///
/// `out_downstream` (nullable) receives the caret affinity. An empty
/// paragraph or null handle returns offset 0.
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

/// `valo_paragraph_rects_for_range` writes selection rectangles for a UTF-8 byte range.
///
/// Writes up to `capacity` rects and returns the total count (call once with
/// capacity 0 to size). Null handle returns 0.
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

/// `valo_paragraph_word_boundary` returns the word range around a UTF-8 byte offset.
///
/// Use it for double-click selection. A null handle returns an empty range.
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

/// `valo_paragraph_line_metrics` writes metrics of line `index`.
///
/// Returns false past the last line, before layout, or when a pointer is
/// null.
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

/// `valo_paragraph_demand_families` writes family names the collection had no face for.
///
/// Names are newline-joined UTF-8 (family names cannot contain newlines).
/// Two-call: pass null/0 to size, then a buffer; returns the total byte
/// length.
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

/// `valo_paragraph_demand_codepoints` writes chars no present face covers, as UTF-32.
///
/// Two-call: pass null/0 to size, then a buffer; returns the total count.
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
