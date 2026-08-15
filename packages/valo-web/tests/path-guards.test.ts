import { describe, expect, it } from "vitest";

import { canvasPath, type CanvasPathSink } from "../src/path2d.js";

/**
 * `affineOf` goes through `DOMMatrix.fromMatrix`, which node has no
 * implementation of. This stand-in resolves a `DOMMatrix2DInit` the way the
 * real one does — the 2D `a`–`f` names, with their `m11`–`m42` aliases — which
 * is all these tests exercise.
 */
(globalThis as { DOMMatrix?: unknown }).DOMMatrix ??= {
  fromMatrix(init: Record<string, number | undefined> = {}) {
    return {
      a: init.a ?? init.m11 ?? 1,
      b: init.b ?? init.m12 ?? 0,
      c: init.c ?? init.m21 ?? 0,
      d: init.d ?? init.m22 ?? 1,
      e: init.e ?? init.m41 ?? 0,
      f: init.f ?? init.m42 ?? 0,
    };
  },
};

/** Records the verbs that survive the guards. */
class Recorder implements CanvasPathSink {
  readonly calls: string[] = [];
  moveTo() { this.calls.push("moveTo"); }
  lineTo() { this.calls.push("lineTo"); }
  quadraticCurveTo() { this.calls.push("quadraticCurveTo"); }
  bezierCurveTo() { this.calls.push("bezierCurveTo"); }
  ellipse() { this.calls.push("ellipse"); }
  arc() { this.calls.push("arc"); }
  arcTo() { this.calls.push("arcTo"); }
  rect() { this.calls.push("rect"); }
  roundRect() { this.calls.push("roundRect"); }
  addPath() { this.calls.push("addPath"); }
  close() { this.calls.push("close"); }
}

/**
 * Each method with a VALID argument list. Substituting a non-finite value at
 * any one position must suppress the call entirely — which is how a forgotten
 * argument in a guard gets caught, rather than only the obvious first one.
 */
const methods: Array<[keyof typeof canvasPath, number[]]> = [
  ["moveTo", [1, 2]],
  ["lineTo", [1, 2]],
  ["quadraticCurveTo", [1, 2, 3, 4]],
  ["bezierCurveTo", [1, 2, 3, 4, 5, 6]],
  ["arcTo", [1, 2, 3, 4, 5]],
  ["arc", [1, 2, 3, 4, 5]],
  ["ellipse", [1, 2, 3, 4, 5, 6, 7]],
  ["rect", [1, 2, 3, 4]],
  ["roundRect", [1, 2, 3, 4, 5]],
];

const invoke = (name: keyof typeof canvasPath, args: number[]): Recorder => {
  const sink = new Recorder();
  // `arc`/`ellipse` take a trailing boolean; the extra argument is harmless
  // for the others because each reads only the parameters it declares.
  (canvasPath[name] as (sink: CanvasPathSink, ...rest: unknown[]) => void)(sink, ...args, false);
  return sink;
};

describe("Canvas path argument rules", () => {
  it.each(methods)("%s emits its verb for finite arguments", (name, args) => {
    expect(invoke(name, args).calls).toEqual([name]);
  });

  it.each(methods)("%s ignores a non-finite argument in any position", (name, args) => {
    for (let position = 0; position < args.length; position += 1) {
      for (const bad of [Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY]) {
        const poisoned = [...args];
        poisoned[position] = bad;
        expect(
          invoke(name, poisoned).calls,
          `${name} accepted ${bad} at argument ${position}`,
        ).toEqual([]);
      }
    }
  });

  it("ignores addPath when the transform is not finite", () => {
    // The other route for NaN into retained path geometry: `addPath` takes a
    // DOMMatrix2DInit, and WHATWG returns early if any of m11/m12/m21/m22/
    // m41/m42 is infinite or NaN.
    const source = {} as never;
    for (const key of ["a", "b", "c", "d", "e", "f"] as const) {
      for (const bad of [Number.NaN, Number.POSITIVE_INFINITY]) {
        const sink = new Recorder();
        canvasPath.addPath(sink, source, { a: 1, b: 0, c: 0, d: 1, e: 0, f: 0, [key]: bad });
        expect(sink.calls, `addPath accepted ${bad} for ${key}`).toEqual([]);
      }
    }

    // A finite transform, and no transform at all, both go through.
    const finite = new Recorder();
    canvasPath.addPath(finite, source, { a: 2, b: 0, c: 0, d: 2, e: 5, f: 5 });
    expect(finite.calls).toEqual(["addPath"]);

    const identity = new Recorder();
    canvasPath.addPath(identity, source);
    expect(identity.calls).toEqual(["addPath"]);
  });

  it("checks finiteness before raising a radius error", () => {
    // WHATWG orders the two: a non-finite call returns quietly even when the
    // radius is also invalid, so this must not throw.
    expect(() => canvasPath.arcTo(new Recorder(), Number.NaN, 0, 0, 0, -1)).not.toThrow();
    expect(() => canvasPath.arc(new Recorder(), Number.NaN, 0, -1, 0, 0, false)).not.toThrow();
    expect(() =>
      canvasPath.ellipse(new Recorder(), Number.NaN, 0, -1, -1, 0, 0, 0, false),
    ).not.toThrow();

    // Finite and invalid still throws.
    expect(() => canvasPath.arcTo(new Recorder(), 0, 0, 0, 0, -1)).toThrow(DOMException);
    expect(() => canvasPath.arc(new Recorder(), 0, 0, -1, 0, 0, false)).toThrow(DOMException);
    expect(() => canvasPath.ellipse(new Recorder(), 0, 0, -1, 1, 0, 0, 0, false)).toThrow(
      DOMException,
    );
  });
});
