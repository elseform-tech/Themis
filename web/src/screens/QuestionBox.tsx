import { useState } from "react";
import type { Question } from "../lib/types";
import "./QuestionBox.css";

interface QuestionBoxProps {
  question: Question;
  /** Nothing came after it yet: the agent has not moved on, so it shows in full. */
  latest: boolean;
  onAnswer: (answer: string) => Promise<void>;
}

function QuestionMark() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <circle cx="12" cy="12" r="9" /><path d="M9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.6.3-1 .9-1 1.6V14" /><path d="M12 17h.01" />
    </svg>
  );
}

/** A question the agent asked without stopping its work. Open while the agent has not
 *  moved on; then one line to expand and answer later. Answered: one quiet line. */
export function QuestionBox({ question, latest, onAnswer }: QuestionBoxProps) {
  const [opened, setOpened] = useState(false);
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const anchor = `themis-question-${question.id}`;

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

  if (!latest && !opened) {
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
        {/* A blocking question has nothing to work on until the answer comes. */}
        {question.assumption && !question.blocking && <p className="themis-question-assumption">Working on: {question.assumption}</p>}
        {!!question.choices?.length && (
          <div className="themis-question-choices">
            {question.choices.map(choice => (
              <button key={choice} type="button" className="themis-question-choice" disabled={sending} onClick={() => void send(choice)}>{choice}</button>
            ))}
          </div>
        )}
        <form className="themis-question-form" onSubmit={event => { event.preventDefault(); void send(draft); }}>
          <label className="themis-sr-only" htmlFor={`${anchor}-answer`}>Your answer</label>
          <input id={`${anchor}-answer`} className="themis-question-input" value={draft} placeholder={question.choices?.length ? "Or write your answer" : "Your answer"}
            maxLength={10000} disabled={sending} onChange={event => setDraft(event.target.value)} />
          <button type="submit" className="themis-goal-icon is-primary themis-icon-tip" data-tooltip="Send answer" aria-label="Send answer" disabled={sending || draft.trim() === ""}>
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M5 12h14M13 6l6 6-6 6" /></svg>
          </button>
        </form>
      </div>
    </section>
  );
}

/** How many questions wait in this chat without being on screen in full; jumps to the oldest. */
export function OpenQuestions({ ids }: { ids: string[] }) {
  if (ids.length === 0) return null;
  return (
    <button type="button" className="themis-open-questions" onClick={() => document.getElementById(`themis-question-${ids[0]}`)?.scrollIntoView({ behavior: "smooth", block: "center" })}>
      <QuestionMark />
      {ids.length === 1 ? "1 open question" : `${ids.length} open questions`}
    </button>
  );
}
