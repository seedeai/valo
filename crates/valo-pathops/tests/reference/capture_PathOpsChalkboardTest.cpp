// Captures Skia's tests/PathOpsChalkboardTest.cpp.
#include "tests/PathOpsChalkboardTest.cpp"

#include "recorder.h"

static void CaptureFile(reference::Capture& capture) {
    capture.runSet({tests, testCount, firstTest, skipTest, stopTest, runReverse, true,
                    {{"chalkboard_threaded", "a threaded generator"}}});
}

static reference::FixtureRegistration registration("PathOpsChalkboardTest",
                                                   "tests/PathOpsChalkboardTest.cpp",
                                                   CaptureFile);
