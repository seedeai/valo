'use client';

import Link from 'next/link';
import { useEffect, useState } from 'react';
import { ArrowUpRight } from 'lucide-react';
import { landingDemos, type Demo } from '@/lib/demos/catalog';
import { ValoCanvas } from './valo-canvas';

/**
 * The landing grid. Each card paints one real frame and then holds still until
 * the pointer arrives, so the page shows six live scenes without spending a
 * GPU on six animations nobody asked for.
 */
export function DemoGrid() {
  const unsupported = useWebGpuObstacle();

  return (
    <>
      {unsupported ? (
        <p className="mb-px border border-valo-line bg-[#161014] p-6 font-mono text-[12px] leading-relaxed text-[#ff9f9f]">
          {unsupported}
        </p>
      ) : null}
      <div className="grid gap-px border border-valo-line bg-valo-line sm:grid-cols-2 lg:grid-cols-3">
        {landingDemos.map((demo) => (
          <DemoCard key={demo.id} demo={demo} live={!unsupported} />
        ))}
      </div>
    </>
  );
}

/**
 * Why the demos cannot run, or `undefined` when they can.
 *
 * Asked once for the whole grid: one notice is enough.
 */
function useWebGpuObstacle(): string | undefined {
  const [obstacle, setObstacle] = useState<string | undefined>();

  useEffect(() => {
    void import('@/lib/valo/device').then(({ webGpuAvailability }) => {
      const availability = webGpuAvailability();
      if (!availability.supported) setObstacle(availability.reason);
    });
  }, []);

  return obstacle;
}

function DemoCard({ demo, live }: { demo: Demo; live: boolean }) {
  const [playing, setPlaying] = useState(false);

  return (
    <article className="group flex flex-col bg-valo-ink">
      <Link
        href={`/playground?demo=${demo.id}`}
        className="flex h-full flex-col"
        onPointerEnter={() => setPlaying(true)}
        onPointerLeave={() => setPlaying(false)}
        onFocus={() => setPlaying(true)}
        onBlur={() => setPlaying(false)}
      >
        {live ? (
          <div className="relative">
            <ValoCanvas
              source={demo.source}
              clearColor="#0e0e12"
              className="aspect-square w-full"
              label={demo.title}
              animate={playing}
            />
            <span className="pointer-events-none absolute bottom-4 right-4 font-mono text-[9px] uppercase tracking-[0.13em] text-white/30 transition-opacity group-hover:opacity-0">
              hover to play
            </span>
          </div>
        ) : (
          <div className="grid aspect-square w-full place-items-center bg-valo-panel">
            <span className="font-mono text-[9px] uppercase tracking-[0.13em] text-white/20">
              needs webgpu
            </span>
          </div>
        )}
        <div className="flex items-center justify-between gap-4 p-5">
          <h3 className="m-0 text-[17px] font-medium tracking-[-0.03em]">{demo.title}</h3>
          <span className="flex shrink-0 items-center gap-1 font-mono text-[10px] uppercase tracking-[0.12em] text-valo-muted transition-colors group-hover:text-valo-lime">
            open
            <ArrowUpRight className="size-3" />
          </span>
        </div>
      </Link>
    </article>
  );
}
