// A link to a chat a Vision runs, with its live status and Stop while it runs. Opening it shows
// the chat inside Visions, where the person can watch it, take over and hand it back.
import { stopThread } from "../../lib/tauri";
import type { OwnedChat } from "../../lib/visions";
import { describeError, toast, useApp } from "../../state/store";
import { threadStatus } from "../Sidebar";

type AppState = ReturnType<typeof useApp>["state"];

/** A channel's chat by id, as last listed. */
export function channelChat(state: AppState, channelId: string, threadId: string): OwnedChat | undefined {
  return (state.visions.chats[channelId] ?? []).find(chat => chat.id === threadId);
}

/** Whether the chat's run is going now. */
export function chatRunning(state: AppState, chat: OwnedChat): boolean {
  return state.running[chat.id] ?? chat.running;
}

/** The chat's state in a few words: the person's hold first, then the sidebar's status. */
export function chatState(state: AppState, chat: OwnedChat): string {
  const status = threadStatus(state, chat);
  if (status === "Working") return "running";
  if (chat.owner.hold === "taken_over") return "you took over";
  if (chat.owner.hold === "stopped_by_person") return "stopped by you";
  if (status === "Needs approval") return "waiting for your approval";
  if (status === "Question") return "asking";
  return status.toLowerCase();
}

function ChatIcon() {
  return <svg className="themis-sidebar-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M7.9 20A9 9 0 1 0 4 16.1L2 22Z" /></svg>;
}

export function ChatLink({ channelId, threadId, title, visionName }: { channelId: string; threadId: string; title: string; visionName: string }) {
  const { state, dispatch } = useApp();
  const chat = channelChat(state, channelId, threadId);
  const running = chat ? chatRunning(state, chat) : false;
  const shownTitle = chat?.title || title;
  const meta = chat ? `${visionName} · ${chatState(state, chat)}` : `${visionName} · removed`;
  async function stop() {
    try { await stopThread(threadId); } catch (error) { toast(dispatch, `Could not stop the chat: ${describeError(error)}`, "danger"); }
  }
  return (
    <div className={`themis-chat-link${running ? " is-running" : ""}`}>
      <button type="button" className="themis-chat-link-open" aria-label={`Open ${shownTitle}, ${meta}`} disabled={!chat} onClick={() => dispatch({ type: "visions/open", channelId, chatId: threadId })}>
        <ChatIcon /><span className="themis-chat-link-title">{shownTitle}</span><span className="themis-chat-link-meta">{meta}</span>
      </button>
      {running && <button type="button" className="themis-visions-quiet-button" aria-label={`Stop ${visionName}'s chat, ${shownTitle}`} onClick={() => void stop()}>
        <svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><rect x="6" y="6" width="12" height="12" rx="2" /></svg>Stop
      </button>}
    </div>
  );
}
