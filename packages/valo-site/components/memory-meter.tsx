'use client';

import { useEffect, useState } from 'react';

export function MemoryMeter() {
  const [canvases, setCanvases] = useState<number | undefined>();

  useEffect(() => {
    let cancelled = false;

    async function sample() {
      const { memoryReport } = await import('@/lib/valo/device');
      const { report, canvases } = await memoryReport();
      if (!cancelled) setCanvases(canvases);
      report.free();
    }

    const timer = setInterval(() => void sample().catch(() => undefined), 1000);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, []);

  return (
    <p
      className="m-0 text-[13px] text-valo-muted"
      data-shared-device-canvases={canvases}
    >
      {canvases ? `${canvases} live valo canvases. One GPU device.` : 'Every valo canvas shares one GPU device.'}
    </p>
  );
}
