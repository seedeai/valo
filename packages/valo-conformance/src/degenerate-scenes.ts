import { FIXTURE_FONT_FAMILY, type CanvasCommand, type CanvasScene } from "./scene.js";

/**
 * Values the spec defines an answer for and that the generated scenes cannot
 * reach: zeroes, negatives, non-finite numbers, sigmas past where Valo changes
 * strategy. Each case is named so a divergence reports itself instead of
 * stopping a property on its first counterexample.
 *
 * Every case pairs its degenerate input with a witness draw, because the
 * failure that matters is rarely "the degenerate call drew the wrong thing" —
 * it is "the degenerate call poisoned the state, the path or the frame that
 * came after it".
 */

const background = "#101218";
const fullTurn = Math.PI * 2;

function scene(name: string, commands: CanvasCommand[]): CanvasScene {
  return { name, background, commands };
}

const witnessFill: CanvasCommand[] = [
  { type: "setFillColor", color: "#35a7ff" },
  { type: "fillRect", x: 40, y: 40, width: 48, height: 48 },
];

function stroke(cap: CanvasLineCap): CanvasCommand {
  return { type: "setStroke", width: 8, cap, join: "miter", miterLimit: 4 };
}

function horizontalStroke(cap: CanvasLineCap, intervals: number[]): CanvasCommand[] {
  return [
    { type: "setStrokeColor", color: "#ff4f79" },
    stroke(cap),
    { type: "setLineDash", intervals, offset: 0 },
    { type: "beginPath" },
    { type: "moveTo", x: 20, y: 64 },
    { type: "lineTo", x: 108, y: 64 },
    { type: "stroke" },
  ];
}

const zeroLengthGeometry: CanvasScene[] = [
  ...(["butt", "round", "square"] as const).map((cap) =>
    scene(`zero-length-subpath-${cap}-cap`, [
      { type: "setStrokeColor", color: "#ff4f79" },
      stroke(cap),
      { type: "beginPath" },
      { type: "moveTo", x: 64, y: 64 },
      { type: "lineTo", x: 64, y: 64 },
      { type: "stroke" },
    ]),
  ),
  scene("zero-size-stroke-rect", [
    { type: "setStrokeColor", color: "#ff4f79" },
    stroke("butt"),
    { type: "strokeRect", x: 64, y: 64, width: 0, height: 0 },
  ]),
  scene("zero-width-stroke-rect", [
    { type: "setStrokeColor", color: "#ff4f79" },
    stroke("butt"),
    { type: "strokeRect", x: 40, y: 30, width: 0, height: 60 },
  ]),
  scene("zero-radius-arc-filled", [
    { type: "setFillColor", color: "#ff4f79" },
    { type: "beginPath" },
    { type: "arc", x: 64, y: 64, radius: 0, start: 0, end: fullTurn, counterclockwise: false },
    { type: "fill", rule: "nonzero" },
  ]),
  scene("zero-radius-ellipse-stroked", [
    { type: "setStrokeColor", color: "#ff4f79" },
    stroke("round"),
    { type: "beginPath" },
    {
      type: "ellipse",
      x: 64, y: 64, radiusX: 0, radiusY: 0, rotation: 0,
      start: 0, end: fullTurn, counterclockwise: false,
    },
    { type: "stroke" },
  ]),
  scene("empty-path-fill-and-stroke", [
    { type: "setFillColor", color: "#ff4f79" },
    { type: "setStrokeColor", color: "#d2ff45" },
    stroke("round"),
    { type: "beginPath" },
    { type: "fill", rule: "nonzero" },
    { type: "stroke" },
    ...witnessFill,
  ]),
];

// A zero-length ON interval paints only where the cap gives it a shape: nothing
// for butt, a dot for round and square.
const zeroLengthDashes: CanvasScene[] = [
  ...(["butt", "round", "square"] as const).map((cap) =>
    scene(`zero-length-dash-on-${cap}-cap`, horizontalStroke(cap, [0, 6])),
  ),
  scene("zero-length-dash-off", horizontalStroke("butt", [6, 0])),
  scene("all-zero-dash-paints-solid", horizontalStroke("butt", [0, 0])),
  scene("zero-length-dash-in-odd-pattern", horizontalStroke("butt", [0, 6, 4])),
  // Each dash is shorter than the cap it wears, so hundreds of them overlap
  // into what is effectively a solid line drawn many times over.
  ...(["butt", "round", "square"] as const).map((cap) =>
    scene(`dash-shorter-than-its-${cap}-cap`, horizontalStroke(cap, [1, 1])),
  ),
];

