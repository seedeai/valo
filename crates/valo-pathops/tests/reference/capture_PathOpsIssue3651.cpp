// Captures Skia's tests/PathOpsIssue3651.cpp.
#include "tests/PathOpsIssue3651.cpp"

#include "recorder.h"

static void CaptureFile(reference::Capture& capture) {
    capture.runSet({tests, testCount, firstTest, skipTest, stopTest, runReverse, true, {}});
}

static reference::FixtureRegistration registration("PathOpsIssue3651", "tests/PathOpsIssue3651.cpp",
                                                   CaptureFile);
