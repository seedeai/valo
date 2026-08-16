import * as fc from "fast-check";
import { beforeAll, test } from "vitest";
import { expectCanvasParity } from "../src/compare.js";
import {
  canvasSceneArbitrary,
  fillTextSceneArbitrary,
  stateStackSceneArbitrary,
  dashedStrokeSceneArbitrary,
  solidStrokeSceneArbitrary,
  strokeTextSceneArbitrary,
} from "../src/generate.js";
import {
  createConformanceHarness,
  renderBoth,
  type ConformanceHarness,
} from "../src/harness.js";

const defaultRuns = 40;
const defaultSeed = 0x5eed;
const defaultTimeLimit = 20_000;

let harness: ConformanceHarness;

beforeAll(async () => {
  harness = await createConformanceHarness();
});

test("generated supported commands match Canvas2D", async () => {
  await assertScenes(canvasSceneArbitrary, {
    // 1200 passing scenes peak at 2.0% differing pixels (p99 0.84%). Sub-pixel
    // edges are what put it there: an integer-only pool held under 1%, because
    // a pixel-aligned edge has exact coverage and nothing to round differently.
    maximumBadPixelRatio: 0.025,
  });
});

test("generated solid stroke styling matches Canvas2D", async () => {
  await assertScenes(solidStrokeSceneArbitrary, {
    // 1200 passing solid scenes peak at 2.4% differing pixels (p99 1.1%),
    // which is a wide stroke's share of edge pixels. A miter spike that should
    // not be there covers around 3% on its own, so this has to stay under that
    // — which is why joins are tested on a polyline: a flattened curve alone
    // would spend the whole budget before the corner was reached.
    maximumBadPixelRatio: 0.03,
  });
});

test("generated dashed stroke styling matches Canvas2D", async () => {
  await assertScenes(dashedStrokeSceneArbitrary, {
    // 4000 passing dashed scenes peak at 4.5% differing pixels (p99 0.46%): a
    // dash pattern puts a cap edge every few pixels and every one of them is an
    // independent chance to disagree. Placement is switched off rather than
    // loosened — a sparse dashed curve carries little ink, so its centroid is a
    // noisy estimate (p99 1.3px, worst 11px), and a stroke drawn in the wrong
    // place shows up in the pixel count long before it would show up there.
    maximumBadPixelRatio: 0.06,
    maximumInkOffset: null,
  });
});

test("generated save/restore nesting matches Canvas2D", async () => {
  await assertScenes(stateStackSceneArbitrary);
});

test("generated fixed-font fill text matches Canvas2D", async () => {
  await assertScenes(fillTextSceneArbitrary, {
    // 1100 passing scenes peak at 2.6% differing pixels (p99 1.4%): glyph
    // rasterization is where the two engines share the least machinery.
    maximumBadPixelRatio: 0.035,
  });
});

test("generated fixed-font stroke text matches Canvas2D geometry", async () => {
  await assertScenes(strokeTextSceneArbitrary, {
    // 1200 passing scenes peak at 3.4% differing pixels (p99 2.4%) — the same
    // glyph outlines as fill text, plus a stroke's worth of edge around each.
    maximumBadPixelRatio: 0.045,
  });
});

async function assertScenes(
  arbitrary: fc.Arbitrary<import("../src/scene.js").CanvasScene>,
  thresholds: Partial<import("../src/thresholds.js").DiffThresholds> = {},
): Promise<void> {
  await fc.assert(
    fc.asyncProperty(arbitrary, async (scene) => {
      await renderBoth(harness, scene);
      await expectCanvasParity(scene, thresholds);
    }),
    {
      numRuns: environmentInteger(__VALO_FUZZ_RUNS__, defaultRuns),
      seed: environmentInteger(__VALO_FUZZ_SEED__, defaultSeed),
      interruptAfterTimeLimit: environmentInteger(
        __VALO_FUZZ_TIME_LIMIT__,
        defaultTimeLimit,
      ),
      markInterruptAsFailure: true,
    },
  );
}

function environmentInteger(value: string, fallback: number): number {
  if (value === "") return fallback;
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed <= 0) {
    throw new Error(`expected a positive integer, got ${value}`);
  }
  return parsed;
}
