'use client';

import dynamic from 'next/dynamic';
import { useEffect, useRef, useState } from 'react';
import { demoById, demos } from '@/lib/demos/catalog';
import type { SurfaceStats } from '@/lib/valo/surface';
import { ValoCanvas } from '@/components/valo-canvas';
import { cn } from '@/lib/cn';

/**
 * Monaco is several megabytes and belongs to this route alone.
 *
 * `ssr: false` is not optional: the editor touches `self` and `Worker` at
 * module scope, neither of which exists while Next renders on the server.
 */
const SceneEditor = dynamic(() => import('./editor'), {
  ssr: false,
  loading: () => (
    <div className="grid h-full place-items-center font-mono text-[10px] uppercase tracking-[0.13em] text-white/25">
      loading editor
    </div>
  ),
});

type Tab = 'scene' | 'setup';

/**
 * Two files, because a scene on its own is half the picture.
 *
 * `scene.js` is a real module: it imports from `valo-web/raw` and exports its
 * draw function, and the same text runs on the landing page. `setup.ts` is the
 * bootstrap that calls it — the device, the fonts, the canvas and the frame
 * loop — served straight from the repository so it cannot describe an API that
 * no longer exists. Nothing is injected into a scene that it did not import.
 */
export function Playground({ initialDemo }: { initialDemo?: string }) {
  const initial = demoById(initialDemo) ?? demos[0]!;
  const [selected, setSelected] = useState(initial.id);
  const [draft, setDraft] = useState(initial.source);
  const [live, setLive] = useState(initial.source);
  const [tab, setTab] = useState<Tab>('scene');
  const [setupSource, setSetupSource] = useState('');
  const [stats, setStats] = useState<SurfaceStats | undefined>();
  const firstRender = useRef(true);

  useEffect(() => {
    if (firstRender.current) {
      firstRender.current = false;
      return;
    }
    const timer = setTimeout(() => setLive(draft), 260);
    return () => clearTimeout(timer);
  }, [draft]);

  useEffect(() => {
    if (tab !== 'setup' || setupSource) return;
    void import('./editor').then(({ fetchSetupSource }) =>
      fetchSetupSource().then(setSetupSource),
    );
  }, [tab, setupSource]);

  function choose(id: string) {
    const demo = demoById(id);
    if (!demo) return;
    setSelected(id);
    setDraft(demo.source);
    setLive(demo.source);
  }

  return (
    <div className="flex min-h-[calc(100vh-56px)] flex-col">
      <header className="flex flex-wrap items-center justify-between gap-4 border-b border-valo-line px-6 py-4 sm:px-10">
        <div className="flex items-center gap-4">
          <p className="valo-eyebrow m-0">Playground</p>
          <select
            value={selected}
            onChange={(event) => choose(event.target.value)}
            className="border border-valo-line bg-valo-panel px-3 py-2 font-mono text-[12px] text-valo-text outline-none focus:border-valo-lime"
          >
            {demos.map((demo) => (
              <option key={demo.id} value={demo.id}>
                {demo.title}
              </option>
            ))}
          </select>
        </div>
        <p className="m-0 font-mono text-[10px] uppercase tracking-[0.12em] text-[#5f5e65]">
          {stats
            ? `${stats.draws} draws · ${stats.drawCalls} calls · ${stats.renderPasses} passes · ${stats.cpuMilliseconds.toFixed(2)} ms cpu`
            : 'raw api · valo-web/raw'}
        </p>
      </header>

      <div className="grid flex-1 grid-cols-1 lg:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
        <div className="flex min-h-[420px] flex-col border-b border-valo-line lg:border-b-0 lg:border-r">
          <div className="flex shrink-0 items-center gap-px border-b border-valo-line bg-valo-line">
            <FileTab name="scene.js" active={tab === 'scene'} onSelect={() => setTab('scene')} />
            <FileTab name="setup.ts" active={tab === 'setup'} onSelect={() => setTab('setup')} />
            <p className="m-0 flex-1 bg-valo-ink px-4 py-2 text-right font-mono text-[10px] uppercase tracking-[0.12em] text-[#5f5e65]">
              {tab === 'scene' ? 'edits render live' : 'read only · the code that runs scene.js'}
            </p>
          </div>
          <div className="min-h-0 flex-1">
            {tab === 'scene' ? (
              <SceneEditor
                value={draft}
                path="file:///scene.js"
                language="javascript"
                onChange={setDraft}
              />
            ) : (
              <SceneEditor
                value={setupSource}
                path="file:///setup.preview.ts"
                language="typescript"
                readOnly
              />
            )}
          </div>
        </div>
        <div className="flex flex-col gap-5 p-6 sm:p-10">
          <ValoCanvas
            source={live}
            clearColor="#0e0e12"
            className="aspect-square w-full border border-valo-line"
            label={demoById(selected)?.title}
            animate
            onStats={setStats}
          />
          <p className="m-0 text-[13px] leading-relaxed text-valo-muted">
            {demoById(selected)?.beyondCanvas2D}
          </p>
          <p className="m-0 font-mono text-[10px] leading-relaxed text-[#5f5e65]">
            {demoById(selected)?.grounding}
          </p>
        </div>
      </div>
    </div>
  );
}

function FileTab({
  name,
  active,
  onSelect,
}: {
  name: string;
  active: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onSelect}
      className={cn(
        'bg-valo-ink px-5 py-2 font-mono text-[11px] transition-colors',
        active ? 'text-valo-lime' : 'text-[#5f5e65] hover:text-valo-text',
      )}
    >
      {name}
    </button>
  );
}
