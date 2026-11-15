// Skia's PathOps test helpers (tests/PathOpsExtendedTest.h), recording each
// call into the current Capture instead of comparing bitmaps, and the Op and
// Simplify a test calls directly, recorded the same way.
#include "recorder.h"

#include "tests/PathOpsDebug.h"
#include "tests/PathOpsExtendedTest.h"
#include "tests/PathOpsThreadedCommon.h"
#include "tests/Test.h"

#include <cstdio>
#include <cstdlib>

using reference::CurrentCapture;
using reference::Expect;

namespace {

bool RecordOp(const SkPath& a, const SkPath& b, SkPathOp op, const char* name,
              const char* helper, Expect expect) {
    return CurrentCapture()->recordOp(name, helper, a, b, op, expect).has_value();
}

bool RecordSimplify(const SkPath& path, const char* name, const char* helper, Expect expect) {
    return CurrentCapture()->recordSimplify(name, helper, path, expect).has_value();
}

[[noreturn]] void Unsupported(const char* what) {
    fprintf(stderr, "%s is not supported by the reference driver\n", what);
    abort();
}

}  // namespace

std::optional<SkPath> Op(const SkPath& one, const SkPath& two, SkPathOp op) {
    if (CurrentCapture()) {
        return CurrentCapture()->recordDirectOp(one, two, op);
    }
    return SkiaOp(one, two, op);
}

std::optional<SkPath> Simplify(const SkPath& path) {
    if (CurrentCapture()) {
        return CurrentCapture()->recordDirectSimplify(path);
    }
    return SkiaSimplify(path);
}

bool testPathOp(skiatest::Reporter*, const SkPath& a, const SkPath& b, const SkPathOp op,
                const char* testName) {
    return RecordOp(a, b, op, testName, "testPathOp", Expect::kYes);
}

bool testPathOpCheck(skiatest::Reporter*, const SkPath& a, const SkPath& b, const SkPathOp op,
                     const char* testName, bool checkFail) {
    return RecordOp(a, b, op, testName, "testPathOpCheck",
                    checkFail ? Expect::kYes : Expect::kNo);
}

bool testPathOpFail(skiatest::Reporter*, const SkPath& a, const SkPath& b, const SkPathOp op,
                    const char* testName) {
    return !RecordOp(a, b, op, testName, "testPathOpFail", Expect::kNo);
}

bool testPathOpFuzz(skiatest::Reporter*, const SkPath& a, const SkPath& b, const SkPathOp op,
                    const char* testName) {
    return RecordOp(a, b, op, testName, "testPathOpFuzz", Expect::kFlaky);
}

bool testSimplify(skiatest::Reporter*, const SkPath& path, const char* filename) {
    return RecordSimplify(path, filename, "testSimplify", Expect::kYes);
}

bool testSimplifyCheck(skiatest::Reporter*, const SkPath& path, const char* filename,
                       bool checkFail) {
    return RecordSimplify(path, filename, "testSimplifyCheck",
                          checkFail ? Expect::kYes : Expect::kNo);
}

bool testSimplifyFail(skiatest::Reporter*, const SkPath& path, const char* filename) {
    return !RecordSimplify(path, filename, "testSimplifyFail", Expect::kNo);
}

bool testSimplifyFuzz(skiatest::Reporter*, const SkPath& path, const char* filename) {
    return RecordSimplify(path, filename, "testSimplifyFuzz", Expect::kFlaky);
}

// The capture files run each TestDesc array themselves, with its filters.
void RunTestSet(skiatest::Reporter*, TestDesc[], size_t, void (*)(skiatest::Reporter*, const char*),
                void (*)(skiatest::Reporter*, const char*),
                void (*)(skiatest::Reporter*, const char*), bool) {
    Unsupported("RunTestSet");
}

void initializeTests(skiatest::Reporter*, const char*) {}

// The threaded generators are linked through their TestDesc arrays but never run.
bool testSimplify(SkPath&, bool, SkPath&, PathOpsThreadState&, const char*) {
    Unsupported("the threaded testSimplify");
}
void PathOpsThreadState::outputProgress(const char*, SkPathFillType) {
    Unsupported("PathOpsThreadState");
}
void PathOpsThreadState::outputProgress(const char*, SkPathOp) {
    Unsupported("PathOpsThreadState");
}
PathOpsThreadedTestRunner::~PathOpsThreadedTestRunner() {}
void PathOpsThreadedTestRunner::render() { Unsupported("PathOpsThreadedTestRunner"); }

bool PathOpsDebug::gJson = false;
bool PathOpsDebug::gMarkJsonFlaky = false;
bool PathOpsDebug::gOutFirst = true;
bool PathOpsDebug::gCheckForDuplicateNames = false;
bool PathOpsDebug::gOutputSVG = false;
FILE* PathOpsDebug::gOut = nullptr;
