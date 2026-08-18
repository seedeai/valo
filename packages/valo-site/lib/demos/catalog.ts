import {
  docsBuilder,
  docsFonts,
  docsList,
  docsPaint,
  docsParagraph,
  docsPath,
  firstRectangle,
} from './docs';
import {
  drawingBackdrop,
  drawingBlends,
  drawingColorFilters,
  drawingImageFilters,
  drawingImages,
  drawingLayers,
  drawingLists,
  drawingMaskBlur,
  drawingShaders,
  drawingShapes,
  drawingStrokes,
  drawingText,
  drawingTransforms,
} from './drawing';
import {
  drawingBackdropGallery,
  drawingBlendsGallery,
  drawingColorFiltersGallery,
  drawingImageFiltersGallery,
  drawingImagesGallery,
  drawingLayersGallery,
  drawingListsGallery,
  drawingMaskBlurGallery,
  drawingShadersGallery,
  drawingShapesGallery,
  drawingStrokesGallery,
  drawingTextGallery,
  drawingTransformsGallery,
} from './drawing-gallery';

/**
 * The scenes this site runs live.
 *
 * Each demo is the SOURCE TEXT, not a function: a landing card, a docs widget,
 * and the playground all evaluate the same string, so what the editor opens is
 * provably what the card ran.
 * Each is a real ES module — it imports from `valo-web/raw` and default-exports
 * its draw function — because a visitor reading one should be reading code they
 * could paste into their own project, not a body with names already in scope.
 *
 * valo's objects own memory on the wasm side, so every scene frees what it
 * makes at the end of the frame, and anything kept across frames is freed in
 * `dispose`. That bookkeeping stays in the demos on purpose: it is part of
 * what using the raw API is like.
 *
 * They are written against the raw API rather than the Canvas2D layer, which is
 * a compatibility surface and a strict subset: most of what is below has no
 * Canvas2D spelling at all.
 *
 * Landing cards leave the top ~48 design units clear — the card label sits
 * there.
 */

export interface Demo {
  readonly id: string;
  readonly title: string;
  /** What this shows that a traditional Canvas2D cannot say. */
  readonly beyondCanvas2D: string;
  /** The golden or example this scene is drawn from. */
  readonly grounding: string;
  readonly source: string;
}

const wordmark = `// Crisp type over a breathing field. The aurora is three coverage-blurred
// vector shapes; the contour lines swim slowly through it; the word carries
// its own soft gradient, one register above the field behind it.
import { BlurStyle, Paint, Paragraph, Path, Shader, TextAlign } from 'valo-web/raw';

let name;

export function dispose() {
  name?.free();
  name = undefined;
}

function contour(builder, W, y, amplitude, paint) {
  const line = new Path();
  line.moveTo(-0.08 * W, y);
  line.bezierCurveTo(0.28 * W, y + amplitude, 0.64 * W, y - amplitude, 1.08 * W, y);
  builder.drawPath(line, 0, paint);
  line.free();
}

/** @param {import('./setup').Scene} scene */
export default function draw(scene) {
  const { builder, width: W, height: H, time, fonts } = scene;

  const bg = new Paint(1, 1, 1, 1);
  const ramp = Shader.linearGradient(
    0, 0, 0, H,
    new Float32Array([0, 0.65, 1]),
    new Float32Array([
      0.043, 0.043, 0.055, 1,
      0.082, 0.090, 0.110, 1,
      0.043, 0.043, 0.055, 1,
    ]),
    0,
  );
  bg.setShader(ramp);
  builder.drawRect(0, 0, W, H, bg);

  // The aurora: blurred fields on a shallow diagonal behind the word.
  const fields = [
    { colour: [0.498, 0.624, 1], alpha: 0.20, cx: 0.30, cy: 0.50, r: 0.20, phase: 0 },
    { colour: [0.620, 0.500, 0.880], alpha: 0.17, cx: 0.55, cy: 0.62, r: 0.24, phase: 2.1 },
    { colour: [0.400, 0.788, 0.761], alpha: 0.15, cx: 0.78, cy: 0.46, r: 0.19, phase: 4.2 },
  ];
  for (const field of fields) {
    const sway = Math.sin((Math.PI * 2 * time) / 2.2 + field.phase);
    const blob = new Path();
    blob.arc(
      field.cx * W + 0.055 * W * sway,
      field.cy * H + 0.025 * H * Math.cos((Math.PI * 2 * time) / 2.2 + field.phase),
      field.r * W, 0, Math.PI * 2,
    );
    const wash = new Paint(field.colour[0], field.colour[1], field.colour[2], field.alpha);
    wash.setMaskBlur(26, BlurStyle.Normal);
    builder.drawPath(blob, 0, wash);
    wash.free();
    blob.free();
  }

  // The contours swim: each line drifts and bobs on its own phase.
  const topo = new Paint(0.467, 0.502, 0.549, 0.13);
  topo.setStroke(1.2, 1, 1, 4, new Float32Array([]), 0);
  for (let index = 0; index < 8; index += 1) {
    const y = 0.29 * H + (index * 0.50 * H) / 7;
    const phase = (Math.PI * 2 * time) / 2.4 + index * 0.7;
    builder.save();
    builder.translate(0.04 * W * Math.sin(phase), 3 * Math.sin(phase * 0.8));
    contour(builder, W, y, (index % 2 === 0 ? 1 : -1) * 0.055 * H * (1 + 0.25 * Math.sin(phase)), topo);
    builder.restore();
  }

  name ??= new Paragraph(
    fonts, 'valo', 'Fira Sans',
    126, 600, false,
    1, 1, 1, 1,
    100, true, 0,
    -3, 0, 1,
    TextAlign.Left, 0, 0, '',
    W, false,
  );
  name.layout(W);
  const x = 0.075 * W;
  const y = 0.56 * H - name.height * 0.5;

  // The word's own gradient: paper into cool paper and back, one register
  // brighter than the field, shimmering slowly across the glyphs.
  const sheen = Shader.linearGradient(
    0, 0, 0.5 * W, 0.14 * H,
    new Float32Array([0, 0.5, 1]),
    new Float32Array([
      0.930, 0.940, 0.905, 0.97,
      0.790, 0.845, 0.975, 0.97,
      0.930, 0.940, 0.905, 0.97,
    ]),
    2,
  );
  sheen.setTransform(new Float32Array([1, 0, 0, 1, time * 0.075 * W, 0]));
  const ink = new Paint(1, 1, 1, 1);
  ink.setShader(sheen);
  builder.drawParagraphWith(name, x, y, ink);

  // The full stop: the card's one lime mark.
  builder.save();
  builder.translate(x + name.maxIntrinsicWidth + 16, y + name.height - 12);
  builder.rotate(Math.PI / 4);
  const stop = new Paint(0.784, 1, 0.239, 1);
  builder.drawRect(-4, -4, 8, 8, stop);
  builder.restore();

  stop.free();
  ink.free();
  sheen.free();
  topo.free();
  bg.free();
  ramp.free();
}`;

