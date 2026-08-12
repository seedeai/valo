import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import type { BrowserCommand } from "vitest/node";
import { PNG } from "pngjs";
import {
  comparePngs,
  type DiffMetrics,
} from "./diff.js";
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
  nativeScreenshotMilliseconds: number;
  valoScreenshotMilliseconds: number;
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
  const nativeScreenshotStart = performance.now();
  const nativeBytes = await context.iframe
    .getByTestId("native-canvas")
    .screenshot({ animations: "disabled", scale: "css", type: "png" });
  const valoScreenshotStart = performance.now();
  const valoBytes = await context.iframe
    .getByTestId("valo-canvas")
    .screenshot({ animations: "disabled", scale: "css", type: "png" });
  const decodeStart = performance.now();
  const nativeImage = PNG.sync.read(nativeBytes);
  const valoImage = PNG.sync.read(valoBytes);
  const compareStart = performance.now();
  const { image, ...metrics } = comparePngs(
    nativeImage,
    valoImage,
    request.thresholds,
    request.background,
  );
  const artifactStart = performance.now();
  const baseTimings = {
    nativeScreenshotMilliseconds: valoScreenshotStart - nativeScreenshotStart,
    valoScreenshotMilliseconds: decodeStart - valoScreenshotStart,
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
  fs.writeFileSync(path.join(directory, "canvas2d.png"), nativeBytes);
  fs.writeFileSync(path.join(directory, "valo.png"), valoBytes);
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
  return result;
}

function safeName(value: string): string {
  return value.replaceAll(/[^a-zA-Z0-9._-]+/g, "-").replaceAll(/^-|-$/g, "").slice(0, 100);
}
