# Valo conformance

Private browser tests for the Canvas-shaped API. The same serializable scene is
replayed into browser Canvas2D and Valo, then the live results are compared with
Vitest's BlazeDiff comparator. A failure saves both renders, the diff, and the
scene under `artifacts/`.

```sh
npm run build:web
npm run test:conformance       # curated + degenerate + API scenes, 6 × 40 fuzz cases, 2 query properties
npm run fuzz:canvas            # VALO_FUZZ_RUNS / SEED / TIME_LIMIT customize it
npm run benchmark:canvas       # JavaScript record + submit cost, no GPU wait
```

## What each suite covers

| Suite | Shape |
|---|---|
| `curated.browser.test.ts` | Hand-written scenes for behaviour worth a name. |
| `fuzz.browser.test.ts` | Generated scenes compared as pixels: the shared command pool, solid strokes, dashed strokes, save/restore nesting, fill text, stroke text. |
| `query.browser.test.ts` | Generated `measureText` and `isPointInPath` calls compared as numbers. No screenshot, so a run costs a shaping pass. |
| `degenerate.browser.test.ts` | Zeroes, negatives, non-finite arguments, blur sigmas past Valo's downsample switch, coordinates far off-canvas. One named test per case. |
| `api.browser.test.ts` | Invalid-argument behaviour and explicit gaps. |

The generator works in **continuous coordinates**: positions, extents, radii,
dash intervals, spacing and filter amounts all carry a sub-pixel part, because a
fractional edge is where two rasterizers most plausibly disagree. Whole numbers
keep their own weight, and are where fast-check shrinks to — a counterexample
that survives shrinking does not depend on sub-pixel coverage.

## How the comparison decides

Two independent measures, both in `thresholds.ts` with the numbers they were
calibrated against:

- **Bad-pixel ratio** — BlazeDiff's perceptual comparison, ignoring pixels it
  classifies as antialiasing.
- **Ink offset** — the distance between the two renders' ink centroids, each
  weighted by how far the pixel stands from the background. This replaced a
  thresholded bounding box, which was decided entirely by its outermost pixels
  and so moved several pixels whenever a shadow's tail or a `multiply` blend
  landed a few levels off the background. A centroid averages over the whole
  shape: steady on faint content, and sensitive to displacements below a pixel
  on solid content, which a box can never resolve.

Where there is too little ink to place, the offset is not reported and the
pixel comparison decides alone.

## Keeping a known divergence from masking everything else

Several generators stop just short of a value Valo and Canvas2D disagree on —
blur sigma at 4√2, zero-length dash intervals, letter spacing that drives the
advance negative, text that puts no ink down. Each of those would fail on
roughly the first run and stop the property before it explored anything else.
Every one is covered by name instead, in `degenerate-scenes.ts` or as its own
test in `query.browser.test.ts`, with a comment at the generator saying where.
When the divergence closes, widen the generator and delete the comment.

## Why these scripts build `valo-web` first

This suite imports `valo-web` by package name, so it resolves through that
package's `exports` to the emitted `dist/` — the same entry point a real
consumer gets. That is deliberate: a polyfill is only proven by testing what
ships, including its `exports` map and its emitted types, rather than the
source those were generated from.

The consequence is that `dist/` is an input to every run, and it is a build
output that nothing tracks. `check:web` only typechecks (`tsc --noEmit`), so it
never regenerates it. Left to chance a run validates whatever `dist/` a machine
last happened to build: it can fail on correct source, and — worse — pass on
stale source. A `dist/` older than its sibling `wasm/` has also crashed the
browser tests outright at the wasm ABI boundary, which reads as dozens of
unrelated failures rather than as a stale build.

So `check:conformance`, `test:conformance`, `fuzz:canvas` and `benchmark:canvas`
all run `build:web` first. Both `wasm-pack` and `tsc` are incremental, so this
costs seconds when the build is already current. It looks redundant next to a CI
job that also builds; it is not, and removing it reintroduces a class of failure
that wastes far more time than it saves.

## Working on the harness

Set `VALO_CONFORMANCE_PROFILE=1` for screenshot, decode, comparison and artifact
timings. Set `VALO_CONFORMANCE_METRICS=1` to print the bad-pixel ratio and ink
offset for every comparison — the thresholds are only defensible against the
spread of values passing scenes actually produce, so re-measure that spread
before changing one.

Replay a reported failure with its `VALO_FUZZ_SEED`; fast-check also prints the
shrink path when one exists.

Both canvases sit in one flex row captured by a single screenshot and split down
the middle. Each Playwright screenshot is a round trip and it dominates the cost
of a run, so the count per scene is what a time budget buys scenes with.
