import {
  replayCommands,
  TEXT_METRIC_KEYS,
  type CanvasCommand,
  type ReplayContext,
  type TextMetricKey,
} from "./scene.js";

/**
 * A query scene asks both renderers the same question and compares the answers
 * as numbers rather than as pixels. `setup` is replayed into each context first
 * so the query observes the same state, transform and path in both.
 */
export interface TextMeasurementScene {
  name: string;
  setup: CanvasCommand[];
  text: string;
}

export interface HitTestScene {
  name: string;
  setup: CanvasCommand[];
  rule: CanvasFillRule;
  points: [x: number, y: number][];
}

/**
 * A hit-test path as geometry rather than as commands, so the generator can
 * prove how far a sample point sits from the boundary before asking about it.
 *
 * Exactly on the boundary the fill rules are defined but a floating-point
 * evaluation of them is a coin flip, and two independent implementations owe
 * each other nothing there. Nearly on it is the interesting case and is a fair
 * question — but only if "nearly" is a number the generator controls, which
 * means measuring the clearance rather than assuming it.
 */
export interface HitTestGeometry {
  polygon: [x: number, y: number][];
  circle: { x: number; y: number; radius: number };
}

export function boundaryClearance(
  geometry: HitTestGeometry,
  [x, y]: [number, number],
): number {
  const { polygon, circle } = geometry;
  const edgeClearances = polygon.map((vertex, index) =>
    distanceToSegment([x, y], vertex, polygon[(index + 1) % polygon.length]!),
  );
  const circleClearance = Math.abs(Math.hypot(x - circle.x, y - circle.y) - circle.radius);
  return Math.min(circleClearance, ...edgeClearances);
}

function distanceToSegment(
  [x, y]: [number, number],
  [startX, startY]: [number, number],
  [endX, endY]: [number, number],
): number {
  const spanX = endX - startX;
  const spanY = endY - startY;
  const lengthSquared = spanX * spanX + spanY * spanY;
  const along = lengthSquared === 0
    ? 0
    : Math.min(1, Math.max(0, ((x - startX) * spanX + (y - startY) * spanY) / lengthSquared));
  return Math.hypot(x - (startX + along * spanX), y - (startY + along * spanY));
}

export interface MetricDifference {
  key: TextMetricKey;
  native: number;
  valo: number;
  delta: number;
}

export interface HitTestDifference {
  point: [x: number, y: number];
  native: boolean;
  valo: boolean;
}

/**
 * The metrics both sides report. Chrome keeps `emHeightAscent` and
 * `emHeightDescent` behind a flag, so there is no reference value for them
 * here even though Valo reports both; `comparableTextMetricKeys` reads the
 * browser rather than hard-coding that, so coverage widens on its own the day
 * Chrome ships them.
 */
export function comparableTextMetricKeys(nativeContext: ReplayContext): TextMetricKey[] {
  const reference = nativeContext.measureText("Ag") as Partial<Record<TextMetricKey, number>>;
  return TEXT_METRIC_KEYS.filter((key) => typeof reference[key] === "number");
}

export function compareTextMetrics(
  nativeContext: ReplayContext,
  valoContext: ReplayContext,
  scene: TextMeasurementScene,
  tolerances: MetricTolerances,
  keys: readonly TextMetricKey[],
): MetricDifference[] {
  const native = measure(nativeContext, scene);
  const valo = measure(valoContext, scene);
  return keys.flatMap((key) => {
    const delta = Math.abs(native[key] - valo[key]);
    return delta <= toleranceFor(key, tolerances)
      ? []
      : [{ key, native: native[key], valo: valo[key], delta }];
  });
}

export function compareHitTests(
  nativeContext: ReplayContext,
  valoContext: ReplayContext,
  scene: HitTestScene,
): HitTestDifference[] {
  const native = hitTest(nativeContext, scene);
  const valo = hitTest(valoContext, scene);
  return scene.points.flatMap((point, index) =>
    native[index] === valo[index]
      ? []
      : [{ point, native: native[index]!, valo: valo[index]! }],
  );
}

export interface MetricTolerances {
  /** Advance width, which both renderers derive from the same font tables. */
  width: number;
  /** Ink extents, which depend on how each rasterizer bounds its outlines. */
  boundingBox: number;
}

function toleranceFor(key: TextMetricKey, tolerances: MetricTolerances): number {
  return key === "width" ? tolerances.width : tolerances.boundingBox;
}

function measure(
  context: ReplayContext,
  scene: TextMeasurementScene,
): Record<TextMetricKey, number> {
  context.reset();
  replayCommands(context, scene.setup);
  const metrics = context.measureText(scene.text);
  return Object.fromEntries(
    TEXT_METRIC_KEYS.map((key) => [key, metrics[key]]),
  ) as Record<TextMetricKey, number>;
}

function hitTest(context: ReplayContext, scene: HitTestScene): boolean[] {
  context.reset();
  replayCommands(context, scene.setup);
  return scene.points.map(([x, y]) => context.isPointInPath(x, y, scene.rule));
}
