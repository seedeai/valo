// The Path.combine calls Flutter's framework makes for a text field's label
// gap and a bottom app bar's notch, with the paths built as Flutter's engine
// builds them (engine/src/flutter/lib/ui/painting/path.cc): each dart:ui Path
// is an SkPathBuilder, its coordinates narrowed from double to float;
// Path.combine is CanvasPath::op, Op on the builders' snapshots.
#include "recorder.h"

#include "include/core/SkPathBuilder.h"
#include "include/core/SkRRect.h"

namespace {

using reference::Capture;
using reference::Expect;

// Path()..addRect(Rect.fromLTRB(left, top, right, bottom)): CanvasPath::addRect.
SkPath RectPath(float left, float top, float right, float bottom) {
    return SkPathBuilder().addRect(SkRect::MakeLTRB(left, top, right, bottom)).snapshot();
}

// Path()..addOval(rect): CanvasPath::addOval.
SkPath OvalPath(float left, float top, float right, float bottom) {
    return SkPathBuilder().addOval(SkRect::MakeLTRB(left, top, right, bottom)).snapshot();
}

// Path()..addRRect(RRect.fromRectAndRadius(rect, Radius.circular(radius))):
// CanvasPath::addRRect of the DlRoundRect dart:ui's RRect becomes (whose
// radii are kept when they fit the rect, and dropped when all are zero),
// through ToSkRRect.
SkPath RoundedRectPath(float left, float top, float right, float bottom, float radius) {
    const SkVector radii[4] = {{radius, radius}, {radius, radius}, {radius, radius},
                               {radius, radius}};
    SkRRect rrect;
    rrect.setRectRadii(SkRect::MakeLTRB(left, top, right, bottom), radii);
    return SkPathBuilder().addRRect(rrect).snapshot();
}

void LabelGap(Capture& capture) {
    capture.comment(
            "label-gap: ShapedInputBorder(shape: RoundedRectangleBorder(borderRadius: "
            "BorderRadius.circular(4))), gapPadding 4, BorderSide() of width 1, painted in "
            "Rect.fromLTWH(0, 0, 300, 56) with gapStart 16, gapExtent 32, gapPercentage 1, "
            "ltr: _gapBorderPath(rect.deflate(0.5), start 12, extent 40) is "
            "Path.combine(difference, Path()..addRRect(0.5, 0.5, 299.5, 55.5, radius 4), "
            "Path()..addRect(Rect.fromLTRB(12, -0.5, 52, 1.5))). "
            "(OutlineInputBorder draws its gap as an open stroked path, without Path.combine.)");
    capture.recordOp("label-gap", "flutter", RoundedRectPath(0.5f, 0.5f, 299.5f, 55.5f, 4),
                     RectPath(12, -0.5f, 52, 1.5f), kDifference_SkPathOp, Expect::kYes);
}

void CircleNotch(Capture& capture) {
    capture.comment(
            "circle-notch: AutomaticNotchedShape(RoundedRectangleBorder(), CircleBorder())"
            ".getOuterPath(Rect.fromLTWH(0, 0, 360, 56), guest) for a BottomAppBar, guest the "
            "56x56 centerDocked FloatingActionButton (152, -28, 208, 28) inflated by "
            "notchMargin 4: Path.combine(difference, Path()..addRRect(0, 0, 360, 56, radius 0), "
            "Path()..addOval(Rect.fromLTRB(148, -32, 212, 32))). "
            "(CircularNotchedRectangle cuts its notch with its own curves, without "
            "Path.combine.)");
    capture.recordOp("circle-notch", "flutter", RoundedRectPath(0, 0, 360, 56, 0),
                     OvalPath(148, -32, 212, 32), kDifference_SkPathOp, Expect::kYes);
}

void RoundedRectNotch(Capture& capture) {
    capture.comment(
            "rounded-rectangle-notch: AutomaticNotchedShape(RoundedRectangleBorder(), "
            "RoundedRectangleBorder(borderRadius: BorderRadius.circular(16)))"
            ".getOuterPath(Rect.fromLTWH(0, 0, 360, 56), guest), guest the 56x56 "
            "centerDocked FloatingActionButton inflated by notchMargin 4: "
            "Path.combine(difference, Path()..addRRect(0, 0, 360, 56, radius 0), "
            "Path()..addRRect(148, -32, 212, 32, radius 16)).");
    capture.recordOp("rounded-rectangle-notch", "flutter", RoundedRectPath(0, 0, 360, 56, 0),
                     RoundedRectPath(148, -32, 212, 32, 16), kDifference_SkPathOp,
                     Expect::kYes);
}

void CaptureFile(Capture& capture) {
    LabelGap(capture);
    CircleNotch(capture);
    RoundedRectNotch(capture);
}

reference::FixtureRegistration registration("flutter", "Flutter's Path.combine calls",
                                            CaptureFile);

}  // namespace
