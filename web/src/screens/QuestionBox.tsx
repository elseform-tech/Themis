import { useEffect, useState } from "react";
import type { Question } from "../lib/types";
import "./QuestionBox.css";

/** The current time, updated when `at` (a time in the future, if any) arrives. */
export function useNowAt(at: number | undefined): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (at === undefined || at <= now) return;
    const timer = window.setTimeout(() => setNow(Date.now()), at - now + 50);
    return () => window.clearTimeout(timer);
  }, [at, now]);
  return now;
}

export function QuestionMark({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <circle cx="12" cy="12" r="9" /><path d="M9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.6.3-1 .9-1 1.6V14" /><path d="M12 17h.01" />
    </svg>
  );
}

interface QuestionBoxProps {
  question: Question;
  onAnswer: (answer: string) => Promise<void>;
  /** Puts the question back into its turn; absent for a blocking question. */
  onSkip?: () => void;
  onFold?: () => void;
}

/** A question from the agent on the composer: the question, its choices one per line
 *  (a choice can be a long sentence) with the one the agent works on meanwhile marked,
 *  a field for another answer, and Skip, which leaves it open in its turn. */
export function QuestionBox({ question, onAnswer, onSkip, onFold }: QuestionBoxProps) {
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const anchor = `themis-question-${question.id}`;
  const options = question.choices?.length ? question.choices : question.assumption ? [question.assumption] : [];

  async function send(answer: string) {
    if (sending || answer.trim() === "") return;
    setSending(true);
    try { await onAnswer(answer.trim()); setDraft(""); } catch { /* the caller says why */ } finally { setSending(false); }
  }

  return (
    <section id={anchor} className="themis-question" aria-label="Question from the agent">
      <div className="themis-question-head">
        <QuestionMark />
        <p className="themis-question-text">{question.question}</p>
        {onFold && (
          <button type="button" className="themis-goal-icon themis-icon-tip" data-tooltip="Fold to icons" aria-label="Fold to icons" onClick={onFold}>
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true"><path d="m6 9 6 6 6-6" /></svg>
          </button>
        )}
      </div>
      <div className="themis-question-body">
        {options.length > 0 && (
          <div className="themis-question-choices">
            {options.map(option => (
              <button key={option} type="button" className={`themis-question-choice${option === question.assumption ? " is-recommended" : ""}`} disabled={sending} onClick={() => void send(option)}>
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
          {draft.trim() !== "" ? (
            <button type="submit" className="themis-goal-icon is-primary themis-icon-tip" data-tooltip="Send answer" aria-label="Send answer" disabled={sending}>
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M5 12h14M13 6l6 6-6 6" /></svg>
            </button>
          ) : onSkip && (
            <button type="button" className="themis-question-skip" onClick={onSkip}>Skip</button>
          )}
        </form>
      </div>
    </section>
  );
}

/** A question in the turn it was asked in, as one line: with its answer, or, while it is
 *  open and not on the composer, a button that puts it back there. */
export function QuestionNote({ question, onComposer, onOpen }: { question: Question; onComposer: boolean; onOpen: () => void }) {
  const anchor = `themis-question-${question.id}`;
  if (question.answer !== undefined) {
    return (
      <div id={anchor} className="themis-question-note" role="note" title={`${question.question} — ${question.answer}`}>
        <QuestionMark size={14} />
        <span className="themis-question-line">{question.question}<span className="themis-question-answer"> — {question.answer}</span></span>
      </div>
    );
  }
  if (onComposer) return null;
  return (
    <button id={anchor} type="button" className="themis-question-note is-open" title="Answer it on the composer" onClick={onOpen}>
      <QuestionMark size={14} />
      <span className="themis-question-line"><strong>Open question</strong> · {question.question}</span>
    </button>
  );
}
