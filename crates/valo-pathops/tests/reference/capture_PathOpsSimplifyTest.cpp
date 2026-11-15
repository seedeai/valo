// Captures Skia's tests/PathOpsSimplifyTest.cpp.
#include "tests/PathOpsSimplifyTest.cpp"

#include "recorder.h"

static void CaptureFile(reference::Capture& capture) {
    capture.runSet({tests, testCount, firstTest, skipTest, stopTest, runReverse, true, {}});
    capture.runSet({subTests, subTestCount, firstSubTest, nullptr, stopTest, runReverse,
                    runSubTests, {}});
    capture.runTest("bug_513001309", test_bug_513001309);
}

static reference::FixtureRegistration registration("PathOpsSimplifyTest",
                                                   "tests/PathOpsSimplifyTest.cpp", CaptureFile);
