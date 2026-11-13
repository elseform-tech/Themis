// The autonomy ladder as Settings › Visions and a Vision's profile show it. Which decisions each
// preset hands the Vision mirrors `vision_decides` in crates/core/src/visions.rs; the server
// applies it, this only shows it.
import type { Autonomy } from "../../lib/visions";

/** The five stops, in slider order. */
export const AUTONOMY_STOPS: Autonomy[] = ["ask", "small", "most", "nearly_all", "custom"];

/** The autonomy levels as Settings › Visions names them. */
export const LEVELS: Record<Autonomy, string> = { ask: "Ask me first", small: "Small calls", most: "Most calls", nearly_all: "Nearly all", custom: "Custom" };

/** Each kind of decision (its `visions.custom` id), its words, and the preset rank that hands it
 *  to the Vision; `null` rows are always the person's. */
export const DECISIONS: Array<{ id: string; label: string; rank: number | null }> = [
  { id: "wording", label: "Wording, names and formatting", rank: 1 },
  { id: "tests", label: "Which tests to add, small fixes", rank: 1 },
  { id: "undoable", label: "Choices it can undo, inside the channel’s goal", rank: 2 },
  { id: "retry", label: "Retrying or re-planning after a failure", rank: 2 },
  { id: "scope", label: "Changing the goal or the scope", rank: 3 },
  { id: "delete", label: "Deleting or replacing work", rank: 3 },
  { id: "dependency", label: "A new dependency or a security setting", rank: 3 },
  { id: "external", label: "Anything that leaves this computer", rank: null },
  { id: "spend", label: "Spending past the budget", rank: null },
];

/** A level's place on the preset ladder; Custom has none. */
export function rankOf(level: Autonomy): number | null {
  const rank = AUTONOMY_STOPS.indexOf(level);
  return level === "custom" ? null : rank;
}

/** Whether a preset hands this decision to the Vision. */
export function presetDecides(level: Autonomy, decision: { rank: number | null }): boolean {
  const rank = rankOf(level);
  return rank !== null && decision.rank !== null && rank >= decision.rank;
}
