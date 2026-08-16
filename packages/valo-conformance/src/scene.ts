export const CANVAS_SIZE = 128;
export const FIXTURE_FONT_FAMILY = "Valo Conformance";
/** The element holding both canvases side by side, captured in one screenshot. */
export const CANVAS_PAIR_TEST_ID = "canvas-pair";

export interface ColorStop {
  offset: number;
  color: string;
}

export type CanvasCommand =
  | { type: "save" }
  | { type: "restore" }
  | { type: "reset" }
  | { type: "setFillColor"; color: string }
  | { type: "setStrokeColor"; color: string }
  | { type: "setFillLinearGradient"; points: [number, number, number, number]; stops: ColorStop[] }
  | { type: "setStrokeLinearGradient"; points: [number, number, number, number]; stops: ColorStop[] }
  | { type: "setFillRadialGradient"; circles: [number, number, number, number, number, number]; stops: ColorStop[] }
  | { type: "setFillConicGradient"; center: [number, number, number]; stops: ColorStop[] }
  | { type: "setGlobalAlpha"; alpha: number }
  | { type: "setComposite"; operation: GlobalCompositeOperation }
  | { type: "setStroke"; width: number; cap: CanvasLineCap; join: CanvasLineJoin; miterLimit: number }
  | { type: "setLineDash"; intervals: number[]; offset: number }
  | { type: "setShadow"; color: string; blur: number; offsetX: number; offsetY: number }
  | { type: "setImageSmoothing"; enabled: boolean }
  | { type: "setFont"; value: string }
  | { type: "setTextAlign"; value: CanvasTextAlign }
  | { type: "setTextBaseline"; value: CanvasTextBaseline }
  | { type: "setTextSpacing"; letter: string; word: string }
  | { type: "setFilter"; value: string }
  | { type: "translate"; x: number; y: number }
  | { type: "scale"; x: number; y: number }
  | { type: "rotate"; radians: number }
  | { type: "transform"; matrix: [number, number, number, number, number, number] }
  | { type: "setTransform"; matrix: [number, number, number, number, number, number] }
  | { type: "resetTransform" }
  | { type: "beginPath" }
  | { type: "closePath" }
  | { type: "moveTo"; x: number; y: number }
  | { type: "lineTo"; x: number; y: number }
  | { type: "quadraticCurveTo"; controlX: number; controlY: number; x: number; y: number }
  | { type: "bezierCurveTo"; control1X: number; control1Y: number; control2X: number; control2Y: number; x: number; y: number }
  | { type: "rect"; x: number; y: number; width: number; height: number }
  | { type: "roundRect"; x: number; y: number; width: number; height: number; radii: number[] }
  | { type: "arc"; x: number; y: number; radius: number; start: number; end: number; counterclockwise: boolean }
  | { type: "ellipse"; x: number; y: number; radiusX: number; radiusY: number; rotation: number; start: number; end: number; counterclockwise: boolean }
  | { type: "arcTo"; x1: number; y1: number; x2: number; y2: number; radius: number }
  | { type: "fill"; rule: CanvasFillRule }
  | { type: "stroke" }
  | { type: "clip"; rule: CanvasFillRule }
  | { type: "fillRect"; x: number; y: number; width: number; height: number }
  | { type: "strokeRect"; x: number; y: number; width: number; height: number }
  | { type: "clearRect"; x: number; y: number; width: number; height: number }
  | { type: "fillText"; text: string; x: number; y: number; maxWidth?: number }
  | { type: "strokeText"; text: string; x: number; y: number; maxWidth?: number }
  | { type: "drawImage"; arguments: ImageArguments }
  | {
      type: "setFillPattern";
      repetition: "repeat";
      transform: [number, number, number, number, number, number];
    };

export type ImageArguments =
  | [destinationX: number, destinationY: number]
  | [destinationX: number, destinationY: number, destinationWidth: number, destinationHeight: number]
  | [
      sourceX: number,
      sourceY: number,
      sourceWidth: number,
      sourceHeight: number,
      destinationX: number,
      destinationY: number,
      destinationWidth: number,
      destinationHeight: number,
    ];

export interface CanvasScene {
  name: string;
  background: string;
  commands: CanvasCommand[];
}

interface GradientLike {
  addColorStop(offset: number, color: string): void;
}

interface PatternLike {
  setTransform(transform?: DOMMatrix2DInit): void;
}

export interface ReplayAssets {
  image: unknown;
}

