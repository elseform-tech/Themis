// The channel's composer, in the chat composer's frame (its focus edge is the focus indicator): the
// message on top, then a row with the mention button and Send. `@` lists the channel's Visions
// (arrow keys move, Enter picks, Escape closes); with several here and no mention yet, "Mention a Vision" offers
// one (nobody wakes otherwise).
import { useId, useRef, useState, type KeyboardEvent } from "react";
import { Button } from "../../components";
import type { ChannelInfo, VisionInfo } from "../../lib/visions";
import { postChannelMessage } from "../../lib/visions";
import { describeError, toast, useApp } from "../../state/store";
import { CloudIcon } from "./CloudIcon";
import "../ThreadView.css";

/** The `@name` being typed at the end of the draft, if any. */
function typedMention(draft: string): string | null {
  return /(?:^|\s)@([^\s@]*)$/.exec(draft)?.[1] ?? null;
}

/** The Visions matching the `@name` being typed, and which of them the arrow keys point at (the
 *  first again on each edit). Escape hides the list until the draft changes. */
function useMentions(draft: string, members: VisionInfo[]) {
  const typed = typedMention(draft);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const [picked, setPicked] = useState(0);
  const matches = typed === null || dismissed === draft ? [] : members.filter(vision => vision.name.toLowerCase().startsWith(typed.toLowerCase()));
  const active = Math.min(picked, Math.max(matches.length - 1, 0));
  const move = (step: number) => setPicked((active + step + matches.length) % matches.length);
  return { typed, matches, active, move, dismiss: () => setDismissed(draft), restart: () => setPicked(0) };
}

export function ChannelComposer({ channel, members, draft, setDraft }: { channel: ChannelInfo; members: VisionInfo[]; draft: string; setDraft: (text: string) => void }) {
  const { dispatch } = useApp();
  const box = useRef<HTMLTextAreaElement>(null);
  const listId = useId();
  const [busy, setBusy] = useState(false);
  const { typed, matches, active, move, dismiss, restart } = useMentions(draft, members);
  const optionId = (index: number) => `${listId}-${index}`;
  function mention(name: string) {
    setDraft(`${draft.slice(0, draft.length - (typed?.length ?? 0) - 1)}@${name} `);
    restart();
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
    if (matches.length > 0 && ["ArrowDown", "ArrowUp", "Escape"].includes(event.key)) {
      event.preventDefault();
      event.stopPropagation();
      if (event.key === "Escape") dismiss();
      else move(event.key === "ArrowDown" ? 1 : -1);
      return;
    }
    if (event.key !== "Enter" || event.shiftKey || event.nativeEvent.isComposing) return;
    event.preventDefault();
    if (matches.length > 0) mention(matches[active]!.name);
    else void send();
  }
  return (
    <div className="themis-thread-composer themis-channel-composer" onClick={event => { if (event.target === event.currentTarget) box.current?.focus(); }}>
      {matches.length > 0 && <div id={listId} className="themis-mention-list" role="listbox" aria-label="Mention">
        {matches.map((vision, index) => (
          <button key={vision.id} id={optionId(index)} type="button" role="option" aria-selected={index === active} tabIndex={-1} onMouseDown={event => event.preventDefault()} onClick={() => mention(vision.name)}>
            <CloudIcon shape={vision.shape} color={vision.color} mood="calm" />{vision.name}
          </button>
        ))}
      </div>}
      <textarea ref={box} rows={1} className="themis-channel-input" aria-label={`Message #${channel.name}`} placeholder={`Message #${channel.name}`} value={draft} disabled={!channel.id}
        role="combobox" aria-expanded={matches.length > 0} aria-autocomplete="list" aria-haspopup="listbox"
        aria-controls={matches.length > 0 ? listId : undefined} aria-activedescendant={matches.length > 0 ? optionId(active) : undefined}
        onChange={event => { setDraft(event.target.value); restart(); }} onKeyDown={onKeyDown} />
      <div className="themis-thread-composer-actions">
        {channel.id && members.length > 1 && !draft.includes("@") && <button type="button" className="themis-channel-mention" aria-label="Mention a Vision" title="Mention a Vision"
          onClick={() => { setDraft(`${draft}${draft && !draft.endsWith(" ") ? " " : ""}@`); box.current?.focus(); }}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" aria-hidden="true"><circle cx="12" cy="12" r="4" /><path d="M16 8v5a3 3 0 0 0 5 0v-1a9 9 0 1 0-3.5 7" /></svg>
        </button>}
        <span className="themis-composer-spacer" />
        <Button className="themis-send" variant="primary" aria-label="Send" title="Send (Enter) · Shift+Enter for a new line" disabled={busy || !draft.trim() || !channel.id} onClick={() => void send()}>
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true"><path d="M12 19V5M5 12l7-7 7 7" /></svg>
        </Button>
      </div>
    </div>
  );
}
