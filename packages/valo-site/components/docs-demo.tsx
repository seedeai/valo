'use client';

import dynamic from 'next/dynamic';
import Link from 'next/link';
import { ArrowUpRight } from 'lucide-react';
import { useEffect, useId, useRef, useState } from 'react';
import { demoById } from '@/lib/demos/catalog';
import { stashPlaygroundDraft } from '@/lib/demos/playground-draft';
import { ValoCanvas } from './valo-canvas';
import { cn } from '@/lib/cn';

/**
 * Monaco is several megabytes. Load it only when a docs page actually mounts
 * a demo, and never during server rendering — the editor touches `self` and
 * `Worker` at module scope.
 */
const SceneEditor = dynamic(() => import('@/components/playground/editor'), {
  ssr: false,
  loading: () => (
    <div className="grid h-full place-items-center font-mono text-[10px] uppercase tracking-[0.13em] text-white/25">
      loading editor
    </div>
  ),
});

/**
 * A live valo canvas for MDX pages, with the scene source in Monaco beside it.
 *
 * `demo` is a catalog id. The widget, the playground, and any landing card that
 * uses the same id all run that string. Edits debounce onto the canvas; Open
 * in playground stashes the current draft so the editor over there starts
 * where this one left off.
 */
export function DocsDemo({
  demo: id,
  animate = false,
  caption,
  className,
}: {
  demo: string;
  animate?: boolean;
  caption?: string;
  className?: string;
}) {
  const path = `file:///docs-demo-${useId().replaceAll(':', '')}.js`;
  const demo = demoById(id);
  const initial = (demo?.source ?? '').trim();
  const [draft, setDraft] = useState(initial);
  const [live, setLive] = useState(initial);
  const skipDebounce = useRef(true);

  useEffect(() => {
    if (skipDebounce.current) {
      skipDebounce.current = false;
      return;
    }
    const timer = setTimeout(() => setLive(draft), 260);
    return () => clearTimeout(timer);
  }, [draft]);

  if (!demo) {
    return (
      <p className="not-prose my-6 border border-valo-line bg-[#161014] p-5 font-mono text-[12px] text-[#ff9f9f]">
        Unknown demo `{id}`.
      </p>
    );
  }

  return (
    <figure className={cn('not-prose my-6 overflow-hidden border border-valo-line', className)}>
      <div className="grid md:grid-cols-2">
        <div className="order-2 flex h-72 flex-col border-t border-valo-line md:order-1 md:h-80 md:border-t-0 md:border-r">
          <div className="flex shrink-0 items-center justify-between gap-3 border-b border-valo-line px-4 py-2">
            <span className="font-mono text-[11px] text-valo-lime">scene.js</span>
            <Link
              href={`/playground?demo=${demo.id}`}
              onClick={() => stashPlaygroundDraft(demo.id, draft)}
              className="flex items-center gap-1 font-mono text-[10px] uppercase tracking-[0.12em] text-valo-muted transition-colors hover:text-valo-lime"
            >
              playground
              <ArrowUpRight className="size-3" />
            </Link>
          </div>
          <div className="min-h-0 flex-1">
            <SceneEditor value={draft} path={path} language="javascript" onChange={setDraft} />
          </div>
        </div>
        <ValoCanvas
          source={live}
          clearColor="#0e0e12"
          className="order-1 h-52 w-full md:order-2 md:h-80"
          animate={animate}
          label={caption}
        />
      </div>
    </figure>
  );
}
