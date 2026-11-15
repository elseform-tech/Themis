// An approval one of a channel's chats waits on, with whose chat asks: the chief's own chats' in
// the channel, a worker's in that worker's thread (board ChannelPlain). Only the person's Allow
// or Deny answers it.
import { ApprovalBox } from "../../components/approval/ApprovalBox";
import { approveAction } from "../../lib/tauri";
import type { ApprovalDecision, ApprovalRequest } from "../../lib/types";
import { describeError, toast, useApp } from "../../state/store";
import { AskFirstBox } from "../AskFirstBox";
import { visionById } from "./ChannelEntry";
import { channelChat } from "./ChatLink";

/** An approval one of the channel's chats waits on, with whose chat asks. */
export function ChannelApproval({ request, channelId }: { request: ApprovalRequest; channelId: string }) {
  const { state, dispatch } = useApp();
  const chat = channelChat(state, channelId, request.thread_id);
  const vision = visionById(state, chat?.owner.vision_id)?.name ?? "A Vision";
  const done = () => dispatch({ type: "approval/dequeued", approvalId: request.approval_id });
  async function decide(decision: ApprovalDecision) {
    try { await approveAction(request.thread_id, request.approval_id, decision); done(); }
    catch (error) { toast(dispatch, `Approval failed: ${describeError(error)}`, "danger"); }
  }
  return (
    <li className="themis-channel-approval">
      <p className="themis-question-from">{vision}'s chat <b>{chat?.title}</b> asks</p>
      {request.ask_first
        ? <AskFirstBox request={request} onDecided={done} onError={error => toast(dispatch, `Could not answer: ${describeError(error)}`, "danger")} />
        : <ApprovalBox request={request} open onDecide={decision => void decide(decision)} />}
    </li>
  );
}
