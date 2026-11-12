// Handing a chat back to its Vision, from the Vision's chat in its channel and from plain chat:
// a button with the Vision's cloud. Plain chat's bar also says who holds the chat now.
import type { ThreadVision } from "../../lib/types";
import { handBackThread } from "../../lib/visions";
import { describeError, toast, useApp } from "../../state/store";
import { ownedChat } from "../../state/visions";
import { chatRunning } from "./ChatLink";
import { CloudIcon } from "./CloudIcon";

type AppState = ReturnType<typeof useApp>["state"];
type Look = Pick<ThreadVision, "name" | "shape" | "color">;

/** How a Vision looks now: its listed profile, else what the chat was marked with. */
export function visionLook(state: AppState, id: string, marked?: Look): Look | undefined {
  return state.visions.list.find(vision => vision.id === id) ?? marked;
}

/** Who holds the chat now; nothing when it is idle and its Vision's. */
export function holdNote(hold: string, vision: string, running: boolean): string {
  if (hold === "taken_over") return "You took over.";
  if (hold === "stopped_by_person") return "You stopped this chat.";
  return running ? `${vision} is running this chat.` : "";
}

export function HandBackButton({ threadId, vision }: { threadId: string; vision: Look }) {
  const { dispatch } = useApp();
  async function handBack() {
    try { await handBackThread(threadId); } catch (error) { toast(dispatch, `Could not hand the chat back: ${describeError(error)}`, "danger"); }
  }
  return (
    <button type="button" className="themis-visions-quiet-button themis-hand-back" onClick={() => void handBack()}>
      <CloudIcon shape={vision.shape} color={vision.color} mood="calm" size={16} />Hand back to {vision.name}
    </button>
  );
}

/** Plain chat's bar for a Vision's chat: who holds it, and Hand back while the person does. */
export function PlainVisionBar() {
  const { state } = useApp();
  const threadId = state.activeThreadId;
  const thread = threadId === null ? undefined : (state.threadsByProject[state.activeProjectRoot ?? ""] ?? []).find(each => each.id === threadId);
  const chat = thread?.vision ? ownedChat(state, thread.id) : undefined;
  const vision = thread?.vision ? visionLook(state, thread.vision.id, thread.vision) : undefined;
  if (!chat || !vision) return null;
  const note = holdNote(chat.owner.hold, vision.name, chatRunning(state, chat));
  const held = chat.owner.hold !== "none";
  if (!note && !held) return null;
  return (
    <div className="themis-plain-vision-bar">
      {note && <span className="themis-vision-chat-note" role="status">{note}</span>}
      {held && <HandBackButton threadId={chat.id} vision={vision} />}
    </div>
  );
}
