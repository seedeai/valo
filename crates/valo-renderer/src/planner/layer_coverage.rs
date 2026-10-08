//! How big a save layer's texture is: Impeller's `ComputeSaveLayerCoverage`
//! (`entity/save_layer_utils.cc`), ported with its unit tests. A layer holds
//! its content, cut to what its image filter needs of the clip and the
//! target (the filter's source coverage of that limit) — never padded for the
//! filter's own halo, which the filter makes room for when it runs.
//!
//! The function is a transcription; a change to it changes which pixels a
//! layer holds, and so what a blur near a clip reads.

use valo_dl::Bounds;
use valo_geometry::{Matrix, Rect};

/// `SIZE_THRESHOLD` is Impeller's `kDefaultSizeThreshold`: a size within
/// this fraction of another counts as the same size, so a texture that
/// varies by little from frame to frame keeps one size and is reused.
const SIZE_THRESHOLD: f32 = 0.3;

/// `SourceCoverage` is what an image filter answers when a save layer is
/// sized: Impeller's `FilterContents::GetSourceCoverage`.
pub(super) trait SourceCoverage {
    /// `source_coverage` is the region of input, in the transformed space,
    /// that the filter needs to produce everything inside `output_limit`;
    /// `None` when no input can produce it.
    fn source_coverage(&self, output_limit: &Rect) -> Option<Rect>;
}

/// `compute_save_layer_coverage` is Impeller's `ComputeSaveLayerCoverage`:
/// the save layer's coverage in the target's space, or `None` when it
/// misses the coverage limit and the layer can be skipped.
///
/// `content_coverage` is in the layer's local space and `effect_transform`
/// takes it into the target's; unbounded content is Impeller's maximum rect.
/// `coverage_limit` is the clip and the target.
/// `flood_coverage` is set for a layer that floods its clip (the recorder's
/// `floods_clip`). It stands for both of Impeller's flags, which size a
/// layer alike: `flood_output_coverage` for a destructive blend, and
/// `flood_input_coverage` for a backdrop or a filter that colours
/// transparent pixels.
pub(super) fn compute_save_layer_coverage(
    content_coverage: &Bounds,
    effect_transform: &Matrix,
    coverage_limit: &Rect,
    image_filter: Option<&dyn SourceCoverage>,
    flood_coverage: bool,
) -> Option<Rect> {
    let floods = flood_coverage || *content_coverage == Bounds::Unbounded;
    // The content coverage must be scaled by any image filter on the layer:
    // a filter that scales by one half needs twice the limit as input.
    if let Some(image_filter) = image_filter {
        let source_coverage_limit = image_filter.source_coverage(coverage_limit)?;
        if floods {
            return Some(source_coverage_limit);
        }
        // An animated filter changes its source coverage every frame, so the
        // intersection would change the texture's size every frame. Close
        // enough to the content, the content itself is used instead: always
        // correct, just larger than the limit needs. Content that does not
        // flood is bounded; empty content covers nothing.
        let transformed_coverage = effect_transform.map_rect(&content_coverage.rect()?);
        let intersected_coverage = transformed_coverage.intersect(&source_coverage_limit);
        if let Some(intersected) = intersected_coverage {
            if size_difference_under_threshold(&transformed_coverage, &intersected) {
                return Some(transformed_coverage);
            }
        }
        return intersected_coverage;
    }
    if floods {
        return Some(*coverage_limit);
    }
    let transformed_coverage = effect_transform.map_rect(&content_coverage.rect()?);
    let intersection = transformed_coverage.intersect(coverage_limit)?;
    // Nearly the limit's size: rounded up to the limit, so the texture is
    // reused.
    if size_difference_under_threshold(&intersection, coverage_limit) {
        return Some(*coverage_limit);
    }
    Some(intersection)
}

/// `size_difference_under_threshold` is Impeller's
/// `SizeDifferenceUnderThreshold`: `a` is within [`SIZE_THRESHOLD`] of `b`
/// in both dimensions.
fn size_difference_under_threshold(a: &Rect, b: &Rect) -> bool {
    (a.width - b.width).abs() / b.width < SIZE_THRESHOLD
        && (a.height - b.height).abs() / b.height < SIZE_THRESHOLD
}

