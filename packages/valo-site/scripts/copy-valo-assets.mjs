/**
 * Stage everything the site serves at runtime rather than bundles.
 *
 * The site consumes the PUBLISHED `valo-web` package from npm — the same
 * artifact every visitor installs — so this script only copies files out of
 * `node_modules` and the repository. No Rust toolchain, no wasm build: it
 * runs anywhere `npm install` ran, which is what makes the site deployable
 * to a plain Node host such as Vercel.
 *
 * Three kinds of asset, each for a different reason:
 *
 * - the wasm binary, because `initializeValo` fetches it by URL. Handing it a
 *   `public/` path keeps the loader identical across bundlers instead of
 *   depending on each one's wasm asset handling.
 * - the fonts, because valo never reaches for a font itself. Text arrives as
 *   registered bytes, so the site has to serve the bytes.
 * - `valo-web`'s emitted `.d.ts`, because the playground feeds them to Monaco.
 *   Completions come from the shipped types, not a hand-written copy, so they
 *   cannot drift from the package.
 * - `standalone-setup.ts`, because the playground shows it as a read-only tab.
 *   Serving the file itself is what makes that tab trustworthy: it is compiled
 *   and typechecked with the rest of the site, so it cannot describe an API
 *   that no longer exists.
 */
import { cpSync, existsSync, mkdirSync, readdirSync, rmSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const siteRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const repoRoot = dirname(dirname(siteRoot));
// Resolved through node's own algorithm, so it finds the package wherever npm
// put it — the registry tarball on a deploy, the workspace link in dev. The
// wasm asset is the anchor because it is in the package's `exports` map;
// `package.json` itself is not.
const webPackage = dirname(
  dirname(createRequire(import.meta.url).resolve('valo-web/wasm/valo_web_bg.wasm')),
);
const staging = join(siteRoot, 'public', 'valo');

function requireBuilt(path, hint) {
  if (existsSync(path)) return path;
  throw new Error(
    `valo-site needs ${path}, which does not exist.\nRun \`${hint}\` first.`,
  );
}

function stageWasm() {
  const wasm = requireBuilt(
    join(webPackage, 'wasm', 'valo_web_bg.wasm'),
    'npm install',
  );
  cpSync(wasm, join(staging, 'valo_web_bg.wasm'));
}

function stageFonts() {
  const source = join(repoRoot, 'assets', 'fonts');
  const destination = join(staging, 'fonts');
  mkdirSync(destination, { recursive: true });
  for (const font of ['fira_sans.ttf', 'jetbrains_mono.ttf']) {
    cpSync(requireBuilt(join(source, font), 'git checkout assets'), join(destination, font));
  }
}

function stageTypes() {
  const distribution = requireBuilt(join(webPackage, 'dist'), 'npm install');
  const destination = join(staging, 'types');
  mkdirSync(destination, { recursive: true });
  for (const entry of readdirSync(distribution)) {
    if (entry.endsWith('.d.ts')) cpSync(join(distribution, entry), join(destination, entry));
  }
  cpSync(
    requireBuilt(join(webPackage, 'wasm', 'valo_web.d.ts'), 'npm install'),
    join(destination, 'valo_web.d.ts'),
  );
}

function stageSetupSource() {
  cpSync(join(siteRoot, 'lib', 'valo', 'standalone-setup.ts'), join(staging, 'setup.ts'));
}

rmSync(staging, { recursive: true, force: true });
mkdirSync(staging, { recursive: true });
stageWasm();
stageFonts();
stageTypes();
stageSetupSource();
console.log(`valo-site: staged wasm, fonts, types and the setup sample from ${webPackage}`);