const paragraphLayout = `// Canvas2D has no line breaking at all: fillText draws one run on one line
// and where it wraps is your problem. This column was shaped ONCE; every
// frame only re-wraps it to the moving measure - the cheap half of text.
import { Paint, Paragraph, Shader, TextAlign } from 'valo-web/raw';

let overline;
let copy;

export function dispose() {
  overline?.free();
  copy?.free();
  overline = copy = undefined;
}

/** @param {import('./setup').Scene} scene */
export default function draw(scene) {
  const { builder, width: W, height: H, time, fonts } = scene;

  const bg = new Paint(1, 1, 1, 1);
  const ramp = Shader.linearGradient(
    0, 0, 0, H,
    new Float32Array([0, 0.65, 1]),
    new Float32Array([
      0.043, 0.043, 0.055, 1,
      0.082, 0.090, 0.110, 1,
      0.043, 0.043, 0.055, 1,
    ]),
    0,
  );
  bg.setShader(ramp);
  builder.drawRect(0, 0, W, H, bg);

  // Fixed baseline rules. The text takes and releases them as it reflows.
  const rules = new Paint(0.467, 0.502, 0.549, 0.08);
  for (let index = 0; index < 8; index += 1) {
    builder.drawRect(0.12 * W, 0.335 * H + index * 25, 0.74 * W, 1, rules);
  }

  overline ??= new Paragraph(
    fonts, 'THE MEASURE', 'Fira Sans',
    13, 600, false,
    0.467, 0.502, 0.549, 1,
    100, true, 0,
    1.4, 0, 1,
    TextAlign.Left, 0, 0, '',
    W, false,
  );
  builder.drawParagraph(overline, 0.12 * W, 0.20 * H);

  copy ??= new Paragraph(
    fonts,
    'A line is not stored in the sentence. It appears when language, ' +
      'measure and rhythm meet. Change the column and the paragraph finds ' +
      'its shape again.',
    'Fira Sans',
    17, 400, false,
    0.914, 0.929, 0.894, 0.88,
    100, true, 0,
    0, 0, 1.48,
    TextAlign.Left, 0, 0, '',
    W, false,
  );

  // The one moving thing: the measure. Re-layout is all this costs.
  const measure = 0.57 * W + 0.13 * W * Math.sin((Math.PI * 2 * time) / 2.4);
  copy.layout(measure);
  builder.drawParagraph(copy, 0.12 * W, 0.29 * H);

  // The measuring rail, and a lime marker riding the paragraph's live bottom.
  const railX = 0.12 * W + measure + 0.035 * W;
  const railPaint = new Paint(0.467, 0.502, 0.549, 0.22);
  builder.drawRect(railX, 0.28 * H, 1, 0.56 * H, railPaint);
  const marker = new Paint(0.784, 1, 0.239, 1);
  builder.drawRect(railX - 3, 0.29 * H + copy.height - 3.5, 7, 7, marker);

  marker.free();
  railPaint.free();
  rules.free();
  bg.free();
  ramp.free();
}`;

const maskBlurStyles = `// A specimen sheet. Four identical blades, ONE colour, four mask-blur
// styles: the blur runs on each blade's coverage, so all four stay single
// analytic quads. Canvas2D's shadowBlur is Normal and nothing else.
import { BlurStyle, Paint, Paragraph, Shader, TextAlign } from 'valo-web/raw';

const NAMES = ['NORMAL', 'SOLID', 'INNER', 'OUTER'];
let labels;

export function dispose() {
  if (labels) for (const label of labels) label.free();
  labels = undefined;
}

/** @param {import('./setup').Scene} scene */
export default function draw(scene) {
  const { builder, width: W, height: H, time, fonts } = scene;

  const bg = new Paint(1, 1, 1, 1);
  const ramp = Shader.linearGradient(
    0, 0, 0, H,
    new Float32Array([0, 0.65, 1]),
    new Float32Array([
      0.043, 0.043, 0.055, 1,
      0.082, 0.090, 0.110, 1,
      0.043, 0.043, 0.055, 1,
    ]),
    0,
  );
  bg.setShader(ramp);
  builder.drawRect(0, 0, W, H, bg);

  labels ??= NAMES.map((text) => new Paragraph(
    fonts, text, 'Fira Sans',
    10, 600, false,
    0.467, 0.502, 0.549, 0.62,
    100, true, 0,
    1.1, 0, 1,
    TextAlign.Left, 0, 0, '',
    W, false,
  ));

  const rows = [0.29, 0.45, 0.61, 0.77];
  const styles = [BlurStyle.Normal, BlurStyle.Solid, BlurStyle.Inner, BlurStyle.Outer];
  const tilts = [-3, 2, -2, 3];
  const alphas = [0.82, 0.92, 0.82, 0.78];
  const sigma = 5.5 + 9.5 * (0.5 + 0.5 * Math.sin((Math.PI * 2 * time) / 2.4));

  const baseline = new Paint(0.467, 0.502, 0.549, 0.10);
  const radii = new Float32Array([12, 12, 4, 4]);

  for (let index = 0; index < 4; index += 1) {
    const rowY = rows[index] * H;
    builder.drawRect(0.30 * W, rowY - 0.5, 0.58 * W, 1, baseline);
    builder.drawParagraph(labels[index], 0.10 * W, rowY - 5);

    const blade = new Paint(0.604, 0.847, 0.816, alphas[index]);
    blade.setMaskBlur(sigma, styles[index]);
    builder.save();
    builder.translate(0.58 * W, rowY);
    builder.rotate((tilts[index] * Math.PI) / 180);
    builder.drawRoundedRect(-0.24 * W, -11.5, 0.48 * W, 23, radii, blade);
    builder.restore();
    blade.free();
  }

  // One lime scan tick, sweeping the sheet once per cycle.
  const scanX = 0.31 * W + ((time / 2.4) % 1) * 0.55 * W;
  const tick = new Paint(0.784, 1, 0.239, 0.9);
  for (const row of rows) builder.drawRect(scanX, row * H - 6, 1, 12, tick);

  tick.free();
  baseline.free();
  bg.free();
  ramp.free();
}`;

