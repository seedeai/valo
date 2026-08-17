export const appName = 'valo';
export const tagline = 'The WebGPU-native 2D render engine.';
export const subtitle =
  'Cross-platform portable 2D graphics engine in Rust. Works on Windows, macOS, Linux, Android, iOS and Web.';
export const description =
  'A WebGPU-native 2D render engine for Rust on native platforms and WebAssembly, with JavaScript, TypeScript and Canvas2D-compatible browser APIs.';
export const keywords = [
  'valo',
  'WebGPU',
  '2D renderer',
  'Rust',
  'wgpu',
  'WebAssembly',
  'Canvas2D',
  'display list',
  'Impeller',
];

/** Same ink and lime as `.valo-mark` in global.css. */
export const brand = {
  ink: '#09090b',
  lime: '#c8ff3d',
  text: '#f4f2ef',
  muted: '#8f8e94',
} as const;

export function siteOrigin(): string {
  return process.env.NEXT_PUBLIC_SITE_URL ?? 'https://valo.im';
}

export const docsRoute = '/docs';
export const docsImageRoute = '/og/docs';

export const gitConfig = {
  user: 'seedeai',
  repo: 'valo',
  branch: 'main',
};

export const repositoryUrl = `https://github.com/${gitConfig.user}/${gitConfig.repo}`;
export const rustDocsUrl = 'https://docs.rs/valo/latest/valo/';

/** Package names, in one place because they appear in prose and in code samples. */
export const packages = {
  rust: 'valo',
  npm: 'valo-web',
} as const;
