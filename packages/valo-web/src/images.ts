import { Image, type Renderer } from "./raw.js";

/**
 * Everything `drawImage` and `createPattern` accept: an already-uploaded Valo
 * `Image`, or any DOM source WebGPU can copy from.
 *
 * DIVERGENCE — cross-origin sources. Canvas2D draws a non-origin-clean image
 * and marks the destination tainted, which then makes `getImageData` throw.
 * WebGPU refuses at the copy instead (`SecurityError`, WebGPU §3.9), so a
 * cross-origin source fails here rather than drawing. Nothing in this shim can
 * bridge that: the pixels never reach the GPU. Tainting would in any case be
 * unobservable, since `getImageData` already refuses for every image. Serve
 * such images same-origin, or with CORS and `crossOrigin` set.
 */
export type ValoImageSource =
  | Image
  | HTMLImageElement
  | HTMLCanvasElement
  | HTMLVideoElement
  | ImageBitmap
  | OffscreenCanvas
  | VideoFrame;

/** What a source looks like right now, and how to tell when it changes. */
interface SourceState {
  width: number;
  height: number;
  /**
   * A value that changes exactly when the pixels can have changed.
   * `undefined` means the source offers no such signal — which, combined
   * with `volatile`, is the difference between "immutable, never re-read"
   * and "mutable and silent, re-read every frame".
   */
  revision: string | undefined;
  /** Re-read every frame; also suppresses mip generation. */
  volatile: boolean;
}

interface CacheEntry {
  image: Image;
  width: number;
  height: number;
  revision: string | undefined;
  /** The frame this entry was last brought up to date on. */
  frame: number;
}

/**
 * One GPU texture per DOM source, kept current.
 *
 * The whole performance story of DOM image sources lives here: a `<img>` must
 * upload once and never again, while a `<video>` must upload once per FRAME
 * however many times it is drawn. Uploading per draw would make this feature
 * slower than the uploaded-`Image` path it exists to replace.
 */
export class ImageSourceCache {
  readonly #renderer: Renderer;
  readonly #entries = new WeakMap<object, CacheEntry>();
  #scratch: HTMLCanvasElement | undefined;
  #frame = 0;

  constructor(renderer: Renderer) {
    this.#renderer = renderer;
  }

  /** Start a new frame: volatile sources become stale again. */
  advanceFrame(): void {
    this.#frame += 1;
  }

  /**
   * The Valo image for `source`, or `undefined` when the source is not ready
   * to be read — an `<img>` that has not decoded and a `<video>` with no
   * current frame are both silent no-ops in Canvas2D, and copying from them
   * would throw.
   *
   * `retained` says whether display lists from EARLIER frames may still
   * reference the previous texture. When they cannot, a volatile source is
   * refreshed in place, which keeps its texture and the renderer's cached
   * bind group; when they can, it gets a fresh image so those earlier draws
   * keep the pixels they were recorded with.
   */
  resolve(source: ValoImageSource, retained: boolean): Image | undefined {
    if (source instanceof Image) return source;
    const state = sourceState(source);
    if (!state) return undefined;

    const entry = this.#entries.get(source);
    if (entry && this.#reuse(entry, state, source, retained)) return entry.image;

    const image = this.#renderer.uploadExternalImage(
      this.#copyable(source, state),
      state.width,
      state.height,
      !state.volatile,
    );
    entry?.image.free();
    this.#entries.set(source, {
      image,
      width: state.width,
      height: state.height,
      revision: state.revision,
      frame: this.#frame,
    });
    return image;
  }

  /** `true` when `entry` now holds `source`'s current pixels. */
  #reuse(
    entry: CacheEntry,
    state: SourceState,
    source: ValoImageSource,
    retained: boolean,
  ): boolean {
    if (entry.width !== state.width || entry.height !== state.height) return false;
    if (!state.volatile) {
      // A non-volatile source can only change when its revision does, so
      // matching revisions — including an `ImageBitmap`'s pair of undefineds,
      // since it is immutable — mean the texture is still current.
      return entry.revision === state.revision;
    }
    // A volatile source may have changed without saying so. A defined
    // revision that still matches rules that out (a paused `<video>` on the
    // same timestamp); otherwise the texture is only trustworthy if it was
    // already read during THIS frame, because nothing can change between two
    // draws in one frame.
    if (state.revision !== undefined && entry.revision === state.revision) return true;
    if (entry.frame === this.#frame) return true;
    // Stale, and an earlier frame's display list may still be showing the
    // texture — those draws keep the pixels they were recorded with, so this
    // needs a new image rather than an in-place refresh.
    if (retained) return false;
    const copyable = this.#copyable(source, state);
    if (!this.#renderer.refreshExternalImage(entry.image, copyable, state.width, state.height)) {
      return false;
    }
    entry.revision = state.revision;
    entry.frame = this.#frame;
    return true;
  }

  /**
   * The source to hand WebGPU, which is usually the caller's own.
   *
   * An `OffscreenCanvas` needs `UNRESTRICTED_EXTERNAL_TEXTURE_COPIES`, and an
   * adapter without it rejects the copy outright. Blitting through a scratch
   * `<canvas>` degrades instead of failing — synchronously, and without
   * consuming the caller's canvas the way `transferToImageBitmap` would.
   *
   * In a worker there is no `<canvas>` to blit through, so the source is
   * passed on unchanged and the renderer's own error explains the situation.
   */
  #copyable(source: ValoImageSource, state: SourceState): ValoImageSource {
    if (this.#renderer.supportsOffscreenCanvasSource) return source;
    if (!("transferToImageBitmap" in source)) return source;
    const scratch = this.#scratchCanvas(state.width, state.height);
    if (!scratch) return source;
    const context = scratch.getContext("2d");
    if (!context) return source;
    context.clearRect(0, 0, state.width, state.height);
    context.drawImage(source as OffscreenCanvas, 0, 0);
    return scratch;
  }

  #scratchCanvas(width: number, height: number): HTMLCanvasElement | undefined {
    if (typeof document === "undefined") return undefined;
    const scratch = (this.#scratch ??= document.createElement("canvas"));
    if (scratch.width !== width) scratch.width = width;
    if (scratch.height !== height) scratch.height = height;
    return scratch;
  }
}

