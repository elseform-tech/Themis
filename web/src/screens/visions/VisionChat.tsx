// A Vision's chat opened from its channel: the chat itself, with who drives it. Typing in it takes
// over (the Vision stands by); "Hand back" gives it back, and the Vision reads what happened.
import { stopThread } from "../../lib/tauri";
import { handBackThread, type ChannelInfo, type OwnedChat } from "../../lib/visions";
import { describeError, toast, useApp } from "../../state/store";
import { ThreadView } from "../ThreadView";
import { visionById } from "./ChannelEntry";
import { chatRunning } from "./ChatLink";

function holdNote(chat: OwnedChat, vision: string, running: boolean): string {
  if (chat.owner.hold === "taken_over") return `You took over. ${vision} waits and reads what you did when you hand it back.`;
  if (chat.owner.hold === "stopped_by_person") return `You stopped this chat. ${vision} leaves it until you hand it back or write in the channel.`;
  return running ? `${vision} is running this chat. Typing here takes over.` : `${vision} runs this chat. Typing here takes over.`;
}

export function VisionChat({ channel, chat }: { channel: ChannelInfo; chat: OwnedChat }) {
  const { state, dispatch } = useApp();
  const vision = visionById(state, chat.owner.vision_id)?.name ?? "Its Vision";
  const running = chatRunning(state, chat);
  const held = chat.owner.hold !== "none";
  async function act(what: () => Promise<unknown>, failed: string) {
    try { await what(); } catch (error) { toast(dispatch, `${failed}: ${describeError(error)}`, "danger"); }
  }
  return (
    <div className="themis-vision-chat">
      <div className="themis-vision-chat-bar">
        <button type="button" className="themis-visions-quiet-button" aria-label={`Back to # ${channel.name}`} onClick={() => dispatch({ type: "visions/open", channelId: channel.id })}># {channel.name}</button>
        <span className="themis-vision-chat-note" role="status">{holdNote(chat, vision, running)}</span>
        {running && <button type="button" className="themis-visions-quiet-button" aria-label="Stop this chat" onClick={() => void act(() => stopThread(chat.id), "Could not stop the chat")}>Stop</button>}
        {held && <button type="button" className="themis-visions-quiet-button is-primary" onClick={() => void act(() => handBackThread(chat.id), "Could not hand the chat back")}>Hand back to {vision}</button>}
      </div>
      <ThreadView />
    </div>
  );
}
