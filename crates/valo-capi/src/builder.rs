//! Display-list recording: one builder handle, valo-dl's op vocabulary,
//! paint by value per op. [`valo_builder_build`] finishes the handle into a
//! display list the context renders (and other lists may embed).

use std::sync::Arc;

use valo::{DisplayList, DisplayListBuilder, DrawGlyphRunExt, DrawParagraphExt};

use crate::{
    borrow, borrow_mut, clip_op, dispose_handle, fill_rule, into_handle, paint_of, ValoCornerRadii,
    ValoImage, ValoPaint, ValoParagraph, ValoPath, ValoRect, ValoTransform,
};

/// `ValoDisplayListBuilder` records drawing commands into a [`ValoDisplayList`].
///
/// Create it with [`valo_builder_new`]. Recording is GPU-free. Finish with
/// [`valo_builder_build`], which consumes the handle, or abandon it with
/// [`valo_builder_dispose`]. Handles are not thread-safe.
pub struct ValoDisplayListBuilder {
    builder: DisplayListBuilder,
}

/// `ValoDisplayList` is an immutable recording of drawing commands.
///
/// Produced by [`valo_builder_build`]. Replay it with [`crate::valo_context_render`]
/// or nest it via [`valo_builder_draw_display_list`]. Dispose with
/// [`valo_display_list_dispose`].
pub struct ValoDisplayList {
    pub(crate) list: Arc<DisplayList>,
}

/// `valo_builder_new` creates an empty display-list recorder.
#[no_mangle]
pub extern "C" fn valo_builder_new() -> *mut ValoDisplayListBuilder {
    into_handle(ValoDisplayListBuilder {
        builder: DisplayListBuilder::new(),
    })
}

/// `valo_builder_build` finishes recording: consumes the builder handle and
/// returns the display list.
///
/// Dispose the list with [`valo_display_list_dispose`]. Null builder returns
/// null.
///
/// # Safety
/// `builder` must be a live [`valo_builder_new`] handle (or null → null).
#[no_mangle]
pub unsafe extern "C" fn valo_builder_build(
    builder: *mut ValoDisplayListBuilder,
) -> *mut ValoDisplayList {
    if builder.is_null() {
        return std::ptr::null_mut();
    }
    let boxed = unsafe { Box::from_raw(builder) };
    into_handle(ValoDisplayList {
        list: Arc::new(boxed.builder.build()),
    })
}

/// `valo_builder_dispose` releases a builder that was not built. Null is a no-op.
///
/// # Safety
/// `builder` must be a live handle not yet built (or null).
#[no_mangle]
pub unsafe extern "C" fn valo_builder_dispose(builder: *mut ValoDisplayListBuilder) {
    unsafe { dispose_handle(builder) }
}

/// `valo_display_list_dispose` releases a finished display list. Null is a no-op.
///
/// # Safety
/// `list` must be a live [`valo_builder_build`] handle (or null).
#[no_mangle]
pub unsafe extern "C" fn valo_display_list_dispose(list: *mut ValoDisplayList) {
    unsafe { dispose_handle(list) }
}

