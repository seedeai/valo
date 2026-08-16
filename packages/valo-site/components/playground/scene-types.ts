/**
 * `@valo/web`'s emitted declarations, staged into `public/valo/types/` by
 * `scripts/copy-valo-assets.mjs` and laid out in the editor's virtual file
 * system exactly as they sit in the package — `dist/` beside `wasm/` — so the
 * relative import inside `raw.d.ts` resolves without rewriting anything.
 *
 * There are no ambient globals to declare alongside them: a scene imports the
 * names it uses, the same as any other module.
 */
export const DIST_TYPES = [
  'canvas',
  'color',
  'css',
  'enums',
  'images',
  'index',
  'matrix',
  'path2d',
  'raw',
  'resources',
] as const;
