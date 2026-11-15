import { beforeAll, describe, test } from "vitest";
import { expectCanvasParity } from "../src/compare.js";
import { degenerateScenes, unblurredScenes } from "../src/degenerate-scenes.js";
import {
  createConformanceHarness,
  renderBoth,
  renderEach,
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

  // Valo keeps Impeller's blur, which leaves a draw as it is below
  // `smallestBlurringSigma` where Canvas2D blurs it: each is held to Canvas2D
  // drawing the same scene unblurred. Exactly, edges included: a blur this
  // small only moves edge pixels, which the default comparison sets aside as
  // antialiasing, and these edges sit on whole pixels, where both agree.
  for (const { scene, unblurred } of unblurredScenes) {
    test(`${scene.name} draws unblurred, as Impeller does`, async () => {
      await renderEach(harness, unblurred, scene);
      await expectCanvasParity(scene, {
        maximumInkOffset: 1,
        includeAntialiasing: true,
        maximumBadPixelRatio: 0,
      });
    });
  }
});
