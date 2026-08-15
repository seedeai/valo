import { describe, expect, it } from "vitest";

import { parseColor } from "../src/color.js";
import { parseFilter, parseFont, parsePixels } from "../src/css.js";

/**
 * Canvas2D ignores an unparseable style value; it never throws from a setter.
 * The parsers behind those setters DO throw, because their other callers are
 * specified to — so this pins down which inputs they reject, which is what the
 * setters turn into silence.
 */
describe("style value validation", () => {
  it("rejects the values a setter has to ignore", () => {
    expect(() => parseColor("chartreuse-ish")).toThrow();
    expect(() => parseColor(String(null))).toThrow();
    expect(() => parseFont("12")).toThrow();
    expect(() => parseFont(String(undefined))).toThrow();
    expect(() => parsePixels("3em")).toThrow();
    expect(() => parsePixels(String(null))).toThrow();
  });

  it("accepts the values a setter has to keep", () => {
    expect(() => parseColor("#ff0000")).not.toThrow();
    expect(() => parseFont("italic bold 12px sans-serif")).not.toThrow();
    expect(() => parsePixels("-2px")).not.toThrow();
  });

  it("reports an unusable filter rather than throwing", () => {
    // `filter` is the one that already signalled by return value; the empty
    // string and the stringified nullish values all have to land here.
    expect(parseFilter("")).toBeUndefined();
    expect(parseFilter(String(null))).toBeUndefined();
    expect(parseFilter(String(undefined))).toBeUndefined();
    expect(parseFilter("not-a-filter(1)")).toBeUndefined();
    expect(parseFilter("none")).toEqual([]);
  });
});
