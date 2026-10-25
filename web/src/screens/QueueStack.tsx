import { useState } from "react";
import { ANSWER_MARKER, type QueuedMessage } from "../lib/types";
import "./GoalBar.css";

interface QueueStackProps {
  queue: QueuedMessage[];
  /** Steering needs a run to deliver into. */
  running: boolean;
  onSteer: (id: string) => void;
  /** Holds (or releases) a message while it is edited, so it is not sent half-written. */
  onHold: (id: string, held: boolean) => void;
  onSave: (id: string, text: string) => Promise<void>;
  onRemove: (id: string) => void;
}

/** Messages waiting for the current run, oldest first, each with Steer, Edit and Remove. */
export function QueueStack({ queue, running, onSteer, onHold, onSave, onRemove }: QueueStackProps) {
  const [editing, setEditing] = useState<{ id: string; text: string } | null>(null);
  if (queue.length === 0) return null;
  function startEdit(message: QueuedMessage) {
    setEditing({ id: message.id, text: message.text });
    onHold(message.id, true);
  }
  function cancel() {
    if (editing) onHold(editing.id, false);
    setEditing(null);
  }
  async function save() {
    if (!editing || editing.text.trim() === "") return;
    await onSave(editing.id, editing.text.trim());
    setEditing(null);
  }
  return (
    <ol className="themis-queue" aria-label="Queued messages">
      {queue.map((message, index) => {
        const isEditing = editing?.id === message.id;
        return (
          <li key={message.id} className={`themis-queue-row${isEditing ? " is-editing" : ""}`}>
            <span className="themis-queue-index" aria-hidden="true">{index + 1}</span>
            {isEditing ? (
              <>
                <label className="themis-sr-only" htmlFor={`themis-queue-${message.id}`}>Edit queued message</label>
                <input id={`themis-queue-${message.id}`} className="themis-queue-input" value={editing.text} autoFocus
                  onChange={event => setEditing({ id: message.id, text: event.target.value })}
                  onKeyDown={event => {
                    if (event.key === "Escape") { event.preventDefault(); cancel(); }
                    if (event.key === "Enter" && !event.nativeEvent.isComposing) { event.preventDefault(); void save(); }
                  }} />
                <button type="button" className="themis-goal-icon themis-icon-tip" data-tooltip="Discard edits" aria-label="Discard edits" onClick={cancel}>
                  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true"><path d="M6 6l12 12M18 6 6 18" /></svg>
                </button>
                <button type="button" className="themis-goal-icon is-primary themis-icon-tip" data-tooltip="Save" aria-label="Save queued message" disabled={editing.text.trim() === ""} onClick={() => void save()}>
                  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M5 12l5 5 9-10" /></svg>
                </button>
              </>
            ) : (
              <>
                <span className="themis-queue-text" title={message.text}>
                  {message.text.startsWith(ANSWER_MARKER) ? "Your answer to the agent's question" : message.text || "Attached files"}
                  {message.attachments.length > 0 && <small> · {message.attachments.length} file{message.attachments.length === 1 ? "" : "s"}</small>}
                </span>
                {message.note && (
                  <span className="themis-queue-note themis-icon-tip" data-tooltip={message.note} tabIndex={0} aria-label={`Not sent: ${message.note}`}>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true"><path d="M12 4 3 20h18ZM12 10v4M12 17v.01" /></svg>
                    Needs an edit
                  </span>
                )}
                {running && message.attachments.length === 0 && !message.note && (
                  <button type="button" className="themis-goal-icon themis-icon-tip" data-tooltip="Steer — send now" aria-label="Steer: send now" onClick={() => onSteer(message.id)}>
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M4 12h12M12 6l6 6-6 6" /></svg>
                  </button>
                )}
                <button type="button" className="themis-goal-icon themis-icon-tip" data-tooltip="Edit" aria-label="Edit queued message" onClick={() => startEdit(message)}>
                  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M4 20h4L19 9l-4-4L4 16Z" /><path d="M13.5 6.5l4 4" /></svg>
                </button>
                <button type="button" className="themis-goal-icon themis-icon-tip" data-tooltip="Remove from queue" aria-label="Remove from queue" onClick={() => onRemove(message.id)}>
                  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true"><path d="M6 6l12 12M18 6 6 18" /></svg>
                </button>
              </>
            )}
          </li>
        );
      })}
    </ol>
  );
}
