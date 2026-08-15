import { commands } from "vitest/browser";
import { expect } from "vitest";
import {
  DEFAULT_THRESHOLDS,
  type DiffThresholds,
} from "./thresholds.js";
import type { CanvasScene } from "./scene.js";
import type { CompareResult } from "./browser-command.js";

declare module "vitest/browser" {
  interface BrowserCommands {
    compareCanvases(request: {
    label: string;
    scene: string;
    background: string;
    thresholds: DiffThresholds;
    }): Promise<CompareResult>;
  }
}

export async function expectCanvasParity(
  scene: CanvasScene,
  thresholds: Partial<DiffThresholds> = {},
): Promise<void> {
  const result = await commands.compareCanvases({
    label: scene.name,
    scene: JSON.stringify(scene, null, 2),
    background: scene.background,
    thresholds: { ...DEFAULT_THRESHOLDS, ...thresholds },
  });
  const diagnostics = [
    `${result.badPixels} bad pixels (${(result.badPixelRatio * 100).toFixed(3)}%)`,
    thresholds.maximumBoundsDelta === undefined && DEFAULT_THRESHOLDS.maximumBoundsDelta === null
      ? ""
      : result.boundsDelta === null
        ? "ink appears in only one renderer"
        : `${result.boundsDelta}px ink-bounds delta`,
    result.artifactDirectory ? `artifacts: ${result.artifactDirectory}` : "",
  ].filter(Boolean).join("; ");
  expect(result.passed, diagnostics).toBe(true);
}
