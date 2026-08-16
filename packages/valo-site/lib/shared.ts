export const appName = 'valo';
export const tagline = 'The WebGPU render engine.';
/** Says the domain the tagline leaves open, for anyone who has not seen a demo. */
export const subtitle =
  'Cross-platform portable 2D graphics engine in Rust. Works on Windows, macOS, Linux, and Web.';

export const docsRoute = '/docs';
export const docsImageRoute = '/og/docs';
export const docsContentRoute = '/llms.mdx/docs';

export const gitConfig = {
  user: 'seedeai',
  repo: 'valo',
  branch: 'main',
};

export const repositoryUrl = `https://github.com/${gitConfig.user}/${gitConfig.repo}`;

/** Package names, in one place because they appear in prose and in code samples. */
export const packages = {
  rust: 'valo',
  npm: '@valo/web',
} as const;

/**
 * The differential conformance result, stated as a number because it is one.
 * The suite renders each scene through valo and through Chrome's own Canvas2D
 * and compares pixels; `npm run test:conformance` is the thing that proves it.
 */
export const conformance = { passing: 105, total: 105 } as const;
