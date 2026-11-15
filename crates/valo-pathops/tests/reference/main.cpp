// Writes one fixture file per captured Skia test file, and a summary of each.
//
// Usage: reference <fixtures-dir> <skia-revision> [fixture-name...]
#include "recorder.h"

#include <cstdio>
#include <cstring>
#include <string>

namespace {

std::string Header(const reference::FixtureFile& file, const char* revision) {
    return std::string("# Skia's PathOps results for ") + file.source + ", Skia " + revision +
           ".\n# Written by tests/reference/build.sh; recorder.cpp there describes the format.\n";
}

bool Selected(const char* name, int argc, char** argv) {
    if (argc <= 3) {
        return true;
    }
    for (int index = 3; index < argc; ++index) {
        if (!strcmp(argv[index], name)) {
            return true;
        }
    }
    return false;
}

bool WriteFixture(const std::string& path, const std::string& text) {
    FILE* file = fopen(path.c_str(), "w");
    if (!file) {
        return false;
    }
    bool written = fwrite(text.data(), 1, text.size(), file) == text.size();
    return fclose(file) == 0 && written;
}

void PrintSummary(const char* name, const reference::CaptureStats& stats) {
    printf("%-26s %6d cases, %4d expected to fail, %4d skipped by Skia's run, "
           "%3d failing Skia's own check, %d differing when rebuilt\n",
           name, stats.cases, stats.expectedToFail, stats.skippedBySkia,
           stats.failedSkiasOwnCheck, stats.rebuiltDiffers);
    for (const std::string& note : stats.notes) {
        printf("    %s\n", note.c_str());
    }
}

}  // namespace

int main(int argc, char** argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: %s <fixtures-dir> <skia-revision> [fixture-name...]\n", argv[0]);
        return 2;
    }
    for (const reference::FixtureFile& file : reference::FixtureFiles()) {
        if (!Selected(file.name, argc, argv)) {
            continue;
        }
        reference::Capture capture(Header(file, argv[2]));
        reference::SetCurrentCapture(&capture);
        file.capture(capture);
        reference::SetCurrentCapture(nullptr);
        std::string path = std::string(argv[1]) + "/" + file.name + ".txt";
        if (!WriteFixture(path, capture.text())) {
            fprintf(stderr, "cannot write %s\n", path.c_str());
            return 1;
        }
        PrintSummary(file.name, capture.stats());
    }
    return 0;
}
