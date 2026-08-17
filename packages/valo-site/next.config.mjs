import { createMDX } from 'fumadocs-mdx/next';

const withMDX = createMDX();

/** @type {import('next').NextConfig} */
const config = {
  reactStrictMode: true,
  // Next writes its own AGENTS.md and CLAUDE.md into the package on dev start.
  // This repo already has an AGENTS.md at the root that says something else,
  // and a second one a directory down would only compete with it.
  agentRules: false,
  // `valo-web` is a workspace sibling shipping untranspiled ESM.
  transpilePackages: ['valo-web'],
  // The wasm module is served from `public/valo/` and fetched by URL, so the
  // bundler never has to understand it. `initializeValo` takes that URL, which
  // keeps the loader identical between webpack, Turbopack and the playground's
  // plain Vite build.
  turbopack: {},
  async rewrites() {
    return [
      { source: '/favicon.ico', destination: '/icon.png' },
      { source: '/docs.md', destination: '/llms.mdx/docs' },
      { source: '/docs/:path*.md', destination: '/llms.mdx/docs/:path*' },
    ];
  },
};

export default withMDX(config);
