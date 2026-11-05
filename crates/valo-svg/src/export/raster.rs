//! The first pass: the regions of a list its SVG won't hold, each with the list that draws it
//! alone and where its pixels go. The writer walks the list by the same rule (`rasterized`), so
//! the nth region it skips is the nth found here.

use std::sync::Arc;

use valo::{Bounds, DisplayList, DisplayListBuilder, MatrixClipState, Op, Rect};

use super::native::rasterized;
use super::NativeEffects;

/// `RasterRegion` is a region of a document drawn as pixels: the list that draws it, already
/// moved and scaled into its pixels, and where those go in the document, in its units.
pub(super) struct RasterRegion {
    pub list: Arc<DisplayList>,
    pub pixel_size: [u32; 2],
    pub bounds: Rect,
}

/// `raster_regions` are the regions of `list`, a document of `size` units drawn with `native`,
/// its SVG won't hold, in the order a walk meets them: `None` for one that shows nowhere in it.
pub(super) fn raster_regions(
    list: &DisplayList,
    size: [f32; 2],
    pixels_per_unit: f32,
    native: NativeEffects,
) -> Vec<Option<RasterRegion>> {
    let area = Rect::new(0.0, 0.0, size[0], size[1]);
    let mut regions = Vec::new();
    let mut ops = list.walk();
    while let Some(op) = ops.next() {
        if rasterized(op, ops.scope(), native) {
            let state = ops.matrix_clip();
            regions.push(raster_region(op, ops.scope(), state, area, pixels_per_unit));
            ops.skip_scope();
        }
    }
    regions
}

/// `raster_region` is `scope`, drawn where `state` places it, as a region of `area`: on whole
/// pixels at `pixels_per_unit`, so its picture lands where it was drawn.
fn raster_region(
    op: &Op,
    scope: &[Op],
    state: &MatrixClipState,
    area: Rect,
    pixels_per_unit: f32,
) -> Option<RasterRegion> {
    let shown = match state.bounds(op) {
        Bounds::Unbounded => Some(area),
        Bounds::Bounded(rect) => rect.intersect(&area),
        Bounds::Empty => None,
    }
    .filter(|rect| !rect.is_empty())?;
    let scale = pixels_per_unit;
    let left = (shown.x * scale).floor();
    let top = (shown.y * scale).floor();
    let right = ((shown.x + shown.width) * scale).ceil();
    let bottom = ((shown.y + shown.height) * scale).ceil();
    let mut b = DisplayListBuilder::new();
    b.translate(-left, -top);
    b.scale(scale, scale);
    b.draw_display_list(&Arc::new(state.isolate(scope)));
    Some(RasterRegion {
        list: Arc::new(b.build()),
        pixel_size: [(right - left) as u32, (bottom - top) as u32],
        bounds: Rect::new(
            left / scale,
            top / scale,
            (right - left) / scale,
            (bottom - top) / scale,
        ),
    })
}
