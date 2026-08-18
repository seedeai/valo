/** Scenes for the Drawing docs. Registered in the playground catalog. */

export const drawingShapes = `import { FillRule, Paint, Path } from 'valo-web/raw';

export default function draw({ builder, width, height }) {
  const lime = new Paint(0.78, 1, 0.24, 1);
  const ink = new Paint(0.95, 0.95, 0.93, 0.9);
  ink.setStroke(2, 1, 1, 4, new Float32Array([]), 0);

  const cell = width / 4;
  const y = height * 0.5 - 36;

  builder.drawRect(cell * 0.22, y, 72, 72, lime);
  builder.drawRoundedRect(cell * 1.22, y, 72, 72, new Float32Array([18, 6, 18, 6]), lime);
  builder.drawCircle(cell * 2.5, height * 0.5, 36, lime);

  const star = new Path();
  const cx = cell * 3.5;
  const cy = height * 0.5;
  for (let i = 0; i < 5; i += 1) {
    const a = -Math.PI / 2 + (i * 4 * Math.PI) / 5;
    const x = cx + Math.cos(a) * 38;
    const y2 = cy + Math.sin(a) * 38;
    if (i === 0) star.moveTo(x, y2); else star.lineTo(x, y2);
  }
  star.close();
  star.circle(cx, cy, 14);
  builder.drawPath(star, FillRule.EvenOdd, lime);
  builder.drawPath(star, FillRule.EvenOdd, ink);

  star.free();
  ink.free();
  lime.free();
}
`;

export const drawingStrokes = `import { Cap, Join, Paint, Path } from 'valo-web/raw';

export default function draw({ builder, width, height }) {
  const rows = [
    { cap: Cap.Round, join: Join.Round, dashes: new Float32Array([]) },
    { cap: Cap.Square, join: Join.Bevel, dashes: new Float32Array([]) },
    { cap: Cap.Butt, join: Join.Miter, dashes: new Float32Array([16, 10]) },
  ];
  const x0 = width * 0.12;
  const x1 = width * 0.5;
  const x2 = width * 0.88;

  rows.forEach((row, i) => {
    const y = height * (0.28 + i * 0.22);
    const paint = new Paint(0.78, 1, 0.24, 1);
    paint.setStroke(14, row.cap, row.join, 4, row.dashes, 0);
    const path = new Path();
    path.moveTo(x0, y + 22);
    path.lineTo(x1, y - 22);
    path.lineTo(x2, y + 22);
    builder.drawPath(path, 0, paint);
    path.free();
    paint.free();
  });
}
`;

export const drawingTransforms = `import { ClipOp, Paint } from 'valo-web/raw';

export default function draw({ builder, width, height }) {
  const lime = new Paint(0.78, 1, 0.24, 1);

  builder.save();
  builder.translate(width * 0.28, height * 0.5);
  builder.rotate(0.35);
  builder.drawRoundedRect(-58, -58, 116, 116, new Float32Array([18]), lime);
  builder.restore();

  const x = width * 0.58;
  const y = height * 0.5 - 58;
  builder.save();
  builder.clipRoundedRect(x, y, 116, 116, new Float32Array([18]), ClipOp.Intersect);
  builder.clipRect(x + 34, y + 34, 48, 48, ClipOp.Difference);
  builder.drawRect(x, y, 116, 116, lime);
  builder.restore();

  lime.free();
}
`;

export const drawingShaders = `import { Paint, Shader, SpreadMode } from 'valo-web/raw';

export default function draw({ builder, width, height }) {
  const spreads = [SpreadMode.Pad, SpreadMode.Repeat, SpreadMode.Reflect];
  const x = width * 0.1;
  const w = width * 0.8;
  const h = 52;
  const gap = 18;
  const y0 = height * 0.5 - (h * 3 + gap * 2) / 2;

  spreads.forEach((spread, i) => {
    const y = y0 + i * (h + gap);
    const paint = new Paint(1, 1, 1, 1);
    const ramp = Shader.linearGradient(
      x, y, x + 90, y,
      new Float32Array([0, 1]),
      new Float32Array([0.12, 0.2, 0.42, 1, 0.78, 1, 0.24, 1]),
      spread,
    );
    paint.setShader(ramp);
    builder.drawRoundedRect(x, y, w, h, new Float32Array([12]), paint);
    paint.free();
    ramp.free();
  });
}
`;

