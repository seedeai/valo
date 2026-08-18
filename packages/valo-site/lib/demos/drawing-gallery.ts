/** Fuller Drawing-docs scenes: every option, after the simple specimen. */

export const drawingShapesGallery = `import { FillRule, Paint, Path } from 'valo-web/raw';

export default function draw({ builder, width, height }) {
  const lime = new Paint(0.78, 1, 0.24, 1);
  const ink = new Paint(0.95, 0.95, 0.93, 0.85);
  ink.setStroke(2, 1, 1, 4, new Float32Array([]), 0);

  function star(cx, cy) {
    const path = new Path();
    for (let i = 0; i < 5; i += 1) {
      const a = -Math.PI / 2 + (i * 4 * Math.PI) / 5;
      const x = cx + Math.cos(a) * 44;
      const y = cy + Math.sin(a) * 44;
      if (i === 0) path.moveTo(x, y); else path.lineTo(x, y);
    }
    path.close();
    path.circle(cx, cy, 16);
    return path;
  }

  const left = star(width * 0.22, height * 0.38);
  builder.drawPath(left, FillRule.NonZero, lime);
  left.free();

  const right = star(width * 0.5, height * 0.38);
  builder.drawPath(right, FillRule.EvenOdd, lime);
  right.free();

  const curve = new Path();
  const x = width * 0.72;
  const y = height * 0.22;
  curve.moveTo(x, y + 80);
  curve.lineTo(x + 18, y + 40);
  curve.quadraticCurveTo(x + 40, y, x + 62, y + 40);
  curve.bezierCurveTo(x + 80, y + 80, x + 40, y + 90, x + 24, y + 70);
  curve.arc(x + 48, y + 108, 22, Math.PI, Math.PI * 1.4);
  builder.drawPath(curve, FillRule.NonZero, ink);
  curve.free();

  ink.free();
  lime.free();
}
`;

export const drawingStrokesGallery = `import { Cap, Join, Paint, Path } from 'valo-web/raw';

export default function draw({ builder, width, height, time }) {
  const caps = [Cap.Butt, Cap.Round, Cap.Square];
  const joins = [Join.Miter, Join.Round, Join.Bevel];
  const cellW = width / 3;
  const cellH = height * 0.28;

  joins.forEach((join, row) => {
    caps.forEach((cap, col) => {
      const cx = cellW * (col + 0.5);
      const cy = 36 + row * cellH;
      const paint = new Paint(0.78, 1, 0.24, 1);
      paint.setStroke(12, cap, join, 4, new Float32Array([]), 0);
      const path = new Path();
      path.moveTo(cx - 36, cy + 18);
      path.lineTo(cx, cy - 14);
      path.lineTo(cx + 36, cy + 18);
      builder.drawPath(path, 0, paint);
      path.free();
      paint.free();
    });
  });

  const dash = new Paint(0.78, 1, 0.24, 1);
  dash.setStroke(8, Cap.Round, Join.Round, 4, new Float32Array([14, 10]), time * 24);
  const line = new Path();
  line.moveTo(width * 0.1, height * 0.9);
  line.lineTo(width * 0.9, height * 0.9);
  builder.drawPath(line, 0, dash);
  line.free();
  dash.free();
}
`;

export const drawingTransformsGallery = `import { ClipOp, Paint } from 'valo-web/raw';

export default function draw({ builder, width, height, time }) {
  const lime = new Paint(0.78, 1, 0.24, 1);
  const violet = new Paint(0.49, 0.29, 1, 1);
  const cx = width * 0.5;
  const cy = height * 0.5;

  builder.save();
  builder.clipRoundedRect(cx - 110, cy - 80, 220, 160, new Float32Array([24]), ClipOp.Intersect);
  builder.clipRect(cx - 28, cy - 28, 56, 56, ClipOp.Difference);

  for (let i = 0; i < 8; i += 1) {
    builder.save();
    builder.translate(cx, cy);
    builder.rotate(time * 0.4 + i * (Math.PI / 4));
    builder.scale(1, 0.35);
    builder.drawCircle(0, 0, 90, i % 2 === 0 ? lime : violet);
    builder.restore();
  }
  builder.restore();

  lime.free();
  violet.free();
}
`;

