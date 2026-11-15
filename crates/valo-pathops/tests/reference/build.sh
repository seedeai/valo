#!/bin/sh
# Builds Skia's PathOps as Skia's release build does, runs Skia's own PathOps
# tests and the Flutter cases through it, and writes each case and Skia's
# result into ../fixtures, which tests/skia.rs checks the port against.
#
# Usage: build.sh <skia-dir> [build-dir]
#
# Only clang++ is needed: no gn, ninja or bazel, nothing downloaded. The build
# directory defaults to the workspace's target/pathops-reference; the Skia
# checkout is only read.
set -eu

if [ $# -lt 1 ]; then
    echo "usage: $0 <skia-dir> [build-dir]" >&2
    exit 2
fi
skia=$(cd "$1" && pwd)
here=$(cd "$(dirname "$0")" && pwd)
build=${2:-$here/../../../../target/pathops-reference}
fixtures=$here/../fixtures
cxx=${CXX:-clang++}
jobs=${JOBS:-8}

# SK_RELEASE and NDEBUG are Skia's release build; SkPathOpsDebug.h then sets
# FORCE_RELEASE, so none of PathOps' debug code runs.
#
# -ffp-contract=off: clang's default (on) fuses a * b + c into one fused
# multiply-add, rounded once instead of twice, wherever the target has FMA
# (every arm64 machine). Rust never fuses, so the port computes what Skia's
# source says, as Skia does on a target without FMA. Built with fusing, about
# 30 of these cases give other results.
flags="-std=c++20 -O2 -DSK_RELEASE -DNDEBUG -ffp-contract=off -w"

# Skia's PathOps but AsWinding and TightBounds, which Op and Simplify do not
# call. SkOpBuilder.cpp is there only to link the SkOpBuilder tests of
# PathOpsBuilderConicTest.cpp, which are not run.
pathops=$(cd "$skia" && ls src/pathops/*.cpp | grep -v -e SkPathOpsAsWinding -e SkPathOpsTightBounds)

# What PathOps and the tests' path building need of Skia's core (SkData,
# SkStream and the file ports only for SkParsePath, which a test parses an
# SVG path with).
core="
src/base/SkArenaAlloc.cpp
src/base/SkBezierCurves.cpp
src/base/SkBuffer.cpp
src/base/SkContainers.cpp
src/base/SkCubics.cpp
src/base/SkDebug.cpp
src/base/SkFloatingPoint.cpp
src/base/SkLog.cpp
src/base/SkMalloc.cpp
src/base/SkQuads.cpp
src/base/SkSafeMath.cpp
src/base/SkSemaphore.cpp
src/base/SkTDArray.cpp
src/base/SkUTF.cpp
src/base/SkUtils.cpp
src/core/SkCubicClipper.cpp
src/core/SkData.cpp
src/core/SkEdgeClipper.cpp
src/core/SkGeometry.cpp
src/core/SkIDChangeListener.cpp
src/core/SkLineClipper.cpp
src/core/SkMatrix.cpp
src/core/SkPath.cpp
src/core/SkPathBuilder.cpp
src/core/SkPathData.cpp
src/core/SkPathDump.cpp
src/core/SkPathIter.cpp
src/core/SkPathPriv.cpp
src/core/SkPathRaw.cpp
src/core/SkPathRawShapes.cpp
src/core/SkPoint.cpp
src/core/SkRRect.cpp
src/core/SkRect.cpp
src/core/SkString.cpp
src/core/SkStream.cpp
src/core/SkStringUtils.cpp
src/ports/SkLog_stdio.cpp
src/ports/SkMemory_malloc.cpp
src/ports/SkOSFile_posix.cpp
src/ports/SkOSFile_stdio.cpp
src/utils/SkParse.cpp
src/utils/SkParsePath.cpp
"

reference="
recorder.cpp
helpers.cpp
flutter.cpp
main.cpp
$(cd "$here" && ls capture_*.cpp)
"

mkdir -p "$build/obj"
rm -f "$build"/obj/*.o

# compile <source-root> <extra flags> <sources...>: each into obj/, in parallel.
compile() {
    root=$1
    extra=$2
    shift 2
    for source in "$@"; do
        echo "$source"
    done | xargs -P "$jobs" -I{} sh -c '
        object="$0/obj/$(echo "{}" | tr / _ | sed "s/\.cpp$/.o/")"
        exec '"$cxx"' '"$flags"' $1 -c "$2/{}" -o "$object"
    ' "$build" "$extra" "$root"
}

# Op and Simplify are renamed SkiaOp and SkiaSimplify where Skia defines
# them, so that the Op and Simplify a test calls are helpers.cpp's, which
# record the call.
renamed="src/pathops/SkPathOpsOp.cpp src/pathops/SkPathOpsSimplify.cpp"
compile "$skia" "-I$skia -DOp=SkiaOp -DSimplify=SkiaSimplify" $renamed
compile "$skia" "-I$skia" $(echo "$pathops" | grep -v -e SkPathOpsOp.cpp -e SkPathOpsSimplify.cpp) $core
# The tests' own CubicPathToQuads, which builds the inputs of a few tests.
compile "$skia" "-I$here/shim -I$skia" tests/PathOpsTestCommon.cpp
compile "$here" "-I$here/shim -I$skia" $reference

"$cxx" "$build"/obj/*.o -o "$build/reference"

mkdir -p "$fixtures"
"$build/reference" "$fixtures" "$(git -C "$skia" rev-parse HEAD)"
