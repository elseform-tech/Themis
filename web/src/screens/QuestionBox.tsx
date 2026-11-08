import { useEffect, useRef, useState } from "react";
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

function Chevron({ d }: { d: string }) {
  return <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true"><path d={d} /></svg>;
}

/** Which page of a box with several questions is showing, and the way between them. */
export interface QuestionPage {
  index: number;
  count: number;
  /** This page's answer so far, when the person came back to it. */
  chosen?: string;
  onBack?: () => void;
  onNext?: () => void;
}

interface QuestionBoxProps {
  question: Question;
  onAnswer: (answer: string) => Promise<void>;
  /** Puts the question back into its turn; absent for a blocking question. */
  onSkip?: () => void;
  onFold?: () => void;
  /** Set when the box holds several questions: "2/3" with Back and Next. */
  page?: QuestionPage;
  /** Moves focus to the first answer when the box shows (after a page turn). */
  focusFirst?: boolean;
}

function PageNav({ page }: { page: QuestionPage }) {
  return (
    <span className="themis-question-pages">
      <button type="button" className="themis-goal-icon themis-icon-tip" data-tooltip="Back" aria-label="Back" disabled={!page.onBack} onClick={page.onBack}><Chevron d="m15 6-6 6 6 6" /></button>
      <span aria-hidden="true">{page.index + 1}/{page.count}</span>
      <span className="themis-sr-only">Question {page.index + 1} of {page.count}</span>
      <button type="button" className="themis-goal-icon themis-icon-tip" data-tooltip="Next" aria-label="Next" disabled={!page.onNext} onClick={page.onNext}><Chevron d="m9 6 6 6-6 6" /></button>
    </span>
  );
}

/** One answer on offer: what picking it does on the first line, and its advantages,
 *  disadvantages and consequences behind its own disclosure. */
function Choice({ question, option, chosen, disabled, onPick }: { question: Question; option: string; chosen: boolean; disabled: boolean; onPick: (answer: string) => void }) {
  const [open, setOpen] = useState(false);
  const details = question.details?.[option];
  const recommended = option === question.assumption;
  const line = details?.does ?? question.reasons?.[option];
  const why = recommended && details ? question.reasons?.[option] : undefined;
  return (
    <div className="themis-question-option">
      <button type="button" className={`themis-question-choice${recommended ? " is-recommended" : ""}${chosen ? " is-chosen" : ""}`} aria-pressed={chosen || undefined} disabled={disabled} onClick={() => onPick(option)}>
        <span className="themis-question-choice-head">
          <span>{option}</span>
          {recommended && <small className="themis-question-tag">{question.choices?.length ? "Recommended" : "Suggested"}</small>}
        </span>
        {line && <small className="themis-question-reason">{line}</small>}
      </button>
      {details && (
        <button type="button" className="themis-goal-icon themis-icon-tip themis-question-more" data-tooltip={open ? "Less" : "More"} aria-label={`More about ${option}`} aria-expanded={open} onClick={() => setOpen(!open)}>
          <Chevron d={open ? "m6 15 6-6 6 6" : "m6 9 6 6 6-6"} />
        </button>
      )}
      {details && open && (
        <dl className="themis-question-details">
          {why && <><dt>Why recommended</dt><dd>{why}</dd></>}
          <dt>Advantages</dt><dd>{details.advantages}</dd>
          <dt>Disadvantages</dt><dd>{details.disadvantages}</dd>
          <dt>Consequences</dt><dd>{details.consequences}</dd>
        </dl>
      )}
    </div>
  );
}

/** A question from the agent on the composer: the question, its choices one per line
 *  (a choice can be a long sentence) with the one the agent works on meanwhile marked,
 *  a field for another answer, and Skip, which leaves it open in its turn. */
export function QuestionBox({ question, onAnswer, onSkip, onFold, page, focusFirst }: QuestionBoxProps) {
  const options = question.choices?.length ? question.choices : question.assumption ? [question.assumption] : [];
  const [draft, setDraft] = useState(() => page?.chosen !== undefined && !options.includes(page.chosen) ? page.chosen : "");
  const [sending, setSending] = useState(false);
  const box = useRef<HTMLElement>(null);
  const anchor = `themis-question-${question.id}`;

  useEffect(() => {
    if (focusFirst) box.current?.querySelector<HTMLElement>(".themis-question-choice, .themis-question-input")?.focus();
  }, [focusFirst]);

  async function send(answer: string) {
    if (sending || answer.trim() === "") return;
    setSending(true);
    try { await onAnswer(answer.trim()); setDraft(""); } catch { /* the caller says why */ } finally { setSending(false); }
  }

  return (
    <section ref={box} id={anchor} className="themis-question" aria-label="Question from the agent">
      <div className="themis-question-head">
        <QuestionMark />
        <p className="themis-question-text">{question.question}</p>
        {page && <PageNav page={page} />}
        {onFold && (
          <button type="button" className="themis-goal-icon themis-icon-tip" data-tooltip="Fold to icons" aria-label="Fold to icons" onClick={onFold}>
            <Chevron d="m6 9 6 6 6-6" />
          </button>
        )}
      </div>
      <div className="themis-question-body">
        {options.length > 0 && (
          <div className="themis-question-choices">
            {options.map(option => <Choice key={option} question={question} option={option} chosen={page?.chosen === option} disabled={sending} onPick={answer => void send(answer)} />)}
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
