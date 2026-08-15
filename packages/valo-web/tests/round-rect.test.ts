import { describe, expect, it } from "vitest";

import { roundRectGeometry } from "../src/path2d.js";

/** Corner radii come back in upper-left, upper-right, lower-right,
 *  lower-left order. */
const corners = (result: ReturnType<typeof roundRectGeometry>) => [...result!.radii];
const box = (result: ReturnType<typeof roundRectGeometry>) => [
  result!.x,
  result!.y,
  result!.width,
  result!.height,
];

describe("roundRect geometry", () => {
  it("expands the CSS radius shorthand across the four corners", () => {
    expect(corners(roundRectGeometry(0, 0, 10, 10, 4))).toEqual([4, 4, 4, 4]);
    expect(corners(roundRectGeometry(0, 0, 10, 10, [1, 2]))).toEqual([1, 2, 1, 2]);
    expect(corners(roundRectGeometry(0, 0, 10, 10, [1, 2, 3]))).toEqual([1, 2, 3, 2]);
    expect(corners(roundRectGeometry(0, 0, 10, 10, [1, 2, 3, 4]))).toEqual([1, 2, 3, 4]);
  });

  it("carries the radii through a horizontal flip", () => {
    // Negative width mirrors the box, so the LEFT corners' radii have to end
    // up on the right. Normalising the box alone rounds the wrong corners.
    const flipped = roundRectGeometry(10, 0, -10, 10, [1, 2, 3, 4]);
    expect(box(flipped)).toEqual([0, 0, 10, 10]);
    expect(corners(flipped)).toEqual([2, 1, 4, 3]);
  });

  it("carries the radii through a vertical flip", () => {
    const flipped = roundRectGeometry(0, 10, 10, -10, [1, 2, 3, 4]);
    expect(box(flipped)).toEqual([0, 0, 10, 10]);
    expect(corners(flipped)).toEqual([4, 3, 2, 1]);
  });

  it("carries the radii through both flips at once", () => {
    const flipped = roundRectGeometry(10, 10, -10, -10, [1, 2, 3, 4]);
    expect(box(flipped)).toEqual([0, 0, 10, 10]);
    expect(corners(flipped)).toEqual([3, 4, 1, 2]);
  });

  it("reverses the winding when exactly one extent is negative", () => {
    // WHATWG: "If w and h are both greater than or equal to 0, or if both are
    // smaller than 0, then the path is drawn clockwise. Otherwise, it is
    // drawn counterclockwise." Under the non-zero rule that decides whether a
    // second rectangle adds to an overlapping one or cancels it, which is the
    // property Chrome exhibits — so normalizing the box has to carry the bit.
    const wound = (width: number, height: number) =>
      roundRectGeometry(0, 0, width, height, 4)!.counterclockwise;
    expect(wound(10, 10)).toBe(false);
    expect(wound(-10, -10)).toBe(false);
    expect(wound(-10, 10)).toBe(true);
    expect(wound(10, -10)).toBe(true);
  });

  it("treats a zero extent as non-negative for the winding rule", () => {
    // The spec says "greater than or equal to 0", so a zero pairs with a
    // positive and opposes a negative.
    const wound = (width: number, height: number) =>
      roundRectGeometry(0, 0, width, height, 0)!.counterclockwise;
    expect(wound(0, 10)).toBe(false);
    expect(wound(0, -10)).toBe(true);
  });

  it("returns nothing for a non-finite box or radius", () => {
    expect(roundRectGeometry(Number.NaN, 0, 10, 10, 2)).toBeUndefined();
    expect(roundRectGeometry(0, 0, Number.POSITIVE_INFINITY, 10, 2)).toBeUndefined();
    // A non-finite RADIUS is a silent no-op, unlike a negative one.
    expect(roundRectGeometry(0, 0, 10, 10, Number.NaN)).toBeUndefined();
  });

  it("throws for a negative radius or a wrong-length list", () => {
    expect(() => roundRectGeometry(0, 0, 10, 10, -1)).toThrow(RangeError);
    expect(() => roundRectGeometry(0, 0, 10, 10, [1, 2, 3, 4, 5])).toThrow(RangeError);
    expect(() => roundRectGeometry(0, 0, 10, 10, [])).toThrow(RangeError);
  });
});