const backdropBlur = `// One fixed lens over moving ribbons. backdropBlur breaks the render pass,
// snapshots what is already recorded beneath the region, blurs it and
// composites it back. Canvas2D's filter only ever applies to what you are
// about to draw, never to what is already there.
import { BlurStyle, ClipOp, FillRule, Paint, Path, Shader } from 'valo-web/raw';

/** @param {import('./setup').Scene} scene */
export default function draw(scene) {
  const { builder, width: W, height: H, time } = scene;

  const bg = new Paint(1, 1, 1, 1);
  const ramp = Shader.linearGradient(
    0, 0, 0, H,
    new Float32Array([0, 0.65, 1]),
    new Float32Array([
      0.043, 0.043, 0.055, 1,
      0.082, 0.090, 0.110, 1,
      0.043, 0.043, 0.055, 1,
    ]),
    0,
  );
  bg.setShader(ramp);
  builder.drawRect(0, 0, W, H, bg);

  // Fixed diagonal rules: the ground the ribbons travel over.
  const rulePaint = new Paint(0.467, 0.502, 0.549, 0.10);
  rulePaint.setStroke(1, 0, 0, 4, new Float32Array([]), 0);
  for (let index = 0; index < 11; index += 1) {
    const y = 0.20 * H + (index * 0.68 * H) / 10;
    const rule = new Path();
    rule.moveTo(-0.10 * W, y);
    rule.lineTo(1.10 * W, y + 0.22 * H);
    builder.drawPath(rule, 0, rulePaint);
    rule.free();
  }

  // Three ribbons drift as a group; each carries its own soft copy beneath.
  const dx = 0.17 * W * Math.sin((Math.PI * 2 * time) / 2.8);
  const dy = 0.025 * H * Math.cos((Math.PI * 2 * time) / 2.8);
  const ribbons = [
    { colour: [0.498, 0.624, 1], alpha: 0.75, width: 26 },
    { colour: [0.4, 0.788, 0.761], alpha: 0.6, width: 21 },
    { colour: [0.824, 0.561, 0.463], alpha: 0.5, width: 18 },
  ];
  builder.save();
  builder.translate(dx, dy);
  for (let index = 0; index < 3; index += 1) {
    const { colour, alpha, width: strokeWidth } = ribbons[index];
    const ribbon = new Path();
    ribbon.moveTo(-0.4 * W, 0.38 * H + index * 0.17 * H);
    ribbon.bezierCurveTo(
      0.30 * W, 0.72 * H - index * 0.05 * H,
      0.68 * W, 0.36 * H + index * 0.11 * H,
      1.4 * W, 0.30 * H + index * 0.21 * H,
    );
    const soft = new Paint(colour[0], colour[1], colour[2], 0.14);
    soft.setStroke(strokeWidth, 1, 1, 4, new Float32Array([]), 0);
    soft.setMaskBlur(18, BlurStyle.Normal);
    builder.drawPath(ribbon, 0, soft);
    const crisp = new Paint(colour[0], colour[1], colour[2], alpha);
    crisp.setStroke(strokeWidth, 1, 1, 4, new Float32Array([]), 0);
    builder.drawPath(ribbon, 0, crisp);
    crisp.free();
    soft.free();
    ribbon.free();
  }
  builder.restore();

  // The lens. It never moves; the crisp/blurred boundary is the demo.
  const lens = new Path();
  lens.roundRect(0.16 * W, 0.31 * H, 0.70 * W, 0.46 * H, new Float32Array([18, 42, 18, 42]), false);
  builder.save();
  builder.clipPath(lens, FillRule.NonZero, ClipOp.Intersect);
  builder.backdropBlur(0.16 * W, 0.31 * H, 0.70 * W, 0.46 * H, 17);
  const tint = new Paint(0.067, 0.075, 0.090, 0.30);
  builder.drawRect(0.16 * W, 0.31 * H, 0.70 * W, 0.46 * H, tint);
  builder.restore();

  const rim = new Paint(0.914, 0.929, 0.894, 0.15);
  rim.setStroke(1, 0, 0, 4, new Float32Array([]), 0);
  builder.drawPath(lens, 0, rim);

  // A sharp reticle inside the glass, so the blur has a crisp reference.
  const reticle = new Paint(0.784, 1, 0.239, 0.9);
  builder.drawRect(0.24 * W - 12, 0.67 * H - 0.5, 24, 1, reticle);
  const dot = new Path();
  dot.arc(0.24 * W, 0.67 * H, 2.5, 0, Math.PI * 2);
  const dotPaint = new Paint(0.914, 0.929, 0.894, 1);
  builder.drawPath(dot, 0, dotPaint);

  dotPaint.free();
  dot.free();
  reticle.free();
  rim.free();
  tint.free();
  lens.free();
  rulePaint.free();
  bg.free();
  ramp.free();
}`;

const gradientSpreads = `// One linear gradient, half a card long. Everything past its two endpoints
// is the SPREAD MODE: Pad holds the edge colours, Repeat tiles the ramp,
// Reflect mirrors it back and forth. Canvas2D gradients only ever pad -
// the lower two bands have no spelling there.
import { Paint, Paragraph, Shader, SpreadMode, TextAlign } from 'valo-web/raw';

const NAMES = ['PAD', 'REPEAT', 'REFLECT'];
let labels;

export function dispose() {
  if (labels) for (const label of labels) label.free();
  labels = undefined;
}

/** @param {import('./setup').Scene} scene */
export default function draw(scene) {
  const { builder, width: W, height: H, time } = scene;

  const bg = new Paint(0.043, 0.043, 0.055, 1);
  builder.drawRect(0, 0, W, H, bg);

  labels ??= NAMES.map((text) => new Paragraph(
    scene.fonts, text, 'Fira Sans',
    9.5, 600, false,
    0.467, 0.502, 0.549, 0.64,
    100, true, 0,
    1.2, 0, 1,
    TextAlign.Left, 0, 0, '',
    W, false,
  ));

  const modes = [SpreadMode.Pad, SpreadMode.Repeat, SpreadMode.Reflect];
  // Muted silk stops - dark navy up through ice, back down to deep sea.
  // Different end colours keep Pad legible; every colour stays mid-dark so
  // Repeat's seam reads as a crisp edge rather than a glitch.
  const offsets = new Float32Array([0, 0.45, 0.75, 1]);
  const colours = new Float32Array([
    0.070, 0.090, 0.165, 1,
    0.460, 0.600, 0.950, 1,
    0.380, 0.310, 0.660, 1,
    0.205, 0.375, 0.370, 1,
  ]);

  const gx = 0.16 * W;
  const span = 0.5 * W;
  const shift = 0.12 * W * Math.sin((Math.PI * 2 * time) / 2.6);

  for (let index = 0; index < 3; index += 1) {
    const labelY = 54 + index * 92;
    const panelY = labelY + 14;

    builder.drawParagraph(labels[index], 0.06 * W, labelY);

    const ramp = Shader.linearGradient(gx, 0, gx + span, 0, offsets, colours, modes[index]);
    ramp.setTransform(new Float32Array([1, 0, 0, 1, shift, 0]));
    const band = new Paint(1, 1, 1, 1);
    band.setShader(ramp);
    builder.drawRect(0, panelY, W, 68, band);
    band.free();
    ramp.free();
  }

  // The vector itself, marked once: two lime ticks riding its live endpoints.
  const tick = new Paint(0.784, 1, 0.239, 0.8);
  builder.drawRect(gx + shift - 0.5, 56, 1, 8, tick);
  builder.drawRect(gx + span + shift - 0.5, 56, 1, 8, tick);

  tick.free();
  bg.free();
}`;

const differenceClip = `// An eclipse cut from an arch. The circle is a DIFFERENCE clip: it subtracts
// itself from the clip stack, and the fill and every contour line vanish
// through it together. Canvas2D's clip() only ever intersects.
import { ClipOp, FillRule, Paint, Path, Shader } from 'valo-web/raw';

/** @param {import('./setup').Scene} scene */
export default function draw(scene) {
  const { builder, width: W, height: H, time } = scene;

  const bg = new Paint(1, 1, 1, 1);
  const ramp = Shader.linearGradient(
    0, 0, 0, H,
    new Float32Array([0, 0.65, 1]),
    new Float32Array([
      0.043, 0.043, 0.055, 1,
      0.082, 0.090, 0.110, 1,
      0.043, 0.043, 0.055, 1,
    ]),
    0,
  );
  bg.setShader(ramp);
  builder.drawRect(0, 0, W, H, bg);

  // A fixed orbit, drawn UNDER the clips: it shows through the hole.
  const orbit = new Path();
  orbit.ellipse(0.54 * W, 0.53 * H, 0.23 * W, 0.18 * H, 0, 0, Math.PI * 2);
  const orbitPaint = new Paint(0.914, 0.929, 0.894, 0.10);
  orbitPaint.setStroke(1, 0, 0, 4, new Float32Array([]), 0);
  builder.drawPath(orbit, 0, orbitPaint);

  builder.save();

  const arch = new Path();
  arch.moveTo(0.18 * W, 0.87 * H);
  arch.lineTo(0.18 * W, 0.48 * H);
  arch.arc(0.5 * W, 0.48 * H, 0.32 * W, Math.PI, Math.PI);
  arch.lineTo(0.82 * W, 0.87 * H);
  arch.close();
  builder.clipPath(arch, FillRule.NonZero, ClipOp.Intersect);

  const hx = 0.54 * W + 0.13 * W * Math.sin((Math.PI * 2 * time) / 2.8);
  const hy = 0.53 * H + 0.075 * H * Math.cos((Math.PI * 2 * time) / 2.8);
  const hr = 0.155 * W + 0.012 * W * Math.sin((Math.PI * 4 * time) / 2.8);
  const hole = new Path();
  hole.arc(hx, hy, hr, 0, Math.PI * 2);
  builder.clipPath(hole, FillRule.NonZero, ClipOp.Difference);

  const satin = Shader.linearGradient(
    0.12 * W, 0.88 * H, 0.78 * W, 0.22 * H,
    new Float32Array([0, 0.48, 0.86, 0.94, 1]),
    new Float32Array([
      0.110, 0.141, 0.212, 1,
      0.357, 0.349, 0.376, 1,
      0.855, 0.871, 0.839, 1,
      0.855, 0.871, 0.839, 1,
      0.784, 1, 0.239, 1,
    ]),
    0,
  );
  const fill = new Paint(1, 1, 1, 1);
  fill.setShader(satin);
  builder.drawRect(0, 0, W, H, fill);

  // Contour lines, clipped with everything else - the subtraction is what
  // makes them stop mid-stroke at the hole's edge.
  builder.save();
  builder.translate(0.5 * W, 0.5 * H);
  builder.rotate(-Math.PI / 4);
  const dark = new Paint(0, 0, 0, 0.24);
  const light = new Paint(0.784, 1, 0.239, 0.30);
  for (let line = -7; line <= 7; line += 1) {
    builder.drawRect(-0.75 * W, line * 13 - 0.5, 1.5 * W, 1, line % 5 === 0 ? light : dark);
  }
  light.free();
  dark.free();
  builder.restore();

  builder.restore();

  fill.free();
  satin.free();
  hole.free();
  arch.free();
  orbitPaint.free();
  orbit.free();
  bg.free();
  ramp.free();
}`;

