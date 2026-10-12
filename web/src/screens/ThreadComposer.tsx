import { AttachmentPreview } from "./AttachmentPreview";
import { PromptInput } from "../components/PromptInput";
import { useEffect, useRef } from "react";
import type { Attachment } from "../lib/tauri";
import type { PromptPlugin, PromptSkill } from "../lib/prompt";
import { Button } from "../components/primitives/Button";
import type { ContextBudget, ThreadInfo } from "../lib/types";
import { ContextMeter } from "./ContextMeter";

interface ThreadComposerProps {
  thread: ThreadInfo;
  draft: string;
  attachments?: Attachment[];
  onAttach?: () => void;
  onRemoveAttachment?: (path: string) => void;
  skills?: PromptSkill[];
  plugins?: PromptPlugin[];
  history?: string[];
  composerDisabled: boolean;
  readOnly: boolean;
  running: boolean;
  stopping: boolean;
  sendError: string | undefined;
  secretConfigured: boolean;
  onDraftChange: (value: string) => void;
  onSend: () => void;
  onStop: () => void;
  onNewThread: () => void;
  onOpenSettings: () => void;
  permissionSaving?: boolean;
  onApprovalModeChange?: (mode: "custom" | "yolo") => void;
  budget?: ContextBudget;
  compacting?: boolean;
  /** Centred, larger composer for an empty conversation. */
  hero?: boolean;
}

export function ThreadComposer({
  thread,
  draft,
  attachments = [],
  onAttach,
  onRemoveAttachment,
  skills = [],
  plugins = [],
  history = [],
  composerDisabled,
  readOnly,
  running,
  stopping,
  sendError,
  secretConfigured,
  onDraftChange,
  onSend,
  onStop,
  onOpenSettings,
  permissionSaving = false,
  onApprovalModeChange,
  budget,
  compacting = false,
  hero = false,
}: ThreadComposerProps) {
  const stopAction = useRef(onStop);
  stopAction.current = onStop;
  useEffect(() => {
    if (!running || stopping) return;
    let firstEscape: number | null = null;
    let requested = false;
    function stopShortcut(event: KeyboardEvent) {
      if (event.isComposing || event.repeat || requested) return;
      if (event.key !== "Escape") { firstEscape = null; return; }
      const now = Date.now();
      if (firstEscape !== null && now - firstEscape <= 500) {
        event.preventDefault();
        requested = true;
        stopAction.current();
      } else firstEscape = now;
    }
    window.addEventListener("keydown", stopShortcut, true);
    return () => window.removeEventListener("keydown", stopShortcut, true);
  }, [running, stopping, thread.id]);
  function insertTrigger(trigger: "/" | "@", menu: HTMLElement | null) {
    menu?.closest("details")?.removeAttribute("open");
    onDraftChange(`${draft}${draft === "" || draft.endsWith(" ") || draft.endsWith("\n") ? "" : " "}${trigger}`);
  }
  return (
    <>
      {sendError && <p className="themis-thread-senderror" role="alert">{sendError}</p>}
      <div className={`themis-thread-composer${hero ? " themis-thread-composer--hero" : ""}${running ? " is-running" : ""}`}>
        {running && <span className="themis-composer-ring" aria-hidden="true" />}
        {!secretConfigured && !running && (
          <p className="themis-thread-hint">
            Connect OpenCode Go to send your first message. {" "}
            <Button variant="ghost" size="small" onClick={onOpenSettings}>Open settings</Button>
          </p>
        )}
        {attachments.length > 0 && <ul className="themis-composer-attachments" aria-label="Attached files">
          {attachments.map(file => <li key={file.path} title={`${file.name} · ${file.size.toLocaleString()} bytes`}>
            <AttachmentPreview threadId={thread.id} path={file.path} />
            <button type="button" aria-label={`Remove ${file.name}`} disabled={composerDisabled} onClick={() => onRemoveAttachment?.(file.path)}>×</button>
          </li>)}
        </ul>}
        <PromptInput id="themis-composer" label="Message" value={draft} skills={skills} plugins={plugins} history={history} disabled={composerDisabled}
          placeholder={running ? "Run in progress…" : readOnly ? "Start a new thread to continue" : "Ask Themis… @ for plugins, / for skills"}
          onChange={onDraftChange} onSubmit={onSend} />
        <div className="themis-thread-composer-actions">
          <details className="themis-composer-add">
            <summary aria-label="Add files, skills or plugins" title="Add files, skills or plugins">
              <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14" /></svg>
            </summary>
            <div className="themis-composer-menu" role="group" aria-label="Attach or reference">
              <button type="button" aria-label="Attach files" disabled={composerDisabled} onClick={event => { event.currentTarget.closest("details")?.removeAttribute("open"); onAttach?.(); }}>
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true"><path d="m8 13 7-7a3 3 0 0 1 4 4L9 20a5 5 0 0 1-7-7L13 2" /></svg>Attach files
              </button>
              <button type="button" disabled={composerDisabled} onClick={event => insertTrigger("/", event.currentTarget)}>
                <span aria-hidden="true" className="themis-composer-menu-key">/</span>Use a skill
              </button>
              <button type="button" disabled={composerDisabled} onClick={event => insertTrigger("@", event.currentTarget)}>
                <span aria-hidden="true" className="themis-composer-menu-key">@</span>Mention a plugin
              </button>
            </div>
          </details>
          <details className="themis-composer-permissions" data-mode={thread.approval_mode ?? "custom"}>
            <summary aria-label={`Permissions: ${thread.approval_mode === "yolo" ? "YOLO" : "Custom"}`} title={thread.approval_mode === "yolo" ? "YOLO bypasses harness tool restrictions for the next run" : "Custom applies your rules and asks for tool approvals"}>
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true"><path d="M12 3 4 6.5V12c0 4.5 8 9 8 9s8-4.5 8-9V6.5Z" /></svg>
              <span>{permissionSaving ? "Saving…" : thread.approval_mode === "yolo" ? "YOLO" : "Custom"}</span>
            </summary>
            <div className="themis-permissions-panel"><p>{thread.approval_mode === "yolo" ? "Bypass harness tool restrictions." : "Apply your rules and ask for tool approvals."} Changes apply to the next run.</p>
              <label><select aria-label="Approval mode" value={thread.approval_mode ?? "custom"} disabled={permissionSaving} onChange={event => onApprovalModeChange?.(event.target.value as "custom" | "yolo")}><option value="yolo">YOLO</option><option value="custom">Custom</option></select></label>
            </div>
          </details>
          <span className="themis-composer-spacer" />
          <ContextMeter budget={budget} compacting={compacting} running={running} />
          {running ? (
            <Button
              className="themis-send"
              variant="ghost"
              aria-label={stopping ? "Stopping" : "Stop"}
              title={stopping ? "Stopping…" : "Stop (Esc twice)"}
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
              title="Send (Enter) · Shift+Enter for a new line · ⌘/Ctrl+Shift+O new thread"
              disabled={composerDisabled || (draft.trim() === "" && attachments.length === 0)}
              onClick={onSend}
            >
              <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true">
                <path d="M12 19V5M5 12l7-7 7 7" />
              </svg>
            </Button>
          )}
        </div>
      </div>
    </>
  );
}
