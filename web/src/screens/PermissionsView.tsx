import { useEffect, useState } from "react";
import { getEffectivePermissions, listEditors, openRuntimeConfig } from "../lib/tauri";
import type { EffectivePermissions } from "../lib/types";
import { describeError } from "../state/store";
import { SettingsGroup } from "./SettingsGroup";

/**
 * Problems with the global configuration that change what runs do. The normal
 * case shows nothing; the full view is `themis permissions`.
 */
/** The parser's message without its list of every allowed field. */
const brief = (message: string) => message.replace(/, expected (one of )?`.*$/s, "");

export function permissionProblems(view: EffectivePermissions): string[] {
  const problems: string[] = [];
  const file = view.files.find(entry => entry.scope === "user");
  if (file?.error) {
    const where = file.error.line ? ` · line ${file.error.line}` : "";
    problems.push(`${file.status === "invalid" ? "Invalid" : "Unreadable"}${where}: ${brief(file.error.message)}. Runs are refused until it is fixed.`);
  } else if (view.validation_error) {
    problems.push(`Invalid: ${brief(view.validation_error.replace(/^runtime configuration: (invalid runtime configuration: )?/, ""))}. Runs are refused until it is fixed.`);
  }
  const sandbox = view.sandbox;
  if (sandbox?.state === "blocked") problems.push(`Sandbox not available: ${sandbox.reason}. Commands are refused.`);
  else if (sandbox?.state === "unavailable") problems.push("No sandbox on this system.");
  else if (sandbox?.state === "off") problems.push("Sandbox off.");
  return problems;
}

/** Whether the global file was read: `ok` (loaded), `off` (not created, defaults apply) or `bad`. */
export function fileState(view: EffectivePermissions): { tone: "ok" | "off" | "bad"; label: string } {
  const file = view.files.find(entry => entry.scope === "user");
  if (file?.status === "invalid" || (file?.status !== "unreadable" && view.validation_error)) {
    return { tone: "bad", label: file?.error?.line ? `Invalid · line ${file.error.line}` : "Invalid" };
  }
  if (file?.status === "unreadable") return { tone: "bad", label: "Unreadable" };
  if (file?.status === "missing") return { tone: "off", label: "Not created · defaults" };
  return { tone: "ok", label: "Loaded" };
}

/** Settings → Permissions: the global file, whether it was read, and which editor opens it. */
export function PermissionsSettings({ editor = "", onEditor }: { editor?: string; onEditor?: (editor: string) => void }) {
  const [view, setView] = useState<EffectivePermissions | null>(null);
  const [editors, setEditors] = useState<Array<{ id: string; name: string }>>([{ id: "", name: "Text editor" }]);
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true;
    void getEffectivePermissions(null, null)
      .then(next => { if (active) setView(next); })
      .catch((failure: unknown) => { if (active) setError(describeError(failure)); });
    void Promise.resolve().then(() => listEditors())
      .then(found => { if (active && found?.editors?.length) setEditors(found.editors); })
      .catch(() => { /* the system text editor stays available */ });
    return () => { active = false; };
  }, []);
  const chosen = editors.some(entry => entry.id === editor) ? editor : "";
  const open = () => { setError(""); void openRuntimeConfig(chosen || undefined).catch((failure: unknown) => setError(describeError(failure))); };
  const path = view?.files.find(entry => entry.scope === "user")?.path;
  const status = view && fileState(view);
  return (
    <SettingsGroup title="Configuration file">
      <div className="themis-settings-control">
        <span title={path}>runtime.jsonc</span>
        {status && <span className={`themis-settings-state is-${status.tone}`}>{status.label}</span>}
      </div>
      <div className="themis-settings-control">
        <span>Open with</span>
        <span className="themis-browser-pick">
          {editors.length > 1
            ? <select aria-label="Open with" value={chosen} onChange={event => onEditor?.(event.target.value)}>
              {editors.map(entry => <option key={entry.id} value={entry.id}>{entry.name}</option>)}
            </select>
            : <span className="themis-settings-hint">{editors[0].name}</span>}
          <button type="button" className="themis-text-action" onClick={open}>Open</button>
        </span>
      </div>
      {/* The status already names the file problem and its line. */}
      {view && permissionProblems(view).map(problem => <p key={problem} role="alert" className="themis-permissions-error">{problem.replace(/^(Invalid|Unreadable)( · line \d+)?: /, "")}</p>)}
      {error && <p role="alert" className="themis-form-error">{error}</p>}
    </SettingsGroup>
  );
}
