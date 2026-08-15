import { Path } from "./raw.js";
import type { Affine } from "./matrix.js";

/**
 * Canvas2D's `Path2D` over a retained Valo path. A host polyfilling the
 * platform assigns this to `globalThis.Path2D`; the shape is the spec's.
 */
export class ValoPath2D {
  readonly #path: Path;

  constructor(source?: ValoPath2D | Path | string) {
    if (source instanceof ValoPath2D) {
      this.#path = source.#path.clone();
    } else if (source instanceof Path) {
      this.#path = source.clone();
    } else {
      this.#path = new Path();
      if (typeof source === "string") appendPathData(this.#path, source);
    }
  }

  /** The underlying Valo path. Callers must not free it. */
  toRaw(): Path {
    return this.#path;
  }

  /** Release the wasm-side path. Optional: the finalizer also collects it. */
  free(): void {
    this.#path.free();
  }

  addPath(path: ValoPath2D, transform?: DOMMatrix2DInit): void {
    canvasPath.addPath(this.#path, path.#path as never, transform);
  }

  closePath(): void {
    this.#path.close();
  }

  // Every rule lives in `canvasPath`; these are its Path2D face.
  moveTo(x: number, y: number): void {
    canvasPath.moveTo(this.#path, x, y);
  }

  lineTo(x: number, y: number): void {
    canvasPath.lineTo(this.#path, x, y);
  }

  quadraticCurveTo(controlX: number, controlY: number, x: number, y: number): void {
    canvasPath.quadraticCurveTo(this.#path, controlX, controlY, x, y);
  }

  bezierCurveTo(
    control1X: number,
    control1Y: number,
    control2X: number,
    control2Y: number,
    x: number,
    y: number,
  ): void {
    canvasPath.bezierCurveTo(this.#path, control1X, control1Y, control2X, control2Y, x, y);
  }

  arcTo(x1: number, y1: number, x2: number, y2: number, radius: number): void {
    canvasPath.arcTo(this.#path, x1, y1, x2, y2, radius);
  }

  rect(x: number, y: number, width: number, height: number): void {
    canvasPath.rect(this.#path, x, y, width, height);
  }

  roundRect(
    x: number,
    y: number,
    width: number,
    height: number,
    radii: number | readonly number[] = 0,
  ): void {
    canvasPath.roundRect(this.#path, x, y, width, height, radii);
  }

  arc(
    x: number,
    y: number,
    radius: number,
    startAngle: number,
    endAngle: number,
    counterclockwise = false,
  ): void {
    canvasPath.arc(this.#path, x, y, radius, startAngle, endAngle, counterclockwise);
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
    canvasPath.ellipse(
      this.#path,
      x,
      y,
      radiusX,
      radiusY,
      rotation,
      startAngle,
      endAngle,
      counterclockwise,
    );
  }
}

/** Every Canvas path method returns without mutating on non-finite input. */
export function allFinite(...values: number[]): boolean {
  return values.every(Number.isFinite);
}

/** Canvas2D's arc/ellipse take an END angle; Valo paths take a sweep. */
export function sweep(start: number, end: number, counterclockwise: boolean): number {
  const fullTurn = Math.PI * 2;
  let result = end - start;
  if (!counterclockwise && result < 0) result = ((result % fullTurn) + fullTurn) % fullTurn;
  if (counterclockwise && result > 0) result = -(((-result % fullTurn) + fullTurn) % fullTurn);
  if (Math.abs(end - start) >= fullTurn) return counterclockwise ? -fullTurn : fullTurn;
  return result;
}

export function radiiArray(radii: number | readonly number[]): Float32Array {
  return new Float32Array(typeof radii === "number" ? [radii] : radii);
}

/** A normalized `roundRect` box: origin at the top-left, corners in
 *  upper-left, upper-right, lower-right, lower-left order. */
export interface RoundRectGeometry {
  x: number;
  y: number;
  width: number;
  height: number;
  radii: Float32Array;
  /** Sign parity of the ORIGINAL extents: exactly one negative reverses it. */
  counterclockwise: boolean;
}

/**
 * `roundRect`'s box and corner radii after the spec's corrections.
 * `undefined` = the call returns without mutating the path.
 *
 * A negative extent FLIPS the rectangle, and the radii have to travel with
 * the flip: a negative width means the left corners' radii are used on the
 * right and vice versa, and a negative height swaps top for bottom.
 * Normalizing the box alone rounds the wrong corners whenever the radii are
 * asymmetric.
 *
 * The direction comes back separately because normalizing the box destroys
 * it. WHATWG: "If w and h are both greater than or equal to 0, or if both are
 * smaller than 0, then the path is drawn clockwise. Otherwise, it is drawn
 * counterclockwise." Under the non-zero fill rule that is the difference
 * between a second rectangle adding to an overlapping one and cancelling it
 * out, so the caller has to carry the bit down to the path builder.
 */
export function roundRectGeometry(
  x: number,
  y: number,
  width: number,
  height: number,
  radii: number | readonly number[],
): RoundRectGeometry | undefined {
  if (!allFinite(x, y, width, height)) return undefined;
  const corners = cornerRadii(radii);
  if (!corners) return undefined;

  const counterclockwise = width >= 0 !== height >= 0;
  let [upperLeft, upperRight, lowerRight, lowerLeft] = corners;
  if (width < 0) {
    [upperLeft, upperRight] = [upperRight, upperLeft];
    [lowerLeft, lowerRight] = [lowerRight, lowerLeft];
    x += width;
    width = -width;
  }
  if (height < 0) {
    [upperLeft, lowerLeft] = [lowerLeft, upperLeft];
    [upperRight, lowerRight] = [lowerRight, upperRight];
    y += height;
    height = -height;
  }
  return {
    x,
    y,
    width,
    height,
    radii: new Float32Array([upperLeft, upperRight, lowerRight, lowerLeft]),
    counterclockwise,
  };
}

/**
 * The 1-to-4 element radius list expanded to the four corners, in
 * upper-left, upper-right, lower-right, lower-left order — CSS's
 * shorthand rule, which the Canvas spec adopts.
 *
 * `undefined` = a non-finite radius, which makes the whole call a no-op.
 * A negative or wrong-length list throws, as the spec requires.
 */
function cornerRadii(
  radii: number | readonly number[],
): [number, number, number, number] | undefined {
  const list = typeof radii === "number" ? [radii] : [...radii];
  if (list.length < 1 || list.length > 4) {
    throw new RangeError(`roundRect takes 1 to 4 radii, received ${list.length}`);
  }
  // Non-finite before negative, matching the order `arcTo`/`arc`/`ellipse`
  // use: `-Infinity` is both, and the quiet return wins.
  if (!allFinite(...list)) return undefined;
  if (list.some((radius) => radius < 0)) {
    throw new RangeError("roundRect radii must be non-negative");
  }
  const [first, second = first, third = first, fourth = second] = list;
  return [first!, second!, third!, fourth!];
}

function affineOf(transform: DOMMatrix2DInit): Affine {
  const matrix = DOMMatrix.fromMatrix(transform);
  return [matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f];
}

// ── SVG path data ───────────────────────────────────────────────────────────
// SVG 2 §9.3.9 error handling: everything up to the first malformed token is
// kept and the rest is dropped. That is what browsers do with a bad `d`, so
// this parser stops rather than throws.

/**
 * The path verbs the parser emits. Valo's `Path` satisfies this structurally;
 * naming it keeps the grammar independent of the wasm boundary, which is what
 * lets the parser be tested without a GPU.
 */
export interface PathSink {
  moveTo(x: number, y: number): void;
  lineTo(x: number, y: number): void;
  quadraticCurveTo(controlX: number, controlY: number, x: number, y: number): void;
  bezierCurveTo(
    control1X: number,
    control1Y: number,
    control2X: number,
    control2Y: number,
    x: number,
    y: number,
  ): void;
  ellipse(
    centerX: number,
    centerY: number,
    radiusX: number,
    radiusY: number,
    rotation: number,
    startAngle: number,
    sweepAngle: number,
  ): void;
  close(): void;
}

/**
 * What the Canvas path methods write to: [`PathSink`] plus the verbs only
 * they produce. The SVG parser emits none of these — it lowers arcs to
 * ellipses and has no rectangle verb — so it keeps the narrower interface.
 */
export interface CanvasPathSink extends PathSink {
  arc(centerX: number, centerY: number, radius: number, startAngle: number, sweep: number): void;
  arcTo(x1: number, y1: number, x2: number, y2: number, radius: number): void;
  rect(x: number, y: number, width: number, height: number): void;
  roundRect(
    x: number,
    y: number,
    width: number,
    height: number,
    radii: Float32Array,
    counterclockwise: boolean,
  ): void;
  addPath(other: never, transform: Float32Array): void;
}

/**
 * The Canvas path verbs with the spec's argument rules applied, over any
 * sink. Every rule lives here exactly once — non-finite input returns without
 * mutating, and the radius errors are raised only AFTER that check — because
 * the alternative is two parallel copies, which is how the context methods
 * ended up guarded while `Path2D` did not.
 *
 * Taking a sink rather than a `Path` is what makes the rules testable without
 * a GPU or a wasm module, the same way the path-data parser is.
 */
export const canvasPath = {
  moveTo(sink: CanvasPathSink, x: number, y: number): void {
    if (!allFinite(x, y)) return;
    sink.moveTo(x, y);
  },

  lineTo(sink: CanvasPathSink, x: number, y: number): void {
    if (!allFinite(x, y)) return;
    sink.lineTo(x, y);
  },

  quadraticCurveTo(
    sink: CanvasPathSink,
    controlX: number,
    controlY: number,
    x: number,
    y: number,
  ): void {
    if (!allFinite(controlX, controlY, x, y)) return;
    sink.quadraticCurveTo(controlX, controlY, x, y);
  },

  bezierCurveTo(
    sink: CanvasPathSink,
    control1X: number,
    control1Y: number,
    control2X: number,
    control2Y: number,
    x: number,
    y: number,
  ): void {
    if (!allFinite(control1X, control1Y, control2X, control2Y, x, y)) return;
    sink.bezierCurveTo(control1X, control1Y, control2X, control2Y, x, y);
  },

  arcTo(sink: CanvasPathSink, x1: number, y1: number, x2: number, y2: number, radius: number): void {
    // Finiteness first: `arcTo(NaN, 0, 0, 0, -1)` returns quietly rather than
    // throwing, because the spec fixes that order.
    if (!allFinite(x1, y1, x2, y2, radius)) return;
    if (radius < 0) throw new DOMException("The radius must be non-negative", "IndexSizeError");
    sink.arcTo(x1, y1, x2, y2, radius);
  },

  arc(
    sink: CanvasPathSink,
    x: number,
    y: number,
    radius: number,
    startAngle: number,
    endAngle: number,
    counterclockwise: boolean,
  ): void {
    if (!allFinite(x, y, radius, startAngle, endAngle)) return;
    if (radius < 0) throw new DOMException("The radius must be non-negative", "IndexSizeError");
    sink.arc(x, y, radius, startAngle, sweep(startAngle, endAngle, counterclockwise));
  },

  ellipse(
    sink: CanvasPathSink,
    x: number,
    y: number,
    radiusX: number,
    radiusY: number,
    rotation: number,
    startAngle: number,
    endAngle: number,
    counterclockwise: boolean,
  ): void {
    if (!allFinite(x, y, radiusX, radiusY, rotation, startAngle, endAngle)) return;
    if (radiusX < 0 || radiusY < 0) {
      throw new DOMException("Ellipse radii must be non-negative", "IndexSizeError");
    }
    sink.ellipse(
      x,
      y,
      radiusX,
      radiusY,
      rotation,
      startAngle,
      sweep(startAngle, endAngle, counterclockwise),
    );
  },

  rect(sink: CanvasPathSink, x: number, y: number, width: number, height: number): void {
    if (!allFinite(x, y, width, height)) return;
    sink.rect(x, y, width, height);
  },

  roundRect(
    sink: CanvasPathSink,
    x: number,
    y: number,
    width: number,
    height: number,
    radii: number | readonly number[],
  ): void {
    const box = roundRectGeometry(x, y, width, height, radii);
    if (!box) return;
    sink.roundRect(box.x, box.y, box.width, box.height, box.radii, box.counterclockwise);
  },

  /**
   * `Path2D.addPath`. A non-finite entry anywhere in the transform makes the
   * whole call a no-op — otherwise it is a route for NaN to reach retained
   * path geometry, which is what the per-verb guards exist to prevent.
   */
  addPath(sink: CanvasPathSink, source: never, transform?: DOMMatrix2DInit): void {
    const values = transform ? affineOf(transform) : [];
    if (!allFinite(...values)) return;
    sink.addPath(source, new Float32Array(values));
  },
};

/** Cursor state a path-data command sequence carries between commands. */
interface Turtle {
  x: number;
  y: number;
  /** Where the current subpath began — where `Z` returns to. */
  startX: number;
  startY: number;
  /** Reflected control point for `S`/`T`, or the current point when the
   *  previous command was not the matching curve kind. */
  controlX: number;
  controlY: number;
  previous: string;
}

export function appendPathData(path: PathSink, data: string): void {
  const scanner = new Scanner(data);
  const turtle: Turtle = {
    x: 0,
    y: 0,
    startX: 0,
    startY: 0,
    controlX: 0,
    controlY: 0,
    previous: "",
  };
  let command = "";
  while (!scanner.atEnd()) {
    const next = scanner.command();
    if (next) {
      // "A path data segment must begin with a moveto command" (SVG Paths
      // §9.3.3). Anything else is an error from the first token, so nothing
      // at all is emitted.
      if (!command && next !== "M" && next !== "m") return;
      command = next;
    } else if (!command) {
      return; // data has to open with a command letter
    } else if (command === "M" || command === "m") {
      // An implicit repetition of moveto is lineto, per the grammar.
      command = command === "M" ? "L" : "l";
    } else if (command === "Z" || command === "z") {
      return; // closepath takes no arguments, so a number here is an error
    }
    if (!runCommand(path, scanner, turtle, command)) return;
  }
  // WHATWG ends `new Path2D(d)` by "creating a new subpath with the last
  // point in path". Only data ending in `Z` needs it: anything else already
  // leaves the contour open at its last point, whereas a closed one would
  // send a following `lineTo` off to start at its own endpoint.
  if (turtle.previous === "Z") path.moveTo(turtle.startX, turtle.startY);
}

/** `false` = malformed input; the caller keeps what was already emitted. */
function runCommand(path: PathSink, scanner: Scanner, turtle: Turtle, command: string): boolean {
  // "If a closepath is followed immediately by any other command, then the
  // next subpath starts at the same initial point as the current subpath."
  // Without the explicit reopen the next segment would begin at its own
  // endpoint and the connecting edge would vanish.
  if (turtle.previous === "Z" && !"MZ".includes(command.toUpperCase())) {
    path.moveTo(turtle.startX, turtle.startY);
  }
  const relative = command === command.toLowerCase();
  const originX = relative ? turtle.x : 0;
  const originY = relative ? turtle.y : 0;
  switch (command.toUpperCase()) {
    case "M": {
      const point = scanner.point(originX, originY);
      if (!point) return false;
      path.moveTo(point[0], point[1]);
      turtle.startX = point[0];
      turtle.startY = point[1];
      return finish(turtle, command, point[0], point[1], point[0], point[1]);
    }
    case "L": {
      const point = scanner.point(originX, originY);
      if (!point) return false;
      path.lineTo(point[0], point[1]);
      return finish(turtle, command, point[0], point[1], point[0], point[1]);
    }
    case "H": {
      const x = scanner.number();
      if (x === undefined) return false;
      const to = originX + x;
      path.lineTo(to, turtle.y);
      return finish(turtle, command, to, turtle.y, to, turtle.y);
    }
    case "V": {
      const y = scanner.number();
      if (y === undefined) return false;
      const to = originY + y;
      path.lineTo(turtle.x, to);
      return finish(turtle, command, turtle.x, to, turtle.x, to);
    }
    case "C": {
      const first = scanner.point(originX, originY);
      const second = scanner.point(originX, originY);
      const end = scanner.point(originX, originY);
      if (!first || !second || !end) return false;
      path.bezierCurveTo(first[0], first[1], second[0], second[1], end[0], end[1]);
      return finish(turtle, command, end[0], end[1], second[0], second[1]);
    }
    case "S": {
      const second = scanner.point(originX, originY);
      const end = scanner.point(originX, originY);
      if (!second || !end) return false;
      const [firstX, firstY] = reflected(turtle, "CS");
      path.bezierCurveTo(firstX, firstY, second[0], second[1], end[0], end[1]);
      return finish(turtle, command, end[0], end[1], second[0], second[1]);
    }
    case "Q": {
      const control = scanner.point(originX, originY);
      const end = scanner.point(originX, originY);
      if (!control || !end) return false;
      path.quadraticCurveTo(control[0], control[1], end[0], end[1]);
      return finish(turtle, command, end[0], end[1], control[0], control[1]);
    }
    case "T": {
      const end = scanner.point(originX, originY);
      if (!end) return false;
      const [controlX, controlY] = reflected(turtle, "QT");
      path.quadraticCurveTo(controlX, controlY, end[0], end[1]);
      return finish(turtle, command, end[0], end[1], controlX, controlY);
    }
    case "A": {
      const radiusX = scanner.number();
      const radiusY = scanner.number();
      const rotation = scanner.number();
      const largeArc = scanner.flag();
      const sweepFlag = scanner.flag();
      const end = scanner.point(originX, originY);
      if (
        radiusX === undefined ||
        radiusY === undefined ||
        rotation === undefined ||
        largeArc === undefined ||
        sweepFlag === undefined ||
        !end
      ) {
        return false;
      }
      appendArc(path, turtle, radiusX, radiusY, rotation, largeArc, sweepFlag, end[0], end[1]);
      return finish(turtle, command, end[0], end[1], end[0], end[1]);
    }
    case "Z": {
      path.close();
      return finish(turtle, command, turtle.startX, turtle.startY, turtle.startX, turtle.startY);
    }
    default:
      return false;
  }
}

function finish(
  turtle: Turtle,
  command: string,
  x: number,
  y: number,
  controlX: number,
  controlY: number,
): boolean {
  turtle.x = x;
  turtle.y = y;
  turtle.controlX = controlX;
  turtle.controlY = controlY;
  turtle.previous = command.toUpperCase();
  return true;
}

/**
 * `S` and `T` mirror the previous curve's trailing control point through the
 * current point — but only when the previous command was the matching curve
 * kind. Otherwise the control point IS the current point, so the curve leaves
 * along the straight continuation.
 */
function reflected(turtle: Turtle, kinds: string): [number, number] {
  if (!kinds.includes(turtle.previous)) return [turtle.x, turtle.y];
  return [2 * turtle.x - turtle.controlX, 2 * turtle.y - turtle.controlY];
}

/**
 * SVG's endpoint-parameterized elliptical arc, converted to the centre
 * parameterization Valo paths take (SVG 2 §B.2.4). Out-of-range radii scale
 * up until the endpoints fit rather than failing, which is the spec's own
 * correction step.
 */
function appendArc(
  path: PathSink,
  turtle: Turtle,
  radiusX: number,
  radiusY: number,
  degrees: number,
  largeArc: boolean,
  sweepFlag: boolean,
  endX: number,
  endY: number,
): void {
  if (turtle.x === endX && turtle.y === endY) return;
  let rx = Math.abs(radiusX);
  let ry = Math.abs(radiusY);
  if (rx === 0 || ry === 0) {
    path.lineTo(endX, endY);
    return;
  }
  const rotation = (degrees * Math.PI) / 180;
  const cosine = Math.cos(rotation);
  const sine = Math.sin(rotation);
  const halfDeltaX = (turtle.x - endX) / 2;
  const halfDeltaY = (turtle.y - endY) / 2;
  const localX = cosine * halfDeltaX + sine * halfDeltaY;
  const localY = -sine * halfDeltaX + cosine * halfDeltaY;

  const oversize = (localX * localX) / (rx * rx) + (localY * localY) / (ry * ry);
  if (oversize > 1) {
    const growth = Math.sqrt(oversize);
    rx *= growth;
    ry *= growth;
  }

  const numerator =
    rx * rx * ry * ry - rx * rx * localY * localY - ry * ry * localX * localX;
  const denominator = rx * rx * localY * localY + ry * ry * localX * localX;
  const scale =
    (largeArc === sweepFlag ? -1 : 1) * Math.sqrt(Math.max(0, numerator / denominator));
  const localCenterX = (scale * rx * localY) / ry;
  const localCenterY = (-scale * ry * localX) / rx;
  const centerX = cosine * localCenterX - sine * localCenterY + (turtle.x + endX) / 2;
  const centerY = sine * localCenterX + cosine * localCenterY + (turtle.y + endY) / 2;

  const startX = (localX - localCenterX) / rx;
  const startY = (localY - localCenterY) / ry;
  const finishX = (-localX - localCenterX) / rx;
  const finishY = (-localY - localCenterY) / ry;
  const startAngle = Math.atan2(startY, startX);
  let sweepAngle = Math.atan2(finishY, finishX) - startAngle;
  const fullTurn = Math.PI * 2;
  if (!sweepFlag && sweepAngle > 0) sweepAngle -= fullTurn;
  if (sweepFlag && sweepAngle < 0) sweepAngle += fullTurn;

  path.ellipse(centerX, centerY, rx, ry, rotation, startAngle, sweepAngle);
}

const COMMANDS = "MmLlHhVvCcSsQqTtAaZz";

/**
 * The path-data tokenizer. Its awkwardness is the grammar's: separators are
 * optional wherever a token can still be told apart, so `10-5` is two numbers
 * and `a1 1 0 011 1` packs both arc flags and an x coordinate into `011`.
 */
class Scanner {
  readonly #text: string;
  #at = 0;

  constructor(text: string) {
    this.#text = text;
  }

  atEnd(): boolean {
    this.#skipSeparators();
    return this.#at >= this.#text.length;
  }

  command(): string | undefined {
    this.#skipSeparators();
    const character = this.#text[this.#at];
    if (character === undefined || !COMMANDS.includes(character)) return undefined;
    this.#at += 1;
    return character;
  }

  point(originX: number, originY: number): [number, number] | undefined {
    const x = this.number();
    const y = this.number();
    if (x === undefined || y === undefined) return undefined;
    return [originX + x, originY + y];
  }

  number(): number | undefined {
    this.#skipSeparators();
    const start = this.#at;
    if (this.#peekIsOneOf("+-")) this.#at += 1;
    const beforeDigits = this.#at;
    this.#skipDigits();
    if (this.#text[this.#at] === ".") {
      this.#at += 1;
      this.#skipDigits();
    }
    if (this.#at === beforeDigits || this.#text[beforeDigits] === undefined) {
      this.#at = start;
      return undefined;
    }
    if (this.#peekIsOneOf("eE")) {
      const beforeExponent = this.#at;
      this.#at += 1;
      if (this.#peekIsOneOf("+-")) this.#at += 1;
      const beforeExponentDigits = this.#at;
      this.#skipDigits();
      if (this.#at === beforeExponentDigits) this.#at = beforeExponent;
    }
    const value = Number(this.#text.slice(start, this.#at));
    if (!Number.isFinite(value)) {
      this.#at = start;
      return undefined;
    }
    return value;
  }

  /** Arc flags are exactly one character, with no separator required after. */
  flag(): boolean | undefined {
    this.#skipSeparators();
    const character = this.#text[this.#at];
    if (character !== "0" && character !== "1") return undefined;
    this.#at += 1;
    return character === "1";
  }

  #skipSeparators(): void {
    while (this.#at < this.#text.length && /[\s,]/.test(this.#text[this.#at]!)) this.#at += 1;
  }

  #skipDigits(): void {
    while (this.#at < this.#text.length && this.#text[this.#at]! >= "0" && this.#text[this.#at]! <= "9") {
      this.#at += 1;
    }
  }

  #peekIsOneOf(characters: string): boolean {
    const character = this.#text[this.#at];
    return character !== undefined && characters.includes(character);
  }
}
