import { diff as comparePixels } from "@blazediff/core";
import { PNG } from "pngjs";
import type { DiffThresholds } from "./thresholds.js";

export interface DiffMetrics {
  passed: boolean;
  badPixels: number;
  badPixelRatio: number;
  inkOffset: number | null;
  inkMass: number;
  width: number;
  height: number;
}

export interface ImageDiff extends DiffMetrics {
  image: PNG;
}

interface InkCentroid {
  x: number;
  y: number;
  mass: number;
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
  const background_ = parseHexColor(background);
  const nativeInk = inkCentroid(nativeImage, background_, thresholds.minimumInkDeviation);
  const valoInk = inkCentroid(valoImage, background_, thresholds.minimumInkDeviation);
  const inkOffset = compareInkPlacement(nativeInk, valoInk, thresholds);
  const inkMass = Math.min(nativeInk.mass, valoInk.mass);
  return {
    passed: badPixelRatio <= thresholds.maximumBadPixelRatio
      && (thresholds.maximumInkOffset === null
        || (inkOffset !== null && inkOffset <= thresholds.maximumInkOffset)),
    badPixels,
    badPixelRatio,
    inkOffset,
    inkMass,
    width,
    height,
    image,
  };
}

/**
 * How far the two renders' ink sits apart, in pixels; `null` when one drew
 * something and the other drew nothing at all.
 *
 * Every pixel is weighted by how far it stands out from the background instead
 * of being thresholded into a bounding box. A box is decided entirely by its
 * outermost pixels, so a shadow's faint tail — or a `multiply` blend landing a
 * few levels off a dark background — moves it several pixels for a difference
 * no viewer could see, and the pixel comparison calls those renders identical.
 * A weighted centroid averages over the whole shape instead: steady on faint
 * content, and on solid content sensitive to displacements below a pixel,
 * which a box can never resolve.
 *
 * Where there is too little ink to place — a destructive composite that leaves
 * the canvas near its background, an `opacity(1%)` fill — this reports no
 * offset and leaves the verdict to the pixel comparison, which is the check
 * that can still say something meaningful about renders that faint.
 */
function compareInkPlacement(
  native: InkCentroid,
  valo: InkCentroid,
  thresholds: DiffThresholds,
): number | null {
  if (native.mass === 0 && valo.mass === 0) return 0;
  if (native.mass === 0 || valo.mass === 0) return null;
  if (native.mass < thresholds.minimumInkMass || valo.mass < thresholds.minimumInkMass) return 0;
  return Math.hypot(native.x - valo.x, native.y - valo.y);
}

function inkCentroid(
  image: PNG,
  background: readonly [number, number, number, number],
  minimumDeviation: number,
): InkCentroid {
  let mass = 0;
  let weightedX = 0;
  let weightedY = 0;
  for (let y = 0; y < image.height; y += 1) {
    for (let x = 0; x < image.width; x += 1) {
      const offset = (y * image.width + x) * 4;
      const deviation = channelDeviation(image, offset, background);
      if (deviation <= minimumDeviation) continue;
      mass += deviation;
      weightedX += deviation * x;
      weightedY += deviation * y;
    }
  }
  return mass === 0 ? { x: 0, y: 0, mass } : { x: weightedX / mass, y: weightedY / mass, mass };
}

function channelDeviation(
  image: PNG,
  offset: number,
  background: readonly [number, number, number, number],
): number {
  let deviation = 0;
  for (let channel = 0; channel < background.length; channel += 1) {
    deviation = Math.max(
      deviation,
      Math.abs(image.data[offset + channel]! - background[channel]!),
    );
  }
  return deviation;
}

function parseHexColor(value: string): [number, number, number, number] {
  const match = /^#([0-9a-f]{6})$/i.exec(value);
  if (!match) throw new Error(`ink placement requires a #rrggbb background, got ${value}`);
  const integer = Number.parseInt(match[1]!, 16);
  return [integer >> 16, (integer >> 8) & 0xff, integer & 0xff, 255];
}

function assertMatchingDimensions(nativeImage: PNG, valoImage: PNG): void {
  if (nativeImage.width === valoImage.width && nativeImage.height === valoImage.height) return;
  throw new Error(
    `canvas dimensions differ: Canvas2D ${nativeImage.width}×${nativeImage.height}, Valo ${valoImage.width}×${valoImage.height}`,
  );
}
