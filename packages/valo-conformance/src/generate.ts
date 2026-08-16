import * as fc from "fast-check";
import {
  FIXTURE_FONT_FAMILY,
  type CanvasCommand,
  type CanvasScene,
  type ImageArguments,
} from "./scene.js";
import {
  boundaryClearance,
  type HitTestGeometry,
  type HitTestScene,
  type TextMeasurementScene,
} from "./query-scene.js";

type CommandBlock = CanvasCommand[];

const colors = [
  "#ff4f79",
  "#d2ff45",
  "#6954ff",
  "#35a7ff",
  "#ff9f43",
  "rgba(238,244,255,0.72)",
] as const;

const backgrounds = ["#101218", "#17151e", "#111822"] as const;
const color = fc.constantFrom(...colors);

// Prime, so a step lands on a mantissa the renderer cannot have special-cased
// the way it can special-case a half or a quarter.
const subpixelSteps = 4093;

// Whole numbers keep their own weight because a pixel-aligned edge has exact
// coverage: a difference there is a real bug rather than an antialiasing
// tie-break. It is also where fast-check shrinks to, so a counterexample that
// survives shrinking is one that does not depend on sub-pixel coverage at all.
const subpixelOffset = fc.oneof(
  { weight: 1, arbitrary: fc.constant(0) },
  { weight: 3, arbitrary: fc.integer({ min: 1, max: subpixelSteps - 1 }) },
);

/**
 * A length or position in pixels. Sub-pixel edges are where two rasterizers
 * most plausibly disagree, and an integer-only generator can never reach one.
 */
function measure(min: number, max: number): fc.Arbitrary<number> {
  const wholeNumber = fc.integer({ min: Math.ceil(min), max: Math.floor(max) });
  return fc
    .tuple(wholeNumber, subpixelOffset)
    .map(([whole, step]) => Math.min(max, whole + step / subpixelSteps));
}

/**
 * A dimensionless or angular quantity — a scale factor, a rotation, an alpha —
 * where the useful resolution is a fraction of the whole range rather than a
 * fraction of a pixel.
 */
function amount(min: number, max: number): fc.Arbitrary<number> {
  return fc
    .integer({ min: 0, max: subpixelSteps })
    .map((step) => min + ((max - min) * step) / subpixelSteps);
}

const coordinate = measure(-12, 116);
const extent = measure(8, 72);
const fillRule = fc.constantFrom<CanvasFillRule>("nonzero", "evenodd");

/**
 * `miterLimit` is a threshold rather than a style: a join renders as a spike
 * below it and as a bevel above it, so what matters is landing near wherever a
 * given join angle flips. One fixed value can only ever see one side.
 */
const miterLimit = fc.oneof(
  { weight: 3, arbitrary: measure(1, 12) },
  { weight: 1, arbitrary: fc.constantFrom(1, Math.SQRT2, 2, 4, 5, 10) },
);

/**
 * Dashing has three axes the fixed `[9, 5]` never moved: pattern length, phase,
 * and interval size. Odd lengths are their own case because the spec
 * concatenates the list with itself, so on and off swap every cycle.
 *
 * Intervals stay at or above a pixel. Below that a dasher walks the contour one
 * epsilon at a time, which turns a fuzz run into a hang rather than a finding;
 * at exactly zero it hits a known divergence that would fail every run and mask
 * the rest of the pool, so `degenerate-scenes.ts` covers that case by name.
 */
const lineDash = fc.record({
  intervals: fc.array(measure(1, 18), { minLength: 0, maxLength: 5 }),
  offset: measure(-12, 24),
});

const filterFunction = fc.oneof(
  // Sigma stops below 4√2, where Valo's downsampled blur path takes over and
  // diverges. Widening it here would fail every run and mask the other eight
  // functions; `degenerate-scenes.ts` covers the large-sigma band by name.
  measure(0, 5).map((pixels) => `blur(${pixels}px)`),
  measure(0, 200).map((percent) => `brightness(${percent}%)`),
  measure(0, 200).map((percent) => `contrast(${percent}%)`),
  measure(0, 100).map((percent) => `grayscale(${percent}%)`),
  measure(-180, 180).map((degrees) => `hue-rotate(${degrees}deg)`),
  measure(0, 100).map((percent) => `invert(${percent}%)`),
  measure(0, 100).map((percent) => `opacity(${percent}%)`),
  measure(0, 250).map((percent) => `saturate(${percent}%)`),
  measure(0, 100).map((percent) => `sepia(${percent}%)`),
);

/**
 * A chain that never shrinks hides a bug in any one function behind the eight
 * others, so length is an axis too — down to the empty chain, `none`.
 */
const filterValue = fc
  .array(filterFunction, { minLength: 0, maxLength: 4 })
  .map((functions) => (functions.length === 0 ? "none" : functions.join(" ")));

const alpha = amount(0.25, 1);
const compositeOperation = fc.constantFrom<GlobalCompositeOperation>(
  "source-over",
  "copy",
  "destination-over",
  "source-in",
  "destination-in",
  "source-out",
  "destination-out",
  "source-atop",
  "destination-atop",
  "xor",
  "lighter",
  "screen",
  "overlay",
  "darken",
  "lighten",
  "color-dodge",
  "color-burn",
  "hard-light",
  "soft-light",
  "difference",
  "exclusion",
  "multiply",
  "hue",
  "saturation",
  "color",
  "luminosity",
);

