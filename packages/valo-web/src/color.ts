export type Rgba = readonly [red: number, green: number, blue: number, alpha: number];

const namedColors: Record<string, Rgba> = {
  black: [0, 0, 0, 1],
  white: [1, 1, 1, 1],
  red: [1, 0, 0, 1],
  green: [0, 0.5019608, 0, 1],
  blue: [0, 0, 1, 1],
  transparent: [0, 0, 0, 0],
};

/** CSS Color 4's common sRGB forms. Unsupported forms fail loudly. */
export function parseColor(value: string): Rgba {
  const color = value.trim().toLowerCase();
  const named = namedColors[color];
  if (named) return named;
  if (color.startsWith("#")) return parseHex(color.slice(1));
  const match = /^(rgba?)\((.*)\)$/.exec(color);
  if (match) return parseRgb(match[2] ?? "");
  throw new TypeError(
    `Unsupported color '${value}'. Valo currently accepts hex, rgb(), rgba(), and basic named colors.`,
  );
}

function parseHex(value: string): Rgba {
  const expanded =
    value.length === 3 || value.length === 4
      ? [...value].map((digit) => digit + digit).join("")
      : value;
  if (expanded.length !== 6 && expanded.length !== 8) {
    throw new TypeError(`Invalid hex color '#${value}'`);
  }
  const channels = [0, 2, 4, 6].map((offset) =>
    offset < expanded.length ? Number.parseInt(expanded.slice(offset, offset + 2), 16) / 255 : 1,
  );
  if (channels.some(Number.isNaN)) throw new TypeError(`Invalid hex color '#${value}'`);
  return channels as unknown as Rgba;
}

function parseRgb(body: string): Rgba {
  const parts = body.replace("/", ",").split(/[ ,]+/).filter(Boolean);
  if (parts.length !== 3 && parts.length !== 4) throw new TypeError(`Invalid rgb() color`);
  const channel = (part: string): number =>
    part.endsWith("%") ? clamp(Number.parseFloat(part) / 100) : clamp(Number.parseFloat(part) / 255);
  const alpha = (part: string | undefined): number =>
    part === undefined
      ? 1
      : part.endsWith("%")
        ? clamp(Number.parseFloat(part) / 100)
        : clamp(Number.parseFloat(part));
  const result: Rgba = [channel(parts[0]!), channel(parts[1]!), channel(parts[2]!), alpha(parts[3])];
  if (result.some(Number.isNaN)) throw new TypeError(`Invalid rgb() color`);
  return result;
}

function clamp(value: number): number {
  return Math.min(1, Math.max(0, value));
}
