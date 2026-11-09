import { useMemo, useState } from "react";
import { ApprovalBox } from "../components";
import { answerQuestion, answerQuestions, approveAction } from "../lib/tauri";
import type { ApprovalDecision, ApprovalRequest, Goal, Question } from "../lib/types";
import { composerQueue, openFor, placeQuestion, readFolded, usePlacements, writeFolded, type ComposerItem } from "../state/composerSlot";
import type { ChatMessage, ToolTraceEntry } from "../state/reducer";
import { describeError, toast, useApp } from "../state/store";
import { GoalBar } from "./GoalBar";
import { AskFirstBox } from "./AskFirstBox";
import { QuestionMark, useNowAt } from "./QuestionBox";
import { QuestionPages, type PageAnswer } from "./QuestionPages";

interface ComposerStackProps {
  threadId: string;
  goal: Goal | null;
  approvals: ApprovalRequest[];
  questions: Question[];
  traces: ToolTraceEntry[];
  onEditGoal: (text: string) => Promise<void>;
  onPauseGoal: () => void;
  onResumeGoal: () => void;
  onRemoveGoal: () => void;
}

function plural(count: number, one: string, many: string): string {
  return `${count} ${count === 1 ? one : many}`;
}

/** What waits behind the box on the composer, in a few words. */
function waitingText(waiting: { approvals: number; questions: number }): string | undefined {
  const parts = [waiting.approvals ? plural(waiting.approvals, "approval", "approvals") : "", waiting.questions ? plural(waiting.questions, "question", "questions") : ""].filter(Boolean);
  return parts.length ? `${parts.join(" · ")} next` : undefined;
}

function Shield() {
  return <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.9" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M12 3l7 3v5c0 4.5-3 8.5-7 10-4-1.5-7-5.5-7-10V6l7-3z" /></svg>;
}

/** What the composer shows now (see `composerQueue`), following each question's wait. */
export function useComposerItem(approvals: ApprovalRequest[], questions: Question[]) {
  const placements = usePlacements();
  const blockingMs = useApp().state.settings.approval_timeout_seconds * 1000;
  const expiries = questions.filter(question => question.answer === undefined).map(question => question.asked_at + openFor(question, blockingMs)).filter(at => at > Date.now());
  const now = useNowAt(expiries.length ? Math.min(...expiries) : undefined);
  return useMemo(() => composerQueue(approvals, questions, placements, now, blockingMs), [approvals, questions, placements, now, blockingMs]);
}

/** The chat's questions, and whether a question box is on its composer (`locked`): the
 *  composer's own input then waits for its answer (Stop still works). It unlocks when the box
 *  moves into its turn. */
export function useComposerQuestions(threadId: string, messages: ChatMessage[]): { questions: Question[]; locked: boolean } {
  const { state } = useApp();
  const approvals = useMemo(() => state.approvals.filter(approval => approval.thread_id === threadId), [state.approvals, threadId]);
  const questions = useMemo(() => messages.flatMap(message => message.question ? [message.question] : []), [messages]);
  return { questions, locked: useComposerItem(approvals, questions).current?.kind === "question" };
}

/** The boxes on top of the composer (canvas board D): the goal as one row, then one box at
 *  a time, an approval before any question, with what waits behind it as a count. All of
 *  it folds to one row of pills; the fold is remembered. An answered or skipped question
 *  leaves the composer for its turn; a decided approval leaves nothing behind. */
