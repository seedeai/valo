//! The calls a [`DisplayListBuilder`](crate::DisplayListBuilder) was given, as given.
//!
//! A builder simplifies what it records: it drops a draw that changes nothing, lowers a
//! rounded rectangle to a path, ignores a restore with no save to close. A test that checks
//! what a painter asked for, not what the list kept, reads the builder's call log instead,
//! as Skia's `SkRecorder` appends one `SkRecords` record per canvas call. Each variant is one
//! public builder method, and its fields are that method's arguments: a public method added
//! to the builder needs a variant here, logged first thing in the method, before anything
//! it returns early for or lowers to.

use std::sync::Arc;

use valo_geometry::{Color, FillRule, Matrix, Path, Point, Rect};

use crate::{
    Backdrop, ClipOp, DisplayList, GlyphPos, Image, MaskKind, Paint, RunFont, RunText, Sampling,
};

/// `Call` is one call made to a [`DisplayListBuilder`](crate::DisplayListBuilder), with its
/// arguments as the caller passed them, before the builder simplifies or drops it.
///
/// It is Skia's `SkRecords`, one record per canvas call. Shared resources are held by
/// reference, as Skia's records hold their refs, and compare as their types do: an
/// [`Image`] and a font by identity, a nested [`DisplayList`] by identity or by its commands.
/// No call carries the transform or clip it was made under; replaying the calls before it
/// gives them.
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    /// `Save` is [`save`](crate::DisplayListBuilder::save).
    Save,
    /// `SaveLayer` is [`save_layer`](crate::DisplayListBuilder::save_layer).
    SaveLayer {
        bounds_hint: Option<Rect>,
        paint: Paint,
    },
    /// `SaveLayerMask` is [`save_layer_mask`](crate::DisplayListBuilder::save_layer_mask).
    SaveLayerMask {
        bounds_hint: Option<Rect>,
        kind: MaskKind,
    },
    /// `SaveLayerBackdrop` is
    /// [`save_layer_backdrop`](crate::DisplayListBuilder::save_layer_backdrop).
    SaveLayerBackdrop {
        bounds_hint: Option<Rect>,
        paint: Paint,
        backdrop: Backdrop,
    },
    /// `Restore` is [`restore`](crate::DisplayListBuilder::restore), logged also when there
    /// is no save for it to close.
    Restore,
    /// `Translate` is [`translate`](crate::DisplayListBuilder::translate).
    Translate { tx: f32, ty: f32 },
    /// `Scale` is [`scale`](crate::DisplayListBuilder::scale).
    Scale { sx: f32, sy: f32 },
    /// `Rotate` is [`rotate`](crate::DisplayListBuilder::rotate).
    Rotate { radians: f32 },
    /// `Concat` is [`concat`](crate::DisplayListBuilder::concat).
    Concat { matrix: Matrix },
    /// `ClipRect` is [`clip_rect`](crate::DisplayListBuilder::clip_rect).
    ClipRect {
        rect: Rect,
        op: ClipOp,
        anti_alias: bool,
    },
    /// `ClipRRect` is [`clip_rrect`](crate::DisplayListBuilder::clip_rrect) and its
    /// per-corner forms, `radii` telling which.
    ClipRRect {
        rect: Rect,
        radii: RRectRadii,
        op: ClipOp,
        anti_alias: bool,
    },
    /// `ClipPath` is [`clip_path`](crate::DisplayListBuilder::clip_path).
    ClipPath {
        path: Arc<Path>,
        fill_rule: FillRule,
        op: ClipOp,
        anti_alias: bool,
    },
    /// `DrawPaint` is [`draw_paint`](crate::DisplayListBuilder::draw_paint).
    DrawPaint { paint: Paint },
    /// `DrawShadow` is [`draw_shadow`](crate::DisplayListBuilder::draw_shadow).
    DrawShadow {
        path: Arc<Path>,
        fill_rule: FillRule,
        color: Color,
        elevation: f32,
        transparent_occluder: bool,
        device_pixel_ratio: f32,
    },
    /// `DrawRect` is [`draw_rect`](crate::DisplayListBuilder::draw_rect).
    DrawRect { rect: Rect, paint: Paint },
    /// `DrawRRect` is [`draw_rrect`](crate::DisplayListBuilder::draw_rrect) and its
    /// per-corner forms, `radii` telling which.
    DrawRRect {
        rect: Rect,
        radii: RRectRadii,
        paint: Paint,
    },
    /// `DrawCircle` is [`draw_circle`](crate::DisplayListBuilder::draw_circle).
    DrawCircle {
        center: Point,
        radius: f32,
        paint: Paint,
    },
    /// `DrawPath` is [`draw_path`](crate::DisplayListBuilder::draw_path).
    DrawPath {
        path: Arc<Path>,
        fill_rule: FillRule,
        paint: Paint,
    },
    /// `DrawImage` is [`draw_image`](crate::DisplayListBuilder::draw_image).
    DrawImage {
        image: Image,
        dst: Rect,
        paint: Paint,
    },
    /// `DrawImageRect` is [`draw_image_rect`](crate::DisplayListBuilder::draw_image_rect).
    DrawImageRect {
        image: Image,
        src: Rect,
        dst: Rect,
        sampling: Sampling,
        paint: Paint,
    },
    /// `DrawGlyphRun` is [`draw_glyph_run`](crate::DisplayListBuilder::draw_glyph_run).
    DrawGlyphRun {
        font: RunFont,
        paint: Paint,
        glyphs: Arc<Vec<GlyphPos>>,
        text: Option<RunText>,
        local_bounds: Rect,
    },
    /// `DrawDisplayList` is [`draw_display_list`](crate::DisplayListBuilder::draw_display_list),
    /// or [`draw_display_list_cached`](crate::DisplayListBuilder::draw_display_list_cached)
    /// when `cached`. The nested list is one call; its own calls are logged by the builder
    /// that recorded it, if that one logs.
    DrawDisplayList {
        list: Arc<DisplayList>,
        cached: bool,
    },
}

/// `RRectRadii` is the corner radii a rounded-rectangle call was given, which also tells
/// which of the rounded-rectangle methods it was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RRectRadii {
    /// `Uniform` is one radius for every corner: `draw_rrect` and `clip_rrect`.
    Uniform(f32),
    /// `Circular` is one radius per corner, clockwise from the top-left: `draw_rrect_radii`
    /// and `clip_rrect_radii`.
    Circular([f32; 4]),
    /// `Elliptical` is `[x, y]` radii per corner, clockwise from the top-left:
    /// `draw_rrect_radii_elliptical` and `clip_rrect_radii_elliptical`.
    Elliptical([[f32; 2]; 4]),
}