const filledRectangle = fc
  .record({ color, alpha, x: coordinate, y: coordinate, width: extent, height: extent })
  .map(({ color: fill, alpha: opacity, x, y, width, height }): CommandBlock => [
    { type: "save" },
    { type: "setFillColor", color: fill },
    { type: "setGlobalAlpha", alpha: opacity },
    { type: "fillRect", x, y, width, height },
    { type: "restore" },
  ]);

const roundedRectangle = fc
  .record({
    color,
    x: coordinate,
    y: coordinate,
    width: extent,
    height: extent,
    radii: fc.tuple(
      measure(0, 24),
      measure(0, 24),
      measure(0, 24),
      measure(0, 24),
    ),
  })
  .map(({ color: fill, x, y, width, height, radii }): CommandBlock => [
    { type: "save" },
    { type: "setFillColor", color: fill },
    { type: "beginPath" },
    { type: "roundRect", x, y, width, height, radii: [...radii] },
    { type: "fill", rule: "nonzero" },
    { type: "restore" },
  ]);

const curvedShape = fc
  .record({
    color,
    startX: coordinate,
    startY: coordinate,
    control1X: coordinate,
    control1Y: coordinate,
    control2X: coordinate,
    control2Y: coordinate,
    endX: coordinate,
    endY: coordinate,
  })
  .map((shape): CommandBlock => [
    { type: "save" },
    { type: "setFillColor", color: shape.color },
    { type: "beginPath" },
    { type: "moveTo", x: shape.startX, y: shape.startY },
    {
      type: "bezierCurveTo",
      control1X: shape.control1X,
      control1Y: shape.control1Y,
      control2X: shape.control2X,
      control2Y: shape.control2Y,
      x: shape.endX,
      y: shape.endY,
    },
    { type: "lineTo", x: shape.startX + 8, y: shape.startY + 12 },
    { type: "closePath" },
    { type: "fill", rule: "evenodd" },
    { type: "restore" },
  ]);

const ellipse = fc
  .record({
    color,
    x: measure(12, 116),
    y: measure(12, 116),
    radiusX: measure(3, 36),
    radiusY: measure(3, 36),
    rotation: amount(-1, 1),
  })
  .map((shape): CommandBlock => [
    { type: "save" },
    { type: "setFillColor", color: shape.color },
    { type: "beginPath" },
    {
      type: "ellipse",
      x: shape.x,
      y: shape.y,
      radiusX: shape.radiusX,
      radiusY: shape.radiusY,
      rotation: shape.rotation,
      start: 0,
      end: Math.PI * 2,
      counterclockwise: false,
    },
    { type: "fill", rule: "nonzero" },
    { type: "restore" },
  ]);

function strokedPath(dashed: boolean): fc.Arbitrary<CommandBlock> {
  return fc
  .record({
    color,
    // Wide enough that the join style is worth pixels. A miter spike on a
    // 2px stroke is a handful of pixels either way, which the bad-pixel ratio
    // absorbs — so a narrow-only pool cannot see `miterLimit` at all.
    width: measure(1, 16),
    cap: fc.constantFrom<CanvasLineCap>("butt", "round", "square"),
    // Weighted to miter because miter is the only join carrying a parameter,
    // and `miterLimit` is only observable where a miter is long enough to be
    // worth clipping.
    join: fc.oneof(
      { weight: 2, arbitrary: fc.constant<CanvasLineJoin>("miter") },
      { weight: 1, arbitrary: fc.constant<CanvasLineJoin>("bevel") },
      { weight: 1, arbitrary: fc.constant<CanvasLineJoin>("round") },
    ),
    miterLimit,
    dash: lineDash,
    x0: coordinate,
    y0: coordinate,
    x1: coordinate,
    y1: coordinate,
    x2: coordinate,
    y2: coordinate,
    // The turn the path takes at its corner, not where the corner leads. A
    // free endpoint almost always gives a gentle turn, and a gentle turn's
    // miter is short enough that no limit in range ever clips it — which is
    // how a fixed `miterLimit` of 5 went unnoticed for the whole pool's life.
    // A miter is 1.4 half-widths long at a right angle and 11.5 at 170°, so the
    // limits in range only bite past about 140° — hence the weight there. Past
    // 170° the path doubles back and a wide stroke overlaps its own body, which
    // is a self-intersection question rather than a join question.
    cornerTurn: fc.oneof(
      { weight: 1, arbitrary: amount(Math.PI / 2, (140 * Math.PI) / 180) },
      { weight: 3, arbitrary: amount((140 * Math.PI) / 180, (170 * Math.PI) / 180) },
    ),
    cornerTurnsLeft: fc.boolean(),
    cornerLength: measure(6, 40),
  })
  .map((path): CommandBlock => [
    { type: "save" },
    { type: "setStrokeColor", color: path.color },
    {
      type: "setStroke",
      width: path.width,
      cap: path.cap,
      join: path.join,
      miterLimit: path.miterLimit,
    },
    {
      type: "setLineDash",
      // A dash shorter than the cap it wears is not a dash: a square cap adds
      // half the stroke width at each end, so a 1px interval on an 8px stroke
      // paints a 9px blob and hundreds of them overlap into a solid line. What
      // that renders to is an accumulation of coverage over hundreds of edges,
      // which is the one shape here where two rasterizers differ by percent
      // rather than by pixels. `degenerate-scenes.ts` names it instead.
      intervals: dashed
        ? path.dash.intervals.map((interval) => Math.max(interval, path.width))
        : [],
      offset: path.dash.offset,
    },
    { type: "beginPath" },
    { type: "moveTo", x: path.x0, y: path.y0 },
    // Dashing is interesting along a curve — the pattern has to be walked by
    // arc length — while caps, joins and the miter limit are about the corner
    // and nothing else. Keeping them on a polyline means the only thing that
    // can differ is the corner, instead of a whole flattened curve's worth of
    // offset polyline swamping it.
    ...(dashed
      ? ([{
          type: "quadraticCurveTo",
          controlX: path.x1,
          controlY: path.y1,
          x: path.x2,
          y: path.y2,
        }] satisfies CanvasCommand[])
      : ([{ type: "lineTo", x: path.x2, y: path.y2 }] satisfies CanvasCommand[])),
    {
      type: "lineTo",
      ...cornerEndpoint({
        ...path,
        arrivalX: dashed ? path.x1 : path.x0,
        arrivalY: dashed ? path.y1 : path.y0,
      }),
    },
    { type: "stroke" },
    { type: "restore" },
  ]);
}

