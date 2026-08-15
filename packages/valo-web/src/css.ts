import { parseColor, type Rgba } from "./color.js";

/**
 * The CSS value grammars Canvas2D embeds in strings: the `font` shorthand and
 * the `filter` function list. Pure parsing, no GPU and no recorder — which is
 * also what makes it directly testable.
 */

export interface ParsedFont {
  italic: boolean;
  smallCaps: boolean;
  weight: number;
  /** CSS `font-width` percentage; 100 is normal. */
  stretch: number;
  size: number;
  /** Line-height MULTIPLIER, or undefined for the font's own metrics. */
  lineHeight: number | undefined;
  families: string[];
}

/** The sizes relative units in the `font` shorthand resolve against. */
export interface FontSizeReference {
  element: number;
  root: number;
}

export const DEFAULT_FONT_SIZE = 16;

export const fontStretchPercentages: Record<CanvasFontStretch, number> = {
  "ultra-condensed": 50,
  "extra-condensed": 62.5,
  condensed: 75,
  "semi-condensed": 87.5,
  normal: 100,
  "semi-expanded": 112.5,
  expanded: 125,
  "extra-expanded": 150,
  "ultra-expanded": 200,
};

/** Matches `variant_caps_of` on the wasm side. */
export const variantCapsIds: Record<CanvasFontVariantCaps, number> = {
  normal: 0,
  "small-caps": 1,
  "all-small-caps": 2,
  "petite-caps": 3,
  "all-petite-caps": 4,
  unicase: 5,
  "titling-caps": 6,
};

/** Matches `text_direction` on the wasm side; `inherit` means "infer". */
export function directionId(direction: CanvasDirection): number {
  return direction === "ltr" ? 1 : direction === "rtl" ? 2 : 0;
}

const CSS_WEIGHTS: Record<string, number> = {
  normal: 400,
  bold: 700,
  // No parent font to step from, so the relative keywords resolve against
  // `normal` — which is what they compute to on an unstyled canvas.
  lighter: 100,
  bolder: 700,
};

/**
 * The `font` shorthand: `[style] [variant] [weight] [width] size[/line-height]
 * family-list`. The size token is what splits the descriptor from the family
 * list, and it is unambiguous because a size always carries a unit while a
 * numeric weight never does.
 */
