import type { Renderer } from '@valo/web/raw';
import { attachRenderer, sharedRuntime } from './device';
import { valoModule, type Scene, type SceneModule } from './scene';

/**
 * One canvas's lifetime on the shared device, and the single frame loop that
 * drives every one of them.
 *
 * A page of live cards each running its own `requestAnimationFrame` would tick
 * once per card per frame and redraw cards nobody is looking at. One driver
 * ticks once, skips what is off screen, skips what is not animating, and stops
 * entirely when the tab is hidden — which matters more here than usual,
 * because all of it lands on a single GPU device.
 */

export interface SurfaceStats {
  readonly draws: number;
  readonly drawCalls: number;
  readonly renderPasses: number;
  readonly cpuMilliseconds: number;
}

export interface MountOptions {
  /** Cleared to this every frame, before the scene records anything. */
  readonly clearColor?: string;
  /**
   * Whether to keep drawing after the first frame.
   *
   * A card starts `false`: it paints once so it shows real output rather than
   * a black square, and animates only while a visitor is pointing at it.
   */
  readonly animate?: boolean;
  readonly onStats?: (stats: SurfaceStats) => void;
  readonly onError?: (message: string) => void;
}

export interface MountedScene {
  /** Swap the module without touching the canvas or the device. */
  setModule(module: SceneModule): void;
  setAnimating(animating: boolean): void;
  /** Redraw once even while paused — what an edit and a resize need. */
  requestFrame(): void;
  dispose(): void;
}

interface ActiveScene {
  readonly canvas: HTMLCanvasElement;
  readonly renderer: Renderer;
  readonly fonts: Scene['fonts'];
  readonly clear: readonly [number, number, number, number];
  readonly options: MountOptions;
  module: SceneModule | undefined;
  origin: number;
  /** Frozen while paused, so a scene resumes where it stopped. */
  elapsed: number;
  animating: boolean;
  visible: boolean;
  failed: boolean;
  /** Set when a paused scene still owes one frame — an edit, or a resize. */
  pending: boolean;
  width: number;
  height: number;
}

/** The short side of every scene, in the units demos are written in. */
export const DESIGN_UNIT = 360;

const active = new Map<HTMLCanvasElement, ActiveScene>();
let frameHandle: number | undefined;

function prefersReducedMotion(): boolean {
  return globalThis.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;
}

function parseClearColor(color: string): [number, number, number, number] {
  const hex = color.replace('#', '');
  const expanded = hex.length === 3 ? [...hex].map((digit) => digit + digit).join('') : hex;
  const value = Number.parseInt(expanded.slice(0, 6), 16);
  if (!Number.isFinite(value)) return [0, 0, 0, 1];
  return [((value >> 16) & 0xff) / 255, ((value >> 8) & 0xff) / 255, (value & 0xff) / 255, 1];
}

/**
 * Match the backing store to the element's CSS box.
 *
 * Returns whether anything moved, because resizing a surface reallocates the
 * swapchain and the persistent backing — worth doing only when the box really
 * changed, not on every observer callback.
 */
function synchronizeSize(scene: ActiveScene): boolean {
  const ratio = Math.min(globalThis.devicePixelRatio || 1, 2);
  const box = scene.canvas.getBoundingClientRect();
  const width = Math.max(1, Math.round(box.width * ratio));
  const height = Math.max(1, Math.round(box.height * ratio));
  if (width === scene.canvas.width && height === scene.canvas.height) return false;
  scene.canvas.width = width;
  scene.canvas.height = height;
  scene.renderer.resize(width, height);
  scene.width = box.width;
  scene.height = box.height;
  return true;
}

