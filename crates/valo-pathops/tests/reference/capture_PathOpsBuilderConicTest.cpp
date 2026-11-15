// Captures the tests of Skia's tests/PathOpsBuilderConicTest.cpp that call
// Op or Simplify with fixed paths; the others build with SkOpBuilder.
#include "tests/PathOpsBuilderConicTest.cpp"

#include "recorder.h"

static void CaptureFile(reference::Capture& capture) {
    capture.exclude("SixtyOvals", "uses SkOpBuilder");
    capture.exclude("SixtyOvalsOneOff", "uses SkOpBuilder");
    capture.runTest("SixtyOvals_2_2_9_73", test_SixtyOvals_2_2_9_73);
    capture.runTest("SixtyOvals_2_2_9_73_reduced", test_SixtyOvals_2_2_9_73_reduced);
    capture.runTest("SixtyOvalsA", test_SixtyOvalsA);
    capture.runTest("SixtyOvalsAX", test_SixtyOvalsAX);
    capture.exclude("PathOpsOvalsAsQuads", "disabled in Skia (if (false))");
    capture.exclude("PathOps64OvalsAsQuads", "disabled in Skia, and uses SkOpBuilder");
}

static reference::FixtureRegistration registration("PathOpsBuilderConicTest",
                                                   "tests/PathOpsBuilderConicTest.cpp",
                                                   CaptureFile);
