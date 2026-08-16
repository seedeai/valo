'use client';

import { useEffect, useRef, useState } from 'react';
import type { MountedScene, SurfaceStats } from '@/lib/valo/surface';
import { cn } from '@/lib/cn';

/**
 * One live valo canvas.
 *
 * The valo modules are reached with `await import()` from an effect and never
 * from the module graph, which is what keeps WebGPU and WebAssembly out of
 * server rendering — neither exists there — and puts them in a chunk the
 * landing page loads after paint rather than before it.
 */

export interface ValoCanvasProps {
  /** Scene source, evaluated here and in the playground from the same text. */
  readonly source: string;
  readonly clearColor?: string;
  readonly className?: string;
  readonly label?: string;
  /**
   * Whether the scene keeps drawing.
   *
   * It always paints its first frame, so a paused canvas shows the scene
   * rather than a black square. Cards pass the pointer's state here: a grid of
   * animating canvases spends real GPU time on demos nobody is looking at.
   */
  readonly animate?: boolean;
  readonly onStats?: (stats: SurfaceStats) => void;
}

export function ValoCanvas({
  source,
  clearColor,
  className,
  label,
  animate = false,
  onStats,
}: ValoCanvasProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const sceneRef = useRef<MountedScene | undefined>(undefined);
  const statsRef = useRef(onStats);
  const [ready, setReady] = useState(false);
  const [error, setError] = useState<string | undefined>();

  statsRef.current = onStats;

  // Mount once per canvas. The source is applied by the effect below, so
  // editing it swaps the module instead of reallocating a swapchain.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    let disposed = false;

    // Held so cleanup can dispose what THIS mount created, even if the mount
    // has not resolved yet. React's StrictMode mounts, unmounts and remounts
    // in development: without this the first mount's `dispose` lands after the
    // second has attached, and — since scenes are keyed by canvas element —
    // evicts the live one. Every canvas goes black, and only in `next dev`.
    const pending = (async () => {
      const [{ mountScene }, { webGpuAvailability }] = await Promise.all([
        import('@/lib/valo/surface'),
        import('@/lib/valo/device'),
      ]);
      const availability = webGpuAvailability();
      if (!availability.supported) {
        setError(availability.reason);
        return undefined;
      }
      try {
        const scene = await mountScene(canvas, {
          clearColor,
          onStats: (stats) => statsRef.current?.(stats),
          onError: setError,
        });
        if (disposed) {
          scene.dispose();
          return undefined;
        }
        sceneRef.current = scene;
        setReady(true);
        return scene;
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
      }
      return undefined;
    })();

    return () => {
      disposed = true;
      void pending.then((scene) => scene?.dispose());
      sceneRef.current = undefined;
    };
  }, [clearColor]);

  useEffect(() => {
    const scene = sceneRef.current;
    if (!ready || !scene) return;
    void import('@/lib/valo/compile').then(({ compileScene }) => {
      const compiled = compileScene(source);
      if (!compiled.ok) {
        setError(compiled.error);
        return;
      }
      setError(undefined);
      scene.setModule(compiled.module);
    });
  }, [ready, source]);

  useEffect(() => {
    if (!ready) return;
    sceneRef.current?.setAnimating(animate);
  }, [ready, animate]);

  return (
    <div className={cn('relative overflow-hidden bg-valo-panel', className)}>
      <canvas ref={canvasRef} className="block h-full w-full" aria-label={label} role="img" />
      {label ? (
        <span className="pointer-events-none absolute left-4 top-4 font-mono text-[9px] uppercase tracking-[0.13em] text-white/35">
          {label}
        </span>
      ) : null}
      {!ready && !error ? (
        <span className="pointer-events-none absolute inset-0 grid place-items-center font-mono text-[10px] uppercase tracking-[0.13em] text-white/25">
          acquiring device
        </span>
      ) : null}
      {error ? (
        <p className="absolute inset-0 m-0 overflow-auto bg-[#161014] p-5 font-mono text-[11px] leading-relaxed text-[#ff9f9f]">
          {error}
        </p>
      ) : null}
    </div>
  );
}
