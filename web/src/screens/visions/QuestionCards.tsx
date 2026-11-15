// A chat's question in its channel: one its Vision answered (with why, and a way to reply about it),
// or one it passed on, which the person answers right here and which then folds to one line.
import { useEffect, useState, type FormEvent, type ReactNode } from "react";
import { answerQuestion } from "../../lib/tauri";
import type { Question } from "../../lib/types";
import { listQuestions, type ChannelEntry } from "../../lib/visions";
import { describeError, toast, useApp } from "../../state/store";

/** The kinds of decision a Vision weighs, by autonomy row id. */
const DECISIONS: Record<string, string> = {
  wording: "Wording", tests: "Tests", undoable: "An undoable step", retry: "A retry", scope: "Scope",
  delete: "Deleting", dependency: "A dependency", external: "Leaves this computer", spend: "Spending",
};

export function AnsweredCard({ entry, vision, title, onReply }: { entry: Extract<ChannelEntry, { type: "answered" }>; vision: string; title: string; onReply: () => void }) {
  return (
    <div className="themis-question-card" role="group" aria-label={`${vision}'s chat ${title} asked`}>
      <p className="themis-question-from">Its chat <b>{title}</b> asked</p>
      <dl className="themis-question-qa"><dt>Q</dt><dd>{entry.question}</dd><dt>A</dt><dd>{entry.answer}</dd></dl>
      <div className="themis-question-foot">
        <span className="themis-question-why">{DECISIONS[entry.decision] ?? entry.decision}: {entry.reason}</span>
        <button type="button" className="themis-visions-quiet-button" onClick={onReply}>Reply about this</button>
      </div>
    </div>
  );
}

/** The passed-on question as the chat holds it (its choices and recommendation). */
function useQuestion(threadId: string, questionId: string): Question | undefined {
  const [question, setQuestion] = useState<Question>();
  useEffect(() => {
    let live = true;
    listQuestions(threadId).then(all => { if (live) setQuestion(all.find(each => each.id === questionId)); }).catch(() => undefined);
    return () => { live = false; };
  }, [threadId, questionId]);
  return question;
}

/** What an Auto block's question asks about: the action its wording names
 *  (crates/core/builtins/prompts/auto-blocked-question.md), else the whole question. */
function shortAction(question: string): string {
  return /^Auto mode blocked an action: (.+?)\. Why: /s.exec(question)?.[1] ?? question;
}

/** An answered question as one line: what the answer did, and to what. */
export function foldedWords(question: string, answer: string): string {
  const what = shortAction(question);
  if (answer === "Allow once") return `Allowed once: ${what}`;
  if (answer === "Skip it") return `Skipped: ${what}`;
  return `Answered “${answer}”: ${what}`;
}

/** A question passed on to the person: a card while its chat holds it open, one quiet line once
 *  answered, and nothing once its chat dropped it unanswered (or before its chat was read). */
export function PassedOnLine({ entry, vision, title, className, time }: { entry: Extract<ChannelEntry, { type: "passed_on" }>; vision: string; title: string; className: string; time: ReactNode }) {
  const { state } = useApp();
  const [sent, setSent] = useState<string | null>(null);
  const answer = sent ?? state.visions.answers[entry.thread_id]?.[entry.question_id];
  if (answer !== undefined) {
    return <li className="themis-team-note"><div className="themis-team-note-line" role="note"><span className="themis-question-folded">{foldedWords(entry.question, answer)}</span>{time}</div></li>;
  }
  if (!(state.visions.questions[entry.thread_id] ?? []).includes(entry.question_id)) return null;
  return <li className={className}><div className="themis-channel-body"><PassedOnCard entry={entry} vision={vision} title={title} onSent={setSent} /></div></li>;
}

function PassedOnCard({ entry, vision, title, onSent }: { entry: Extract<ChannelEntry, { type: "passed_on" }>; vision: string; title: string; onSent: (answer: string) => void }) {
  const { dispatch } = useApp();
  const [own, setOwn] = useState("");
  const [busy, setBusy] = useState(false);
  const question = useQuestion(entry.thread_id, entry.question_id);
  async function answer(text: string) {
    if (!text.trim() || busy) return;
    setBusy(true);
    try {
      await answerQuestion(entry.thread_id, entry.question_id, text.trim());
      onSent(text.trim());
    } catch (error) {
      toast(dispatch, `Could not send the answer: ${describeError(error)}`, "danger");
      setBusy(false);
    }
  }
  const submit = (event: FormEvent) => { event.preventDefault(); void answer(own); };
  return (
    <div className="themis-question-card" role="group" aria-label={`${vision}'s chat ${title} asks you`}>
      <p className="themis-question-from">Its chat <b>{title}</b> asks you</p>
      <p className="themis-question-text">{entry.question}</p>
      {question?.choices?.length ? <div className="themis-question-choices">{question.choices.map(choice => (
        <button key={choice} type="button" className={choice === question.assumption ? "is-recommended" : undefined} disabled={busy} onClick={() => void answer(choice)}>
          {choice === question.assumption ? `${choice} (recommended)` : choice}
        </button>
      ))}</div> : null}
      <form onSubmit={submit}><input className="themis-question-own" aria-label="Answer in your own words" placeholder="Or answer in your own words" value={own} disabled={busy} onChange={event => setOwn(event.target.value)} /></form>
    </div>
  );
}