export interface ReplayContext {
  fillStyle: string | GradientLike | PatternLike;
  strokeStyle: string | GradientLike | PatternLike;
  globalAlpha: number;
  globalCompositeOperation: GlobalCompositeOperation;
  lineWidth: number;
  lineCap: CanvasLineCap;
  lineJoin: CanvasLineJoin;
  miterLimit: number;
  lineDashOffset: number;
  shadowColor: string;
  shadowBlur: number;
  shadowOffsetX: number;
  shadowOffsetY: number;
  imageSmoothingEnabled: boolean;
  font: string;
  textAlign: CanvasTextAlign;
  textBaseline: CanvasTextBaseline;
  letterSpacing: string;
  wordSpacing: string;
  filter: string;
  save(): void;
  restore(): void;
  reset(): void;
  translate(x: number, y: number): void;
  scale(x: number, y: number): void;
  rotate(radians: number): void;
  transform(a: number, b: number, c: number, d: number, e: number, f: number): void;
  setTransform(a: number, b: number, c: number, d: number, e: number, f: number): void;
  resetTransform(): void;
  beginPath(): void;
  closePath(): void;
  moveTo(x: number, y: number): void;
  lineTo(x: number, y: number): void;
  quadraticCurveTo(controlX: number, controlY: number, x: number, y: number): void;
  bezierCurveTo(control1X: number, control1Y: number, control2X: number, control2Y: number, x: number, y: number): void;
  rect(x: number, y: number, width: number, height: number): void;
  roundRect(x: number, y: number, width: number, height: number, radii?: number | number[]): void;
  arc(x: number, y: number, radius: number, start: number, end: number, counterclockwise?: boolean): void;
  ellipse(x: number, y: number, radiusX: number, radiusY: number, rotation: number, start: number, end: number, counterclockwise?: boolean): void;
  arcTo(x1: number, y1: number, x2: number, y2: number, radius: number): void;
  fill(rule?: CanvasFillRule): void;
  stroke(): void;
  clip(rule?: CanvasFillRule): void;
  fillRect(x: number, y: number, width: number, height: number): void;
  strokeRect(x: number, y: number, width: number, height: number): void;
  clearRect(x: number, y: number, width: number, height: number): void;
  setLineDash(intervals: number[]): void;
  createLinearGradient(x0: number, y0: number, x1: number, y1: number): GradientLike;
  createRadialGradient(x0: number, y0: number, radius0: number, x1: number, y1: number, radius1: number): GradientLike;
  createConicGradient(startAngle: number, x: number, y: number): GradientLike;
  createPattern(image: unknown, repetition: string | null): PatternLike | null;
  fillText(text: string, x: number, y: number, maxWidth?: number): void;
  strokeText(text: string, x: number, y: number, maxWidth?: number): void;
  drawImage(image: unknown, ...argumentsList: number[]): void;
  measureText(text: string): TextMetricsLike;
  isPointInPath(x: number, y: number, rule?: CanvasFillRule): boolean;
}

/**
 * The `TextMetrics` members Valo reports. `hangingBaseline`,
 * `ideographicBaseline` and `alphabeticBaseline` are absent from
 * `ValoTextMetrics`, so there is nothing on the Valo side to compare.
 */
export const TEXT_METRIC_KEYS = [
  "width",
  "actualBoundingBoxLeft",
  "actualBoundingBoxRight",
  "actualBoundingBoxAscent",
  "actualBoundingBoxDescent",
  "fontBoundingBoxAscent",
  "fontBoundingBoxDescent",
  "emHeightAscent",
  "emHeightDescent",
] as const;

export type TextMetricKey = (typeof TEXT_METRIC_KEYS)[number];

export type TextMetricsLike = Readonly<Record<TextMetricKey, number>>;

export function replayCommands(
  context: ReplayContext,
  commands: readonly CanvasCommand[],
  assets?: ReplayAssets,
): void {
  for (const command of commands) replayCommand(context, command, assets);
}

