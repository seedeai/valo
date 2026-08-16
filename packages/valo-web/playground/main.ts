import {
  ColorFilter,
  DisplayListBuilder,
  FontCollection,
  Paint,
  Paragraph,
  Path,
  Shader,
  createDevice,
} from "../src/raw.ts";
import { initializeValo } from "../src/raw.ts";
import { ValoCanvasRenderingContext2D } from "../src/canvas.ts";

const canvas = document.querySelector<HTMLCanvasElement>("#valo")!;
const title = document.querySelector<HTMLElement>("#title")!;
const description = document.querySelector<HTMLElement>("#description")!;
const chapterNumber = document.querySelector<HTMLElement>("#chapter-number")!;
const stats = document.querySelector<HTMLElement>("#stats")!;
const hint = document.querySelector<HTMLElement>("#hint")!;
const apiKind = document.querySelector<HTMLElement>("#api-kind")!;
const status = document.querySelector<HTMLElement>("#status")!;
const statusDot = document.querySelector<HTMLElement>("#status-dot")!;
const error = document.querySelector<HTMLElement>("#error")!;

interface Chapter {
  slug: string;
  nav: string;
  title: string;
  description: string;
  hint: string;
  api: string;
  draw: (time: number) => void;
  animated?: boolean;
}

await initializeValo();
// One device, one canvas here — but the device is the shareable half, and a
// page of live demos attaches every canvas to a single one of these rather
// than paying for a GPU device per card.
const device = await createDevice();
const renderer = device.attach(canvas);
const fonts = new FontCollection();
const fontResponse = await fetch(new URL("../../../assets/fonts/fira_sans.ttf", import.meta.url));
const fontBytes = new Uint8Array(await fontResponse.arrayBuffer());
fonts.registerFont("Fira Sans", fontBytes, true);

const compat = new ValoCanvasRenderingContext2D(canvas, renderer, {
  autoPresent: false,
  clearColor: "#111116",
});
compat.fonts.registerFont("Fira Sans", fontBytes, true);

let cursor = { x: 560, y: 340 };
let chapterIndex = 0;
let animation = 0;

const chapters: Chapter[] = [
  {
    slug: "overview",
    nav: "Overview",
    title: "One engine.\nEvery surface.",
    description:
      "Valo records a retained display list on the CPU, plans GPU passes once per frame, and renders through WebGPU without runtime shader compilation.",
    hint: "Move the pointer — the light follows",
    api: "raw wasm",
    animated: true,
    draw: drawOverview,
  },
  {
    slug: "geometry",
    nav: "Geometry",
    title: "Curves without\ncompromise.",
    description:
      "Cubic paths, elliptical arcs, per-corner radii, dashes, clips, transforms, and exact fill hit-testing share the same geometry core on web and native.",
    hint: "Arc · ellipse · arcTo · even-odd",
    api: "raw wasm",
    draw: drawGeometry,
  },
  {
    slug: "filters",
    nav: "Filters + layers",
    title: "Compositing is\npart of the engine.",
    description:
      "Color matrices fold into simple sources, image filters snapshot before sampling, and backdrop blur reads the existing frame through explicit pass breaks.",
    hint: "Color matrix · blend · backdrop blur",
    api: "raw wasm",
    animated: true,
    draw: drawFilters,
  },
  {
    slug: "type",
    nav: "Typography",
    title: "Text is a first-class\nrender object.",
    description:
      "Explicit web fonts feed retained paragraphs with shaping, fallback, bidi, wrapping, editing metrics, and three GPU rendering tiers.",
    hint: "Font bytes are explicit — no hidden browser fallback",
    api: "raw wasm",
    draw: drawTypography,
  },
  {
    slug: "canvas",
    nav: "Canvas adapter",
    title: "Canvas-shaped.\nValo-powered.",
    description:
      "A typed compatibility layer batches familiar Canvas2D mutations into retained lists while exposing layers, color matrices, and backdrop blur directly.",
    hint: "Same scene style, GPU-native execution",
    api: "typescript canvas",
    draw: drawCanvasAdapter,
  },
];

buildNavigation();
canvas.addEventListener("pointermove", (event) => {
  const bounds = canvas.getBoundingClientRect();
  cursor = {
    x: ((event.clientX - bounds.left) / bounds.width) * canvas.width,
    y: ((event.clientY - bounds.top) / bounds.height) * canvas.height,
  };
});
document.querySelector("#previous")!.addEventListener("click", () => select(chapterIndex - 1));
document.querySelector("#next")!.addEventListener("click", () => select(chapterIndex + 1));
window.addEventListener("hashchange", selectFromHash);

status.textContent = "WebGPU ready";
statusDot.classList.add("ready");
selectFromHash();

function frame(time: number): void {
  animation = requestAnimationFrame(frame);
  const chapter = chapters[chapterIndex]!;
  if (chapter.animated) chapter.draw(time);
}
animation = requestAnimationFrame(frame);