const solidStrokedPath = strokedPath(false);
const dashedStrokedPath = strokedPath(true);

/**
 * Where the closing segment ends if the path turns by `cornerTurn` where it
 * arrives at the corner. A curve arrives along the tangent from its control
 * point and a straight segment along itself, so the caller says which.
 */
function cornerEndpoint(path: {
  arrivalX: number;
  arrivalY: number;
  x2: number;
  y2: number;
  cornerTurn: number;
  cornerTurnsLeft: boolean;
  cornerLength: number;
}): { x: number; y: number } {
  const arrival = Math.atan2(path.y2 - path.arrivalY, path.x2 - path.arrivalX);
  const turn = path.cornerTurnsLeft ? -path.cornerTurn : path.cornerTurn;
  const departure = arrival + turn;
  return {
    x: path.x2 + path.cornerLength * Math.cos(departure),
    y: path.y2 + path.cornerLength * Math.sin(departure),
  };
}

const gradientStroke = fc
  .record({
    first: color,
    second: color,
    width: measure(1, 9),
  })
  .map((path): CommandBlock => [
    { type: "save" },
    {
      type: "setStrokeLinearGradient",
      points: [12, 18, 112, 106],
      stops: [
        { offset: 0, color: path.first },
        { offset: 1, color: path.second },
      ],
    },
    { type: "setStroke", width: path.width, cap: "round", join: "round", miterLimit: 4 },
    { type: "beginPath" },
    { type: "moveTo", x: 12, y: 96 },
    { type: "bezierCurveTo", control1X: 32, control1Y: 8, control2X: 82, control2Y: 118, x: 112, y: 26 },
    { type: "stroke" },
    { type: "restore" },
  ]);

const transformedRectangle = fc
  .record({
    color,
    x: measure(24, 104),
    y: measure(24, 104),
    width: measure(8, 48),
    height: measure(8, 48),
    rotation: amount(-1.2, 1.2),
    scaleX: amount(0.5, 1.6),
    scaleY: amount(0.5, 1.6),
  })
  .map((shape): CommandBlock => [
    { type: "save" },
    { type: "translate", x: shape.x, y: shape.y },
    { type: "rotate", radians: shape.rotation },
    { type: "scale", x: shape.scaleX, y: shape.scaleY },
    { type: "setFillColor", color: shape.color },
    {
      type: "fillRect",
      x: -shape.width / 2,
      y: -shape.height / 2,
      width: shape.width,
      height: shape.height,
    },
    { type: "restore" },
  ]);

const affineRectangle = fc
  .record({
    color,
    shearX: amount(-0.4, 0.4),
    shearY: amount(-0.4, 0.4),
    translationX: measure(20, 60),
    translationY: measure(20, 60),
  })
  .map((shape): CommandBlock => [
    { type: "save" },
    {
      type: "transform",
      matrix: [1, shape.shearY, shape.shearX, 1, shape.translationX, shape.translationY],
    },
    { type: "setFillColor", color: shape.color },
    { type: "fillRect", x: 0, y: 0, width: 44, height: 36 },
    { type: "resetTransform" },
    { type: "setStrokeColor", color: shape.color },
    { type: "setStroke", width: 2, cap: "butt", join: "miter", miterLimit: 10 },
    { type: "strokeRect", x: 8, y: 92, width: 112, height: 24 },
    { type: "restore" },
  ]);

const assignedTransformRectangle = fc
  .record({ color, translationX: coordinate, translationY: coordinate })
  .map((shape): CommandBlock => [
    { type: "save" },
    {
      type: "setTransform",
      matrix: [1, 0.2, -0.15, 1, shape.translationX, shape.translationY],
    },
    { type: "setFillColor", color: shape.color },
    { type: "fillRect", x: 0, y: 0, width: 34, height: 28 },
    { type: "restore" },
  ]);

