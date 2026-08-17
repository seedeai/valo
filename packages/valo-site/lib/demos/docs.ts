/** Scenes embedded on documentation pages. Registered in the playground catalog. */

export const firstRectangle = `import { Paint } from 'valo-web/raw';

export default function draw({ builder }) {
  const paint = new Paint(0.78, 1, 0.24, 1);
  builder.drawRect(40, 40, 240, 140, paint);
  paint.free();
}
`;
