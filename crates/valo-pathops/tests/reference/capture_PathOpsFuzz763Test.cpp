// Captures Skia's tests/PathOpsFuzz763Test.cpp.
#include "tests/PathOpsFuzz763Test.cpp"

#include "recorder.h"

static void CaptureFile(reference::Capture& capture) {
    capture.runSet({tests, testCount, firstTest, skipTest, stopTest, runReverse, true, {}});
}

static reference::FixtureRegistration registration("PathOpsFuzz763Test", "tests/PathOpsFuzz763Test.cpp",
                                                   CaptureFile);
