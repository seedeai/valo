import { beforeAll, describe, test } from "vitest";
import { expectCanvasParity } from "../src/compare.js";
import { curatedScenes } from "../src/curated-scenes.js";
import {
  createConformanceHarness,
  renderBoth,
  type ConformanceHarness,
} from "../src/harness.js";

describe("Canvas2D parity", () => {
  let harness: ConformanceHarness;

  beforeAll(async () => {
    harness = await createConformanceHarness();
  });

  for (const scene of curatedScenes) {
    test(scene.name, async () => {
      await renderBoth(harness, scene);
      // Fixed scenes carry no sampling tail, so placement is held to a pixel.
      // The text scene is the exception on both counts: three glyph runs at
      // different sizes, where the two rasterizers share the least machinery.
      await expectCanvasParity(
        scene,
        scene.name === "fixed-font-fill-and-stroke-text"
          ? { maximumBadPixelRatio: 0.04, maximumInkOffset: 3 }
          : { maximumInkOffset: 1 },
      );
    });
  }
});
