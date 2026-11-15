// The admin's proposal in # administration: what it would set up, in its words and item by item, and whether it
// waits for the person's answer or was set up (with links to the channels it made or changed). A
// setup whose proposal is not loaded shows on its own. A team or a hire (board AdminChannel) is a
// card of its own: the channel, the chief and workers with their roles, then "Created" once set up.
import type { ChannelEntry, Proposal, ProposalChange, SetUpItem } from "../../lib/visions";
import { useApp } from "../../state/store";
import { LEVELS } from "./autonomy";
import { CharacterAvatar } from "./CharacterAvatar";
import { lookOf } from "./cast";
import type { ProposedMember, TeamProposal } from "./team";

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
          <b><CharacterAvatar look={lookOf(vision)} size={16} />New Vision {vision.name}</b>
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

function Member({ member }: { member: ProposedMember }) {
  return <li className="themis-team-card-member">
    <CharacterAvatar look={lookOf({ color: member.color ?? "", accessory: member.accessory })} size={22} />
    <b>{member.name}</b>{member.role && <span>{member.role}</span>}
  </li>;
}

/** The channel as a pill; once it exists, the pill opens it. */
function ChannelPill({ name, channelId, onOpenChannel }: { name: string; channelId?: string; onOpenChannel: (channelId: string) => void }) {
  return channelId
    ? <button type="button" className="themis-mention" aria-label={`Open # ${name}`} onClick={() => onOpenChannel(channelId)}># {name}</button>
    : <span className="themis-mention"># {name}</span>;
}

const Created = () => <span className="themis-team-card-done"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M5 12l5 5L20 7" /></svg>Created</span>;

/** A team or a hire, as a card under the admin's words. */
function TeamCard({ proposal, applied, onOpenChannel }: { proposal: Proposal & TeamProposal; applied?: Applied; onOpenChannel: (channelId: string) => void }) {
  const { state } = useApp();
  const made = applied?.items.find(item => item.kind === "channel");
  if (proposal.team) {
    const { team } = proposal;
    const name = team.channel.name;
    return <>
      <p className="themis-channel-text">{proposal.summary}</p>
      <div className="themis-team-card" role="group" aria-label={`New channel # ${name}`}>
        <h3><ChannelPill name={name} channelId={made?.id} onOpenChannel={onOpenChannel} />New channel{applied && <Created />}</h3>
        <ul>{[{ ...team.chief, role: "Chief of staff" }, ...team.workers].map(member => <Member key={member.name} member={member} />)}</ul>
        {(team.autonomy || team.daily_budget_usd !== undefined) && <div className="themis-team-card-facts">
          {team.autonomy && <span>Autonomy <b>{LEVELS[team.autonomy]}</b></span>}
          {team.daily_budget_usd !== undefined && <span>Budget <b>${team.daily_budget_usd} a day</b></span>}
        </div>}
      </div>
    </>;
  }
  const hire = proposal.hire!;
  // A hire names its channel by name, in this project.
  const channel = (state.visions.channels[state.activeProjectRoot ?? ""] ?? []).find(each => each.name === hire.channel);
  const name = channel?.name ?? "";
  return <>
    <p className="themis-channel-text">{proposal.summary}</p>
    <div className="themis-team-card" role="group" aria-label={name ? `New worker for # ${name}` : "New worker"}>
      <h3>{name && <ChannelPill name={name} channelId={channel?.id} onOpenChannel={onOpenChannel} />}New worker{applied && <Created />}</h3>
      <ul><Member member={hire.worker} /></ul>
    </div>
  </>;
}

export function ProposalCard({ entry, applied, onOpenChannel }: { entry?: Proposed; applied?: Applied; onOpenChannel: (channelId: string) => void }) {
  const proposal = entry?.proposal as (Proposal & TeamProposal) | undefined;
  if (proposal?.team || proposal?.hire) return <TeamCard proposal={proposal} applied={applied} onOpenChannel={onOpenChannel} />;
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