export function ComposerStack({ threadId, goal, approvals, questions, traces, onEditGoal, onPauseGoal, onResumeGoal, onRemoveGoal }: ComposerStackProps) {
  const { dispatch } = useApp();
  const [folded, setFoldedState] = useState(readFolded);
  const { current, waiting } = useComposerItem(approvals, questions);
  const shownGoal = goal ?? null;
  const setFolded = (value: boolean) => { setFoldedState(value); writeFolded(value); };

  async function decide(request: ApprovalRequest, decision: ApprovalDecision) {
    try {
      await approveAction(request.thread_id, request.approval_id, decision);
      dispatch({ type: "approval/dequeued", approvalId: request.approval_id });
    } catch (error: unknown) {
      toast(dispatch, `Approval failed: ${describeError(error)}`, "danger");
    }
  }

  if (!shownGoal && !current) return null;

  if (folded) {
    const questionCount = waiting.questions + (current?.kind === "question" ? current.questions.length : 0);
    const approval = current?.kind === "approval" ? current.request : null;
    const approvalPreview = approval ? approval.command?.argv.join(" ") ?? approval.tool : "";
    return (
      <div className="themis-composer-pills" role="group" aria-label="Goal, approvals and questions, folded">
        {shownGoal && (
          <button type="button" className="themis-composer-pill" aria-label={`Goal: ${shownGoal.text}. Unfold`} onClick={() => setFolded(false)}>
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" aria-hidden="true"><circle cx="12" cy="12" r="8" /><circle cx="12" cy="12" r="4" /></svg>
            <span className="themis-composer-pill-text">{shownGoal.text}</span>
          </button>
        )}
        {approval && (
          <button type="button" className="themis-composer-pill is-approval" aria-label={`Approval waiting: ${approvalPreview}. Unfold`} onClick={() => setFolded(false)}>
            <Shield />
            <code className="themis-composer-pill-text">{approvalPreview}</code>
            {waiting.approvals > 0 && <span>+{waiting.approvals}</span>}
          </button>
        )}
        {questionCount > 0 && (
          <button type="button" className="themis-composer-pill" aria-label={`${plural(questionCount, "question", "questions")} waiting. Unfold`} onClick={() => setFolded(false)}>
            <QuestionMark size={13} />
            <span>{questionCount}</span>
          </button>
        )}
        <button type="button" className="themis-goal-icon themis-icon-tip themis-composer-unfold" data-tooltip="Unfold" aria-label="Unfold" onClick={() => setFolded(false)}>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true"><path d="m6 15 6-6 6 6" /></svg>
        </button>
      </div>
    );
  }

  const fold = () => setFolded(true);
  return (
    <>
      {shownGoal && <GoalBar goal={shownGoal} onEdit={onEditGoal} onPause={onPauseGoal} onResume={onResumeGoal} onRemove={onRemoveGoal} onFold={current ? undefined : fold} />}
      {current && <ComposerSlot current={current} threadId={threadId} traces={traces} next={waitingText(waiting)} onFold={fold} onDecide={decide} />}
    </>
  );
}

/** The one approval or question the composer shows. */
function ComposerSlot({ current, threadId, traces, next, onFold, onDecide }: {
  current: ComposerItem; threadId: string; traces: ToolTraceEntry[]; next: string | undefined;
  onFold: () => void; onDecide: (request: ApprovalRequest, decision: ApprovalDecision) => Promise<void>;
}) {
  const { dispatch } = useApp();
  async function send(answers: PageAnswer[]) {
    try {
      if (answers.length === 1) await answerQuestion(threadId, answers[0]!.questionId, answers[0]!.answer);
      else await answerQuestions(threadId, answers);
    } catch (error) {
      toast(dispatch, `Could not send the answer: ${describeError(error)}`, "danger");
      throw error;
    }
  }
  return (
    <div className="themis-composer-slot">
      {current.kind === "approval" && current.request.ask_first ? (
        // Auto and YOLO ask about a destructive step as a question, never an approval box.
        <AskFirstBox key={current.request.approval_id} request={current.request} onFold={onFold}
          onDecided={() => dispatch({ type: "approval/dequeued", approvalId: current.request.approval_id })}
          onError={error => toast(dispatch, `Could not answer: ${describeError(error)}`, "danger")} />
      ) : current.kind === "approval" ? (
        <ApprovalBox key={current.request.approval_id} request={current.request} open
          // An MCP tool's readable name ("files · read text file"), as the step list shows it.
          label={[...traces].reverse().find(entry => entry.tool === current.request.tool && entry.label)?.label}
          next={next}
          onFold={onFold}
          onDecide={decision => void onDecide(current.request, decision)} />
      ) : (
        <QuestionPages key={current.questions[0]!.group ?? current.questions[0]!.id} pages={current.questions} onSend={send}
          onLeave={ids => ids.forEach(id => placeQuestion(id, "turn"))} onFold={onFold} />
      )}
    </div>
  );
}
