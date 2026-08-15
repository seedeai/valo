import { Shader, type Image } from "./raw.js";
import type { ValoImageSource } from "./images.js";
import { parseColor, type Rgba } from "./color.js";
import { identity, type Affine } from "./matrix.js";

interface Stop {
  offset: number;
  color: Rgba;
}

type GradientKind =
  | { type: "linear"; values: readonly [number, number, number, number] }
  | { type: "radial"; values: readonly [number, number, number, number, number, number] }
  | { type: "sweep"; values: readonly [number, number, number] };

export class ValoCanvasGradient {
  readonly #kind: GradientKind;
  readonly #stops: Stop[] = [];
  #transform: Affine = identity;

  constructor(kind: GradientKind) {
    this.#kind = kind;
  }

  addColorStop(offset: number, color: string): void {
    if (!Number.isFinite(offset) || offset < 0 || offset > 1) {
      throw new DOMException("Gradient offsets must be between 0 and 1", "IndexSizeError");
    }
    this.#stops.push({ offset, color: parseColor(color) });
  }

  setTransform(transform: DOMMatrix2DInit): void {
    const matrix = DOMMatrix.fromMatrix(transform);
    this.#transform = [matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f];
  }

  toRaw(): Shader {
    const stops = normalizedStops(this.#stops);
    const offsets = new Float32Array(stops.map((stop) => stop.offset));
    const colors = new Float32Array(stops.flatMap((stop) => [...stop.color]));
    let shader: Shader;
    switch (this.#kind.type) {
      case "linear":
        shader = Shader.linearGradient(...this.#kind.values, offsets, colors, 0);
        break;
      case "radial":
        shader = Shader.radialGradient(...this.#kind.values, offsets, colors, 0);
        break;
      case "sweep":
        shader = Shader.sweepGradient(...this.#kind.values, offsets, colors);
        break;
    }
    shader.setTransform(new Float32Array(this.#transform));
    return shader;
  }
}

/** TileMode ids across the wasm boundary. */
const REPEAT = 1;
const DECAL = 3;

/**
 * The four Canvas repetition values as per-axis tile modes. The non-repeating
 * axis is DECAL — nothing outside the image — rather than clamp-to-edge,
 * which would smear the border pixels across the rest of the shape.
 */
const REPETITIONS: Record<string, readonly [number, number]> = {
  repeat: [REPEAT, REPEAT],
  "repeat-x": [REPEAT, DECAL],
  "repeat-y": [DECAL, REPEAT],
  "no-repeat": [DECAL, DECAL],
};

export class ValoCanvasPattern {
  readonly #source: ValoImageSource;
  readonly #tileX: number;
  readonly #tileY: number;
  #transform: Affine = identity;

  constructor(source: ValoImageSource, repetition: string | null) {
    // The spec treats null and the empty string as "repeat".
    const tiling = REPETITIONS[repetition ? repetition : "repeat"];
    if (!tiling) {
      throw new DOMException(
        `'${repetition}' is not a valid pattern repetition`,
        "SyntaxError",
      );
    }
    this.#source = source;
    [this.#tileX, this.#tileY] = tiling;
  }

  /** The source this pattern samples, re-read per frame if it is live. */
  get source(): ValoImageSource {
    return this.#source;
  }

  setTransform(transform: DOMMatrix2DInit): void {
    const matrix = DOMMatrix.fromMatrix(transform);
    this.#transform = [matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f];
  }

  toRaw(image: Image, imageSmoothingEnabled: boolean, mipmap: number): Shader {
    const shader = Shader.imagePattern(
      image,
      imageSmoothingEnabled ? 0 : 1,
      mipmap,
      this.#tileX,
      this.#tileY,
    );
    shader.setTransform(new Float32Array(this.#transform));
    return shader;
  }
}

function normalizedStops(stops: readonly Stop[]): Stop[] {
  if (stops.length === 0) {
    return [
      { offset: 0, color: [0, 0, 0, 0] },
      { offset: 1, color: [0, 0, 0, 0] },
    ];
  }
  const sorted = [...stops].sort((left, right) => left.offset - right.offset);
  if (sorted.length === 1) {
    return [
      { offset: 0, color: sorted[0]!.color },
      { offset: 1, color: sorted[0]!.color },
    ];
  }
  return sorted;
}
