// The admin's proposal in # hq: what it would set up, in its words and item by item, and whether it
// waits for the person's answer or was set up (with links to the channels it made or changed). A
// setup whose proposal is not loaded shows on its own.
import type { ChannelEntry, Proposal, ProposalChange, SetUpItem } from "../../lib/visions";
import { LEVELS } from "./autonomy";
import { CloudIcon } from "./CloudIcon";

type Proposed = Extract<ChannelEntry, { type: "proposal" }>;
type Applied = Extract<ChannelEntry, { type: "proposal_applied" }>;

const list = (names: string[]) => names.join(", ");

/** What a new Vision is set up with, besides its job: its model, level and budget when named. */
function newVisionWords(vision: Proposal["visions"][number]): string {
  return [
    vision.role,
    vision.model ? `runs on ${vision.model}` : "",
    vision.autonomy ? LEVELS[vision.autonomy] : "",
    vision.daily_budget_usd !== undefined ? `$${vision.daily_budget_usd} a day` : "",
  ].filter(Boolean).join(" · ");
}

/** A change in plain words: what it is about, and what changes. */
function changeWords(change: ProposalChange): [string, string] {
  const parts: string[] = [];
  if (change.vision !== undefined) {
    if (change.name) parts.push(`Renamed to ${change.name}`);
    if (change.role) parts.push(`New job: ${change.role}`);
    if (change.paused !== undefined) parts.push(change.paused ? "Paused" : "Resumed");
    if (change.autonomy) parts.push("Decides less on its own");
    if (change.daily_budget_usd !== undefined) parts.push(`$${change.daily_budget_usd} a day`);
    return [change.vision, parts.join(" · ")];
  }
  if (change.name) parts.push(`Renamed to # ${change.name}`);
  if (change.topic) parts.push(`Topic: ${change.topic}`);
  if (change.add_members?.length) parts.push(`Adds ${list(change.add_members)}`);
  if (change.remove_members?.length) parts.push(`Removes ${list(change.remove_members)}`);
  return [`# ${change.channel ?? ""}`, parts.join(" · ")];
}

function Items({ proposal }: { proposal: Proposal }) {
  return (
    <ul className="themis-proposal-items">
      {proposal.channels.map(channel => (
        <li key={`c:${channel.name}`}>
          <b>New channel # {channel.name}</b>
          <span>{[channel.topic, channel.members.length ? `with ${list(channel.members)}` : ""].filter(Boolean).join(" · ")}</span>
        </li>
      ))}
      {proposal.visions.map(vision => (
        <li key={`v:${vision.name}`}>
          <b><CloudIcon shape={vision.shape} color={vision.color} mood="calm" size={16} />New Vision {vision.name}</b>
          <span>{newVisionWords(vision)}</span>
        </li>
      ))}
      {proposal.changes.map((change, index) => {
        const [what, how] = changeWords(change);
        return <li key={`x:${index}`}><b>{what}</b><span>{how}</span></li>;
      })}
    </ul>
  );
}

/** Made things first, then changed ones; channels before Visions in each. */
function ordered(items: SetUpItem[]): SetUpItem[] {
  const rank = (item: SetUpItem) => (item.action === "created" ? 0 : 2) + (item.kind === "channel" ? 0 : 1);
  return [...items].sort((a, b) => rank(a) - rank(b));
}

/** What was set up, as chips side by side: a channel's opens it. */
function SetUp({ items, onOpenChannel }: { items: SetUpItem[]; onOpenChannel: (channelId: string) => void }) {
  return (
    <div className="themis-question-why themis-proposal-done" role="status">
      <span>Set up</span>
      <ul>{ordered(items).map(item => <li key={`${item.kind}:${item.id}`}>
        {item.kind === "channel"
          ? <button type="button" className="themis-chat-chip" aria-label={`Open # ${item.name}`} onClick={() => onOpenChannel(item.id)}># {item.name}</button>
          : <span className="themis-chat-chip">{item.name}</span>}
      </li>)}</ul>
    </div>
  );
}

export function ProposalCard({ entry, applied, onOpenChannel }: { entry?: Proposed; applied?: Applied; onOpenChannel: (channelId: string) => void }) {
  return (
    <div className="themis-question-card themis-proposal" role="group" aria-label="Proposal">
      {entry && <>
        <p className="themis-question-text">{entry.proposal.summary}</p>
        <Items proposal={entry.proposal} />
      </>}
      {applied
        ? <SetUp items={applied.items} onOpenChannel={onOpenChannel} />
        : <p className="themis-question-why" role="status">Waiting for your answer</p>}
    </div>
  );
}
