import { beforeAll, describe, expect, test } from "vitest";
import {
  createConformanceHarness,
  type ConformanceHarness,
} from "../src/harness.js";
import { FIXTURE_FONT_FAMILY } from "../src/scene.js";

describe("Canvas-shaped query behavior", () => {
  let harness: ConformanceHarness;
  let nativeContext: CanvasRenderingContext2D;

  beforeAll(async () => {
    harness = await createConformanceHarness();
    const context = harness.nativeCanvas.getContext("2d");
    if (!context) throw new Error("Canvas2D is unavailable");
    nativeContext = context;
  });

  test("fixed-font advance width agrees with Canvas2D", () => {
    for (const size of [12, 24]) {
      const font = `${size}px '${FIXTURE_FONT_FAMILY}'`;
      nativeContext.font = font;
      harness.valoContext.font = font;
      for (const text of ["Valo canvas", "A quick fox", "fuzz test", "Valo ", " ", "a\tb", "a\nb"]) {
        const nativeWidth = nativeContext.measureText(text).width;
        const valoWidth = harness.valoContext.measureText(text).width;
        expect(Math.abs(nativeWidth - valoWidth)).toBeLessThanOrEqual(0.5);
      }
    }
  });

  test("fill hit-testing observes the current transform", () => {
    nativeContext.reset();
    nativeContext.translate(20, 12);
    nativeContext.beginPath();
    nativeContext.rect(4, 6, 30, 24);

    harness.valoContext.reset();
    harness.valoContext.translate(20, 12);
    harness.valoContext.beginPath();
    harness.valoContext.rect(4, 6, 30, 24);

    for (const point of [[28, 24], [10, 10], [58, 42]] as const) {
      expect(harness.valoContext.isPointInPath(point[0], point[1])).toBe(
        nativeContext.isPointInPath(point[0], point[1]),
      );
    }
  });

  test("fixed-font spacing agrees with Canvas2D", () => {
    const font = `31px '${FIXTURE_FONT_FAMILY}'`;
    nativeContext.font = font;
    nativeContext.letterSpacing = "2px";
    harness.valoContext.font = font;
    harness.valoContext.letterSpacing = "2px";
    const nativeWidth = nativeContext.measureText("Canvas").width;
    const valoWidth = harness.valoContext.measureText("Canvas").width;
    expect(Math.abs(nativeWidth - valoWidth)).toBeLessThanOrEqual(0.5);
  });

  test("fixed-font bounding metrics follow Canvas baselines", () => {
    const font = `30px '${FIXTURE_FONT_FAMILY}'`;
    for (const baseline of ["top", "middle", "alphabetic", "bottom"] as const) {
      nativeContext.reset();
      nativeContext.font = font;
      nativeContext.textBaseline = baseline;
      harness.valoContext.reset();
      harness.valoContext.font = font;
      harness.valoContext.textBaseline = baseline;
      const nativeMetrics = nativeContext.measureText("fuzz test");
      const valoMetrics = harness.valoContext.measureText("fuzz test");
      for (const key of [
        "actualBoundingBoxLeft",
        "actualBoundingBoxRight",
        "actualBoundingBoxAscent",
        "actualBoundingBoxDescent",
        "fontBoundingBoxAscent",
        "fontBoundingBoxDescent",
      ] as const) {
        expect(Math.abs(nativeMetrics[key] - valoMetrics[key])).toBeLessThanOrEqual(1);
      }
    }
  });

  test("empty text and ignored zero line width follow Canvas2D", () => {
    const font = `30px '${FIXTURE_FONT_FAMILY}'`;
    nativeContext.font = font;
    harness.valoContext.font = font;
    const valoMetrics = harness.valoContext.measureText("");
    const nativeEmptyMetrics = nativeContext.measureText("");
    expect(valoMetrics.width).toBe(0);
    // Ink-free text still HAS an actual bounding box — an empty one on the
    // alphabetic baseline. This asserted 0/0 until that was fixed, which
    // placed the box at the anchor instead, so it moved with textBaseline.
    // Chrome is the reference here, not the previous valo behaviour.
    for (const key of [
      "actualBoundingBoxLeft",
      "actualBoundingBoxRight",
      "actualBoundingBoxAscent",
      "actualBoundingBoxDescent",
    ] as const) {
      expect(Math.abs(valoMetrics[key] - nativeEmptyMetrics[key]), key)
        .toBeLessThanOrEqual(1);
    }
    expect(Math.abs(valoMetrics.fontBoundingBoxAscent - nativeEmptyMetrics.fontBoundingBoxAscent))
      .toBeLessThanOrEqual(1);
    expect(Math.abs(valoMetrics.fontBoundingBoxDescent - nativeEmptyMetrics.fontBoundingBoxDescent))
      .toBeLessThanOrEqual(1);

    nativeContext.lineWidth = 3;
    nativeContext.lineWidth = 0;
    harness.valoContext.lineWidth = 3;
    harness.valoContext.lineWidth = 0;
    expect(harness.valoContext.lineWidth).toBe(nativeContext.lineWidth);
  });

  test("rendering hints are saved and restored", () => {
    harness.valoContext.reset();
    expect(harness.valoContext.imageSmoothingQuality).toBe("low");
    expect(harness.valoContext.textRendering).toBe("auto");

    harness.valoContext.imageSmoothingQuality = "high";
    harness.valoContext.textRendering = "geometricPrecision";
    harness.valoContext.save();
    harness.valoContext.imageSmoothingQuality = "medium";
    harness.valoContext.textRendering = "optimizeSpeed";
    harness.valoContext.restore();

    expect(harness.valoContext.imageSmoothingQuality).toBe("high");
    expect(harness.valoContext.textRendering).toBe("geometricPrecision");
  });

  test("CSS filters are validated and saved as Canvas state", () => {
    harness.valoContext.reset();
    expect(harness.valoContext.filter).toBe("none");
    harness.valoContext.filter = "BLUR(0)";
    expect(harness.valoContext.filter).toBe("BLUR(0)");
    harness.valoContext.filter = "brightness(80%) blur(2px)";
    harness.valoContext.save();
    harness.valoContext.filter = "sepia(50%)";
    // drop-shadow used to be rejected and leave the previous value standing.
    // It is implemented now, so it takes effect like any other function.
    harness.valoContext.filter = "drop-shadow(2px 2px black)";
    expect(harness.valoContext.filter).toBe("drop-shadow(2px 2px black)");
    harness.valoContext.filter = "sepia(50%)";
    harness.valoContext.restore();
    expect(harness.valoContext.filter).toBe("brightness(80%) blur(2px)");
  });

  test("only synchronous readback still refuses", () => {
    // getImageData is the one deliberate refusal left: WebGPU has no
    // synchronous readback. putImageData became an upload and
    // isPointInStroke a hit test against the stroke's own triangles, so
    // both now answer instead of throwing.
    expect(() => harness.valoContext.getImageData()).toThrowError(DOMException);
    expect(harness.valoContext.isPointInStroke(0, 0)).toBe(false);
  });

  test("every pattern repetition is accepted", () => {
    // These threw until the per-axis tile modes the engine already carried
    // were wired through. The assertion is inverted rather than deleted so
    // the suite keeps a record that this was once a documented gap.
    for (const repetition of ["repeat", "repeat-x", "repeat-y", "no-repeat"] as const) {
      expect(harness.valoContext.createPattern(
        harness.valoAssets.image as never,
        repetition,
      )).toBeTruthy();
    }
  });

  test("Canvas range errors remain visible", () => {
    expect(() => harness.valoContext.arc(0, 0, -1, 0, 1)).toThrowError(DOMException);
    expect(() => harness.valoContext.ellipse(0, 0, 1, -1, 0, 0, 1)).toThrowError(DOMException);
    expect(() => harness.valoContext.arcTo(0, 0, 1, 1, -1)).toThrowError(DOMException);
    expect(() => harness.valoContext.createRadialGradient(0, 0, -1, 0, 0, 2))
      .toThrowError(DOMException);
    const gradient = harness.valoContext.createLinearGradient(0, 0, 1, 1);
    expect(() => gradient.addColorStop(2, "red")).toThrowError(DOMException);
  });
});
