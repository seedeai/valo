import { describe, expect, it } from "vitest";

import { ImageSourceCache, sourceState, type ValoImageSource } from "../src/images.js";

/**
 * Counts what actually reaches the GPU. Every assertion here is about upload
 * volume, which is the only reason the cache exists.
 */
class RecordingRenderer {
  uploads = 0;
  refreshes = 0;
  supportsOffscreenCanvasSource = true;
  /** The source each upload was actually handed — the OffscreenCanvas
   *  fallback is only visible here. */
  readonly copied: unknown[] = [];
  #next = 1;

  uploadExternalImage(source: unknown, width: number, height: number, mipmaps: boolean) {
    this.uploads += 1;
    this.copied.push(source);
    return { id: this.#next++, width, height, mipmaps, free() {} };
  }

  refreshExternalImage(_image: unknown, _source: unknown, _width: number, _height: number) {
    this.refreshes += 1;
    return true;
  }
}

function cacheOver(renderer: RecordingRenderer): ImageSourceCache {
  return new ImageSourceCache(renderer as unknown as ConstructorParameters<
    typeof ImageSourceCache
  >[0]);
}

/** An `<img>` that has decoded: immutable until its `src` changes. */
function imageElement(src = "a.png"): ValoImageSource {
  return { naturalWidth: 32, naturalHeight: 32, complete: true, currentSrc: src, src } as
    unknown as ValoImageSource;
}

/** A `<canvas>`: mutable, and it reports nothing when it changes. */
function canvasElement(): ValoImageSource {
  return { width: 32, height: 32, getContext: () => null } as unknown as ValoImageSource;
}

function videoElement(currentTime: number): ValoImageSource {
  return { videoWidth: 32, videoHeight: 32, readyState: 4, currentTime } as
    unknown as ValoImageSource;
}

/** An `ImageBitmap`: a size and nothing else. */
function imageBitmap(): ValoImageSource {
  return { width: 32, height: 32 } as unknown as ValoImageSource;
}

/**
 * A `VideoFrame`. Note what it does NOT have: `width`, `height`, or any of
 * the other sources' markers. Installing a constructor is what makes
 * `instanceof` work, since the classifier looks it up on `globalThis`.
 */
function videoFrame(timestamp: number, displayWidth = 32): ValoImageSource {
  const frame = Object.create(FakeVideoFrame.prototype) as Record<string, unknown>;
  frame.displayWidth = displayWidth;
  frame.displayHeight = 18;
  frame.codedWidth = 64;
  frame.codedHeight = 34;
  frame.timestamp = timestamp;
  return frame as unknown as ValoImageSource;
}

class FakeVideoFrame {}

function withVideoFrameConstructor<T>(body: () => T): T {
  const global = globalThis as { VideoFrame?: unknown };
  const original = global.VideoFrame;
  global.VideoFrame = FakeVideoFrame;
  try {
    return body();
  } finally {
    if (original === undefined) delete global.VideoFrame;
    else global.VideoFrame = original;
  }
}

describe("source classification", () => {
  it("separates the mutable sources from the immutable ones", () => {
    expect(sourceState(canvasElement())?.volatile).toBe(true);
    expect(sourceState(videoElement(0))?.volatile).toBe(true);
    expect(sourceState(imageElement())?.volatile).toBe(false);
    expect(sourceState(imageBitmap())?.volatile).toBe(false);
  });

  it("classifies a VideoFrame by its display size", () => {
    withVideoFrameConstructor(() => {
      const state = sourceState(videoFrame(1000));
      // Display, not coded: the coded size carries the codec's macroblock
      // padding, and Canvas2D draws what the frame displays as.
      expect(state).toMatchObject({ width: 32, height: 18, volatile: true });
      // Volatile keeps mips off a per-frame source; the timestamp still pins
      // the frame so redrawing the same one does not re-upload.
      expect(state?.revision).toBe("1000");
    });
  });

  it("reads a VideoFrame once however many times it is drawn", () => {
    withVideoFrameConstructor(() => {
      const renderer = new RecordingRenderer();
      const cache = cacheOver(renderer);
      const frame = videoFrame(1000);

      for (let draw = 0; draw < 5; draw += 1) cache.resolve(frame, false);
      cache.advanceFrame();
      cache.resolve(frame, false);

      expect(renderer.uploads).toBe(1);
      expect(renderer.refreshes).toBe(0);
      // One frame of a stream: mips would be built and thrown away.
      expect(renderer.copied.length).toBe(1);
    });
  });

  it("gives each VideoFrame of a stream its own upload", () => {
    withVideoFrameConstructor(() => {
      const renderer = new RecordingRenderer();
      const cache = cacheOver(renderer);
      for (const timestamp of [0, 33, 66]) {
        cache.resolve(videoFrame(timestamp), false);
        cache.advanceFrame();
      }
      expect(renderer.uploads).toBe(3);
    });
  });

  it("reports nothing to read for a source that is not ready", () => {
    const undecoded = { naturalWidth: 0, naturalHeight: 0, complete: true, src: "", currentSrc: "" };
    expect(sourceState(undecoded as unknown as ValoImageSource)).toBeUndefined();
    const seeking = { videoWidth: 32, videoHeight: 32, readyState: 0, currentTime: 0 };
    expect(sourceState(seeking as unknown as ValoImageSource)).toBeUndefined();
  });
});

describe("ImageSourceCache", () => {
  it("re-reads a canvas on every frame", () => {
    // The regression: a canvas has no revision, so a reuse rule that compared
    // revisions first saw `undefined === undefined`, reused forever, and froze
    // the source on its first frame.
    const renderer = new RecordingRenderer();
    const cache = cacheOver(renderer);
    const canvas = canvasElement();

    cache.resolve(canvas, false);
    cache.advanceFrame();
    cache.resolve(canvas, false);
    cache.advanceFrame();
    cache.resolve(canvas, false);

    expect(renderer.uploads).toBe(1);
    expect(renderer.refreshes).toBe(2);
  });

  it("reads a canvas once per frame however many times it is drawn", () => {
    const renderer = new RecordingRenderer();
    const cache = cacheOver(renderer);
    const canvas = canvasElement();

    for (let draw = 0; draw < 10; draw += 1) cache.resolve(canvas, false);

    expect(renderer.uploads).toBe(1);
    expect(renderer.refreshes).toBe(0);
  });

  it("never re-reads a decoded image or an ImageBitmap", () => {
    const renderer = new RecordingRenderer();
    const cache = cacheOver(renderer);
    const still = imageElement();
    const bitmap = imageBitmap();

    for (let frame = 0; frame < 5; frame += 1) {
      cache.resolve(still, false);
      cache.resolve(bitmap, false);
      cache.advanceFrame();
    }

    expect(renderer.uploads).toBe(2);
    expect(renderer.refreshes).toBe(0);
  });

  it("re-reads an image whose src changed", () => {
    const renderer = new RecordingRenderer();
    const cache = cacheOver(renderer);
    const element = imageElement("a.png") as { currentSrc: string; src: string };

    cache.resolve(element as unknown as ValoImageSource, false);
    element.currentSrc = "b.png";
    element.src = "b.png";
    cache.advanceFrame();
    cache.resolve(element as unknown as ValoImageSource, false);

    expect(renderer.uploads + renderer.refreshes).toBe(2);
  });

  it("leaves a paused video alone across frames", () => {
    const renderer = new RecordingRenderer();
    const cache = cacheOver(renderer);
    const video = videoElement(1.5) as { currentTime: number };

    cache.resolve(video as unknown as ValoImageSource, false);
    cache.advanceFrame();
    cache.resolve(video as unknown as ValoImageSource, false);
    expect(renderer.uploads).toBe(1);
    expect(renderer.refreshes).toBe(0);

    // Playing again advances the timestamp, and that has to be picked up.
    video.currentTime = 1.6;
    cache.advanceFrame();
    cache.resolve(video as unknown as ValoImageSource, false);
    expect(renderer.refreshes).toBe(1);
  });

  it("mints a new image rather than refreshing while earlier frames are retained", () => {
    // Refreshing in place would rewrite the texture an already-recorded draw
    // is still showing, so retained history has to buy a new one.
    const renderer = new RecordingRenderer();
    const cache = cacheOver(renderer);
    const canvas = canvasElement();

    cache.resolve(canvas, true);
    cache.advanceFrame();
    cache.resolve(canvas, true);

    expect(renderer.uploads).toBe(2);
    expect(renderer.refreshes).toBe(0);
  });

  it("blits an OffscreenCanvas through a scratch canvas when the adapter cannot copy it", () => {
    const scratch = { width: 0, height: 0, getContext: () => ({ clearRect() {}, drawImage() {} }) };
    const created: string[] = [];
    (globalThis as { document?: unknown }).document = {
      createElement(tag: string) {
        created.push(tag);
        return scratch;
      },
    };
    try {
      const renderer = new RecordingRenderer();
      renderer.supportsOffscreenCanvasSource = false;
      const cache = cacheOver(renderer);
      const offscreen = {
        width: 32,
        height: 32,
        getContext: () => null,
        transferToImageBitmap: () => null,
      } as unknown as ValoImageSource;

      cache.resolve(offscreen, false);

      expect(created).toEqual(["canvas"]);
      expect(renderer.copied[0]).toBe(scratch);
      expect(scratch.width).toBe(32);
    } finally {
      delete (globalThis as { document?: unknown }).document;
    }
  });

  it("hands an OffscreenCanvas straight through when the adapter can copy it", () => {
    const renderer = new RecordingRenderer();
    const cache = cacheOver(renderer);
    const offscreen = {
      width: 32,
      height: 32,
      getContext: () => null,
      transferToImageBitmap: () => null,
    } as unknown as ValoImageSource;

    cache.resolve(offscreen, false);
    expect(renderer.copied[0]).toBe(offscreen);
  });

  it("keeps mips off a VideoFrame", () => {
    withVideoFrameConstructor(() => {
      const renderer = new RecordingRenderer();
      const cache = cacheOver(renderer);
      expect(cache.resolve(videoFrame(1000), false)).toMatchObject({ mipmaps: false });
    });
  });

  it("asks for mips only where they will be reused", () => {
    const renderer = new RecordingRenderer();
    const cache = cacheOver(renderer);
    expect(cache.resolve(imageElement(), false)).toMatchObject({ mipmaps: true });
    expect(cache.resolve(canvasElement(), false)).toMatchObject({ mipmaps: false });
  });
});
