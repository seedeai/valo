import * as fc from "fast-check";
import {
  FIXTURE_FONT_FAMILY,
  type CanvasCommand,
  type CanvasScene,
  type ImageArguments,
} from "./scene.js";

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
const coordinate = fc.integer({ min: -12, max: 116 });
const extent = fc.integer({ min: 8, max: 72 });
const alpha = fc.integer({ min: 25, max: 100 }).map((value) => value / 100);
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
      fc.integer({ min: 0, max: 24 }),
      fc.integer({ min: 0, max: 24 }),
      fc.integer({ min: 0, max: 24 }),
      fc.integer({ min: 0, max: 24 }),
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
    x: fc.integer({ min: 12, max: 116 }),
    y: fc.integer({ min: 12, max: 116 }),
    radiusX: fc.integer({ min: 3, max: 36 }),
    radiusY: fc.integer({ min: 3, max: 36 }),
    rotation: fc.integer({ min: -100, max: 100 }).map((value) => value / 100),
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

const strokedPath = fc
  .record({
    color,
    width: fc.integer({ min: 1, max: 10 }),
    cap: fc.constantFrom<CanvasLineCap>("butt", "round", "square"),
    join: fc.constantFrom<CanvasLineJoin>("bevel", "round", "miter"),
    x0: coordinate,
    y0: coordinate,
    x1: coordinate,
    y1: coordinate,
    x2: coordinate,
    y2: coordinate,
    dashed: fc.boolean(),
  })
  .map((path): CommandBlock => [
    { type: "save" },
    { type: "setStrokeColor", color: path.color },
    {
      type: "setStroke",
      width: path.width,
      cap: path.cap,
      join: path.join,
      miterLimit: 5,
    },
    { type: "setLineDash", intervals: path.dashed ? [9, 5] : [], offset: 2 },
    { type: "beginPath" },
    { type: "moveTo", x: path.x0, y: path.y0 },
    {
      type: "quadraticCurveTo",
      controlX: path.x1,
      controlY: path.y1,
      x: path.x2,
      y: path.y2,
    },
    { type: "stroke" },
    { type: "restore" },
  ]);

const gradientStroke = fc
  .record({
    first: color,
    second: color,
    width: fc.integer({ min: 1, max: 9 }),
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
    x: fc.integer({ min: 24, max: 104 }),
    y: fc.integer({ min: 24, max: 104 }),
    width: fc.integer({ min: 8, max: 48 }),
    height: fc.integer({ min: 8, max: 48 }),
    rotation: fc.integer({ min: -120, max: 120 }).map((value) => value / 100),
    scaleX: fc.integer({ min: 50, max: 160 }).map((value) => value / 100),
    scaleY: fc.integer({ min: 50, max: 160 }).map((value) => value / 100),
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
    shearX: fc.integer({ min: -40, max: 40 }).map((value) => value / 100),
    shearY: fc.integer({ min: -40, max: 40 }).map((value) => value / 100),
    translationX: fc.integer({ min: 20, max: 60 }),
    translationY: fc.integer({ min: 20, max: 60 }),
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
    { type: "clip", rule: "nonzero" },
    {
      type: "setFillLinearGradient",
      points: [shape.x, shape.y, shape.x + shape.width, shape.y + shape.height],
      stops: [
        { offset: 0, color: shape.first },
        { offset: 1, color: shape.second },
      ],
    },
    {
      type: "fillRect",
      x: shape.x,
      y: shape.y,
      width: shape.width,
      height: shape.height,
    },
    { type: "restore" },
  ]);

