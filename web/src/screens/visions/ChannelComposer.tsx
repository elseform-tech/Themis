// The channel's composer. `@` lists the channel's Visions to mention; with several here and no
// mention yet, "Mention a Vision" says who should pick the message up (nobody wakes otherwise).
import { useRef, useState, type KeyboardEvent } from "react";
import type { ChannelInfo, VisionInfo } from "../../lib/visions";
import { postChannelMessage } from "../../lib/visions";
import { describeError, toast, useApp } from "../../state/store";
import { CloudIcon } from "./CloudIcon";

/** The `@name` being typed at the end of the draft, if any. */
function typedMention(draft: string): string | null {
  return /(?:^|\s)@([^\s@]*)$/.exec(draft)?.[1] ?? null;
}

export function ChannelComposer({ channel, members, draft, setDraft }: { channel: ChannelInfo; members: VisionInfo[]; draft: string; setDraft: (text: string) => void }) {
  const { dispatch } = useApp();
  const box = useRef<HTMLTextAreaElement>(null);
  const [busy, setBusy] = useState(false);
  const typed = typedMention(draft);
  const matches = typed === null ? [] : members.filter(vision => vision.name.toLowerCase().startsWith(typed.toLowerCase()));
  function mention(name: string) {
    setDraft(`${draft.slice(0, draft.length - (typed?.length ?? 0) - 1)}@${name} `);
    box.current?.focus();
  }
  async function send() {
    const text = draft.trim();
    if (!text || busy) return;
    setBusy(true);
    try { await postChannelMessage(channel.id, text); setDraft(""); }
    catch (error) { toast(dispatch, `Could not post the message: ${describeError(error)}`, "danger"); }
    finally { setBusy(false); }
  }
  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key !== "Enter" || event.shiftKey || event.nativeEvent.isComposing) return;
    event.preventDefault();
    if (matches.length > 0) mention(matches[0]!.name);
    else void send();
  }
  return (
    <div className="themis-channel-composer">
      {matches.length > 0 && <div className="themis-mention-list" role="listbox" aria-label="Mention">
        {matches.map((vision, index) => (
          <button key={vision.id} type="button" role="option" aria-selected={index === 0} onMouseDown={event => event.preventDefault()} onClick={() => mention(vision.name)}>
            <CloudIcon shape={vision.shape} color={vision.color} mood="calm" />{vision.name}
          </button>
        ))}
      </div>}
      <textarea ref={box} rows={1} aria-label={`Message #${channel.name}`} placeholder={`Message #${channel.name}`} value={draft} disabled={!channel.id} onChange={event => setDraft(event.target.value)} onKeyDown={onKeyDown} />
      {channel.id && members.length > 1 && !draft.includes("@") && <button type="button" className="themis-channel-mention" aria-label="Mention a Vision" title="Mention a Vision"
        onClick={() => { setDraft(`${draft}${draft && !draft.endsWith(" ") ? " " : ""}@`); box.current?.focus(); }}>
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" aria-hidden="true"><circle cx="12" cy="12" r="4" /><path d="M16 8v5a3 3 0 0 0 5 0v-1a9 9 0 1 0-3.5 7" /></svg>
      </button>}
      <button type="button" className="themis-channel-send" aria-label="Send" disabled={busy || !draft.trim() || !channel.id} onClick={() => void send()}>
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M12 19V5M6 11l6-6 6 6" /></svg>
      </button>
    </div>
  );
}
