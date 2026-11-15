// A worker's thread under the chief's message that tasked it (board ChannelPlain): "Rook's thread",
// its state (Working, Needs you, Done, Idle; Paused or Stuck when so) with its dot, and a chevron.
// It opens the worker's thread beside the channel. No cloud: the words say who.
import type { ChannelInfo, VisionInfo } from "../../lib/visions";
import { useApp } from "../../state/store";
import { workerStatus } from "./team";

const CHEVRON = <svg className="themis-thread-indicator-more" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="m9 6 6 6-6 6" /></svg>;

export function ThreadIndicator({ channel, worker }: { channel: ChannelInfo; worker: VisionInfo }) {
  const { state, dispatch } = useApp();
  const status = workerStatus(state, channel, worker);
  return <button type="button" className="themis-thread-indicator" aria-label={`${worker.name}'s thread, ${status.label}`}
    onClick={() => dispatch({ type: "visions/panel", panel: { kind: "worker", visionId: worker.id } })}>
    <b>{worker.name}'s thread</b>
    <span className="themis-thread-indicator-state"><span className="themis-state-dot" style={{ background: status.dot }} aria-hidden="true" />{status.label}</span>
    {CHEVRON}
  </button>;
}

/** One indicator per worker the message tasked that is still on this channel's team. */
export function ThreadIndicators({ channel, tasked }: { channel: ChannelInfo; tasked: string[] | undefined }) {
  const { state } = useApp();
  const workers = [...new Set(tasked ?? [])]
    .filter(id => id !== channel.chief && channel.members.includes(id))
    .flatMap(id => state.visions.list.find(vision => vision.id === id) ?? [] as VisionInfo[]);
  if (workers.length === 0) return null;
  return <div className="themis-thread-indicators">{workers.map(worker => <ThreadIndicator key={worker.id} channel={channel} worker={worker} />)}</div>;
}
