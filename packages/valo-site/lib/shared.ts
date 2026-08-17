export const appName = 'valo';
export const tagline = 'The WebGPU-native 2D render engine.';
export const subtitle =
  'Cross-platform portable 2D graphics engine in Rust. Works on Windows, macOS, Linux, Android, iOS and Web.';

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
  npm: 'valo-web',
} as const;