const clippedGradient = fc
  .record({
    first: color,
    second: color,
    x: coordinate,
    y: coordinate,
    width: extent,
    height: extent,
    rule: fillRule,
  })
  .map((shape): CommandBlock => [
    { type: "save" },
    { type: "beginPath" },
    {
      type: "roundRect",
      x: shape.x,
      y: shape.y,
      width: shape.width,
      height: shape.height,
      radii: [12],
    },
    // The two subpaths overlap and wind the same way, so `nonzero` unions them
    // while `evenodd` punches the intersection out. Without a second subpath
    // the clip rule would be unobservable.
    {
      type: "arc",
      x: shape.x + shape.width / 2,
      y: shape.y + shape.height / 2,
      radius: Math.min(shape.width, shape.height) / 2 + 6,
      start: 0,
      end: Math.PI * 2,
      counterclockwise: false,
    },
    { type: "clip", rule: shape.rule },
    {
      type: "setFillLinearGradient",
      points: [shape.x, shape.y, shape.x + shape.width, shape.y + shape.height],
      stops: [
        { offset: 0, color: shape.first },
        { offset: 1, color: shape.second },
      ],
    },
    { type: "fillRect", x: 0, y: 0, width: 128, height: 128 },
    { type: "restore" },
  ]);

const radialGradient = fc
  .record({
    first: color,
    second: color,
    x: measure(24, 104),
    y: measure(24, 104),
    radius: measure(12, 48),
  })
  .map((shape): CommandBlock => [
    { type: "save" },
    {
      type: "setFillRadialGradient",
      circles: [shape.x, shape.y, 0, shape.x, shape.y, shape.radius],
      stops: [
        { offset: 0, color: shape.first },
        { offset: 1, color: shape.second },
      ],
    },
    {
      type: "fillRect",
      x: shape.x - shape.radius,
      y: shape.y - shape.radius,
      width: shape.radius * 2,
      height: shape.radius * 2,
    },
    { type: "restore" },
  ]);

const arcShape = fc
  .record({
    color,
    x: measure(16, 112),
    y: measure(16, 112),
    radius: measure(4, 34),
    start: amount(-3, 3),
    sweep: amount(0.4, 6.2),
    counterclockwise: fc.boolean(),
  })
  .map((shape): CommandBlock => [
    { type: "save" },
    { type: "setFillColor", color: shape.color },
    { type: "beginPath" },
    { type: "moveTo", x: shape.x, y: shape.y },
    {
      type: "arc",
      x: shape.x,
      y: shape.y,
      radius: shape.radius,
      start: shape.start,
      end: shape.counterclockwise ? shape.start - shape.sweep : shape.start + shape.sweep,
      counterclockwise: shape.counterclockwise,
    },
    { type: "closePath" },
    { type: "fill", rule: "nonzero" },
    { type: "restore" },
  ]);

const arcToShape = fc
  .record({
    color,
    startX: measure(8, 44),
    startY: measure(72, 116),
    cornerX: measure(40, 88),
    cornerY: measure(16, 64),
    endX: measure(84, 120),
    endY: measure(72, 116),
    radius: measure(2, 28),
  })
  .map((shape): CommandBlock => [
    { type: "save" },
    { type: "setStrokeColor", color: shape.color },
    { type: "setStroke", width: 4, cap: "round", join: "round", miterLimit: 4 },
    { type: "beginPath" },
    { type: "moveTo", x: shape.startX, y: shape.startY },
    {
      type: "arcTo",
      x1: shape.cornerX,
      y1: shape.cornerY,
      x2: shape.endX,
      y2: shape.endY,
      radius: shape.radius,
    },
    { type: "lineTo", x: shape.endX, y: shape.endY },
    { type: "stroke" },
    { type: "restore" },
  ]);

const conicGradient = fc
  .record({
    first: color,
    second: color,
    angle: amount(-Math.PI, Math.PI),
    x: measure(42, 86),
    y: measure(42, 86),
  })
  .map((shape): CommandBlock => [
    { type: "save" },
    {
      type: "setFillConicGradient",
      center: [shape.angle, shape.x, shape.y],
      stops: [
        { offset: 0, color: shape.first },
        { offset: 0.5, color: shape.second },
        { offset: 1, color: shape.first },
      ],
    },
    { type: "fillRect", x: 12, y: 12, width: 104, height: 104 },
    { type: "restore" },
  ]);

const compositedRectangles = fc
  .record({ first: color, second: color, operation: compositeOperation })
  .map((shape): CommandBlock => [
    { type: "save" },
    { type: "setFillColor", color: shape.first },
    { type: "fillRect", x: 18, y: 18, width: 58, height: 58 },
    { type: "setComposite", operation: shape.operation },
    { type: "setFillColor", color: shape.second },
    { type: "fillRect", x: 48, y: 48, width: 62, height: 62 },
    { type: "restore" },
  ]);

