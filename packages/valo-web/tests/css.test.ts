import { describe, expect, it } from "vitest";

import { parseFilter, parseFont } from "../src/css.js";

const REFERENCE = { element: 20, root: 10 };

describe("the font shorthand", () => {
  it("reads the size and the family list", () => {
    const font = parseFont("16px Fira Sans, serif");
    expect(font.size).toBe(16);
    expect(font.families).toEqual(["Fira Sans", "serif"]);
    expect(font.weight).toBe(400);
    expect(font.italic).toBe(false);
  });

  it("strips quotes from family names", () => {
    expect(parseFont(`12px "Fira Sans", 'Noto Sans'`).families).toEqual([
      "Fira Sans",
      "Noto Sans",
    ]);
  });

  it("reads style, variant, weight and width in any order", () => {
    const font = parseFont("italic small-caps 700 condensed 12px serif");
    expect(font).toMatchObject({ italic: true, smallCaps: true, weight: 700, stretch: 75 });
    const reordered = parseFont("condensed 700 small-caps italic 12px serif");
    expect(reordered).toMatchObject({ italic: true, smallCaps: true, weight: 700, stretch: 75 });
  });

  it("maps the weight keywords", () => {
    expect(parseFont("bold 12px serif").weight).toBe(700);
    expect(parseFont("normal 12px serif").weight).toBe(400);
    expect(parseFont("lighter 12px serif").weight).toBe(100);
  });

  it("treats oblique as italic", () => {
    expect(parseFont("oblique 12px serif").italic).toBe(true);
  });

  it("resolves relative units against the reference sizes", () => {
    expect(parseFont("2em serif", REFERENCE).size).toBe(40);
    expect(parseFont("2rem serif", REFERENCE).size).toBe(20);
    expect(parseFont("150% serif", REFERENCE).size).toBe(30);
    expect(parseFont("12pt serif", REFERENCE).size).toBe(16);
  });

  it("reads a line height as a multiplier", () => {
    expect(parseFont("16px/1.5 serif").lineHeight).toBe(1.5);
    expect(parseFont("16px/24px serif").lineHeight).toBe(1.5);
    expect(parseFont("16px/150% serif").lineHeight).toBe(1.5);
    expect(parseFont("16px/normal serif").lineHeight).toBeUndefined();
    expect(parseFont("16px serif").lineHeight).toBeUndefined();
  });

  it("does not mistake a numeric weight for a size", () => {
    const font = parseFont("300 14px serif");
    expect(font.weight).toBe(300);
    expect(font.size).toBe(14);
  });

  it("rejects a font with no size or no family", () => {
    expect(() => parseFont("serif")).toThrow(TypeError);
    expect(() => parseFont("12px")).toThrow(TypeError);
    expect(() => parseFont("nonsense 12px serif")).toThrow(TypeError);
  });
});

describe("the filter list", () => {
  it("reads none as an empty chain", () => {
    expect(parseFilter("none")).toEqual([]);
  });

  it("drops functions that would change nothing", () => {
    expect(parseFilter("blur(0px) opacity(100%) grayscale(0)")).toEqual([]);
  });

  it("keeps chain order", () => {
    const stages = parseFilter("blur(2px) invert(1) blur(3px)");
    expect(stages?.map((stage) => stage.type)).toEqual(["blur", "color", "blur"]);
  });

  it("reads drop-shadow offsets and halves the radius into a sigma", () => {
    expect(parseFilter("drop-shadow(4px 6px 10px)")).toEqual([
      { type: "drop-shadow", offsetX: 4, offsetY: 6, sigma: 5, color: [0, 0, 0, 1] },
    ]);
  });

  it("defaults a drop-shadow radius to zero", () => {
    expect(parseFilter("drop-shadow(-2px 3px)")).toEqual([
      { type: "drop-shadow", offsetX: -2, offsetY: 3, sigma: 0, color: [0, 0, 0, 1] },
    ]);
  });

  it("takes a drop-shadow colour before or after the lengths", () => {
    const trailing = parseFilter("drop-shadow(1px 2px 4px #ff0000)");
    const leading = parseFilter("drop-shadow(#ff0000 1px 2px 4px)");
    expect(trailing).toEqual(leading);
    expect(trailing?.[0]).toMatchObject({ color: [1, 0, 0, 1] });
  });

  it("keeps a functional colour in one piece", () => {
    const stages = parseFilter("drop-shadow(1px 1px rgba(0, 0, 255, 0.5))");
    expect(stages?.[0]).toMatchObject({ type: "drop-shadow", offsetX: 1, offsetY: 1 });
  });

  it("composes drop-shadow with other functions", () => {
    const stages = parseFilter("blur(1px) drop-shadow(2px 2px 2px black) saturate(2)");
    expect(stages?.map((stage) => stage.type)).toEqual(["blur", "drop-shadow", "color"]);
  });

  it("rejects malformed input rather than guessing", () => {
    expect(parseFilter("drop-shadow(1px)")).toBeUndefined();
    expect(parseFilter("drop-shadow(1px 2px 3px 4px)")).toBeUndefined();
    expect(parseFilter("drop-shadow(1px 2px -3px)")).toBeUndefined();
    expect(parseFilter("blur(2)")).toBeUndefined();
    expect(parseFilter("nope(1)")).toBeUndefined();
    expect(parseFilter("")).toBeUndefined();
  });
});
