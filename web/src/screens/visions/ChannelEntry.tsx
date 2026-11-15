// One line of a channel's log (board ChannelPlain): the person's message as a bubble on the right
// with its time, the chief's as a bubble on the left with one indicator per worker it tasked there,
// a chat it started, a question it answered or passed on, or a notice. No avatars or names: the
// side says who.
import { Fragment, useState, type ReactNode } from "react";
import { answerChannelRequest, type ChannelEntry as Entry, type ChannelInfo, type ChannelLine, type VisionInfo } from "../../lib/visions";
import { ResponseBody } from "../../components/ResponseBody";
import { describeError, toast, useApp } from "../../state/store";
import { ChatChip, ChatLink, channelChat } from "./ChatLink";
import { ProposalCard } from "./ProposalCard";
import { AnsweredCard, PassedOnCard } from "./QuestionCards";
import { isTeamNoteEntry, scheduleWords, TEAM_NOTE_TYPES, teamNote, type TeamNoteEntry } from "./team";
import { ThreadIndicators } from "./ThreadIndicator";

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
    case "teams_arrived": return say("Each channel now has a chief of staff: its first Vision. The others work for the chief");
    case "team_paused": return say(`${vision}'s team paused after many turns in a row. Write here to carry on`);
    case "request_cap": return say(`${vision} reached today's limit of requests to other channels`);
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

const ARROW = <svg className="themis-team-note-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M7 7h11v11M18 7 6 19" /></svg>;
const CLOCK = <svg className="themis-team-note-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="9" /><path d="M12 7v5h4" /></svg>;

/** A request between channels or a new routine, as one quiet line (board TeamChannel). The other
 *  channel's pill opens it. A line missing what it shows is skipped. */
/** "Take it on" and "Decline" on a request this channel received, while nobody answered it. */
function RequestAnswer({ requestId }: { requestId: string }) {
  const { state, dispatch } = useApp();
  const [busy, setBusy] = useState(false);
  const answered = Object.values(state.visions.lines).some(lines => lines.some(line => line.entry.type === "request_answered" && line.entry.request_id === requestId));
  if (answered) return null;
  const answer = (accept: boolean) => {
    setBusy(true);
    answerChannelRequest(requestId, accept)
      .catch(reason => toast(dispatch, `Could not answer the request: ${describeError(reason)}`, "danger"))
      .finally(() => setBusy(false));
  };
  return <span className="themis-team-note-actions">
    <button type="button" className="themis-mention" disabled={busy} onClick={() => answer(true)}>Take it on</button>
    <button type="button" className="themis-mention" disabled={busy} onClick={() => answer(false)}>Decline</button>
  </span>;
}

/** A request line's words around the other channel's pill. */
function requestWords(entry: Exclude<TeamNoteEntry, { type: "routine_set" }>, pill: ReactNode): ReactNode {
  switch (entry.type) {
    case "request_sent": return <><span>Asked</span>{pill}<span>to {entry.ask}.</span></>;
    case "request_received": return <>{pill}<span>asked us to {entry.ask}.</span><RequestAnswer requestId={entry.request_id} /></>;
    case "worker_asked": return <>{pill}<span>asks for a worker: {entry.role}. {entry.why}</span></>;
    case "request_answered": return <><span>Request with</span>{pill}<span>{entry.accepted ? "taken on" : "declined"}{entry.note ? `: ${entry.note}` : "."}</span></>;
  }
}

function TeamNote({ line }: { line: ChannelLine }) {
  const { state, dispatch } = useApp();
  const entry = teamNote(line.entry);
  if (!entry) return null;
  let body: ReactNode;
  if (entry.type === "routine_set") {
    body = <>{CLOCK}<span>{entry.changed ? "Changed a routine" : "Added a routine"}: <b>{scheduleWords(entry.schedule)}</b>, {entry.instruction}</span></>;
  } else {
    const name = Object.values(state.visions.channels).flat().find(channel => channel.id === entry.channel_id)?.name ?? "another channel";
    const pill = <button type="button" className="themis-mention" onClick={() => dispatch({ type: "visions/open", channelId: entry.channel_id })}># {name}</button>;
    body = <>{ARROW}{requestWords(entry, pill)}</>;
  }
  return <li className="themis-team-note"><div className="themis-team-note-line" role="note">{body}<time className="themis-channel-time">{clock(line.at_ms)}</time></div></li>;
}

