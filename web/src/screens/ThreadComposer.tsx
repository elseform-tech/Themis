import { AttachmentPreview } from "./AttachmentPreview";
import { PromptInput } from "../components/PromptInput";
import { useEffect, useRef } from "react";
import type { Attachment } from "../lib/tauri";
import type { PromptPlugin, PromptSkill } from "../lib/prompt";
import { Button } from "../components/primitives/Button";
import type { GoModel, ThreadInfo } from "../lib/types";
// Official OpenCode theme assets: https://github.com/anomalyco/opencode/tree/dev/packages/console/app/src/asset
import opencodeLogoDark from "../assets/opencode-logo-dark.svg";
import opencodeLogoLight from "../assets/opencode-logo-light.svg";

interface ThreadComposerProps {
  thread: ThreadInfo;
  draft: string;
  attachments?: Attachment[];
  onAttach?: () => void;
  onRemoveAttachment?: (path: string) => void;
  skills?: PromptSkill[];
  plugins?: PromptPlugin[];
  history?: string[];
  models: GoModel[];
  effortLevels: string[];
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
  permissionSaving?: boolean;
  onApprovalModeChange?: (mode: "custom" | "yolo") => void;
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
  models,
  effortLevels,
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
  onProviderChange,
  onEffortChange,
  onOpenSettings,
  permissionSaving = false,
  onApprovalModeChange,
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
  const choices = ["", ...effortLevels];
  const effortIndex = Math.max(0, choices.indexOf(effort));
  return (
    <>
      {sendError && <p className="themis-thread-senderror" role="alert">{sendError}</p>}
      <div className="themis-thread-composer">
        {!secretConfigured && (
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
          placeholder={running ? "Run in progress…" : readOnly ? "Start a new thread to continue" : "Message Themis… @ for plugins, / for skills"}
          onChange={onDraftChange} onSubmit={onSend} />
        <div className="themis-thread-composer-actions">
          <Button variant="ghost" size="small" aria-label="Attach files" title="Attach files" disabled={composerDisabled} onClick={onAttach}>
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true"><path d="m8 13 7-7a3 3 0 0 1 4 4L9 20a5 5 0 0 1-7-7L13 2" /></svg>
          </Button>
          <details className="themis-composer-permissions" data-mode={thread.approval_mode ?? "custom"}>
            <summary aria-label={`Permissions: ${thread.approval_mode === "yolo" ? "YOLO" : "Custom"}`} title={thread.approval_mode === "yolo" ? "YOLO bypasses harness tool restrictions for the next run" : "Custom applies your rules and asks for tool approvals"}>
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true"><path d="M12 3 3 7v5c0 5 9 9 9 9s9-4 9-9V7Z" /></svg>
              <span>{permissionSaving ? "Saving…" : thread.approval_mode === "yolo" ? "YOLO" : "Custom"}</span>
            </summary>
            <div className="themis-permissions-panel"><p>{thread.approval_mode === "yolo" ? "Bypass harness tool restrictions." : "Apply your rules and ask for tool approvals."} Changes apply to the next run.</p>
              <label><select aria-label="Approval mode" value={thread.approval_mode ?? "custom"} disabled={permissionSaving} onChange={event => onApprovalModeChange?.(event.target.value as "custom" | "yolo")}><option value="yolo">YOLO</option><option value="custom">Custom</option></select></label>
            </div>
          </details>
          <div className="themis-composer-selectors">
            <div className="themis-composer-model-picker">
              <span className="themis-composer-model-provider" aria-label="OpenCode Go">
                <img className="themis-composer-logo-dark" src={opencodeLogoDark} alt="" />
                <img className="themis-composer-logo-light" src={opencodeLogoLight} alt="" />
                <span>Go</span>
              </span>
              <select
                aria-label="Model"
                title="Model"
                value={thread.model}
                disabled={composerBusy || switching}
                onChange={event => onProviderChange(event.target.value)}
              >
                {[...new Set([thread.model, ...models.map(model => model.id)])].map(model => (
                  <option key={model} value={model}>{model || "Choose a Go model"}</option>
                ))}
              </select>
            </div>
            <details className="themis-composer-effort">
              <summary className="themis-composer-effort-trigger" aria-label={`Reasoning effort: ${effort || "Default"}`} title="Reasoning effort">
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                  <path d="M12 5a3 3 0 0 0-5.6-1.5A4 4 0 0 0 3 10a4 4 0 0 0 1 7 3 3 0 0 0 5 3l3-2V5ZM12 5a3 3 0 0 1 5.6-1.5A4 4 0 0 1 21 10a4 4 0 0 1-1 7 3 3 0 0 1-5 3l-3-2V5Z" />
                  <path d="M8 8c0 2-2 2-2 4m2 3c2 0 3 1 3 3m5-10c0 2 2 2 2 4m-2 3c-2 0-3 1-3 3" />
                </svg>
                <span>{effort || "Default"}</span>
              </summary>
              <div className="themis-composer-effort-panel">
                <label htmlFor="themis-effort">Effort <strong>{effort || "Default"}</strong></label>
                <input id="themis-effort" aria-label="Reasoning effort" type="range" min="0" max={Math.max(1, effortLevels.length)} step="1"
                  value={effortIndex} disabled={composerBusy || effortLevels.length === 0}
                  aria-valuetext={effort || "Default"}
                  onChange={event => onEffortChange(choices[Number(event.target.value)] ?? "")} />
                <div className="themis-composer-effort-ends"><span>Default</span><span>{effortLevels[effortLevels.length - 1] || "No catalog levels"}</span></div>
              </div>
            </details>
          </div>
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
              title="Send (Enter)"
              disabled={composerDisabled || (draft.trim() === "" && attachments.length === 0)}
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
