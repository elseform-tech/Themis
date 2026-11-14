// The worker chips under a channel's name (board TeamChannel): one per worker with its cloud,
// name, role and status dot. A chip opens that worker's thread with the chief beside the channel;
// the open one is outlined, and pressing it again closes the thread.
import type { ChannelInfo, VisionInfo } from "../../lib/visions";
import { useApp } from "../../state/store";
import { CharacterAvatar } from "./CharacterAvatar";
import { lookOf } from "./cast";
import { workerStatus } from "./team";

export function WorkerChips({ channel, workers }: { channel: ChannelInfo; workers: VisionInfo[] }) {
  const { state, dispatch } = useApp();
  if (workers.length === 0) return null;
  const panel = state.visions.open.panel;
  const open = panel?.kind === "worker" ? panel.visionId : null;
  return (
    <div className="themis-team-chips" role="group" aria-label="Team">
      {workers.map(worker => {
        const status = workerStatus(state, channel, worker);
        const pressed = open === worker.id;
        return (
          <button key={worker.id} type="button" className="themis-team-chip" aria-pressed={pressed}
            aria-label={[worker.name, worker.role, status.label].filter(Boolean).join(", ")}
            onClick={() => dispatch({ type: "visions/panel", panel: pressed ? null : { kind: "worker", visionId: worker.id } })}>
            <CharacterAvatar look={lookOf(worker)} size={22} />
            <b>{worker.name}</b>
            {worker.role && <span className="themis-team-chip-role">{worker.role}</span>}
            <span className="themis-state-dot" style={{ background: status.dot }} aria-hidden="true" />
          </button>
        );
      })}
    </div>
  );
}