const negativeExtents: CanvasScene[] = [
  scene("negative-fill-rect", [
    { type: "setFillColor", color: "#ff4f79" },
    { type: "fillRect", x: 90, y: 90, width: -50, height: -40 },
  ]),
  scene("negative-clear-rect", [
    { type: "setFillColor", color: "#ff4f79" },
    { type: "fillRect", x: 20, y: 20, width: 88, height: 88 },
    { type: "clearRect", x: 90, y: 90, width: -50, height: -40 },
  ]),
  scene("negative-stroke-rect", [
    { type: "setStrokeColor", color: "#ff4f79" },
    stroke("butt"),
    { type: "strokeRect", x: 90, y: 90, width: -50, height: -40 },
  ]),
  scene("negative-round-rect", [
    { type: "setFillColor", color: "#ff4f79" },
    { type: "beginPath" },
    { type: "roundRect", x: 94, y: 74, width: -60, height: -40, radii: [10] },
    { type: "fill", rule: "nonzero" },
  ]),
  scene("round-rect-radii-exceed-size", [
    { type: "setFillColor", color: "#ff4f79" },
    { type: "beginPath" },
    { type: "roundRect", x: 34, y: 34, width: 60, height: 40, radii: [40, 40, 40, 40] },
    { type: "fill", rule: "nonzero" },
  ]),
  scene("zero-width-fill-rect", [
    { type: "setFillColor", color: "#ff4f79" },
    { type: "fillRect", x: 40, y: 30, width: 0, height: 60 },
    ...witnessFill,
  ]),
];

// Every one of these is defined as a silent no-op that leaves the path, the
// state and the transform exactly as it found them.
const nonFiniteArguments: CanvasScene[] = [
  scene("non-finite-line-to-keeps-path", [
    { type: "setFillColor", color: "#ff4f79" },
    { type: "beginPath" },
    { type: "moveTo", x: 30, y: 30 },
    { type: "lineTo", x: Number.NaN, y: 60 },
    { type: "lineTo", x: 90, y: 30 },
    { type: "lineTo", x: 90, y: 90 },
    { type: "closePath" },
    { type: "fill", rule: "nonzero" },
  ]),
  scene("non-finite-rect-keeps-path", [
    { type: "setFillColor", color: "#ff4f79" },
    { type: "beginPath" },
    { type: "rect", x: 10, y: 10, width: Number.POSITIVE_INFINITY, height: 40 },
    { type: "rect", x: 30, y: 30, width: 50, height: 50 },
    { type: "fill", rule: "nonzero" },
  ]),
  scene("non-finite-bezier-keeps-path", [
    { type: "setFillColor", color: "#ff4f79" },
    { type: "beginPath" },
    { type: "moveTo", x: 30, y: 90 },
    {
      type: "bezierCurveTo",
      control1X: Number.NaN, control1Y: 20, control2X: 80, control2Y: 20,
      x: 90, y: 90,
    },
    { type: "lineTo", x: 90, y: 40 },
    { type: "lineTo", x: 30, y: 40 },
    { type: "closePath" },
    { type: "fill", rule: "nonzero" },
  ]),
  scene("non-finite-fill-rect-is-a-no-op", [
    { type: "setFillColor", color: "#ff4f79" },
    { type: "fillRect", x: Number.NaN, y: 20, width: 40, height: 40 },
    ...witnessFill,
  ]),
  scene("non-finite-translate-is-a-no-op", [
    { type: "translate", x: Number.NaN, y: 10 },
    ...witnessFill,
  ]),
  scene("non-finite-scale-is-a-no-op", [
    { type: "scale", x: Number.POSITIVE_INFINITY, y: 1 },
    ...witnessFill,
  ]),
  scene("non-finite-rotate-is-a-no-op", [
    { type: "rotate", radians: Number.NaN },
    ...witnessFill,
  ]),
  scene("non-finite-transform-is-a-no-op", [
    { type: "transform", matrix: [1, 0, 0, 1, Number.POSITIVE_INFINITY, 0] },
    ...witnessFill,
  ]),
  scene("non-finite-set-transform-is-a-no-op", [
    { type: "translate", x: 20, y: 20 },
    { type: "setTransform", matrix: [1, 0, 0, Number.NaN, 0, 0] },
    ...witnessFill,
  ]),
  scene("non-finite-global-alpha-is-a-no-op", [
    { type: "setGlobalAlpha", alpha: Number.NaN },
    ...witnessFill,
  ]),
  scene("out-of-range-global-alpha-is-a-no-op", [
    { type: "setGlobalAlpha", alpha: 4 },
    ...witnessFill,
  ]),
  scene("non-finite-line-width-keeps-previous", [
    { type: "setStrokeColor", color: "#ff4f79" },
    stroke("butt"),
    { type: "setStroke", width: Number.NaN, cap: "butt", join: "miter", miterLimit: 4 },
    { type: "beginPath" },
    { type: "moveTo", x: 20, y: 64 },
    { type: "lineTo", x: 108, y: 64 },
    { type: "stroke" },
  ]),
  scene("non-finite-shadow-offset-is-a-no-op", [
    { type: "setShadow", color: "#d2ff45", blur: 6, offsetX: Number.NaN, offsetY: 6 },
    ...witnessFill,
  ]),
  scene("non-finite-text-position-is-a-no-op", [
    { type: "setFont", value: `24px '${FIXTURE_FONT_FAMILY}'` },
    { type: "setFillColor", color: "#ff4f79" },
    { type: "fillText", text: "gone", x: Number.NaN, y: 60 },
    { type: "fillText", text: "kept", x: 20, y: 100 },
  ]),
  scene("non-finite-dash-interval-keeps-previous", [
    { type: "setStrokeColor", color: "#ff4f79" },
    stroke("butt"),
    { type: "setLineDash", intervals: [12, 6], offset: 0 },
    { type: "setLineDash", intervals: [12, Number.NaN], offset: 0 },
    { type: "beginPath" },
    { type: "moveTo", x: 20, y: 64 },
    { type: "lineTo", x: 108, y: 64 },
    { type: "stroke" },
  ]),
];

