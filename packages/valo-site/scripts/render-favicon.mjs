/**
 * Rasterize `.valo-mark` to a PNG with a real alpha channel, and write a
 * matching SVG. `next/og` ImageResponse cannot emit transparency.
 *
 * Geometry matches the CSS: three 50% corners, one 12% corner, rotated −45°
 * so the tight corner points down. The painted bbox is then translated in
 * canvas space so it sits on the viewBox / PNG centre. Moving the rotation
 * origin is not the same as that translate — do not substitute one for the other.
 */
import { writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { PNG } from 'pngjs';

const LIME = { r: 200, g: 255, b: 61 };
const LIME_HEX = '#c8ff3d';
const PNG_SIZE = 96;
const PNG_MARK = 56;
const SVG_SIZE = 32;
const SVG_MARK = 20;
const SAMPLES = 4;
const SQRT_HALF = Math.SQRT1_2;

function hypot(x, y) {
  return Math.sqrt(x * x + y * y);
}

function inRoundedRect(x, y, size, radii) {
  if (x < 0 || y < 0 || x > size || y > size) return false;
  const [topLeft, topRight, bottomRight, bottomLeft] = radii;
  if (x < topLeft && y < topLeft) return hypot(x - topLeft, y - topLeft) <= topLeft;
  if (x > size - topRight && y < topRight) {
    return hypot(x - (size - topRight), y - topRight) <= topRight;
  }
  if (x > size - bottomRight && y > size - bottomRight) {
    return hypot(x - (size - bottomRight), y - (size - bottomRight)) <= bottomRight;
  }
  if (x < bottomLeft && y > size - bottomLeft) {
    return hypot(x - bottomLeft, y - (size - bottomLeft)) <= bottomLeft;
  }
  return true;
}

function inMark(localX, localY, mark) {
  const round = mark / 2;
  return inRoundedRect(localX, localY, mark, [round, round, round, mark * 0.12]);
}

/**
 * Canvas → mark-local. CSS `rotate(-45deg)` sends the bottom-left corner down;
 * this is that transform inverted, y downward.
 */
function toMarkLocal(canvasX, canvasY, originX, originY, mark) {
  const dx = canvasX - originX;
  const dy = canvasY - originY;
  return {
    x: (dx - dy) * SQRT_HALF + mark / 2,
    y: (dx + dy) * SQRT_HALF + mark / 2,
  };
}

function coverage(canvasX, canvasY, size, mark, shiftX = 0, shiftY = 0) {
  const origin = size / 2;
  let hits = 0;
  const step = 1 / SAMPLES;
  for (let sy = 0; sy < SAMPLES; sy += 1) {
    for (let sx = 0; sx < SAMPLES; sx += 1) {
      const local = toMarkLocal(
        canvasX + (sx + 0.5) * step - shiftX,
        canvasY + (sy + 0.5) * step - shiftY,
        origin,
        origin,
        mark,
      );
      if (inMark(local.x, local.y, mark)) hits += 1;
    }
  }
  return hits / (SAMPLES * SAMPLES);
}

function bbox(size, mark) {
  let minX = size;
  let minY = size;
  let maxX = 0;
  let maxY = 0;
  for (let y = 0; y < size; y += 1) {
    for (let x = 0; x < size; x += 1) {
      if (coverage(x, y, size, mark) < 0.5) continue;
      minX = Math.min(minX, x);
      minY = Math.min(minY, y);
      maxX = Math.max(maxX, x);
      maxY = Math.max(maxY, y);
    }
  }
  return {
    minX,
    minY,
    maxX,
    maxY,
    cx: (minX + maxX) / 2,
    cy: (minY + maxY) / 2,
  };
}

function centeringShift(size, mark) {
  const box = bbox(size, mark);
  // Bbox-centre, then a little extra lift so the downward point does not
  // pin the glyph to the bottom of a square favicon.
  const opticalLift = mark * 0.06;
  return { x: size / 2 - box.cx, y: size / 2 - box.cy - opticalLift };
}

function renderPng() {
  const shift = centeringShift(PNG_SIZE, PNG_MARK);
  const png = new PNG({ width: PNG_SIZE, height: PNG_SIZE, colorType: 6 });
  for (let y = 0; y < PNG_SIZE; y += 1) {
    for (let x = 0; x < PNG_SIZE; x += 1) {
      const alpha = Math.round(coverage(x, y, PNG_SIZE, PNG_MARK, shift.x, shift.y) * 255);
      const index = (y * PNG_SIZE + x) * 4;
      png.data[index] = LIME.r;
      png.data[index + 1] = LIME.g;
      png.data[index + 2] = LIME.b;
      png.data[index + 3] = alpha;
    }
  }
  return PNG.sync.write(png);
}

function renderSvg() {
  const shift = centeringShift(SVG_SIZE, SVG_MARK);
  const half = SVG_MARK / 2;
  const tight = SVG_MARK * 0.12;
  const origin = SVG_SIZE / 2;
  const path = [
    `M${half} 0`,
    `A${half} ${half} 0 0 1 ${SVG_MARK} ${half}`,
    `A${half} ${half} 0 0 1 ${half} ${SVG_MARK}`,
    `H${tight}`,
    `A${tight} ${tight} 0 0 1 0 ${SVG_MARK - tight}`,
    `V${half}`,
    `A${half} ${half} 0 0 1 ${half} 0Z`,
  ].join('');
  return [
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${SVG_SIZE} ${SVG_SIZE}">`,
    `  <g transform="translate(${shift.x.toFixed(3)} ${shift.y.toFixed(3)})">`,
    `    <g transform="translate(${origin} ${origin}) rotate(-45) translate(${-half} ${-half})">`,
    `      <path fill="${LIME_HEX}" d="${path}"/>`,
    `    </g>`,
    `  </g>`,
    `</svg>`,
    ``,
  ].join('\n');
}

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const pngPath = join(root, 'app', 'icon.png');
const svgPath = join(root, 'public', 'favicon.svg');
writeFileSync(pngPath, renderPng());
writeFileSync(svgPath, renderSvg());
console.log(`valo-site: wrote transparent favicon ${pngPath}`);
console.log(`valo-site: wrote ${svgPath}`);
