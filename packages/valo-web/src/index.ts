import { createDevice, createRenderer, type Device, type MemoryReport } from "./raw.js";
import { initializeValo } from "./raw.js";
import {
  ValoCanvasRenderingContext2D,
  type ValoCanvasOptions,
} from "./canvas.js";

export * from "./canvas.js";
export * from "./color.js";
export * from "./matrix.js";
export * from "./resources.js";
export * from "./images.js";
export * from "./path2d.js";
export * from "./css.js";
export * from "./raw.js";

/**
 * One GPU device that can drive MANY canvases.
 *
 * A page of live demos wants this: a traditional 2D context carries its own
 * device and browsers cap those around 16, so a dozen animated cards is near
 * the ceiling before anything is drawn. Attaching them all to one device
 * shares the glyph atlas, the image cache and the render-target pool between
 * them, which is a much larger saving than the driver overhead.
 */
export class ValoDevice {
  readonly #device: Device;

  private constructor(device: Device) {
    this.#device = device;
  }

  /** Initialize WebAssembly and acquire one GPU device. */
  static async create(): Promise<ValoDevice> {
    await initializeValo();
    return new ValoDevice(await createDevice());
  }

  /** Give `canvas` a Canvas-shaped recorder on this device. */
  attach(
    canvas: HTMLCanvasElement,
    options: ValoCanvasOptions = {},
  ): ValoCanvasRenderingContext2D {
    return new ValoCanvasRenderingContext2D(canvas, this.#device.attach(canvas), options);
  }

  /** How many canvases are currently attached. */
  get attachedCanvases(): number {
    return this.#device.attachedCanvases;
  }

  /** What this device holds across every canvas on it. */
  memoryReport(): MemoryReport {
    return this.#device.memoryReport();
  }

  /** Release the device. Attached canvases must be discarded first. */
  free(): void {
    this.#device.free();
  }
}

/**
 * Initialize WebAssembly and attach a Canvas-shaped Valo recorder.
 *
 * This gives the canvas a device of its own. A page with several live
 * canvases should use {@link ValoDevice.create} once and
 * {@link ValoDevice.attach} per canvas instead.
 */
export async function createValoCanvas(
  canvas: HTMLCanvasElement,
  options: ValoCanvasOptions = {},
): Promise<ValoCanvasRenderingContext2D> {
  await initializeValo();
  const renderer = await createRenderer(canvas);
  return new ValoCanvasRenderingContext2D(canvas, renderer, options);
}
