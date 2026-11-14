// Visions mode's main view: a channel (`# administration` is one too), a chat opened from a channel,
// or a chief's profile in the channel's place, with a worker's thread or the channel's details
// beside it. What is open is `visions.open` alone; when it is gone the view says so in place and
// never moves on its own.
import type { ReactNode } from "react";
import { useApp } from "../../state/store";
import { ChannelDetails } from "./ChannelDetails";
import { ChannelView } from "./ChannelView";
import { channelChat } from "./ChatLink";
import { projectChannels } from "./channels";
import { leadsAChannel } from "./team";
import { TeamThread } from "./TeamThread";
import { VisionChat } from "./VisionChat";
import { VisionProfile } from "./VisionProfile";

function Gone({ text, back }: { text: string; back?: { name: string; onBack: () => void } }) {
  return (
    <section className="themis-channel" aria-label={text}>
      {back && <div className="themis-vision-chat-bar"><button type="button" className="themis-visions-quiet-button" aria-label={`Back to # ${back.name}`} onClick={back.onBack}># {back.name}</button></div>}
      <p className="themis-channel-empty themis-channel-gone">{text}</p>
    </section>
  );
}

export function VisionsView() {
  const { state } = useApp();
  if (state.mainView !== "visions") return null;
  const panel = state.visions.open.panel;
  const current = projectChannels(state).current;
  // Only a chief (Atlas too) has a profile; a worker opens its thread with its chief.
  const profile = panel?.kind === "vision" && leadsAChannel(state, panel.visionId) ? panel.visionId : null;
  const worker = panel?.kind === "worker" || (panel?.kind === "vision" && !profile) ? panel.visionId : null;
  const view = profile ? <VisionProfile key={profile} visionId={profile} /> : <MainView />;
  let side: ReactNode = null;
  // TODO(Task 9 wiring): the worker's log needs a person-facing read of its team log.
  if (worker && current?.members.includes(worker)) side = <TeamThread key={worker} channel={current} workerId={worker} log={null} />;
  else if (panel?.kind === "channel" && current?.id) side = <ChannelDetails key={current.id} channel={current} />;
  return <div className="themis-visions-main">{view}{side}</div>;
}

function MainView() {
  const { state, dispatch } = useApp();
  const { current: channel, gone } = projectChannels(state);
  if (gone) return <Gone text="This channel was deleted." />;
  if (!channel) return null;
  const chatId = state.visions.open.chatId;
  if (!chatId || !channel.id) return <ChannelView key={channel.id || "hq"} channel={channel} />;
  const chat = channelChat(state, channel.id, chatId);
  if (chat) return <VisionChat channel={channel} chat={chat} />;
  const listed = state.visions.chats[channel.id] !== undefined;
  return listed ? <Gone text="This chat was deleted." back={{ name: channel.name, onBack: () => dispatch({ type: "visions/open", channelId: channel.id }) }} /> : null;
}
