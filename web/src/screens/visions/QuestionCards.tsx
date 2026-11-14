// A chat's question in its channel: one its Vision answered (with why, and a way to reply about it),
// or one it passed on, which the person answers right here.
import { useEffect, useState, type FormEvent } from "react";
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

/** The passed-on question as the chat holds it (its choices and recommendation), read while open. */
function useQuestion(threadId: string, questionId: string, open: boolean): Question | undefined {
  const [question, setQuestion] = useState<Question>();
  useEffect(() => {
    if (!open) return;
    let live = true;
    listQuestions(threadId).then(all => { if (live) setQuestion(all.find(each => each.id === questionId)); }).catch(() => undefined);
    return () => { live = false; };
  }, [threadId, questionId, open]);
  return question;
}

export function PassedOnCard({ entry, vision, title }: { entry: Extract<ChannelEntry, { type: "passed_on" }>; vision: string; title: string }) {
  const { state, dispatch } = useApp();
  const [sent, setSent] = useState<string | null>(null);
  const [own, setOwn] = useState("");
  const [busy, setBusy] = useState(false);
  const open = sent === null && (state.visions.questions[entry.thread_id] ?? []).includes(entry.question_id);
  const question = useQuestion(entry.thread_id, entry.question_id, open);
  async function answer(text: string) {
    if (!text.trim() || busy) return;
    setBusy(true);
    try { await answerQuestion(entry.thread_id, entry.question_id, text.trim()); setSent(text.trim()); }
    catch (error) { toast(dispatch, `Could not send the answer: ${describeError(error)}`, "danger"); }
    finally { setBusy(false); }
  }
  const submit = (event: FormEvent) => { event.preventDefault(); void answer(own); };
  return (
    <div className="themis-question-card" role="group" aria-label={`${vision}'s chat ${title} asks you`}>
      <p className="themis-question-from">Its chat <b>{title}</b> {open ? "asks you" : "asked you"}</p>
      <p className="themis-question-text">{entry.question}</p>
      {open ? <>
        {question?.choices?.length ? <div className="themis-question-choices">{question.choices.map(choice => (
          <button key={choice} type="button" className={choice === question.assumption ? "is-recommended" : undefined} disabled={busy} onClick={() => void answer(choice)}>
            {choice === question.assumption ? `${choice} (recommended)` : choice}
          </button>
        ))}</div> : null}
        <form onSubmit={submit}><input className="themis-question-own" aria-label="Answer in your own words" placeholder="Or answer in your own words" value={own} disabled={busy} onChange={event => setOwn(event.target.value)} /></form>
      </> : <p className="themis-question-why">{(sent ?? question?.answer) ? `You answered: ${sent ?? question?.answer}` : "Answered"}</p>}
    </div>
  );
}