const sweepGradient = `// An off-centre fan. The geometry never moves: the sweep gradient rotates
// around the pivot, and a colour matrix on the paint tilts the whole palette
// warm and cool. Canvas2D has neither a conic paint nor a paint matrix.
import { BlurStyle, ColorFilter, Paint, Path, Shader } from 'valo-web/raw';

const IDENTITY = [1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0];
const CROSS_MIX = [
  0.72, 0.18, 0.06, 0, 0.015,
  0.08, 0.82, 0.10, 0, 0,
  0.16, 0.10, 0.72, 0, 0.018,
  0, 0, 0, 1, 0,
];

/** @param {import('./setup').Scene} scene */
export default function draw(scene) {
  const { builder, width: W, height: H, time } = scene;

  const bg = new Paint(1, 1, 1, 1);
  const ramp = Shader.linearGradient(
    0, 0, 0, H,
    new Float32Array([0, 0.65, 1]),
    new Float32Array([
      0.043, 0.043, 0.055, 1,
      0.082, 0.090, 0.110, 1,
      0.043, 0.043, 0.055, 1,
    ]),
    0,
  );
  bg.setShader(ramp);
  builder.drawRect(0, 0, W, H, bg);

  const px = 0.23 * W;
  const py = 0.82 * H;
  const DEG = Math.PI / 180;

  // Fixed hairline arcs: the instrument's registration marks.
  const hairline = new Paint(0.914, 0.929, 0.894, 0.11);
  hairline.setStroke(1, 0, 0, 4, new Float32Array([]), 0);
  for (const radius of [0.28, 0.47, 0.66]) {
    const arcPath = new Path();
    arcPath.arc(px, py, radius * W, -82 * DEG, 80 * DEG);
    builder.drawPath(arcPath, 0, hairline);
    arcPath.free();
  }

  // The fan spans -76deg..-8deg of the wheel; the -124deg phase parks the
  // coloured band inside the blades at t=0 instead of a paper stretch.
  const sweep = Shader.sweepGradient(
    px, py, (Math.PI * 2 * time) / 3.0 - 124 * Math.PI / 180,
    new Float32Array([0, 0.10, 0.22, 0.34, 0.66, 1]),
    new Float32Array([
      0.886, 0.910, 0.875, 1,
      0.886, 0.910, 0.875, 1,
      0.784, 1, 0.239, 1,
      0.427, 0.553, 0.741, 1,
      0.780, 0.561, 0.447, 1,
      0.886, 0.910, 0.875, 1,
    ]),
  );

  // The palette tilt: identity blended toward a warm/cool cross-mix.
  const u = 0.5 + 0.5 * Math.sin((Math.PI * 2 * time) / 2.6);
  const mix = new Float32Array(20);
  for (let cell = 0; cell < 20; cell += 1) {
    mix[cell] = IDENTITY[cell] * (1 - u) + CROSS_MIX[cell] * u;
  }
  const tilt = ColorFilter.matrix(mix);

  const rIn = 0.11 * W;
  const rOut = 0.74 * W;

  function wedge(a0, a1, inner, outer) {
    const blade = new Path();
    blade.moveTo(px + Math.cos(a0) * inner, py + Math.sin(a0) * inner);
    blade.lineTo(px + Math.cos(a0) * outer, py + Math.sin(a0) * outer);
    blade.arc(px, py, outer, a0, a1 - a0);
    blade.lineTo(px + Math.cos(a1) * inner, py + Math.sin(a1) * inner);
    blade.arc(px, py, inner, a1, a0 - a1);
    blade.close();
    return blade;
  }

  // Atmosphere: only the fan's outer band, softly, beneath the crisp blades.
  const glow = wedge(-76 * DEG, -2.5 * DEG, 0.60 * W, rOut);
  const glowPaint = new Paint(1, 1, 1, 0.15);
  glowPaint.setShader(sweep);
  glowPaint.setMaskBlur(18, BlurStyle.Normal);
  builder.drawPath(glow, 0, glowPaint);

  const blades = new Paint(1, 1, 1, 1);
  blades.setShader(sweep);
  blades.setColorFilter(tilt);
  for (let index = 0; index < 9; index += 1) {
    const a0 = (-76 + index * 7.5) * DEG;
    const blade = wedge(a0, a0 + 5.5 * DEG, rIn, rOut);
    builder.drawPath(blade, 0, blades);
    blade.free();
  }

  const pivot = new Path();
  pivot.arc(px, py, 2.5, 0, Math.PI * 2);
  const pivotPaint = new Paint(0.784, 1, 0.239, 1);
  builder.drawPath(pivot, 0, pivotPaint);

  pivotPaint.free();
  pivot.free();
  blades.free();
  glowPaint.free();
  glow.free();
  tilt.free();
  sweep.free();
  hairline.free();
  bg.free();
  ramp.free();
}`;

const blendModes = `// These modes have to read the destination, so they cannot be a pipeline
// blend state. Each one breaks the pass: the target resolves into a snapshot
// and one shader computes the blend and the composite together.
import { BlendMode, Paint, Shader } from 'valo-web/raw';

/** @param {import('./setup').Scene} scene */
export default function draw(scene) {
  const { builder, width, height, time } = scene;

  const base = Shader.linearGradient(
    0, 0, width, height,
    new Float32Array([0, 0.5, 1]),
    new Float32Array([0.95, 0.55, 0.2, 1, 0.85, 0.2, 0.45, 1, 0.2, 0.35, 0.9, 1]),
    0,
  );
  const basePaint = new Paint(1, 1, 1, 1);
  basePaint.setShader(base);
  builder.drawRect(0, 0, width, height, basePaint);

  const modes = [
    BlendMode.Multiply, BlendMode.Screen, BlendMode.Overlay,
    BlendMode.ColorDodge, BlendMode.Difference, BlendMode.Exclusion,
    BlendMode.Hue, BlendMode.SoftLight, BlendMode.Luminosity,
  ];
  const cellWidth = width / 3;
  const cellHeight = height / 3;

  for (let index = 0; index < modes.length; index += 1) {
    const x = (index % 3) * cellWidth;
    const y = Math.floor(index / 3) * cellHeight;
    const pulse = 0.5 + 0.5 * Math.sin(time * 1.1 + index * 0.6);
    const paint = new Paint(0.784, 1, 0.239, 0.5 + pulse * 0.45);
    paint.setBlendMode(modes[index]);
    builder.drawRoundedRect(
      x + cellWidth * 0.12, y + cellHeight * 0.12,
      cellWidth * 0.76, cellHeight * 0.76,
      new Float32Array([cellHeight * 0.18]),
      paint,
    );
    paint.free();
  }

  basePaint.free();
  base.free();
}`;