/**
 * The CONSTRUCTOR is looked up on `globalThis` rather than named directly:
 * WebCodecs is not in every browser, so `VideoFrame` may be absent at runtime
 * even where the type exists at compile time.
 *
 * The check has to come BEFORE the width/height fallback in `sourceState`. A
 * `VideoFrame` has `displayWidth`/`displayHeight` and no `width` at all, so
 * the fallback would read `undefined` for both and sail straight past its
 * zero check.
 */
function isVideoFrame(source: object): source is VideoFrame {
  const constructor = (globalThis as { VideoFrame?: Function }).VideoFrame;
  return typeof constructor === "function" && source instanceof constructor;
}

/**
 * How a source reports its size and its changes. `undefined` = it has no
 * pixels to read yet.
 *
 * Sources are told apart by SHAPE rather than by `instanceof`. An `<img>`
 * belonging to an iframe fails `instanceof HTMLImageElement` against the
 * parent realm's constructor — a real case for anything handed across a realm
 * boundary — and misclassifying a source here is how it ends up frozen or
 * re-uploaded per draw. Duck-typing also lets this be unit-tested with no DOM.
 */
export function sourceState(source: ValoImageSource): SourceState | undefined {
  if ("naturalWidth" in source) {
    const element = source as HTMLImageElement;
    // Canvas2D draws nothing for an undecoded image; copyExternalImageToTexture
    // throws for one. `complete` alone is not enough — it is also true for an
    // image that failed to load.
    if (!element.complete || element.naturalWidth === 0 || element.naturalHeight === 0) {
      return undefined;
    }
    return {
      width: element.naturalWidth,
      height: element.naturalHeight,
      revision: element.currentSrc || element.src,
      volatile: false,
    };
  }
  if ("videoWidth" in source) {
    const element = source as HTMLVideoElement;
    // HAVE_CURRENT_DATA: there is a frame at the current position.
    if (element.readyState < 2 || element.videoWidth === 0 || element.videoHeight === 0) {
      return undefined;
    }
    return {
      width: element.videoWidth,
      height: element.videoHeight,
      // A paused video keeps showing one frame, and its time says so.
      revision: String(element.currentTime),
      volatile: true,
    };
  }
  if (isVideoFrame(source)) {
    const frame = source;
    // DISPLAY dimensions, not coded: a frame's coded size carries the codec's
    // macroblock padding and its own aspect correction, and Canvas2D draws
    // what the frame displays as.
    if (frame.displayWidth === 0 || frame.displayHeight === 0) return undefined;
    return {
      width: frame.displayWidth,
      height: frame.displayHeight,
      // Volatile even though a decoded frame's pixels never change: this is
      // one frame of a stream, so mips would be built and thrown away. The
      // timestamp still pins it, so the same frame object drawn repeatedly is
      // read once rather than once per canvas frame.
      revision: String(frame.timestamp),
      volatile: true,
    };
  }
  const { width, height } = source;
  if (width === 0 || height === 0) return undefined;
  // A canvas can be drawn into at any moment and reports nothing about it, so
  // it has to be re-read every frame. An ImageBitmap is immutable — it has no
  // rendering context to be drawn into — and never does.
  return {
    width,
    height,
    revision: undefined,
    volatile: "getContext" in source,
  };
}

