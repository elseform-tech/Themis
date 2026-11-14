// A channel's header (boards TeamChannel, AdminChannel): one slim row with the channel's name in
// the display font and gradient (it opens the channel's details) and one chip per worker. Once the
// channel has a conversation, the chief sits at the top right as a small badge (its look, name and
// state) that opens its profile; in a new channel the chief stands large in the middle of the log
// instead (ChiefWelcome). `# administration` shows Atlas, its admin, with no chips.
import type { ChannelInfo, VisionInfo } from "../../lib/visions";
import { ADMIN_VISION_ID } from "../../lib/visions";
import { useApp } from "../../state/store";
import { CharacterAvatar } from "./CharacterAvatar";
import { lookOf } from "./cast";
import { EXPRESSION_LABEL, type Expression } from "./Expression";
import { ChiefScene } from "./three/ChiefScene";
import { chiefExpression, statusOf, teamOf } from "./team";
import { WorkerChips } from "./WorkerChips";

function ChannelName({ channel }: { channel: ChannelInfo }) {
  const { state, dispatch } = useApp();
  const name = <><span className="themis-team-hash">#</span><span className="themis-team-gradient">{channel.name}</span></>;
  if (!channel.id) return <h1 className="themis-team-name">{name}</h1>;
  const open = state.visions.open.panel?.kind === "channel";
  return <h1 className="themis-team-name"><button type="button" className="themis-channel-title" aria-expanded={open} onClick={() => dispatch({ type: "visions/panel", panel: open ? null : { kind: "channel" } })}>{name}</button></h1>;
}

/** The chief's job, state and the way to its profile, as the header badge and the welcome share them. */
function useChief(channel: ChannelInfo, chief: VisionInfo) {
  const { state, dispatch } = useApp();
  const job = chief.id === ADMIN_VISION_ID ? "Admin" : "Chief of staff";
  const expression: Expression = chiefExpression(state, channel, chief);
  const label = `${chief.name}, ${job.toLowerCase()}, ${EXPRESSION_LABEL[expression]}`;
  const profile = () => dispatch({ type: "visions/panel", panel: { kind: "vision", visionId: chief.id } });
  return { job, expression, label, profile, dot: statusOf(expression).dot };
}

function ChiefBadge({ channel, chief }: { channel: ChannelInfo; chief: VisionInfo }) {
  const { label, profile, dot } = useChief(channel, chief);
  return <button type="button" className="themis-chief-badge" aria-label={label} onClick={profile}>
    <CharacterAvatar look={lookOf(chief)} size={28} live />
    <b>{chief.name}</b>
    <span className="themis-state-dot" style={{ background: dot }} aria-hidden="true" />
    <svg className="themis-chief-badge-more" viewBox="0 0 24 24" width="14" height="14" aria-hidden="true"><path d="m9 6 6 6-6 6" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" /></svg>
  </button>;
}

/** A new channel's chief, large in the middle of the log, with its name, job and state. */
export function ChiefWelcome({ channel }: { channel: ChannelInfo }) {
  const { state } = useApp();
  const { chief } = teamOf(state, channel);
  if (!chief) return null;
  return <ChiefWelcomeOf channel={channel} chief={chief} />;
}

function ChiefWelcomeOf({ channel, chief }: { channel: ChannelInfo; chief: VisionInfo }) {
  const { job, expression, label, profile, dot } = useChief(channel, chief);
  return <div className="themis-chief-welcome">
    <ChiefScene look={lookOf(chief)} expression={expression} name={chief.name} size={184} />
    <button type="button" className="themis-chief-welcome-name" aria-label={label} onClick={profile}><b>{chief.name}</b><span>{job}</span></button>
    <span className="themis-chief-welcome-state"><span className="themis-state-dot" style={{ background: dot }} aria-hidden="true" />{EXPRESSION_LABEL[expression]}</span>
  </div>;
}

/** `fresh`: the channel has no conversation yet (the chief shows in the log, not here). */
export function ChannelHeader({ channel, fresh }: { channel: ChannelInfo; fresh: boolean }) {
  const { state } = useApp();
  const { chief, workers } = teamOf(state, channel);
  const admin = chief?.id === ADMIN_VISION_ID;
  return (
    <header className="themis-team-head">
      <ChannelName channel={channel} />
      {chief && !admin && <WorkerChips channel={channel} workers={workers} />}
      {chief && !fresh && <ChiefBadge channel={channel} chief={chief} />}
    </header>
  );
}