const groupOpacity = `// Left: three circles at per-draw alpha, so every overlap blends twice and
// darkens. Right: the same circles inside a save layer - they render opaque
// into one texture and the alpha applies ONCE, to the group.
// Canvas2D's globalAlpha only ever does the left-hand thing.
import { FillRule, Paint, Path } from 'valo-web/raw';

/** @param {import('./setup').Scene} scene */
export default function draw(scene) {
  const { builder, width, height, time } = scene;

  const backdrop = new Paint(0.043, 0.043, 0.055, 1);
  builder.drawRect(0, 0, width, height, backdrop);

  const alpha = 0.3 + 0.35 * (0.5 + 0.5 * Math.sin(time * 0.9));

  function trio(centreX, centreY, paint) {
    for (const offset of [[0, -34], [-30, 22], [30, 22]]) {
      const circle = new Path();
      circle.arc(centreX + offset[0], centreY + offset[1], 46, 0, Math.PI * 2);
      builder.drawPath(circle, FillRule.NonZero, paint);
      circle.free();
    }
  }

  const perDraw = new Paint(0.784, 1, 0.239, alpha);
  trio(width * 0.28, height * 0.5, perDraw);

  const groupAlpha = new Paint(0, 0, 0, alpha);
  builder.saveLayer(groupAlpha);
  const opaque = new Paint(0.49, 0.29, 1, 1);
  trio(width * 0.72, height * 0.5, opaque);
  builder.restore();

  opaque.free();
  groupAlpha.free();
  perDraw.free();
  backdrop.free();
}`;

const strokes = `// A signal braid. The geometry never moves - only each stroke's dash PHASE
// advances. Dashing runs on the flattened contour before the stroker, so
// every dash is real geometry: round caps on the short marks, square ends
// and bevelled corners on the angular track.
import { BlurStyle, Cap, Join, Paint, Path, Shader } from 'valo-web/raw';

/** @param {import('./setup').Scene} scene */
export default function draw(scene) {
  const { builder, width: W, height: H, time } = scene;

  const bg = new Paint(1, 1, 1, 1);
  const ramp = Shader.linearGradient(
    0, 0, 0, H,
    new Float32Array([0, 0.65, 1]),
    new Float32Array([
      0.043, 0.043, 0.055, 1,
      0.082, 0.090, 0.110, 1,
      0.043, 0.043, 0.055, 1,
    ]),
    0,
  );
  bg.setShader(ramp);
  builder.drawRect(0, 0, W, H, bg);

  const wide = new Path();
  wide.moveTo(-0.08 * W, 0.76 * H);
  wide.bezierCurveTo(0.20 * W, 0.82 * H, 0.18 * W, 0.30 * H, 0.50 * W, 0.37 * H);
  wide.bezierCurveTo(0.74 * W, 0.39 * H, 0.82 * W, 0.19 * H, 1.08 * W, 0.31 * H);

  const fine = new Path();
  fine.moveTo(-0.08 * W, 0.51 * H);
  fine.bezierCurveTo(0.22 * W, 0.28 * H, 0.37 * W, 0.79 * H, 0.62 * W, 0.68 * H);
  fine.bezierCurveTo(0.78 * W, 0.60 * H, 0.83 * W, 0.84 * H, 1.08 * W, 0.71 * H);

  const angular = new Path();
  angular.moveTo(0.04 * W, 0.88 * H);
  angular.lineTo(0.25 * W, 0.67 * H);
  angular.lineTo(0.43 * W, 0.76 * H);
  angular.lineTo(0.59 * W, 0.47 * H);
  angular.lineTo(0.80 * W, 0.53 * H);
  angular.lineTo(1.04 * W, 0.23 * H);

  // Each path first as its stationary track, then its moving signal over it.
  const guide = new Paint(0.467, 0.502, 0.549, 0.12);
  guide.setStroke(1, Cap.Round, Join.Round, 4, new Float32Array([]), 0);
  for (const track of [wide, fine, angular]) builder.drawPath(track, 0, guide);

  const dashWide = new Float32Array([52, 18, 6, 44]);
  const soft = new Paint(0.886, 0.910, 0.875, 0.11);
  soft.setStroke(8.5, Cap.Round, Join.Round, 4, dashWide, -48 * time);
  soft.setMaskBlur(11, BlurStyle.Normal);
  builder.drawPath(wide, 0, soft);
  const paper = new Paint(0.886, 0.910, 0.875, 0.88);
  paper.setStroke(8.5, Cap.Round, Join.Round, 4, dashWide, -48 * time);
  builder.drawPath(wide, 0, paper);

  const sea = new Paint(0.4, 0.788, 0.761, 0.72);
  sea.setStroke(4.5, Cap.Round, Join.Round, 4, new Float32Array([30, 14, 3, 14, 3, 56]), 40 * time);
  builder.drawPath(fine, 0, sea);

  const ice = new Paint(0.498, 0.624, 1, 0.62);
  ice.setStroke(3.5, Cap.Square, Join.Bevel, 3, new Float32Array([36, 12, 5, 12, 5, 50]), -50 * time);
  builder.drawPath(angular, 0, ice);

  // Registration: a lime pin the braid weaves around, and its hairline cross.
  const cx = 0.61 * W;
  const cy = 0.55 * H;
  const tick = new Paint(0.914, 0.929, 0.894, 0.18);
  builder.drawRect(cx - 12, cy - 0.5, 7, 1, tick);
  builder.drawRect(cx + 5, cy - 0.5, 7, 1, tick);
  builder.drawRect(cx - 0.5, cy - 12, 1, 7, tick);
  builder.drawRect(cx - 0.5, cy + 5, 1, 7, tick);
  builder.save();
  builder.translate(cx, cy);
  builder.rotate(Math.PI / 4);
  const pin = new Paint(0.784, 1, 0.239, 1);
  builder.drawRect(-3, -3, 6, 6, pin);
  builder.restore();

  const lane = new Paint(0.467, 0.502, 0.549, 0.18);
  for (let index = 0; index < 3; index += 1) {
    builder.drawRect(0.05 * W + index * 7, 0.92 * H, 1, 6, lane);
  }

  lane.free();
  pin.free();
  tick.free();
  ice.free();
  sea.free();
  paper.free();
  soft.free();
  guide.free();
  angular.free();
  fine.free();
  wide.free();
  bg.free();
  ramp.free();
}`;

