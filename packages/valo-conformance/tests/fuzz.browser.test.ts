import * as fc from "fast-check";
import { beforeAll, test } from "vitest";
import { expectCanvasParity } from "../src/compare.js";
import {
  canvasSceneArbitrary,
  fillTextSceneArbitrary,
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
  await assertScenes(canvasSceneArbitrary);
});

test("generated fixed-font fill text matches Canvas2D", async () => {
  await assertScenes(fillTextSceneArbitrary, {
    maximumBadPixelRatio: 0.025,
    maximumBoundsDelta: 3,
  });
});

test("generated fixed-font stroke text matches Canvas2D geometry", async () => {
  await assertScenes(strokeTextSceneArbitrary, {
    maximumBadPixelRatio: 0.05,
    maximumBoundsDelta: 3,
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
