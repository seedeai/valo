import { describe, expect, it } from "vitest";

import { appendPathData, type PathSink } from "../src/path2d.js";

/** Records the verbs the parser emits, rounded so float noise never fails a
 *  test that is really about the grammar. */
class Recorder implements PathSink {
  readonly verbs: string[] = [];

  moveTo(x: number, y: number): void {
    this.#push("M", x, y);
  }
  lineTo(x: number, y: number): void {
    this.#push("L", x, y);
  }
  quadraticCurveTo(cx: number, cy: number, x: number, y: number): void {
    this.#push("Q", cx, cy, x, y);
  }
  bezierCurveTo(
    c1x: number,
    c1y: number,
    c2x: number,
    c2y: number,
    x: number,
    y: number,
  ): void {
    this.#push("C", c1x, c1y, c2x, c2y, x, y);
  }
  ellipse(
    cx: number,
    cy: number,
    rx: number,
    ry: number,
    rotation: number,
    start: number,
    sweep: number,
  ): void {
    this.#push("E", cx, cy, rx, ry, rotation, start, sweep);
  }
  close(): void {
    this.verbs.push("Z");
  }

  #push(verb: string, ...values: number[]): void {
    this.verbs.push(`${verb} ${values.map((v) => round(v)).join(" ")}`);
  }
}

function round(value: number): number {
  return Math.round(value * 1000) / 1000;
}

function parse(data: string): string[] {
  const recorder = new Recorder();
  appendPathData(recorder, data);
  return recorder.verbs;
}

describe("SVG path data", () => {
  it("reads absolute and relative commands", () => {
    expect(parse("M10 20 L30 40 l5 5")).toEqual(["M 10 20", "L 30 40", "L 35 45"]);
  });

  it("repeats the last command for extra coordinate pairs", () => {
    expect(parse("M0 0 L1 1 2 2 3 3")).toEqual(["M 0 0", "L 1 1", "L 2 2", "L 3 3"]);
  });

  it("rejects data that does not begin with a moveto", () => {
    // SVG Paths §9.3.3: a path data segment must begin with moveto, so the
    // whole string is an error rather than an implicit start at the origin.
    expect(parse("L10 10")).toEqual([]);
    expect(parse("Z")).toEqual([]);
    expect(parse("C1 1 2 2 3 3")).toEqual([]);
  });

  it("turns a repeated moveto into a lineto", () => {
    expect(parse("M1 1 2 2 3 3")).toEqual(["M 1 1", "L 2 2", "L 3 3"]);
    expect(parse("m1 1 2 2")).toEqual(["M 1 1", "L 3 3"]);
  });

  it("needs no separator where the tokens are already distinct", () => {
    expect(parse("M10-5L.5.5")).toEqual(["M 10 -5", "L 0.5 0.5"]);
  });

  it("accepts exponents", () => {
    expect(parse("M1e2 2E-1")).toEqual(["M 100 0.2"]);
  });

  it("applies H and V against the current point", () => {
    expect(parse("M10 10 H30 v5 h-5")).toEqual([
      "M 10 10",
      "L 30 10",
      "L 30 15",
      "L 25 15",
    ]);
  });

  it("reflects the previous control point for S", () => {
    // The new first control mirrors the previous SECOND control (5,10)
    // through the current point (10,0), giving (15,-10).
    expect(parse("M0 0 C5 0 5 10 10 0 S15 -10 20 0")).toEqual([
      "M 0 0",
      "C 5 0 5 10 10 0",
      "C 15 -10 15 -10 20 0",
    ]);
  });

  it("uses the current point when S does not follow a cubic", () => {
    expect(parse("M0 0 L10 0 S15 -10 20 0")).toEqual([
      "M 0 0",
      "L 10 0",
      "C 10 0 15 -10 20 0",
    ]);
  });

  it("reflects the previous control point for T", () => {
    expect(parse("M0 0 Q5 10 10 0 T20 0")).toEqual([
      "M 0 0",
      "Q 5 10 10 0",
      "Q 15 -10 20 0",
    ]);
  });

  it("packs arc flags without separators", () => {
    // `011 1` is large-arc 0, sweep 1, then x=1 y=1.
    const packed = parse("M0 0a1 1 0 011 1");
    const spaced = parse("M0 0 a1 1 0 0 1 1 1");
    expect(packed).toEqual(spaced);
    expect(packed[1]).toMatch(/^E /);
  });

  it("places an arc's centre between its endpoints", () => {
    // A half turn of the unit circle from (0,0) to (2,0) is centred at (1,0).
    expect(parse("M0 0 A1 1 0 0 1 2 0")).toEqual([
      "M 0 0",
      `E 1 0 1 1 0 ${round(Math.PI)} ${round(Math.PI)}`,
    ]);
  });

  it("scales up radii too small to span the endpoints", () => {
    const [, arc] = parse("M0 0 A1 1 0 0 1 4 0");
    expect(arc).toBe(`E 2 0 2 2 0 ${round(Math.PI)} ${round(Math.PI)}`);
  });

  it("degrades a zero-radius arc to a line", () => {
    expect(parse("M0 0 A0 5 0 0 1 10 0")).toEqual(["M 0 0", "L 10 0"]);
  });

  it("reopens the subpath at its start after a closepath", () => {
    expect(parse("M0 0 L10 0 Z L20 20")).toEqual([
      "M 0 0",
      "L 10 0",
      "Z",
      "M 0 0",
      "L 20 20",
    ]);
  });

  it("leaves a subpath open at the last point when the data ends closed", () => {
    // WHATWG's `new Path2D(d)` finishes by creating a subpath at the last
    // point, so a `lineTo` added afterwards runs from the seam rather than
    // starting over at its own endpoint.
    expect(parse("M0 0 L10 0 Z")).toEqual(["M 0 0", "L 10 0", "Z", "M 0 0"]);
    expect(parse("M0 0 L10 0")).toEqual(["M 0 0", "L 10 0"]);
  });

  it("keeps everything parsed before a malformed token", () => {
    expect(parse("M10 10 L20 20 L30 oops")).toEqual(["M 10 10", "L 20 20"]);
    expect(parse("Q1 2 3")).toEqual([]);
  });

  it("ignores data that does not open with a command", () => {
    expect(parse("10 20 30 40")).toEqual([]);
    expect(parse("")).toEqual([]);
  });
});
