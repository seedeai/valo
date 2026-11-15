// Captures Skia's tests/PathOpsOpTest.cpp.
#include "tests/PathOpsOpTest.cpp"

#include "recorder.h"

static void CaptureFile(reference::Capture& capture) {
    capture.runTest("bug_513820666", test_bug_513820666);
    capture.runSet({tests, testCount, firstTest, skipTest, stopTest, runReverse, true, {}});
    capture.runSet({subTests, subTestCount, firstSubTest, nullptr, stopTest, runReverse,
                    runSubTests, {}});
    capture.runSet({failTests, failTestCount, nullptr, nullptr, nullptr, false, true, {}});
    capture.comment("not captured: PathOpsRepOp, a repeat of failTests' fuzz763_5a");
}

static reference::FixtureRegistration registration("PathOpsOpTest", "tests/PathOpsOpTest.cpp",
                                                   CaptureFile);
