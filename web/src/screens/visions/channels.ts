// The open project's channels as Visions shows them: `# administration` first, then the rest, and which one is
// open. An open channel that is gone stays open (as gone) rather than falling back to another.
import { ADMINISTRATION, type ChannelInfo } from "../../lib/visions";
import type { useApp } from "../../state/store";

type AppState = ReturnType<typeof useApp>["state"];

/** `# administration`: the project's built-in channel, or an empty one until it exists. */
function emptyHq(root: string): ChannelInfo {
  return { id: "", name: ADMINISTRATION, topic: "", project_root: root, approval_mode: "custom", members: [], chief: "", revision: "" };
}

export interface ProjectChannels {
  hq: ChannelInfo;
  channels: ChannelInfo[];
  /** The open channel; null while the project's channels are not read yet, or when it is gone. */
  current: ChannelInfo | null;
  /** The open channel was listed before and no longer is. */
  gone: boolean;
}

export function projectChannels(state: AppState): ProjectChannels {
  const root = state.activeProjectRoot ?? "";
  const listed = state.visions.channels[root];
  const all = listed ?? [];
  const hq = all.find(channel => channel.name === ADMINISTRATION) ?? emptyHq(root);
  const channels = all.filter(channel => channel !== hq);
  const openId = state.visions.open.channelId;
  if (openId === null) return { hq, channels, current: hq, gone: false };
  const current = all.find(channel => channel.id === openId) ?? null;
  return { hq, channels, current, gone: current === null && listed !== undefined };
}