const clearedRectangle = fc
  .record({ color, x: measure(8, 64), y: measure(8, 64) })
  .map((shape): CommandBlock => [
    { type: "save" },
    { type: "setFillColor", color: shape.color },
    { type: "fillRect", x: shape.x, y: shape.y, width: 56, height: 48 },
    { type: "clearRect", x: shape.x + 14, y: shape.y + 12, width: 24, height: 20 },
    { type: "restore" },
  ]);

const shadowedRectangle = fc
  .record({
    color,
    shadow: color,
    operation: compositeOperation,
    blur: measure(0, 8),
    offsetX: measure(-6, 8),
    offsetY: measure(-6, 8),
  })
  .map((shape): CommandBlock => [
    { type: "save" },
    {
      type: "setShadow",
      color: shape.shadow,
      blur: shape.blur,
      offsetX: shape.offsetX,
      offsetY: shape.offsetY,
    },
    { type: "setComposite", operation: shape.operation },
    { type: "setFillColor", color: shape.color },
    { type: "fillRect", x: 34, y: 34, width: 52, height: 48 },
    { type: "restore" },
  ]);

const filteredRectangle = fc
  .record({ color, filter: filterValue })
  .map((shape): CommandBlock => [
    { type: "save" },
    { type: "setFilter", value: shape.filter },
    { type: "setFillColor", color: shape.color },
    { type: "fillRect", x: 30, y: 32, width: 62, height: 54 },
    { type: "restore" },
  ]);

const shadowedGradient = fc
  .record({
    first: color,
    second: color,
    shadow: color,
    blur: measure(0, 8),
  })
  .map((shape): CommandBlock => [
    { type: "save" },
    { type: "setShadow", color: shape.shadow, blur: shape.blur, offsetX: 5, offsetY: 6 },
    {
      type: "setFillLinearGradient",
      points: [24, 24, 92, 80],
      stops: [
        { offset: 0, color: shape.first },
        { offset: 1, color: shape.second },
      ],
    },
    { type: "fillRect", x: 24, y: 24, width: 68, height: 56 },
    { type: "restore" },
  ]);

const fixedImage = fc
  .record({
    x: measure(0, 104),
    y: measure(0, 104),
    width: measure(8, 48),
    height: measure(8, 48),
    sourceX: measure(0, 8),
    sourceY: measure(0, 8),
    sourceWidth: measure(4, 8),
    sourceHeight: measure(4, 8),
    signature: fc.constantFrom<2 | 4 | 8>(2, 4, 8),
    smoothing: fc.boolean(),
  })
  .map((image): CommandBlock => {
    const argumentsList: ImageArguments = image.signature === 2
      ? [image.x, image.y]
      : image.signature === 4
        ? [image.x, image.y, image.width, image.height]
        : [
            image.sourceX,
            image.sourceY,
            image.sourceWidth,
            image.sourceHeight,
            image.x,
            image.y,
            image.width,
            image.height,
          ];
    return [
      { type: "save" },
      { type: "setImageSmoothing", enabled: image.smoothing },
      { type: "drawImage", arguments: argumentsList },
      { type: "restore" },
    ];
  });

const imagePattern = fc
  .record({
    scale: amount(0.5, 1.6),
    x: measure(0, 12),
    y: measure(0, 12),
  })
  .map((pattern): CommandBlock => [
    { type: "save" },
    {
      type: "setFillPattern",
      repetition: "repeat",
      transform: [pattern.scale, 0, 0, pattern.scale, pattern.x, pattern.y],
    },
    { type: "fillRect", x: 16, y: 16, width: 88, height: 72 },
    { type: "restore" },
  ]);

const fixedTextParameters = fc
  .record({
    color,
    text: fc.constantFrom("Valo", "Canvas", "fuzz test", "A quick fox", "Valo ", "a\tb", "a\nb"),
    size: fc.integer({ min: 12, max: 34 }),
    // Keep the glyph body on-canvas even after alignment and the generated
    // transform. Edge clipping is covered by dedicated scenes and otherwise
    // turns bounds comparison into a one-pixel visibility lottery.
    x: measure(44, 92),
    y: measure(32, 92),
    align: fc.constantFrom<CanvasTextAlign>("left", "center", "right", "start", "end"),
    baseline: fc.constantFrom<CanvasTextBaseline>(
      "top",
      "hanging",
      "middle",
      "alphabetic",
      "ideographic",
      "bottom",
    ),
    maximumWidth: fc.option(measure(30, 110), { nil: undefined }),
    letterSpacing: measure(-1, 2),
    wordSpacing: measure(0, 3),
    translationX: measure(-8, 8),
    translationY: measure(-8, 8),
    rotation: amount(-0.2, 0.2),
    scaleX: amount(0.8, 1.25),
    scaleY: amount(0.8, 1.25),
    clipped: fc.boolean(),
    shadowed: fc.boolean(),
    composite: fc.constantFrom<GlobalCompositeOperation>(
      "source-over",
      "copy",
      "destination-out",
      "xor",
      "multiply",
    ),
  });

