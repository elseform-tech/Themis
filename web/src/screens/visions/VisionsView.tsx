// Visions mode's main view: a channel (`# hq` is one too), or a chat opened from a channel.
import { useApp } from "../../state/store";
import { ChannelView } from "./ChannelView";
import { channelChat } from "./ChatLink";
import { projectChannels } from "./channels";
import { VisionChat } from "./VisionChat";

export function VisionsView() {
  const { state } = useApp();
  if (state.mainView !== "visions") return null;
  const { current: channel } = projectChannels(state);
  const chatId = state.visions.open.chatId;
  const chat = chatId && channel.id ? channelChat(state, channel.id, chatId) : undefined;
  if (chat) return <VisionChat channel={channel} chat={chat} />;
  return <ChannelView key={channel.id || "hq"} channel={channel} />;
}
