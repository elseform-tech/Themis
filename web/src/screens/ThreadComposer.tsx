import { AttachmentPreview } from "./AttachmentPreview";
import { PromptInput, type PromptCommand } from "../components/PromptInput";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { Attachment } from "../lib/tauri";
import type { PromptPlugin, PromptSkill } from "../lib/prompt";
import { Button } from "../components/primitives/Button";
import type { ContextBudget, ThreadInfo } from "../lib/types";
import { isModeInitial } from "../lib/keyboard";
import { looksLikeSecret } from "../lib/secretLook";
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
  /** The secret OpenCode Go rejected: sending waits until it is fixed in Settings. */
  keyRejected?: string;
  onDraftChange: (value: string) => void;
  onSend: () => void;
  onStop: () => void;
  onNewThread: () => void;
  onOpenSettings: () => void;
  permissionSaving?: boolean;
  onApprovalModeChange?: (mode: "custom" | "yolo") => void;
  /** Blocks the configuration wrote that YOLO keeps (network, Keychain, deny rules). */
  yoloKeeps?: string[];
  budget?: ContextBudget;
  /** Project directory and current Git branch shown beside the composer controls. */
  location?: { name: string; root: string; branch: string | null; branches?: string[]; onSwitchBranch?: (branch: string) => void };
  compacting?: boolean;
  /** Centred, larger composer for an empty conversation. */
  hero?: boolean;
  /** Shown on top of the composer: the chat's goal bar and queued messages. */
  top?: ReactNode;
}

/** How long after a first Esc a second one stops the run (and the hint shows). */
const ESC_STOP_WINDOW_MS = 1500;