export const drawingShadersGallery = `import { Paint, Shader, SpreadMode } from 'valo-web/raw';

export default function draw({ builder, width, height }) {
  const stops = new Float32Array([0, 0.5, 1]);
  const colours = new Float32Array([
    0.12, 0.2, 0.42, 1,
    0.78, 1, 0.24, 1,
    0.49, 0.29, 1, 1,
  ]);
  const size = Math.min(120, width * 0.28);
  const y = height * 0.5 - size / 2;
  const xs = [width * 0.12, width * 0.5 - size / 2, width * 0.88 - size];

  const linear = new Paint(1, 1, 1, 1);
  const linearShader = Shader.linearGradient(xs[0], y, xs[0] + size, y + size, stops, colours, SpreadMode.Pad);
  linear.setShader(linearShader);
  builder.drawRoundedRect(xs[0], y, size, size, new Float32Array([16]), linear);

  const radial = new Paint(1, 1, 1, 1);
  const cx = xs[1] + size / 2;
  const cy = y + size / 2;
  const radialShader = Shader.radialGradient(cx, cy, 0, cx, cy, size * 0.55, stops, colours, SpreadMode.Pad);
  radial.setShader(radialShader);
  builder.drawCircle(cx, cy, size * 0.5, radial);

  const sweep = new Paint(1, 1, 1, 1);
  const sx = xs[2] + size / 2;
  const sweepShader = Shader.sweepGradient(sx, cy, 0, stops, colours);
  sweep.setShader(sweepShader);
  builder.drawRoundedRect(xs[2], y, size, size, new Float32Array([16]), sweep);

  [linear, radial, sweep, linearShader, radialShader, sweepShader].forEach((item) => item.free());
}
`;

export const drawingBlendsGallery = `import { BlendMode, Paint, ParagraphBuilder, TextAlign, TextStyle } from 'valo-web/raw';

export default function draw({ builder, width, height, fonts }) {
  const modes = Object.entries(BlendMode);
  const cols = 6;
  const rows = 5;
  const pad = 8;
  const cellW = (width - pad * 2) / cols;
  const cellH = (height - pad * 2) / rows;

  modes.forEach(([name, mode], i) => {
    const col = i % cols;
    const row = Math.floor(i / cols);
    const x = pad + col * cellW;
    const y = pad + row * cellH;
    const dest = new Paint(0.49, 0.29, 1, 1);
    builder.drawRoundedRect(x + 8, y + 4, cellW - 16, cellH - 22, new Float32Array([8]), dest);
    const src = new Paint(0.78, 1, 0.24, 0.9);
    src.setBlendMode(mode);
    builder.drawCircle(x + cellW * 0.62, y + (cellH - 18) * 0.55, Math.min(cellW, cellH) * 0.22, src);
    dest.free();
    src.free();

    const style = new TextStyle('Fira Sans', 10);
    style.setColor(0.72, 0.72, 0.74, 1);
    const paragraphs = new ParagraphBuilder();
    paragraphs.setAlign(TextAlign.Center);
    paragraphs.addText(name, style);
    const label = paragraphs.build(fonts);
    label.layout(cellW);
    builder.drawParagraph(label, x, y + cellH - 18);
    label.free();
    paragraphs.free();
    style.free();
  });
}
`;

