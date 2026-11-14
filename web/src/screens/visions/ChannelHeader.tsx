// A channel's header (boards TeamChannel, AdminChannel): the chief in 3D with its glow and ground
// shadow, the channel's name large in the display font and gradient (it opens the channel's
// details), the chief's name and job with its state, and one chip per worker. The chief's picture
// and name open its profile. `# administration` shows Atlas, its admin, with no chips.
import type { ChannelInfo } from "../../lib/visions";
import { ADMIN_VISION_ID } from "../../lib/visions";
import { useApp } from "../../state/store";
import { lookOf } from "./cast";
import { EXPRESSION_LABEL } from "./Expression";
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

export function ChannelHeader({ channel }: { channel: ChannelInfo }) {
  const { state, dispatch } = useApp();
  const { chief, workers } = teamOf(state, channel);
  if (!chief) return <header className="themis-team-head"><div className="themis-team-text"><ChannelName channel={channel} /></div></header>;
  const admin = chief.id === ADMIN_VISION_ID;
  const job = admin ? "Admin" : "Chief of staff";
  const expression = chiefExpression(state, channel, chief);
  const status = statusOf(expression);
  const profile = () => dispatch({ type: "visions/panel", panel: { kind: "vision", visionId: chief.id } });
  return (
    <header className="themis-team-head">
      <div className="themis-team-picture" onClick={profile}>
        <ChiefScene look={lookOf(chief)} expression={expression} name={chief.name} size={admin ? 120 : 130} />
      </div>
      <div className="themis-team-text">
        <ChannelName channel={channel} />
        <div className="themis-team-chief">
          <button type="button" className="themis-team-chief-name" aria-label={`${chief.name}, ${job.toLowerCase()}`} onClick={profile}><b>{chief.name}</b><span>{job}</span></button>
          <span className="themis-state-dot" style={{ background: status.dot }} aria-hidden="true" />
          <span className="themis-team-state">{EXPRESSION_LABEL[expression]}</span>
        </div>
        {!admin && <WorkerChips channel={channel} workers={workers} />}
      </div>
    </header>
  );
}
