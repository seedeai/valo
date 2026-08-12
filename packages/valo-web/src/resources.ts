import { Shader, type Image } from "./raw.js";
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

export class ValoCanvasPattern {
  readonly #image: Image;
  #transform: Affine = identity;

  constructor(image: Image, repetition: string | null) {
    if (repetition !== null && repetition !== "repeat") {
      throw new DOMException(
        "Valo currently supports repeating patterns; use drawImage for non-repeating images",
        "NotSupportedError",
      );
    }
    this.#image = image;
  }

  setTransform(transform: DOMMatrix2DInit): void {
    const matrix = DOMMatrix.fromMatrix(transform);
    this.#transform = [matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f];
  }

  toRaw(imageSmoothingEnabled: boolean): Shader {
    const shader = Shader.imagePattern(this.#image, imageSmoothingEnabled ? 0 : 1, 1, 1);
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
