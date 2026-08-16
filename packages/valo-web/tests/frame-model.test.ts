import { describe, expect, it } from "vitest";

import { clearsWholeCanvas } from "../src/canvas.js";
import { identity, type Affine } from "../src/matrix.js";

const canvas: readonly [number, number] = [300, 150];
const whole: readonly [number, number, number, number] = [0, 0, 300, 150];

describe("clearsWholeCanvas", () => {
  // This predicate is what lets a present skip restoring the persistent
  // backing. Saying `true` when the clear does NOT cover everything discards
  // pixels the canvas promised to keep, and nothing errors — the ink is just
  // gone. So every condition gets its own case.
  it("accepts a clear that covers the surface exactly or more", () => {
    expect(clearsWholeCanvas(identity, 0, whole, canvas)).toBe(true);
    expect(clearsWholeCanvas(identity, 0, [-10, -10, 400, 400], canvas)).toBe(true);
  });

  it("rejects a clear that leaves any edge uncovered", () => {
    expect(clearsWholeCanvas(identity, 0, [1, 0, 300, 150], canvas)).toBe(false);
    expect(clearsWholeCanvas(identity, 0, [0, 1, 300, 150], canvas)).toBe(false);
    expect(clearsWholeCanvas(identity, 0, [0, 0, 299, 150], canvas)).toBe(false);
    expect(clearsWholeCanvas(identity, 0, [0, 0, 300, 149], canvas)).toBe(false);
  });

  it("rejects a transformed clear, whose rectangle is not what it wipes", () => {
    const translated: Affine = [1, 0, 0, 1, 10, 0];
    const scaled: Affine = [2, 0, 0, 2, 0, 0];
    expect(clearsWholeCanvas(translated, 0, whole, canvas)).toBe(false);
    expect(clearsWholeCanvas(scaled, 0, whole, canvas)).toBe(false);
  });

  it("rejects a clipped clear, which reaches only part of the surface", () => {
    expect(clearsWholeCanvas(identity, 1, whole, canvas)).toBe(false);
  });
});
