/**
 * Stage everything the site serves at runtime rather than bundles.
 *
 * Three kinds of asset, each for a different reason:
 *
 * - the wasm binary, because `initializeValo` fetches it by URL. Handing it a
 *   `public/` path keeps the loader identical across bundlers instead of
 *   depending on each one's wasm asset handling.
 * - the fonts, because valo never reaches for a font itself. Text arrives as
 *   registered bytes, so the site has to serve the bytes.
 * - `valo-web`'s emitted `.d.ts`, because the playground feeds them to Monaco.
 *   Completions on `ctx.` come from the shipped types, not a hand-written copy,
 *   so they cannot drift from the package.
 * - `standalone-setup.ts`, because the playground shows it as a read-only tab.
 *   Serving the file itself is what makes that tab trustworthy: it is compiled
 *   and typechecked with the rest of the site, so it cannot describe an API
 *   that no longer exists.
 */
import { cpSync, existsSync, mkdirSync, readdirSync, rmSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const siteRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const repoRoot = dirname(dirname(siteRoot));
const webPackage = join(repoRoot, 'packages', 'valo-web');
const staging = join(siteRoot, 'public', 'valo');

function requireBuilt(path, hint) {
  if (existsSync(path)) return path;
  throw new Error(
    `valo-site needs ${path}, which does not exist.\nRun \`${hint}\` from the repository root first.`,
  );
}

function stageWasm() {
  const wasm = requireBuilt(
    join(webPackage, 'wasm', 'valo_web_bg.wasm'),
    'npm run build:web',
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
  const distribution = requireBuilt(join(webPackage, 'dist'), 'npm run build:web');
  const destination = join(staging, 'types');
  mkdirSync(destination, { recursive: true });
  for (const entry of readdirSync(distribution)) {
    if (entry.endsWith('.d.ts')) cpSync(join(distribution, entry), join(destination, entry));
  }
  cpSync(
    requireBuilt(join(webPackage, 'wasm', 'valo_web.d.ts'), 'npm run build:web'),
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
console.log(`valo-site: staged wasm, fonts, types and the setup sample into ${staging}`);
