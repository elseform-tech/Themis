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

/** The drawn parts of a face: path data, each empty when the face has none. */
export interface Face {
  eyesFill: string; glint: string; eyesLine: string; brows: string; mouth: string; mouthFill: string; cheeks: string;
  /** The whole body fades for a paused face. */
  opacity: number;
}

const ell = (cx: number, cy: number, rx: number, ry: number) => `M${cx - rx} ${cy}a${rx} ${ry} 0 1 0 ${2 * rx} 0a${rx} ${ry} 0 1 0 ${-2 * rx} 0Z`;
const EMPTY: Face = { eyesFill: "", glint: "", eyesLine: "", brows: "", mouth: "", mouthFill: "none", cheeks: "", opacity: 1 };

/** The face for `expression` with eyes at x = `l` and `r`, height `y`. */
export function faceFor(expression: Expression, l: number, r: number, y: number): Face {
  const mid = (l + r) / 2;
  switch (expression) {
    case "calm": return { ...EMPTY, eyesFill: ell(l, y, 1.7, 2.3) + ell(r, y, 1.7, 2.3), glint: ell(l - 0.5, y - 1, 0.55, 0.6) + ell(r - 0.5, y - 1, 0.55, 0.6), mouth: `M${mid - 1.8} ${y + 4.2}q1.8 1.4 3.6 0` };
    case "working": return { ...EMPTY, eyesFill: ell(l + 0.5, y - 0.8, 1.6, 1.8) + ell(r + 0.5, y - 0.8, 1.6, 1.8), glint: ell(l + 1, y - 1.5, 0.5, 0.55) + ell(r + 1, y - 1.5, 0.5, 0.55), brows: `M${l - 1.9} ${y - 3.8}l3.4-.6M${r - 1.5} ${y - 4.4}l3.4.6`, mouth: `M${mid - 1.3} ${y + 4.4}h2.6` };
    case "done": return { ...EMPTY, eyesLine: `M${l - 2} ${y + 0.4}q2-2.6 4 0M${r - 2} ${y + 0.4}q2-2.6 4 0`, mouth: `M${mid - 2.8} ${y + 3.4}q2.8 3.2 5.6 0Z`, mouthFill: "#5B1830", cheeks: ell(l - 3.6, y + 2.6, 1.8, 1.1) + ell(r + 3.6, y + 2.6, 1.8, 1.1) };
    case "needs": return { ...EMPTY, eyesFill: ell(l, y - 0.5, 2, 2.8) + ell(r, y - 0.5, 2, 2.8), glint: ell(l - 0.7, y - 1.7, 0.7, 0.8) + ell(r - 0.7, y - 1.7, 0.7, 0.8), brows: `M${l - 2.2} ${y - 5}q2.2-1.4 4.4 0M${r - 2.2} ${y - 5}q2.2-1.4 4.4 0`, mouth: ell(mid, y + 5, 1, 1.1), mouthFill: "#5B1830" };
    case "stuck": return { ...EMPTY, eyesFill: ell(l, y + 0.4, 1.6, 2) + ell(r, y + 0.4, 1.6, 2), glint: ell(l - 0.5, y - 0.4, 0.5, 0.55) + ell(r - 0.5, y - 0.4, 0.5, 0.55), brows: `M${l - 2.2} ${y - 2.6}l4-1.4M${r + 2.2} ${y - 2.6}l-4-1.4`, mouth: `M${mid - 2.4} ${y + 5.4}q1.2-1.2 2.4 0t2.4 0` };
    case "paused": return { ...EMPTY, opacity: 0.7, eyesLine: `M${l - 2} ${y + 0.6}q2 1.4 4 0M${r - 2} ${y + 0.6}q2 1.4 4 0`, mouth: `M${mid - 1} ${y + 4.6}h2` };
  }
}
