import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import type { BrowserCommand } from "vitest/node";
import { PNG } from "pngjs";
import {
  comparePngs,
  type DiffMetrics,
} from "./diff.js";
import { CANVAS_PAIR_TEST_ID } from "./scene.js";
import type { DiffThresholds } from "./thresholds.js";

export interface CompareRequest {
  label: string;
  scene: string;
  background: string;
  thresholds: DiffThresholds;
}

export interface CompareResult extends DiffMetrics {
  artifactDirectory?: string;
  timings: ComparisonTimings;
}

export interface ComparisonTimings {
  screenshotMilliseconds: number;
  decodeMilliseconds: number;
  compareMilliseconds: number;
  artifactMilliseconds: number;
}

const artifactRoot = fileURLToPath(new URL("../artifacts/", import.meta.url));

export const compareCanvases: BrowserCommand<[request: CompareRequest]> = async (
  context,
  request,
) => {
  if (context.provider.name !== "playwright") {
    throw new Error(`canvas comparison requires Playwright, got ${context.provider.name}`);
  }
  const screenshotStart = performance.now();
  const pairBytes = await context.iframe
    .getByTestId(CANVAS_PAIR_TEST_ID)
    .screenshot({ animations: "disabled", scale: "css", type: "png" });
  const decodeStart = performance.now();
  const [nativeImage, valoImage] = splitCanvasPair(PNG.sync.read(pairBytes));
  const compareStart = performance.now();
  const { image, ...metrics } = comparePngs(
    nativeImage,
    valoImage,
    request.thresholds,
    request.background,
  );
  const artifactStart = performance.now();
  const baseTimings = {
    screenshotMilliseconds: decodeStart - screenshotStart,
    decodeMilliseconds: compareStart - decodeStart,
    compareMilliseconds: artifactStart - compareStart,
  };
  if (metrics.passed) {
    return reportProfile(request.label, {
      ...metrics,
      timings: { ...baseTimings, artifactMilliseconds: 0 },
    });
  }

  const directory = path.join(artifactRoot, safeName(request.label));
  fs.mkdirSync(directory, { recursive: true });
  fs.writeFileSync(path.join(directory, "canvas2d.png"), PNG.sync.write(nativeImage));
  fs.writeFileSync(path.join(directory, "valo.png"), PNG.sync.write(valoImage));
  fs.writeFileSync(path.join(directory, "diff.png"), PNG.sync.write(image));
  fs.writeFileSync(path.join(directory, "scene.json"), `${request.scene}\n`);
  return reportProfile(request.label, {
    ...metrics,
    artifactDirectory: directory,
    timings: {
      ...baseTimings,
      artifactMilliseconds: performance.now() - artifactStart,
    },
  });
};

function reportProfile(label: string, result: CompareResult): CompareResult {
  if (process.env.VALO_CONFORMANCE_PROFILE === "1") {
    console.table({ [label]: result.timings });
  }
  // The comparison thresholds are only defensible against the spread of values
  // that passing scenes actually produce, so make that spread observable.
  if (process.env.VALO_CONFORMANCE_METRICS === "1") {
    process.stdout.write(
      `metric ${label} badRatio=${result.badPixelRatio.toFixed(5)} inkOffset=${result.inkOffset === null ? "none" : result.inkOffset.toFixed(3)} inkMass=${Math.round(result.inkMass)}\n`,
    );
  }
  return result;
}

function safeName(value: string): string {
  return value.replaceAll(/[^a-zA-Z0-9._-]+/g, "-").replaceAll(/^-|-$/g, "").slice(0, 100);
}

/** Halves the paired capture back into the two renders it holds. */
function splitCanvasPair(pair: PNG): [native: PNG, valo: PNG] {
  if (pair.width % 2 !== 0) {
    throw new Error(`the canvas pair should capture at an even width, got ${pair.width}`);
  }
  const width = pair.width / 2;
  const native = new PNG({ width, height: pair.height });
  const valo = new PNG({ width, height: pair.height });
  PNG.bitblt(pair, native, 0, 0, width, pair.height, 0, 0);
  PNG.bitblt(pair, valo, width, 0, width, pair.height, 0, 0);
  return [native, valo];
}
