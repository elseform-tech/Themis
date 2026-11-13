// One line of a channel's log: the person's message, a Vision's message, a chat it started, a
// question it answered or passed on, or a notice. Avatars never move.
import { Fragment, type ReactNode } from "react";
import { ADMIN_VISION_ID, type ChannelEntry as Entry, type ChannelLine, type VisionInfo } from "../../lib/visions";
import { useApp } from "../../state/store";
import { ChatChip, ChatLink, channelChat } from "./ChatLink";
import { CloudIcon } from "./CloudIcon";
import { ProposalCard } from "./ProposalCard";
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

export type NoticeChat = "work" | "own" | "none";

/** What a notice says about the Vision, in plain words. A work chat it names follows the sentence
 *  as a chip (`chip`), never inside it; `own` is the Vision's own reply run. */
export function noticeText(entry: Extract<Entry, { type: "notice" }>, vision: string, chat: NoticeChat): { text: string; chip: boolean } {
  const work = chat === "work";
  const say = (text: string, chip = false) => ({ text: chip ? text : `${text}.`, chip });
  switch (entry.kind) {
    case "stopped": return chat === "own" ? say(`You stopped ${vision}'s reply`) : say(`You stopped a chat ${vision} started`, work);
    case "paused": return say(`You paused ${vision}`);
    case "resumed": return say(`${vision} is back`);
    case "budget_reached": return say(`${vision} reached today's budget`, work);
    case "chat_failed": return chat === "own" ? say(`${vision} couldn't finish its reply`) : say(`A chat ${vision} started failed`, work);
    case "handed_back": return work ? say(`You handed this chat back to ${vision}`, true) : say(`You handed a chat back to ${vision}`);
    case "sandbox_unavailable": return say(`${vision} couldn't run: no sandbox is available`);
    case "budget_unknown": return say(`${vision} stopped: today's spend is unknown`);
    case "unpriced": return say(`${entry.unpriced_requests ?? "Some"} of today's requests had no price`);
  }
}

/** Whether a notice's chat is one of the channel's work chats, the Vision's own chat, or neither. */
function noticeChat(state: AppState, channelId: string, entry: Extract<Entry, { type: "notice" }>): NoticeChat {
  const id = entry.thread_id;
  if (!id) return "none";
  if (channelChat(state, channelId, id)) return "work";
  return state.visions.list.some(vision => vision.vision_chats.some(own => own.thread_id === id)) ? "own" : "none";
}

function Notice({ line, channelId }: { line: ChannelLine; channelId: string }) {
  const { state } = useApp();
  const entry = line.entry as Extract<Entry, { type: "notice" }>;
  const vision = visionById(state, entry.vision_id)?.name ?? "A Vision";
  const kind = noticeChat(state, channelId, entry);
  const { text, chip } = noticeText(entry, vision, kind);
  return (
    <li className="themis-channel-notice" role="note">
      <span className="themis-channel-notice-text">{text}{chip && entry.thread_id && <ChatChip channelId={channelId} threadId={entry.thread_id} />}</span>
      <time className="themis-channel-time">{clock(line.at_ms)}</time>
    </li>
  );
}

function Avatar({ vision }: { vision: VisionInfo | undefined }) {
  if (!vision) return <span className="themis-channel-avatar themis-channel-person" aria-hidden="true">?</span>;
  return <span className="themis-channel-avatar"><CloudIcon shape={vision.shape} color={vision.color} mood="calm" size={30} crown={vision.id === ADMIN_VISION_ID} /></span>;
}

/** The loaded line of the proposal or setup `proposalId` of type `type`, if any. */
function proposalLine<T extends "proposal" | "proposal_applied">(state: AppState, channelId: string, type: T, proposalId: string): Extract<Entry, { type: T }> | undefined {
  const line = (state.visions.lines[channelId] ?? []).find(each => each.entry.type === type && (each.entry as { proposal_id: string }).proposal_id === proposalId);
  return line?.entry as Extract<Entry, { type: T }> | undefined;
}

function Body({ line, channelId, onReply }: { line: ChannelLine; channelId: string; onReply: (text: string) => void }): ReactNode {
  const { state, dispatch } = useApp();
  const { entry } = line;
  const names = state.visions.list.map(vision => vision.name);
  if (entry.type === "person" || entry.type === "vision") return <p className="themis-channel-text">{withMentions(entry.text, names)}</p>;
  if (entry.type === "notice") return null;
  if (entry.type === "proposal" || entry.type === "proposal_applied") {
    const open = (id: string) => dispatch({ type: "visions/open", channelId: id });
    return entry.type === "proposal"
      ? <ProposalCard entry={entry} applied={proposalLine(state, channelId, "proposal_applied", entry.proposal_id)} onOpenChannel={open} />
      : <ProposalCard applied={entry} onOpenChannel={open} />;
  }
  const vision = visionById(state, entry.vision_id)?.name ?? "A Vision";
  const title = entry.type === "chat_started" ? entry.title : channelChat(state, channelId, entry.thread_id)?.title ?? "a chat";
  if (entry.type === "chat_started") return <><p className="themis-channel-text">Started a chat.</p><ChatLink channelId={channelId} threadId={entry.thread_id} title={title} visionName={vision} /></>;
  if (entry.type === "answered") return <AnsweredCard entry={entry} vision={vision} title={title} onReply={() => onReply(`@${vision} about “${entry.question}”: `)} />;
  return <PassedOnCard entry={entry} vision={vision} title={title} />;
}

export function ChannelEntryView({ line, channelId, fresh, onReply }: { line: ChannelLine; channelId: string; fresh: boolean; onReply: (text: string) => void }) {
  const { state } = useApp();
  const { entry } = line;
  if (entry.type === "notice") return <Notice line={line} channelId={channelId} />;
  // A setup shows on its proposal's card while that is loaded.
  if (entry.type === "proposal_applied" && proposalLine(state, channelId, "proposal", entry.proposal_id)) return null;
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
