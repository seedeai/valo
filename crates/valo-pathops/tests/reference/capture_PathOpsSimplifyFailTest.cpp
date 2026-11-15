// Captures Skia's tests/PathOpsSimplifyFailTest.cpp.
#include "tests/PathOpsSimplifyFailTest.cpp"

#include "recorder.h"

static void CaptureFile(reference::Capture& capture) {
    capture.runTest("PathOpsSimplifyFail", test_PathOpsSimplifyFail);
    capture.comment("not captured: PathOpsSimplifyDontFailOne, a repeat of "
                    "PathOpsSimplifyFail's dontFailOne(17)");
}

static reference::FixtureRegistration registration("PathOpsSimplifyFailTest",
                                                   "tests/PathOpsSimplifyFailTest.cpp",
                                                   CaptureFile);
