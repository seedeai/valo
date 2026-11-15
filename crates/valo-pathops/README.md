# valo-pathops

Skia's PathOps, ported line for line to Rust: boolean operations on paths (difference, intersect, union, xor, reverse difference) giving Skia's results bit for bit. Flutter's `Path.combine` calls Skia's `Op` under both Skia and Impeller, so this is what a Flutter port needs to cut one area from another.

It takes `SkPath`s (built with `SkPathBuilder`, or from raw arrays with `SkPath::from_raw`) and provides `op` and `simplify`, which return Skia's result filled even-odd, or `PathOpsError` exactly where Skia returns no path. It has no dependencies. valo-geometry uses it for `Path::combine`, which is the API valo's users call; this crate keeps Skia's names so it can be read beside Skia.

## Source

Ported from Skia at revision `d45969a5752ed559261ad982690e5b927186d12e` (2026-05-20), from `src/pathops` and the parts of Skia's core it calls. Skia is BSD-3-Clause, Copyright (c) 2011 Google Inc.; its licence is in `LICENSE-skia` and covers the ported code.

Each module is one Skia file pair, header and source together, under the same name (`SkOpSegment.{h,cpp}` is `op_segment.rs`), in Skia's order, with Skia's type names and its function names in snake case. Skia's objects point at each other; here each is a copyable handle named after its class, standing for Skia's pointer, with its fields in `SkOpGlobalState`'s arenas, so the port needs no `unsafe`. Arithmetic is in Skia's order, in doubles where Skia uses doubles; Rust never fuses a multiply and an add, which matches Skia built with `-ffp-contract=off`.

### Ported

What `Op` reaches: `src/pathops` without the files below, 19,687 lines of Skia (counting headers, comments and blank lines), leaving out within them the debug code (`DEBUG_*`, `SkDEBUGCODE`, `dump*`, `debug*`, which Skia's `FORCE_RELEASE` turns off in every build) and the declarations `Op` never calls. `Simplify` is included because `Op` calls it when one operand is empty.

From Skia's core, the parts PathOps calls (`src/core`): `SkPath`'s storage and the queries PathOps makes (`isRect`, `isConvex` with its convexity computation, `isFinite`, bounds, iteration), `SkPathBuilder` and its reversing helpers, the `SkGeometry` functions (`SkConic::chopAt`, `SkChopQuadAtMaxCurvature`, `SkFindQuadMaxCurvature`, `SkClassifyCubic`), the float helpers, and `SkTQSort`, whose unstable order decides ties.

The port is 23,400 lines of Rust in `src`.

### Left out

| Skia file | Lines | Why |
|---|---|---|
| `SkPathOpsDebug.{h,cpp}` | 3,541 | debug dumps and checks, off in Skia's own builds |
| `SkPathOpsAsWinding.cpp` | 459 | `AsWinding`, not reached by `Op` |
| `SkOpBuilder.cpp` | 211 | `SkOpBuilder`, not reached by `Op` |
| `SkPathOpsTightBounds.cpp` | 83 | `TightBounds`, not reached by `Op` |
| `SkDCubicToQuads.cpp` | 45 | not reached by `Op` |

Skia can return an inverse fill type when an input has one; `SkPathFillType` keeps Skia's four, so the inverse cases are ported and tested, though valo's `Path::combine` never passes an inverse fill.

## Tests

`tests/skia.rs` runs the port over 1,889 cases recorded from real Skia and requires Skia's answer bit for bit: success or failure, then fill type, verbs, points and conic weights. The cases are every `Op` and `Simplify` call in Skia's own tests (`PathOpsOpTest`, `PathOpsBattles`, `PathOpsFuzz763Test`, `PathOpsIssue3651`, `PathOpsSkpTest`, `PathOpsInverseTest`, `PathOpsSimplifyTest`, `PathOpsSimplifyFailTest`, `PathOpsTigerTest`, `PathOpsChalkboardTest`, `PathOpsBuilderConicTest`), including the ones Skia expects to fail, and Flutter's `Path.combine` calls (a text field's label gap, a bottom app bar's notch around a circle and around a rounded rectangle). `tests/reference` builds Skia and records them; its README says how, and how to run one case.

```sh
cargo test -p valo-pathops
PATHOPS_CASE=circle-notch cargo test -p valo-pathops --test skia -- --nocapture
tests/reference/build.sh /path/to/skia   # regenerate tests/fixtures from Skia
```
