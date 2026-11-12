// Visions mode's main view: a channel (`# hq` is one too), or a chat opened from a channel. What is
// open is `visions.open` alone; when it is gone the view says so in place and never moves on its own.
import { useApp } from "../../state/store";
import { ChannelView } from "./ChannelView";
import { channelChat } from "./ChatLink";
import { projectChannels } from "./channels";
import { VisionChat } from "./VisionChat";

function Gone({ text, back }: { text: string; back?: { name: string; onBack: () => void } }) {
  return (
    <section className="themis-channel" aria-label={text}>
      {back && <div className="themis-vision-chat-bar"><button type="button" className="themis-visions-quiet-button" aria-label={`Back to # ${back.name}`} onClick={back.onBack}># {back.name}</button></div>}
      <p className="themis-channel-empty themis-channel-gone">{text}</p>
    </section>
  );
}

export function VisionsView() {
  const { state, dispatch } = useApp();
  if (state.mainView !== "visions") return null;
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