function fixedText(stroke: boolean): fc.Arbitrary<CommandBlock> {
  return fixedTextParameters.map((text): CommandBlock => [
    { type: "save" },
    { type: "translate", x: text.translationX, y: text.translationY },
    { type: "rotate", radians: text.rotation },
    { type: "scale", x: text.scaleX, y: text.scaleY },
    ...(text.clipped
      ? ([
          { type: "beginPath" },
          { type: "rect", x: 8, y: 8, width: 112, height: 112 },
          { type: "clip", rule: "nonzero" },
        ] satisfies CanvasCommand[])
      : []),
    ...(text.shadowed
      ? ([
          { type: "setShadow", color: "rgba(15,20,28,0.72)", blur: 5, offsetX: 4, offsetY: 5 },
        ] satisfies CanvasCommand[])
      : []),
    { type: "setComposite", operation: text.composite },
    { type: "setFont", value: `${text.size}px '${FIXTURE_FONT_FAMILY}'` },
    { type: "setTextAlign", value: text.align },
    { type: "setTextBaseline", value: text.baseline },
    {
      type: "setTextSpacing",
      letter: `${text.letterSpacing}px`,
      word: `${text.wordSpacing}px`,
    },
    ...(stroke
      ? ([
          { type: "setStrokeColor", color: text.color },
          { type: "setStroke", width: 2, cap: "round", join: "round", miterLimit: 4 },
          {
            type: "strokeText",
            text: text.text,
            x: text.x,
            y: text.y,
            ...(text.maximumWidth === undefined ? {} : { maxWidth: text.maximumWidth }),
          },
        ] satisfies CanvasCommand[])
      : ([
          { type: "setFillColor", color: text.color },
          {
            type: "fillText",
            text: text.text,
            x: text.x,
            y: text.y,
            ...(text.maximumWidth === undefined ? {} : { maxWidth: text.maximumWidth }),
          },
        ] satisfies CanvasCommand[])),
    { type: "restore" },
  ]);
}

const fixedFillText = fixedText(false);
const fixedStrokeText = fixedText(true);

const commandBlock = fc.oneof(
  filledRectangle,
  roundedRectangle,
  curvedShape,
  ellipse,
  solidStrokedPath,
  dashedStrokedPath,
  gradientStroke,
  transformedRectangle,
  affineRectangle,
  assignedTransformRectangle,
  clippedGradient,
  radialGradient,
  arcShape,
  arcToShape,
  conicGradient,
  compositedRectangles,
  clearedRectangle,
  shadowedRectangle,
  shadowedGradient,
  filteredRectangle,
  fixedImage,
  imagePattern,
);

export const canvasSceneArbitrary: fc.Arbitrary<CanvasScene> = fc
  .record({
    background: fc.constantFrom(...backgrounds),
    blocks: fc.array(commandBlock, { minLength: 1, maxLength: 7 }),
  })
  .map(({ background, blocks }) => {
    const commands = blocks.flat();
    return {
      name: `fuzz-${hashScene(background, commands)}`,
      background,
      commands,
    };
  });

export const fillTextSceneArbitrary: fc.Arbitrary<CanvasScene> = fc
  .record({
    background: fc.constantFrom(...backgrounds),
    block: fixedFillText,
  })
  .map(({ background, block }) => ({
    name: `fill-text-fuzz-${hashScene(background, block)}`,
    background,
    commands: block,
  }));

export const strokeTextSceneArbitrary: fc.Arbitrary<CanvasScene> = fc
  .record({
    background: fc.constantFrom(...backgrounds),
    block: fixedStrokeText,
  })
  .map(({ background, block }) => ({
    name: `stroke-text-fuzz-${hashScene(background, block)}`,
    background,
    commands: block,
  }));

/**
 * Strokes get their own properties rather than one twenty-first of the shared
 * pool: cap, join, miter limit, width, dash pattern and dash phase are six
 * independent axes, and a threshold like `miterLimit` only shows itself when a
 * run happens to draw a sharp miter join — too rare a coincidence to leave to
 * a pool this wide.
 *
 * Solid and dashed are separate because their noise floors are decades apart.
 * A dashed curve puts a cap edge every few pixels and every edge is a fresh
 * chance to disagree, so its floor is set by how many dashes fit on the path.
 * Holding a solid stroke to that floor would hide exactly what a solid stroke
 * is here to show: one miter spike, which is small but has no business
 * differing at all.
 */
export const solidStrokeSceneArbitrary: fc.Arbitrary<CanvasScene> = strokeScenes(
  "solid-stroke-fuzz",
  solidStrokedPath,
);

export const dashedStrokeSceneArbitrary: fc.Arbitrary<CanvasScene> = strokeScenes(
  "dashed-stroke-fuzz",
  dashedStrokedPath,
);

function strokeScenes(
  prefix: string,
  block: fc.Arbitrary<CommandBlock>,
): fc.Arbitrary<CanvasScene> {
  return fc
    .record({ background: fc.constantFrom(...backgrounds), block })
    .map((scene) => ({
      name: `${prefix}-${hashScene(scene.background, scene.block)}`,
      background: scene.background,
      commands: scene.block,
    }));
}

/**
 * Save/restore stress. The interesting failures are not "the fill went to the
 * wrong place" but "a scope leaked": a clip that outlives its `restore`, a
 * `restore` with nothing to pop that discards live state, a `reset` that leaves
 * the transform behind.
 *
 * `reset` clears the bitmap as well as the state, and the two renderers
 * establish their background differently — Canvas2D by painting it, Valo by
 * clearing the frame to it — so each `reset` repaints the background to put
 * both back on the same footing before the comparison means anything.
 */
