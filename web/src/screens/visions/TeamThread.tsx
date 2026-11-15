// A worker's thread with its chief (boards TeamChannel, ChannelPlain), beside the channel and
// read-only: the header names both ("Rook with Juno", each with its cloud); the chief's tasks and
// redirects sit on the right and the worker's reports and pushback on the left, each with a small
// "name · time" above; then the approvals the worker's chats wait on (the person answers them
// here), and links to the worker's chats in this channel at the foot. There is no box to write
// in; the person talks to the chief. A running work chat's link keeps its Stop: stopping a run is
// one of the writes the Workspace rules allow the person (as in the Tasks tab).
import type { ChannelInfo, VisionInfo } from "../../lib/visions";
import { useApp } from "../../state/store";
import { ChannelApproval } from "./ChannelApproval";
import { CharacterAvatar } from "./CharacterAvatar";
import { ChatLink } from "./ChatLink";
import { lookOf } from "./cast";
import { CloseIcon, PanelButton } from "./SidePanel";
import { teamOf, type TeamLine } from "./team";
import "./VisionsPanels.css";

const clock = (atMs: number) => new Date(atMs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });

function Line({ line, worker, chief }: { line: TeamLine; worker: VisionInfo; chief: VisionInfo }) {
  const { entry } = line;
  if (entry.kind === "paused" || entry.kind === "resumed") {
    return <li className="themis-team-thread-note">{chief.name} {entry.kind} {worker.name}<time className="themis-channel-time">{clock(line.at_ms)}</time></li>;
  }
  const fromWorker = entry.kind === "report" || entry.kind === "pushback";
  const who = fromWorker ? worker : chief;
  return (
    <li className={`themis-team-thread-entry ${fromWorker ? "is-theirs" : "is-mine"}`}>
      <span className="themis-team-thread-who">{who.name} · <time>{clock(line.at_ms)}</time></span>
      <div className={`themis-bubble ${fromWorker ? "themis-bubble-chief" : "themis-bubble-me"}`}>
        <p className="themis-channel-text">{entry.text}</p>
        {entry.kind === "pushback" && entry.safer && <p className="themis-channel-text">Instead: {entry.safer}</p>}
      </div>
    </li>
  );
}

/** `log` is the thread oldest first; null while the server gives the person no team log yet. */
export function TeamThread({ channel, workerId, log }: { channel: ChannelInfo; workerId: string; log: TeamLine[] | null }) {
  const { state, dispatch } = useApp();
  const { chief } = teamOf(state, channel);
  const worker = state.visions.list.find(vision => vision.id === workerId);
  if (!worker || !chief) return null;
  const chats = (state.visions.chats[channel.id] ?? []).filter(chat => chat.owner.vision_id === worker.id && chat.owner.kind === "work");
  const ids = new Set(chats.map(chat => chat.id));
  const approvals = state.approvals.filter(request => ids.has(request.thread_id));
  return (
    <aside className="themis-visions-panel themis-team-thread" aria-label={`${worker.name} with ${chief.name}`}>
      <div className="themis-visions-panel-head">
        <CharacterAvatar look={lookOf(worker)} size={22} accessory={false} />
        <span className="themis-visions-panel-title themis-team-thread-title">{worker.name} <span className="themis-team-with">with</span> <CharacterAvatar look={lookOf(chief)} size={16} accessory={false} /> <span className="themis-team-with">{chief.name}</span></span>
        <PanelButton label="Close" onClick={() => dispatch({ type: "visions/panel", panel: null })}><CloseIcon /></PanelButton>
      </div>
      {log && log.length > 0 && <ol className="themis-team-thread-log">{log.map(line => <Line key={line.seq} line={line} worker={worker} chief={chief} />)}</ol>}
      {approvals.length > 0 && <ul className="themis-team-thread-approvals">{approvals.map(request => <ChannelApproval key={request.approval_id} request={request} channelId={channel.id} />)}</ul>}
      {chats.length > 0 && <div className="themis-team-thread-chats">
        {chats.map(chat => <ChatLink key={chat.id} channelId={channel.id} threadId={chat.id} title={chat.title} visionName={worker.name} />)}
      </div>}
    </aside>
  );
}