/** The loaded line of the proposal or setup `proposalId` of type `type`, if any. */
function proposalLine<T extends "proposal" | "proposal_applied">(state: AppState, channelId: string, type: T, proposalId: string): Extract<Entry, { type: T }> | undefined {
  const line = (state.visions.lines[channelId] ?? []).find(each => each.entry.type === type && (each.entry as { proposal_id: string }).proposal_id === proposalId);
  return line?.entry as Extract<Entry, { type: T }> | undefined;
}

/** A card or chat link from a Vision (a proposal, a question, a chat it started). */
function Body({ line, channelId, onReply }: { line: ChannelLine; channelId: string; onReply: (text: string) => void }): ReactNode {
  const { state, dispatch } = useApp();
  const { entry } = line;
  if (entry.type === "person" || entry.type === "vision" || entry.type === "notice" || isTeamNoteEntry(entry)) return null;
  if (entry.type === "proposal" || entry.type === "proposal_applied") {
    const open = (id: string) => dispatch({ type: "visions/open", channelId: id });
    return entry.type === "proposal"
      ? <ProposalCard entry={entry} applied={proposalLine(state, channelId, "proposal_applied", entry.proposal_id)} onOpenChannel={open} />
      : <ProposalCard applied={entry} onOpenChannel={open} />;
  }
  const chat = channelChat(state, channelId, entry.thread_id);
  // A question passed on still names whose chat asks.
  const asker = entry.type === "passed_on" ? chat?.owner.vision_id ?? entry.vision_id : entry.vision_id;
  const vision = visionById(state, asker)?.name ?? "A Vision";
  const title = entry.type === "chat_started" ? entry.title : chat?.title ?? "a chat";
  if (entry.type === "chat_started") return <><p className="themis-channel-text">Started a chat.</p><ChatLink channelId={channelId} threadId={entry.thread_id} title={title} visionName={vision} /></>;
  if (entry.type === "answered") return <AnsweredCard entry={entry} vision={vision} title={title} onReply={() => onReply(`About “${entry.question}”: `)} />;
  return <PassedOnCard entry={entry} vision={vision} title={title} />;
}

export function ChannelEntryView({ line, channel, fresh, onReply }: { line: ChannelLine; channel: ChannelInfo; fresh: boolean; onReply: (text: string) => void }) {
  const { state } = useApp();
  const { entry } = line;
  const channelId = channel.id;
  if (TEAM_NOTE_TYPES.has(entry.type)) return <TeamNote line={line} />;
  if (entry.type === "notice") return <Notice line={line} channelId={channelId} />;
  // A setup shows on its proposal's card while that is loaded.
  if (entry.type === "proposal_applied" && proposalLine(state, channelId, "proposal", entry.proposal_id)) return null;
  const names = state.visions.list.map(vision => vision.name);
  if (entry.type === "person") {
    return <li className="themis-channel-entry is-mine">
      <p className="themis-bubble themis-bubble-me">{withMentions(entry.text, names)}</p>
      <time className="themis-channel-time">{clock(line.at_ms)}</time>
    </li>;
  }
  const className = `themis-channel-entry is-theirs${fresh ? " is-fresh" : ""}`;
  // A Vision writes Markdown like any chat reply, and it shows the same way.
  if (entry.type === "vision") {
    return <li className={className}>
      <div className="themis-bubble themis-bubble-chief themis-channel-markdown"><ResponseBody text={entry.text} /></div>
      <ThreadIndicators channel={channel} tasked={entry.tasked} />
    </li>;
  }
  return <li className={className}><div className="themis-channel-body"><Body line={line} channelId={channelId} onReply={onReply} /></div></li>;
}
