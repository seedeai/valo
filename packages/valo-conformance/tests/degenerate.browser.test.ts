import { beforeAll, describe, test } from "vitest";
import { expectCanvasParity } from "../src/compare.js";
import { degenerateScenes } from "../src/degenerate-scenes.js";
import {
  createConformanceHarness,
  renderBoth,
  type ConformanceHarness,
} from "../src/harness.js";

describe("degenerate and extreme arguments", () => {
  let harness: ConformanceHarness;

  beforeAll(async () => {
    harness = await createConformanceHarness();
  });

  for (const scene of degenerateScenes) {
    test(scene.name, async () => {
      await renderBoth(harness, scene);
      // A fixed scene has no distribution behind it, so it is held to the
      // placement the geometry actually implies rather than to a tolerance
      // sized for a generator's tail: every passing case here lands on an ink
      // offset of exactly zero.
      await expectCanvasParity(scene, { maximumInkOffset: 1 });
    });
  }
});
