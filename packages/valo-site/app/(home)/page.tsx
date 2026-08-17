import Link from 'next/link';
import { Canvas2DParity } from '@/components/canvas2d-parity';
import { DemoGrid } from '@/components/demo-grid';
import { HeroDemo } from '@/components/hero-demo';
import { MemoryMeter } from '@/components/memory-meter';
import { HERO_DEMO_ID, demoById } from '@/lib/demos/catalog';
import { packages, repositoryUrl, subtitle } from '@/lib/shared';

const heroDemo = demoById(HERO_DEMO_ID)!;

export default function HomePage() {
  return (
    <main className="mx-auto w-full max-w-[1180px] px-6 pb-28 sm:px-10">
      <Hero />
      <Interfaces />
      <LiveDemos />
      <Compatibility />
      <GetStarted />
    </main>
  );
}

function Hero() {
  return (
    <section className="grid gap-10 border-b border-valo-line py-20 lg:grid-cols-[minmax(0,1fr)_minmax(360px,1fr)] lg:items-center">
      <div>
        <h1 className="valo-display m-0 text-[clamp(34px,6vw,72px)] font-medium">
          The
          <br />
          <span className="text-valo-lime">WebGPU</span>
          <br />
          render engine.
        </h1>
        <p className="m-0 mt-7 max-w-[46ch] text-[16px] text-valo-muted">{subtitle}</p>
        <div className="mt-9 flex flex-wrap items-center gap-3">
          <Link
            href="/docs/getting-started"
            className="bg-valo-lime px-6 py-3 text-[13px] font-semibold tracking-[-0.01em] text-valo-ink transition-opacity hover:opacity-85"
          >
            Get started
          </Link>
          <Link
            href="/playground"
            className="border border-valo-line px-6 py-3 text-[13px] font-medium transition-colors hover:border-white/35 hover:bg-white/5"
          >
            Playground
          </Link>
        </div>
      </div>

      <Link href={`/playground?demo=${heroDemo.id}`} aria-label="Open the hero demo in the playground">
        <HeroDemo source={heroDemo.source} />
      </Link>
    </section>
  );
}

function Interfaces() {
  return (
    <section className="grid gap-px border-b border-valo-line bg-valo-line py-px md:grid-cols-2">
      <Door
        title="Rust"
        command={`cargo add ${packages.rust}`}
        note="The same crate on native platforms and WebAssembly."
        backends={['Metal', 'Vulkan', 'Direct3D', 'Web']}
      />
      <Door
        title="JavaScript / TypeScript"
        command={`npm install ${packages.npm}`}
        note="Valo's engine API or its Canvas2D-compatible adapter."
        backends={['WebGPU', 'WebGL']}
      />
    </section>
  );
}

/**
 * The backends read as compatibility metadata on one baseline — a rail, not a
 * row of badges. Their logos are trademark-licensed, so it is type only.
 */
function Door({
  title,
  command,
  note,
  backends,
}: {
  title: string;
  command: string;
  note: string;
  backends: readonly string[];
}) {
  return (
    <div className="group/door bg-valo-ink py-10 md:px-8">
      <p className="valo-eyebrow m-0">{title}</p>
      <code className="mt-4 block font-mono text-[15px] text-valo-lime">{command}</code>
      <p className="m-0 mt-3 text-[13px] text-valo-muted">{note}</p>
      <div className="mt-5 flex min-h-6 items-center">
        <span className="w-[58px] font-mono text-[9px] font-medium uppercase tracking-[0.18em] text-[#676e77]">
          {backends.length > 1 ? 'Backends' : 'Backend'}
        </span>
        <span
          aria-hidden
          className="mx-3 h-px w-6 bg-valo-lime opacity-55 transition-all duration-150 ease-out group-hover/door:w-8 group-hover/door:opacity-100"
        />
        <div className="flex flex-wrap items-center gap-y-2 font-mono text-[10px] font-medium uppercase leading-none tracking-[0.12em] text-[#b8bdb6] transition-colors duration-150 group-hover/door:text-[#e9ede4]">
          {backends.map((name, index) => (
            <span key={name} className="flex items-center">
              {index > 0 ? <span aria-hidden className="mx-3 h-3 w-px bg-[#2b2e35]" /> : null}
              {name}
            </span>
          ))}
        </div>
      </div>
    </div>
  );
}

function LiveDemos() {
  return (
    <section className="py-20">
      <header className="mb-9 flex flex-wrap items-end justify-between gap-4">
        <div>
          <p className="valo-eyebrow m-0">Examples</p>
          <h2 className="valo-display m-0 mt-4 text-[clamp(30px,4vw,46px)] font-medium">
            Drawn live by valo.
          </h2>
        </div>
        <MemoryMeter />
      </header>
      <DemoGrid />
    </section>
  );
}

function Compatibility() {
  return (
    <section className="border-t border-valo-line py-20">
      <header className="mb-9">
        <p className="valo-eyebrow m-0">Compatibility</p>
        <h2 className="valo-display m-0 mt-4 text-[clamp(30px,4vw,46px)] font-medium">
          A Canvas2D-compatible API.
        </h2>
        <p className="m-0 mt-5 max-w-[66ch] text-[14px] leading-relaxed text-valo-muted">
          The same scenes are drawn by valo and Chrome&rsquo;s own Canvas2D.
        </p>
      </header>
      <Canvas2DParity />
    </section>
  );
}

function GetStarted() {
  return (
    <section className="border-y border-valo-line py-20">
      <p className="valo-eyebrow m-0">Get started</p>
      <div className="mt-7 flex flex-col gap-3 font-mono text-[14px] text-valo-lime sm:flex-row sm:gap-10">
        <code>cargo add {packages.rust}</code>
        <code>npm install {packages.npm}</code>
      </div>
      <div className="mt-9 flex flex-wrap items-center gap-3">
        <Link
          href="/docs"
          className="bg-valo-lime px-6 py-3 text-[13px] font-semibold tracking-[-0.01em] text-valo-ink transition-opacity hover:opacity-85"
        >
          Read the docs
        </Link>
        <a
          href={repositoryUrl}
          className="border border-valo-line px-6 py-3 text-[13px] font-medium transition-colors hover:border-white/35 hover:bg-white/5"
        >
          GitHub
        </a>
        <span className="ml-3 font-mono text-[11px] uppercase tracking-[0.12em] text-[#5f5e65]">
          MIT · 0.1.1
        </span>
      </div>
    </section>
  );
}
