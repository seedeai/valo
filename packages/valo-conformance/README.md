# Valo conformance

Private browser tests for the Canvas-shaped API. The same serializable scene is
replayed into browser Canvas2D and Valo, then the live results are compared with
Vitest's BlazeDiff comparator. A failure saves both renders, the diff, and the
scene under `artifacts/`.

```sh
npm run build:web
npm run test:conformance       # curated/API scenes + 3 × 40 deterministic fuzz cases
npm run fuzz:canvas            # VALO_FUZZ_RUNS / SEED / TIME_LIMIT customize it
npm run benchmark:canvas       # JavaScript record + submit cost, no GPU wait
```

The generator covers geometry, strokes, all blend modes, clips, transforms,
linear/radial/conic gradients, shadows, every `drawImage` signature, smoothing,
patterns, and fixed-font fill/stroke text. Invalid-argument behavior and explicit
gaps have direct API tests. Early failures exposed real gradient interpolation,
destructive-compositing, shadow-alpha, baseline, and non-uniform text-scale bugs.

Set `VALO_CONFORMANCE_PROFILE=1` to print screenshot, decode, comparison, and
artifact timings. Replay a reported failure with its `VALO_FUZZ_SEED`; fast-check
also prints the shrink path when one exists.