/// Impeller's `save_layer_utils_unittests.cc`. Its image filters are matrix
/// filters, which valo does not have; [`MatrixFilter`] stands in with
/// `MatrixFilterContents::GetFilterSourceCoverage`.
#[cfg(test)]
mod tests {
    use super::*;

    /// A matrix image filter's source coverage, under no effect transform:
    /// the limit through the inverse matrix.
    struct MatrixFilter(Matrix);

    impl SourceCoverage for MatrixFilter {
        fn source_coverage(&self, output_limit: &Rect) -> Option<Rect> {
            Some(self.0.invert()?.map_rect(output_limit))
        }
    }

    fn scale(x: f32, y: f32, z: f32) -> MatrixFilter {
        MatrixFilter(Matrix::from_flutter_array(&[
            x, 0.0, 0.0, 0.0, //
            0.0, y, 0.0, 0.0, //
            0.0, 0.0, z, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ]))
    }

    fn translate(x: f32, y: f32) -> MatrixFilter {
        MatrixFilter(Matrix::translation(x, y))
    }

    fn ltrb(left: f32, top: f32, right: f32, bottom: f32) -> Rect {
        Rect::from_ltrb(left, top, right, bottom)
    }

    fn coverage(
        content: Rect,
        effect_transform: Matrix,
        limit: Rect,
        filter: Option<&dyn SourceCoverage>,
    ) -> Option<Rect> {
        compute_save_layer_coverage(
            &Bounds::of(content),
            &effect_transform,
            &limit,
            filter,
            false,
        )
    }

    fn assert_rect_near(actual: Option<Rect>, expected: Rect) {
        let actual = actual.expect("coverage");
        let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
        assert!(
            near(actual.x, expected.x)
                && near(actual.y, expected.y)
                && near(actual.right(), expected.right())
                && near(actual.bottom(), expected.bottom()),
            "{actual:?} != {expected:?}"
        );
    }

    #[test]
    fn simple_paint_computed_coverage() {
        let result = coverage(
            ltrb(0.0, 0.0, 10.0, 10.0),
            Matrix::IDENTITY,
            ltrb(0.0, 0.0, 2400.0, 1800.0),
            None,
        );
        assert_eq!(result, Some(ltrb(0.0, 0.0, 10.0, 10.0)));
    }

    #[test]
    fn backdrop_filter_computed_coverage() {
        let result = compute_save_layer_coverage(
            &Bounds::of(ltrb(0.0, 0.0, 10.0, 10.0)),
            &Matrix::IDENTITY,
            &ltrb(0.0, 0.0, 2400.0, 1800.0),
            None,
            true,
        );
        assert_eq!(result, Some(ltrb(0.0, 0.0, 2400.0, 1800.0)));
    }

    #[test]
    fn image_filter_computed_coverage() {
        let filter = scale(2.0, 2.0, 1.0);
        let result = coverage(
            ltrb(0.0, 0.0, 10.0, 10.0),
            Matrix::IDENTITY,
            ltrb(0.0, 0.0, 2400.0, 1800.0),
            Some(&filter),
        );
        assert_eq!(result, Some(ltrb(0.0, 0.0, 10.0, 10.0)));
    }

    #[test]
    fn image_filter_small_scale_computed_coverage_larger_than_bounds_limit() {
        let filter = scale(2.0, 2.0, 1.0);
        let result = coverage(
            ltrb(0.0, 0.0, 10.0, 10.0),
            Matrix::IDENTITY,
            ltrb(0.0, 0.0, 5.0, 5.0),
            Some(&filter),
        );
        assert_eq!(result, Some(ltrb(0.0, 0.0, 2.5, 2.5)));
    }

    #[test]
    fn image_filter_large_scale_computed_coverage_larger_than_bounds_limit() {
        let filter = scale(0.5, 0.5, 1.0);
        let result = coverage(
            ltrb(0.0, 0.0, 10.0, 10.0),
            Matrix::IDENTITY,
            ltrb(0.0, 0.0, 5.0, 5.0),
            Some(&filter),
        );
        assert_eq!(result, Some(ltrb(0.0, 0.0, 10.0, 10.0)));
    }

    #[test]
    fn disjoint_coverage() {
        let result = coverage(
            ltrb(200.0, 200.0, 210.0, 210.0),
            Matrix::IDENTITY,
            ltrb(0.0, 0.0, 100.0, 100.0),
            None,
        );
        assert_eq!(result, None);
    }