const gradientText = `// Text is an ordinary render object here. The paragraph shapes ONCE and is
// kept; drawParagraphWith fills its glyphs with any paint, gradient included.
// The glyph atlas lives on the DEVICE, so every card on this page that sets
// type rasters into the same one.
import { Paint, Paragraph, Shader, TextAlign } from 'valo-web/raw';

let headline;

export function dispose() {
  headline?.free();
  headline = undefined;
}

/** @param {import('./setup').Scene} scene */
export default function draw(scene) {
  const { builder, width, height, time, fonts } = scene;

  const backdrop = new Paint(0.043, 0.043, 0.055, 1);
  builder.drawRect(0, 0, width, height, backdrop);

  headline ??= new Paragraph(
    fonts, 'Type at\\nGPU speed', 'Fira Sans',
    54, 600, false,
    1, 1, 1, 1,
    100, true, 0,
    0, 0, 0.98,
    TextAlign.Left, 0, 0, '',
    width * 0.82, false,
  );
  // Shaping already happened. Re-wrapping is the cheap half, so the paragraph
  // survives a resize without paying for it again.
  headline.layout(width * 0.82);

  const sweep = Shader.linearGradient(
    width * 0.08, 0, width * 0.9, height,
    new Float32Array([0, 0.5, 1]),
    new Float32Array([0.784, 1, 0.239, 1, 1, 0.34, 0.47, 1, 0.49, 0.29, 1, 1]),
    0,
  );
  sweep.setTransform(new Float32Array([1, 0, 0, 1, Math.sin(time * 0.8) * 60, 0]));
  const ink = new Paint(1, 1, 1, 1);
  ink.setShader(sweep);

  builder.drawParagraphWith(headline, width * 0.09, height * 0.5 - headline.height * 0.5, ink);

  const rule = new Paint(0.784, 1, 0.239, 1);
  builder.drawRect(
    width * 0.09, height * 0.5 + headline.height * 0.5 + 14,
    headline.maxIntrinsicWidth, 3, rule,
  );

  rule.free();
  ink.free();
  sweep.free();
  backdrop.free();
}`;

const filterChain = `// Image filters compose. This one is a drop shadow with a blur behind it,
// built as a chain and hung on the paint - the layer is filtered as a whole
// rather than each shape carrying its own shadow.
import { FillRule, ImageFilter, Paint, Path } from 'valo-web/raw';

/** @param {import('./setup').Scene} scene */
export default function draw(scene) {
  const { builder, width, height, time } = scene;

  const backdrop = new Paint(0.043, 0.043, 0.055, 1);
  builder.drawRect(0, 0, width, height, backdrop);

  const wobble = Math.sin(time * 0.9);
  const shadow = ImageFilter.dropShadow(10 + wobble * 6, 14, 9, 9, 0.49, 0.29, 1, 0.85);
  const soften = ImageFilter.blur(1.4, 1.4);
  const chain = ImageFilter.compose(shadow, soften);

  const filtered = new Paint(1, 1, 1, 1);
  filtered.setImageFilter(chain);
  builder.saveLayer(filtered);

  const star = new Path();
  const centreX = width * 0.5;
  const centreY = height * 0.46;
  for (let point = 0; point < 5; point += 1) {
    const angle = -Math.PI / 2 + point * 4 * Math.PI / 5 + time * 0.35;
    const x = centreX + Math.cos(angle) * 96;
    const y = centreY + Math.sin(angle) * 96;
    if (point === 0) star.moveTo(x, y); else star.lineTo(x, y);
  }
  star.close();

  const gold = new Paint(0.95, 0.78, 0.3, 1);
  builder.drawPath(star, FillRule.NonZero, gold);

  const bar = new Paint(0.784, 1, 0.239, 1);
  builder.drawRoundedRect(width * 0.24, height * 0.78, width * 0.52, 22, new Float32Array([11]), bar);
  builder.restore();

  bar.free();
  gold.free();
  star.free();
  filtered.free();
  chain.free();
  soften.free();
  shadow.free();
  backdrop.free();
}`;

const retainedList = `// One recording, replayed sixteen times. The frame below is recorded into a
// display list ONCE - its bounds and depth slots worked out at record time -
// and every copy is drawDisplayListCached under a different transform.
// Canvas2D is immediate mode: there is no list to keep, nothing to replay.
import { Cap, DisplayListBuilder, Join, Paint, Path, Shader } from 'valo-web/raw';

let frame;

export function dispose() {
  frame?.free();
  frame = undefined;
}

// The recorded object: one asymmetric instrument frame in local space. The
// off-centre tab and square matter - every replay is recognisably the SAME
// recording, not generic concentric rectangles.
function recordFrame() {
  const recorder = new DisplayListBuilder();

  const outer = new Path();
  outer.roundRect(-66, -36, 132, 72, new Float32Array([18, 6, 18, 6]), false);
  const outerInk = new Paint(0.886, 0.910, 0.875, 0.34);
  outerInk.setStroke(1.5, Cap.Round, Join.Round, 4, new Float32Array([]), 0);
  recorder.drawPath(outer, 0, outerInk);

  const inner = new Path();
  inner.roundRect(-53, -25, 106, 50, new Float32Array([12, 4, 12, 4]), false);
  const innerInk = new Paint(0.498, 0.624, 1, 0.18);
  innerInk.setStroke(0.75, Cap.Round, Join.Round, 4, new Float32Array([]), 0);
  recorder.drawPath(inner, 0, innerInk);

  const tab = new Paint(0.886, 0.910, 0.875, 0.46);
  recorder.drawRect(31, 23, 18, 2, tab);
  const square = new Paint(0.467, 0.502, 0.549, 0.55);
  recorder.drawRect(-43, -1, 3, 3, square);

  const list = recorder.build();
  square.free();
  tab.free();
  innerInk.free();
  inner.free();
  outerInk.free();
  outer.free();
  recorder.free();
  return list;
}

/** @param {import('./setup').Scene} scene */
export default function draw(scene) {
  const { builder, width: W, height: H, time } = scene;
  const DEG = Math.PI / 180;

  const bg = new Paint(1, 1, 1, 1);
  const ramp = Shader.linearGradient(
    0, 0, 0, H,
    new Float32Array([0, 0.65, 1]),
    new Float32Array([
      0.043, 0.043, 0.055, 1,
      0.082, 0.090, 0.110, 1,
      0.043, 0.043, 0.055, 1,
    ]),
    0,
  );
  bg.setShader(ramp);
  builder.drawRect(0, 0, W, H, bg);

  const horizon = new Paint(0.467, 0.502, 0.549, 0.08);
  builder.drawRect(0.10 * W, 0.82 * H, 0.80 * W, 1, horizon);

  frame ??= recordFrame();

  const px = 0.60 * W;
  const py = 0.57 * H;

  // Sixteen replays, small to large, twisting in a wave that travels from
  // the vanishing point toward the camera.
  const group = new Paint(0, 0, 0, 0.92);
  builder.saveLayer(group);
  for (let index = 0; index < 16; index += 1) {
    const wave = Math.sin((Math.PI * 2 * time) / 2.8 - index * 0.22);
    const scale = (0.26 + index * 0.125) * (1 + 0.025 * wave);
    builder.save();
    builder.translate(px - index * 0.004 * W, py + index * 0.003 * H + 4 * wave);
    builder.rotate((index * 5.2 - 39) * DEG + 7.5 * DEG * wave);
    builder.scale(scale, scale);
    builder.drawDisplayListCached(frame);
    builder.restore();
  }
  builder.restore();

  // The vanishing point: hairline ticks and the card's one lime mark.
  const tick = new Paint(0.914, 0.929, 0.894, 0.16);
  builder.drawRect(px - 12, py - 0.5, 8, 1, tick);
  builder.drawRect(px + 4, py - 0.5, 8, 1, tick);
  builder.drawRect(px - 0.5, py - 12, 1, 8, tick);
  builder.drawRect(px - 0.5, py + 4, 1, 8, tick);
  builder.save();
  builder.translate(px, py);
  builder.rotate(Math.PI / 4);
  const pin = new Paint(0.784, 1, 0.239, 1);
  builder.drawRect(-3, -3, 6, 6, pin);
  builder.restore();

  pin.free();
  tick.free();
  group.free();
  horizon.free();
  bg.free();
  ramp.free();
}`;

