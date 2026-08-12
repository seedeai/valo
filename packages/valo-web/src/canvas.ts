import {
  ColorFilter,
  DisplayListBuilder,
  FontCollection,
  Paint,
  Paragraph,
  Path,
  type DisplayList,
  type Image,
  type Renderer,
} from "./raw.js";
import { parseColor, type Rgba } from "./color.js";
import {
  asDomMatrix,
  identity,
  inverse,
  mapPoint,
  multiply,
  type Affine,
} from "./matrix.js";
import { ValoCanvasGradient, ValoCanvasPattern } from "./resources.js";

export type ValoCanvasStyle = string | ValoCanvasGradient | ValoCanvasPattern;
export type ValoFillRule = "nonzero" | "evenodd";

export interface ValoCanvasOptions {
  autoPresent?: boolean;
  clearColor?: string;
}

interface ClipRecord {
  path: Path;
  rule: number;
  transform: Affine;
}

interface State {
  transform: Affine;
  clips: ClipRecord[];
  fillStyle: ValoCanvasStyle;
  strokeStyle: ValoCanvasStyle;
  globalAlpha: number;
  blendMode: number;
  lineWidth: number;
  lineCap: CanvasLineCap;
  lineJoin: CanvasLineJoin;
  miterLimit: number;
  lineDash: number[];
  lineDashOffset: number;
  shadowColor: string;
  shadowBlur: number;
  shadowOffsetX: number;
  shadowOffsetY: number;
  imageSmoothingEnabled: boolean;
  imageSmoothingQuality: ImageSmoothingQuality;
  font: string;
  textAlign: CanvasTextAlign;
  textBaseline: CanvasTextBaseline;
  letterSpacing: string;
  wordSpacing: string;
  textRendering: CanvasTextRendering;
  colorMatrix: number[] | undefined;
}

const defaultState = (): State => ({
  transform: identity,
  clips: [],
  fillStyle: "#000000",
  strokeStyle: "#000000",
  globalAlpha: 1,
  blendMode: 3,
  lineWidth: 1,
  lineCap: "butt",
  lineJoin: "miter",
  miterLimit: 10,
  lineDash: [],
  lineDashOffset: 0,
  shadowColor: "transparent",
  shadowBlur: 0,
  shadowOffsetX: 0,
  shadowOffsetY: 0,
  imageSmoothingEnabled: true,
  imageSmoothingQuality: "low",
  font: "10px sans-serif",
  textAlign: "start",
  textBaseline: "alphabetic",
  letterSpacing: "0px",
  wordSpacing: "0px",
  textRendering: "auto",
  colorMatrix: undefined,
});

export interface ValoFrameStats {
  cpuMilliseconds: number;
  gpuMilliseconds: number;
  draws: number;
  drawCalls: number;
  renderPasses: number;
  filterPasses: number;
  culled: number;
}

/**
 * Canvas-shaped recorder over Valo. Mutations are batched and presented once
 * per animation frame by default. `present()` provides explicit frame control.
 */
export class ValoCanvasRenderingContext2D {
  readonly canvas: HTMLCanvasElement;
  readonly fonts = new FontCollection();

  readonly #renderer: Renderer;
  readonly #autoPresent: boolean;
  #clearColor: Rgba;
  #state = defaultState();
  #stack: State[] = [];
  #path = new Path();
  #builder = new DisplayListBuilder();
  #history: DisplayList[] = [];
  #scheduled = false;
  #dirty = false;
  #lastStats: ValoFrameStats | undefined;

  constructor(canvas: HTMLCanvasElement, renderer: Renderer, options: ValoCanvasOptions = {}) {
    this.canvas = canvas;
    this.#renderer = renderer;
    this.#autoPresent = options.autoPresent ?? true;
    this.#clearColor = parseColor(options.clearColor ?? "transparent");
  }

  get fillStyle(): ValoCanvasStyle {
    return this.#state.fillStyle;
  }
  set fillStyle(value: ValoCanvasStyle) {
    this.#state.fillStyle = value;
  }

  get strokeStyle(): ValoCanvasStyle {
    return this.#state.strokeStyle;
  }
  set strokeStyle(value: ValoCanvasStyle) {
    this.#state.strokeStyle = value;
  }

  get globalAlpha(): number {
    return this.#state.globalAlpha;
  }
  set globalAlpha(value: number) {
    if (Number.isFinite(value) && value >= 0 && value <= 1) this.#state.globalAlpha = value;
  }

