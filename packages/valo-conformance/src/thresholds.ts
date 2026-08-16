export interface DiffThresholds {
  perceptualThreshold: number;
  includeAntialiasing: boolean;
  maximumBadPixelRatio: number;
  /**
   * How far the two renders' weighted ink centroids may sit apart, in pixels.
   *
   * A centroid's precision falls off as the ink thins out — across 9000
   * measured comparisons the offset's p99 runs from 1.6px at the low end of
   * the mass range to 0.19px at the high end — and 6 clears the whole spread
   * with room (the largest offset any passing scene produced was 4.0). It
   * stays useful because this is a coarse guard, not a precise one: the pixel
   * comparison is what resolves small displacements, and every divergence
   * this harness has actually found moved a centroid by 13px or more, or moved
   * it from something to nothing.
   */
  maximumInkOffset: number | null;
  /**
   * How far a channel must sit from the background before a pixel counts as
   * ink. Matches the ±3-per-channel tolerance the golden tests treat as the
   * same colour: below it, an `opacity(1%)` fill composites to within one
   * rounding step of the background and whether it registers at all is a coin
   * flip that the two renderers land on opposite sides of.
   */
  minimumInkDeviation: number;
  /**
   * Total weight a render needs before its ink can be placed at all. Below
   * this the scene is faint enough that where its centroid sits says more
   * about rounding than about geometry, so placement defers to the pixel
   * comparison. 4096 is roughly a 16-pixel patch of solid colour.
   */
  minimumInkMass: number;
}

export const DEFAULT_THRESHOLDS: DiffThresholds = {
  perceptualThreshold: 0.1,
  includeAntialiasing: false,
  maximumBadPixelRatio: 0.01,
  maximumInkOffset: 6,
  minimumInkDeviation: 3,
  minimumInkMass: 4096,
};

/** How far two `TextMetrics` readings may differ before the metric fails. */
export interface MetricTolerances {
  /** Advance width: both renderers read the same `hmtx` entries. */
  width: number;
  /** Ink and font extents, which depend on how each side bounds outlines. */
  boundingBox: number;
}

export const DEFAULT_METRIC_TOLERANCES: MetricTolerances = {
  width: 0.5,
  boundingBox: 1,
};