/** An open menu, popover, panel or dialog that an Esc would close instead. */
function openLayer(): boolean {
  return document.querySelector('details[data-popover][open], [role="dialog"], [role="menu"], [role="listbox"]') !== null;
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
  keyRejected,
  onDraftChange,
  onSend,
  onStop,
  onOpenSettings,
  permissionSaving = false,
  onApprovalModeChange,
  yoloKeeps,
  budget,
  compacting = false,
  hero = false,
  location,
  top,
}: ThreadComposerProps) {
  const stopAction = useRef(onStop);
  stopAction.current = onStop;
  // Esc twice stops the run; after the first press a hint says so until the window closes.
  const [escArmed, setEscArmed] = useState(false);
  useEffect(() => {
    if (!running || stopping) { setEscArmed(false); return; }
    let firstEscape: number | null = null;
    let requested = false;
    let hint: ReturnType<typeof setTimeout> | undefined;
    const disarm = () => { firstEscape = null; clearTimeout(hint); setEscArmed(false); };
    function stopShortcut(event: KeyboardEvent) {
      if (event.isComposing || event.repeat || requested) return;
      if (event.key !== "Escape") { disarm(); return; }
      // An Esc that closes a menu, popover or panel is for that, never half of a stop.
      if (openLayer()) { disarm(); return; }
      const now = Date.now();
      if (firstEscape !== null && now - firstEscape <= ESC_STOP_WINDOW_MS) {
        event.preventDefault();
        requested = true;
        disarm();
        stopAction.current();
      } else {
        firstEscape = now;
        setEscArmed(true);
        clearTimeout(hint);
        hint = setTimeout(disarm, ESC_STOP_WINDOW_MS);
      }
    }
    window.addEventListener("keydown", stopShortcut, true);
    return () => { clearTimeout(hint); window.removeEventListener("keydown", stopShortcut, true); };
  }, [running, stopping, thread.id]);
  // A click on the box's padding or quiet areas focuses the text area; real controls keep their own behaviour,
  // and so do the approval and question boxes stacked on top (a click there focuses the box, so 1–3 decide).
  function focusEditorFromBox(event: React.MouseEvent<HTMLDivElement>) {
    if ((event.target as Element).closest("button, summary, a, select, input, textarea, details, label, ul, [role='textbox'], .themis-permissions-panel, .themis-composer-menu, .themis-approval-box, .themis-question")) return;
    event.currentTarget.querySelector<HTMLElement>(".themis-prompt-editor")?.focus();
  }
  function insertTrigger(trigger: "/" | "@", menu: HTMLElement | null) {
    menu?.closest("details")?.removeAttribute("open");
    onDraftChange(`${draft}${draft === "" || draft.endsWith(" ") || draft.endsWith("\n") ? "" : " "}${trigger}`);
  }
  // Dismissed until the text stops looking like a secret, so the next one speaks up again.
  const secretLook = useMemo(() => looksLikeSecret(draft), [draft]);
  const [secretDismissed, setSecretDismissed] = useState(false);
  if (secretDismissed && !secretLook) setSecretDismissed(false);
  // A rejected key would only fail the run: wait for it to be fixed (queueing still works).
  const sendBlocked = !running && Boolean(keyRejected);
  return (
    <>
      {sendError && <p className="themis-thread-senderror" role="alert">{sendError}</p>}
      <div onClick={focusEditorFromBox} className={`themis-thread-composer${hero ? " themis-thread-composer--hero" : ""}${running ? " is-running" : ""}`}>
        {running && <span className="themis-composer-ring" aria-hidden="true" />}
        {top}
        {!secretConfigured && !running && (
          <p className="themis-thread-hint">
            Connect OpenCode Go to send your first message. {" "}
            <Button variant="ghost" size="small" onClick={onOpenSettings}>Model settings</Button>
          </p>
        )}
        {secretConfigured && keyRejected && !running && (
          <p className="themis-thread-hint" role="alert">
            OpenCode Go rejected the key in {keyRejected}. {" "}
            <Button variant="ghost" size="small" onClick={onOpenSettings}>Model settings</Button>
          </p>
        )}
        {secretLook && !secretDismissed && (
          <p className="themis-thread-hint themis-composer-secret" role="status">
            <span>This looks like a secret. It&apos;d be sent to the model provider. If it&apos;s real, use Settings › Secrets, and rotate it if already shared.</span>
            <button type="button" aria-label="Dismiss" onClick={() => setSecretDismissed(true)}>×</button>
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
          onChange={onDraftChange} onSubmit={() => { if (!sendBlocked) onSend(); }} />
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
            <summary onMouseDown={event => event.preventDefault()} aria-label={`Permissions: ${thread.approval_mode === "yolo" ? "YOLO" : "Custom"}`} title={thread.approval_mode === "yolo" ? "YOLO: no approvals, no sandbox; only Themis settings are protected" : "Custom applies your rules and asks for tool approvals"}>
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true"><path d="M12 3 4 6.5V12c0 4.5 8 9 8 9s8-4.5 8-9V6.5Z" /></svg>
              <span className={`themis-composer-label${thread.approval_mode === "yolo" ? " is-yolo" : ""}`}>{permissionSaving ? "Saving…" : thread.approval_mode === "yolo" ? "YOLO" : "Custom"}</span>
            </summary>
            <div className="themis-permissions-panel"><p>{thread.approval_mode === "yolo" ? `No approvals, no sandbox: the agent can change anything on this computer. Only Themis settings are protected${yoloKeeps?.length ? `; still applies: ${yoloKeeps.join(", ")}` : ""}.` : "Apply your rules and ask for tool approvals."} Changes apply to the next run.</p>
              <label><select aria-label="Approval mode" onKeyDown={event => { if (!event.nativeEvent.isComposing && isModeInitial(event.nativeEvent)) event.preventDefault(); }} value={thread.approval_mode ?? "custom"} disabled={permissionSaving} onChange={event => onApprovalModeChange?.(event.target.value as "custom" | "yolo")}><option value="yolo">YOLO</option><option value="custom">Custom</option></select></label>
            </div>
          </details>

          <span className="themis-composer-spacer" />
          <ContextMeter budget={budget} compacting={compacting} running={running} threadId={thread.id} />
          {running && escArmed && !stopping && <span className="themis-esc-hint" role="status">Press Esc again to stop</span>}
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
              disabled={composerDisabled || sendBlocked || (draft.trim() === "" && attachments.length === 0)}
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
            {location.branch && <BranchPicker branch={location.branch} branches={location.branches ?? []} busy={running} onSwitch={location.onSwitchBranch} />}
          </div>}
      </div>
    </>
  );
}

const branchIcon = <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" aria-hidden="true"><circle cx="6" cy="5" r="2" /><circle cx="6" cy="19" r="2" /><circle cx="18" cy="7" r="2" /><path d="M6 7v10M18 9c0 5-6 4-11 8" /></svg>;
/** The checkout's branch, and the way to change it. Not while a chat is working: switching
 *  would move files underneath it. */
export function BranchPicker({ branch, branches, busy, onSwitch }: { branch: string; branches: string[]; busy: boolean; onSwitch?: (branch: string) => void }) {
  const others = branches.filter(name => name !== branch);
  if (busy || !onSwitch || others.length === 0) return <>{branchIcon}<span className="themis-composer-branch themis-composer-label">{branch}</span></>;
  return <details data-popover className="themis-branch-picker">
    <summary aria-label={`Branch ${branch}. Switch branch`} title="Switch branch">{branchIcon}<span className="themis-composer-branch themis-composer-label">{branch}</span><svg className="themis-branch-chevron" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true"><path d="m6 9 6 6 6-6" /></svg></summary>
    <div className="themis-branch-menu" role="menu" aria-label="Switch branch">
      {others.map(name => <button key={name} type="button" role="menuitem" onClick={event => { event.currentTarget.closest("details")?.removeAttribute("open"); onSwitch(name); }}>{name}</button>)}
    </div>
  </details>;
}
