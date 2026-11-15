// Captures Skia's tests/PathOpsSkpTest.cpp.
#include "tests/PathOpsSkpTest.cpp"

#include "recorder.h"

static void CaptureFile(reference::Capture& capture) {
    capture.runSet({tests, testCount, firstTest, skipTest, stopTest, runReverse, true, {}});
}

static reference::FixtureRegistration registration("PathOpsSkpTest", "tests/PathOpsSkpTest.cpp",
                                                   CaptureFile);