const nestedLayers = `// A layer inside a layer, with a rotated clip in the inner one. The recorder
// works the bounds and the depth slots out at RECORD time, so replay never
// looks ahead and never patches anything at restore.
import { ClipOp, FillRule, Paint, Path } from 'valo-web/raw';

/** @param {import('./setup').Scene} scene */
export default function draw(scene) {
  const { builder, width, height, time } = scene;

  const backdrop = new Paint(0.043, 0.043, 0.055, 1);
  builder.drawRect(0, 0, width, height, backdrop);

  const outer = new Paint(0, 0, 0, 0.62);
  builder.saveLayer(outer);

  const panel = new Paint(0.15, 0.16, 0.22, 1);
  builder.drawRoundedRect(width * 0.1, height * 0.24, width * 0.8, height * 0.52, new Float32Array([24]), panel);

  const inner = new Paint(0, 0, 0, 0.9);
  builder.saveLayer(inner);
  builder.save();
  builder.translate(width * 0.5, height * 0.5);
  builder.rotate(Math.sin(time * 0.5) * 0.4);
  builder.clipRect(-width * 0.36, -46, width * 0.72, 92, ClipOp.Intersect);
  builder.rotate(-Math.sin(time * 0.5) * 0.4);
  builder.translate(-width * 0.5, -height * 0.5);

  for (let index = 0; index < 8; index += 1) {
    const dot = new Path();
    dot.arc(width * 0.12 + index * width * 0.11, height * 0.5 + Math.sin(time * 1.4 + index) * 26, 25, 0, Math.PI * 2);
    const paint = new Paint(0.95, 0.78, 0.3, 1);
    builder.drawPath(dot, FillRule.NonZero, paint);
    paint.free();
    dot.free();
  }

  builder.restore();
  builder.restore();
  builder.restore();

  inner.free();
  panel.free();
  outer.free();
  backdrop.free();
}`;

