// A channel: its log (the newest lines, earlier ones on request), the approvals its Visions'
// chats wait on (only the person's Allow or Deny answers them) and the composer.
import { useLayoutEffect, useRef, useState } from "react";
import { ApprovalBox } from "../../components/approval/ApprovalBox";
import { approveAction } from "../../lib/tauri";
import type { ApprovalDecision, ApprovalRequest } from "../../lib/types";
import { ADMIN_VISION_ID, type ChannelInfo, type ChannelLine, type VisionInfo } from "../../lib/visions";
import { describeError, toast, useApp } from "../../state/store";
import { visionMoods } from "../../state/visions";
import { AskFirstBox } from "../AskFirstBox";
import { ChannelComposer } from "./ChannelComposer";
import { ChannelEntryView, visionById } from "./ChannelEntry";
import { channelChat } from "./ChatLink";
import { CloudIcon } from "./CloudIcon";
import { reducedMotion, useArrivals, useShownAMoment, useWakes } from "./thinking";

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

/** The Visions whose run in this channel is going (also after a post of their own). */
function runningHere(state: ReturnType<typeof useApp>["state"], channel: ChannelInfo): string[] {
  return state.visions.list
    .filter(vision => vision.vision_chats.some(own => own.channel_id === channel.id && (state.running[own.thread_id] ?? own.running)))
    .map(vision => vision.id);
}

/** "<Name> is thinking" with three dots rising and falling; plain text with reduced motion. */
function Thinking({ visions, reduced }: { visions: VisionInfo[]; reduced: boolean }) {
  return <>{visions.map(vision => (
    <li key={vision.id} className="themis-channel-thinking" role="status">
      <span className="themis-channel-avatar"><CloudIcon shape={vision.shape} color={vision.color} mood="think" size={30} live crown={vision.id === ADMIN_VISION_ID} /></span>
      <span className="themis-channel-thinking-text">{vision.name} is thinking{reduced ? "…" : <span className="themis-thinking-dots" aria-hidden="true"><i /><i /><i /></span>}</span>
    </li>
  ))}</>;
}

/** Keeps the log at its newest line while the person is there; reading back stays put. `tail`
 *  changes with anything added at the bottom (a line, an approval, a thinking row or the post that
 *  replaces it); `first` with lines loaded above, which keep what was on screen where it was
 *  (`holdPlace` before the load, `releasePlace` when it failed or added nothing). */
function useStickToEnd(tail: string, first: number | undefined) {
  const log = useRef<HTMLDivElement>(null);
  const atEnd = useRef(true);
  const place = useRef<{ height: number; top: number; first: number | undefined } | null>(null);
  useLayoutEffect(() => {
    const element = log.current;
    if (!element) return;
    const held = place.current;
    if (held && held.first !== first) {
      element.scrollTop = held.top + element.scrollHeight - held.height;
      place.current = null;
      return;
    }
    if (atEnd.current) element.scrollTop = element.scrollHeight;
    // Something arrived below while older lines load: measure again from here.
    if (held) place.current = { height: element.scrollHeight, top: element.scrollTop, first };
  }, [tail, first]);
  const onScroll = () => {
    const element = log.current;
    if (element) atEnd.current = element.scrollHeight - element.scrollTop - element.clientHeight < 48;
  };
  const holdPlace = () => {
    const element = log.current;
    if (element) place.current = { height: element.scrollHeight, top: element.scrollTop, first };
  };
  const releasePlace = () => { place.current = null; };
  return { log, onScroll, holdPlace, releasePlace };
}

/** The newest line already there when the channel's lines were first read (none in an empty
 *  channel): later lines fade in. Null until the lines are read. */
function useShownUpTo(lines: ChannelLine[] | undefined): number | null {
  const now = lines === undefined ? null : lines[lines.length - 1]?.seq ?? -1;
  const [shownUpTo, setShownUpTo] = useState<number | null>(now);
  if (shownUpTo === null && now !== null) setShownUpTo(now);
  return shownUpTo ?? now;
}

/** Who is thinking here (running, or woken by the person's post), each row kept a moment, and
 *  the lines shown so far as new ones arrive one after another. */
