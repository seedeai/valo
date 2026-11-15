// Captures Skia's tests/PathOpsTigerTest.cpp.
#include "tests/PathOpsTigerTest.cpp"

#include "recorder.h"

static void CaptureFile(reference::Capture& capture) {
    capture.runSet({tests, testCount, firstTest, skipTest, stopTest, runReverse, true,
                    {{"tiger_threaded", "a threaded generator"}}});
}

static reference::FixtureRegistration registration("PathOpsTigerTest",
                                                   "tests/PathOpsTigerTest.cpp", CaptureFile);