export const demos: readonly Demo[] = [
  {
    id: 'wordmark',
    title: 'valo, drawn by valo',
    beyondCanvas2D: 'Coverage-blurred fields, swimming contours and gradient-filled shaped text.',
    grounding: 'examples/backdrop.rs · goldens/m11_gradient_text.png',
    source: wordmark,
  },
  {
    id: 'paragraph',
    title: 'Paragraph layout',
    beyondCanvas2D: 'Canvas2D has no line breaking. fillText draws one run on one line.',
    grounding: 'goldens/m6_text.png · crates/valo-text',
    source: paragraphLayout,
  },
  {
    id: 'backdrop-blur',
    title: 'Frosted glass',
    beyondCanvas2D: 'Canvas2D filters what you draw, never what is already on the canvas.',
    grounding: 'examples/backdrop.rs · goldens/m5_backdrop.png',
    source: backdropBlur,
  },
  {
    id: 'mask-blur-styles',
    title: 'Blur styles',
    beyondCanvas2D: 'Canvas2D has shadowBlur, which is Normal and nothing else.',
    grounding: 'examples/mask_styles.rs · goldens/m5_styles.png',
    source: maskBlurStyles,
  },
  {
    id: 'difference-clip',
    title: 'Difference clips',
    beyondCanvas2D: 'Canvas2D clip() only intersects. A hole has to be faked with compositing.',
    grounding: 'examples/clips.rs · goldens/f5_local_matrix_union_clip.png',
    source: differenceClip,
  },
  {
    id: 'gradient-spreads',
    title: 'Gradient spreads',
    beyondCanvas2D: 'Canvas2D gradients only pad. Repeat and reflect have no spelling.',
    grounding: 'goldens/f1_gradient_spreads.png',
    source: gradientSpreads,
  },
  {
    id: 'sweep-gradient',
    title: 'Sweep and colour matrix',
    beyondCanvas2D: 'Colour matrices apply to the paint, not only to a CSS filter string.',
    grounding: 'examples/gradients.rs · goldens/color_filters.png',
    source: sweepGradient,
  },
  {
    id: 'blend-modes',
    title: 'Destination-reading blends',
    beyondCanvas2D: 'Expressible in Canvas2D — but here it is per-paint, not global state.',
    grounding: 'examples/blends.rs · goldens/m4_blends.png',
    source: blendModes,
  },
  {
    id: 'group-opacity',
    title: 'Group opacity',
    beyondCanvas2D: 'globalAlpha is per-draw only; there is no group to fade.',
    grounding: 'examples/layers.rs · goldens/m12_nested_opacity.png',
    source: groupOpacity,
  },
  {
    id: 'strokes',
    title: 'Precision strokes',
    beyondCanvas2D: 'Dash arrays, phase, caps and joins run on the flattened contour — every dash is real geometry.',
    grounding: 'examples/strokes.rs · goldens/m10_strokes.png',
    source: strokes,
  },
  {
    id: 'gradient-text',
    title: 'Gradient text',
    beyondCanvas2D: 'The atlas is per-device, so every card that sets type shares one raster.',
    grounding: 'goldens/m11_gradient_text.png · goldens/m6_text.png',
    source: gradientText,
  },
  {
    id: 'filter-chain',
    title: 'Composed image filters',
    beyondCanvas2D: 'Canvas2D shadows are per-draw; this filters the composited group.',
    grounding: 'goldens/image_filter_drop_shadow.png · goldens/f7_filters_tier1.png',
    source: filterChain,
  },
  {
    id: 'retained-list',
    title: 'Retained replay',
    beyondCanvas2D: 'One recording, replayed sixteen times under live transforms. Canvas2D has no list to keep.',
    grounding: 'goldens/raster_cache_quad.png · the display-list model itself',
    source: retainedList,
  },
  {
    id: 'nested-layers',
    title: 'Nested layers',
    beyondCanvas2D: 'The recorder computes bounds and depth slots up front; replay never looks ahead.',
    grounding: 'examples/layers.rs · goldens/m4_layers.png',
    source: nestedLayers,
  },
  {
    id: 'first-rectangle',
    title: 'First rectangle',
    beyondCanvas2D: 'A recorded display list, not a canvas state machine — the same first draw as Getting started.',
    grounding: 'content/docs/getting-started.mdx',
    source: firstRectangle,
  },
  {
    id: 'docs-paint',
    title: 'Paint',
    beyondCanvas2D: 'Each draw carries its own paint — no global fillStyle.',
    grounding: 'content/docs/reference/paint.mdx',
    source: docsPaint,
  },
  {
    id: 'docs-builder',
    title: 'DisplayListBuilder',
    beyondCanvas2D: 'Transforms and draws are recorded, not issued to a GPU immediately.',
    grounding: 'content/docs/reference/display-list-builder.mdx',
    source: docsBuilder,
  },
  {
    id: 'docs-list',
    title: 'DisplayList',
    beyondCanvas2D: 'One recording, replayed several times. Canvas2D has no list to keep.',
    grounding: 'content/docs/reference/display-list.mdx',
    source: docsList,
  },
  {
    id: 'docs-path',
    title: 'Path',
    beyondCanvas2D: 'Even-odd fill on a retained path, including a circular hole.',
    grounding: 'content/docs/reference/path.mdx',
    source: docsPath,
  },
  {
    id: 'docs-fonts',
    title: 'FontCollection',
    beyondCanvas2D: 'Families are names the host registered, not CSS font stacks.',
    grounding: 'content/docs/reference/font-collection.mdx',
    source: docsFonts,
  },
  {
    id: 'docs-paragraph',
    title: 'Paragraph',
    beyondCanvas2D: 'Styled spans are built, laid out, then recorded like any other draw.',
    grounding: 'content/docs/reference/paragraph.mdx',
    source: docsParagraph,
  },
  {
    id: 'drawing-shapes',
    title: 'Shapes',
    beyondCanvas2D: 'Rects, rounded corners, circles, and even-odd paths on one retained list.',
    grounding: 'content/docs/drawing/shapes.mdx',
    source: drawingShapes,
  },
  {
    id: 'drawing-strokes',
    title: 'Strokes',
    beyondCanvas2D: 'Caps, joins, and dashes are paint style, not a second draw API.',
    grounding: 'content/docs/drawing/strokes.mdx',
    source: drawingStrokes,
  },
  {
    id: 'drawing-transforms',
    title: 'Transforms and clips',
    beyondCanvas2D: 'A hole is a Difference clip, not a compositing trick.',
    grounding: 'content/docs/drawing/transforms.mdx',
    source: drawingTransforms,
  },
  {
    id: 'drawing-shaders',
    title: 'Colour and shaders',
    beyondCanvas2D: 'Gradients can pad, repeat, or reflect. Canvas2D only pads.',
    grounding: 'content/docs/drawing/shaders.mdx',
    source: drawingShaders,
  },
  {
    id: 'drawing-blends',
    title: 'Blend modes',
    beyondCanvas2D: 'Blend mode sits on the paint, not on the canvas.',
    grounding: 'content/docs/drawing/blend-modes.mdx',
    source: drawingBlends,
  },
  {
    id: 'drawing-mask-blur',
    title: 'Mask blur',
    beyondCanvas2D: 'Four coverage-blur styles. Canvas2D shadowBlur is Normal only.',
    grounding: 'content/docs/drawing/mask-blur.mdx',
    source: drawingMaskBlur,
  },
  {
    id: 'drawing-layers',
    title: 'Layers',
    beyondCanvas2D: 'Group opacity applies once. Per-draw alpha darkens overlaps twice.',
    grounding: 'content/docs/drawing/layers.mdx',
    source: drawingLayers,
  },
  {
    id: 'drawing-color-filters',
    title: 'Colour filters',
    beyondCanvas2D: 'A 4×5 matrix and a blend filter sit on the paint, not on the canvas.',
    grounding: 'content/docs/drawing/color-filters.mdx',
    source: drawingColorFilters,
  },
  {
    id: 'drawing-image-filters',
    title: 'Image filters',
    beyondCanvas2D: 'A drop shadow is an image filter on the paint, not a canvas shadow.',
    grounding: 'content/docs/drawing/image-filters.mdx',
    source: drawingImageFilters,
  },
  {
    id: 'drawing-images',
    title: 'Images',
    beyondCanvas2D: 'The host uploads decoded pixels. valo never fetches a URL.',
    grounding: 'content/docs/drawing/images.mdx',
    source: drawingImages,
  },
  {
    id: 'drawing-text',
    title: 'Text',
    beyondCanvas2D: 'A laid-out paragraph can be filled with any paint, gradient included.',
    grounding: 'content/docs/drawing/text.mdx',
    source: drawingText,
  },
  {
    id: 'drawing-backdrop',
    title: 'Backdrop blur',
    beyondCanvas2D: 'A save-layer that opens already filled with a blur of what lies beneath.',
    grounding: 'content/docs/drawing/backdrop-blur.mdx',
    source: drawingBackdrop,
  },
  {
    id: 'drawing-lists',
    title: 'Nested lists',
    beyondCanvas2D: 'One recording, replayed several times. Canvas2D has no list to keep.',
    grounding: 'content/docs/drawing/nested-lists.mdx',
    source: drawingLists,
  },
  {
    id: 'drawing-shapes-gallery',
    title: 'Shapes gallery',
    beyondCanvas2D: 'NonZero vs EvenOdd, plus line, quad, cubic, and arc on one path.',
    grounding: 'content/docs/drawing/shapes.mdx',
    source: drawingShapesGallery,
  },
  {
    id: 'drawing-strokes-gallery',
    title: 'Strokes gallery',
    beyondCanvas2D: 'Every cap and join, then a dash whose phase walks.',
    grounding: 'content/docs/drawing/strokes.mdx',
    source: drawingStrokesGallery,
  },
  {
    id: 'drawing-transforms-gallery',
    title: 'Transforms gallery',
    beyondCanvas2D: 'Nested rotate and scale inside an intersect clip with a difference hole.',
    grounding: 'content/docs/drawing/transforms.mdx',
    source: drawingTransformsGallery,
  },
  {
    id: 'drawing-shaders-gallery',
    title: 'Shaders gallery',
    beyondCanvas2D: 'Linear, radial, and sweep from the same stops.',
    grounding: 'content/docs/drawing/shaders.mdx',
    source: drawingShadersGallery,
  },
  {
    id: 'drawing-blends-gallery',
    title: 'Blend modes gallery',
    beyondCanvas2D: 'Every BlendMode, one cell each.',
    grounding: 'content/docs/drawing/blend-modes.mdx',
    source: drawingBlendsGallery,
  },
  {
    id: 'drawing-mask-blur-gallery',
    title: 'Mask blur gallery',
    beyondCanvas2D: 'All four styles on both fill and stroke.',
    grounding: 'content/docs/drawing/mask-blur.mdx',
    source: drawingMaskBlurGallery,
  },
  {
    id: 'drawing-layers-gallery',
    title: 'Layers gallery',
    beyondCanvas2D: 'A bounds-cropped layer, then a nested pair.',
    grounding: 'content/docs/drawing/layers.mdx',
    source: drawingLayersGallery,
  },
  {
    id: 'drawing-color-filters-gallery',
    title: 'Colour filters gallery',
    beyondCanvas2D: 'Greyscale, invert, sepia, and several blend filters.',
    grounding: 'content/docs/drawing/color-filters.mdx',
    source: drawingColorFiltersGallery,
  },
  {
    id: 'drawing-image-filters-gallery',
    title: 'Image filters gallery',
    beyondCanvas2D: 'Blur, drop shadow, colour, and a composed chain.',
    grounding: 'content/docs/drawing/image-filters.mdx',
    source: drawingImageFiltersGallery,
  },
  {
    id: 'drawing-images-gallery',
    title: 'Images gallery',
    beyondCanvas2D: 'Linear vs nearest, then Clamp, Repeat, Mirror, Decal.',
    grounding: 'content/docs/drawing/images.mdx',
    source: drawingImagesGallery,
  },
  {
    id: 'drawing-text-gallery',
    title: 'Text gallery',
    beyondCanvas2D: 'Spans, small caps, underline, and a stroked paragraph.',
    grounding: 'content/docs/drawing/text.mdx',
    source: drawingTextGallery,
  },
  {
    id: 'drawing-backdrop-gallery',
    title: 'Backdrop gallery',
    beyondCanvas2D: 'Two glass panels, two sigmas, over the same scene.',
    grounding: 'content/docs/drawing/backdrop-blur.mdx',
    source: drawingBackdropGallery,
  },
  {
    id: 'drawing-lists-gallery',
    title: 'Nested lists gallery',
    beyondCanvas2D: 'One recording, translated, rotated, scaled, some cached.',
    grounding: 'content/docs/drawing/nested-lists.mdx',
    source: drawingListsGallery,
  },
];

/** The scene the landing page's hero runs. Deliberately not in the grid. */
export const HERO_DEMO_ID = 'wordmark';

/**
 * The grid, in order. Each one is something Canvas2D cannot say — the rest of
 * the catalog stays in the playground.
 */
export const LANDING_DEMO_IDS = [
  'paragraph',
  'backdrop-blur',
  'mask-blur-styles',
  'difference-clip',
  'retained-list',
  'strokes',
] as const;

export function demoById(id: string | null | undefined): Demo | undefined {
  return demos.find((demo) => demo.id === id);
}

export const landingDemos: readonly Demo[] = LANDING_DEMO_IDS.map((id) => demoById(id)!);