    #[test]
    fn disjoint_coverage_transformed_by_image_filter() {
        let filter = translate(-200.0, -200.0);
        let result = coverage(
            ltrb(200.0, 200.0, 210.0, 210.0),
            Matrix::IDENTITY,
            ltrb(0.0, 0.0, 100.0, 100.0),
            Some(&filter),
        );
        assert_eq!(result, Some(ltrb(200.0, 200.0, 210.0, 210.0)));
    }

    #[test]
    fn disjoint_coverage_transformed_by_ctm() {
        let result = coverage(
            ltrb(200.0, 200.0, 210.0, 210.0),
            Matrix::translation(-200.0, -200.0),
            ltrb(0.0, 0.0, 100.0, 100.0),
            None,
        );
        assert_eq!(result, Some(ltrb(0.0, 0.0, 10.0, 10.0)));
    }

    #[test]
    fn basic_empty_coverage() {
        let result = coverage(
            ltrb(0.0, 0.0, 0.0, 0.0),
            Matrix::IDENTITY,
            ltrb(0.0, 0.0, 2400.0, 1800.0),
            None,
        );
        assert_eq!(result, None);
    }

    #[test]
    fn image_filter_empty_coverage() {
        let filter = translate(-200.0, -200.0);
        let result = coverage(
            ltrb(0.0, 0.0, 0.0, 0.0),
            Matrix::IDENTITY,
            ltrb(0.0, 0.0, 2400.0, 1800.0),
            Some(&filter),
        );
        assert_eq!(result, None);
    }

    #[test]
    fn backdrop_filter_empty_coverage() {
        let result = compute_save_layer_coverage(
            &Bounds::of(ltrb(0.0, 0.0, 0.0, 0.0)),
            &Matrix::IDENTITY,
            &ltrb(0.0, 0.0, 2400.0, 1800.0),
            None,
            true,
        );
        assert_eq!(result, Some(ltrb(0.0, 0.0, 2400.0, 1800.0)));
    }

    /// Unbounded content, Impeller's maximum rect, covers the limit as a
    /// flood does, or what the filter needs of it.
    #[test]
    fn unbounded_content_covers_the_limit() {
        let limit = ltrb(0.0, 0.0, 2400.0, 1800.0);
        let shifted = Matrix::translation(100.0, 0.0);
        let unfiltered =
            compute_save_layer_coverage(&Bounds::Unbounded, &shifted, &limit, None, false);
        assert_eq!(unfiltered, Some(limit));
        let filter = scale(0.5, 0.5, 1.0);
        let filtered =
            compute_save_layer_coverage(&Bounds::Unbounded, &shifted, &limit, Some(&filter), false);
        assert_eq!(filtered, Some(ltrb(0.0, 0.0, 4800.0, 3600.0)));
    }

    #[test]
    fn flood_input_coverage() {
        let result = compute_save_layer_coverage(
            &Bounds::of(ltrb(0.0, 0.0, 0.0, 0.0)),
            &Matrix::IDENTITY,
            &ltrb(0.0, 0.0, 2400.0, 1800.0),
            None,
            true,
        );
        assert_eq!(result, Some(ltrb(0.0, 0.0, 2400.0, 1800.0)));
    }

    #[test]
    fn flood_input_coverage_with_image_filter() {
        let filter = scale(0.5, 0.5, 1.0);
        let result = compute_save_layer_coverage(
            &Bounds::of(ltrb(0.0, 0.0, 0.0, 0.0)),
            &Matrix::IDENTITY,
            &ltrb(0.0, 0.0, 2400.0, 1800.0),
            Some(&filter),
            true,
        );
        assert_eq!(result, Some(ltrb(0.0, 0.0, 4800.0, 3600.0)));
    }

    /// Even when a backdrop floods the input, an image filter that covers
    /// nothing culls the layer.
    #[test]
    fn flood_input_coverage_with_image_filter_with_no_coverage_produces_no_coverage() {
        let filter = scale(1.0, 1.0, 0.0);
        let result = compute_save_layer_coverage(
            &Bounds::of(ltrb(0.0, 0.0, 0.0, 0.0)),
            &Matrix::IDENTITY,
            &ltrb(0.0, 0.0, 2400.0, 1800.0),
            Some(&filter),
            true,
        );
        assert_eq!(result, None);
    }

