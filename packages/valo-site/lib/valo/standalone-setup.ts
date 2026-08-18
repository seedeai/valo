import {
  DisplayListBuilder,
  FontCollection,
  createDevice,
  initializeValo,
  type Renderer,
} from 'valo-web/raw';

/**
 * The smallest complete valo bootstrap, and the playground shows this file.
 *
 * It is real, compiled code rather than a prose sample: `tsc` checks it with
 * everything else, so a scene written against it in the editor is written
 * against an API that exists. The site's own driver in `surface.ts` is this
 * plus the parts a page of live cards needs — one device shared by every
 * canvas, an intersection observer, and a single frame loop for all of them.
 *
 * Read top to bottom it is the whole picture: load the wasm, take a device,
 * register fonts, attach a canvas, then each frame record a display list and
 * hand it to the renderer.
 */

/** What a scene is handed for one frame. */
export interface Scene {
  /**
   * Already scaled by the device-pixel ratio, so a scene records in CSS
   * pixels and never multiplies by `devicePixelRatio` itself.
   */
  readonly builder: DisplayListBuilder;
  readonly width: number;
  readonly height: number;
  /** Seconds since this scene started. */
  readonly time: number;
  /** Registered once per device. Every canvas on it shares the glyph atlas. */
  readonly fonts: FontCollection;
  /** This canvas's renderer. Upload images here; recording still uses `builder`. */
  readonly renderer: Renderer;
}

/**
 * A scene module: one function per frame, and an optional teardown.
 *
 * `load` runs once after the module is installed, before the first draw that
 * depends on it. Use it to fetch and upload images. `dispose` is where
 * anything built to outlive a frame gets released — valo's objects own memory
 * on the wasm side, so whoever keeps one owes it a `free()`.
 */
export interface SceneModule {
  default(scene: Scene): void;
  load?(scene: Pick<Scene, 'renderer'>): void | Promise<void>;
  dispose?(): void;
}

export interface StandaloneScene {
  stop(): void;
}

/** Draw `module` into `canvas` until `stop()`. */
export async function runScene(
  canvas: HTMLCanvasElement,
  module: SceneModule,
  fontUrls: Readonly<Record<string, string>> = {},
): Promise<StandaloneScene> {
  await initializeValo();

  // One device per page, not per canvas: the glyph atlas, the image cache and
  // the render-target pool all live on it, and `device.attach` allocates only
  // this canvas's swapchain and backing.
  const device = await createDevice();
  const renderer: Renderer = device.attach(canvas);

  const fonts = new FontCollection();
  for (const [family, url] of Object.entries(fontUrls)) {
    const bytes = new Uint8Array(await (await fetch(url)).arrayBuffer());
    fonts.registerFont(family, bytes, true);
  }

  await module.load?.({ renderer });

  const origin = performance.now();
  let frame = 0;

  function draw(now: number) {
    const ratio = Math.min(globalThis.devicePixelRatio || 1, 2);
    const box = canvas.getBoundingClientRect();
    const width = Math.max(1, Math.round(box.width * ratio));
    const height = Math.max(1, Math.round(box.height * ratio));
    if (width !== canvas.width || height !== canvas.height) {
      canvas.width = width;
      canvas.height = height;
      renderer.resize(width, height);
    }

    // Recording is CPU-only: a builder collects operations, `build()` freezes
    // them into a display list, and the renderer plans that list into passes.
    const builder = new DisplayListBuilder();
    builder.scale(ratio, ratio);
    module.default({
      builder,
      width: box.width,
      height: box.height,
      time: (now - origin) / 1000,
      fonts,
      renderer,
    });
    const list = builder.build();

    // `true` discards what was on the canvas first. Pass `false` and the frame
    // composites onto the previous one instead.
    renderer.render(list, true, 0.043, 0.043, 0.055, 1)?.free();

    list.free();
    builder.free();
    frame = requestAnimationFrame(draw);
  }

  frame = requestAnimationFrame(draw);

  return {
    stop() {
      cancelAnimationFrame(frame);
      module.dispose?.();
      // Releases this canvas's swapchain and backing. The device outlives it,
      // and so do the caches every other canvas is still using.
      renderer.free();
      fonts.free();
    },
  };
}
