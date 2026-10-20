import { AttachmentPreview } from "./AttachmentPreview";
import { PromptInput, type PromptCommand } from "../components/PromptInput";
import { useEffect, useRef, type ReactNode } from "react";
import type { Attachment } from "../lib/tauri";
import type { PromptPlugin, PromptSkill } from "../lib/prompt";
import { Button } from "../components/primitives/Button";
import type { ContextBudget, ThreadInfo } from "../lib/types";
import { isModeInitial } from "../lib/keyboard";
import { ContextMeter } from "./ContextMeter";

const COMMANDS: PromptCommand[] = [
  { id: "goal", name: "/goal", description: "Keep working until a goal is met — checked before every stop", command: true },
];

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
  /** Project directory and current Git branch shown beside the composer controls. */
  location?: { name: string; root: string; branch: string | null };
  compacting?: boolean;
  /** Centred, larger composer for an empty conversation. */
  hero?: boolean;
  /** Shown on top of the composer: the chat's goal bar and queued messages. */
  top?: ReactNode;
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
  location,
  top,
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
  // A click on the box's padding or quiet areas focuses the text area; real controls keep their own behaviour.
  function focusEditorFromBox(event: React.MouseEvent<HTMLDivElement>) {
    if ((event.target as Element).closest("button, summary, a, select, input, textarea, details, label, ul, [role='textbox'], .themis-permissions-panel, .themis-composer-menu")) return;
    event.currentTarget.querySelector<HTMLElement>(".themis-prompt-editor")?.focus();
  }
  function insertTrigger(trigger: "/" | "@", menu: HTMLElement | null) {
    menu?.closest("details")?.removeAttribute("open");
    onDraftChange(`${draft}${draft === "" || draft.endsWith(" ") || draft.endsWith("\n") ? "" : " "}${trigger}`);
  }
  return (
    <>
      {sendError && <p className="themis-thread-senderror" role="alert">{sendError}</p>}
      <div onClick={focusEditorFromBox} className={`themis-thread-composer${hero ? " themis-thread-composer--hero" : ""}${running ? " is-running" : ""}`}>
        {running && <span className="themis-composer-ring" aria-hidden="true" />}
        {top}
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
        <PromptInput id="themis-composer" label="Message" value={draft} skills={skills} plugins={plugins} commands={COMMANDS} history={history} disabled={composerDisabled}
          placeholder={running ? "Queue a message — or steer it in" : readOnly ? "Start a new thread to continue" : "Ask Themis… @ for plugins, / for skills"}
          onChange={onDraftChange} onSubmit={onSend} />
        <div className="themis-thread-composer-actions">
          <details className="themis-composer-add" data-popover>
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
          <details className="themis-composer-permissions" data-popover data-mode={thread.approval_mode ?? "custom"}>
            <summary onMouseDown={event => event.preventDefault()} aria-label={`Permissions: ${thread.approval_mode === "yolo" ? "YOLO" : "Custom"}`} title={thread.approval_mode === "yolo" ? "YOLO: no approval prompts; the sandbox still applies" : "Custom applies your rules and asks for tool approvals"}>
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true"><path d="M12 3 4 6.5V12c0 4.5 8 9 8 9s8-4.5 8-9V6.5Z" /></svg>
              <span className="themis-composer-label">{permissionSaving ? "Saving…" : thread.approval_mode === "yolo" ? "YOLO" : "Custom"}</span>
            </summary>
            <div className="themis-permissions-panel"><p>{thread.approval_mode === "yolo" ? "No approval prompts; the sandbox still applies." : "Apply your rules and ask for tool approvals."} Changes apply to the next run.</p>
              <label><select aria-label="Approval mode" onKeyDown={event => { if (!event.nativeEvent.isComposing && isModeInitial(event.nativeEvent)) event.preventDefault(); }} value={thread.approval_mode ?? "custom"} disabled={permissionSaving} onChange={event => onApprovalModeChange?.(event.target.value as "custom" | "yolo")}><option value="yolo">YOLO</option><option value="custom">Custom</option></select></label>
            </div>
          </details>

          <span className="themis-composer-spacer" />
          <ContextMeter budget={budget} compacting={compacting} running={running} />
          {running && (
            <Button
              className="themis-send"
              variant="ghost"
              aria-label={stopping ? "Stopping" : "Stop"}
              title={stopping ? "Stopping…" : "Stop this turn (Esc twice)"}
              disabled={stopping}
              onClick={onStop}
            >
              <svg width="14" height="14" viewBox="0 0 16 16" aria-hidden="true">
                <rect x="3" y="3" width="10" height="10" rx="2" fill="currentColor" />
              </svg>
            </Button>
          )}
          {(!running || draft.trim() !== "" || attachments.length > 0) && (
            <Button
              className="themis-send"
              variant="primary"
              aria-label={running ? "Queue message" : "Send"}
              title={running ? "Queue (Enter) · sent when this turn ends, or steer it in" : "Send (Enter) · Shift+Enter for a new line · ⌘/Ctrl+Shift+O new thread"}
              disabled={composerDisabled || (draft.trim() === "" && attachments.length === 0)}
              onClick={onSend}
            >
              <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true">
                <path d="M12 19V5M5 12l7-7 7 7" />
              </svg>
            </Button>
          )}
        </div>
        {location && <div className="themis-composer-location" title={location.branch ? `${location.name} · ${location.branch}\n${location.root}` : `${location.name}\n${location.root}`} aria-label={location.branch ? `${location.name}, branch ${location.branch}` : location.name}>
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z" /></svg>
            <span className="themis-composer-label">{location.name}</span>
            {location.branch && <><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" aria-hidden="true"><circle cx="6" cy="5" r="2" /><circle cx="6" cy="19" r="2" /><circle cx="18" cy="7" r="2" /><path d="M6 7v10M18 9c0 5-6 4-11 8" /></svg><span className="themis-composer-branch themis-composer-label">{location.branch}</span></>}
          </div>}
      </div>
    </>
  );
}
