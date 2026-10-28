import { useSyncExternalStore } from "react";
import type { ApprovalRequest, Question } from "../lib/types";
import { readSession, writeSession } from "./session";

/** How long an unanswered question that does not block the work stays on the composer
 *  before it moves back into its turn as one line. */
export const QUESTION_OPEN_MS = 5 * 60 * 1000;

const PLACEMENT_KEY = "composerQuestions";
const FOLDED_KEY = "composerFolded";
const MAX_PLACED = 200;

/** Where the person put an open question: back into its turn (Skip) or onto the composer
 *  again (opened from its turn). A per-viewer convenience; the time rule decides the rest. */
export type Placement = "turn" | "composer";
let placed = new Map<string, Placement>(Object.entries(readSession<Record<string, Placement>>(PLACEMENT_KEY, {})));
const listeners = new Set<() => void>();

/** Puts an open question back into its turn, or onto the composer. */
export function placeQuestion(id: string, placement: Placement) {
  const next = new Map(placed);
  next.delete(id);
  next.set(id, placement);
  placed = new Map([...next].slice(-MAX_PLACED));
  writeSession(PLACEMENT_KEY, Object.fromEntries(placed));
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

export function usePlacements(): ReadonlyMap<string, Placement> {
  return useSyncExternalStore(subscribe, () => placed);
}

/** Whether an unanswered question is on the composer at `now`: a blocking one always
 *  (the work waits for it); another until it is put back into its turn or
 *  `QUESTION_OPEN_MS` passes, unless the person opened it again. */
export function questionOnComposer(question: Question, placements: ReadonlyMap<string, Placement>, now: number): boolean {
  if (question.answer !== undefined) return false;
  if (question.blocking) return true;
  const placement = placements.get(question.id);
  if (placement) return placement === "composer";
  return now - question.asked_at < QUESTION_OPEN_MS;
}

export type ComposerItem =
  | { kind: "approval"; request: ApprovalRequest }
  | { kind: "question"; question: Question };

/** What the composer shows, one at a time: approvals first (the run waits on them), then
 *  questions, oldest first; `waiting` is how many more are behind it. */
export function composerQueue(approvals: ApprovalRequest[], questions: Question[], placements: ReadonlyMap<string, Placement>, now: number): { current: ComposerItem | null; waiting: { approvals: number; questions: number } } {
  const open = questions.filter(question => questionOnComposer(question, placements, now)).sort((a, b) => a.asked_at - b.asked_at);
  const items: ComposerItem[] = [
    ...approvals.map(request => ({ kind: "approval" as const, request })),
    ...open.map(question => ({ kind: "question" as const, question })),
  ];
  const current = items[0] ?? null;
  return {
    current,
    waiting: {
      approvals: approvals.length - (current?.kind === "approval" ? 1 : 0),
      questions: open.length - (current?.kind === "question" ? 1 : 0),
    },
  };
}

export function readFolded(): boolean {
  return readSession<boolean>(FOLDED_KEY, false) === true;
}

export function writeFolded(folded: boolean) {
  writeSession(FOLDED_KEY, folded);
}
