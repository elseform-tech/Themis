import { Button } from "../components/primitives/Button";
import type { ThreadInfo } from "../lib/types";

interface ThreadComposerProps {
  thread: ThreadInfo;
  draft: string;
  models: string[];
  supportsEffort: boolean;
  effort: string;
  composerDisabled: boolean;
  composerBusy: boolean;
  readOnly: boolean;
  running: boolean;
  stopping: boolean;
  switching: boolean;
  sendError: string | undefined;
  secretConfigured: boolean;
  onDraftChange: (value: string) => void;
  onSend: () => void;
  onStop: () => void;
  onNewThread: () => void;
  onProviderChange: (model: string) => void;
  onEffortChange: (effort: string) => void;
  onOpenSettings: () => void;
}

export function ThreadComposer({
  thread,
  draft,
  models,
  supportsEffort,
  effort,
  composerDisabled,
  composerBusy,
  readOnly,
  running,
  stopping,
  switching,
  sendError,
  secretConfigured,
  onDraftChange,
  onSend,
  onStop,
  onNewThread,
  onProviderChange,
  onEffortChange,
  onOpenSettings,
}: ThreadComposerProps) {
  return (
    <>
      {sendError && <p className="themis-thread-senderror" role="alert">{sendError}</p>}
      <div className="themis-thread-composer">
        {thread.provider !== "ollama" && thread.provider !== "custom" && !secretConfigured && (
          <p className="themis-thread-hint">
            Connect {thread.provider === "go" ? "OpenCode Go" : thread.provider} to send your first message. {" "}
            <Button variant="ghost" size="small" onClick={onOpenSettings}>Open settings</Button>
          </p>
        )}
        <label className="themis-sr-only" htmlFor="themis-composer">Message</label>
        <textarea
          id="themis-composer"
          className="themis-thread-input"
          placeholder={running ? "Run in progress…" : readOnly ? "Start a new thread to continue" : "Message Themis…"}
          value={draft}
          disabled={composerDisabled}
          rows={2}
          onChange={event => onDraftChange(event.target.value)}
          onKeyDown={event => {
            if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
              event.preventDefault();
              onSend();
            }
          }}
        />
        <div className="themis-thread-composer-actions">
          <Button
            variant="ghost"
            size="small"
            aria-label="New thread"
            title="New thread (⌘/Ctrl+Shift+O)"
            onClick={onNewThread}
          >+
          </Button>
          <div className="themis-composer-selectors">
            <select
              aria-label="Model"
              title="Model"
              value={thread.model}
              disabled={composerBusy || switching}
              onChange={event => onProviderChange(event.target.value)}
            >
              {[...new Set([thread.model, ...models])].map(model => (
                <option key={model} value={model}>{model || "Default model"}</option>
              ))}
            </select>
            <select
              aria-label="Reasoning effort"
              title={supportsEffort ? "Reasoning effort" : "This model manages its own reasoning"}
              value={effort}
              disabled={composerBusy || !supportsEffort}
              onChange={event => onEffortChange(event.target.value)}
            >
              <option value="">Default effort</option>
              <option value="low">Low</option>
              <option value="medium">Medium</option>
              <option value="high">High</option>
            </select>
          </div>
          {running ? (
            <Button
              className="themis-send"
              variant="ghost"
              aria-label={stopping ? "Stopping" : "Stop"}
              title={stopping ? "Stopping after current action" : "Stop"}
              disabled={stopping}
              onClick={onStop}
            >
              <svg width="14" height="14" viewBox="0 0 16 16" aria-hidden="true">
                <rect x="3" y="3" width="10" height="10" rx="2" fill="currentColor" />
              </svg>
            </Button>
          ) : (
            <Button
              className="themis-send"
              variant="primary"
              aria-label="Send"
              title="Send (Enter)"
              disabled={composerDisabled || draft.trim() === ""}
              onClick={onSend}
            >
              <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
                <path d="M8 13V3M3 8l5-5 5 5" />
              </svg>
            </Button>
          )}
        </div>
        <div className="themis-composer-shortcuts">
          <span>↵ Send · ⇧↵ New line</span>
          <span>⌘/Ctrl ⇧ O New thread</span>
        </div>
      </div>
    </>
  );
}
