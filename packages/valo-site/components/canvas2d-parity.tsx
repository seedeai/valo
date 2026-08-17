'use client';

import { useEffect, useRef, useState } from 'react';
import { PARITY_FONT_FAMILY, PARITY_SIZE, paritySource } from '@/lib/demos/parity';

/**
 * The same Canvas2D code running through valo and through the browser, next to
 * each other. valo's context is built over the SHARED device the rest of the
 * page already uses, so the comparison costs the page one more canvas rather
 * than one more GPU device.
 */
export function Canvas2DParity() {
  const valoRef = useRef<HTMLCanvasElement>(null);
  const nativeRef = useRef<HTMLCanvasElement>(null);
  const [error, setError] = useState<string | undefined>();

  useEffect(() => {
    const valoCanvas = valoRef.current;
    const nativeCanvas = nativeRef.current;
    if (!valoCanvas || !nativeCanvas) return;
    let disposed = false;

    void (async () => {
      try {
        const { webGpuAvailability, attachCanvas2D } = await import('@/lib/valo/device');
        const availability = webGpuAvailability();
        if (!availability.supported) {
          setError(availability.reason);
          return;
        }

        const ratio = Math.min(globalThis.devicePixelRatio || 1, 2);
        for (const canvas of [valoCanvas, nativeCanvas]) {
          canvas.width = PARITY_SIZE.width * ratio;
          canvas.height = PARITY_SIZE.height * ratio;
        }

        const fontBytes = await (await fetch('/valo/fonts/fira_sans.ttf')).arrayBuffer();
        // The browser needs the same face registered its own way, or the two
        // sides would be comparing valo against whatever the system picked.
        const face = new FontFace(PARITY_FONT_FAMILY, fontBytes.slice(0));
        await face.load();
        document.fonts.add(face);

        const draw = new Function('ctx', 'FONT', paritySource) as (
          context: unknown,
          font: string,
        ) => void;

        const valoContext = await attachCanvas2D(valoCanvas, { autoPresent: false });
        if (disposed) return;
        await valoContext.registerFont(PARITY_FONT_FAMILY, new Uint8Array(fontBytes), true);
        valoContext.beginFrame('#0e0e12');
        valoContext.scale(ratio, ratio);
        draw(valoContext, PARITY_FONT_FAMILY);
        valoContext.present();

        const nativeContext = nativeCanvas.getContext('2d');
        if (!nativeContext) {
          setError('This browser did not give us a 2D context to compare against.');
          return;
        }
        nativeContext.scale(ratio, ratio);
        draw(nativeContext, PARITY_FONT_FAMILY);
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
      }
    })();

    return () => {
      disposed = true;
    };
  }, []);

  return (
    <div className="grid gap-px border border-valo-line bg-valo-line md:grid-cols-2">
      <ParityPane
        title="valo"
        note="valo-web · WebGPU · tested against Chrome"
        canvasRef={valoRef}
        error={error}
      />
      <ParityPane
        title="the browser"
        note="CanvasRenderingContext2D · whatever this browser ships"
        canvasRef={nativeRef}
      />
    </div>
  );
}

function ParityPane({
  title,
  note,
  canvasRef,
  error,
}: {
  title: string;
  note: string;
  canvasRef: React.RefObject<HTMLCanvasElement | null>;
  error?: string;
}) {
  return (
    <figure className="m-0 bg-valo-ink p-6">
      <figcaption className="mb-4 flex items-baseline justify-between gap-3">
        <span className="text-[15px] font-medium tracking-[-0.03em]">{title}</span>
        <span className="font-mono text-[9px] uppercase tracking-[0.12em] text-[#5f5e65]">
          {note}
        </span>
      </figcaption>
      <div className="relative overflow-hidden border border-valo-line bg-valo-panel">
        <canvas
          ref={canvasRef}
          className="block w-full"
          style={{ aspectRatio: `${PARITY_SIZE.width} / ${PARITY_SIZE.height}` }}
        />
        {error ? (
          <p className="absolute inset-0 m-0 overflow-auto bg-[#161014] p-4 font-mono text-[11px] leading-relaxed text-[#ff9f9f]">
            {error}
          </p>
        ) : null}
      </div>
    </figure>
  );
}