/// Records one display-list op as a `valo_builder_*` C function.
///
/// Invocation rustdoc covers that op's arguments and units. This expansion
/// always appends the shared null-safety contract: a null `builder` (and any
/// other handle argument) is a no-op.
macro_rules! builder_op {
    ($(#[$doc:meta])* $name:ident($($arg:ident: $ty:ty),*), |$b:ident| $body:expr) => {
        $(#[$doc])*
        ///
        /// # Safety
        /// `builder` must be a live handle (or null, a no-op); any other
        /// handle argument likewise.
        #[no_mangle]
        pub unsafe extern "C" fn $name(builder: *mut ValoDisplayListBuilder $(, $arg: $ty)*) {
            if let Some(handle) = unsafe { borrow_mut(builder) } {
                let $b = &mut handle.builder;
                $body;
            }
        }
    };
}

// ── state & transforms ──────────────────────────────────────────────────

builder_op!(
    /// `valo_builder_save` preserves the current transform and clip until the matching restore.
    valo_builder_save(),
    |b| b.save()
);
builder_op!(
    /// `valo_builder_save_layer` begins a composited layer.
    ///
    /// The paint's alpha, blend mode, and mask blur apply to the layer as a
    /// whole on restore.
    valo_builder_save_layer(paint: ValoPaint),
    |b| b.save_layer(None, &paint_of(paint))
);
builder_op!(
    /// `valo_builder_restore` closes the most recent save or layer scope.
    ///
    /// An unmatched restore triggers a debug assertion and is ignored in
    /// release builds.
    valo_builder_restore(),
    |b| b.restore()
);
builder_op!(
    /// `valo_builder_translate` offsets subsequent drawing and clipping, in logical pixels.
    valo_builder_translate(x: f32, y: f32),
    |b| b.translate(x, y)
);
builder_op!(
    /// `valo_builder_scale` scales subsequent drawing and clipping.
    valo_builder_scale(x: f32, y: f32),
    |b| b.scale(x, y)
);
builder_op!(
    /// `valo_builder_rotate` rotates subsequent drawing and clipping clockwise, in radians.
    valo_builder_rotate(radians: f32),
    |b| b.rotate(radians)
);
builder_op!(
    /// `valo_builder_transform` concatenates a 2×3 affine transform.
    valo_builder_transform(transform: ValoTransform),
    |b| b.concat(&transform.into())
);

/// `valo_builder_transform_matrix` concatenates a full column-major 4×4.
///
/// The 16 floats are the Flutter canvas order — perspective rows included;
/// the z output is ignored for painting.
///
/// # Safety
/// `builder` must be a live handle (or null, a no-op); `matrix` must point
/// to 16 readable floats.
#[no_mangle]
pub unsafe extern "C" fn valo_builder_transform_matrix(
    builder: *mut ValoDisplayListBuilder,
    matrix: *const f32,
) {
    let Some(handle) = (unsafe { crate::borrow_mut(builder) }) else {
        return;
    };
    if matrix.is_null() {
        return;
    }
    let values: [f32; 16] = unsafe { std::slice::from_raw_parts(matrix, 16) }
        .try_into()
        .expect("sixteen floats");
    handle
        .builder
        .concat(&valo::Matrix::from_flutter_array(&values));
}

// ── clips ───────────────────────────────────────────────────────────────

builder_op!(
    /// `valo_builder_clip_rect` applies a rectangular clip until the current scope ends.
    ///
    /// `operation`: 0 intersect, 1 difference.
    valo_builder_clip_rect(rect: ValoRect, operation: i32),
    |b| b.clip_rect(rect, clip_op(operation))
);
builder_op!(
    /// `valo_builder_clip_rounded_rect` applies a rounded-rectangle clip until the current scope ends.
    ///
    /// `operation`: 0 intersect, 1 difference.
    valo_builder_clip_rounded_rect(rect: ValoRect, radii: ValoCornerRadii, operation: i32),
    |b| b.clip_rrect_radii_elliptical(rect, radii.to_elliptical(), clip_op(operation))
);

/// `valo_builder_clip_path` applies a path clip until the current scope ends.
///
/// `rule`: 0 non-zero, 1 even-odd; `operation`: 0 intersect, 1 difference.
///
/// # Safety
/// `builder` and `path` must be live handles (or null, a no-op).
#[no_mangle]
pub unsafe extern "C" fn valo_builder_clip_path(
    builder: *mut ValoDisplayListBuilder,
    path: *mut ValoPath,
    rule: i32,
    operation: i32,
) {
    let (Some(handle), Some(path)) = (unsafe { borrow_mut(builder) }, unsafe { borrow_mut(path) })
    else {
        return;
    };
    handle
        .builder
        .clip_path(&path.built(), fill_rule(rule), clip_op(operation));
}

// ── draws ───────────────────────────────────────────────────────────────

builder_op!(
    /// `valo_builder_draw_rect` fills or strokes an axis-aligned rectangle.
    valo_builder_draw_rect(rect: ValoRect, paint: ValoPaint),
    |b| b.draw_rect(rect, &paint_of(paint))
);
builder_op!(
    /// `valo_builder_draw_rounded_rect` fills or strokes a rounded rectangle.
    valo_builder_draw_rounded_rect(rect: ValoRect, radii: ValoCornerRadii, paint: ValoPaint),
    |b| b.draw_rrect_radii_elliptical(rect, radii.to_elliptical(), &paint_of(paint))
);
builder_op!(
    /// `valo_builder_draw_circle` fills or strokes a circle in logical pixels.
    valo_builder_draw_circle(center_x: f32, center_y: f32, radius: f32, paint: ValoPaint),
    |b| b.draw_circle((center_x, center_y), radius, &paint_of(paint))
);

/// `valo_builder_draw_path` fills or strokes a path.
///
/// `rule`: 0 non-zero, 1 even-odd.
///
/// # Safety
/// `builder` and `path` must be live handles (or null, a no-op).
#[no_mangle]
pub unsafe extern "C" fn valo_builder_draw_path(
    builder: *mut ValoDisplayListBuilder,
    path: *mut ValoPath,
    rule: i32,
    paint: ValoPaint,
) {
    let (Some(handle), Some(path)) = (unsafe { borrow_mut(builder) }, unsafe { borrow_mut(path) })
    else {
        return;
    };
    handle
        .builder
        .draw_path(&path.built(), fill_rule(rule), &paint_of(paint));
}

/// `valo_builder_draw_image_rect` draws `source` (texel rect) of the image into `destination`.
///
/// The paint's alpha, blend mode, and mask blur apply; its RGB channels are
/// ignored. `sampling`: 0 linear (mipmapped), 1 nearest.
///
/// # Safety
/// `builder` and `image` must be live handles (or null, a no-op).
#[no_mangle]
pub unsafe extern "C" fn valo_builder_draw_image_rect(
    builder: *mut ValoDisplayListBuilder,
    image: *const ValoImage,
    source: ValoRect,
    destination: ValoRect,
    sampling: i32,
    paint: ValoPaint,
) {
    let (Some(handle), Some(image)) = (unsafe { borrow_mut(builder) }, unsafe { borrow(image) })
    else {
        return;
    };
    handle.builder.draw_image_rect(
        &image.image,
        source.into(),
        destination.into(),
        crate::types::sampling(sampling),
        &paint_of(paint),
    );
}

/// `valo_builder_draw_display_list` replays a finished display list here (nesting).
///
/// # Safety
/// `builder` and `list` must be live handles (or null, a no-op).
#[no_mangle]
pub unsafe extern "C" fn valo_builder_draw_display_list(
    builder: *mut ValoDisplayListBuilder,
    list: *const ValoDisplayList,
) {
    let (Some(handle), Some(list)) = (unsafe { borrow_mut(builder) }, unsafe { borrow(list) })
    else {
        return;
    };
    handle.builder.draw_display_list(&list.list);
}

/// `valo_builder_draw_paragraph` draws a laid-out paragraph with its styles' own colors.
///
/// Top-left is (`x`, `y`) in logical pixels. Call [`crate::valo_paragraph_layout`]
/// first.
///
/// # Safety
/// `builder` and `paragraph` must be live handles (or null, a no-op).
#[no_mangle]
pub unsafe extern "C" fn valo_builder_draw_paragraph(
    builder: *mut ValoDisplayListBuilder,
    paragraph: *const ValoParagraph,
    x: f32,
    y: f32,
) {
    let (Some(handle), Some(paragraph)) =
        (unsafe { borrow_mut(builder) }, unsafe { borrow(paragraph) })
    else {
        return;
    };
    handle.builder.draw_paragraph(&paragraph.paragraph, (x, y));
}

/// `valo_builder_draw_paragraph_with` draws a laid-out paragraph with `paint`
/// overriding every run's own fill.
///
/// Use it for stroked or blended text. Colour, style, stroke, blend mode,
/// mask blur and colour filter all apply; shadows and decorations still
/// come from the paragraph's styles. [`ValoPaint`] carries no shader, so
/// gradient-filled text is not reachable from C. A stroke width of 0 is a
/// hairline (one device pixel); only a negative width draws nothing.
///
/// # Safety
/// `builder` and `paragraph` must be live handles (or null, a no-op).
#[no_mangle]
pub unsafe extern "C" fn valo_builder_draw_paragraph_with(
    builder: *mut ValoDisplayListBuilder,
    paragraph: *const ValoParagraph,
    x: f32,
    y: f32,
    paint: ValoPaint,
) {
    let (Some(handle), Some(paragraph)) =
        (unsafe { borrow_mut(builder) }, unsafe { borrow(paragraph) })
    else {
        return;
    };
    handle
        .builder
        .draw_paragraph_with(&paragraph.paragraph, (x, y), &unsafe { paint_of(paint) });
}
