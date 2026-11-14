// The workers in a channel's header (board TeamChannel): small overlapping avatars, each with its
// state dot. Hover or focus shows its name, role and state (the same words as its label); a click
// opens that worker's thread with the chief beside the channel; the open one is outlined, and
// pressing it again closes the thread.
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
    <div className="themis-team-stack" role="group" aria-label="Team">
      {workers.map(worker => {
        const status = workerStatus(state, channel, worker);
        const pressed = open === worker.id;
        const label = [worker.name, worker.role, status.label].filter(Boolean).join(", ");
        return (
          <button key={worker.id} type="button" className="themis-team-worker" aria-pressed={pressed} aria-label={label}
            onClick={() => dispatch({ type: "visions/panel", panel: pressed ? null : { kind: "worker", visionId: worker.id } })}>
            <CharacterAvatar look={lookOf(worker)} size={24} />
            <span className="themis-team-worker-dot" style={{ background: status.dot }} aria-hidden="true" />
            <span className="themis-team-tip" aria-hidden="true">{[worker.name, worker.role, status.label].filter(Boolean).join(" · ")}</span>
          </button>
        );
      })}
    </div>
  );
}
