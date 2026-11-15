// The chief's six expressions (board ChiefMoods): which one a server state shows, its label, its
// state dot and the face geometry for a body's eye positions. Pure; no React.

export type Expression = "calm" | "working" | "done" | "needs" | "stuck" | "paused";

export const EXPRESSIONS: readonly Expression[] = ["calm", "working", "done", "needs", "stuck", "paused"];

export const EXPRESSION_LABEL: Record<Expression, string> = { calm: "Calm", working: "Working", done: "Done", needs: "Needs you", stuck: "Stuck", paused: "Paused" };

/** What the server knows about a Vision or chief that its face shows. */
export interface ExpressionState {
  /** A run is going now. */
  running: boolean;
  /** An approval or open question waits on the person. */
  waitingOnPerson: boolean;
  /** Its latest turn finished with evidence of the work. */
  finished: boolean;
  /** Its latest run failed with no success since. */
  failed: boolean;
  /** It was refused or cannot continue (a wall, a missing permission). */
  blocked: boolean;
  /** The person paused it. */
  paused: boolean;
  /** Its budget stops it running now. */
  overBudget: boolean;
}

/** The one expression a state shows. Most final first: paused (or over budget) outranks all, then
 *  stuck (failed or blocked), needs you, working, done; calm when nothing applies. */
export function expressionFor(state: ExpressionState): Expression {
  if (state.paused || state.overBudget) return "paused";
  if (state.failed || state.blocked) return "stuck";
  if (state.waitingOnPerson) return "needs";
  if (state.running) return "working";
  if (state.finished) return "done";
  return "calm";
}

/** The state dot's colour, an app token. */
export const EXPRESSION_DOT: Record<Expression, string> = {
  calm: "var(--themis-text-2)",
  working: "var(--themis-warning)",
  done: "var(--themis-success)",
  needs: "var(--themis-accent)",
  stuck: "var(--themis-danger)",
  paused: "var(--themis-text-2)",
};

/** The motion class a face plays (Visions.css), none when reduced motion asks for stillness. */
export const EXPRESSION_MOTION: Record<Expression, string> = { calm: "vc-calm", working: "vc-work", done: "vc-calm", needs: "vc-needs", stuck: "vc-worry", paused: "vc-sleep" };

/** The drawn parts of a face, eyes and brows only (board Cloud3D: no mouth in any expression):
 *  path data, each empty when the face has none. */
export interface Face {
  eyesFill: string; glint: string; eyesLine: string; brows: string;
  /** The whole body fades for a paused face. */
  opacity: number;
}

const ell = (cx: number, cy: number, rx: number, ry: number) => `M${cx - rx} ${cy}a${rx} ${ry} 0 1 0 ${2 * rx} 0a${rx} ${ry} 0 1 0 ${-2 * rx} 0Z`;
const EMPTY: Face = { eyesFill: "", glint: "", eyesLine: "", brows: "", opacity: 1 };
const pair = (draw: (x: number) => string, l: number, r: number) => draw(l) + draw(r);

/** The face for `expression` with eyes at x = `l` and `r`, height `y`. Calm, Working, Needs you and
 *  Done are the board's; Stuck and Paused follow them. */
export function faceFor(expression: Expression, l: number, r: number, y: number): Face {
  switch (expression) {
    case "calm": return { ...EMPTY, eyesFill: pair(x => ell(x, y, 1.7, 2.3), l, r), glint: pair(x => ell(x - 0.5, y - 1, 0.55, 0.6), l, r) };
    case "working": return { ...EMPTY, eyesFill: pair(x => ell(x, y + 0.4, 1.6, 1.8), l, r), glint: pair(x => ell(x + 0.5, y - 0.2, 0.5, 0.5), l, r), brows: `M${l - 2.3} ${y - 2.8}l3.2-.5M${r - 1.9} ${y - 3.3}l3.2.5` };
    case "done": return { ...EMPTY, eyesFill: pair(x => `M${x - 1.9} ${y + 0.8}q1.9-2.6 3.8 0q-1.9-1.1-3.8 0Z`, l, r) };
    case "needs": return { ...EMPTY, eyesFill: pair(x => ell(x, y, 2, 2.7), l, r), glint: pair(x => ell(x - 0.7, y - 1.2, 0.7, 0.8), l, r), brows: `M${l - 2.2} ${y - 3.8}q2-1.2 4 0M${r - 2} ${y - 3.8}q2-1.2 4 0` };
    case "stuck": return { ...EMPTY, eyesFill: pair(x => ell(x, y + 0.4, 1.6, 2), l, r), glint: pair(x => ell(x - 0.5, y - 0.4, 0.5, 0.55), l, r), brows: `M${l - 2.2} ${y - 2.6}l4-1.4M${r + 2.2} ${y - 2.6}l-4-1.4` };
    case "paused": return { ...EMPTY, opacity: 0.7, eyesLine: pair(x => `M${x - 2} ${y + 0.6}q2 1.4 4 0`, l, r) };
  }
}
