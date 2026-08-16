/**
 * One scene, drawn twice: once through valo's Canvas2D layer and once through
 * the browser's own `CanvasRenderingContext2D`.
 *
 * This is the compatibility claim made checkable on the page. It is the same
 * claim the differential conformance suite makes offline, where every scene is
 * rendered both ways and the pixels are compared — the difference is only that
 * the suite can fail the build and this can only be looked at.
 *
 * The scene follows the demo cards' design language — near-black ramp, paper
 * type, grey hairlines, one lime signal — but says it entirely in Canvas2D:
 * gradients, arcs, `fillText`, `setLineDash`.
 */
export const PARITY_FONT_FAMILY = 'Valo Site Sans';

export const paritySource = `const ramp = ctx.createLinearGradient(0, 0, 0, 200);
ramp.addColorStop(0, '#0b0b0e');
ramp.addColorStop(0.65, '#15171c');
ramp.addColorStop(1, '#0b0b0e');
ctx.fillStyle = ramp;
ctx.fillRect(0, 0, 320, 200);

ctx.fillStyle = 'rgba(119, 128, 140, 0.10)';
for (let line = 0; line < 3; line += 1) {
  ctx.fillRect(24, 132 + line * 16, 272, 1);
}

const glow = ctx.createRadialGradient(243, 78, 0, 243, 78, 54);
glow.addColorStop(0, 'rgba(127, 159, 255, 0.16)');
glow.addColorStop(1, 'rgba(127, 159, 255, 0)');
ctx.fillStyle = glow;
ctx.beginPath();
ctx.arc(243, 78, 54, 0, Math.PI * 2);
ctx.fill();

ctx.strokeStyle = 'rgba(233, 237, 228, 0.14)';
ctx.lineWidth = 1;
ctx.beginPath();
ctx.arc(243, 78, 40, 0, Math.PI * 2);
ctx.stroke();
ctx.beginPath();
ctx.arc(243, 78, 26, 0, Math.PI * 2);
ctx.stroke();

const angle = -0.6;
const tipX = 243 + 40 * Math.cos(angle);
const tipY = 78 + 40 * Math.sin(angle);
ctx.strokeStyle = 'rgba(233, 237, 228, 0.35)';
ctx.beginPath();
ctx.moveTo(243, 78);
ctx.lineTo(tipX, tipY);
ctx.stroke();
ctx.fillStyle = '#c8ff3d';
ctx.beginPath();
ctx.arc(tipX, tipY, 2.5, 0, Math.PI * 2);
ctx.fill();

ctx.textBaseline = 'alphabetic';
ctx.fillStyle = '#e9ede4';
ctx.font = "600 26px '" + FONT + "'";
ctx.fillText('Same code.', 24, 64);
ctx.fillStyle = 'rgba(119, 128, 140, 0.9)';
ctx.font = "400 26px '" + FONT + "'";
ctx.fillText('Same pixels.', 24, 96);
ctx.fillStyle = '#c8ff3d';
ctx.fillRect(24, 110, 34, 3);

ctx.strokeStyle = 'rgba(233, 237, 228, 0.35)';
ctx.lineWidth = 2;
ctx.lineCap = 'round';
ctx.setLineDash([2, 8]);
ctx.beginPath();
ctx.moveTo(24, 176);
ctx.lineTo(296, 176);
ctx.stroke();
ctx.strokeStyle = '#c8ff3d';
ctx.beginPath();
ctx.moveTo(24, 176);
ctx.lineTo(72, 176);
ctx.stroke();`;

/** The scene's design size. Both canvases are backed at this size × dpr. */
export const PARITY_SIZE = { width: 320, height: 200 } as const;