function renderScene(scene: ActiveScene, now: number): void {
  const module = scene.module;
  if (!module) return;
  const builder = new valoModule.DisplayListBuilder();
  let list: ReturnType<typeof builder.build> | undefined;
  try {
    // Scenes record in design units, where the canvas's SHORT side is always
    // `DESIGN_UNIT`. A stroke width of 8 then means the same thing on a card
    // and in the playground, so scene code carries absolute numbers instead of
    // a fraction of whatever box it landed in.
    const unit = Math.min(scene.width, scene.height) / DESIGN_UNIT;
    const pixelRatio = scene.canvas.width / Math.max(scene.width, 1);
    builder.scale(pixelRatio * unit, pixelRatio * unit);
    module.default({
      builder,
      width: scene.width / unit,
      height: scene.height / unit,
      time: scene.elapsed + (now - scene.origin) / 1000,
      fonts: scene.fonts,
    });
    list = builder.build();
    const stats = scene.renderer.render(list, true, ...scene.clear);
    if (stats) {
      scene.options.onStats?.({
        draws: stats.draws,
        drawCalls: stats.drawCalls,
        renderPasses: stats.renderPasses,
        cpuMilliseconds: stats.cpuMilliseconds,
      });
      stats.free();
    }
  } catch (error) {
    // One bad scene must not take the driver — and must not repeat sixty times
    // a second — so it retires and says why.
    scene.failed = true;
    scene.options.onError?.(error instanceof Error ? error.message : String(error));
  } finally {
    list?.free();
    builder.free();
  }
}

function tick(now: number): void {
  frameHandle = undefined;
  const moving = !prefersReducedMotion() && !document.hidden;
  for (const scene of active.values()) {
    if (scene.failed) continue;
    const resized = synchronizeSize(scene);
    const due = scene.pending || resized || (moving && scene.animating && scene.visible);
    if (!due) continue;
    scene.pending = false;
    renderScene(scene, now);
  }
  schedule();
}

function schedule(): void {
  if (frameHandle !== undefined || active.size === 0) return;
  frameHandle = requestAnimationFrame(tick);
}

let visibility: IntersectionObserver | undefined;

function observer(): IntersectionObserver {
  visibility ??= new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        const scene = active.get(entry.target as HTMLCanvasElement);
        if (scene) scene.visible = entry.isIntersecting;
      }
      schedule();
    },
    { rootMargin: '128px' },
  );
  return visibility;
}

/** Attach `canvas` to the shared device. It draws once a module is set. */
export async function mountScene(
  canvas: HTMLCanvasElement,
  options: MountOptions = {},
): Promise<MountedScene> {
  const { fonts } = await sharedRuntime();
  const box = canvas.getBoundingClientRect();
  const renderer = await attachRenderer(canvas);
  const scene: ActiveScene = {
    canvas,
    renderer,
    fonts,
    clear: parseClearColor(options.clearColor ?? '#0b0b0e'),
    options,
    module: undefined,
    origin: performance.now(),
    elapsed: 0,
    animating: options.animate ?? false,
    visible: true,
    failed: false,
    pending: false,
    width: box.width || canvas.width,
    height: box.height || canvas.height,
  };
  active.set(canvas, scene);
  observer().observe(canvas);
  schedule();

  return {
    setModule(next) {
      scene.module?.dispose?.();
      scene.module = next;
      scene.failed = false;
      scene.origin = performance.now();
      scene.elapsed = 0;
      scene.pending = true;
      schedule();
    },
    setAnimating(animating) {
      if (animating === scene.animating) return;
      // Freeze the clock on pause and restart it on resume, so a card picks up
      // the motion where the pointer left it instead of jumping.
      if (animating) scene.origin = performance.now();
      else scene.elapsed += (performance.now() - scene.origin) / 1000;
      scene.animating = animating;
      schedule();
    },
    requestFrame() {
      scene.pending = true;
      schedule();
    },
    dispose() {
      // Scenes are keyed by canvas element, so a LATE dispose from a replaced
      // mount must not evict the one that replaced it. Only the scene that is
      // actually registered may deregister itself.
      const current = active.get(canvas) === scene;
      if (current) {
        observer().unobserve(canvas);
        active.delete(canvas);
      }
      scene.module?.dispose?.();
      // The device outlives its canvases: this releases only this canvas's
      // swapchain and backing, and the shared caches stay warm for the rest.
      renderer.free();
      if (active.size === 0 && frameHandle !== undefined) {
        cancelAnimationFrame(frameHandle);
        frameHandle = undefined;
      }
    },
  };
}
