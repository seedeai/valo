// Stands in for Skia's tests/Test.h when Skia's PathOps tests are compiled by
// build.sh: a DEF_TEST becomes a plain function the capture files call, and a
// failed REPORTER_ASSERT or ERRORF is printed with the case being captured
// instead of failing a Skia test run.
#ifndef Test_DEFINED
#define Test_DEFINED

#include "include/core/SkString.h"
#include "include/core/SkTypes.h"

#include <string>

namespace skiatest {

class Reporter {
public:
    void bumpTestCount() {}
    bool allowExtendedTest() const { return false; }
    bool verbose() const { return false; }
    void push(const SkString&) {}
    void push(const std::string&) {}
    void pop() {}
};

}  // namespace skiatest

namespace reference {
void ReportCheckFailure(const char* file, int line, const char* condition);
}

#define REPORTER_ASSERT(r, cond, ...)                                  \
    do {                                                               \
        if (!(cond)) {                                                 \
            reference::ReportCheckFailure(__FILE__, __LINE__, #cond);  \
        }                                                              \
    } while (0)

#define ERRORF(r, ...) reference::ReportCheckFailure(__FILE__, __LINE__, "ERRORF")

#define DEF_TEST(name, reporter) static void test_##name(skiatest::Reporter* reporter)

#endif
