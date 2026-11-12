// One line of a channel's log: the person's message, a Vision's message, a chat it started, a
// question it answered or passed on, or a notice. Avatars never move.
import { Fragment, type ReactNode } from "react";
import type { ChannelEntry as Entry, ChannelLine, VisionInfo } from "../../lib/visions";
import { useApp } from "../../state/store";
import { ChatLink, channelChat } from "./ChatLink";
import { CloudIcon } from "./CloudIcon";
import { AnsweredCard, PassedOnCard } from "./QuestionCards";

type AppState = ReturnType<typeof useApp>["state"];

export function visionById(state: AppState, id: string | undefined): VisionInfo | undefined {
  return state.visions.list.find(vision => vision.id === id);
}

/** `@Name` of a known Vision shows as a mention. */
function withMentions(text: string, names: string[]): ReactNode {
  const known = new Set(names.map(name => name.toLowerCase()));
  return text.split(/(@[\p{L}\p{N}_-]+)/u).map((part, index) =>
    part.startsWith("@") && known.has(part.slice(1).toLowerCase())
      ? <span key={index} className="themis-mention">{part}</span>
      : <Fragment key={index}>{part}</Fragment>);
}

function clock(atMs: number): string {
  return new Date(atMs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

/** What a notice says, about the Vision and chat it names. */
export function noticeText(entry: Extract<Entry, { type: "notice" }>, vision: string, title: string | undefined): string {
  const chat = title ? ` ${title}` : "";
  switch (entry.kind) {
    case "stopped": return `You stopped ${vision}'s chat${chat}.`;
    case "paused": return `You paused ${vision}.`;
    case "resumed": return `${vision} is back.`;
    case "budget_reached": return `${vision} reached today's budget and stopped.`;
    case "chat_failed": return `${vision}'s chat${chat} failed.`;
    case "handed_back": return `You handed${chat || " a chat"} back to ${vision}.`;
    case "sandbox_unavailable": return `${vision} could not run: no sandbox available.`;
    case "budget_unknown": return `${vision} stopped: today's spend is unknown.`;
    case "unpriced": return `${entry.unpriced_requests ?? "Some"} of today's requests had no price.`;
  }
}

function Avatar({ vision }: { vision: VisionInfo | undefined }) {
  if (!vision) return <span className="themis-channel-avatar themis-channel-person" aria-hidden="true">?</span>;
  return <span className="themis-channel-avatar"><CloudIcon shape={vision.shape} color={vision.color} mood="calm" size={30} /></span>;
}

function Body({ line, channelId, onReply }: { line: ChannelLine; channelId: string; onReply: (text: string) => void }): ReactNode {
  const { state } = useApp();
  const { entry } = line;
  const names = state.visions.list.map(vision => vision.name);
  if (entry.type === "person" || entry.type === "vision") return <p className="themis-channel-text">{withMentions(entry.text, names)}</p>;
  if (entry.type === "notice") return null;
  const vision = visionById(state, entry.vision_id)?.name ?? "A Vision";
  const title = entry.type === "chat_started" ? entry.title : channelChat(state, channelId, entry.thread_id)?.title ?? "a chat";
  if (entry.type === "chat_started") return <><p className="themis-channel-text">Started a chat.</p><ChatLink channelId={channelId} threadId={entry.thread_id} title={title} visionName={vision} /></>;
  if (entry.type === "answered") return <AnsweredCard entry={entry} vision={vision} title={title} onChange={() => onReply(`@${vision} about “${entry.question}”: `)} />;
  return <PassedOnCard entry={entry} vision={vision} title={title} />;
}

export function ChannelEntryView({ line, channelId, fresh, onReply }: { line: ChannelLine; channelId: string; fresh: boolean; onReply: (text: string) => void }) {
  const { state } = useApp();
  const { entry } = line;
  if (entry.type === "notice") {
    const vision = visionById(state, entry.vision_id)?.name ?? "A Vision";
    const chat = entry.thread_id ? channelChat(state, channelId, entry.thread_id)?.title : undefined;
    return <li className="themis-channel-notice" role="note">{noticeText(entry, vision, chat)}<time className="themis-channel-time">{clock(line.at_ms)}</time></li>;
  }
  const vision = entry.type === "person" ? undefined : visionById(state, entry.vision_id);
  const who = entry.type === "person" ? "You" : vision?.name ?? "A Vision";
  return (
    <li className={`themis-channel-entry${fresh && entry.type !== "person" ? " is-fresh" : ""}`}>
      {entry.type === "person" ? <span className="themis-channel-avatar themis-channel-person" aria-hidden="true">Y</span> : <Avatar vision={vision} />}
      <div className="themis-channel-body">
        <div className="themis-channel-who">{who}<time className="themis-channel-time">{clock(line.at_ms)}</time></div>
        <Body line={line} channelId={channelId} onReply={onReply} />
      </div>
    </li>
  );
}