function buildNavigation(): void {
  const nav = document.querySelector<HTMLElement>("#chapters")!;
  for (const [index, chapter] of chapters.entries()) {
    const button = document.createElement("button");
    button.className = "chapter";
    button.textContent = `${String(index + 1).padStart(2, "0")}  ${chapter.nav}`;
    button.addEventListener("click", () => select(index));
    nav.append(button);
  }
}

function selectFromHash(): void {
  const slug = location.hash.slice(1);
  const index = chapters.findIndex((chapter) => chapter.slug === slug);
  select(index >= 0 ? index : 0, false);
}

function select(index: number, writeHash = true): void {
  chapterIndex = (index + chapters.length) % chapters.length;
  const chapter = chapters[chapterIndex]!;
  if (writeHash) history.replaceState(null, "", `#${chapter.slug}`);
  document.querySelectorAll(".chapter").forEach((element, itemIndex) => {
    element.classList.toggle("active", itemIndex === chapterIndex);
  });
  chapterNumber.textContent = `${String(chapterIndex + 1).padStart(2, "0")} / ${String(chapters.length).padStart(2, "0")}`;
  title.innerHTML = chapter.title.replace("\n", "<br>");
  description.textContent = chapter.description;
  hint.textContent = chapter.hint;
  apiKind.textContent = chapter.api;
  if (!chapter.animated) chapter.draw(performance.now());
}

function present(builder: DisplayListBuilder, clear: readonly [number, number, number, number]): void {
  const list = builder.build();
  const frameStats = renderer.render(list, true, ...clear);
  if (frameStats) {
    stats.textContent = `${frameStats.draws} draws · ${frameStats.renderPasses} passes · ${frameStats.cpuMilliseconds.toFixed(2)} ms cpu`;
    frameStats.free();
  }
  list.free();
  builder.free();
}

function drawOverview(time: number): void {
  const builder = new DisplayListBuilder();
  const background = gradient(0, 0, 1120, 680, [
    [0, 0.055, 0.055, 0.075, 1],
    [1, 0.12, 0.09, 0.18, 1],
  ]);
  builder.drawRect(0, 0, 1120, 680, background);
  background.free();

  const glow = Shader.radialGradient(
    cursor.x, cursor.y, 0, cursor.x, cursor.y, 330,
    new Float32Array([0, 0.45, 1]),
    new Float32Array([0.78, 1, 0.24, 0.26, 0.42, 0.2, 1, 0.08, 0.1, 0.08, 0.15, 0]),
    0,
  );
  const glowPaint = new Paint(1, 1, 1, 1);
  glowPaint.setShader(glow);
  builder.drawRect(0, 0, 1120, 680, glowPaint);
  glow.free(); glowPaint.free();

  const spin = time * 0.0003;
  for (let index = 0; index < 5; index += 1) {
    const x = 135 + index * 185;
    const y = 180 + Math.sin(spin * 3 + index) * 45;
    const paint = gradient(x, y, x + 150, y + 230, [
      [0, 0.78, 1, 0.24, 0.92],
      [0.55, 0.49, 0.29, 1, 0.78],
      [1, 0.1, 0.08, 0.16, 0.94],
    ]);
    builder.drawRoundedRect(x, y, 145, 230, new Float32Array([34, 12, 34, 12]), paint);
    paint.free();
  }
  present(builder, [0.04, 0.04, 0.055, 1]);
}

function drawGeometry(): void {
  const builder = new DisplayListBuilder();
  grid(builder);
  const path = new Path();
  path.moveTo(140, 455);
  path.bezierCurveTo(165, 110, 390, 90, 430, 340);
  path.arcTo(480, 520, 650, 410, 90);
  path.ellipse(690, 330, 170, 105, -0.35, 0.35, Math.PI * 1.55);
  const stroke = new Paint(0.78, 1, 0.24, 1);
  stroke.setStroke(7, 1, 1, 4, new Float32Array([22, 10, 5, 10]), 0);
  builder.drawPath(path, 0, stroke);

  const ring = new Path();
  ring.roundRect(810, 175, 190, 310, new Float32Array([50, 18, 72, 24]), false);
  const fill = gradient(810, 175, 1000, 485, [
    [0, 0.46, 0.25, 1, 0.92],
    [1, 0.95, 0.3, 0.55, 0.88],
  ]);
  builder.drawPath(ring, 0, fill);
  path.free(); ring.free(); stroke.free(); fill.free();
  present(builder, [0.055, 0.055, 0.07, 1]);
}