export const drawingMaskBlurGallery = `import { BlurStyle, Cap, Join, Paint } from 'valo-web/raw';

export default function draw({ builder, width, height }) {
  const styles = [BlurStyle.Normal, BlurStyle.Solid, BlurStyle.Inner, BlurStyle.Outer];
  const size = Math.min(64, width * 0.16);
  const gap = (width - size * 4) / 5;

  styles.forEach((style, i) => {
    const x = gap + i * (size + gap);
    const fill = new Paint(0.78, 1, 0.24, 1);
    fill.setMaskBlur(12, style);
    builder.drawRoundedRect(x, height * 0.28 - size / 2, size, size, new Float32Array([14]), fill);
    fill.free();

    const stroke = new Paint(0.78, 1, 0.24, 1);
    stroke.setStroke(6, Cap.Round, Join.Round, 4, new Float32Array([]), 0);
    stroke.setMaskBlur(10, style);
    builder.drawCircle(x + size / 2, height * 0.7, size * 0.38, stroke);
    stroke.free();
  });
}
`;

export const drawingLayersGallery = `import { Paint } from 'valo-web/raw';

export default function draw({ builder, width, height }) {
  const lime = new Paint(0.78, 1, 0.24, 1);
  const violet = new Paint(0.49, 0.29, 1, 1);

  const crop = new Paint(0, 0, 0, 0.85);
  builder.saveLayerBounds(width * 0.08, height * 0.18, width * 0.36, height * 0.64, crop);
  builder.drawCircle(width * 0.18, height * 0.5, 70, lime);
  builder.drawCircle(width * 0.34, height * 0.5, 70, violet);
  builder.restore();

  const outer = new Paint(0, 0, 0, 0.55);
  builder.saveLayer(outer);
  builder.drawCircle(width * 0.68, height * 0.38, 54, lime);
  const inner = new Paint(0, 0, 0, 0.5);
  builder.saveLayer(inner);
  builder.drawCircle(width * 0.78, height * 0.58, 54, violet);
  builder.restore();
  builder.restore();

  lime.free();
  violet.free();
  crop.free();
  outer.free();
  inner.free();
}
`;

export const drawingColorFiltersGallery = `import { BlendMode, ColorFilter, Paint } from 'valo-web/raw';

export default function draw({ builder, width, height }) {
  const matrices = [
    null,
    new Float32Array([
      0.2126, 0.7152, 0.0722, 0, 0,
      0.2126, 0.7152, 0.0722, 0, 0,
      0.2126, 0.7152, 0.0722, 0, 0,
      0, 0, 0, 1, 0,
    ]),
    new Float32Array([
      -1, 0, 0, 0, 1,
      0, -1, 0, 0, 1,
      0, 0, -1, 0, 1,
      0, 0, 0, 1, 0,
    ]),
    new Float32Array([
      0.393, 0.769, 0.189, 0, 0,
      0.349, 0.686, 0.168, 0, 0,
      0.272, 0.534, 0.131, 0, 0,
      0, 0, 0, 1, 0,
    ]),
  ];
  const blends = [BlendMode.Hue, BlendMode.Screen, BlendMode.Overlay, BlendMode.Color];
  const w = Math.min(72, width * 0.18);
  const gap = (width - w * 4) / 5;

  function tile(x, y, filter) {
    const paint = new Paint(0.78, 1, 0.24, 1);
    if (filter) paint.setColorFilter(filter);
    builder.drawRoundedRect(x, y, w, 64, new Float32Array([12]), paint);
    paint.free();
    filter?.free();
  }

  matrices.forEach((values, i) => {
    tile(gap + i * (w + gap), height * 0.22, values ? ColorFilter.matrix(values) : null);
  });
  blends.forEach((mode, i) => {
    tile(gap + i * (w + gap), height * 0.58, ColorFilter.blend(0.49, 0.29, 1, 1, mode));
  });
}
`;

