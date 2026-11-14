//! The first pass: the regions of a page PDF can't hold, each with the list that draws it alone
//! and where its pixels go. The writer walks a page by the same rule (`rasterized`), so the nth
//! region it skips is the nth found here.

use std::sync::Arc;

use valo::{Bounds, DisplayList, DisplayListBuilder, MatrixClipState, Op, Paint, Rect, Shader};

/// `RasterRegion` is a region of a page drawn as pixels: the list that draws it, already moved
/// and scaled into its pixels, and where those go on the page, in page units.
pub(crate) struct RasterRegion {
    pub list: Arc<DisplayList>,
    pub pixel_size: [u32; 2],
    pub page_bounds: Rect,
}

/// `raster_regions` are the regions of `list`, a page of `size` units, PDF can't hold, in the
/// order a walk meets them: `None` for one that shows nowhere on the page.
pub(crate) fn raster_regions(
    list: &DisplayList,
    size: [f32; 2],
    pixels_per_unit: f32,
) -> Vec<Option<RasterRegion>> {
    let page = Rect::new(0.0, 0.0, size[0], size[1]);
    let mut regions = Vec::new();
    let mut ops = list.walk();
    while let Some(op) = ops.next() {
        if rasterized(op, ops.scope()) {
            let state = ops.matrix_clip();
            regions.push(raster_region(op, ops.scope(), state, page, pixels_per_unit));
            ops.skip_scope();
        }
    }
    regions
}

/// `rasterized` is whether PDF can't hold `op` and the `scope` it opens: a blur, a shadow, a
/// filter, an image, a shader PDF has no word for, or a layer with a filter, a backdrop or a mask, or one a
/// mask inside it masks.
pub(crate) fn rasterized(op: &Op, scope: &[Op]) -> bool {
    match op {
        Op::SaveLayer {
            paint,
            mask_composite,
            backdrop,
            ..
        } => {
            mask_composite.is_some()
                || backdrop.is_some()
                || paint.effective_image_filter().is_some()
                || paint.color_filter.is_some()
                || holds_a_mask(scope)
        }
        Op::RRectBlur { .. } | Op::DrawShadow { .. } | Op::DrawImage { .. } => true,
        Op::DrawPaint { paint, .. }
        | Op::DrawRect { paint, .. }
        | Op::DrawPath { paint, .. }
        | Op::GlyphRun { paint, .. } => needs_pixels(paint),
        _ => false,
    }
}

fn needs_pixels(paint: &Paint) -> bool {
    paint.mask_blur.is_some()
        || paint.effective_image_filter().is_some()
        || paint.color_filter.is_some()
        || matches!(
            paint.shader,
            Some(Shader::Image { .. } | Shader::Runtime(_))
        )
}

/// `holds_a_mask` is whether a mask layer opens directly inside the layer `scope` opens: a mask
/// applies to the layer it is in.
fn holds_a_mask(scope: &[Op]) -> bool {
    let mut depth = 0usize;
    for op in scope {
        match op {
            Op::SaveLayer {
                mask_composite: Some(_),
                ..
            } if depth == 1 => return true,
            Op::Save | Op::SaveLayer { .. } => depth += 1,
            Op::Restore => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

/// `raster_region` is `scope`, drawn where `state` places it, as a region of `page`: on whole
/// pixels at `pixels_per_unit`, so its picture lands where it was drawn.
fn raster_region(
    op: &Op,
    scope: &[Op],
    state: &MatrixClipState,
    page: Rect,
    pixels_per_unit: f32,
) -> Option<RasterRegion> {
    let shown = match state.bounds(op) {
        Bounds::Unbounded => Some(page),
        Bounds::Bounded(rect) => rect.intersect(&page),
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
        page_bounds: Rect::new(
            left / scale,
            top / scale,
            (right - left) / scale,
            (bottom - top) / scale,
        ),
    })
}
