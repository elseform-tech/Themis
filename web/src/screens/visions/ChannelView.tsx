// A channel: its log (the newest lines, earlier ones on request), the approvals its Visions'
// chats wait on (only the person's Allow or Deny answers them) and the composer.
import { useLayoutEffect, useRef, useState } from "react";
import { ApprovalBox } from "../../components/approval/ApprovalBox";
import { approveAction } from "../../lib/tauri";
import type { ApprovalDecision, ApprovalRequest } from "../../lib/types";
import type { ChannelInfo } from "../../lib/visions";
import { describeError, toast, useApp } from "../../state/store";
import { visionMoods } from "../../state/visions";
import { AskFirstBox } from "../AskFirstBox";
import { ChannelComposer } from "./ChannelComposer";
import { ChannelEntryView, visionById } from "./ChannelEntry";
import { channelChat } from "./ChatLink";
import { CloudIcon } from "./CloudIcon";

/** An approval one of the channel's chats waits on, with whose chat asks. */
function ChannelApproval({ request, channelId }: { request: ApprovalRequest; channelId: string }) {
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

/** Keeps the log at its newest line while the person is there; reading back stays put. */
function useStickToEnd(count: number) {
  const log = useRef<HTMLDivElement>(null);
  const atEnd = useRef(true);
  useLayoutEffect(() => {
    const element = log.current;
    if (element && atEnd.current) element.scrollTop = element.scrollHeight;
  }, [count]);
  const onScroll = () => {
    const element = log.current;
    if (element) atEnd.current = element.scrollHeight - element.scrollTop - element.clientHeight < 48;
  };
  return { log, onScroll };
}

export function ChannelView({ channel }: { channel: ChannelInfo }) {
  const { state, loadOlderLines } = useApp();
  const [draft, setDraft] = useState("");
  const [loading, setLoading] = useState(false);
  const lines = state.visions.lines[channel.id] ?? [];
  const chats = new Set((state.visions.chats[channel.id] ?? []).map(chat => chat.id));
  const approvals = state.approvals.filter(request => chats.has(request.thread_id));
  const members = channel.members.flatMap(id => visionById(state, id) ?? []);
  const moods = visionMoods(state);
  const { log, onScroll } = useStickToEnd(lines.length + approvals.length);
  async function older() {
    setLoading(true);
    try { await loadOlderLines(channel.id); } finally { setLoading(false); }
  }
  return (
    <section className="themis-channel" aria-label={`# ${channel.name}`}>
      <header className="themis-channel-head">
        <h1># {channel.name}</h1>
        {channel.topic && <span className="themis-channel-topic">{channel.topic}</span>}
        {members.length > 0 && <span className="themis-channel-members" role="img" aria-label={`Here: ${members.map(vision => vision.name).join(", ")}`}>
          {members.map(vision => <CloudIcon key={vision.id} shape={vision.shape} color={vision.color} mood={moods[vision.id] ?? "calm"} size={20} />)}
        </span>}
      </header>
      <div className="themis-channel-log" ref={log} onScroll={onScroll}>
        {state.visions.nextBefore[channel.id] != null && <button type="button" className="themis-show-more" disabled={loading} onClick={() => void older()}>{loading ? "Loading…" : "Show earlier messages"}</button>}
        {lines.length === 0 && <p className="themis-channel-empty">{members.length === 0 ? "No Visions here yet." : `Write to ${members.length === 1 ? members[0]!.name : "a Vision with @"} to start.`}</p>}
        <ol className="themis-channel-lines">
          {lines.map(line => <ChannelEntryView key={line.seq} line={line} channelId={channel.id} onReply={setDraft} />)}
          {approvals.map(request => <ChannelApproval key={request.approval_id} request={request} channelId={channel.id} />)}
        </ol>
      </div>
      <ChannelComposer channel={channel} members={members} draft={draft} setDraft={setDraft} />
    </section>
  );
}
