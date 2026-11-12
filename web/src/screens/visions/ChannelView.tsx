// A channel: its log (the newest lines, earlier ones on request), the approvals its Visions'
// chats wait on (only the person's Allow or Deny answers them) and the composer.
import { useLayoutEffect, useRef, useState } from "react";
import { ApprovalBox } from "../../components/approval/ApprovalBox";
import { approveAction } from "../../lib/tauri";
import type { ApprovalDecision, ApprovalRequest } from "../../lib/types";
import type { ChannelInfo, ChannelLine, VisionInfo } from "../../lib/visions";
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

/** Visions whose run in this channel is going and that have not posted since the newest line:
 *  a run that answered the newest message has spoken. */
function thinkingHere(state: ReturnType<typeof useApp>["state"], channel: ChannelInfo): VisionInfo[] {
  const lines = state.visions.lines[channel.id] ?? [];
  const last = lines[lines.length - 1]?.entry;
  const spoke = last && last.type !== "person" && last.type !== "notice" ? last.vision_id : undefined;
  return state.visions.list.filter(vision => vision.id !== spoke
    && vision.vision_chats.some(own => own.channel_id === channel.id && (state.running[own.thread_id] ?? own.running)));
}

function Thinking({ visions }: { visions: VisionInfo[] }) {
  return <>{visions.map(vision => (
    <li key={vision.id} className="themis-channel-thinking" role="status">
      <span className="themis-channel-avatar"><CloudIcon shape={vision.shape} color={vision.color} mood="think" size={30} live /></span>
      <span className="themis-shim">{vision.name} is thinking…</span>
    </li>
  ))}</>;
}

/** Keeps the log at its newest line while the person is there; reading back stays put, and lines
 *  loaded above (`holdPlace` first) keep what was on screen where it was. */
function useStickToEnd(count: number) {
  const log = useRef<HTMLDivElement>(null);
  const atEnd = useRef(true);
  const place = useRef<{ height: number; top: number } | null>(null);
  useLayoutEffect(() => {
    const element = log.current;
    if (!element) return;
    if (place.current) element.scrollTop = place.current.top + element.scrollHeight - place.current.height;
    else if (atEnd.current) element.scrollTop = element.scrollHeight;
    place.current = null;
  }, [count]);
  const onScroll = () => {
    const element = log.current;
    if (element) atEnd.current = element.scrollHeight - element.scrollTop - element.clientHeight < 48;
  };
  const holdPlace = () => {
    const element = log.current;
    if (element) place.current = { height: element.scrollHeight, top: element.scrollTop };
  };
  return { log, onScroll, holdPlace };
}

/** The newest line already there when the channel was first shown with its lines: later ones fade in. */
function useShownUpTo(lines: ChannelLine[]): number | null {
  const last = lines[lines.length - 1]?.seq;
  const [shownUpTo, setShownUpTo] = useState<number | null>(last ?? null);
  if (shownUpTo === null && last !== undefined) setShownUpTo(last);
  return shownUpTo ?? last ?? null;
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
  const thinking = channel.id ? thinkingHere(state, channel) : [];
  const { log, onScroll, holdPlace } = useStickToEnd(lines.length + approvals.length + thinking.length);
  const shownUpTo = useShownUpTo(lines);
  async function older() {
    setLoading(true);
    holdPlace();
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
        {lines.length === 0 && thinking.length === 0 && <p className="themis-channel-empty">No messages yet.</p>}
        <ol className="themis-channel-lines">
          {lines.map(line => <ChannelEntryView key={line.seq} line={line} channelId={channel.id} fresh={shownUpTo !== null && line.seq > shownUpTo} onReply={setDraft} />)}
          {approvals.map(request => <ChannelApproval key={request.approval_id} request={request} channelId={channel.id} />)}
          <Thinking visions={thinking} />
        </ol>
      </div>
      <ChannelComposer channel={channel} members={members} draft={draft} setDraft={setDraft} />
    </section>
  );
}
