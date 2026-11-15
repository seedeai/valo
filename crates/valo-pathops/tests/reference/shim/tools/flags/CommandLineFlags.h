// Stands in for Skia's tools/flags/CommandLineFlags.h: PathOpsBuilderConicTest.cpp
// defines flags only for its SkOpBuilder tests, which are not captured.
#ifndef CommandLineFlags_DEFINED
#define CommandLineFlags_DEFINED

#define DEFINE_int(name, defaultValue, helpString) int FLAGS_##name = defaultValue

#endif
