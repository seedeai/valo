import { diff as comparePixels } from "@blazediff/core";
import { PNG } from "pngjs";
import type { DiffThresholds } from "./thresholds.js";

export interface DiffMetrics {
  passed: boolean;
  badPixels: number;
  badPixelRatio: number;
  boundsDelta: number | null;
  width: number;
  height: number;
}

export interface ImageDiff extends DiffMetrics {
  image: PNG;
}

export function comparePngs(
  nativeImage: PNG,
  valoImage: PNG,
  thresholds: DiffThresholds,
  background: string,
): ImageDiff {
  assertMatchingDimensions(nativeImage, valoImage);
  const { width, height } = nativeImage;
  const image = new PNG({ width, height });
  const badPixels = comparePixels(
    nativeImage.data,
    valoImage.data,
    image.data,
    width,
    height,
    {
      threshold: thresholds.perceptualThreshold,
      includeAA: thresholds.includeAntialiasing,
      diffColor: [255, 32, 64],
      aaColor: [255, 196, 32],
    },
  );
  const badPixelRatio = badPixels / (width * height);
  const boundsDelta = compareInkBounds(nativeImage, valoImage, parseHexColor(background));
  return {
    passed: badPixelRatio <= thresholds.maximumBadPixelRatio
      && (thresholds.maximumBoundsDelta === null
        || (boundsDelta !== null && boundsDelta <= thresholds.maximumBoundsDelta)),
    badPixels,
    badPixelRatio,
    boundsDelta,
    width,
    height,
    image,
  };
}

function compareInkBounds(
  nativeImage: PNG,
  valoImage: PNG,
  background: readonly [number, number, number, number],
): number | null {
  const nativeBounds = inkBounds(nativeImage, background);
  const valoBounds = inkBounds(valoImage, background);
  if (!nativeBounds && !valoBounds) return 0;
  if (!nativeBounds || !valoBounds) return null;
  const edges = [0, 0, nativeImage.width - 1, nativeImage.height - 1] as const;
  const comparableDeltas = nativeBounds.flatMap((value, index) => {
    const valoValue = valoBounds[index]!;
    return value === edges[index] || valoValue === edges[index]
      ? []
      : [Math.abs(value - valoValue)];
  });
  return comparableDeltas.length === 0 ? 0 : Math.max(...comparableDeltas);
}

function inkBounds(
  image: PNG,
  background: readonly [number, number, number, number],
): [number, number, number, number] | undefined {
  let left = image.width;
  let top = image.height;
  let right = -1;
  let bottom = -1;
  for (let y = 0; y < image.height; y += 1) {
    for (let x = 0; x < image.width; x += 1) {
      const offset = (y * image.width + x) * 4;
      const differs = background.some(
        (channel, index) => Math.abs(image.data[offset + index]! - channel) > 2,
      );
      if (!differs) continue;
      left = Math.min(left, x);
      top = Math.min(top, y);
      right = Math.max(right, x);
      bottom = Math.max(bottom, y);
    }
  }
  return right < left ? undefined : [left, top, right, bottom];
}

function parseHexColor(value: string): [number, number, number, number] {
  const match = /^#([0-9a-f]{6})$/i.exec(value);
  if (!match) throw new Error(`ink-bounds comparison requires a #rrggbb background, got ${value}`);
  const integer = Number.parseInt(match[1]!, 16);
  return [integer >> 16, (integer >> 8) & 0xff, integer & 0xff, 255];
}

function assertMatchingDimensions(nativeImage: PNG, valoImage: PNG): void {
  if (nativeImage.width === valoImage.width && nativeImage.height === valoImage.height) return;
  throw new Error(
    `canvas dimensions differ: Canvas2D ${nativeImage.width}×${nativeImage.height}, Valo ${valoImage.width}×${valoImage.height}`,
  );
}
