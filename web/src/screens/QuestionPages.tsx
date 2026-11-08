import { useState } from "react";
import type { Question } from "../lib/types";
import { QuestionBox } from "./QuestionBox";

export interface PageAnswer {
  questionId: string;
  answer: string;
}

interface QuestionPagesProps {
  /** The questions asked together, in the order asked; one is a plain question box. */
  pages: Question[];
  /** Sends the answers given, together, once the last page is answered or skipped. */
  onSend: (answers: PageAnswer[]) => Promise<void>;
  /** Puts the questions left unanswered back into their turns. */
  onLeave: (ids: string[]) => void;
  onFold?: () => void;
}

/** One question box for the questions the agent asked together: a page each ("1/3"), Back
 *  and Next between them. Picking or writing an answer, or Skip, turns to the next page; on
 *  the last page the answers given go to the agent together, and the skipped questions stay
 *  open in their turns. A question the work waits on has no Skip. */
export function QuestionPages({ pages, onSend, onLeave, onFold }: QuestionPagesProps) {
  const [index, setIndex] = useState(0);
  const [answers, setAnswers] = useState<Record<string, string>>({});
  const [turned, setTurned] = useState(false);
  const at = Math.min(index, pages.length - 1);
  const page = pages[at]!;
  const last = at === pages.length - 1;

  function go(to: number) {
    setIndex(to);
    setTurned(true);
  }

  async function finish(given: Record<string, string>) {
    const sent = pages.flatMap(question => given[question.id] === undefined ? [] : [{ questionId: question.id, answer: given[question.id]! }]);
    if (sent.length) await onSend(sent);
    const left = pages.filter(question => given[question.id] === undefined).map(question => question.id);
    if (left.length) onLeave(left);
  }

  async function answer(text: string) {
    const given = { ...answers, [page.id]: text };
    setAnswers(given);
    if (last) await finish(given);
    else go(at + 1);
  }

  function skip() {
    const given = Object.fromEntries(Object.entries(answers).filter(([id]) => id !== page.id));
    setAnswers(given);
    if (last) void finish(given).catch(() => undefined);
    else go(at + 1);
  }

  const chosen = answers[page.id];
  return (
    <QuestionBox key={page.id} question={page} onAnswer={answer} onSkip={page.blocking ? undefined : skip} onFold={onFold} focusFirst={turned}
      page={pages.length > 1 ? {
        index: at,
        count: pages.length,
        chosen,
        onBack: at > 0 ? () => go(at - 1) : undefined,
        onNext: !last && chosen !== undefined ? () => go(at + 1) : undefined,
      } : undefined} />
  );
}
