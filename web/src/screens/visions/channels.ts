// The open project's channels as Visions shows them: `# hq` first, then the rest.
import type { ChannelInfo } from "../../lib/visions";
import type { useApp } from "../../state/store";

type AppState = ReturnType<typeof useApp>["state"];

/** `# hq`: the project's channel named hq, or an empty one until it exists. */
function emptyHq(root: string): ChannelInfo {
  return { id: "", name: "hq", topic: "", project_root: root, approval_mode: "custom", members: [], revision: "" };
}

export function projectChannels(state: AppState): { hq: ChannelInfo; channels: ChannelInfo[]; current: ChannelInfo } {
  const root = state.activeProjectRoot ?? "";
  const all = state.visions.channels[root] ?? [];
  const hq = all.find(channel => channel.name === "hq") ?? emptyHq(root);
  const channels = all.filter(channel => channel !== hq);
  const current = channels.find(channel => channel.id === state.visions.open.channelId) ?? hq;
  return { hq, channels, current };
}
