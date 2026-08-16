/**
 * A browser smoke test for the site, and — incidentally — the only automated
 * coverage `Device.attach` has anywhere.
 *
 * `test:web` is deliberately wasm-free and the conformance suite drives the
 * single-canvas shorthand, so nothing else exercises a dozen canvases sharing
 * one device. That is exactly the arrangement this page rests on, so it is
 * worth a test that would notice it breaking.
 *
 * What it asserts is deliberately coarse — that each canvas produced varied
 * pixels, that one device is serving all of them, that Monaco loads on the
 * playground and nowhere else. Pixel-exactness belongs to the conformance
 * suite; this is here to catch a page that renders nothing.
 *
 * Run against a production build: `npm run test:site` from the repository root.
 */
import { spawn } from 'node:child_process';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { gzipSync } from 'node:zlib';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';
import { PNG } from 'pngjs';

const siteRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const PORT = Number(process.env.VALO_SITE_PORT ?? 3311);
const BASE = `http://localhost:${PORT}`;
const EXPECTED_DEMOS = 6;

let failures = 0;
function check(name, ok, detail = '') {
  console.log(`${ok ? 'ok  ' : 'FAIL'}  ${name}${detail ? `  — ${detail}` : ''}`);
  if (!ok) failures += 1;
}

/**
 * How many distinct colours an element shows.
 *
 * Read from a screenshot rather than the canvas: a WebGPU canvas is not a
 * usable `drawImage` source in headless Chromium, so sampling it directly
 * reports every scene as blank and every assertion as passing-by-accident.
 */
async function distinctColours(locator, path) {
  await locator.screenshot({ path });
  const png = PNG.sync.read(readFileSync(path));
  const seen = new Set();
  for (let index = 0; index < png.data.length; index += 4 * 53) {
    seen.add(`${png.data[index]},${png.data[index + 1]},${png.data[index + 2]}`);
  }
  return seen.size;
}

/** Whether two screenshots of `locator`, a second apart, differ at all. */
async function movesOverASecond(locator, prefix) {
  await locator.screenshot({ path: `${prefix}-a.png` });
  await locator.page().waitForTimeout(900);
  await locator.screenshot({ path: `${prefix}-b.png` });
  return !readFileSync(`${prefix}-a.png`).equals(readFileSync(`${prefix}-b.png`));
}

async function checkHoverDrivenAnimation(page, card) {
  const canvas = card.locator('canvas');
  check('a card holds still until hovered', !(await movesOverASecond(canvas, '/tmp/valo-site-idle')));
  await card.hover();
  await page.waitForTimeout(300);
  check('and animates while hovered', await movesOverASecond(canvas, '/tmp/valo-site-hover'));
  await page.mouse.move(0, 0);
  await page.waitForTimeout(300);
}

function startServer() {
  const server = spawn('npx', ['next', 'start', '-p', String(PORT)], {
    cwd: siteRoot,
    stdio: 'ignore',
  });
  return server;
}

async function waitForServer() {
  for (let attempt = 0; attempt < 60; attempt += 1) {
    try {
      const response = await fetch(BASE);
      if (response.ok) return;
    } catch {
      /* not listening yet */
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(`the site never came up on ${BASE}`);
}

/** Monaco must not reach the landing page. Checked against the built output. */
function auditLandingBundle() {
  const html = join(siteRoot, '.next/server/app/index.html');
  if (!existsSync(html)) {
    check('landing bundle audit', false, 'no production build to inspect');
    return;
  }
  const markup = readFileSync(html, 'utf8');
  const scripts = [
    ...new Set(
      [...markup.matchAll(/<script[^>]+src="\/_next\/(static\/chunks\/[^"]+\.js)"/g)].map(
        (match) => match[1],
      ),
    ),
  ];
  let raw = 0;
  let gzip = 0;
  const withMonaco = [];
  for (const script of scripts) {
    const bytes = readFileSync(join(siteRoot, '.next', script));
    raw += bytes.length;
    gzip += gzipSync(bytes).length;
    if (/monaco|MonacoEnvironment/i.test(bytes.toString())) withMonaco.push(script);
  }
  // Absence only means something if the detector can find Monaco at all, so
  // confirm it IS in the build — just in a chunk the landing page never loads.
  let monacoBytes = 0;
  const chunkRoot = join(siteRoot, '.next/static/chunks');
  const walk = (directory) => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) walk(path);
      else if (path.endsWith('.js')) {
        const bytes = readFileSync(path);
        if (/monaco|MonacoEnvironment/i.test(bytes.toString())) monacoBytes += bytes.length;
      }
    }
  };
  walk(chunkRoot);

  check('Monaco is in the build at all', monacoBytes > 1_000_000, `${(monacoBytes / 1024 / 1024).toFixed(2)} MiB`);
  check('but not on the landing page', withMonaco.length === 0, withMonaco.join(', '));
  console.log(
    `      landing initial JS: ${(raw / 1024).toFixed(1)} KiB raw, ${(gzip / 1024).toFixed(1)} KiB gzip`,
  );
}

