/** Scenes embedded on documentation pages. Registered in the playground catalog. */

export const firstRectangle = `import { Paint } from 'valo-web/raw';

export default function draw({ builder }) {
  const paint = new Paint(0.78, 1, 0.24, 1);
  builder.drawRect(40, 40, 240, 140, paint);
  paint.free();
}
`;

export const docsPaint = `import { BlurStyle, Cap, Join, Paint } from 'valo-web/raw';

export default function draw({ builder }) {
  const fill = new Paint(0.78, 1, 0.24, 1);
  fill.setMaskBlur(10, BlurStyle.Normal);
  builder.drawRoundedRect(48, 48, 200, 110, new Float32Array([16]), fill);

  const stroke = new Paint(0.95, 0.95, 0.93, 1);
  stroke.setStroke(4, Cap.Round, Join.Round, 4, new Float32Array([]), 0);
  builder.drawRoundedRect(48, 48, 200, 110, new Float32Array([16]), stroke);

  fill.free();
  stroke.free();
}
`;

export const docsBuilder = `import { Paint } from 'valo-web/raw';

export default function draw({ builder, width, height }) {
  const fill = new Paint(0.78, 1, 0.24, 1);
  builder.save();
  builder.translate(width * 0.5, height * 0.5);
  builder.rotate(0.2);
  builder.drawRoundedRect(-90, -48, 180, 96, new Float32Array([18]), fill);
  builder.restore();

  const stroke = new Paint(0.95, 0.95, 0.93, 0.85);
  stroke.setStroke(2, 1, 1, 4, new Float32Array([]), 0);
  builder.drawCircle(width * 0.5, height * 0.5, 92, stroke);

  fill.free();
  stroke.free();
}
`;

export const docsList = `import { DisplayListBuilder, Paint } from 'valo-web/raw';

export default function draw({ builder }) {
  const cell = new DisplayListBuilder();
  const paint = new Paint(0.78, 1, 0.24, 1);
  cell.drawRoundedRect(0, 0, 72, 48, new Float32Array([10]), paint);
  const list = cell.build();

  const gap = 20;
  for (let i = 0; i < 3; i += 1) {
    builder.save();
    builder.translate(40 + i * (72 + gap), 72);
    builder.drawDisplayList(list);
    builder.restore();
  }

  list.free();
  cell.free();
  paint.free();
}
`;

export const docsPath = `import { FillRule, Paint, Path } from 'valo-web/raw';

export default function draw({ builder }) {
  const path = new Path();
  path.moveTo(48, 48);
  path.lineTo(240, 48);
  path.quadraticCurveTo(280, 110, 240, 172);
  path.close();
  path.circle(150, 100, 32);

  const fill = new Paint(0.78, 1, 0.24, 1);
  builder.drawPath(path, FillRule.EvenOdd, fill);

  path.free();
  fill.free();
}
`;

export const docsFonts = `import { ParagraphBuilder, TextStyle } from 'valo-web/raw';

export default function draw({ builder, fonts }) {
  const sans = new TextStyle('Fira Sans', 24);
  sans.setColor(0.95, 0.95, 0.93, 1);
  const mono = new TextStyle('JetBrains Mono', 24);
  mono.setColor(0.78, 1, 0.24, 1);

  const paragraphs = new ParagraphBuilder();
  paragraphs.addText('Fira Sans', sans);
  const heading = paragraphs.build(fonts);
  heading.layout(280);
  builder.drawParagraph(heading, 40, 48);

  paragraphs.addText('JetBrains Mono', mono);
  const code = paragraphs.build(fonts);
  code.layout(280);
  builder.drawParagraph(code, 40, 96);

  heading.free();
  code.free();
  paragraphs.free();
  sans.free();
  mono.free();
}
`;

export const docsParagraph = `import { ParagraphBuilder, TextAlign, TextStyle } from 'valo-web/raw';

export default function draw({ builder, fonts, width }) {
  const body = new TextStyle('Fira Sans', 22);
  body.setColor(0.95, 0.95, 0.93, 1);
  const accent = new TextStyle('Fira Sans', 22);
  accent.setColor(0.78, 1, 0.24, 1);
  accent.setWeight(600);

  const paragraphs = new ParagraphBuilder();
  paragraphs.setAlign(TextAlign.Left);
  paragraphs.addText('Hello, ', body);
  paragraphs.addText('Valo', accent);
  const paragraph = paragraphs.build(fonts);
  paragraph.layout(width - 80);
  builder.drawParagraph(paragraph, 40, 48);

  paragraph.free();
  paragraphs.free();
  body.free();
  accent.free();
}
`;