export function parseFont(
  value: string,
  reference: FontSizeReference = { element: DEFAULT_FONT_SIZE, root: DEFAULT_FONT_SIZE },
): ParsedFont {
  const source = value.trim();
  const size = /(^|\s)([+]?(?:\d+(?:\.\d*)?|\.\d+)(?:px|pt|pc|in|cm|mm|q|em|rem|ex|ch|%))(?:\s*\/\s*([^\s]+))?\s+(\S.*)$/i
    .exec(source);
  if (!size) {
    throw new TypeError(
      `Unsupported font '${value}'. Use '[style] [variant] [weight] [width] <size> <family-list>'.`,
    );
  }
  const descriptor = source.slice(0, size.index).trim();
  const pixels = resolveLength(size[2]!, reference);
  if (pixels === undefined) throw new TypeError(`Unsupported font size in '${value}'`);
  return {
    ...parseFontDescriptor(descriptor, value),
    size: pixels,
    lineHeight: parseLineHeight(size[3], pixels, reference),
    families: size[4]!.split(",").map((family) => family.trim().replace(/^['"]|['"]$/g, "")),
  };
}

/** The keywords ahead of the size, in any order — browsers accept them that
 *  way even though the grammar fixes a sequence. */
function parseFontDescriptor(
  descriptor: string,
  original: string,
): Pick<ParsedFont, "italic" | "smallCaps" | "weight" | "stretch"> {
  const parsed = { italic: false, smallCaps: false, weight: 400, stretch: 100 };
  if (descriptor.length === 0) return parsed;
  for (const token of descriptor.split(/\s+/)) {
    const keyword = token.toLowerCase();
    if (keyword === "normal") continue;
    if (keyword === "italic" || keyword.startsWith("oblique")) {
      parsed.italic = true;
    } else if (keyword === "small-caps") {
      parsed.smallCaps = true;
    } else if (keyword in CSS_WEIGHTS) {
      parsed.weight = CSS_WEIGHTS[keyword]!;
    } else if (/^\d+$/.test(keyword)) {
      parsed.weight = Number.parseInt(keyword, 10);
    } else if (keyword in fontStretchPercentages) {
      parsed.stretch = fontStretchPercentages[keyword as CanvasFontStretch]!;
    } else if (/^[0-9.]+%$/.test(keyword)) {
      parsed.stretch = Number.parseFloat(keyword);
    } else if (keyword !== "oblique") {
      throw new TypeError(`Unsupported font descriptor '${token}' in '${original}'`);
    }
  }
  return parsed;
}

/** `<length>` → px. Font-relative units need the element's own size, which
 *  is why the reference travels this far down. */
function resolveLength(token: string, reference: FontSizeReference): number | undefined {
  const match = /^([+]?(?:\d+(?:\.\d*)?|\.\d+))(px|pt|pc|in|cm|mm|q|em|rem|ex|ch|%)$/i.exec(token);
  if (!match) return undefined;
  const amount = Number(match[1]);
  switch (match[2]!.toLowerCase()) {
    case "px": return amount;
    case "pt": return (amount * 96) / 72;
    case "pc": return amount * 16;
    case "in": return amount * 96;
    case "cm": return (amount * 96) / 2.54;
    case "mm": return (amount * 96) / 25.4;
    case "q": return (amount * 96) / 101.6;
    case "em": return amount * reference.element;
    case "rem": return amount * reference.root;
    // No font is resolved yet, so `ex` and `ch` take the CSS fallback ratios
    // (0.5em and 0.25em) rather than the actual x-height and zero advance.
    case "ex": return amount * reference.element * 0.5;
    case "ch": return amount * reference.element * 0.25;
    default: return (amount * reference.element) / 100;
  }
}

/** `font: 12px/1.5 sans` — a multiplier, a length, or a percentage. */
function parseLineHeight(
  token: string | undefined,
  size: number,
  reference: FontSizeReference,
): number | undefined {
  if (token === undefined || token.toLowerCase() === "normal") return undefined;
  if (/^[+]?(?:\d+(?:\.\d*)?|\.\d+)$/.test(token)) return Number(token);
  const length = resolveLength(token, { element: size, root: reference.root });
  if (length === undefined || size <= 0) return undefined;
  return length / size;
}

export function pixelsOrDefault(computed: string): number {
  const pixels = Number.parseFloat(computed);
  return Number.isFinite(pixels) && pixels > 0 ? pixels : DEFAULT_FONT_SIZE;
}

/** A `letterSpacing` / `wordSpacing` value: px only, as the spec says. */
export function parsePixels(value: string): number {
  const match = /^(-?[0-9.]+)px$/.exec(value.trim());
  if (!match) throw new TypeError(`Expected a pixel length, received '${value}'`);
  return Number.parseFloat(match[1]!);
}

export type FilterStage =
  | { type: "blur"; sigma: number }
  | { type: "color"; matrix: number[] }
  | { type: "drop-shadow"; offsetX: number; offsetY: number; sigma: number; color: Rgba };

export function parseFilter(value: string): FilterStage[] | undefined {
  const source = value.trim();
  if (source.toLowerCase() === "none") return [];
  if (source.length === 0) return undefined;
  const stages: FilterStage[] = [];
  // A colour argument can itself be a function call, so the argument list
  // tracks one level of nesting rather than banning parentheses outright.
  const call = /([a-z-]+)\(((?:[^()]|\([^()]*\))*)\)/iy;
  let offset = 0;
  while (offset < source.length) {
    while (/\s/.test(source[offset] ?? "")) offset += 1;
    call.lastIndex = offset;
    const match = call.exec(source);
    if (!match) return undefined;
    const stage = parseFilterFunction(match[1]!.toLowerCase(), match[2]!.trim().toLowerCase());
    if (stage === undefined) return undefined;
    if (stage) stages.push(stage);
    offset = call.lastIndex;
  }
  return stages;
}

function parseFilterFunction(name: string, argument: string): FilterStage | null | undefined {
  if (name === "drop-shadow") return parseDropShadow(argument);
  if (name === "blur") {
    const match = /^([+]?(?:\d+(?:\.\d*)?|\.\d+))(px)?$/.exec(argument || "0");
    if (match?.[2] === undefined && Number(match?.[1]) !== 0) return undefined;
    const radius = match ? Number(match[1]) : Number.NaN;
    if (!Number.isFinite(radius)) return undefined;
    return radius === 0 ? null : { type: "blur", sigma: radius };
  }
  if (name === "hue-rotate") {
    const radians = parseAngle(argument || "0deg");
    if (radians === undefined) return undefined;
    return Math.abs(radians % (Math.PI * 2)) < 1e-12
      ? null
      : { type: "color", matrix: hueRotate(radians) };
  }
  const amount = parseFilterAmount(argument || "100%");
  if (amount === undefined) return undefined;
  switch (name) {
    case "brightness": return amount === 1 ? null : { type: "color", matrix: brightness(amount) };
    case "contrast": return amount === 1 ? null : { type: "color", matrix: contrast(amount) };
    case "grayscale": return amount === 0 ? null : { type: "color", matrix: grayscale(Math.min(amount, 1)) };
    case "invert": return amount === 0 ? null : { type: "color", matrix: invert(Math.min(amount, 1)) };
    case "opacity": return amount === 1 ? null : { type: "color", matrix: opacity(Math.min(amount, 1)) };
    case "saturate": return amount === 1 ? null : { type: "color", matrix: saturate(amount) };
    case "sepia": return amount === 0 ? null : { type: "color", matrix: sepia(Math.min(amount, 1)) };
    default: return undefined;
  }
}

/**
 * `drop-shadow(<offset-x> <offset-y> [<blur-radius>] [<color>])`. CSS calls the
 * third value a blur RADIUS and defines it as twice the gaussian sigma, which
 * is the same convention `shadowBlur` already follows here.
 *
 * The colour may appear first or last, per the CSS filter grammar.
 */
function parseDropShadow(argument: string): FilterStage | null | undefined {
  const parts = splitShadowArguments(argument);
  if (!parts) return undefined;
  const lengths: number[] = [];
  let color: Rgba | undefined;
  for (const part of parts) {
    const length = parseFilterLength(part);
    if (length !== undefined) {
      lengths.push(length);
      continue;
    }
    if (color !== undefined) return undefined;
    try {
      color = parseColor(part);
    } catch {
      return undefined;
    }
  }
  if (lengths.length < 2 || lengths.length > 3) return undefined;
  const radius = lengths[2] ?? 0;
  if (radius < 0) return undefined;
  return {
    type: "drop-shadow",
    offsetX: lengths[0]!,
    offsetY: lengths[1]!,
    sigma: radius / 2,
    // CSS says an omitted colour takes the `color` property, which a canvas
    // has no equivalent of; browsers use black there too.
    color: color ?? [0, 0, 0, 1],
  };
}

/** Split on whitespace, but keep a `rgb(...)` colour in one piece. */
function splitShadowArguments(argument: string): string[] | undefined {
  const parts: string[] = [];
  let depth = 0;
  let current = "";
  for (const character of argument.trim()) {
    if (character === "(") depth += 1;
    if (character === ")") depth -= 1;
    if (depth < 0) return undefined;
    if (depth === 0 && /\s/.test(character)) {
      if (current) parts.push(current);
      current = "";
      continue;
    }
    current += character;
  }
  if (depth !== 0) return undefined;
  if (current) parts.push(current);
  return parts.length > 0 ? parts : undefined;
}

/** A signed CSS length in the filter grammar; only px is meaningful here. */
function parseFilterLength(value: string): number | undefined {
  const match = /^([+-]?(?:\d+(?:\.\d*)?|\.\d+))(px)?$/.exec(value);
  if (!match) return undefined;
  const amount = Number(match[1]);
  if (!Number.isFinite(amount)) return undefined;
  // A bare number is only legal when it is zero, like every CSS length.
  return match[2] === undefined && amount !== 0 ? undefined : amount;
}

function parseFilterAmount(value: string): number | undefined {
  const match = /^([+]?(?:\d+(?:\.\d*)?|\.\d+))(%)?$/.exec(value);
  if (!match) return undefined;
  const amount = Number(match[1]) / (match[2] ? 100 : 1);
  return Number.isFinite(amount) ? amount : undefined;
}

function parseAngle(value: string): number | undefined {
  const match = /^([+-]?(?:\d+(?:\.\d*)?|\.\d+))(deg|grad|rad|turn)$/.exec(value);
  if (!match) return undefined;
  const amount = Number(match[1]);
  const radians = match[2] === "deg"
    ? amount * Math.PI / 180
    : match[2] === "grad"
      ? amount * Math.PI / 200
      : match[2] === "turn"
        ? amount * Math.PI * 2
        : amount;
  return Number.isFinite(radians) ? radians : undefined;
}

// ── the colour matrices the CSS filter functions lower to ──────────────

function diagonal(red: number, green: number, blue: number, alpha = 1): number[] {
  return [
    red, 0, 0, 0, 0,
    0, green, 0, 0, 0,
    0, 0, blue, 0, 0,
    0, 0, 0, alpha, 0,
  ];
}

function brightness(amount: number): number[] {
  return diagonal(amount, amount, amount);
}

function contrast(amount: number): number[] {
  const offset = 0.5 * (1 - amount);
  return [
    amount, 0, 0, 0, offset,
    0, amount, 0, 0, offset,
    0, 0, amount, 0, offset,
    0, 0, 0, 1, 0,
  ];
}

function opacity(amount: number): number[] {
  return diagonal(1, 1, 1, amount);
}

function invert(amount: number): number[] {
  const scale = 1 - 2 * amount;
  return [
    scale, 0, 0, 0, amount,
    0, scale, 0, 0, amount,
    0, 0, scale, 0, amount,
    0, 0, 0, 1, 0,
  ];
}

function saturate(amount: number): number[] {
  return [
    0.213 + 0.787 * amount, 0.715 - 0.715 * amount, 0.072 - 0.072 * amount, 0, 0,
    0.213 - 0.213 * amount, 0.715 + 0.285 * amount, 0.072 - 0.072 * amount, 0, 0,
    0.213 - 0.213 * amount, 0.715 - 0.715 * amount, 0.072 + 0.928 * amount, 0, 0,
    0, 0, 0, 1, 0,
  ];
}

function grayscale(amount: number): number[] {
  return saturate(1 - amount);
}

function sepia(amount: number): number[] {
  return [
    1 - 0.607 * amount, 0.769 * amount, 0.189 * amount, 0, 0,
    0.349 * amount, 1 - 0.314 * amount, 0.168 * amount, 0, 0,
    0.272 * amount, 0.534 * amount, 1 - 0.869 * amount, 0, 0,
    0, 0, 0, 1, 0,
  ];
}

function hueRotate(angle: number): number[] {
  const cosine = Math.cos(angle);
  const sine = Math.sin(angle);
  return [
    0.213 + 0.787 * cosine - 0.213 * sine,
    0.715 - 0.715 * cosine - 0.715 * sine,
    0.072 - 0.072 * cosine + 0.928 * sine, 0, 0,
    0.213 - 0.213 * cosine + 0.143 * sine,
    0.715 + 0.285 * cosine + 0.140 * sine,
    0.072 - 0.072 * cosine - 0.283 * sine, 0, 0,
    0.213 - 0.213 * cosine - 0.787 * sine,
    0.715 - 0.715 * cosine + 0.715 * sine,
    0.072 + 0.928 * cosine + 0.072 * sine, 0, 0,
    0, 0, 0, 1, 0,
  ];
}
