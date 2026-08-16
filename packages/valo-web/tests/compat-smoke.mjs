/**
 * Proves the compat build actually falls back: a browser with NO WebGPU gets
 * frames out of `wasm-compat/` via wgpu's WebGL2 backend.
 *
 * This is the only automated coverage that path has. Nothing else in the
 * repository ever runs without WebGPU: the conformance suite and the site
 * smoke both require it, so a compat regression — a WebGPU-sized limit, a
 * swapchain usage GL cannot serve, an adapter requested without a surface —
 * would otherwise ship silently and fail only on a visitor's machine.
 *
 * Asserted from a screenshot, not the canvas: reading a GPU canvas back
 * through drawImage in headless Chromium reports blank pixels and would pass
 * on a black canvas.
 *
 * Run after `npm run build`: `npm run test:compat` (repo root wires it).
 */
import { createServer } from 'node:http';
import { readFileSync } from 'node:fs';
import { dirname, extname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';
import { PNG } from 'pngjs';

const packageRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const PORT = Number(process.env.VALO_COMPAT_PORT ?? 3417);

const PAGE = `<!doctype html>
<meta charset="utf-8">
<canvas id="c" width="300" height="300" style="width:300px;height:300px"></canvas>
<pre id="log"></pre>
<script type="module">
  const log = (m) => { document.getElementById('log').textContent += m + '\\n'; };
  try {
    log('gpu:' + String(!!navigator.gpu));
    const valo = await import('/wasm-compat/valo_web.js');
    await valo.default('/wasm-compat/valo_web_bg.wasm');
    const renderer = await valo.createRenderer(document.getElementById('c'));
    const builder = new valo.DisplayListBuilder();
    const bg = new valo.Paint(0.05, 0.05, 0.08, 1);
    builder.drawRect(0, 0, 300, 300, bg);
    const lime = new valo.Paint(0.784, 1, 0.239, 1);
    builder.drawRoundedRect(60, 60, 180, 180, new Float32Array([40]), lime);
    const glow = new valo.Paint(0.49, 0.29, 1, 1);
    glow.setMaskBlur(12, 0);
    builder.drawRoundedRect(120, 120, 120, 120, new Float32Array([20]), glow);
    const list = builder.build();
    renderer.render(list, true, 0, 0, 0, 1);
    log('render:ok');
  } catch (error) {
    log('render:FAIL ' + (error && error.message || error));
  }
</script>`;

const types = { '.js': 'text/javascript', '.wasm': 'application/wasm', '.html': 'text/html' };

const server = createServer((request, response) => {
  if (request.url === '/') {
    response.writeHead(200, { 'content-type': 'text/html' });
    response.end(PAGE);
    return;
  }
  try {
    const body = readFileSync(join(packageRoot, request.url.slice(1)));
    response.writeHead(200, { 'content-type': types[extname(request.url)] ?? 'application/octet-stream' });
    response.end(body);
  } catch {
    response.writeHead(404).end();
  }
});

let failures = 0;
function check(name, ok, detail = '') {
  console.log(`${ok ? 'ok  ' : 'FAIL'}  ${name}${detail ? `  — ${detail}` : ''}`);
  if (!ok) failures += 1;
}

let browser;
try {
  await new Promise((resolve) => server.listen(PORT, resolve));
  browser = await chromium.launch({ channel: 'chromium', headless: true });
  const page = await browser.newPage({ viewport: { width: 400, height: 400 } });
  // The point of the test: this browser has no WebGPU at all.
  await page.addInitScript(() => {
    Object.defineProperty(navigator, 'gpu', { get: () => undefined, configurable: true });
  });
  await page.goto(`http://localhost:${PORT}/`);
  await page.waitForFunction(
    () => /render:(ok|FAIL)/.test(document.getElementById('log')?.textContent ?? ''),
    undefined,
    { timeout: 30_000 },
  );
  const log = await page.locator('#log').innerText();
  check('WebGPU is absent', /gpu:false/.test(log), log.split('\n')[0]);
  check('the compat build rendered on WebGL2', /render:ok/.test(log), log.replace(/\n/g, ' '));

  await page.locator('#c').screenshot({ path: '/tmp/valo-compat-smoke.png' });
  const png = PNG.sync.read(readFileSync('/tmp/valo-compat-smoke.png'));
  const seen = new Set();
  for (let index = 0; index < png.data.length; index += 4 * 37) {
    seen.add(`${png.data[index]},${png.data[index + 1]},${png.data[index + 2]}`);
  }
  check('and produced varied pixels', seen.size > 8, `${seen.size} colours`);
} finally {
  await browser?.close();
  server.close();
}

console.log(failures === 0 ? '\ncompat smoke: all checks passed' : `\ncompat smoke: ${failures} failed`);
process.exit(failures === 0 ? 0 : 1);
