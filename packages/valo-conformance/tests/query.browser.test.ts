import * as fc from "fast-check";
import { beforeAll, expect, test } from "vitest";
import {
  hitTestSceneArbitrary,
  textMeasurementSceneArbitrary,
} from "../src/generate.js";
import {
  createConformanceHarness,
  type ConformanceHarness,
} from "../src/harness.js";
import {
  comparableTextMetricKeys,
  compareHitTests,
  compareTextMetrics,
} from "../src/query-scene.js";
import { DEFAULT_METRIC_TOLERANCES } from "../src/thresholds.js";
import { FIXTURE_FONT_FAMILY, type CanvasCommand, type ReplayContext, type TextMetricKey } from "../src/scene.js";

// Queries answer in JavaScript, so a run costs a shaping pass rather than two
// screenshots and a decode. The budget is per-property wall clock either way.
const defaultRuns = 300;
const defaultSeed = 0x5eed;
const defaultTimeLimit = 20_000;

let harness: ConformanceHarness;
let nativeContext: ReplayContext;
let valoContext: ReplayContext;
let metricKeys: TextMetricKey[];

beforeAll(async () => {
  harness = await createConformanceHarness();
  const context = harness.nativeCanvas.getContext("2d");
  if (!context) throw new Error("Canvas2D is unavailable");
  nativeContext = context as unknown as ReplayContext;
  valoContext = harness.valoContext as unknown as ReplayContext;
  metricKeys = comparableTextMetricKeys(nativeContext);
});

test("the browser still reports the metrics this suite compares", () => {
  // A reminder rather than a rule: when Chrome starts reporting a metric Valo
  // already computes, this fails and the property should start covering it.
  expect(metricKeys).toEqual([
    "width",
    "actualBoundingBoxLeft",
    "actualBoundingBoxRight",
    "actualBoundingBoxAscent",
    "actualBoundingBoxDescent",
    "fontBoundingBoxAscent",
    "fontBoundingBoxDescent",
  ]);
});

// Text that puts no ink on the canvas still has a bounding box; the question
// is where it sits. These are named rather than generated so each answer is
// reported on its own instead of stopping the property at the first one.
for (const text of ["", " ", "   ", "\t", "\n"]) {
  test(`measureText(${JSON.stringify(text)}) metrics match Canvas2D`, () => {
    const scene = {
      name: `measure-text-ink-free-${JSON.stringify(text)}`,
      setup: [
        { type: "setFont", value: `24px '${FIXTURE_FONT_FAMILY}'` },
        { type: "setTextBaseline", value: "top" },
      ] satisfies CanvasCommand[],
      text,
    };
    const differences = compareTextMetrics(
      nativeContext,
      valoContext,
      scene,
      DEFAULT_METRIC_TOLERANCES,
      metricKeys,
    );
    expect(differences, describeMetrics(text, differences)).toEqual([]);
  });
}

// Letter spacing can outrun the glyph advances and leave the run narrower than
// nothing. The spec has no floor at zero, and Canvas2D reports the negative.
for (const [text, letterSpacing] of [["iiii", "-2px"], ["ii", "-4px"], ["W", "-40px"]] as const) {
  test(`measureText(${JSON.stringify(text)}) at ${letterSpacing} matches Canvas2D`, () => {
    const scene = {
      name: `measure-text-negative-advance-${text}`,
      setup: [
        { type: "setFont", value: `8px '${FIXTURE_FONT_FAMILY}'` },
        { type: "setTextSpacing", letter: letterSpacing, word: "0px" },
      ] satisfies CanvasCommand[],
      text,
    };
    const differences = compareTextMetrics(
      nativeContext,
      valoContext,
      scene,
      DEFAULT_METRIC_TOLERANCES,
      metricKeys,
    );
    expect(differences, describeMetrics(text, differences)).toEqual([]);
  });
}

test("generated measureText metrics match Canvas2D", async () => {
  await assertProperty(textMeasurementSceneArbitrary, (scene) => {
    const differences = compareTextMetrics(
      nativeContext,
      valoContext,
      scene,
      DEFAULT_METRIC_TOLERANCES,
      metricKeys,
    );
    expect(differences, describeMetrics(scene.text, differences)).toEqual([]);
  });
});

test("generated isPointInPath answers match Canvas2D", async () => {
  await assertProperty(hitTestSceneArbitrary, (scene) => {
    const differences = compareHitTests(nativeContext, valoContext, scene);
    expect(differences, describeHitTests(scene.rule, differences)).toEqual([]);
  });
});

async function assertProperty<Scene>(
  arbitrary: fc.Arbitrary<Scene>,
  check: (scene: Scene) => void,
): Promise<void> {
  await fc.assert(fc.property(arbitrary, check), {
    numRuns: environmentInteger(__VALO_FUZZ_RUNS__, defaultRuns),
    seed: environmentInteger(__VALO_FUZZ_SEED__, defaultSeed),
    interruptAfterTimeLimit: environmentInteger(__VALO_FUZZ_TIME_LIMIT__, defaultTimeLimit),
    markInterruptAsFailure: true,
  });
}

function describeMetrics(
  text: string,
  differences: ReturnType<typeof compareTextMetrics>,
): string {
  return `measureText(${JSON.stringify(text)}): ${differences
    .map((one) => `${one.key} Canvas2D ${one.native.toFixed(3)} vs Valo ${one.valo.toFixed(3)}`)
    .join("; ")}`;
}

function describeHitTests(
  rule: string,
  differences: ReturnType<typeof compareHitTests>,
): string {
  return `isPointInPath(${rule}): ${differences
    .map((one) => `(${one.point[0].toFixed(3)}, ${one.point[1].toFixed(3)}) Canvas2D ${one.native} vs Valo ${one.valo}`)
    .join("; ")}`;
}

function environmentInteger(value: string, fallback: number): number {
  if (value === "") return fallback;
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed <= 0) {
    throw new Error(`expected a positive integer, got ${value}`);
  }
  return parsed;
}
