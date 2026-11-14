// A worker's thread with its chief (board TeamChannel), beside the channel and read-only: the
// tasks and redirects the chief gave, the worker's reports and pushback, and links to the
// worker's chats in this channel. There is no box to write in; the person talks to the chief.
import type { ChannelInfo, VisionInfo } from "../../lib/visions";
import { useApp } from "../../state/store";
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
  const who = entry.kind === "report" || entry.kind === "pushback" ? worker : chief;
  return (
    <li className="themis-team-thread-entry">
      <CharacterAvatar look={lookOf(who)} size={22} />
      <div className="themis-channel-body">
        <div className="themis-channel-who">{who.name}<time className="themis-channel-time">{clock(line.at_ms)}</time></div>
        <p className="themis-channel-text">{entry.text}</p>
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
  return (
    <aside className="themis-visions-panel themis-team-thread" aria-label={`${worker.name} with ${chief.name}`}>
      <div className="themis-visions-panel-head">
        <CharacterAvatar look={lookOf(worker)} size={24} />
        <span className="themis-visions-panel-title">{worker.name} <span className="themis-team-with">with {chief.name}</span></span>
        <PanelButton label="Close" onClick={() => dispatch({ type: "visions/panel", panel: null })}><CloseIcon /></PanelButton>
      </div>
      {log && log.length > 0 && <ol className="themis-team-thread-log">{log.map(line => <Line key={line.seq} line={line} worker={worker} chief={chief} />)}</ol>}
      {chats.length > 0 && <div className="themis-team-thread-chats">
        {chats.map(chat => <ChatLink key={chat.id} channelId={channel.id} threadId={chat.id} title={chat.title} visionName={worker.name} />)}
      </div>}
    </aside>
  );
}
