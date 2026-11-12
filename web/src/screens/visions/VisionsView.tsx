// Visions mode's main view: `# hq`, a channel, or a chat opened from a channel.
import { useState } from "react";
import { Button } from "../../components";
import { useApp } from "../../state/store";
import { ChannelView } from "./ChannelView";
import { channelChat } from "./ChatLink";
import { VisionsSheets, type Sheet } from "./NewChannelSheet";
import { VisionChat } from "./VisionChat";

/** `# hq`: where Visions starts. Phase 1 has no agent here, only the two ways to begin. */
function HqPage({ project }: { project: string }) {
  const [sheet, setSheet] = useState<Sheet>(null);
  return (
    <section className="themis-channel" aria-label="# hq">
      <header className="themis-channel-head"><h1># hq</h1></header>
      <div className="themis-visions-start">
        <p>A channel is one kind of work in {project}. Its Visions start and run chats for you, answer what they may, and report here.</p>
        <div className="themis-visions-start-actions">
          <Button variant="primary" onClick={() => setSheet("channel")}>New channel</Button>
          <Button variant="ghost" onClick={() => setSheet("vision")}>New Vision</Button>
        </div>
      </div>
      <VisionsSheets sheet={sheet} onClose={() => setSheet(null)} />
    </section>
  );
}

export function VisionsView() {
  const { state } = useApp();
  if (state.mainView !== "visions") return null;
  const root = state.activeProjectRoot;
  const channel = (root ? state.visions.channels[root] ?? [] : []).find(each => each.id === state.visions.open.channelId);
  const project = state.projects.find(each => each.root === root)?.name ?? "this project";
  if (!channel) return <HqPage project={project} />;
  const chatId = state.visions.open.chatId;
  const chat = chatId ? channelChat(state, channel.id, chatId) : undefined;
  if (chat) return <VisionChat channel={channel} chat={chat} />;
  return <ChannelView key={channel.id} channel={channel} />;
}
