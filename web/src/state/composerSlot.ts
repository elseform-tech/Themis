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

/** How long a blocking question waits on the composer by default: the approval timeout's
 *  default (Settings › Permissions › Approval timeout, 10 minutes). */
export const BLOCKING_OPEN_MS = 600 * 1000;

/** How long an unanswered question stays on the composer: `QUESTION_OPEN_MS`, or for one the
 *  work waits on, the approval timeout (`blockingMs`). Then it moves into its turn as one line
 *  and nothing starts; answering it there still starts the next turn. */
export function openFor(question: Question, blockingMs: number): number {
  return question.blocking ? blockingMs : QUESTION_OPEN_MS;
}

/** Whether an unanswered question is on the composer at `now`: until it is put back into its
 *  turn or its wait (`openFor`) passes, unless the person opened it again. */
export function questionOnComposer(question: Question, placements: ReadonlyMap<string, Placement>, now: number, blockingMs = BLOCKING_OPEN_MS): boolean {
  if (question.answer !== undefined) return false;
  const placement = placements.get(question.id);
  if (placement) return placement === "composer";
  return now - question.asked_at < openFor(question, blockingMs);
}

export type ComposerItem =
  | { kind: "approval"; request: ApprovalRequest }
  /** The pages of one box: questions asked together (one `group`), in the order asked. */
  | { kind: "question"; questions: Question[] };

/** Open questions as boxes, in the order given (a box where its first question falls): the
 *  questions of one group are one box, its pages in the order asked. */
function questionBoxes(open: Question[]): Question[][] {
  const boxes = new Map<string, Question[]>();
  for (const question of open) {
    const key = question.group ?? question.id;
    boxes.set(key, [...boxes.get(key) ?? [], question]);
  }
  return [...boxes.values()].map(pages => [...pages].sort((a, b) => a.asked_at - b.asked_at));
}

/** Open questions in the order the composer shows them: the one the person opened from its turn
 *  most recently first (they just asked for it), then the rest oldest first. */
function questionOrder(open: Question[], placements: ReadonlyMap<string, Placement>): Question[] {
  const opened = [...placements].filter(([, placement]) => placement === "composer").map(([id]) => id);
  const rank = (question: Question) => opened.lastIndexOf(question.id);
  return [...open].sort((a, b) => rank(b) - rank(a) || a.asked_at - b.asked_at);
}

/** What the composer shows, one at a time: approvals first (the run waits on them), then
 *  questions, the one just opened from its turn first and otherwise oldest first, those asked
 *  together as the pages of one box (in the order asked); `waiting` is how many more are
 *  behind it. */
export function composerQueue(approvals: ApprovalRequest[], questions: Question[], placements: ReadonlyMap<string, Placement>, now: number, blockingMs = BLOCKING_OPEN_MS): { current: ComposerItem | null; waiting: { approvals: number; questions: number } } {
  const open = questionOrder(questions.filter(question => questionOnComposer(question, placements, now, blockingMs)), placements);
  const items: ComposerItem[] = [
    ...approvals.map(request => ({ kind: "approval" as const, request })),
    ...questionBoxes(open).map(questions => ({ kind: "question" as const, questions })),
  ];
  const current = items[0] ?? null;
  return {
    current,
    waiting: {
      approvals: approvals.length - (current?.kind === "approval" ? 1 : 0),
      questions: open.length - (current?.kind === "question" ? current.questions.length : 0),
    },
  };
}

export function readFolded(): boolean {
  return readSession<boolean>(FOLDED_KEY, false) === true;
}

export function writeFolded(folded: boolean) {
  writeSession(FOLDED_KEY, folded);
}
