/**
 * Names for the integers valo's wasm boundary takes.
 *
 * `Paint.setBlendMode(24)` and `paint.setMaskBlur(sigma, 3)` are the real
 * calls — the boundary passes plain `u32` and the TypeScript wrapper keeps its
 * own private tables. Demo code that says `BlendMode.Multiply` reads as the
 * thing it does, and one table here is easier to keep true than the same magic
 * number written across a dozen demos.
 *
 * Every value is the index the Rust side uses in `crates/valo-web/src/types.rs`.
 * If `valo-web` ever exports these, delete this file and import from there.
 */

/** Order of `types::blend_mode`. */
export const BlendMode = {
  Clear: 0,
  Src: 1,
  Dst: 2,
  SrcOver: 3,
  DstOver: 4,
  SrcIn: 5,
  DstIn: 6,
  SrcOut: 7,
  DstOut: 8,
  SrcAtop: 9,
  DstAtop: 10,
  Xor: 11,
  Plus: 12,
  Modulate: 13,
  Screen: 14,
  Overlay: 15,
  Darken: 16,
  Lighten: 17,
  ColorDodge: 18,
  ColorBurn: 19,
  HardLight: 20,
  SoftLight: 21,
  Difference: 22,
  Exclusion: 23,
  Multiply: 24,
  Hue: 25,
  Saturation: 26,
  Color: 27,
  Luminosity: 28,
} as const;

/**
 * Order of `types::blur_style`. `Solid`, `Inner` and `Outer` have no Canvas2D
 * spelling at all — they are the mask blur applied to the shape's coverage
 * rather than to its pixels.
 */
export const BlurStyle = {
  Normal: 0,
  Solid: 1,
  Inner: 2,
  Outer: 3,
} as const;

/** Order of `types::cap`. */
export const Cap = { Butt: 0, Round: 1, Square: 2 } as const;

/** Order of `types::join`. */
export const Join = { Miter: 0, Round: 1, Bevel: 2 } as const;

/** Order of `types::fill_rule`. */
export const FillRule = { NonZero: 0, EvenOdd: 1 } as const;

/** Order of `types::clip_op`. `Difference` is a clip Canvas2D cannot express. */
export const ClipOp = { Intersect: 0, Difference: 1 } as const;

/** Order of `types::spread_mode`: what a gradient does outside its own range. */
export const SpreadMode = { Pad: 0, Repeat: 1, Reflect: 2 } as const;

/** Order of `types::text_align`. */
export const TextAlign = { Left: 0, Center: 1, Right: 2, Justify: 3 } as const;

export type BlendModeValue = (typeof BlendMode)[keyof typeof BlendMode];
export type BlurStyleValue = (typeof BlurStyle)[keyof typeof BlurStyle];