    /// The source coverage limit is 0..90.9; the content is close enough to
    /// be used whole.
    #[test]
    fn coverage_limit_ignored_if_intersected_value_is_close_to_actual_coverage_smaller_with_image_filter(
    ) {
        let filter = scale(1.1, 1.1, 1.0);
        let result = coverage(
            ltrb(0.0, 0.0, 100.0, 100.0),
            Matrix::IDENTITY,
            ltrb(0.0, 0.0, 100.0, 100.0),
            Some(&filter),
        );
        assert_eq!(result, Some(ltrb(0.0, 0.0, 100.0, 100.0)));
    }

    /// The source coverage limit is 0..111.1; the intersection is the
    /// content.
    #[test]
    fn coverage_limit_ignored_if_intersected_value_is_close_to_actual_coverage_larger_with_image_filter(
    ) {
        let filter = scale(0.9, 0.9, 1.0);
        let result = coverage(
            ltrb(0.0, 0.0, 100.0, 100.0),
            Matrix::IDENTITY,
            ltrb(0.0, 0.0, 100.0, 100.0),
            Some(&filter),
        );
        assert_eq!(result, Some(ltrb(0.0, 0.0, 100.0, 100.0)));
    }

    #[test]
    fn coverage_limit_respected_if_substantially_different_from_content_coverage() {
        let filter = scale(2.0, 2.0, 1.0);
        let result = coverage(
            ltrb(0.0, 0.0, 1000.0, 1000.0),
            Matrix::IDENTITY,
            ltrb(0.0, 0.0, 100.0, 100.0),
            Some(&filter),
        );
        assert_eq!(result, Some(ltrb(0.0, 0.0, 50.0, 50.0)));
    }

    #[test]
    fn round_up_coverage_when_close_to_coverage_limit() {
        let result = coverage(
            ltrb(0.0, 0.0, 90.0, 90.0),
            Matrix::IDENTITY,
            ltrb(0.0, 0.0, 100.0, 100.0),
            None,
        );
        assert_eq!(result, Some(ltrb(0.0, 0.0, 100.0, 100.0)));
    }

    #[test]
    fn dont_round_up_coverage_when_not_close_to_coverage_limit_width() {
        let result = coverage(
            ltrb(0.0, 0.0, 50.0, 90.0),
            Matrix::IDENTITY,
            ltrb(0.0, 0.0, 100.0, 100.0),
            None,
        );
        assert_eq!(result, Some(ltrb(0.0, 0.0, 50.0, 90.0)));
    }

    #[test]
    fn dont_round_up_coverage_when_not_close_to_coverage_limit_height() {
        let result = coverage(
            ltrb(0.0, 0.0, 90.0, 50.0),
            Matrix::IDENTITY,
            ltrb(0.0, 0.0, 100.0, 100.0),
            None,
        );
        assert_eq!(result, Some(ltrb(0.0, 0.0, 90.0, 50.0)));
    }

    #[test]
    fn dont_round_up_coverage_when_not_close_to_coverage_limit_width_height() {
        let result = coverage(
            ltrb(0.0, 0.0, 50.0, 50.0),
            Matrix::IDENTITY,
            ltrb(0.0, 0.0, 100.0, 100.0),
            None,
        );
        assert_eq!(result, Some(ltrb(0.0, 0.0, 50.0, 50.0)));
    }

    /// Not one of Impeller's: a blur's source coverage grows the limit by
    /// its reach, and content inside that grown limit is kept whole.
    #[test]
    fn a_blurred_layer_keeps_content_its_blur_reaches_past_the_limit() {
        struct Outset(f32);
        impl SourceCoverage for Outset {
            fn source_coverage(&self, output_limit: &Rect) -> Option<Rect> {
                Some(output_limit.expand(self.0))
            }
        }
        let result = coverage(
            ltrb(-100.0, 0.0, 50.0, 50.0),
            Matrix::IDENTITY,
            ltrb(0.0, 0.0, 100.0, 100.0),
            Some(&Outset(8.0)),
        );
        assert_rect_near(result, ltrb(-8.0, 0.0, 50.0, 50.0));
    }
}