export const drawingBlends = `import { BlendMode, Paint } from 'valo-web/raw';

export default function draw({ builder, width, height }) {
  const modes = [BlendMode.SrcOver, BlendMode.Multiply, BlendMode.Screen];
  const cell = width / 3;

  modes.forEach((mode, i) => {
    const cx = cell * (i + 0.5);
    const cy = height * 0.5;
    const under = new Paint(0.49, 0.29, 1, 1);
    builder.drawCircle(cx - 18, cy, 42, under);
    const over = new Paint(0.78, 1, 0.24, 1);
    over.setBlendMode(mode);
    builder.drawCircle(cx + 18, cy, 42, over);
    under.free();
    over.free();
  });
}
`;

export const drawingMaskBlur = `import { BlurStyle, Paint } from 'valo-web/raw';

export default function draw({ builder, width, height }) {
  const styles = [BlurStyle.Normal, BlurStyle.Solid, BlurStyle.Inner, BlurStyle.Outer];
  const size = 72;
  const gap = (width - size * 4) / 5;
  const y = height * 0.5 - size / 2;

  styles.forEach((style, i) => {
    const paint = new Paint(0.78, 1, 0.24, 1);
    paint.setMaskBlur(14, style);
    builder.drawRoundedRect(gap + i * (size + gap), y, size, size, new Float32Array([16]), paint);
    paint.free();
  });
}
`;

export const drawingLayers = `import { Paint } from 'valo-web/raw';

export default function draw({ builder, width, height }) {
  function trio(cx, cy, paint) {
    builder.drawCircle(cx, cy - 28, 38, paint);
    builder.drawCircle(cx - 32, cy + 22, 38, paint);
    builder.drawCircle(cx + 32, cy + 22, 38, paint);
  }

  const faded = new Paint(0.78, 1, 0.24, 0.4);
  trio(width * 0.28, height * 0.5, faded);

  const group = new Paint(0, 0, 0, 0.4);
  builder.saveLayer(group);
  const opaque = new Paint(0.78, 1, 0.24, 1);
  trio(width * 0.72, height * 0.5, opaque);
  builder.restore();

  faded.free();
  group.free();
  opaque.free();
}
`;

export const drawingColorFilters = `import { BlendMode, ColorFilter, Paint } from 'valo-web/raw';

export default function draw({ builder, width, height }) {
  const y = height * 0.5 - 40;
  const w = Math.min(96, width * 0.22);
  const gap = (width - w * 3) / 4;

  function tile(x, filter) {
    const paint = new Paint(0.78, 1, 0.24, 1);
    if (filter) paint.setColorFilter(filter);
    builder.drawRoundedRect(x, y, w, 80, new Float32Array([16]), paint);
    paint.free();
  }

  tile(gap, null);

  const grey = ColorFilter.matrix(new Float32Array([
    0.2126, 0.7152, 0.0722, 0, 0,
    0.2126, 0.7152, 0.0722, 0, 0,
    0.2126, 0.7152, 0.0722, 0, 0,
    0, 0, 0, 1, 0,
  ]));
  tile(gap * 2 + w, grey);

  const tint = ColorFilter.blend(0.49, 0.29, 1, 1, BlendMode.Modulate);
  tile(gap * 3 + w * 2, tint);

  tint.free();
  grey.free();
}
`;

export const drawingImageFilters = `import { ImageFilter, Paint } from 'valo-web/raw';

export default function draw({ builder, width, height }) {
  const plate = new Paint(0.16, 0.16, 0.18, 1);
  builder.drawRoundedRect(width * 0.5 - 120, height * 0.5 - 80, 240, 160, new Float32Array([28]), plate);

  const shadow = ImageFilter.dropShadow(8, 14, 10, 10, 0, 0, 0, 0.55);
  const paint = new Paint(0.78, 1, 0.24, 1);
  paint.setImageFilter(shadow);
  builder.drawRoundedRect(width * 0.5 - 70, height * 0.5 - 48, 140, 96, new Float32Array([20]), paint);

  paint.free();
  shadow.free();
  plate.free();
}
`;