/**
 * Sigma sweeps across `blur_scale`'s switch to a downsampled blur, which is at
 * 4√2 ≈ 5.657 for a CSS filter and at twice that for `shadowBlur`, since a
 * shadow's sigma is half its blur.
 */
const blurSigmas: CanvasScene[] = [
  ...[4, 5.65, 5.7, 8, 12, 24, 64].map((pixels) =>
    scene(`filter-blur-${pixels}px`, [
      { type: "setFilter", value: `blur(${pixels}px)` },
      { type: "setFillColor", color: "#ff4f79" },
      { type: "fillRect", x: 40, y: 40, width: 48, height: 48 },
    ]),
  ),
  ...[8, 11.2, 11.4, 24, 48].map((pixels) =>
    scene(`shadow-blur-${pixels}px`, [
      { type: "setShadow", color: "#d2ff45", blur: pixels, offsetX: 0, offsetY: 0 },
      { type: "setFillColor", color: "#ff4f79" },
      { type: "fillRect", x: 48, y: 48, width: 32, height: 32 },
    ]),
  ),
];

const extremeCoordinates: CanvasScene[] = [
  ...[1e3, 1e5, 1e7].map((far) =>
    scene(`path-reaching-${far}-off-canvas`, [
      { type: "setFillColor", color: "#ff4f79" },
      { type: "beginPath" },
      { type: "moveTo", x: -far, y: 40 },
      { type: "lineTo", x: far, y: 50 },
      { type: "lineTo", x: far, y: 90 },
      { type: "lineTo", x: -far, y: 80 },
      { type: "closePath" },
      { type: "fill", rule: "nonzero" },
    ]),
  ),
  ...[100, 10_000, 1_000_000].map((factor) =>
    scene(`scale-up-by-${factor}`, [
      { type: "setFillColor", color: "#ff4f79" },
      { type: "scale", x: factor, y: factor },
      {
        type: "fillRect",
        x: 30 / factor, y: 30 / factor, width: 60 / factor, height: 60 / factor,
      },
    ]),
  ),
  ...[100, 10_000].map((factor) =>
    scene(`scale-down-by-${factor}`, [
      { type: "setFillColor", color: "#ff4f79" },
      { type: "scale", x: 1 / factor, y: 1 / factor },
      {
        type: "fillRect",
        x: 30 * factor, y: 30 * factor, width: 60 * factor, height: 60 * factor,
      },
    ]),
  ),
  scene("degenerate-scale-collapses-the-draw", [
    { type: "setFillColor", color: "#ff4f79" },
    { type: "save" },
    { type: "scale", x: 0, y: 1 },
    { type: "fillRect", x: 20, y: 20, width: 60, height: 60 },
    { type: "restore" },
    ...witnessFill,
  ]),
];

export const degenerateScenes: CanvasScene[] = [
  ...zeroLengthGeometry,
  ...zeroLengthDashes,
  ...negativeExtents,
  ...nonFiniteArguments,
  ...blurSigmas,
  ...extremeCoordinates,
];
