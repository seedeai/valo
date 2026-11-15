// Captures Skia's tests/PathOpsInverseTest.cpp.
#include "tests/PathOpsInverseTest.cpp"

#include "recorder.h"

static void CaptureFile(reference::Capture& capture) {
    capture.runTest("PathOpsInverse", test_PathOpsInverse);
}

static reference::FixtureRegistration registration("PathOpsInverseTest",
                                                   "tests/PathOpsInverseTest.cpp", CaptureFile);
