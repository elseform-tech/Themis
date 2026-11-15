// The channel's composer (boards ChannelPlain, AdminChannel), in the chat composer's frame (its
// focus edge is the focus indicator): "Message <chief>" and Send. A post wakes only the chief, so
// there is nothing to mention. There is no mode shield: the channel's autonomy and Full access, in
// its details, set its chats' approvals.
import { useRef, useState, type KeyboardEvent } from "react";
import { Button } from "../../components";
import { postChannelMessage, type ChannelInfo } from "../../lib/visions";
import { describeError, toast, useApp } from "../../state/store";
import "../ThreadView.css";

export function ChannelComposer({ channel, chiefName, draft, setDraft, onPosted }: { channel: ChannelInfo; chiefName: string; draft: string; setDraft: (text: string) => void; /** The post's seq and the Visions it woke. */ onPosted?: (seq: number, woken: string[]) => void }) {
  const { dispatch } = useApp();
  const box = useRef<HTMLTextAreaElement>(null);
  const [busy, setBusy] = useState(false);
  const label = chiefName ? `Message ${chiefName}` : `Message #${channel.name}`;
  async function send() {
    const text = draft.trim();
    if (!text || busy) return;
    setBusy(true);
    try { const posted = await postChannelMessage(channel.id, text); setDraft(""); onPosted?.(posted.seq, posted.woken ?? []); }
    catch (error) { toast(dispatch, `Could not post the message: ${describeError(error)}`, "danger"); }
    finally { setBusy(false); }
  }
  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key !== "Enter" || event.shiftKey || event.nativeEvent.isComposing) return;
    event.preventDefault();
    void send();
  }
  return (
    <div className="themis-thread-composer themis-channel-composer" onClick={event => { if (event.target === event.currentTarget) box.current?.focus(); }}>
      <textarea ref={box} rows={1} className="themis-channel-input" aria-label={label} placeholder={label} value={draft} disabled={!channel.id}
        onChange={event => setDraft(event.target.value)} onKeyDown={onKeyDown} />
      <div className="themis-thread-composer-actions">
        <span className="themis-composer-spacer" />
        <Button className="themis-send" variant="primary" aria-label="Send" disabled={busy || !draft.trim() || !channel.id} onClick={() => void send()}>
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true"><path d="M12 19V5M5 12l7-7 7 7" /></svg>
        </Button>
      </div>
    </div>
  );
}
