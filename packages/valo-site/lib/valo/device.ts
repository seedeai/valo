import {
  FontCollection,
  createDevice,
  initializeValo,
  type Device,
  type MemoryReport,
  type Renderer,
} from 'valo-web/raw';
import { ValoCanvasRenderingContext2D, type ValoCanvasOptions } from 'valo-web';

/**
 * The site's single point of contact with valo's GPU objects.
 *
 * Everything that draws on this site goes through here, and the reason is the
 * claim the landing page makes: ONE device drives every canvas on the page.
 * A traditional 2D context carries a device of its own and browsers cap those
 * around sixteen, so a grid of live demos would be at the ceiling before it
 * drew anything. Here the glyph atlas, the image cache and the render-target
 * pool live on the device, and every card shares one of each — twelve cards
 * setting the same typeface raster it once.
 *
 * Nothing in this module may be imported from a module graph that reaches the
 * server: `navigator.gpu` and WebAssembly both exist only in the browser. Its
 * callers reach it with `await import()` from an effect, which is what keeps
 * it out of server rendering.
 */

/** Where the fonts and the wasm binary are served from. Staged by `scripts/copy-valo-assets.mjs`. */
const WASM_URL = '/valo/valo_web_bg.wasm';
const FONTS: readonly { family: string; url: string; fallback: boolean }[] = [
  { family: 'Fira Sans', url: '/valo/fonts/fira_sans.ttf', fallback: true },
  { family: 'JetBrains Mono', url: '/valo/fonts/jetbrains_mono.ttf', fallback: false },
];

export const TEXT_FAMILY = 'Fira Sans';
export const MONO_FAMILY = 'JetBrains Mono';

/** The device and the font data every canvas on the page shares. */
export interface SharedRuntime {
  readonly device: Device;
  /** Registered once. Font bytes are CPU-side, the atlas they feed is on the device. */
  readonly fonts: FontCollection;
}

export type WebGpuAvailability =
  | { readonly supported: true }
  | { readonly supported: false; readonly reason: string };

/**
 * Why WebGPU is unavailable, phrased for a visitor rather than a developer.
 * Checked before any wasm loads so a card can say something useful instead of
 * showing an empty square.
 */
export function webGpuAvailability(): WebGpuAvailability {
  // Truthiness rather than `'gpu' in navigator`: some environments define the
  // property and leave it undefined, and that is still no WebGPU. Narrowed by
  // hand because `lib.dom` has no `gpu` and the site needs no other WebGPU type.
  const webgpu = (globalThis.navigator as { gpu?: unknown } | undefined)?.gpu;
  if (!webgpu) {
    return {
      supported: false,
      reason:
        'This browser has no WebGPU. valo renders through it directly, so the live demos need Chrome 113+, Edge 113+, or Safari 18+.',
    };
  }
  if (!globalThis.isSecureContext) {
    return {
      supported: false,
      reason: 'WebGPU is only exposed in a secure context. Load this page over HTTPS or localhost.',
    };
  }
  return { supported: true };
}

let runtime: Promise<SharedRuntime> | undefined;

/**
 * The one device, created on first use and reused for the life of the page.
 *
 * Memoised on the promise rather than the result so that a grid whose cards
 * all mount in the same tick acquires exactly one adapter between them.
 */
export function sharedRuntime(): Promise<SharedRuntime> {
  runtime ??= createRuntime();
  return runtime;
}

async function createRuntime(): Promise<SharedRuntime> {
  const availability = webGpuAvailability();
  if (!availability.supported) throw new Error(availability.reason);
  await initializeValo({ module_or_path: WASM_URL });
  return { device: await acquireDevice(), fonts: await registerFonts() };
}

/**
 * `navigator.gpu` existing does not mean an adapter does — a blocklisted
 * driver, a headless session or a software fallback all fail here. The wgpu
 * error is precise and unreadable, so it is restated and kept.
 */
async function acquireDevice(): Promise<Device> {
  try {
    return await createDevice();
  } catch (cause) {
    const detail = cause instanceof Error ? cause.message : String(cause);
    throw new Error(
      `This browser exposes WebGPU but gave us no GPU adapter, so valo has nothing to draw on. ` +
        `That is usually a blocklisted driver or a headless session.\n\n${detail}`,
    );
  }
}

async function registerFonts(): Promise<FontCollection> {
  const fonts = new FontCollection();
  const loaded = await Promise.all(
    FONTS.map(async (font) => ({
      ...font,
      bytes: new Uint8Array(await (await fetch(font.url)).arrayBuffer()),
    })),
  );
  for (const font of loaded) fonts.registerFont(font.family, font.bytes, font.fallback);
  return fonts;
}

/** Give `canvas` a renderer on the shared device. Only a swapchain and a backing are allocated. */
export async function attachRenderer(canvas: HTMLCanvasElement): Promise<Renderer> {
  const { device } = await sharedRuntime();
  return device.attach(canvas);
}

/**
 * The same shared device, seen through the Canvas2D compatibility layer.
 *
 * `ValoDevice.create()` would acquire a SECOND device, which is precisely what
 * this page is arguing against, so the shim is constructed over a renderer from
 * the device already in hand. The compatibility card and the engine cards then
 * sit on one device and show up in one memory report.
 */
export async function attachCanvas2D(
  canvas: HTMLCanvasElement,
  options: ValoCanvasOptions = {},
): Promise<ValoCanvasRenderingContext2D> {
  const renderer = await attachRenderer(canvas);
  return new ValoCanvasRenderingContext2D(canvas, renderer, options);
}

/** What the whole page costs the GPU, across every canvas attached. */
export async function memoryReport(): Promise<{ report: MemoryReport; canvases: number }> {
  const { device } = await sharedRuntime();
  return { report: device.memoryReport(), canvases: device.attachedCanvases };
}
