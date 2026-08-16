'use client';

import { useEffect, useState } from 'react';
import { ValoCanvas } from './valo-canvas';

/**
 * The one canvas on the page that animates unprompted. Everything below it
 * waits for a pointer.
 */
export function HeroDemo({ source }: { source: string }) {
  const [supported, setSupported] = useState<boolean | undefined>();

  useEffect(() => {
    void import('@/lib/valo/device').then(({ webGpuAvailability }) => {
      setSupported(webGpuAvailability().supported);
    });
  }, []);

  if (!supported) {
    return (
      <div className="grid aspect-[5/4] w-full place-items-center border border-valo-line bg-valo-panel">
        <span className="font-mono text-[9px] uppercase tracking-[0.13em] text-white/20">
          {supported === false ? 'WebGPU required' : 'acquiring device'}
        </span>
      </div>
    );
  }

  return (
    <ValoCanvas
      source={source}
      clearColor="#0e0e12"
      className="aspect-[5/4] w-full border border-valo-line"
      label="live"
      animate
    />
  );
}
