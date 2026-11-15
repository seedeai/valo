// Captures Skia's tests/PathOpsBattles.cpp.
#include "tests/PathOpsBattles.cpp"

#include "recorder.h"

static void CaptureFile(reference::Capture& capture) {
    capture.runSet({tests, testCount, firstTest, nullptr, stopTest, runReverse, true, {}});
}

static reference::FixtureRegistration registration("PathOpsBattles", "tests/PathOpsBattles.cpp",
                                                   CaptureFile);