export const drawingImageFiltersGallery = `import { ColorFilter, ImageFilter, Paint } from 'valo-web/raw';

export default function draw({ builder, width, height }) {
  const w = Math.min(88, width * 0.2);
  const gap = (width - w * 4) / 5;
  const y = height * 0.5 - 36;
  const grey = ColorFilter.matrix(new Float32Array([
    0.2126, 0.7152, 0.0722, 0, 0,
    0.2126, 0.7152, 0.0722, 0, 0,
    0.2126, 0.7152, 0.0722, 0, 0,
    0, 0, 0, 1, 0,
  ]));
  const shadow = ImageFilter.dropShadow(6, 10, 6, 6, 0, 0, 0, 0.5);
  const soften = ImageFilter.blur(1.2, 1.2);
  const filters = [
    ImageFilter.blur(4, 4),
    ImageFilter.dropShadow(6, 10, 8, 8, 0.49, 0.29, 1, 0.7),
    ImageFilter.color(grey),
    ImageFilter.compose(shadow, soften),
  ];

  filters.forEach((filter, i) => {
    const plate = new Paint(0.16, 0.16, 0.18, 1);
    builder.drawRoundedRect(gap + i * (w + gap) - 8, y - 12, w + 16, 96, new Float32Array([16]), plate);
    const paint = new Paint(0.78, 1, 0.24, 1);
    paint.setImageFilter(filter);
    builder.drawRoundedRect(gap + i * (w + gap), y, w, 72, new Float32Array([14]), paint);
    paint.free();
    filter.free();
    plate.free();
  });
  shadow.free();
  soften.free();
  grey.free();
}
`;

export const drawingImagesGallery = `import { Filter, MipmapMode, Paint, TileMode } from 'valo-web/raw';

let cat;

export async function load({ renderer }) {
  const bitmap = await createImageBitmap(await (await fetch('/images/cat.jpg')).blob());
  cat?.free();
  cat = renderer.uploadImageBitmap(bitmap, true);
  bitmap.close();
}

export function dispose() {
  cat?.free();
  cat = undefined;
}

export default function draw({ builder, width, height }) {
  if (!cat) return;
  const paint = new Paint(1, 1, 1, 1);
  const tiles = [TileMode.Clamp, TileMode.Repeat, TileMode.Mirror, TileMode.Decal];
  const w = Math.min(90, width * 0.2);
  const gap = (width - w * 4) / 5;
  const src = Math.min(cat.width, cat.height) * 0.45;
  const sx = (cat.width - src) / 2;
  const sy = (cat.height - src) / 2;

  [Filter.Linear, Filter.Nearest].forEach((filter, i) => {
    const dw = 88;
    const dx = width * (0.22 + i * 0.4) - dw / 2;
    builder.drawImageRect(
      cat, sx, sy, src * 0.25, src * 0.25,
      dx, height * 0.08, dw, dw,
      filter, MipmapMode.None, TileMode.Clamp, TileMode.Clamp, paint,
    );
  });

  tiles.forEach((tile, i) => {
    const dx = gap + i * (w + gap);
    builder.drawImageRect(
      cat,
      -cat.width * 0.35, -cat.height * 0.35,
      cat.width * 1.7, cat.height * 1.7,
      dx, height * 0.52, w, w,
      Filter.Linear, MipmapMode.Linear, tile, tile, paint,
    );
  });
  paint.free();
}
`;