function replayCommand(
  context: ReplayContext,
  command: CanvasCommand,
  assets: ReplayAssets | undefined,
): void {
  switch (command.type) {
    case "save": context.save(); break;
    case "restore": context.restore(); break;
    case "reset": context.reset(); break;
    case "setFillColor": context.fillStyle = command.color; break;
    case "setStrokeColor": context.strokeStyle = command.color; break;
    case "setFillLinearGradient": {
      const gradient = context.createLinearGradient(...command.points);
      addStops(gradient, command.stops);
      context.fillStyle = gradient;
      break;
    }
    case "setStrokeLinearGradient": {
      const gradient = context.createLinearGradient(...command.points);
      addStops(gradient, command.stops);
      context.strokeStyle = gradient;
      break;
    }
    case "setFillRadialGradient": {
      const gradient = context.createRadialGradient(...command.circles);
      addStops(gradient, command.stops);
      context.fillStyle = gradient;
      break;
    }
    case "setFillConicGradient": {
      const gradient = context.createConicGradient(...command.center);
      addStops(gradient, command.stops);
      context.fillStyle = gradient;
      break;
    }
    case "setGlobalAlpha": context.globalAlpha = command.alpha; break;
    case "setComposite": context.globalCompositeOperation = command.operation; break;
    case "setStroke":
      context.lineWidth = command.width;
      context.lineCap = command.cap;
      context.lineJoin = command.join;
      context.miterLimit = command.miterLimit;
      break;
    case "setLineDash":
      context.setLineDash(command.intervals);
      context.lineDashOffset = command.offset;
      break;
    case "setShadow":
      context.shadowColor = command.color;
      context.shadowBlur = command.blur;
      context.shadowOffsetX = command.offsetX;
      context.shadowOffsetY = command.offsetY;
      break;
    case "setImageSmoothing": context.imageSmoothingEnabled = command.enabled; break;
    case "setFont": context.font = command.value; break;
    case "setTextAlign": context.textAlign = command.value; break;
    case "setTextBaseline": context.textBaseline = command.value; break;
    case "setTextSpacing":
      context.letterSpacing = command.letter;
      context.wordSpacing = command.word;
      break;
    case "setFilter": context.filter = command.value; break;
    case "translate": context.translate(command.x, command.y); break;
    case "scale": context.scale(command.x, command.y); break;
    case "rotate": context.rotate(command.radians); break;
    case "transform": context.transform(...command.matrix); break;
    case "setTransform": context.setTransform(...command.matrix); break;
    case "resetTransform": context.resetTransform(); break;
    case "beginPath": context.beginPath(); break;
    case "closePath": context.closePath(); break;
    case "moveTo": context.moveTo(command.x, command.y); break;
    case "lineTo": context.lineTo(command.x, command.y); break;
    case "quadraticCurveTo": context.quadraticCurveTo(command.controlX, command.controlY, command.x, command.y); break;
    case "bezierCurveTo": context.bezierCurveTo(command.control1X, command.control1Y, command.control2X, command.control2Y, command.x, command.y); break;
    case "rect": context.rect(command.x, command.y, command.width, command.height); break;
    case "roundRect": context.roundRect(command.x, command.y, command.width, command.height, command.radii); break;
    case "arc": context.arc(command.x, command.y, command.radius, command.start, command.end, command.counterclockwise); break;
    case "ellipse": context.ellipse(command.x, command.y, command.radiusX, command.radiusY, command.rotation, command.start, command.end, command.counterclockwise); break;
    case "arcTo": context.arcTo(command.x1, command.y1, command.x2, command.y2, command.radius); break;
    case "fill": context.fill(command.rule); break;
    case "stroke": context.stroke(); break;
    case "clip": context.clip(command.rule); break;
    case "fillRect": context.fillRect(command.x, command.y, command.width, command.height); break;
    case "strokeRect": context.strokeRect(command.x, command.y, command.width, command.height); break;
    case "clearRect": context.clearRect(command.x, command.y, command.width, command.height); break;
    case "fillText": drawText(context.fillText.bind(context), command); break;
    case "strokeText": drawText(context.strokeText.bind(context), command); break;
    case "drawImage": context.drawImage(assetImage(assets), ...command.arguments); break;
    case "setFillPattern": {
      const pattern = context.createPattern(assetImage(assets), command.repetition);
      if (!pattern) throw new Error("could not create the fixed image pattern");
      pattern.setTransform(affineMatrix(command.transform));
      context.fillStyle = pattern;
      break;
    }
  }
}

function addStops(gradient: GradientLike, stops: readonly ColorStop[]): void {
  for (const stop of stops) gradient.addColorStop(stop.offset, stop.color);
}

function drawText(
  draw: (text: string, x: number, y: number, maxWidth?: number) => void,
  command: Extract<CanvasCommand, { type: "fillText" | "strokeText" }>,
): void {
  if (command.maxWidth === undefined) draw(command.text, command.x, command.y);
  else draw(command.text, command.x, command.y, command.maxWidth);
}

function assetImage(assets: ReplayAssets | undefined): unknown {
  if (!assets) throw new Error("this scene requires the fixed image fixture");
  return assets.image;
}

function affineMatrix(
  [a, b, c, d, e, f]: [number, number, number, number, number, number],
): DOMMatrix2DInit {
  return { a, b, c, d, e, f };
}
