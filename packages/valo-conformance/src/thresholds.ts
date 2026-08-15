export interface DiffThresholds {
  perceptualThreshold: number;
  includeAntialiasing: boolean;
  maximumBadPixelRatio: number;
  maximumBoundsDelta: number | null;
}

export const DEFAULT_THRESHOLDS: DiffThresholds = {
  perceptualThreshold: 0.1,
  includeAntialiasing: false,
  maximumBadPixelRatio: 0.01,
  maximumBoundsDelta: 2,
};