const stateStackBlock = fc
  .record({
    color,
    background: fc.constantFrom(...backgrounds),
    depth: fc.integer({ min: 1, max: 24 }),
    surplusRestores: fc.integer({ min: 0, max: 4 }),
    step: measure(0.5, 3),
    clipped: fc.boolean(),
    resetMidScene: fc.boolean(),
  })
  .map((stack): { background: string; commands: CommandBlock } => {
    const scopes = Array.from({ length: stack.depth }, (_, level): CanvasCommand[] => [
      { type: "save" },
      { type: "translate", x: stack.step, y: stack.step / 2 },
      ...(stack.clipped
        ? ([
            { type: "beginPath" },
            {
              type: "rect",
              x: 2,
              y: 2,
              width: 124 - level * 2,
              height: 124 - level * 2,
            },
            { type: "clip", rule: level % 2 === 0 ? "nonzero" : "evenodd" },
          ] satisfies CanvasCommand[])
        : []),
    ]).flat();
    return {
      background: stack.background,
      commands: [
        ...Array.from({ length: stack.surplusRestores }, (): CanvasCommand => ({ type: "restore" })),
        ...scopes,
        { type: "setFillColor", color: stack.color },
        // Bars rather than a flood fill: a full-canvas fill has its edges off
        // the canvas, so it looks the same however far the stack has drifted.
        ...Array.from({ length: 8 }, (_, bar): CanvasCommand => ({
          type: "fillRect",
          x: 6 + bar * 15,
          y: 6,
          width: 7,
          height: 108,
        })),
        ...Array.from(
          { length: stack.depth + stack.surplusRestores },
          (): CanvasCommand => ({ type: "restore" }),
        ),
        ...(stack.resetMidScene
          ? ([
              { type: "reset" },
              { type: "setFillColor", color: stack.background },
              { type: "fillRect", x: 0, y: 0, width: 128, height: 128 },
            ] satisfies CanvasCommand[])
          : []),
        // After every scope has closed, the transform and clip must be back to
        // where they started, so this lands in the same place either way.
        { type: "setFillColor", color: "#35a7ff" },
        { type: "fillRect", x: 8, y: 100, width: 112, height: 20 },
      ],
    };
  });

export const stateStackSceneArbitrary: fc.Arbitrary<CanvasScene> = stateStackBlock.map(
  ({ background, commands }) => ({
    name: `state-stack-fuzz-${hashScene(background, commands)}`,
    background,
    commands,
  }),
);

/**
 * Every string here puts ink on the canvas. Strings that do not — `""`, `" "` —
 * hit a divergence in how the empty bounding box is positioned that would fail
 * every run and mask the rest, so `query.browser.test.ts` names those cases.
 */
const measuredText = fc.constantFrom(
  "Valo",
  "Canvas",
  "fuzz test",
  "A quick fox",
  "Valo ",
  "a\tb",
  "a\nb",
  "iiii",
  "WWWW",
  "AVAWAY",
  ".",
  "gjpqy",
);

/**
 * `measureText` reports in the current coordinate space, so the transform in
 * the setup must not move any of the numbers — that invariance is half of what
 * this checks.
 */
export const textMeasurementSceneArbitrary: fc.Arbitrary<TextMeasurementScene> = fc
  .record({
    text: measuredText,
    size: measure(6, 48),
    align: fc.constantFrom<CanvasTextAlign>("left", "center", "right", "start", "end"),
    baseline: fc.constantFrom<CanvasTextBaseline>(
      "top",
      "hanging",
      "middle",
      "alphabetic",
      "ideographic",
      "bottom",
    ),
    // Spacing stays above where the advances sum to a negative width: Valo
    // clamps that to zero and Canvas2D does not, which would fail every run
    // and mask the rest. `query.browser.test.ts` names that case.
    letterSpacing: measure(-1, 4),
    wordSpacing: measure(-1, 6),
    translationX: measure(-30, 30),
    translationY: measure(-30, 30),
    scale: amount(0.4, 2.5),
    rotation: amount(-1, 1),
  })
  .map((query) => {
    const setup: CanvasCommand[] = [
      { type: "translate", x: query.translationX, y: query.translationY },
      { type: "rotate", radians: query.rotation },
      { type: "scale", x: query.scale, y: query.scale },
      { type: "setFont", value: `${query.size}px '${FIXTURE_FONT_FAMILY}'` },
      { type: "setTextAlign", value: query.align },
      { type: "setTextBaseline", value: query.baseline },
      {
        type: "setTextSpacing",
        letter: `${query.letterSpacing}px`,
        word: `${query.wordSpacing}px`,
      },
    ];
    return {
      name: `measure-text-fuzz-${hashScene(query.text, setup)}`,
      setup,
      text: query.text,
    };
  });

/**
 * A star polygon around a centre: strictly increasing angles with a minimum
 * gap, so the outline is simple and no edge collapses to a sliver. A polygon
 * from independent random vertices shrinks toward degeneracy, and a degenerate
 * outline has no well-defined inside to compare.
 */
