// The 3D chief's face: the board's face shapes (`faceFor`) drawn onto a canvas that becomes the
// face texture, and the board's per-expression motion as a pure function of time. No three.js here.
import { faceFor, type Expression, type Face } from "../Expression";

/** Texture pixels per scene unit. The face covers the board's 48-unit square. */
export const FACE_SCALE = 8;
export const FACE_UNITS = 48;

/** How far the body moves: a scale about its feet and a shift, in scene units (dy up). */
export interface Offset { scale: number; dx: number; dy: number }

/** Linear keyframes [fraction of the period, value], the board's CSS keyframes. */
function keyframes(points: Array<[number, number]>, fraction: number): number {
  for (let i = 1; i < points.length; i++) {
    const [f1, v1] = points[i]!;
    const [f0, v0] = points[i - 1]!;
    if (fraction <= f1) return v0 + (v1 - v0) * ((fraction - f0) / (f1 - f0 || 1));
  }
  return points[points.length - 1]![1];
}
const wave = (t: number, period: number) => (1 - Math.cos((2 * Math.PI * t) / period)) / 2;

/** The motion of `expression` `t` seconds in (ChiefMoods: breathe, bob, hop, shiver, slow breathe). */
export function motionOffset(expression: Expression, t: number): Offset {
  switch (expression) {
    case "calm": case "done": return { scale: 1 + 0.04 * wave(t, 4.5), dx: 0, dy: 0 };
    case "working": return { scale: 1, dx: 0, dy: 1.2 * wave(t, 1.3) };
    case "needs": return { scale: 1, dx: 0, dy: keyframes([[0, 0], [0.7, 0], [0.78, 3], [0.86, 0], [0.92, 1], [1, 0]], (t % 3) / 3) };
    case "stuck": return { scale: 1, dx: keyframes([[0, 0], [0.2, -0.4], [0.4, 0.4], [0.6, 0], [1, 0]], (t % 2.6) / 2.6), dy: 0 };
    case "paused": return { scale: 1 + 0.04 * wave(t, 7), dx: 0, dy: 0 };
  }
}

/** Whether the eyes are shut at `t`: calm and done faces blink briefly every five seconds. */
export function blinking(expression: Expression, t: number): boolean {
  if (expression !== "calm" && expression !== "done") return false;
  const f = (t % 5) / 5;
  return f > 0.93 && f < 0.97;
}

const INK = "#0B1220";

/** Draws `face` on `ctx` (a FACE_UNITS square at FACE_SCALE); `eyeY` is where a blink squashes the eyes. */
export function drawFace(ctx: CanvasRenderingContext2D, face: Face, eyeY: number, blink: boolean): void {
  const size = FACE_UNITS * FACE_SCALE;
  ctx.clearRect(0, 0, size, size);
  ctx.save();
  ctx.scale(FACE_SCALE, FACE_SCALE);
  ctx.lineCap = "round"; ctx.lineJoin = "round";
  const fill = (d: string, color: string, alpha = 1) => { if (!d) return; ctx.globalAlpha = alpha; ctx.fillStyle = color; ctx.fill(new Path2D(d)); ctx.globalAlpha = 1; };
  const stroke = (d: string, width: number) => { if (!d) return; ctx.strokeStyle = INK; ctx.lineWidth = width; ctx.stroke(new Path2D(d)); };
  fill(face.cheeks, "#FF8FB0", 0.6);
  ctx.save();
  if (blink) { ctx.translate(0, eyeY); ctx.scale(1, 0.1); ctx.translate(0, -eyeY); }
  fill(face.eyesFill, INK);
  stroke(face.eyesLine, 1.6);
  ctx.restore();
  if (!blink) fill(face.glint, "#fff");
  stroke(face.brows, 1.2);
  if (face.mouth) { fill(face.mouthFill === "none" ? "" : face.mouth, face.mouthFill); stroke(face.mouth, 1.2); }
  ctx.restore();
}

/** The face for an expression at a body's eye positions (re-exported for the scene). */
export { faceFor };
