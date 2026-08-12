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
      await expectCanvasParity(
        scene,
        scene.name === "fixed-font-fill-and-stroke-text"
          ? { maximumBadPixelRatio: 0.04, maximumBoundsDelta: 2 }
          : {},
      );
    });
  }
});
