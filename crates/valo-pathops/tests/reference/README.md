# Skia reference for valo-pathops

`build.sh` builds real Skia's PathOps with clang++ alone, runs Skia's own PathOps tests and the Flutter cases through it, and writes every call of `Op` or `Simplify`, with Skia's result, into `../fixtures`. `../skia.rs` runs the port over the same inputs and requires the same result bit for bit.

```sh
tests/reference/build.sh /path/to/skia [build-dir]   # Skia d45969a5752ed559261ad982690e5b927186d12e
cargo test -p valo-pathops --test skia -- --nocapture
PATHOPS_CASE=circle-notch cargo test -p valo-pathops --test skia -- --nocapture
```

The build is Skia's release (`SK_RELEASE`, `NDEBUG`, so PathOps' `FORCE_RELEASE` debug code is out), `-O2`, and `-ffp-contract=off`, since Rust never fuses a multiply and an add (`build.sh` says why). The build directory defaults to the workspace's `target/pathops-reference`; the Skia checkout is only read.

## How the tests are captured

- Each `capture_*.cpp` includes one of Skia's test files and runs its tests: every test of each `TestDesc` array, whatever the file's `firstTest`/`skipTest`/`stopTest` and `runSubTests` say, recording as `skia-runs no` the tests Skia's own run leaves out; and the `DEF_TEST`s that call `Op` or `Simplify` themselves. What is left out is listed in the fixture's comments: threaded generators, `SkOpBuilder` tests, disabled tests and repeats.
- `helpers.cpp` implements the helpers of Skia's `tests/PathOpsExtendedTest.h` (`testPathOp`, `testPathOpCheck`, `testSimplifyFuzz`, ...) to record each call and the success the helper expects (`ExpectSuccess` there: yes, no, flaky). `Op` and `Simplify` are renamed where Skia defines them, so that a test's own calls reach `helpers.cpp`, which records them as `helper direct`, `expect unchecked`.
- `shim/` stands in for Skia's `tests/Test.h` and `tools/flags/CommandLineFlags.h`.
- An input is recorded as the arrays the test's `SkPath` stores, and Skia's result is for that path rebuilt by `SkPath::Raw`, as the Rust test rebuilds it with `SkPath::from_raw`; the driver checks that the rebuilt path gives the same result as the test's own. A non-finite input is Skia's error path, which stores nothing: it is recorded as `nonfinite` and rebuilt from a NaN point.

`recorder.cpp` describes the fixture format.

## The Flutter cases

`flutter.cpp` builds the inputs of the `Path.combine(PathOperation.difference, ...)` calls in Flutter's framework (flutter/flutter at 9c427b5d) as Flutter's engine does (`engine/src/flutter/lib/ui/painting/path.cc`): each dart:ui `Path` is an `SkPathBuilder`, coordinates narrowed to float; `addRect` and `addOval` are the builder's; `addRRect` goes through `DlRoundRect` (radii kept when they fit, dropped when all zero) and `ToSkRRect`'s `setRectRadii`; `Path.combine` is `CanvasPath::op`, `Op` of the builders' snapshots. The paths come from this Skia, not the engine's own Skia revision.

- `label-gap`: `ShapedInputBorder(shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(4)))` painted in a 300×56 rect with a gap from x = 12, 40 wide (gapStart 16, gapExtent 32, gapPadding 4). Its `_gapBorderPath` subtracts the rect (12, -0.5, 52, 1.5) from the rounded rect (0.5, 0.5, 299.5, 55.5), radius 4: the rect deflated by half the default 1 px side. `OutlineInputBorder` itself does not call `Path.combine`: its `_gapBorderPath` draws the outline as an open path of arcs and lines that skips the gap, then strokes it.
- `circle-notch`: `AutomaticNotchedShape(RoundedRectangleBorder(), CircleBorder())` for a 360×56 `BottomAppBar` and a 56×56 `centerDocked` FAB, (152, -28, 208, 28), inflated by `notchMargin` 4: the oval (148, -32, 212, 32) subtracted from the bar's zero-radius rounded rect. `CircularNotchedRectangle`, the usual circular notch, does not call `Path.combine`: it draws the notch with its own curves.
- `rounded-rectangle-notch`: `AutomaticNotchedShape(RoundedRectangleBorder(), RoundedRectangleBorder(borderRadius: BorderRadius.circular(16)))`, the same bar and FAB: the rounded rect (148, -32, 212, 32), radius 16, subtracted from the bar.
