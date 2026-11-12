// A Vision's chat opened from its channel: the chat itself, with who holds it. Typing in it takes
// over (the Vision stands by); "Hand back" gives it back, and the Vision reads what happened.
import { useEffect } from "react";
import { stopThread } from "../../lib/tauri";
import { handBackThread, type ChannelInfo, type OwnedChat } from "../../lib/visions";
import { describeError, toast, useApp } from "../../state/store";
import { ThreadView } from "../ThreadView";
import { visionById } from "./ChannelEntry";
import { chatRunning } from "./ChatLink";

/** Who holds the chat now; nothing when it is idle and its Vision's. */
export function holdNote(chat: OwnedChat, vision: string, running: boolean): string {
  if (chat.owner.hold === "taken_over") return "You took over.";
  if (chat.owner.hold === "stopped_by_person") return "You stopped this chat.";
  return running ? `${vision} is running this chat.` : "";
}

export function VisionChat({ channel, chat }: { channel: ChannelInfo; chat: OwnedChat }) {
  const { state, dispatch } = useApp();
  const vision = visionById(state, chat.owner.vision_id)?.name ?? "Its Vision";
  const running = chatRunning(state, chat);
  const held = chat.owner.hold !== "none";
  const note = holdNote(chat, vision, running);
  // The thread view shows the active chat: keep it the one `visions.open` names.
  const shown = state.activeThreadId === chat.id;
  useEffect(() => {
    if (!shown) dispatch({ type: "visions/open", channelId: channel.id, chatId: chat.id });
  }, [shown, dispatch, channel.id, chat.id]);
  async function act(what: () => Promise<unknown>, failed: string) {
    try { await what(); } catch (error) { toast(dispatch, `${failed}: ${describeError(error)}`, "danger"); }
  }
  return (
    <section className="themis-vision-chat" aria-label={`${vision}'s chat`}>
      <div className="themis-vision-chat-bar">
        <button type="button" className="themis-visions-quiet-button" aria-label={`Back to # ${channel.name}`} onClick={() => dispatch({ type: "visions/open", channelId: channel.id })}># {channel.name}</button>
        <span className="themis-vision-chat-note">{note && <span role="status">{note}</span>}</span>
        {running && <button type="button" className="themis-visions-quiet-button" aria-label="Stop this chat" onClick={() => void act(() => stopThread(chat.id), "Could not stop the chat")}>Stop</button>}
        {held && <button type="button" className="themis-visions-quiet-button is-primary" onClick={() => void act(() => handBackThread(chat.id), "Could not hand the chat back")}>Hand back to {vision}</button>}
      </div>
      {shown && <ThreadView />}
    </section>
  );
}
