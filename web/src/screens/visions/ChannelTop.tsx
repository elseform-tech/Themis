// The top of a channel (board ChannelPlain), in place of a header: the channel's name small at the
// top left (it opens the channel's details), the chief floating at the top middle with its name
// and state beside it (it opens the chief's profile) and a band that fades the log out under them.
// A new channel shows the chief large in the middle of the log instead (ChiefWelcome).
// `# administration` floats Atlas, its admin.
import type { ChannelInfo, VisionInfo } from "../../lib/visions";
import { ADMIN_VISION_ID } from "../../lib/visions";
import { useApp } from "../../state/store";
import { visionMoods } from "../../state/visions";
import { CharacterAvatar } from "./CharacterAvatar";
import { lookOf } from "./cast";
import { EXPRESSION_LABEL, type Expression } from "./Expression";
import { ChiefScene } from "./three/ChiefScene";
import { chiefExpression, statusOf, teamOf } from "./team";

function ChannelName({ channel }: { channel: ChannelInfo }) {
  const { state, dispatch } = useApp();
  const name = <><span className="themis-channel-name-hash" aria-hidden="true">#</span> {channel.name}</>;
  if (!channel.id) return <h1 className="themis-channel-name">{name}</h1>;
  const open = state.visions.open.panel?.kind === "channel";
  return <h1 className="themis-channel-name">
    <button type="button" aria-label={`# ${channel.name}: channel details`} aria-expanded={open} onClick={() => dispatch({ type: "visions/panel", panel: open ? null : { kind: "channel" } })}>{name}</button>
  </h1>;
}

/** The chief's job, state and the way to its profile, as the floating chief and the welcome share them. */
function useChief(channel: ChannelInfo, chief: VisionInfo) {
  const { state, dispatch } = useApp();
  const job = chief.id === ADMIN_VISION_ID ? "Admin" : "Chief of staff";
  const expression: Expression = chiefExpression(state, channel, chief);
  const label = `${chief.name}, ${job.toLowerCase()}, ${EXPRESSION_LABEL[expression]}`;
  const profile = () => dispatch({ type: "visions/panel", panel: { kind: "vision", visionId: chief.id } });
  return { job, expression, label, profile, dot: statusOf(expression).dot, mood: visionMoods(state)[chief.id] ?? "calm" };
}

function FloatingChief({ channel, chief }: { channel: ChannelInfo; chief: VisionInfo }) {
  const { expression, label, profile, dot, mood } = useChief(channel, chief);
  return <button type="button" className="themis-floating-chief" aria-label={label} onClick={profile}>
    <CharacterAvatar look={lookOf(chief)} size={40} mood={mood} live />
    <span className="themis-floating-chief-name"><b>{chief.name}</b><span className="themis-state-dot" style={{ background: dot }} aria-hidden="true" />{EXPRESSION_LABEL[expression]}</span>
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

/** `fresh`: the channel has no conversation yet (the chief stands in the log, not up here). */
export function ChannelTop({ channel, fresh }: { channel: ChannelInfo; fresh: boolean }) {
  const { state } = useApp();
  const { chief } = teamOf(state, channel);
  return <>
    <div className="themis-channel-fade" aria-hidden="true" />
    <ChannelName channel={channel} />
    {chief && !fresh && <FloatingChief channel={channel} chief={chief} />}
  </>;
}
