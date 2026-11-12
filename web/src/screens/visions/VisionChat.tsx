// A Vision's chat opened from its channel: the chat itself, with who holds it. Typing in it takes
// over (the Vision stands by); the cloud's "Hand back" gives it back, and the Vision reads what
// happened.
import { useEffect } from "react";
import { stopThread } from "../../lib/tauri";
import type { ChannelInfo, OwnedChat } from "../../lib/visions";
import { describeError, toast, useApp } from "../../state/store";
import { ThreadView } from "../ThreadView";
import { visionById } from "./ChannelEntry";
import { chatRunning } from "./ChatLink";
import { HandBackButton, holdNote } from "./HandBack";

export function VisionChat({ channel, chat }: { channel: ChannelInfo; chat: OwnedChat }) {
  const { state, dispatch } = useApp();
  const profile = visionById(state, chat.owner.vision_id);
  const vision = profile?.name ?? "Its Vision";
  const running = chatRunning(state, chat);
  const held = chat.owner.hold !== "none";
  const note = holdNote(chat.owner.hold, vision, running);
  // The thread view shows the active chat: keep it the one `visions.open` names.
  const shown = state.activeThreadId === chat.id;
  useEffect(() => {
    if (!shown) dispatch({ type: "visions/reshow" });
  }, [shown, dispatch]);
  async function stop() {
    try { await stopThread(chat.id); } catch (error) { toast(dispatch, `Could not stop the chat: ${describeError(error)}`, "danger"); }
  }
  return (
    <section className="themis-vision-chat" aria-label={`${vision}'s chat`}>
      <div className="themis-vision-chat-bar">
        <button type="button" className="themis-visions-quiet-button" aria-label={`Back to # ${channel.name}`} onClick={() => dispatch({ type: "visions/open", channelId: channel.id })}># {channel.name}</button>
        <span className="themis-vision-chat-note">{note && <span role="status">{note}</span>}</span>
        {running && <button type="button" className="themis-visions-quiet-button" aria-label="Stop this chat" onClick={() => void stop()}>Stop</button>}
        {held && <HandBackButton threadId={chat.id} vision={profile ?? { name: vision, shape: "tall", color: "blue" }} />}
      </div>
      {shown && <ThreadView />}
    </section>
  );
}