function useChannelMotion(state: ReturnType<typeof useApp>["state"], channel: ChannelInfo, read: ChannelLine[] | undefined) {
  const lines = read ?? [];
  const reduced = reducedMotion();
  const running = channel.id ? runningHere(state, channel) : [];
  const { waking, woke } = useWakes(lines, running);
  const thinkingIds = useShownAMoment([...new Set([...running, ...waking])], reduced);
  const thinking = thinkingIds.flatMap(id => visionById(state, id) ?? []);
  const shownUpTo = useShownUpTo(read);
  const arrived = useArrivals(lines, shownUpTo, reduced);
  return { reduced, thinking, woke, shownUpTo, shown: lines.filter(line => line.seq <= arrived) };
}

/** The channel's name; once saved, it opens and closes the channel's details. */
function ChannelTitle({ channel }: { channel: ChannelInfo }) {
  const { state, dispatch } = useApp();
  if (!channel.id) return <h1># {channel.name}</h1>;
  const open = state.visions.open.panel?.kind === "channel";
  return <h1><button type="button" className="themis-channel-title" aria-expanded={open} title="Channel details" onClick={() => dispatch({ type: "visions/panel", panel: open ? null : { kind: "channel" } })}># {channel.name}</button></h1>;
}

export function ChannelView({ channel }: { channel: ChannelInfo }) {
  const { state, loadOlderLines } = useApp();
  const [draft, setDraft] = useState("");
  const [loading, setLoading] = useState(false);
  const read = state.visions.lines[channel.id];
  const lines = read ?? [];
  const chats = new Set((state.visions.chats[channel.id] ?? []).map(chat => chat.id));
  const approvals = state.approvals.filter(request => chats.has(request.thread_id));
  const members = channel.members.flatMap(id => visionById(state, id) ?? []);
  const moods = visionMoods(state);
  const { reduced, thinking, woke, shownUpTo, shown } = useChannelMotion(state, channel, read);
  const tail = `${shown[shown.length - 1]?.seq}:${approvals.length}:${thinking.map(vision => vision.id).join(",")}`;
  const { log, onScroll, holdPlace, releasePlace } = useStickToEnd(tail, lines[0]?.seq);
  async function older() {
    setLoading(true);
    holdPlace();
    let added = false;
    // On success the held place is used (and dropped) when the added lines render.
    try { added = await loadOlderLines(channel.id); } finally { setLoading(false); if (!added) releasePlace(); }
  }
  return (
    <section className="themis-channel" aria-label={`# ${channel.name}`}>
      <header className="themis-channel-head">
        <ChannelTitle channel={channel} />
        {channel.topic && <span className="themis-channel-topic">{channel.topic}</span>}
        {members.length > 0 && <span className="themis-channel-members" role="img" aria-label={`Here: ${members.map(vision => vision.name).join(", ")}`}>
          {members.map(vision => <CloudIcon key={vision.id} shape={vision.shape} color={vision.color} mood={moods[vision.id] ?? "calm"} size={20} crown={vision.id === ADMIN_VISION_ID} />)}
        </span>}
      </header>
      <div className="themis-channel-log" ref={log} onScroll={onScroll}>
        {state.visions.nextBefore[channel.id] != null && <button type="button" className="themis-show-more" disabled={loading} onClick={() => void older()}>{loading ? "Loading…" : "Show earlier messages"}</button>}
        {lines.length === 0 && thinking.length === 0 && <p className="themis-channel-empty">No messages yet.</p>}
        <ol className="themis-channel-lines">
          {shown.map(line => <ChannelEntryView key={line.seq} line={line} channelId={channel.id} fresh={!reduced && shownUpTo !== null && line.seq > shownUpTo} onReply={setDraft} />)}
          {approvals.map(request => <ChannelApproval key={request.approval_id} request={request} channelId={channel.id} />)}
          <Thinking visions={thinking} reduced={reduced} />
        </ol>
      </div>
      <ChannelComposer channel={channel} members={members} draft={draft} setDraft={setDraft} onPosted={woke} />
    </section>
  );
}