function drawFilters(time: number): void {
  const builder = new DisplayListBuilder();
  const wave = time * 0.001;
  for (let index = 0; index < 7; index += 1) {
    const x = 90 + index * 145;
    const y = 160 + Math.sin(wave + index * 0.7) * 70;
    const paint = gradient(x, y, x + 100, y + 310, [
      [0, 0.9, 0.22 + index * 0.04, 0.55, 1],
      [1, 0.18, 0.17, 0.9, 1],
    ]);
    builder.drawRoundedRect(x, y, 105, 310, new Float32Array([42]), paint);
    paint.free();
  }
  builder.backdropBlur(190, 230, 740, 210, 18);
  const glass = new Paint(0.07, 0.075, 0.1, 0.54);
  builder.drawRoundedRect(190, 230, 740, 210, new Float32Array([26]), glass);
  glass.free();

  const matrix = ColorFilter.matrix(new Float32Array([
    0.4, 0.6, 0, 0, 0, 0.25, 0.5, 0.25, 0, 0, 0.05, 0.35, 0.6, 0, 0, 0, 0, 0, 1, 0,
  ]));
  const filtered = new Paint(1, 1, 1, 0.82);
  filtered.setColorFilter(matrix);
  builder.drawRoundedRect(420, 270, 280, 130, new Float32Array([18]), filtered);
  matrix.free(); filtered.free();
  present(builder, [0.045, 0.045, 0.06, 1]);
}

function drawTypography(): void {
  const builder = new DisplayListBuilder();
  grid(builder);
  const hero = new Paragraph(
    fonts, "Type moves at GPU speed.", "Fira Sans", 68, 600, false,
    0.95, 0.95, 0.93, 1, 100, true, 0, -1.8, 0, 0, 0, 0, 0, "", 820, false,
  );
  const body = new Paragraph(
    fonts,
    "Paragraphs shape once, wrap cheaply, and retain the font faces they resolved. English · العربية · עברית",
    "Fira Sans", 25, 400, false, 0.66, 0.65, 0.7, 1, 100, true, 0, 0, 1, 1.35, 0, 0, 0, "", 760, false,
  );
  builder.drawParagraph(hero, 105, 175);
  builder.drawParagraph(body, 110, 320);
  const accent = new Paint(0.78, 1, 0.24, 1);
  builder.drawRect(110, 455, Math.min(760, body.maxIntrinsicWidth), 3, accent);
  accent.free(); hero.free(); body.free();
  present(builder, [0.048, 0.048, 0.062, 1]);
}

function drawCanvasAdapter(): void {
  compat.beginFrame("#0d0d12");
  const context = compat;
  context.save();
  context.beginPath();
  context.roundRect(75, 95, 970, 470, 38);
  context.clip();
  // Canvas freezes the clip. Starting a new path must not invalidate it.
  context.beginPath();
  context.fillStyle = "#c8ff3d";
  context.roundRect(110, 130, 320, 390, [56, 18, 56, 18]);
  context.fill();
  context.beginPath();
  context.fillStyle = "rgba(113, 73, 255, .92)";
  context.arc(580, 325, 170, 0, Math.PI * 2);
  context.fill();
  context.beginPath();
  const gradient = context.createLinearGradient(715, 150, 1010, 510);
  gradient.addColorStop(0, "#ff5678");
  gradient.addColorStop(1, "#612cff");
  context.fillStyle = gradient;
  context.roundRect(750, 170, 250, 330, [20, 70, 20, 70]);
  context.fill();
  context.backdropBlur(260, 260, 600, 150, 14);
  context.restore();
  const frameStats = context.present();
  if (frameStats) {
    stats.textContent = `${frameStats.draws} draws · ${frameStats.renderPasses} passes · ${frameStats.cpuMilliseconds.toFixed(2)} ms cpu`;
  }
}

function grid(builder: DisplayListBuilder): void {
  const paint = new Paint(0.22, 0.22, 0.28, 0.42);
  paint.setStroke(1, 0, 0, 4, new Float32Array(), 0);
  for (let x = 0; x <= 1120; x += 56) builder.drawRect(x, 0, 0.01, 680, paint);
  for (let y = 0; y <= 680; y += 56) builder.drawRect(0, y, 1120, 0.01, paint);
  paint.free();
}

function gradient(
  startX: number,
  startY: number,
  endX: number,
  endY: number,
  stops: readonly (readonly [number, number, number, number, number])[],
): Paint {
  const shader = Shader.linearGradient(
    startX, startY, endX, endY,
    new Float32Array(stops.map((stop) => stop[0])),
    new Float32Array(stops.flatMap((stop) => stop.slice(1))),
    0,
  );
  const paint = new Paint(1, 1, 1, 1);
  paint.setShader(shader);
  shader.free();
  return paint;
}

window.addEventListener("unhandledrejection", showError);
window.addEventListener("error", showError);
function showError(event: PromiseRejectionEvent | ErrorEvent): void {
  error.hidden = false;
  error.textContent = String("reason" in event ? event.reason : event.error ?? event.message);
  status.textContent = "render error";
}