  get globalCompositeOperation(): GlobalCompositeOperation {
    return blendModeNames[this.#state.blendMode] ?? "source-over";
  }
  set globalCompositeOperation(value: GlobalCompositeOperation) {
    const mode = blendModes[value];
    if (mode !== undefined) this.#state.blendMode = mode;
  }

  get lineWidth(): number {
    return this.#state.lineWidth;
  }
  set lineWidth(value: number) {
    if (Number.isFinite(value) && value > 0) this.#state.lineWidth = value;
  }

  get lineCap(): CanvasLineCap {
    return this.#state.lineCap;
  }
  set lineCap(value: CanvasLineCap) {
    this.#state.lineCap = value;
  }

  get lineJoin(): CanvasLineJoin {
    return this.#state.lineJoin;
  }
  set lineJoin(value: CanvasLineJoin) {
    this.#state.lineJoin = value;
  }

  get miterLimit(): number {
    return this.#state.miterLimit;
  }
  set miterLimit(value: number) {
    if (Number.isFinite(value) && value > 0) this.#state.miterLimit = value;
  }

  get lineDashOffset(): number {
    return this.#state.lineDashOffset;
  }
  set lineDashOffset(value: number) {
    if (Number.isFinite(value)) this.#state.lineDashOffset = value;
  }

  get shadowColor(): string {
    return this.#state.shadowColor;
  }
  set shadowColor(value: string) {
    parseColor(value);
    this.#state.shadowColor = value;
  }

  get shadowBlur(): number {
    return this.#state.shadowBlur;
  }
  set shadowBlur(value: number) {
    if (Number.isFinite(value) && value >= 0) this.#state.shadowBlur = value;
  }

  get shadowOffsetX(): number {
    return this.#state.shadowOffsetX;
  }
  set shadowOffsetX(value: number) {
    if (Number.isFinite(value)) this.#state.shadowOffsetX = value;
  }

  get shadowOffsetY(): number {
    return this.#state.shadowOffsetY;
  }
  set shadowOffsetY(value: number) {
    if (Number.isFinite(value)) this.#state.shadowOffsetY = value;
  }

  get imageSmoothingEnabled(): boolean {
    return this.#state.imageSmoothingEnabled;
  }
  set imageSmoothingEnabled(value: boolean) {
    this.#state.imageSmoothingEnabled = value;
  }

  get imageSmoothingQuality(): ImageSmoothingQuality {
    return this.#state.imageSmoothingQuality;
  }
  set imageSmoothingQuality(value: ImageSmoothingQuality) {
    if (value === "low" || value === "medium" || value === "high") {
      this.#state.imageSmoothingQuality = value;
    }
  }

  get font(): string {
    return this.#state.font;
  }
  set font(value: string) {
    parseFont(value);
    this.#state.font = value;
  }

  get textAlign(): CanvasTextAlign {
    return this.#state.textAlign;
  }
  set textAlign(value: CanvasTextAlign) {
    this.#state.textAlign = value;
  }

  get textBaseline(): CanvasTextBaseline {
    return this.#state.textBaseline;
  }
  set textBaseline(value: CanvasTextBaseline) {
    this.#state.textBaseline = value;
  }

  get letterSpacing(): string {
    return this.#state.letterSpacing;
  }
  set letterSpacing(value: string) {
    parsePixels(value);
    this.#state.letterSpacing = value;
  }

  get wordSpacing(): string {
    return this.#state.wordSpacing;
  }
  set wordSpacing(value: string) {
    parsePixels(value);
    this.#state.wordSpacing = value;
  }

  get textRendering(): CanvasTextRendering {
    return this.#state.textRendering;
  }
  set textRendering(value: CanvasTextRendering) {
    if (
      value === "auto" ||
      value === "optimizeSpeed" ||
      value === "optimizeLegibility" ||
      value === "geometricPrecision"
    ) {
      this.#state.textRendering = value;
    }
  }

  save(): void {
    this.#stack.push(cloneState(this.#state));
    this.#builder.save();
  }

  restore(): void {
    const restored = this.#stack.pop();
    if (!restored) return;
    disposeState(this.#state);
    this.#state = restored;
    this.#builder.restore();
  }

  reset(): void {
    this.#releaseHistory();
    this.#builder.free();
    this.#path.free();
    disposeState(this.#state);
    for (const saved of this.#stack) disposeState(saved);
    this.#state = defaultState();
    this.#stack = [];
    this.#path = new Path();
    this.#builder = new DisplayListBuilder();
    this.#dirty = true;
    this.#schedule();
  }

  beginPath(): void {
    this.#path.free();
    this.#path = new Path();
  }

  closePath(): void {
    this.#path.close();
  }

  moveTo(x: number, y: number): void {
    this.#path.moveTo(x, y);
  }

  lineTo(x: number, y: number): void {
    this.#path.lineTo(x, y);
  }

  quadraticCurveTo(controlX: number, controlY: number, x: number, y: number): void {
    this.#path.quadraticCurveTo(controlX, controlY, x, y);
  }

  bezierCurveTo(
    control1X: number,
    control1Y: number,
    control2X: number,
    control2Y: number,
    x: number,
    y: number,
  ): void {
    this.#path.bezierCurveTo(control1X, control1Y, control2X, control2Y, x, y);
  }

  rect(x: number, y: number, width: number, height: number): void {
    this.#path.rect(x, y, width, height);
  }

  roundRect(
    x: number,
    y: number,
    width: number,
    height: number,
    radii: number | readonly number[] = 0,
  ): void {
    this.#path.roundRect(x, y, width, height, radiiArray(radii));
  }

  arc(
    x: number,
    y: number,
    radius: number,
    startAngle: number,
    endAngle: number,
    counterclockwise = false,
  ): void {
    if (radius < 0) throw new DOMException("The radius must be non-negative", "IndexSizeError");
    this.#path.arc(x, y, radius, startAngle, sweep(startAngle, endAngle, counterclockwise));
  }

  ellipse(
    x: number,
    y: number,
    radiusX: number,
    radiusY: number,
    rotation: number,
    startAngle: number,
    endAngle: number,
    counterclockwise = false,
  ): void {
    if (radiusX < 0 || radiusY < 0) {
      throw new DOMException("Ellipse radii must be non-negative", "IndexSizeError");
    }
    this.#path.ellipse(
      x,
      y,
      radiusX,
      radiusY,
      rotation,
      startAngle,
      sweep(startAngle, endAngle, counterclockwise),
    );
  }

  arcTo(x1: number, y1: number, x2: number, y2: number, radius: number): void {
    if (radius < 0) throw new DOMException("The radius must be non-negative", "IndexSizeError");
    this.#path.arcTo(x1, y1, x2, y2, radius);
  }

  fill(pathOrRule?: Path | ValoFillRule, rule: ValoFillRule = "nonzero"): void {
    const path = pathOrRule instanceof Path ? pathOrRule : this.#path;
    const fillRule = typeof pathOrRule === "string" ? pathOrRule : rule;
    this.#drawShape((paint) => this.#builder.drawPath(path, ruleId(fillRule), paint), false);
  }

  stroke(path: Path = this.#path): void {
    this.#drawShape((paint) => this.#builder.drawPath(path, 0, paint), true);
  }

  clip(pathOrRule?: Path | ValoFillRule, rule: ValoFillRule = "nonzero"): void {
    const path = pathOrRule instanceof Path ? pathOrRule : this.#path;
    const fillRule = typeof pathOrRule === "string" ? pathOrRule : rule;
    const record = {
      path: path.clone(),
      rule: ruleId(fillRule),
      transform: [...this.#state.transform] as Affine,
    };
    this.#state.clips = [...this.#state.clips, record];
    this.#builder.clipPath(record.path, record.rule, 0);
  }

  fillRect(x: number, y: number, width: number, height: number): void {
    this.#drawShape((paint) => this.#builder.drawRect(x, y, width, height, paint), false);
  }

  strokeRect(x: number, y: number, width: number, height: number): void {
    this.#drawShape((paint) => this.#builder.drawRect(x, y, width, height, paint), true);
  }

  clearRect(x: number, y: number, width: number, height: number): void {
    if (
      equalMatrix(this.#state.transform, identity) &&
      this.#state.clips.length === 0 &&
      x <= 0 &&
      y <= 0 &&
      x + width >= this.canvas.width &&
      y + height >= this.canvas.height
    ) {
      this.#releaseHistory();
      this.#builder.free();
      this.#builder = new DisplayListBuilder();
      this.#replayState();
    } else {
      const paint = new Paint(0, 0, 0, 0);
      paint.setBlendMode(0);
      this.#builder.drawRect(x, y, width, height, paint);
      paint.free();
    }
    this.#markDirty();
  }

  drawImage(image: Image, ...argumentsList: number[]): void {
    const [sourceX, sourceY, sourceWidth, sourceHeight, destinationX, destinationY, destinationWidth, destinationHeight] =
      imageArguments(image, argumentsList);
    this.#drawStyled(
      (paint) => this.#builder.drawImageRect(
        image,
        sourceX,
        sourceY,
        sourceWidth,
        sourceHeight,
        destinationX,
        destinationY,
        destinationWidth,
        destinationHeight,
        this.#state.imageSmoothingEnabled ? 0 : 1,
        0,
        0,
        paint,
      ),
      false,
      "#ffffff",
    );
  }

  createLinearGradient(x0: number, y0: number, x1: number, y1: number): ValoCanvasGradient {
    return new ValoCanvasGradient({ type: "linear", values: [x0, y0, x1, y1] });
  }

  createRadialGradient(
    x0: number,
    y0: number,
    radius0: number,
    x1: number,
    y1: number,
    radius1: number,
  ): ValoCanvasGradient {
    if (radius0 < 0 || radius1 < 0) {
      throw new DOMException("Gradient radii must be non-negative", "IndexSizeError");
    }
    return new ValoCanvasGradient({
      type: "radial",
      values: [x0, y0, radius0, x1, y1, radius1],
    });
  }

  createConicGradient(startAngle: number, x: number, y: number): ValoCanvasGradient {
    return new ValoCanvasGradient({ type: "sweep", values: [x, y, startAngle] });
  }

  createPattern(image: Image, repetition: string | null = "repeat"): ValoCanvasPattern {
    return new ValoCanvasPattern(image, repetition);
  }

  isPointInPath(
    pathOrX: Path | number,
    xOrY: number,
    yOrRule?: number | ValoFillRule,
    maybeRule: ValoFillRule = "nonzero",
  ): boolean {
    const path = pathOrX instanceof Path ? pathOrX : this.#path;
    const x = pathOrX instanceof Path ? xOrY : pathOrX;
    const y = pathOrX instanceof Path ? (yOrRule as number) : xOrY;
    const rule = pathOrX instanceof Path ? maybeRule : (yOrRule as ValoFillRule | undefined) ?? "nonzero";
    const inverseTransform = inverse(this.#state.transform);
    if (!inverseTransform) return false;
    const local = mapPoint(inverseTransform, x, y);
    return path.contains(local[0], local[1], ruleId(rule));
  }

  isPointInStroke(): boolean {
    throw new DOMException(
      "isPointInStroke is not implemented; use Path.contains for fill hit-testing or application geometry for strokes",
      "NotSupportedError",
    );
  }

  translate(x: number, y: number): void {
    this.transform(1, 0, 0, 1, x, y);
  }

  scale(x: number, y: number): void {
    this.transform(x, 0, 0, y, 0, 0);
  }

  rotate(angle: number): void {
    const cosine = Math.cos(angle);
    const sine = Math.sin(angle);
    this.transform(cosine, sine, -sine, cosine, 0, 0);
  }

  transform(a: number, b: number, c: number, d: number, e: number, f: number): void {
    const local: Affine = [a, b, c, d, e, f];
    this.#state.transform = multiply(this.#state.transform, local);
    this.#builder.transform(new Float32Array(local));
  }

  setTransform(
    aOrMatrix: number | DOMMatrix2DInit,
    b?: number,
    c?: number,
    d?: number,
    e?: number,
    f?: number,
  ): void {
    const target: Affine =
      typeof aOrMatrix === "number"
        ? [aOrMatrix, b ?? 0, c ?? 0, d ?? 1, e ?? 0, f ?? 0]
        : domMatrixValues(aOrMatrix);
    const currentInverse = inverse(this.#state.transform);
    if (!currentInverse) return;
    this.#builder.transform(new Float32Array(multiply(currentInverse, target)));
    this.#state.transform = target;
  }

  resetTransform(): void {
    this.setTransform(...identity);
  }

  getTransform(): DOMMatrix {
    return asDomMatrix(this.#state.transform);
  }

  setLineDash(segments: number[]): void {
    if (segments.some((value) => !Number.isFinite(value) || value < 0)) return;
    this.#state.lineDash = segments.length % 2 === 1 ? [...segments, ...segments] : [...segments];
  }

  getLineDash(): number[] {
    return [...this.#state.lineDash];
  }

  async registerFont(
    family: string,
    bytes: ArrayBuffer | Uint8Array,
    fallback = false,
  ): Promise<void> {
    const view = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    if (!this.fonts.registerFont(family, view, fallback)) {
      throw new DOMException(`Could not parse font '${family}'`, "DataError");
    }
  }

  fillText(text: string, x: number, y: number, maxWidth = Number.POSITIVE_INFINITY): void {
    this.#drawText(text, x, y, maxWidth, false);
  }

  strokeText(text: string, x: number, y: number, maxWidth = Number.POSITIVE_INFINITY): void {
    this.#drawText(text, x, y, maxWidth, true);
  }

  measureText(text: string): ValoTextMetrics {
    const paragraph = this.#paragraph(text, Number.POSITIVE_INFINITY);
    const [horizontal, vertical] = this.#textOffset(paragraph);
    const alphabeticDisplacement = vertical + paragraph.alphabeticBaseline;
    const hasOutline = paragraph.hasOutline;
    const metrics = new ValoTextMetrics(
      paragraph.width,
      hasOutline ? -horizontal - paragraph.outlineLeft : 0,
      hasOutline ? horizontal + paragraph.outlineRight : 0,
      hasOutline ? -vertical - paragraph.outlineTop : 0,
      hasOutline ? vertical + paragraph.outlineBottom : 0,
      paragraph.primaryFontAscent - alphabeticDisplacement,
      paragraph.primaryFontDescent + alphabeticDisplacement,
      paragraph.emAscent - alphabeticDisplacement,
      paragraph.emDescent + alphabeticDisplacement,
    );
    paragraph.free();
    return metrics;
  }

  createImageData(width: number, height: number): ImageData {
    return new ImageData(width, height);
  }

  getImageData(): never {
    throw new DOMException(
      "Synchronous GPU readback is unavailable. Keep source pixels in JavaScript or use a native Canvas2D fallback for read-heavy code.",
      "NotSupportedError",
    );
  }

  putImageData(): never {
    throw new DOMException(
      "putImageData is asynchronous in a GPU renderer. Upload the bytes with Renderer.uploadRgba and draw the returned Valo Image.",
      "NotSupportedError",
    );
  }

  /** Valo extension: apply a 4×5 color matrix to subsequent draws. */
  setColorMatrix(values: readonly number[] | null): void {
    if (values && values.length !== 20) {
      throw new RangeError("a color matrix must contain 20 values");
    }
    this.#state.colorMatrix = values ? [...values] : undefined;
  }

  /** Valo extension: open an explicitly composited layer. */
  saveLayer(alpha = 1, blend: GlobalCompositeOperation = "source-over"): void {
    const paint = new Paint(0, 0, 0, alpha);
    paint.setBlendMode(blendModes[blend] ?? 3);
    this.#stack.push(cloneState(this.#state));
    this.#builder.saveLayer(paint);
    paint.free();
  }

  /** Valo extension: frosted glass over pixels already recorded. */
  backdropBlur(x: number, y: number, width: number, height: number, sigma: number): void {
    this.#builder.backdropBlur(x, y, width, height, sigma);
    this.#markDirty();
  }

  /** Drop retained draws and start the next frame from a clear surface. */
  beginFrame(clearColor = "transparent"): void {
    this.#clearColor = parseColor(clearColor);
    this.#releaseHistory();
    this.#builder.free();
    this.#builder = new DisplayListBuilder();
    this.#replayState();
    this.#dirty = true;
  }

  /** Present all mutations recorded since the previous call. */
  present(): ValoFrameStats | undefined {
    this.#scheduled = false;
    if (!this.#dirty) return this.#lastStats;

    const delta = this.#builder.build();
    this.#builder.free();
    if (delta.draw_count > 0) this.#history.push(delta);
    else delta.free();

    const frame = new DisplayListBuilder();
    for (const list of this.#history) frame.drawDisplayList(list);
    const displayList = frame.build();
    frame.free();
    const [red, green, blue, alpha] = this.#clearColor;
    const rawStats = this.#renderer.render(displayList, true, red, green, blue, alpha);
    this.#lastStats = rawStats
      ? {
          cpuMilliseconds: rawStats.cpuMilliseconds,
          gpuMilliseconds: rawStats.gpuMilliseconds,
          draws: rawStats.draws,
          drawCalls: rawStats.drawCalls,
          renderPasses: rawStats.renderPasses,
          filterPasses: rawStats.filterPasses,
          culled: rawStats.culled,
        }
      : undefined;
    rawStats?.free();
    displayList.free();

    this.#builder = new DisplayListBuilder();
    this.#replayState();
    this.#dirty = false;
    return this.#lastStats;
  }

  resizeToDisplaySize(devicePixelRatio = window.devicePixelRatio): boolean {
    const width = Math.max(1, Math.round(this.canvas.clientWidth * devicePixelRatio));
    const height = Math.max(1, Math.round(this.canvas.clientHeight * devicePixelRatio));
    if (this.canvas.width === width && this.canvas.height === height) return false;
    this.canvas.width = width;
    this.canvas.height = height;
    this.#renderer.resize(width, height);
    this.#dirty = true;
    this.#schedule();
    return true;
  }

  #drawShape(draw: (paint: Paint) => void, stroke: boolean): void {
    this.#drawStyled(
      draw,
      stroke,
      stroke ? this.#state.strokeStyle : this.#state.fillStyle,
    );
  }

  #drawStyled(
    draw: (paint: Paint) => void,
    stroke: boolean,
    style: ValoCanvasStyle,
  ): void {
    const composite = canvasDestructiveBlendModes.has(this.#state.blendMode);
    if (composite) {
      const layerPaint = new Paint(1, 1, 1, 1);
      layerPaint.setBlendMode(this.#state.blendMode);
      this.#builder.saveLayer(layerPaint);
      layerPaint.free();
    }
    const sourceBlendMode = composite ? blendModes["source-over"]! : this.#state.blendMode;
    this.#drawShadow(draw, stroke, style, sourceBlendMode);
    const paint = this.#paint(style, stroke, true, sourceBlendMode);
    draw(paint);
    paint.free();
    if (composite) this.#builder.restore();
    this.#markDirty();
  }

  #drawText(text: string, x: number, y: number, maxWidth: number, stroke: boolean): void {
    if (maxWidth <= 0 || Number.isNaN(maxWidth)) return;
    const paragraph = this.#paragraph(text, Number.POSITIVE_INFINITY);
    const horizontalScale = textHorizontalScale(paragraph.width, maxWidth);
    const [left, top] = this.#textOffset(paragraph);
    this.#builder.save();
    this.#builder.translate(x, y);
    if (horizontalScale !== 1) this.#builder.scale(horizontalScale, 1);
    this.#drawShape(
      (paint) => this.#builder.drawParagraphWith(paragraph, left, top, paint),
      stroke,
    );
    this.#builder.restore();
    paragraph.free();
  }

  #drawShadow(
    draw: (paint: Paint) => void,
    stroke: boolean,
    style: ValoCanvasStyle,
    blendMode: number,
  ): void {
    const color = parseColor(this.#state.shadowColor);
    if (color[3] === 0) return;
    const paint = this.#paint(style, stroke, false, blendMode);
    const filter = ColorFilter.matrix(shadowColorMatrix(color));
    paint.setColorFilter(filter);
    filter.free();
    if (this.#state.shadowBlur > 0) paint.setMaskBlur(this.#state.shadowBlur / 2, 0);
    this.#builder.save();
    this.#builder.translate(this.#state.shadowOffsetX, this.#state.shadowOffsetY);
    draw(paint);
    this.#builder.restore();
    paint.free();
  }

  #paint(
    style: ValoCanvasStyle,
    stroke: boolean,
    includeFilter = true,
    blendMode = this.#state.blendMode,
  ): Paint {
    const color = typeof style === "string" ? parseColor(style) : ([1, 1, 1, 1] as const);
    const paint = new Paint(color[0], color[1], color[2], color[3] * this.#state.globalAlpha);
    paint.setBlendMode(blendMode);
    if (style instanceof ValoCanvasGradient) {
      const shader = style.toRaw();
      paint.setShader(shader);
      shader.free();
    } else if (style instanceof ValoCanvasPattern) {
      const shader = style.toRaw(this.#state.imageSmoothingEnabled);
      paint.setShader(shader);
      shader.free();
    }
    if (stroke) {
      paint.setStroke(
        this.#state.lineWidth,
        capId(this.#state.lineCap),
        joinId(this.#state.lineJoin),
        this.#state.miterLimit,
        new Float32Array(this.#state.lineDash),
        this.#state.lineDashOffset,
      );
    }
    if (includeFilter && this.#state.colorMatrix) {
      const filter = ColorFilter.matrix(new Float32Array(this.#state.colorMatrix));
      paint.setColorFilter(filter);
      filter.free();
    }
    return paint;
  }

  #paragraph(text: string, maxWidth: number): Paragraph {
    const font = parseFont(this.#state.font);
    const color = parseColor(
      typeof this.#state.fillStyle === "string" ? this.#state.fillStyle : "#000000",
    );
    return new Paragraph(
      this.fonts,
      text,
      font.families.join("\n"),
      font.size,
      font.weight,
      font.italic,
      color[0],
      color[1],
      color[2],
      color[3] * this.#state.globalAlpha,
      parsePixels(this.#state.letterSpacing),
      parsePixels(this.#state.wordSpacing),
      0,
      0,
      0,
      "",
      maxWidth,
    );
  }

  #textOffset(paragraph: Paragraph): [number, number] {
    const horizontal =
      this.#state.textAlign === "center"
        ? -paragraph.width / 2
        : this.#state.textAlign === "right" || this.#state.textAlign === "end"
          ? -paragraph.width
          : 0;
    const vertical =
      this.#state.textBaseline === "top"
        ? paragraph.topBaselineOrigin
        : this.#state.textBaseline === "hanging"
          ? paragraph.hangingBaselineOrigin
        : this.#state.textBaseline === "middle"
          ? paragraph.middleBaselineOrigin
        : this.#state.textBaseline === "bottom"
            ? paragraph.bottomBaselineOrigin
        : this.#state.textBaseline === "ideographic"
            ? paragraph.ideographicBaselineOrigin
            : -paragraph.alphabeticBaseline;
    return [horizontal, vertical];
  }

  #markDirty(): void {
    this.#dirty = true;
    this.#schedule();
  }

  #schedule(): void {
    if (!this.#autoPresent || this.#scheduled) return;
    this.#scheduled = true;
    requestAnimationFrame(() => this.present());
  }

  #releaseHistory(): void {
    for (const list of this.#history) list.free();
    this.#history = [];
  }

  #replayState(): void {
    let appliedTransform = identity;
    let appliedClipCount = 0;
    for (const saved of this.#stack) {
      [appliedTransform, appliedClipCount] = replayState(
        this.#builder,
        appliedTransform,
        appliedClipCount,
        saved,
      );
      this.#builder.save();
    }
    replayState(this.#builder, appliedTransform, appliedClipCount, this.#state);
  }
}

export class ValoTextMetrics {
  constructor(
    readonly width: number,
    readonly actualBoundingBoxLeft: number,
    readonly actualBoundingBoxRight: number,
    readonly actualBoundingBoxAscent: number,
    readonly actualBoundingBoxDescent: number,
    readonly fontBoundingBoxAscent: number,
    readonly fontBoundingBoxDescent: number,
    readonly emHeightAscent: number,
    readonly emHeightDescent: number,
  ) {}
}

function textHorizontalScale(width: number, maxWidth: number): number {
  if (!Number.isFinite(maxWidth) || maxWidth < 0 || width <= maxWidth || width === 0) return 1;
  return maxWidth / width;
}

function replayState(
  builder: DisplayListBuilder,
  currentTransform: Affine,
  currentClipCount: number,
  state: State,
): [Affine, number] {
  let transform = currentTransform;
  for (const clip of state.clips.slice(currentClipCount)) {
    transform = setBuilderTransform(builder, transform, clip.transform);
    builder.clipPath(clip.path, clip.rule, 0);
  }
  transform = setBuilderTransform(builder, transform, state.transform);
  return [transform, state.clips.length];
}

function setBuilderTransform(
  builder: DisplayListBuilder,
  current: Affine,
  target: Affine,
): Affine {
  const currentInverse = inverse(current);
  if (currentInverse) builder.transform(new Float32Array(multiply(currentInverse, target)));
  return target;
}

function cloneState(state: State): State {
  return {
    ...state,
    transform: [...state.transform] as Affine,
    clips: state.clips.map((clip) => ({
      path: clip.path.clone(),
      rule: clip.rule,
      transform: [...clip.transform] as Affine,
    })),
    lineDash: [...state.lineDash],
    colorMatrix: state.colorMatrix ? [...state.colorMatrix] : undefined,
  };
}

function disposeState(state: State): void {
  for (const clip of state.clips) clip.path.free();
  state.clips = [];
}

function sweep(start: number, end: number, counterclockwise: boolean): number {
  const fullTurn = Math.PI * 2;
  let result = end - start;
  if (!counterclockwise && result < 0) result = ((result % fullTurn) + fullTurn) % fullTurn;
  if (counterclockwise && result > 0) result = -(((-result % fullTurn) + fullTurn) % fullTurn);
  if (Math.abs(end - start) >= fullTurn) return counterclockwise ? -fullTurn : fullTurn;
  return result;
}

function ruleId(rule: ValoFillRule): number {
  return rule === "evenodd" ? 1 : 0;
}

function radiiArray(radii: number | readonly number[]): Float32Array {
  return new Float32Array(typeof radii === "number" ? [radii] : radii);
}

function capId(cap: CanvasLineCap): number {
  return cap === "round" ? 1 : cap === "square" ? 2 : 0;
}

function joinId(join: CanvasLineJoin): number {
  return join === "round" ? 1 : join === "bevel" ? 2 : 0;
}

function domMatrixValues(value: DOMMatrix2DInit): Affine {
  const matrix = DOMMatrix.fromMatrix(value);
  return [matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f];
}

function equalMatrix(left: Affine, right: Affine): boolean {
  return left.every((value, index) => value === right[index]);
}

function imageArguments(
  image: Image,
  values: readonly number[],
): readonly [number, number, number, number, number, number, number, number] {
  if (values.length === 2) {
    return [0, 0, image.width, image.height, values[0]!, values[1]!, image.width, image.height];
  }
  if (values.length === 4) {
    return [0, 0, image.width, image.height, values[0]!, values[1]!, values[2]!, values[3]!];
  }
  if (values.length === 8) {
    return [values[0]!, values[1]!, values[2]!, values[3]!, values[4]!, values[5]!, values[6]!, values[7]!];
  }
  throw new TypeError("drawImage expects 3, 5, or 9 total arguments");
}

function shadowColorMatrix([red, green, blue, alpha]: Rgba): Float32Array {
  return new Float32Array([
    0, 0, 0, 0, red,
    0, 0, 0, 0, green,
    0, 0, 0, 0, blue,
    0, 0, 0, alpha, 0,
  ]);
}

interface ParsedFont {
  italic: boolean;
  weight: number;
  size: number;
  families: string[];
}

function parseFont(value: string): ParsedFont {
  const match = /^(?:(italic)\s+)?(?:(normal|bold|[1-9]00)\s+)?([0-9.]+)px\s+(.+)$/.exec(
    value.trim(),
  );
  if (!match) {
    throw new TypeError(
      `Unsupported font '${value}'. Use '[italic] [weight] <size>px <family-list>'.`,
    );
  }
  return {
    italic: match[1] === "italic",
    weight: match[2] === "bold" ? 700 : Number.parseInt(match[2] ?? "400", 10),
    size: Number.parseFloat(match[3]!),
    families: match[4]!.split(",").map((family) => family.trim().replace(/^['"]|['"]$/g, "")),
  };
}

function parsePixels(value: string): number {
  const match = /^(-?[0-9.]+)px$/.exec(value.trim());
  if (!match) throw new TypeError(`Expected a pixel length, received '${value}'`);
  return Number.parseFloat(match[1]!);
}

const blendModes: Partial<Record<GlobalCompositeOperation, number>> = {
  copy: 1,
  "source-over": 3,
  "destination-over": 4,
  "source-in": 5,
  "destination-in": 6,
  "source-out": 7,
  "destination-out": 8,
  "source-atop": 9,
  "destination-atop": 10,
  xor: 11,
  lighter: 12,
  screen: 14,
  overlay: 15,
  darken: 16,
  lighten: 17,
  "color-dodge": 18,
  "color-burn": 19,
  "hard-light": 20,
  "soft-light": 21,
  difference: 22,
  exclusion: 23,
  multiply: 24,
  hue: 25,
  saturation: 26,
  color: 27,
  luminosity: 28,
};

const blendModeNames = Object.fromEntries(
  Object.entries(blendModes).map(([name, identifier]) => [identifier, name]),
) as Partial<Record<number, GlobalCompositeOperation>>;

// Canvas applies these operators to the transparent source outside a shape,
// so the operation covers the active clip rather than only the source ink.
const canvasDestructiveBlendModes = new Set([
  blendModes.copy!,
  blendModes["source-in"]!,
  blendModes["destination-in"]!,
  blendModes["source-out"]!,
  blendModes["destination-atop"]!,
]);
