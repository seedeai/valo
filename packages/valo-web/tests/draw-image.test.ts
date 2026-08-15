import { describe, expect, it } from "vitest";

import { imageArguments } from "../src/canvas.js";
import type { Image } from "../src/raw.js";

/** Only the dimensions matter to the rectangle rules. */
const image = { width: 100, height: 50 } as unknown as Image;

describe("drawImage rectangles", () => {
  it("fills in the whole image for the short forms", () => {
    expect(imageArguments(image, [10, 20])).toEqual([0, 0, 100, 50, 10, 20, 100, 50]);
    expect(imageArguments(image, [10, 20, 200, 100])).toEqual([0, 0, 100, 50, 10, 20, 200, 100]);
  });

  it("flips a negative destination instead of dropping the draw", () => {
    // Valo's Rect treats a negative extent as empty, so without normalizing
    // this the draw would silently disappear rather than mirror.
    expect(imageArguments(image, [40, 40, -40, -40])).toEqual([0, 0, 100, 50, 0, 0, 40, 40]);
    expect(imageArguments(image, [0, 0, 100, 50, 40, 40, -40, -40])).toEqual([
      0, 0, 100, 50, 0, 0, 40, 40,
    ]);
  });

  it("flips a negative source rectangle too", () => {
    expect(imageArguments(image, [100, 50, -100, -50, 0, 0, 100, 50])).toEqual([
      0, 0, 100, 50, 0, 0, 100, 50,
    ]);
  });

  it("clips a source rectangle to the image and the destination in proportion", () => {
    // Half the source rectangle hangs off the right edge, so half the
    // destination width goes with it and the visible half keeps its place.
    expect(imageArguments(image, [50, 0, 100, 50, 0, 0, 200, 100])).toEqual([
      50, 0, 50, 50, 0, 0, 100, 100,
    ]);
    // Overhang on the near side moves the destination origin instead.
    expect(imageArguments(image, [-50, 0, 100, 50, 0, 0, 200, 100])).toEqual([
      0, 0, 50, 50, 100, 0, 100, 100,
    ]);
  });

  it("draws nothing when the source rectangle misses the image entirely", () => {
    expect(imageArguments(image, [200, 0, 50, 50, 0, 0, 50, 50])).toBeUndefined();
    expect(imageArguments(image, [0, 0, 0, 50, 0, 0, 50, 50])).toBeUndefined();
  });
});
