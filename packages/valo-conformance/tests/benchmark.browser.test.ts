import { beforeAll, expect, test } from "vitest";
import { makeBenchmarkScene } from "../src/benchmark-scene.js";
import {
  createConformanceHarness,
  type ConformanceHarness,
} from "../src/harness.js";
import {
  CANVAS_SIZE,
  replayCommands,
  type CanvasScene,
  type ReplayContext,
} from "../src/scene.js";

const warmupFrames = 100;
const framesPerSample = 50;
const sampleCount = 30;

let harness: ConformanceHarness;

beforeAll(async () => {
  harness = await createConformanceHarness();
});

test("reports Canvas2D and Valo frame cost", async () => {
  const scene = makeBenchmarkScene();
  const nativeContext = nativeContextOf(harness.nativeCanvas);
  measureNativeFrames(nativeContext, scene, warmupFrames);
  measureValoFrames(harness, scene, warmupFrames);
  await nextAnimationFrame();

  const nativeSamples: number[] = [];
  const valoSamples: number[] = [];
  for (let sample = 0; sample < sampleCount; sample += 1) {
    if (sample % 2 === 0) {
      nativeSamples.push(measureNativeFrames(nativeContext, scene, framesPerSample));
      valoSamples.push(measureValoFrames(harness, scene, framesPerSample));
    } else {
      valoSamples.push(measureValoFrames(harness, scene, framesPerSample));
      nativeSamples.push(measureNativeFrames(nativeContext, scene, framesPerSample));
    }
    await nextAnimationFrame();
  }

  const native = summarize(nativeSamples);
  const valo = summarize(valoSamples);
  console.info(formatSummary("Canvas2D", native));
  console.info(formatSummary("Valo", valo));
  console.info(
    `${scene.commands.length} commands/frame; ${sampleCount} samples × ${framesPerSample} frames.`,
  );
  console.info("Times include frame reset, recording, and submission; they do not wait for GPU completion.");

  expect(Number.isFinite(native.medianMilliseconds)).toBe(true);
  expect(Number.isFinite(valo.medianMilliseconds)).toBe(true);
});

function nativeContextOf(canvas: HTMLCanvasElement): CanvasRenderingContext2D {
  const context = canvas.getContext("2d", { alpha: true, colorSpace: "srgb" });
  if (!context) throw new Error("Canvas2D is unavailable");
  return context;
}

function measureNativeFrames(
  context: CanvasRenderingContext2D,
  scene: CanvasScene,
  frameCount: number,
): number {
  const start = performance.now();
  for (let frame = 0; frame < frameCount; frame += 1) {
    context.reset();
    context.fillStyle = scene.background;
    context.fillRect(0, 0, CANVAS_SIZE, CANVAS_SIZE);
    replayCommands(context as ReplayContext, scene.commands);
  }
  return (performance.now() - start) / frameCount;
}

function measureValoFrames(
  currentHarness: ConformanceHarness,
  scene: CanvasScene,
  frameCount: number,
): number {
  const context = currentHarness.valoContext;
  const start = performance.now();
  for (let frame = 0; frame < frameCount; frame += 1) {
    context.reset();
    context.beginFrame(scene.background);
    replayCommands(context as ReplayContext, scene.commands);
    context.present();
  }
  return (performance.now() - start) / frameCount;
}

function nextAnimationFrame(): Promise<void> {
  return new Promise((resolve) => requestAnimationFrame(() => resolve()));
}

interface TimingSummary {
  medianMilliseconds: number;
  p95Milliseconds: number;
  meanMilliseconds: number;
}

function summarize(samples: readonly number[]): TimingSummary {
  const sorted = [...samples].sort((left, right) => left - right);
  return {
    medianMilliseconds: percentile(sorted, 0.5),
    p95Milliseconds: percentile(sorted, 0.95),
    meanMilliseconds: samples.reduce((sum, sample) => sum + sample, 0) / samples.length,
  };
}

function percentile(sortedSamples: readonly number[], percentileValue: number): number {
  const index = Math.min(
    sortedSamples.length - 1,
    Math.floor(sortedSamples.length * percentileValue),
  );
  return sortedSamples[index] ?? Number.NaN;
}

function formatSummary(label: string, summary: TimingSummary): string {
  return [
    `${label}:`,
    `median ${summary.medianMilliseconds.toFixed(3)} ms`,
    `p95 ${summary.p95Milliseconds.toFixed(3)} ms`,
    `mean ${summary.meanMilliseconds.toFixed(3)} ms`,
  ].join(" ");
}
