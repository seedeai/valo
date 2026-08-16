/**
 * Names for the integers the wasm boundary takes.
 *
 * `wasm-bindgen` passes plain `u32` across every one of these, so the raw API
 * would otherwise be written in magic numbers: `paint.setMaskBlur(sigma, 3)`
 * rather than `paint.setMaskBlur(sigma, BlurStyle.Outer)`.
 *
 * Every table below is the index order its counterpart in
 * `crates/valo-web/src/types.rs` decodes, and adding a variant anywhere but the
 * end changes both. They are objects rather than TypeScript `enum`s so that the
 * emitted package has no runtime construct a consumer did not ask for.
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
 * spelling: they are the blur applied to the shape's coverage rather than to
 * its pixels, and `shadowBlur` only ever gives you `Normal`.
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

/** Order of `types::sampling`'s filter argument. */
export const Filter = { Linear: 0, Nearest: 1 } as const;

/** Order of `types::mipmap_mode`. */
export const MipmapMode = { None: 0, Nearest: 1, Linear: 2 } as const;

/** Order of `types::tile_mode`: what a shader does outside its own extent. */
export const TileMode = { Clamp: 0, Repeat: 1, Mirror: 2, Decal: 3 } as const;

/** Order of `text::text_align`. */
export const TextAlign = { Left: 0, Center: 1, Right: 2, Justify: 3 } as const;

/** `text::text_direction`. `Infer` is Canvas2D's `"inherit"`: read it from the content. */
export const TextDirection = { Infer: 0, Ltr: 1, Rtl: 2 } as const;

/** Order of `text::variant_caps_of`. */
export const VariantCaps = {
  Normal: 0,
  SmallCaps: 1,
  AllSmallCaps: 2,
  PetiteCaps: 3,
  AllPetiteCaps: 4,
  Unicase: 5,
  TitlingCaps: 6,
} as const;

export type BlendModeValue = (typeof BlendMode)[keyof typeof BlendMode];
export type BlurStyleValue = (typeof BlurStyle)[keyof typeof BlurStyle];
export type CapValue = (typeof Cap)[keyof typeof Cap];
export type JoinValue = (typeof Join)[keyof typeof Join];
export type FillRuleValue = (typeof FillRule)[keyof typeof FillRule];
export type ClipOpValue = (typeof ClipOp)[keyof typeof ClipOp];
export type SpreadModeValue = (typeof SpreadMode)[keyof typeof SpreadMode];
export type TextAlignValue = (typeof TextAlign)[keyof typeof TextAlign];
export type TextDirectionValue = (typeof TextDirection)[keyof typeof TextDirection];