export const drawingImages = `import { ClipOp, Filter, MipmapMode, Paint, TileMode } from 'valo-web/raw';

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

  const pad = 28;
  const scale = Math.min((width - pad * 2) / cat.width, (height - pad * 2) / cat.height);
  const dw = cat.width * scale;
  const dh = cat.height * scale;
  const dx = (width - dw) / 2;
  const dy = (height - dh) / 2;

  const paint = new Paint(1, 1, 1, 1);
  builder.save();
  builder.clipRoundedRect(dx, dy, dw, dh, new Float32Array([18]), ClipOp.Intersect);
  builder.drawImageRect(
    cat,
    0, 0, cat.width, cat.height,
    dx, dy, dw, dh,
    Filter.Linear, MipmapMode.Linear,
    TileMode.Clamp, TileMode.Clamp,
    paint,
  );
  builder.restore();
  paint.free();
}
`;

export const drawingText = `import { Paint, ParagraphBuilder, Shader, SpreadMode, TextStyle } from 'valo-web/raw';

export default function draw({ builder, width, height, fonts }) {
  const style = new TextStyle('Fira Sans', 64);
  style.setColor(1, 1, 1, 1);
  const paragraphs = new ParagraphBuilder();
  paragraphs.addText('Valo', style);
  const paragraph = paragraphs.build(fonts);
  paragraph.layout(width - 48);

  const ramp = Shader.linearGradient(
    0, 0, Math.max(180, paragraph.maxIntrinsicWidth), 0,
    new Float32Array([0, 1]),
    new Float32Array([0.78, 1, 0.24, 1, 0.49, 0.29, 1, 1]),
    SpreadMode.Pad,
  );
  const paint = new Paint(1, 1, 1, 1);
  paint.setShader(ramp);
  builder.drawParagraphWith(paragraph, 24, height * 0.5 - paragraph.height * 0.5, paint);

  paragraph.free();
  paragraphs.free();
  style.free();
  paint.free();
  ramp.free();
}
`;

export const drawingBackdrop = `import { ClipOp, Paint } from 'valo-web/raw';

export default function draw({ builder, width, height, time }) {
  const colours = [
    [0.95, 0.75, 0.3],
    [0.3, 0.85, 0.6],
    [0.4, 0.65, 1],
  ];
  const cols = 4;
  const rows = 3;
  const stepX = width / cols;
  const stepY = height / rows;
  const radius = Math.min(stepX, stepY) * 0.28;
  for (let i = 0; i < cols * rows; i += 1) {
    const [r, g, b] = colours[i % 3];
    const paint = new Paint(r, g, b, 1);
    const drift = Math.sin(time * 0.7 + i) * 8;
    builder.drawCircle(
      stepX * ((i % cols) + 0.5) + drift,
      stepY * (Math.floor(i / cols) + 0.5),
      radius,
      paint,
    );
    paint.free();
  }

  const glass = {
    x: width * 0.18,
    y: height * 0.2,
    w: width * 0.64,
    h: height * 0.58,
  };
  builder.save();
  builder.clipRoundedRect(glass.x, glass.y, glass.w, glass.h, new Float32Array([18]), ClipOp.Intersect);
  builder.backdropBlur(glass.x, glass.y, glass.w, glass.h, 14);
  builder.restore();

  const tint = new Paint(1, 1, 1, 0.12);
  builder.drawRoundedRect(glass.x, glass.y, glass.w, glass.h, new Float32Array([18]), tint);
  tint.free();
}
`;

export const drawingLists = `import { DisplayListBuilder, Paint } from 'valo-web/raw';

export default function draw({ builder, width, height }) {
  const paint = new Paint(0.78, 1, 0.24, 1);
  const cell = new DisplayListBuilder();
  cell.drawRoundedRect(0, 0, 72, 48, new Float32Array([10]), paint);
  const list = cell.build();

  const gap = 20;
  const x0 = (width - (72 * 3 + gap * 2)) / 2;
  const y = height * 0.5 - 24;
  for (let i = 0; i < 3; i += 1) {
    builder.save();
    builder.translate(x0 + i * (72 + gap), y);
    builder.drawDisplayList(list);
    builder.restore();
  }

  list.free();
  cell.free();
  paint.free();
}
`;
