// Records PathOps cases and Skia's results for them as fixture text.
//
// Fixture format, one item per line, fields separated by single spaces:
//
//   # comment
//   case <name>
//   kind op <difference|intersect|union|xor|reverse-difference> | kind simplify
//   helper <the Skia test helper that made the call, or "direct">
//   expect <yes|no|flaky|unchecked>
//   skia-runs <yes|no>          (no: Skia's own run skips this test)
//   input <path>                (two for an op, one for simplify)
//   result <path> | result none (none: Skia returned no path)
//
// A <path> is "<fill> <verbs> <points> <weights>": fill is winding, evenodd,
// inverse-winding or inverse-evenodd; verbs one letter each (M L Q K C Z, K
// for a conic); points comma-separated x:y pairs and weights comma-separated,
// each an f32 bit pattern in 8 hex digits; "-" for an empty list. A path that
// is not finite (SkPath's error path, holding nothing) is "<fill> nonfinite".
#include "recorder.h"

#include "include/core/SkPathTypes.h"
#include "src/base/SkFloatBits.h"
#include "tests/Test.h"

#include <cmath>
#include <cstdio>
#include <cstring>

namespace reference {

namespace {

Capture* gCurrentCapture = nullptr;

const char* FillName(SkPathFillType fill) {
    switch (fill) {
        case SkPathFillType::kWinding: return "winding";
        case SkPathFillType::kEvenOdd: return "evenodd";
        case SkPathFillType::kInverseWinding: return "inverse-winding";
        case SkPathFillType::kInverseEvenOdd: return "inverse-evenodd";
    }
    return "?";
}

const char* OpName(SkPathOp op) {
    switch (op) {
        case kDifference_SkPathOp: return "difference";
        case kIntersect_SkPathOp: return "intersect";
        case kUnion_SkPathOp: return "union";
        case kXOR_SkPathOp: return "xor";
        case kReverseDifference_SkPathOp: return "reverse-difference";
    }
    return "?";
}

const char* ExpectName(Expect expect) {
    switch (expect) {
        case Expect::kYes: return "yes";
        case Expect::kNo: return "no";
        case Expect::kFlaky: return "flaky";
        case Expect::kUnchecked: return "unchecked";
    }
    return "?";
}

char VerbLetter(SkPathVerb verb) {
    switch (verb) {
        case SkPathVerb::kMove: return 'M';
        case SkPathVerb::kLine: return 'L';
        case SkPathVerb::kQuad: return 'Q';
        case SkPathVerb::kConic: return 'K';
        case SkPathVerb::kCubic: return 'C';
        case SkPathVerb::kClose: return 'Z';
    }
    return '?';
}

std::string Bits(float value) {
    char text[9];
    snprintf(text, sizeof(text), "%08x", (uint32_t)SkFloat2Bits(value));
    return text;
}

std::string PathText(const SkPath& path) {
    std::string text = FillName(path.getFillType());
    if (!path.isFinite()) {
        return text + " nonfinite";
    }
    text += ' ';
    if (path.verbs().empty()) {
        text += '-';
    }
    for (SkPathVerb verb : path.verbs()) {
        text += VerbLetter(verb);
    }
    text += ' ';
    if (path.points().empty()) {
        text += '-';
    }
    for (size_t index = 0; index < path.points().size(); ++index) {
        const SkPoint& point = path.points()[index];
        text += (index ? "," : "") + Bits(point.fX) + ":" + Bits(point.fY);
    }
    text += ' ';
    if (path.conicWeights().empty()) {
        text += '-';
    }
    for (size_t index = 0; index < path.conicWeights().size(); ++index) {
        text += (index ? "," : "") + Bits(path.conicWeights()[index]);
    }
    return text;
}

// The path the fixture's text gives back through SkPath::Raw, as the Rust test
// builds it with SkPath::from_raw: a non-finite path from a NaN point, which
// Raw rejects.
SkPath Rebuilt(const SkPath& path) {
    if (!path.isFinite()) {
        const SkPoint nan = {NAN, NAN};
        const SkPathVerb move = SkPathVerb::kMove;
        return SkPath::Raw({&nan, 1}, {&move, 1}, {}, path.getFillType());
    }
    return SkPath::Raw(path.points(), path.verbs(), path.conicWeights(), path.getFillType());
}

std::string ResultText(const std::optional<SkPath>& result) {
    return result.has_value() ? PathText(*result) : std::string("none");
}

std::string OpCall(const SkPath& one, const SkPath& two, SkPathOp op) {
    return std::string(OpName(op)) + "|" + PathText(one) + "|" + PathText(two);
}

std::string SimplifyCall(const SkPath& path) { return "simplify|" + PathText(path); }

// Skia's RunTestSet (tests/PathOpsExtendedTest.cpp), marking the tests it
// would run instead of running them.
std::vector<bool> TestsSkiaRuns(const TestSet& set) {
    std::vector<bool> runs(set.count, false);
    if (!set.skiaRunsSet) {
        return runs;
    }
    TestDesc* tests = set.tests;
    size_t index;
    if (set.first) {
        index = set.count - 1;
        while (index > 0 && tests[index].fun != set.first) {
            --index;
        }
        runs[index] = true;
        if (tests[index].fun == set.stop) {
            return runs;
        }
    }
    index = set.reverse ? set.count - 1 : 0;
    size_t last = set.reverse ? 0 : set.count - 1;
    bool foundSkip = !set.skip;
    do {
        if (tests[index].fun == set.skip) {
            foundSkip = true;
        }
        if (foundSkip && tests[index].fun != set.first) {
            runs[index] = true;
        }
        if (tests[index].fun == set.stop || index == last) {
            break;
        }
        index += set.reverse ? -1 : 1;
    } while (true);
    return runs;
}

bool MeetsExpectation(Expect expect, bool succeeded) {
    switch (expect) {
        case Expect::kYes: return succeeded;
        case Expect::kNo: return !succeeded;
        default: return true;
    }
}

}  // namespace

Capture* CurrentCapture() { return gCurrentCapture; }
void SetCurrentCapture(Capture* capture) { gCurrentCapture = capture; }

std::vector<FixtureFile>& FixtureFiles() {
    static std::vector<FixtureFile> files;
    return files;
}

Capture::Capture(std::string header) : fText(std::move(header)) {}

void Capture::comment(const std::string& text) { fText += "# " + text + "\n"; }

void Capture::exclude(const char* name, const char* reason) {
    this->comment(std::string("not captured: ") + name + ", " + reason);
}

void Capture::beginTest(const char* name, bool skiaRuns) {
    fTestName = name;
    fSkiaRuns = skiaRuns;
    fCallsInTest.clear();
}

void Capture::runSet(const TestSet& set) {
    std::vector<bool> skiaRuns = TestsSkiaRuns(set);
    skiatest::Reporter reporter;
    for (size_t index = 0; index < set.count; ++index) {
        const TestDesc& test = set.tests[index];
        auto excluded = set.excluded.find(test.str);
        if (excluded != set.excluded.end()) {
            this->exclude(test.str, excluded->second.c_str());
            continue;
        }
        this->beginTest(test.str, skiaRuns[index]);
        (*test.fun)(&reporter, test.str);
    }
}

void Capture::runTest(const char* name, DefinedTest test) {
    skiatest::Reporter reporter;
    this->beginTest(name, true);
    (*test)(&reporter);
}

std::string Capture::uniqueName(const char* name) {
    int count = ++fNameCounts[name];
    return count == 1 ? std::string(name) : std::string(name) + "#" + std::to_string(count);
}

std::optional<SkPath> Capture::recordOp(const char* name, const char* helper, const SkPath& one,
                       const SkPath& two, SkPathOp op, Expect expect) {
    fCallsInTest.insert(OpCall(one, two, op));
    SkPath oneRebuilt = Rebuilt(one), twoRebuilt = Rebuilt(two);
    std::optional<SkPath> result = SkiaOp(oneRebuilt, twoRebuilt, op);
    std::optional<SkPath> resultAsBuilt = SkiaOp(one, two, op);
    if (ResultText(result) != ResultText(resultAsBuilt)) {
        ++fStats.rebuiltDiffers;
        fStats.notes.push_back(std::string(name) + ": the inputs rebuilt by SkPath::Raw give "
                               "another result than the test's own paths");
    }
    this->writeCase(name, helper, std::string("op ") + OpName(op), {oneRebuilt, twoRebuilt},
                    expect, result);
    return resultAsBuilt;
}

std::optional<SkPath> Capture::recordSimplify(const char* name, const char* helper, const SkPath& path,
                             Expect expect) {
    fCallsInTest.insert(SimplifyCall(path));
    SkPath rebuilt = Rebuilt(path);
    std::optional<SkPath> result = SkiaSimplify(rebuilt);
    std::optional<SkPath> resultAsBuilt = SkiaSimplify(path);
    if (ResultText(result) != ResultText(resultAsBuilt)) {
        ++fStats.rebuiltDiffers;
        fStats.notes.push_back(std::string(name) + ": the input rebuilt by SkPath::Raw gives "
                               "another result than the test's own path");
    }
    this->writeCase(name, helper, "simplify", {rebuilt}, expect, result);
    return resultAsBuilt;
}

std::optional<SkPath> Capture::recordDirectOp(const SkPath& one, const SkPath& two,
                                              SkPathOp op) {
    if (fCallsInTest.count(OpCall(one, two, op))) {
        return SkiaOp(one, two, op);
    }
    return this->recordOp(fTestName.c_str(), "direct", one, two, op, Expect::kUnchecked);
}

std::optional<SkPath> Capture::recordDirectSimplify(const SkPath& path) {
    if (fCallsInTest.count(SimplifyCall(path))) {
        return SkiaSimplify(path);
    }
    return this->recordSimplify(fTestName.c_str(), "direct", path, Expect::kUnchecked);
}

void Capture::writeCase(const char* name, const char* helper, const std::string& kind,
                        const std::vector<SkPath>& inputs, Expect expect,
                        const std::optional<SkPath>& result) {
    std::string caseName = this->uniqueName(name);
    fText += "case " + caseName + "\n";
    fText += "kind " + kind + "\n";
    fText += std::string("helper ") + helper + "\n";
    fText += std::string("expect ") + ExpectName(expect) + "\n";
    fText += std::string("skia-runs ") + (fSkiaRuns ? "yes" : "no") + "\n";
    for (const SkPath& input : inputs) {
        fText += "input " + PathText(input) + "\n";
    }
    fText += "result " + ResultText(result) + "\n";

    ++fStats.cases;
    fStats.expectedToFail += expect == Expect::kNo;
    fStats.skippedBySkia += !fSkiaRuns;
    if (!MeetsExpectation(expect, result.has_value())) {
        ++fStats.failedSkiasOwnCheck;
        fStats.notes.push_back(caseName + ": expected " + ExpectName(expect) + ", Skia " +
                               (result.has_value() ? "succeeded" : "failed"));
    }
}

void Capture::noteCheckFailure(const char* file, int line, const char* condition) {
    const char* base = strrchr(file, '/');
    fStats.notes.push_back(fTestName + ": its own check failed at " + (base ? base + 1 : file) +
                           ":" + std::to_string(line) + ": " + condition);
}

void ReportCheckFailure(const char* file, int line, const char* condition) {
    if (Capture* capture = CurrentCapture()) {
        capture->noteCheckFailure(file, line, condition);
    }
}

}  // namespace reference