const radialGradient = fc
  .record({
    first: color,
    second: color,
    x: fc.integer({ min: 24, max: 104 }),
    y: fc.integer({ min: 24, max: 104 }),
    radius: fc.integer({ min: 12, max: 48 }),
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
    x: fc.integer({ min: 16, max: 112 }),
    y: fc.integer({ min: 16, max: 112 }),
    radius: fc.integer({ min: 4, max: 34 }),
    start: fc.integer({ min: -300, max: 300 }).map((value) => value / 100),
    sweep: fc.integer({ min: 40, max: 620 }).map((value) => value / 100),
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
    startX: fc.integer({ min: 8, max: 44 }),
    startY: fc.integer({ min: 72, max: 116 }),
    cornerX: fc.integer({ min: 40, max: 88 }),
    cornerY: fc.integer({ min: 16, max: 64 }),
    endX: fc.integer({ min: 84, max: 120 }),
    endY: fc.integer({ min: 72, max: 116 }),
    radius: fc.integer({ min: 2, max: 28 }),
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
    angle: fc.integer({ min: -314, max: 314 }).map((value) => value / 100),
    x: fc.integer({ min: 42, max: 86 }),
    y: fc.integer({ min: 42, max: 86 }),
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
  .record({ color, x: fc.integer({ min: 8, max: 64 }), y: fc.integer({ min: 8, max: 64 }) })
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
    blur: fc.integer({ min: 0, max: 8 }),
    offsetX: fc.integer({ min: -6, max: 8 }),
    offsetY: fc.integer({ min: -6, max: 8 }),
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
  .record({
    color,
    blur: fc.integer({ min: 0, max: 5 }),
    brightness: fc.integer({ min: 50, max: 150 }),
    hue: fc.integer({ min: -120, max: 120 }),
    opacity: fc.integer({ min: 40, max: 100 }),
  })
  .map((shape): CommandBlock => [
    { type: "save" },
    {
      type: "setFilter",
      value: `blur(${shape.blur}px) brightness(${shape.brightness}%) contrast(110%) grayscale(20%) hue-rotate(${shape.hue}deg) invert(15%) opacity(${shape.opacity}%) saturate(125%) sepia(18%)`,
    },
    { type: "setFillColor", color: shape.color },
    { type: "fillRect", x: 30, y: 32, width: 62, height: 54 },
    { type: "restore" },
  ]);

const shadowedGradient = fc
  .record({
    first: color,
    second: color,
    shadow: color,
    blur: fc.integer({ min: 0, max: 8 }),
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
    x: fc.integer({ min: 0, max: 104 }),
    y: fc.integer({ min: 0, max: 104 }),
    width: fc.integer({ min: 8, max: 48 }),
    height: fc.integer({ min: 8, max: 48 }),
    sourceX: fc.integer({ min: 0, max: 8 }),
    sourceY: fc.integer({ min: 0, max: 8 }),
    sourceWidth: fc.integer({ min: 4, max: 8 }),
    sourceHeight: fc.integer({ min: 4, max: 8 }),
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
    scale: fc.integer({ min: 50, max: 160 }).map((value) => value / 100),
    x: fc.integer({ min: 0, max: 12 }),
    y: fc.integer({ min: 0, max: 12 }),
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
    x: fc.integer({ min: 44, max: 92 }),
    y: fc.integer({ min: 32, max: 92 }),
    align: fc.constantFrom<CanvasTextAlign>("left", "center", "right", "start", "end"),
    baseline: fc.constantFrom<CanvasTextBaseline>(
      "top",
      "hanging",
      "middle",
      "alphabetic",
      "ideographic",
      "bottom",
    ),
    maximumWidth: fc.option(fc.integer({ min: 30, max: 110 }), { nil: undefined }),
    letterSpacing: fc.integer({ min: -1, max: 2 }),
    wordSpacing: fc.integer({ min: 0, max: 3 }),
    translationX: fc.integer({ min: -8, max: 8 }),
    translationY: fc.integer({ min: -8, max: 8 }),
    rotation: fc.integer({ min: -20, max: 20 }).map((value) => value / 100),
    scaleX: fc.integer({ min: 80, max: 125 }).map((value) => value / 100),
    scaleY: fc.integer({ min: 80, max: 125 }).map((value) => value / 100),
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
  strokedPath,
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

function hashScene(background: string, commands: readonly CanvasCommand[]): string {
  const source = JSON.stringify([background, commands]);
  let hash = 2_166_136_261;
  for (let index = 0; index < source.length; index += 1) {
    hash ^= source.charCodeAt(index);
    hash = Math.imul(hash, 16_777_619);
  }
  return (hash >>> 0).toString(16).padStart(8, "0");
}
