import { approveAction, enqueueMessage, steerQueuedMessage } from "../lib/tauri";
import type { ApprovalDecision, ApprovalRequest, Question } from "../lib/types";
import { QuestionBox } from "./QuestionBox";

export const RUN_ONCE = "Run it once";
export const SKIP_IT = "Skip it";

/** The question a destructive step in Auto or YOLO is put as: what it is about to do, and what
 *  each answer does. The step waits for the answer (or the approval timeout). */
export function askFirstQuestion(request: ApprovalRequest): Question {
  const what = request.command ? `\`${request.command.argv.join(" ")}\`` : request.summary;
  const who = request.subagent ? `${request.subagent.name} (subagent) is` : "The agent is";
  return {
    id: request.approval_id,
    run_id: "",
    question: `${who} about to run ${what}. It is destructive and hard to undo. Run it?`,
    choices: [RUN_ONCE, SKIP_IT],
    reasons: {
      [RUN_ONCE]: "Runs exactly this step, once. It may not be possible to undo.",
      [SKIP_IT]: "Not run. The agent is told you skipped it and carries on with the rest.",
    },
    blocking: true,
    asked_at: 0,
  };
}

/** What an answer means for the waiting step: a choice decides it; anything else is a message
 *  for the agent, steered in (which releases the step unrun so the agent reads it first). */
export function askFirstDecision(answer: string): ApprovalDecision | null {
  return answer === RUN_ONCE ? "once" : answer === SKIP_IT ? "deny" : null;
}

/** A destructive step Auto or YOLO asks about first, shown as a question box. */
export function AskFirstBox({ request, onDecided, onError, onFold }: { request: ApprovalRequest; onDecided: () => void; onError: (error: unknown) => void; onFold?: () => void }) {
  async function answer(text: string) {
    const decision = askFirstDecision(text);
    try {
      if (decision) {
        await approveAction(request.thread_id, request.approval_id, decision);
      } else {
        const queued = await enqueueMessage(request.thread_id, text);
        await steerQueuedMessage(request.thread_id, queued.id);
      }
    } catch (error) {
      onError(error);
      throw error;
    }
    onDecided();
  }
  return <QuestionBox question={askFirstQuestion(request)} onAnswer={answer} onFold={onFold} />;
}
