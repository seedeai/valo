import {
  createRenderer,
  initializeValo,
  ValoCanvasRenderingContext2D,
} from "valo-web";
import {
  CANVAS_PAIR_TEST_ID,
  CANVAS_SIZE,
  FIXTURE_FONT_FAMILY,
  replayCommands,
  type CanvasScene,
  type ReplayAssets,
  type ReplayContext,
} from "./scene.js";

export interface ConformanceHarness {
  nativeCanvas: HTMLCanvasElement;
  valoCanvas: HTMLCanvasElement;
  valoContext: ValoCanvasRenderingContext2D;
  nativeAssets: ReplayAssets;
  valoAssets: ReplayAssets;
}

export async function createConformanceHarness(): Promise<ConformanceHarness> {
  document.body.replaceChildren();
  Object.assign(document.body.style, { margin: "0", padding: "0", background: "#000" });
  const nativeCanvas = makeCanvas("native-canvas");
  const valoCanvas = makeCanvas("valo-canvas");
  document.body.append(makeCanvasPair(nativeCanvas, valoCanvas));
  await initializeValo();
  const renderer = await createRenderer(valoCanvas);
  const valoContext = new ValoCanvasRenderingContext2D(valoCanvas, renderer, {
    autoPresent: false,
  });
  await registerFixtureFont(valoContext);
  const { nativeImage, pixels } = makeFixtureImage();
  const valoImage = renderer.uploadRgba(16, 16, pixels, false, false);
  return {
    nativeCanvas,
    valoCanvas,
    valoContext,
    nativeAssets: { image: nativeImage },
    valoAssets: { image: valoImage },
  };
}

export async function renderBoth(harness: ConformanceHarness, scene: CanvasScene): Promise<void> {
  renderNative(harness.nativeCanvas, scene, harness.nativeAssets);
  renderValo(harness.valoContext, scene, harness.valoAssets);
  await settlePresentation();
}

export function renderNative(
  canvas: HTMLCanvasElement,
  scene: CanvasScene,
  assets?: ReplayAssets,
): number {
  canvas.width = CANVAS_SIZE;
  canvas.height = CANVAS_SIZE;
  const context = canvas.getContext("2d", { alpha: true, colorSpace: "srgb" });
  if (!context) throw new Error("Canvas2D is unavailable");
  context.fillStyle = scene.background;
  context.fillRect(0, 0, CANVAS_SIZE, CANVAS_SIZE);
  const start = performance.now();
  replayCommands(context as ReplayContext, scene.commands, assets);
  return performance.now() - start;
}

export function renderValo(
  context: ValoCanvasRenderingContext2D,
  scene: CanvasScene,
  assets?: ReplayAssets,
): number {
  context.reset();
  context.beginFrame(scene.background);
  const start = performance.now();
  replayCommands(context as ReplayContext, scene.commands, assets);
  context.present();
  return performance.now() - start;
}

/**
 * The two canvases sit flush in one shrink-wrapped row so the comparison can
 * capture both in a single screenshot and split it down the middle. Each
 * Playwright screenshot is a round trip that dominates the cost of a run, and
 * two of them per scene is what decides how many scenes a time budget buys.
 */
function makeCanvasPair(
  nativeCanvas: HTMLCanvasElement,
  valoCanvas: HTMLCanvasElement,
): HTMLElement {
  const pair = document.createElement("div");
  pair.dataset.testid = CANVAS_PAIR_TEST_ID;
  Object.assign(pair.style, {
    display: "flex",
    gap: "0",
    width: `${CANVAS_SIZE * 2}px`,
    height: `${CANVAS_SIZE}px`,
  });
  pair.append(nativeCanvas, valoCanvas);
  return pair;
}

function makeCanvas(testIdentifier: string): HTMLCanvasElement {
  const canvas = document.createElement("canvas");
  canvas.width = CANVAS_SIZE;
  canvas.height = CANVAS_SIZE;
  canvas.dataset.testid = testIdentifier;
  Object.assign(canvas.style, {
    width: `${CANVAS_SIZE}px`,
    height: `${CANVAS_SIZE}px`,
    display: "block",
  });
  return canvas;
}

function settlePresentation(): Promise<void> {
  return new Promise((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
  });
}

async function registerFixtureFont(context: ValoCanvasRenderingContext2D): Promise<void> {
  const response = await fetch(new URL("../../../assets/fonts/fira_sans.ttf", import.meta.url));
  if (!response.ok) throw new Error(`could not load the fixture font: ${response.status}`);
  const bytes = await response.arrayBuffer();
  const nativeFont = new FontFace(FIXTURE_FONT_FAMILY, bytes);
  await nativeFont.load();
  document.fonts.add(nativeFont);
  await context.registerFont(FIXTURE_FONT_FAMILY, bytes, true);
}

function makeFixtureImage(): { nativeImage: HTMLCanvasElement; pixels: Uint8Array } {
  const size = 16;
  const pixels = new Uint8Array(size * size * 4);
  for (let y = 0; y < size; y += 1) {
    for (let x = 0; x < size; x += 1) {
      const offset = (y * size + x) * 4;
      const alternate = ((x >> 2) + (y >> 2)) % 2 === 0;
      pixels.set(alternate ? [255, 79, 121, 255] : [83, 213, 255, 180], offset);
    }
  }
  const nativeImage = document.createElement("canvas");
  nativeImage.width = size;
  nativeImage.height = size;
  const context = nativeImage.getContext("2d");
  if (!context) throw new Error("Canvas2D is unavailable for the image fixture");
  context.putImageData(new ImageData(new Uint8ClampedArray(pixels), size, size), 0, 0);
  return { nativeImage, pixels };
}
