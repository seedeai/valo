// Records PathOps cases and Skia's results for them as fixture text.
//
// A Capture is one fixture file. The capture_*.cpp files each compile one of
// Skia's test files and run its tests through a Capture; Skia's test helpers
// (helpers.cpp) record each Op or Simplify call into the current Capture.
#ifndef reference_recorder_DEFINED
#define reference_recorder_DEFINED

#include "include/core/SkPath.h"
#include "include/pathops/SkPathOps.h"
#include "tests/PathOpsExtendedTest.h"

#include <map>
#include <set>
#include <string>
#include <vector>

// Skia's own Op and Simplify: build.sh renames them in SkPathOpsOp.cpp and
// SkPathOpsSimplify.cpp, so that the Op and Simplify a test calls directly are
// helpers.cpp's recording versions.
std::optional<SkPath> SkiaOp(const SkPath& one, const SkPath& two, SkPathOp op);
std::optional<SkPath> SkiaSimplify(const SkPath& path);

namespace reference {

// What a Skia test helper expects of Op or Simplify, as ExpectSuccess in
// tests/PathOpsExtendedTest.cpp; kUnchecked for a direct call, whose test
// checks the result itself, if at all.
enum class Expect { kYes, kNo, kFlaky, kUnchecked };

using TestFunction = void (*)(skiatest::Reporter*, const char* filename);
using DefinedTest = void (*)(skiatest::Reporter*);

// A TestDesc array and the filters Skia's DEF_TEST hands RunTestSet for it.
struct TestSet {
    TestDesc* tests;
    size_t count;
    TestFunction first;
    TestFunction skip;
    TestFunction stop;
    bool reverse;
    bool skiaRunsSet;  // false when Skia's DEF_TEST leaves the array out (runSubTests)
    std::map<std::string, std::string> excluded;  // test name -> why it is not captured
};

struct CaptureStats {
    int cases = 0;
    int expectedToFail = 0;
    int skippedBySkia = 0;
    int failedSkiasOwnCheck = 0;
    int rebuiltDiffers = 0;
    std::vector<std::string> notes;
};

class Capture {
public:
    explicit Capture(std::string header);

    // Runs every test of the set, ignoring its filters but recording which
    // tests Skia's own run leaves out.
    void runSet(const TestSet& set);
    // Runs a DEF_TEST's body, as Skia's run does.
    void runTest(const char* name, DefinedTest test);
    void exclude(const char* name, const char* reason);
    void comment(const std::string& text);
    // A REPORTER_ASSERT or ERRORF of the running test that failed.
    void noteCheckFailure(const char* file, int line, const char* condition);

    // Each returns Skia's result for the paths as the test built them.
    std::optional<SkPath> recordOp(const char* name, const char* helper, const SkPath& one,
                                   const SkPath& two, SkPathOp op, Expect expect);
    std::optional<SkPath> recordSimplify(const char* name, const char* helper,
                                         const SkPath& path, Expect expect);
    // An Op or Simplify a test calls itself, named after the running test; a
    // repeat of a call the same test made before is not recorded again.
    std::optional<SkPath> recordDirectOp(const SkPath& one, const SkPath& two, SkPathOp op);
    std::optional<SkPath> recordDirectSimplify(const SkPath& path);

    const std::string& text() const { return fText; }
    const CaptureStats& stats() const { return fStats; }

private:
    void beginTest(const char* name, bool skiaRuns);
    std::string uniqueName(const char* name);
    void writeCase(const char* name, const char* helper, const std::string& kind,
                   const std::vector<SkPath>& inputs, Expect expect,
                   const std::optional<SkPath>& result);

    std::string fText;
    CaptureStats fStats;
    std::string fTestName;
    bool fSkiaRuns = true;
    std::map<std::string, int> fNameCounts;
    std::set<std::string> fCallsInTest;
};

// The Capture a running test records into, or null outside a capture.
Capture* CurrentCapture();
void SetCurrentCapture(Capture* capture);

// One fixture file: its name (the fixture is <name>.txt) and what fills it.
struct FixtureFile {
    const char* name;
    const char* source;
    void (*capture)(Capture&);
};
std::vector<FixtureFile>& FixtureFiles();

struct FixtureRegistration {
    FixtureRegistration(const char* name, const char* source, void (*capture)(Capture&)) {
        FixtureFiles().push_back({name, source, capture});
    }
};

}  // namespace reference

#endif
