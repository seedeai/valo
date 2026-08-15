import { createRenderer } from "./raw.js";
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

/** Initialize WebAssembly and attach a Canvas-shaped Valo recorder. */
export async function createValoCanvas(
  canvas: HTMLCanvasElement,
  options: ValoCanvasOptions = {},
): Promise<ValoCanvasRenderingContext2D> {
  await initializeValo();
  const renderer = await createRenderer(canvas);
  return new ValoCanvasRenderingContext2D(canvas, renderer, options);
}
