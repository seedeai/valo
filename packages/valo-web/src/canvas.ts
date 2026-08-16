import {
  ColorFilter,
  DisplayListBuilder,
  FontCollection,
  ImageFilter,
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
import { ImageSourceCache, type ValoImageSource } from "./images.js";
import { radiiArray, sweep, ValoPath2D } from "./path2d.js";
import {
  DEFAULT_FONT_SIZE,
  directionId,
  fontStretchPercentages,
  parseFilter,
  parseFont,
  parsePixels,
  pixelsOrDefault,
  variantCapsIds,
  type FilterStage,
  type FontSizeReference,
} from "./css.js";

export type ValoCanvasStyle = string | ValoCanvasGradient | ValoCanvasPattern;
export type ValoFillRule = "nonzero" | "evenodd";
/** Either path shape the drawing methods accept. */
export type ValoPathArgument = ValoPath2D | Path;

export interface ValoCanvasOptions {
  autoPresent?: boolean;
  clearColor?: string;
}

/** What `getContextAttributes()` reports. Valo's surface is always alpha-capable
 *  and never desynchronized: it presents once per frame under its own control. */
export interface ValoContextAttributes {
  alpha: boolean;
  colorSpace: PredefinedColorSpace;
  desynchronized: boolean;
  willReadFrequently: boolean;
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
  fontKerning: CanvasFontKerning;
  fontStretch: CanvasFontStretch;
  fontVariantCaps: CanvasFontVariantCaps;
  textAlign: CanvasTextAlign;
  textBaseline: CanvasTextBaseline;
  direction: CanvasDirection;
  letterSpacing: string;
  wordSpacing: string;
  textRendering: CanvasTextRendering;
  filter: string;
  imageFilter: ImageFilter | undefined;
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
  fontKerning: "auto",
  fontStretch: "normal",
  fontVariantCaps: "normal",
  textAlign: "start",
  textBaseline: "alphabetic",
  direction: "inherit",
  letterSpacing: "0px",
  wordSpacing: "0px",
  textRendering: "auto",
  filter: "none",
  imageFilter: undefined,
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
  #path = new ValoPath2D();
  #builder = new DisplayListBuilder();
  /**
   * The next present starts from a CLEAR rather than from what is already on
   * the canvas — `reset`, `beginFrame`, or a `clearRect` that provably covers
   * the whole surface. The renderer skips its restore draw then, which is
   * valo's analogue of Chrome dropping its copy-on-write when the new record
   * replaces everything.
   *
   * True at construction: the first present has nothing to preserve.
   */
  #discardNext = true;
  #images: ImageSourceCache;
  #scheduled = false;
  #dirty = false;
  #lastStats: ValoFrameStats | undefined;

  constructor(canvas: HTMLCanvasElement, renderer: Renderer, options: ValoCanvasOptions = {}) {
    this.canvas = canvas;
    this.#renderer = renderer;
    this.#images = new ImageSourceCache(renderer);
    this.#autoPresent = options.autoPresent ?? true;
    this.#clearColor = parseColor(options.clearColor ?? "transparent");
  }

  get fillStyle(): ValoCanvasStyle {
    return this.#state.fillStyle;
  }
  set fillStyle(value: ValoCanvasStyle) {
    const style = acceptedStyle(value);
    if (style !== undefined) this.#state.fillStyle = style;
  }

  get strokeStyle(): ValoCanvasStyle {
    return this.#state.strokeStyle;
  }
  set strokeStyle(value: ValoCanvasStyle) {
    const style = acceptedStyle(value);
    if (style !== undefined) this.#state.strokeStyle = style;
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
    const text = String(value);
    if (!accepted(() => parseColor(text))) return;
    this.#state.shadowColor = text;
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
    const text = String(value);
    if (!accepted(() => parseFont(text, this.#fontSizeReference()))) return;
    this.#state.font = text;
  }

  get fontKerning(): CanvasFontKerning {
    return this.#state.fontKerning;
  }
  set fontKerning(value: CanvasFontKerning) {
    if (value === "auto" || value === "normal" || value === "none") {
      this.#state.fontKerning = value;
    }
  }

  get fontStretch(): CanvasFontStretch {
    return this.#state.fontStretch;
  }
  set fontStretch(value: CanvasFontStretch) {
    if (value in fontStretchPercentages) this.#state.fontStretch = value;
  }

  get fontVariantCaps(): CanvasFontVariantCaps {
    return this.#state.fontVariantCaps;
  }
  set fontVariantCaps(value: CanvasFontVariantCaps) {
    if (value in variantCapsIds) this.#state.fontVariantCaps = value;
  }

  get direction(): CanvasDirection {
    return this.#state.direction;
  }
  set direction(value: CanvasDirection) {
    if (value === "ltr" || value === "rtl" || value === "inherit") {
      this.#state.direction = value;
    }
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
    const text = String(value);
    if (!accepted(() => parsePixels(text))) return;
    this.#state.letterSpacing = text;
  }

  get wordSpacing(): string {
    return this.#state.wordSpacing;
  }
  set wordSpacing(value: string) {
    const text = String(value);
    if (!accepted(() => parsePixels(text))) return;
    this.#state.wordSpacing = text;
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

  get filter(): string {
    return this.#state.filter;
  }
  set filter(value: string) {
    const text = String(value);
    const stages = parseFilter(text);
    if (!stages) return;
    const filter = stages.length > 0 ? buildImageFilter(stages) : undefined;
    this.#state.imageFilter?.free();
    this.#state.filter = text;
    this.#state.imageFilter = filter;
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
    this.#discardNext = true;
    this.#builder.free();
    this.#path.free();
    disposeState(this.#state);
    for (const saved of this.#stack) disposeState(saved);
    this.#state = defaultState();
    this.#stack = [];
    this.#path = new ValoPath2D();
    this.#builder = new DisplayListBuilder();
    this.#dirty = true;
    this.#schedule();
  }

  beginPath(): void {
    this.#path.free();
    this.#path = new ValoPath2D();
  }

  // The current path is a `ValoPath2D`, so these are one-line forwards. The
  // spec's non-finite and radius rules live there and cannot drift between
  // the two surfaces — which is exactly how `Path2D` ended up unguarded while
  // the context methods were guarded.
  closePath(): void {
    this.#path.closePath();
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
    this.#path.roundRect(x, y, width, height, radii);
  }

  arc(
    x: number,
    y: number,
    radius: number,
    startAngle: number,
    endAngle: number,
    counterclockwise = false,
  ): void {
    this.#path.arc(x, y, radius, startAngle, endAngle, counterclockwise);
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
    this.#path.ellipse(x, y, radiusX, radiusY, rotation, startAngle, endAngle, counterclockwise);
  }

  arcTo(x1: number, y1: number, x2: number, y2: number, radius: number): void {
    this.#path.arcTo(x1, y1, x2, y2, radius);
  }

  fill(pathOrRule?: ValoPathArgument | ValoFillRule, rule: ValoFillRule = "nonzero"): void {
    const path = rawPath(pathOrRule) ?? this.#path.toRaw();
    const fillRule = typeof pathOrRule === "string" ? pathOrRule : rule;
    this.#drawShape((paint) => this.#builder.drawPath(path, ruleId(fillRule), paint), false);
  }

  stroke(path?: ValoPathArgument): void {
    const target = rawPath(path) ?? this.#path.toRaw();
    this.#drawShape((paint) => this.#builder.drawPath(target, 0, paint), true);
  }

  clip(pathOrRule?: ValoPathArgument | ValoFillRule, rule: ValoFillRule = "nonzero"): void {
    const path = rawPath(pathOrRule) ?? this.#path.toRaw();
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
    if (!allFinite(x, y, width, height)) return;
    const [left, top, w, h] = normalizedRectangle(x, y, width, height);
    this.#drawShape((paint) => this.#builder.drawRect(left, top, w, h, paint), false);
  }

  strokeRect(x: number, y: number, width: number, height: number): void {
    if (!allFinite(x, y, width, height)) return;
    this.#drawShape((paint) => this.#builder.drawRect(x, y, width, height, paint), true);
  }

  clearRect(x: number, y: number, width: number, height: number): void {
    if (!allFinite(x, y, width, height)) return;
    [x, y, width, height] = normalizedRectangle(x, y, width, height);
    if (
      clearsWholeCanvas(this.#state.transform, this.#state.clips.length, [x, y, width, height], [
        this.canvas.width,
        this.canvas.height,
      ])
    ) {
      this.#discardNext = true;
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

  drawImage(source: ValoImageSource, ...argumentsList: number[]): void {
    // Arity is overload resolution, so it fails BEFORE the source is looked
    // at: `drawImage(img, 1, 2, 3)` is a TypeError even for an image that has
    // not decoded, where a correct call would draw nothing at all.
    if (argumentsList.length !== 2 && argumentsList.length !== 4 && argumentsList.length !== 8) {
      throw new TypeError("drawImage expects 3, 5, or 9 total arguments");
    }
    if (!allFinite(...argumentsList)) return;
    const image = this.#resolveImage(source);
    // A source with nothing to draw yet — an undecoded `<img>`, a `<video>`
    // with no current frame — is a silent no-op, as in Canvas2D.
    if (!image) return;
    const rectangles = imageArguments(image, argumentsList);
    if (!rectangles) return;
    const [sourceX, sourceY, sourceWidth, sourceHeight, destinationX, destinationY, destinationWidth, destinationHeight] =
      rectangles;
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
        this.#mipmapMode(),
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

  /**
   * KNOWN DIVERGENCE, left deliberately: this pattern is LIVE. WHATWG checks
   * the source's usability here and snapshots it, so later changes to the
   * image must not show; Valo instead re-resolves the source at every paint,
   * which means a canvas mutated after `createPattern()` changes the pattern,
   * and a not-yet-decoded image yields a pattern rather than `null`.
   *
   * Snapshotting is a behaviour change the owner should weigh — it costs an
   * upload per `createPattern` call and makes a pattern over an animating
   * canvas stop animating — so it is flagged rather than quietly decided.
   */
  createPattern(source: ValoImageSource, repetition: string | null = "repeat"): ValoCanvasPattern {
    return new ValoCanvasPattern(source, repetition);
  }

  isPointInPath(
    pathOrX: ValoPathArgument | number,
    xOrY: number,
    yOrRule?: number | ValoFillRule,
    maybeRule: ValoFillRule = "nonzero",
  ): boolean {
    const explicit = rawPath(pathOrX);
    const path = explicit ?? this.#path.toRaw();
    const [x, y] = explicit ? [xOrY, yOrRule as number] : [pathOrX as number, xOrY];
    const rule = explicit
      ? maybeRule
      : ((yOrRule as ValoFillRule | undefined) ?? "nonzero");
    const local = this.#toPathSpace(x, y);
    if (!local) return false;
    return path.contains(local[0], local[1], ruleId(rule));
  }

  isPointInStroke(pathOrX: ValoPathArgument | number, xOrY: number, y?: number): boolean {
    const explicit = rawPath(pathOrX);
    const path = explicit ?? this.#path.toRaw();
    const [pointX, pointY] = explicit ? [xOrY, y as number] : [pathOrX as number, xOrY];
    const local = this.#toPathSpace(pointX, pointY);
    if (!local) return false;
    return path.strokeContains(
      local[0],
      local[1],
      this.#state.lineWidth,
      capId(this.#state.lineCap),
      joinId(this.#state.lineJoin),
      this.#state.miterLimit,
      new Float32Array(this.#state.lineDash),
      this.#state.lineDashOffset,
    );
  }

  /** Whether the GPU device was lost. Valo does not yet observe device loss,
   *  so this is honest only for the case it can answer: a live context. */
  isContextLost(): boolean {
    return false;
  }

  getContextAttributes(): ValoContextAttributes {
    return {
      alpha: true,
      colorSpace: "srgb",
      desynchronized: false,
      willReadFrequently: false,
    };
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
    if (!allFinite(a, b, c, d, e, f)) return;
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
    if (!allFinite(...target)) return;
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

  fillText(text: string, x: number, y: number, maxWidth?: number): void {
    this.#drawText(text, x, y, maxWidth, false);
  }

  strokeText(text: string, x: number, y: number, maxWidth?: number): void {
    this.#drawText(text, x, y, maxWidth, true);
  }

  measureText(text: string): ValoTextMetrics {
    const paragraph = this.#paragraph(text, Number.POSITIVE_INFINITY);
    const [horizontal, vertical] = this.#textOffset(paragraph);
    const alphabeticDisplacement = vertical + paragraph.alphabeticBaseline;
    const hasOutline = paragraph.hasOutline;
    // Text with no ink still HAS an actual bounding box: an empty one sitting
    // on the alphabetic baseline. Reporting 0/0 instead would place it at the
    // anchor, which moves with `textBaseline` and is a different point —
    // measuring `""` under `textBaseline: "top"` would claim the box is at the
    // top of the em rather than a font's ascent below it.
    const emptyAscent = -alphabeticDisplacement;
    const metrics = new ValoTextMetrics(
      paragraph.width,
      hasOutline ? -horizontal - paragraph.outlineLeft : 0,
      hasOutline ? horizontal + paragraph.outlineRight : 0,
      hasOutline ? -vertical - paragraph.outlineTop : emptyAscent,
      hasOutline ? vertical + paragraph.outlineBottom : -emptyAscent,
      paragraph.primaryFontAscent - alphabeticDisplacement,
      paragraph.primaryFontDescent + alphabeticDisplacement,
      paragraph.emAscent - alphabeticDisplacement,
      paragraph.emDescent + alphabeticDisplacement,
      // The three baselines, measured UP from the anchor `textBaseline` put
      // the text on — the same sign convention as the ascents above.
      -alphabeticDisplacement,
      paragraph.hangingBaselineOffset - alphabeticDisplacement,
      paragraph.ideographicBaselineOffset - alphabeticDisplacement,
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

  /**
   * `ImageData` is one of the sources `copyExternalImageToTexture` accepts, so
   * this is an ordinary upload-and-blit. Unlike every other source it is NOT
   * cached: the caller owns a mutable `Uint8ClampedArray` and typically edits
   * it between calls, so reusing a texture would show stale pixels.
   *
   * Per the spec this ignores the transform, globalAlpha, the blend mode and
   * every filter — it REPLACES the destination pixels. DIVERGENCE: the spec
   * also says it ignores the CLIP, which valo cannot honour, because clips are
   * recorded into the display list and a draw cannot step outside the scope it
   * sits in. Under an active clip the write is clipped.
   */
  putImageData(
    data: ImageData,
    x: number,
    y: number,
    dirtyX = 0,
    dirtyY = 0,
    dirtyWidth = data.width,
    dirtyHeight = data.height,
  ): void {
    const region = clampedDirtyRect(data, dirtyX, dirtyY, dirtyWidth, dirtyHeight);
    if (!region) return;
    const [left, top, width, height] = region;
    // Only the dirty rectangle is copied: a one-pixel update to a large
    // ImageData would otherwise move the whole buffer to the GPU and leave a
    // full-size texture retained by the display list to sample one texel.
    const image = this.#renderer.uploadExternalImageRegion(data, left, top, width, height);
    const paint = new Paint(1, 1, 1, 1);
    paint.setBlendMode(blendModes.copy!);
    // Everything about the current state is bypassed, including the
    // transform, so the placement is recorded under an identity matrix.
    this.#builder.save();
    const currentInverse = inverse(this.#state.transform);
    if (currentInverse) this.#builder.transform(new Float32Array(currentInverse));
    this.#builder.drawImageRect(
      image,
      0,
      0,
      width,
      height,
      x + left,
      y + top,
      width,
      height,
      1,
      0,
      0,
      0,
      paint,
    );
    this.#builder.restore();
    paint.free();
    image.free();
    this.#markDirty();
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
    this.#discardNext = true;
    this.#builder.free();
    this.#builder = new DisplayListBuilder();
    this.#replayState();
    this.#dirty = true;
  }

  /** Present all mutations recorded since the previous call. */
  present(): ValoFrameStats | undefined {
    this.#scheduled = false;
    if (!this.#dirty) return this.#lastStats;

    // Only this frame's new work crosses the boundary. The renderer keeps a
    // persistent backing and restores it before drawing the delta, so the
    // cost of a present is the delta plus one fullscreen restore — flat,
    // however long the canvas has been accumulating.
    const displayList = this.#builder.build();
    this.#builder.free();
    const [red, green, blue, alpha] = this.#clearColor;
    const rawStats = this.#renderer.render(
      displayList,
      this.#discardNext,
      red,
      green,
      blue,
      alpha,
    );
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
    this.#discardNext = false;
    // Live sources (a `<video>`, a `<canvas>`) become stale here, so the next
    // frame re-reads them once however many times they are drawn.
    this.#images.advanceFrame();
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

  /** A hit-test point carried from canvas space into path space. */
  #toPathSpace(x: number, y: number): [number, number] | undefined {
    const inverseTransform = inverse(this.#state.transform);
    if (!inverseTransform) return undefined;
    return mapPoint(inverseTransform, x, y);
  }

  /**
   * The image for a source, uploading or refreshing it as needed.
   *
   * Nothing is retained across presents any more — the persistent backing
   * holds the pixels, not a list of past display lists — so a live source can
   * always refresh its texture in place. The already-submitted draw keeps the
   * pixels it was recorded with because the queue orders the copy after it.
   */
  #resolveImage(source: ValoImageSource): Image | undefined {
    return this.#images.resolve(source);
  }

  /**
   * `imageSmoothingQuality` as a mip mode. This is a REAL sampling difference,
   * not a stored-and-ignored setting: `low` pins the sampler to level 0, so a
   * minified draw stays sharp and aliases, while the higher tiers pick and
   * then blend mip levels. Skia's own top tier is cubic resampling, which
   * valo has no pipeline for, so `high` lands on trilinear.
   */
  #mipmapMode(): number {
    switch (this.#state.imageSmoothingQuality) {
      case "low": return 0;
      case "medium": return 1;
      default: return 2;
    }
  }

  #drawShape(
    draw: (paint: Paint) => void,
    stroke: boolean,
    transform: Affine = this.#state.transform,
  ): void {
    this.#drawStyled(
      draw,
      stroke,
      stroke ? this.#state.strokeStyle : this.#state.fillStyle,
      transform,
    );
  }

  #drawStyled(
    draw: (paint: Paint) => void,
    stroke: boolean,
    style: ValoCanvasStyle,
    transform: Affine = this.#state.transform,
  ): void {
    const shadowColor = this.#activeShadowColor();
    if (shadowColor) {
      this.#drawCanvasSource(
        (blendMode) => this.#drawShadow(draw, stroke, style, shadowColor, blendMode, transform),
        transform,
        false,
      );
    }
    this.#drawCanvasSource(
      (blendMode) => {
        const paint = this.#paint(style, stroke, true, blendMode);
        draw(paint);
        paint.free();
      },
      transform,
      true,
    );
    this.#markDirty();
  }

  // Each logical Canvas source (shadow, then shape) owns its compositing
  // layer. CSS filters run on that source in device space; destructive modes
  // also need the layer's transparent pixels across the active clip.
  #drawCanvasSource(
    draw: (blendMode: number) => void,
    transform: Affine,
    applyFilter: boolean,
  ): void {
    const imageFilter = applyFilter ? this.#state.imageFilter : undefined;
    const destructive = canvasDestructiveBlendModes.has(this.#state.blendMode);
    if (!destructive && !imageFilter) {
      draw(this.#state.blendMode);
      return;
    }

    const inverseTransform = imageFilter ? inverse(transform) : undefined;
    if (imageFilter && !inverseTransform) return;
    if (inverseTransform) {
      this.#builder.save();
      this.#builder.transform(new Float32Array(inverseTransform));
    }
    const layerPaint = new Paint(1, 1, 1, 1);
    layerPaint.setBlendMode(this.#state.blendMode);
    if (imageFilter) layerPaint.setImageFilter(imageFilter);
    this.#builder.saveLayer(layerPaint);
    layerPaint.free();
    if (imageFilter) this.#builder.transform(new Float32Array(transform));
    draw(blendModes["source-over"]!);
    this.#builder.restore();
    if (inverseTransform) this.#builder.restore();
  }

  #drawText(
    text: string,
    x: number,
    y: number,
    maxWidth: number | undefined,
    stroke: boolean,
  ): void {
    // Text preparation returns when any SUPPLIED argument is non-finite, so
    // an omitted `maxWidth` and an explicit `Infinity` are different calls:
    // the first draws, the second draws nothing.
    if (!allFinite(x, y)) return;
    if (maxWidth !== undefined && (!Number.isFinite(maxWidth) || maxWidth <= 0)) return;
    const limit = maxWidth ?? Number.POSITIVE_INFINITY;
    const paragraph = this.#paragraph(text, Number.POSITIVE_INFINITY);
    const horizontalScale = textHorizontalScale(paragraph.width, limit);
    const [left, top] = this.#textOffset(paragraph);
    this.#builder.save();
    this.#builder.translate(x, y);
    if (horizontalScale !== 1) this.#builder.scale(horizontalScale, 1);
    const translated = multiply(this.#state.transform, [1, 0, 0, 1, x, y]);
    const transform =
      horizontalScale === 1
        ? translated
        : multiply(translated, [horizontalScale, 0, 0, 1, 0, 0]);
    this.#drawShape(
      (paint) => this.#builder.drawParagraphWith(paragraph, left, top, paint),
      stroke,
      transform,
    );
    this.#builder.restore();
    paragraph.free();
  }

  #drawShadow(
    draw: (paint: Paint) => void,
    stroke: boolean,
    style: ValoCanvasStyle,
    color: Rgba,
    blendMode: number,
    transform: Affine,
  ): void {
    const inverseTransform = inverse(transform);
    if (!inverseTransform) return;

    const layerPaint = new Paint(1, 1, 1, 1);
    layerPaint.setBlendMode(blendMode);
    const filter = ColorFilter.matrix(shadowColorMatrix(color));
    layerPaint.setColorFilter(filter);
    filter.free();
    if (this.#state.shadowBlur > 0) layerPaint.setMaskBlur(this.#state.shadowBlur / 2, 0);

    // Canvas shadows live in device space. Cancel the complete draw transform
    // before opening the effect layer, then place the original geometry below
    // a device-space translation. The layer's blur therefore also sees the
    // identity transform, matching Impeller's `respect_ctm = false` path.
    this.#builder.save();
    this.#builder.transform(new Float32Array(inverseTransform));
    this.#builder.saveLayer(layerPaint);
    layerPaint.free();
    this.#builder.translate(this.#state.shadowOffsetX, this.#state.shadowOffsetY);
    this.#builder.transform(new Float32Array(transform));
    const sourcePaint = this.#paint(style, stroke, false, blendModes["source-over"]!);
    draw(sourcePaint);
    sourcePaint.free();
    this.#builder.restore();
    this.#builder.restore();
  }

  #activeShadowColor(): Rgba | undefined {
    const color = parseColor(this.#state.shadowColor);
    if (
      color[3] === 0 ||
      (this.#state.shadowBlur === 0 &&
        this.#state.shadowOffsetX === 0 &&
        this.#state.shadowOffsetY === 0)
    ) {
      return undefined;
    }
    return color;
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
      const image = this.#resolveImage(style.source);
      if (image) {
        const shader = style.toRaw(image, this.#state.imageSmoothingEnabled, this.#mipmapMode());
        paint.setShader(shader);
        shader.free();
      } else {
        // A pattern over a source with no pixels yet paints nothing, the same
        // way `drawImage` on one does.
        paint.setColor(0, 0, 0, 0);
      }
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
    const font = parseFont(this.#state.font, this.#fontSizeReference());
    const color = parseColor(
      typeof this.#state.fillStyle === "string" ? this.#state.fillStyle : "#000000",
    );
    const preparedText = text.replace(/[\u0009-\u000d]/g, " ");
    // The shorthand seeds small-caps and width; the dedicated attributes win
    // once they have been moved off their own defaults.
    const variantCaps =
      this.#state.fontVariantCaps === "normal" && font.smallCaps
        ? variantCapsIds["small-caps"]
        : variantCapsIds[this.#state.fontVariantCaps];
    const stretch =
      this.#state.fontStretch === "normal"
        ? font.stretch
        : fontStretchPercentages[this.#state.fontStretch];
    return new Paragraph(
      this.fonts,
      preparedText,
      font.families.join("\n"),
      font.size,
      font.weight,
      font.italic,
      color[0],
      color[1],
      color[2],
      color[3] * this.#state.globalAlpha,
      stretch,
      this.#state.fontKerning !== "none",
      variantCaps,
      parsePixels(this.#state.letterSpacing),
      parsePixels(this.#state.wordSpacing),
      font.lineHeight ?? 0,
      0,
      directionId(this.#state.direction),
      0,
      "",
      maxWidth,
      true,
    );
  }

  /**
   * What `em`, `ex`, `ch` and `%` in the `font` shorthand resolve against: the
   * canvas element's own computed font-size, with `rem` against the root
   * element. Outside a document both fall back to the CSS initial 16px rather
   * than to a guess.
   */
  #fontSizeReference(): FontSizeReference {
    if (typeof getComputedStyle !== "function" || !this.canvas.isConnected) {
      return { element: DEFAULT_FONT_SIZE, root: DEFAULT_FONT_SIZE };
    }
    return {
      element: pixelsOrDefault(getComputedStyle(this.canvas).fontSize),
      root: pixelsOrDefault(
        getComputedStyle(this.canvas.ownerDocument.documentElement).fontSize,
      ),
    };
  }

  #textOffset(paragraph: Paragraph): [number, number] {
    // `start` and `end` follow the writing direction, so they are the only
    // alignments `direction` can move. `inherit` keeps the LTR reading, since
    // valo has no ambient element style to inherit from.
    const rightToLeft = this.#state.direction === "rtl";
    const align = this.#state.textAlign;
    const alignsRight =
      align === "right" ||
      (align === "end" && !rightToLeft) ||
      (align === "start" && rightToLeft);
    const horizontal =
      align === "center" ? -paragraph.width / 2 : alignsRight ? -paragraph.width : 0;
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
    readonly alphabeticBaseline: number,
    /** Approximated at 0.8 × ascent: valo does not read the OpenType `BASE`
     *  table, so it always takes the fallback Skia uses for fonts without one. */
    readonly hangingBaseline: number,
    readonly ideographicBaseline: number,
  ) {}
}

/**
 * Canvas2D's rule for every style attribute: a value the parser cannot make
 * sense of is IGNORED, leaving the previous one in place. Nothing a setter
 * receives throws — not `null`, not `"12"`, not `"chartreuse-ish"`.
 *
 * The parsers themselves throw because their other callers are specified to:
 * `addColorStop` raises `SyntaxError`, and the paragraph builder must not
 * silently lay text out in the wrong font. This is the one place that turns
 * that back into the setters' silence, so the `catch` covers exactly one
 * parse and nothing else.
 */
function accepted(parse: () => unknown): boolean {
  try {
    parse();
    return true;
  } catch {
    return false;
  }
}

/** A fill/stroke style the state may keep: gradients and patterns as they
 *  are, colour strings only once they parse. */
function acceptedStyle(value: ValoCanvasStyle): ValoCanvasStyle | undefined {
  if (value instanceof ValoCanvasGradient || value instanceof ValoCanvasPattern) return value;
  if (typeof value !== "string") return undefined;
  return accepted(() => parseColor(value)) ? value : undefined;
}

/**
 * Whether a `clearRect` provably wipes the ENTIRE canvas, so the next present
 * can start from a clear instead of restoring the persistent backing.
 *
 * Every condition is load-bearing. A transform or an active clip means the
 * recorded rectangle is not the region actually cleared, and the bounds have
 * to cover the surface rather than merely overlap it. Answering `true` too
 * eagerly silently discards pixels the canvas promised to keep, which is the
 * one failure of this predicate that produces no error — just missing ink.
 */
export function clearsWholeCanvas(
  transform: Affine,
  clipCount: number,
  rectangle: readonly [number, number, number, number],
  canvas: readonly [number, number],
): boolean {
  const [x, y, width, height] = rectangle;
  return (
    equalMatrix(transform, identity) &&
    clipCount === 0 &&
    x <= 0 &&
    y <= 0 &&
    x + width >= canvas[0] &&
    y + height >= canvas[1]
  );
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
    imageFilter: state.imageFilter?.clone(),
    colorMatrix: state.colorMatrix ? [...state.colorMatrix] : undefined,
  };
}

function disposeState(state: State): void {
  for (const clip of state.clips) clip.path.free();
  state.clips = [];
  state.imageFilter?.free();
  state.imageFilter = undefined;
}

function ruleId(rule: ValoFillRule): number {
  return rule === "evenodd" ? 1 : 0;
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

export type ImageRectangles = readonly [
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
];

/**
 * The source and destination rectangles for a `drawImage` call, after the
 * spec's two corrections. `undefined` = the call draws nothing.
 *
 * NEGATIVE sizes flip the rectangle rather than emptying it, so
 * `drawImage(img, 0, 0, 40, 40, 40, 40, -40, -40)` draws into `(0, 0, 40, 40)`
 * exactly like the positive form. Valo's `Rect` treats a negative extent as
 * empty, so without this the draw silently disappears.
 *
 * A source rectangle reaching OUTSIDE the image is clipped to it, and the
 * destination is clipped by the same proportion so the visible part keeps its
 * position and scale. Without this the sampler clamps instead, smearing the
 * image's border texels across the overhang.
 */
/**
 * Canvas2D IGNORES a call carrying a non-finite argument — it returns
 * silently rather than throwing or applying it. Valo's recorder has no such
 * rule and would take NaN at face value, and NaN spreads: it reaches the
 * transform stack and the recorded bounds, so every LATER draw in the frame
 * disappears too. One stray value blanks the rest of the canvas, which is why
 * this is a boundary guard rather than something the core defends against.
 */
function allFinite(...values: number[]): boolean {
  return values.every(Number.isFinite);
}

export function imageArguments(
  image: Image,
  values: readonly number[],
): ImageRectangles | undefined {
  if (values.length === 2) {
    return [0, 0, image.width, image.height, values[0]!, values[1]!, image.width, image.height];
  }
  if (values.length === 4) {
    const [x, y, width, height] = normalizedRectangle(values[0]!, values[1]!, values[2]!, values[3]!);
    return [0, 0, image.width, image.height, x, y, width, height];
  }
  const source = normalizedRectangle(values[0]!, values[1]!, values[2]!, values[3]!);
  const destination = normalizedRectangle(values[4]!, values[5]!, values[6]!, values[7]!);
  return clippedToImage(image, source, destination);
}

type Rectangle = readonly [number, number, number, number];

/**
 * Canvas2D rectangles admit negative extents and mean the same rectangle
 * walked the other way, so they normalise before use. Valo's `Rect` stores
 * width and height as given — correct for a geometry type, where a negative
 * extent is degenerate rather than reversed — so the conversion belongs here.
 *
 * `strokeRect` deliberately does NOT normalise: it is specified on the signed
 * dimensions, and its traversal direction is what places the dash phase.
 */
function normalizedRectangle(x: number, y: number, width: number, height: number): Rectangle {
  return [
    width < 0 ? x + width : x,
    height < 0 ? y + height : y,
    Math.abs(width),
    Math.abs(height),
  ];
}

function clippedToImage(
  image: Image,
  source: Rectangle,
  destination: Rectangle,
): ImageRectangles | undefined {
  const [sourceX, sourceY, sourceWidth, sourceHeight] = source;
  if (sourceWidth === 0 || sourceHeight === 0) return undefined;
  const left = Math.max(sourceX, 0);
  const top = Math.max(sourceY, 0);
  const right = Math.min(sourceX + sourceWidth, image.width);
  const bottom = Math.min(sourceY + sourceHeight, image.height);
  if (right <= left || bottom <= top) return undefined;

  const [destinationX, destinationY, destinationWidth, destinationHeight] = destination;
  const horizontal = destinationWidth / sourceWidth;
  const vertical = destinationHeight / sourceHeight;
  return [
    left,
    top,
    right - left,
    bottom - top,
    destinationX + (left - sourceX) * horizontal,
    destinationY + (top - sourceY) * vertical,
    (right - left) * horizontal,
    (bottom - top) * vertical,
  ];
}

function shadowColorMatrix([red, green, blue, alpha]: Rgba): Float32Array {
  return new Float32Array([
    0, 0, 0, 0, red,
    0, 0, 0, 0, green,
    0, 0, 0, 0, blue,
    0, 0, 0, alpha, 0,
  ]);
}

/** Either path shape, as the raw Valo path the recorder needs. */
function rawPath(source: unknown): Path | undefined {
  if (source instanceof ValoPath2D) return source.toRaw();
  if (source instanceof Path) return source;
  return undefined;
}

/**
 * `putImageData`'s dirty rectangle, normalized for negative extents and
 * clipped to the data — the spec's own steps. `undefined` = nothing to write.
 */
function clampedDirtyRect(
  data: ImageData,
  x: number,
  y: number,
  width: number,
  height: number,
): [number, number, number, number] | undefined {
  const left = Math.max(0, Math.min(x, x + width));
  const top = Math.max(0, Math.min(y, y + height));
  const right = Math.min(data.width, Math.max(x, x + width));
  const bottom = Math.min(data.height, Math.max(y, y + height));
  if (right <= left || bottom <= top) return undefined;
  return [left, top, right - left, bottom - top];
}

export

function buildImageFilter(stages: readonly FilterStage[]): ImageFilter {
  let chain: ImageFilter | undefined;
  for (const stage of stages) {
    const next =
      stage.type === "blur"
        ? ImageFilter.blur(stage.sigma, stage.sigma)
        : stage.type === "drop-shadow"
          ? ImageFilter.dropShadow(
              stage.offsetX,
              stage.offsetY,
              stage.sigma,
              stage.sigma,
              stage.color[0],
              stage.color[1],
              stage.color[2],
              stage.color[3],
            )
          : imageColorFilter(stage.matrix);
    if (!chain) {
      chain = next;
      continue;
    }
    const composed = ImageFilter.compose(next, chain);
    next.free();
    chain.free();
    chain = composed;
  }
  if (!chain) throw new Error("an image-filter chain needs at least one stage");
  return chain;
}

function imageColorFilter(matrix: readonly number[]): ImageFilter {
  const color = ColorFilter.matrix(new Float32Array(matrix));
  const image = ImageFilter.color(color);
  color.free();
  return image;
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