export const drawingTextGallery = `import { Cap, DecorationKind, Join, Paint, ParagraphBuilder, TextAlign, TextStyle, VariantCaps } from 'valo-web/raw';

export default function draw({ builder, width, height, fonts }) {
  const body = new TextStyle('Fira Sans', 22);
  body.setColor(0.95, 0.95, 0.93, 1);
  const heavy = new TextStyle('Fira Sans', 22);
  heavy.setColor(0.78, 1, 0.24, 1);
  heavy.setWeight(700);
  const italic = new TextStyle('Fira Sans', 22);
  italic.setColor(0.95, 0.95, 0.93, 1);
  italic.setItalic(true);
  italic.setDecoration(DecorationKind.Underline);
  const caps = new TextStyle('Fira Sans', 18);
  caps.setColor(0.72, 0.72, 0.74, 1);
  caps.setVariantCaps(VariantCaps.SmallCaps);

  const mixed = new ParagraphBuilder();
  mixed.addText('Hello ', body);
  mixed.addText('Valo', heavy);
  mixed.addText(' engine', italic);
  const hello = mixed.build(fonts);
  hello.layout(width - 48);
  builder.drawParagraph(hello, 24, height * 0.18);

  const aligned = new ParagraphBuilder();
  aligned.setAlign(TextAlign.Center);
  aligned.addText('Small caps · centered', caps);
  const center = aligned.build(fonts);
  center.layout(width - 48);
  builder.drawParagraph(center, 24, height * 0.42);

  const strokeStyle = new TextStyle('Fira Sans', 42);
  strokeStyle.setColor(1, 1, 1, 1);
  const outline = new ParagraphBuilder();
  outline.addText('Stroke', strokeStyle);
  const strokeText = outline.build(fonts);
  strokeText.layout(width - 48);
  const stroke = new Paint(0.78, 1, 0.24, 1);
  stroke.setStroke(1.6, Cap.Round, Join.Round, 4, new Float32Array([]), 0);
  builder.drawParagraphWith(strokeText, 24, height * 0.62, stroke);

  [hello, center, strokeText, mixed, aligned, outline, body, heavy, italic, caps, strokeStyle, stroke]
    .forEach((item) => item.free());
}
`;

export const drawingBackdropGallery = `import { ClipOp, Paint } from 'valo-web/raw';

export default function draw({ builder, width, height, time }) {
  const colours = [[0.95, 0.75, 0.3], [0.3, 0.85, 0.6], [0.4, 0.65, 1]];
  for (let i = 0; i < 12; i += 1) {
    const paint = new Paint(...colours[i % 3], 1);
    builder.drawCircle(
      width * (0.12 + (i % 4) * 0.25) + Math.sin(time * 0.6 + i) * 10,
      height * (0.18 + Math.floor(i / 4) * 0.28),
      26,
      paint,
    );
    paint.free();
  }

  function glass(x, y, w, h, sigma) {
    builder.save();
    builder.clipRoundedRect(x, y, w, h, new Float32Array([16]), ClipOp.Intersect);
    builder.backdropBlur(x, y, w, h, sigma);
    builder.restore();
    const tint = new Paint(1, 1, 1, 0.1);
    builder.drawRoundedRect(x, y, w, h, new Float32Array([16]), tint);
    tint.free();
  }

  glass(width * 0.08, height * 0.22, width * 0.38, height * 0.56, 6);
  glass(width * 0.54, height * 0.22, width * 0.38, height * 0.56, 22);
}
`;

export const drawingListsGallery = `import { DisplayListBuilder, Paint } from 'valo-web/raw';

export default function draw({ builder, width, height }) {
  const lime = new Paint(0.78, 1, 0.24, 1);
  const ink = new Paint(0.12, 0.12, 0.14, 1);
  const cell = new DisplayListBuilder();
  cell.drawRoundedRect(0, 0, 64, 44, new Float32Array([10]), lime);
  cell.drawRect(8, 14, 20, 16, ink);
  const list = cell.build();

  const poses = [
    { x: width * 0.12, y: height * 0.38, rotate: 0, scale: 1, cached: false },
    { x: width * 0.38, y: height * 0.38, rotate: 0, scale: 1, cached: true },
    { x: width * 0.62, y: height * 0.42, rotate: 0.4, scale: 1, cached: true },
    { x: width * 0.82, y: height * 0.4, rotate: 0, scale: 1.35, cached: true },
  ];
  poses.forEach((pose) => {
    builder.save();
    builder.translate(pose.x, pose.y);
    builder.rotate(pose.rotate);
    builder.scale(pose.scale, pose.scale);
    if (pose.cached) builder.drawDisplayListCached(list);
    else builder.drawDisplayList(list);
    builder.restore();
  });

  list.free();
  cell.free();
  lime.free();
  ink.free();
}
`;
