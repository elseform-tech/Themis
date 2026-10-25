import { useEffect, useState } from "react";
import type { Question } from "../lib/types";
import "./QuestionBox.css";

/** How long an unanswered question stays open in full before it shrinks to one line. */
export const QUESTION_OPEN_MS = 5 * 60 * 1000;

/** Whether an unanswered question shows as one line by now: never while it blocks the work. */
export function questionCollapsed(question: Question, now: number): boolean {
  return !question.blocking && now - question.asked_at >= QUESTION_OPEN_MS;
}

/** The current time, updated when `at` (a time in the future, if any) arrives. */
function useNowAt(at: number | undefined): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (at === undefined || at <= now) return;
    const timer = window.setTimeout(() => setNow(Date.now()), at - now + 50);
    return () => window.clearTimeout(timer);
  }, [at, now]);
  return now;
}

interface QuestionBoxProps {
  question: Question;
  onAnswer: (answer: string) => Promise<void>;
}

function QuestionMark() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <circle cx="12" cy="12" r="9" /><path d="M9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.6.3-1 .9-1 1.6V14" /><path d="M12 17h.01" />
    </svg>
  );
}

/** A question the agent asked without stopping its work. Open in full until it is
 *  answered or [`QUESTION_OPEN_MS`] passes; then one line that opens again to answer
 *  later. Answered: one quiet line with the answer. */
export function QuestionBox({ question, onAnswer }: QuestionBoxProps) {
  const [opened, setOpened] = useState(false);
  const now = useNowAt(question.answer === undefined && !question.blocking ? question.asked_at + QUESTION_OPEN_MS : undefined);
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const anchor = `themis-question-${question.id}`;
  const options = question.choices?.length ? question.choices : question.assumption ? [question.assumption] : [];

  if (question.answer !== undefined) {
    return (
      <div id={anchor} className="themis-question is-answered" role="note" title={`${question.question} — ${question.answer}`}>
        <QuestionMark />
        <span className="themis-question-line">{question.question}<span className="themis-question-answer"> — {question.answer}</span></span>
      </div>
    );
  }

  async function send(answer: string) {
    if (sending || answer.trim() === "") return;
    setSending(true);
    try { await onAnswer(answer.trim()); setDraft(""); } catch { /* the caller says why */ } finally { setSending(false); }
  }

  if (questionCollapsed(question, now) && !opened) {
    return (
      <button id={anchor} type="button" className="themis-question is-collapsed" onClick={() => setOpened(true)}>
        <QuestionMark />
        <span className="themis-question-line"><strong>Open question</strong> · {question.question}</span>
      </button>
    );
  }

  return (
    <section id={anchor} className="themis-question" aria-label="Question from the agent">
      <QuestionMark />
      <div className="themis-question-body">
        <p className="themis-question-text">{question.question}</p>
        {/* 2–4 choices with the agent's recommendation first, or its one suggestion; the
            agent works on that answer until the person picks or writes another. */}
        {options.length > 0 && (
          <div className="themis-question-choices">
            {options.map(option => (
              <button key={option} type="button" className="themis-question-choice" disabled={sending} onClick={() => void send(option)}>
                <span className="themis-question-choice-head">
                  <span>{option}</span>
                  {option === question.assumption && <small className="themis-question-tag">{question.choices?.length ? "Recommended" : "Suggested"}</small>}
                </span>
                {question.reasons?.[option] && <small className="themis-question-reason">{question.reasons[option]}</small>}
              </button>
            ))}
          </div>
        )}
        <form className="themis-question-form" onSubmit={event => { event.preventDefault(); void send(draft); }}>
          <label className="themis-sr-only" htmlFor={`${anchor}-answer`}>Your answer</label>
          <input id={`${anchor}-answer`} className="themis-question-input" value={draft} placeholder={options.length ? "Or write your answer" : "Your answer"}
            maxLength={10000} disabled={sending} onChange={event => setDraft(event.target.value)} />
          <button type="submit" className="themis-goal-icon is-primary themis-icon-tip" data-tooltip="Send answer" aria-label="Send answer" disabled={sending || draft.trim() === ""}>
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M5 12h14M13 6l6 6-6 6" /></svg>
          </button>
        </form>
      </div>
    </section>
  );
}

/** How many unanswered questions in this chat have shrunk to one line; jumps to the oldest. */
export function OpenQuestions({ questions }: { questions: Question[] }) {
  const open = questions.filter(question => question.answer === undefined);
  const next = open.filter(question => !question.blocking).map(question => question.asked_at + QUESTION_OPEN_MS).filter(at => at > Date.now());
  const now = useNowAt(next.length ? Math.min(...next) : undefined);
  const ids = open.filter(question => questionCollapsed(question, now)).map(question => question.id);
  if (ids.length === 0) return null;
  return (
    <button type="button" className="themis-open-questions" onClick={() => document.getElementById(`themis-question-${ids[0]}`)?.scrollIntoView({ behavior: "smooth", block: "center" })}>
      <QuestionMark />
      {ids.length === 1 ? "1 open question" : `${ids.length} open questions`}
    </button>
  );
}