const server = startServer();
let browser;
try {
  await waitForServer();
  auditLandingBundle();

  browser = await chromium.launch({
    channel: 'chromium',
    headless: true,
    args: ['--enable-unsafe-webgpu'],
  });
  const page = await browser.newPage({ viewport: { width: 1400, height: 1000 } });
  const consoleErrors = [];
  page.on('console', (message) => message.type() === 'error' && consoleErrors.push(message.text()));
  page.on('pageerror', (error) => consoleErrors.push(`pageerror: ${error.message}`));

  await page.goto(BASE, { waitUntil: 'networkidle' });
  await page.evaluate(async () => {
    for (let y = 0; y < document.body.scrollHeight; y += 400) {
      window.scrollTo(0, y);
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
  });
  await page.waitForTimeout(2500);

  const cards = page.locator('article');
  check('every demo card is on the page', (await cards.count()) === EXPECTED_DEMOS, `${await cards.count()}`);

  for (let index = 0; index < (await cards.count()); index += 1) {
    const card = cards.nth(index);
    const title = (await card.locator('h3').innerText()).trim();
    const colours = await distinctColours(card.locator('canvas'), `/tmp/valo-site-card-${index}.png`);
    check(`card draws: ${title}`, colours > 8, `${colours} colours`);
  }

  check('no card reported a render error', (await page.locator('article p.text-\\[\\#ff9f9f\\]').count()) === 0);

  // A card paints one frame and then holds still until the pointer arrives.
  // Both halves matter: still-on-load is what saves the GPU, and moving-on-hover
  // is what makes the grid worth having, so neither is asserted without the other.
  await checkHoverDrivenAnimation(page, cards.first());
  const canvases = await page.locator('canvas[aria-label]').count();
  check('the landing page has one live canvas per demo plus the hero', canvases === EXPECTED_DEMOS + 1, `${canvases}`);
  check(
    'the hero canvas is live',
    (await distinctColours(page.locator('canvas[aria-label="live"]'), '/tmp/valo-site-hero.png')) > 8,
  );
  const sharedCanvases = Number(
    await page.locator('[data-shared-device-canvases]').getAttribute('data-shared-device-canvases'),
  );
  check('the landing canvases share one device', sharedCanvases >= EXPECTED_DEMOS + 1, `${sharedCanvases} attached`);

  const panes = page.locator('figure canvas');
  for (let index = 0; index < (await panes.count()); index += 1) {
    const colours = await distinctColours(panes.nth(index), `/tmp/valo-site-parity-${index}.png`);
    check(`canvas2d parity pane ${index} drew`, colours > 8, `${colours} colours`);
  }

  // ---- playground ----
  await page.goto(`${BASE}/playground?demo=backdrop-blur`, { waitUntil: 'networkidle' });
  await page.waitForSelector('.monaco-editor', { timeout: 60_000 });
  await page.waitForTimeout(6000);

  check(
    'the playground opens the demo the card linked to',
    /backdropBlur/.test(await page.locator('.monaco-editor .view-lines').first().innerText()),
  );
  check(
    'the playground canvas is live',
    (await distinctColours(page.locator('canvas[role="img"]'), '/tmp/valo-site-playground.png')) > 8,
  );

  // Monaco 0.56 takes keystrokes through the EditContext API, which Playwright
  // cannot drive, so the suggest widget cannot be opened from here. What
  // completions actually depend on CAN be checked: whether the TypeScript
  // worker resolves valo's symbols from the shipped `.d.ts`. Served, the demo
  // typechecks clean; with those files blocked, the same demo must light up.
  check('the editor typechecks demo code', (await page.locator('.squiggly-error').count()) === 0);

  // The setup tab is the answer to "what runs this?", so it has to be showing
  // the file itself rather than an empty pane that failed to fetch.
  await page.getByRole('button', { name: 'setup.ts' }).click();
  await page.waitForTimeout(2500);
  // Only the lines Monaco has rendered are readable, so this asserts against
  // the top of the file rather than anything further down.
  const setupText = await page.locator('.monaco-editor .view-lines').first().innerText();
  check('the setup tab shows the real bootstrap', /createDevice/.test(setupText), setupText.slice(0, 40).replace(/\n/g, ' '));
  await page.getByRole('button', { name: 'scene.js' }).click();

  const blinded = await browser.newPage({ viewport: { width: 1400, height: 900 } });
  await blinded.route('**/valo/types/*.d.ts', (route) => route.abort());
  await blinded.goto(`${BASE}/playground?demo=strokes`, { waitUntil: 'networkidle' });
  await blinded.waitForSelector('.monaco-editor', { timeout: 60_000 });
  await blinded.waitForTimeout(7000);
  const blindErrors = await blinded.locator('.squiggly-error').count();
  check("and it is @valo/web's own types doing it", blindErrors > 0, `${blindErrors} errors without them`);
  await blinded.close();

  await page.goto(`${BASE}/docs`, { waitUntil: 'networkidle' });
  check('docs render', (await page.locator('h1').first().innerText()).length > 0);

  // ---- no WebGPU ----
  const bare = await browser.newPage({ viewport: { width: 1200, height: 900 } });
  await bare.addInitScript(() => {
    Object.defineProperty(navigator, 'gpu', { get: () => undefined, configurable: true });
  });
  await bare.goto(BASE, { waitUntil: 'networkidle' });
  await bare.waitForTimeout(2500);
  const notice = await bare.locator('p.text-\\[\\#ff9f9f\\]').first().innerText();
  check('without WebGPU the page says so', /WebGPU/.test(notice), notice.slice(0, 60));
  check('and says it once, not per card', (await bare.locator('article p.text-\\[\\#ff9f9f\\]').count()) === 0);
  check(
    'and leaves a placeholder rather than a blank square',
    (await bare.getByText('needs webgpu').count()) === EXPECTED_DEMOS,
  );

  const noisy = consoleErrors.filter((text) => !/favicon|Failed to load resource/.test(text));
  check('no console errors', noisy.length === 0, noisy.slice(0, 3).join(' | '));
} finally {
  await browser?.close();
  server.kill();
}

console.log(failures === 0 ? '\nsite smoke: all checks passed' : `\nsite smoke: ${failures} failed`);
process.exit(failures === 0 ? 0 : 1);