const hitTestPolygon = fc
  .record({
    radii: fc.array(measure(14, 52), { minLength: 3, maxLength: 7 }),
    firstAngle: amount(0, Math.PI * 2),
  })
  .map(({ radii, firstAngle }): [number, number][] =>
    radii.map((radius, index) => {
      const angle = firstAngle + (index * Math.PI * 2) / radii.length;
      return [64 + radius * Math.cos(angle), 64 + radius * Math.sin(angle)];
    }),
  );

const seamAvoidingAngle = 0.37;

/** Distances from the boundary worth asking about, on either side of it. */
const boundaryOffset = fc
  .tuple(amount(0.3, 8), fc.boolean())
  .map(([distance, outward]) => (outward ? distance : -distance));

function boundaryProbes(
  geometry: HitTestGeometry,
  offsets: readonly number[],
): [number, number][] {
  const { polygon, circle } = geometry;
  const edgeProbes = polygon.flatMap((vertex, index): [number, number][] => {
    const next = polygon[(index + 1) % polygon.length]!;
    const spanX = next[0] - vertex[0];
    const spanY = next[1] - vertex[1];
    const length = Math.hypot(spanX, spanY) || 1;
    const normal: [number, number] = [-spanY / length, spanX / length];
    const midpoint: [number, number] = [(vertex[0] + next[0]) / 2, (vertex[1] + next[1]) / 2];
    const offset = offsets[index % offsets.length]!;
    return [
      [midpoint[0] + normal[0] * offset, midpoint[1] + normal[1] * offset],
      // Toward the centre from a vertex approximates its angle bisector, which
      // is the direction that leaves both of its edges at once.
      [
        vertex[0] + ((64 - vertex[0]) / (Math.hypot(64 - vertex[0], 64 - vertex[1]) || 1)) * offset,
        vertex[1] + ((64 - vertex[1]) / (Math.hypot(64 - vertex[0], 64 - vertex[1]) || 1)) * offset,
      ],
    ];
  });
  const circleProbes = offsets.map((offset, index): [number, number] => {
    // Never angle 0: that is where `arc` opens and closes, and a probe sharing
    // its y sends a horizontal crossing test straight through the seam vertex.
    // What two implementations do there is tie-breaking, not containment.
    const angle = seamAvoidingAngle + (index * Math.PI * 2) / offsets.length;
    const radius = circle.radius + offset;
    return [circle.x + radius * Math.cos(angle), circle.y + radius * Math.sin(angle)];
  });
  return [...edgeProbes, ...circleProbes];
}

export const hitTestSceneArbitrary: fc.Arbitrary<HitTestScene> = fc
  .record({
    polygon: hitTestPolygon,
    // A second, overlapping subpath is what makes the fill rule observable.
    circle: fc.record({ x: measure(30, 98), y: measure(30, 98), radius: measure(8, 40) }),
    rule: fillRule,
    offsets: fc.array(boundaryOffset, { minLength: 1, maxLength: 5 }),
    loose: fc.array(fc.tuple(measure(-8, 136), measure(-8, 136)), { minLength: 1, maxLength: 6 }),
    translationX: measure(-20, 20),
    translationY: measure(-20, 20),
    scale: amount(0.5, 1.8),
  })
  .map((query) => {
    const geometry: HitTestGeometry = { polygon: query.polygon, circle: query.circle };
    const [first, ...rest] = query.polygon;
    const setup: CanvasCommand[] = [
      { type: "translate", x: query.translationX, y: query.translationY },
      { type: "scale", x: query.scale, y: query.scale },
      { type: "beginPath" },
      { type: "moveTo", x: first![0], y: first![1] },
      ...rest.map((vertex): CanvasCommand => ({ type: "lineTo", x: vertex[0], y: vertex[1] })),
      { type: "closePath" },
      {
        type: "arc",
        x: query.circle.x,
        y: query.circle.y,
        radius: query.circle.radius,
        start: 0,
        end: Math.PI * 2,
        counterclockwise: false,
      },
    ];
    const candidates = [
      ...boundaryProbes(geometry, query.offsets),
      ...(query.loose as [number, number][]),
    ];
    return {
      name: `hit-test-fuzz-${hashScene(query.rule, setup)}`,
      setup,
      rule: query.rule,
      // Ask only where the answer is decided. A probe aimed at one edge can
      // still land on another, so the clearance is measured, not assumed. A
      // quarter pixel clears the ~0.02px the two sides' circle approximations
      // differ by, which is the only band where they are answering about
      // different curves rather than about the same one.
      points: candidates
        .filter((point) => boundaryClearance(geometry, point) * query.scale >= 0.25)
        .map(([x, y]): [number, number] => [
          x * query.scale + query.translationX,
          y * query.scale + query.translationY,
        ]),
    };
  });

function hashScene(background: string, commands: readonly CanvasCommand[]): string {
  const source = JSON.stringify([background, commands]);
  let hash = 2_166_136_261;
  for (let index = 0; index < source.length; index += 1) {
    hash ^= source.charCodeAt(index);
    hash = Math.imul(hash, 16_777_619);
  }
  return (hash >>> 0).toString(16).padStart(8, "0");
}
