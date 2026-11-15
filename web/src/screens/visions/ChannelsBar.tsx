// The channels bar (boards ChannelPlain, ChannelAutonomy): in Visions mode it takes the chat list's
// place under the project switcher. Channels: `# administration` pinned first, then the project's
// channels, each `#` and its name with what waits on the person there. Under them, Team: the open
// channel's chief (its profile) and, indented, its workers (each opens its thread with the chief),
// each with its small cloud, role and state. `# administration` has no Team: Atlas floats in it.
import { useId, useState } from "react";
import { ADMIN_VISION_ID, ADMINISTRATION, type ChannelInfo, type VisionInfo } from "../../lib/visions";
import { useApp } from "../../state/store";
import { channelNeeds } from "../../state/visions";
import { CharacterAvatar } from "./CharacterAvatar";
import { lookOf } from "./cast";
import { projectChannels } from "./channels";
import { NewChannelSheet } from "./NewChannelSheet";
import { chiefExpression, statusOf, teamOf, workerStatus, type Status } from "./team";

function ChannelRow({ channel, active, onOpen }: { channel: ChannelInfo; active: boolean; onOpen: (id: string | null) => void }) {
  const needs = channelNeeds(channel);
  const label = needs > 0 ? `# ${channel.name}, ${needs} ${needs === 1 ? "needs" : "need"} you` : `# ${channel.name}`;
  return (
    <button type="button" className={`themis-sidebar-row themis-channel-row ${active ? "themis-sidebar-row--active" : ""} ${needs > 0 ? "is-waiting" : ""}`} aria-label={label} aria-current={active ? "page" : undefined} onClick={() => onOpen(channel.id || null)}>
      <span className="themis-channel-hash" aria-hidden="true">#</span>
      <span className="themis-sidebar-name">{channel.name}</span>
      {needs > 0 && <span className="themis-thread-tag" aria-hidden="true">{needs}</span>}
    </button>
  );
}

/** One member of the team: its cloud (no accessory), name, role and state dot. */
function MemberRow({ vision, role, status, size, worker, open, onClick }: { vision: VisionInfo; role: string; status: Status; size: number; worker: boolean; open: boolean; onClick: () => void }) {
  return (
    <button type="button" className={`themis-sidebar-row themis-team-row${worker ? " is-worker" : ""}${open ? " themis-sidebar-row--active" : ""}`}
      aria-label={[vision.name, role, status.label].filter(Boolean).join(", ")} aria-current={open ? "true" : undefined} onClick={onClick}>
      <CharacterAvatar look={lookOf(vision)} size={size} accessory={false} />
      <span className="themis-sidebar-name"><span className="themis-team-row-name">{vision.name}</span> <span className="themis-team-row-role">{role}</span></span>
      <span className="themis-state-dot" style={{ background: status.dot }} aria-hidden="true" />
    </button>
  );
}

function TeamSection({ channel }: { channel: ChannelInfo }) {
  const { state, dispatch } = useApp();
  const heading = useId();
  const { chief, workers } = teamOf(state, channel);
  if (!chief || chief.id === ADMIN_VISION_ID) return null;
  const panel = state.visions.open.panel;
  const opened = (kind: "vision" | "worker", id: string) => panel?.kind === kind && panel.visionId === id;
  const toggle = (kind: "vision" | "worker", id: string) => dispatch({ type: "visions/panel", panel: opened(kind, id) ? null : { kind, visionId: id } });
  return <>
    <h2 className="themis-sidebar-section" id={heading}>Team</h2>
    <ul className="themis-sidebar-list" aria-labelledby={heading}>
      <li><MemberRow vision={chief} role={chief.role || "Chief of staff"} status={statusOf(chiefExpression(state, channel, chief))} size={22} worker={false} open={opened("vision", chief.id)} onClick={() => toggle("vision", chief.id)} /></li>
      {workers.map(worker => <li key={worker.id}>
        <MemberRow vision={worker} role={worker.role} status={workerStatus(state, channel, worker)} size={18} worker open={opened("worker", worker.id)} onClick={() => toggle("worker", worker.id)} />
      </li>)}
    </ul>
  </>;
}

/** What the bar shows under the project switcher (Sidebar keeps the switcher and the frame). */
export function ChannelsBar() {
  const { state, dispatch } = useApp();
  const [making, setMaking] = useState(false);
  const { hq, channels, current } = projectChannels(state);
  const openChannel = (channelId: string | null) => dispatch({ type: "visions/open", channelId });
  return <>
    <div className="themis-sidebar-projects themis-channels-bar">
      <h2 className="themis-sidebar-section">Channels
        <button type="button" className="themis-visions-add" aria-label="New channel" onClick={() => setMaking(true)}>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14" /></svg>
        </button>
      </h2>
      <ul className="themis-sidebar-list">
        {[hq, ...channels].map(channel => <li key={channel.id || ADMINISTRATION}><ChannelRow channel={channel} active={current === channel} onOpen={openChannel} /></li>)}
      </ul>
      {current?.id && <TeamSection channel={current} />}
    </div>
    {making && <NewChannelSheet onClose={() => setMaking(false)} />}
  </>;
}
