import { useState } from "react";
import { Button } from "../components";
import type { Question } from "../lib/types";
import { QuestionBox, QuestionMark } from "./QuestionBox";

export interface PageAnswer {
  questionId: string;
  answer: string;
}

interface QuestionPagesProps {
  /** The questions asked together, in the order asked; one is a plain question box. */
  pages: Question[];
  /** Sends the answers given, together. */
  onSend: (answers: PageAnswer[]) => Promise<void>;
  /** Puts the questions left unanswered back into their turns. */
  onLeave: (ids: string[]) => void;
  onFold?: () => void;
}

/** The answers before they go: each question with its answer (or "Skipped"), a click goes
 *  back to that page, and Submit sends them all. */
function Review({ pages, answers, onEdit, onSubmit }: { pages: Question[]; answers: Record<string, string>; onEdit: (index: number) => void; onSubmit: () => Promise<void> }) {
  const [sending, setSending] = useState(false);
  async function submit() {
    setSending(true);
    try { await onSubmit(); } catch { /* the caller says why */ } finally { setSending(false); }
  }
  return (
    <section className="themis-question" aria-label="Your answers">
      <div className="themis-question-head">
        <QuestionMark />
        <p className="themis-question-text">Your answers</p>
      </div>
      <div className="themis-question-body">
        <ul className="themis-question-review">
          {pages.map((page, index) => (
            <li key={page.id}>
              <button type="button" aria-label={`Change: ${page.question}`} onClick={() => onEdit(index)}>
                <span>{page.question}</span>
                <span className={`themis-question-review-answer${answers[page.id] === undefined ? " is-skipped" : ""}`}>{answers[page.id] ?? "Skipped"}</span>
              </button>
            </li>
          ))}
        </ul>
        <Button className="themis-question-submit" variant="primary" size="small" disabled={sending} onClick={() => void submit()}>Submit</Button>
      </div>
    </section>
  );
}

/** One question box for the questions the agent asked together: a page each ("1/3"), Back
 *  and Next between them. Picking or writing an answer, or Skip, turns to the next page; after
 *  the last, a review lists every answer, a click goes back to change one, and Submit sends
 *  them together (skipped questions stay open in their turns). A single question sends at
 *  once. A question the work waits on has no Skip. */
export function QuestionPages({ pages, onSend, onLeave, onFold }: QuestionPagesProps) {
  const [index, setIndex] = useState(0);
  const [answers, setAnswers] = useState<Record<string, string>>({});
  const [review, setReview] = useState(false);
  const [reviewed, setReviewed] = useState(false);
  const [turned, setTurned] = useState(false);
  const at = Math.min(index, pages.length - 1);
  const page = pages[at]!;
  const last = at === pages.length - 1;

  async function finish(given: Record<string, string>) {
    const sent = pages.flatMap(question => given[question.id] === undefined ? [] : [{ questionId: question.id, answer: given[question.id]! }]);
    if (sent.length) await onSend(sent);
    const left = pages.filter(question => given[question.id] === undefined).map(question => question.id);
    if (left.length) onLeave(left);
  }

  function go(to: number) {
    setIndex(to);
    setReview(false);
    setTurned(true);
  }

  /** After a page is answered or skipped: the next page, or the review once every page was seen. */
  function onward() {
    if (last || reviewed) {
      setReview(true);
      setReviewed(true);
    } else go(at + 1);
  }

  async function answer(text: string) {
    const given = { ...answers, [page.id]: text };
    setAnswers(given);
    if (pages.length === 1) await finish(given);
    else onward();
  }

  function skip() {
    const given = Object.fromEntries(Object.entries(answers).filter(([id]) => id !== page.id));
    setAnswers(given);
    if (pages.length === 1) void finish(given).catch(() => undefined);
    else onward();
  }

  if (review) return <Review pages={pages} answers={answers} onEdit={go} onSubmit={() => finish(answers)} />;
  const chosen = answers[page.id];
  return (
    <QuestionBox key={page.id} question={page} onAnswer={answer} onSkip={page.blocking ? undefined : skip} onFold={onFold} focusFirst={turned}
      page={pages.length > 1 ? {
        index: at,
        count: pages.length,
        chosen,
        onBack: at > 0 ? () => go(at - 1) : undefined,
        onNext: chosen !== undefined ? onward : undefined,
      } : undefined} />
  );
}
