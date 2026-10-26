import { useEffect, useState } from "react";
import { getEffectivePermissions, listEditors, openRuntimeConfig } from "../lib/tauri";
import type { EffectivePermissions, EffectiveRule } from "../lib/types";
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
  // Landlock cannot keep YOLO commands out of a project's .themis/ (only the file
  // tools do); bubblewrap can, and Themis prefers it when it is installed.
  else if (sandbox?.state === "on" && sandbox.backend === "Landlock") problems.push("Install bubblewrap to also keep YOLO commands out of a project's .themis/ folder.");
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

const ACTION_LABEL = { allow: "Allow", ask: "Ask", deny: "Deny" } as const;
const SOURCE_LABEL: Record<string, string> = { default: "Built-in", user: "", project: "Project" };

/** One Custom rule: what it matches, read-only (rules are edited in runtime.jsonc). */
function RuleRow({ rule }: { rule: EffectiveRule }) {
  const when = rule.command?.length ? rule.command.join(" ") : rule.risk ? `${rule.risk} actions` : "";
  return <div className="themis-settings-control themis-permission-rule">
    <span className="themis-permission-rule-match"><code>{rule.tool}</code>{when && <small>{when}</small>}</span>
    <span className="themis-permission-rule-action">{SOURCE_LABEL[rule.source] && <small>{SOURCE_LABEL[rule.source]}</small>}<span className={`is-${rule.action}`}>{ACTION_LABEL[rule.action]}</span></span>
  </div>;
}

type ApprovalPrefs = { default_approval_mode?: "custom" | "yolo"; approval_timeout_seconds: number };

/** Settings → Permissions: new-chat mode and timeout, the Custom rules, and the global file. */
export function PermissionsSettings({ editor = "", onEditor, prefs, onPrefs }: { editor?: string; onEditor?: (editor: string) => void; prefs?: ApprovalPrefs; onPrefs?: (patch: Partial<ApprovalPrefs>) => void }) {
  const [timeout, setTimeoutDraft] = useState(prefs?.approval_timeout_seconds ?? 300);
  useEffect(() => { if (prefs) setTimeoutDraft(prefs.approval_timeout_seconds); }, [prefs]);
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
  const approval = view?.approval;
  return (<>
    {prefs && <SettingsGroup title="Approvals">
      <label className="themis-settings-control" title="Each chat can switch in its composer. YOLO asks nothing; the sandbox and Browser blocks still apply.">New chats start in
        <select aria-label="New chats start in" value={prefs.default_approval_mode ?? "custom"} onChange={event => onPrefs?.({ default_approval_mode: event.target.value as "custom" | "yolo" })}>
          <option value="custom">Custom</option><option value="yolo">YOLO</option>
        </select>
      </label>
      <label className="themis-settings-control" title="An unanswered approval is denied after this long">Approval timeout
        <span className="themis-browser-pick"><input className="themis-settings-number" aria-label="Approval timeout (seconds)" type="number" min={30} max={600} value={timeout} onChange={event => setTimeoutDraft(Number(event.target.value))} onBlur={() => { if (timeout !== prefs.approval_timeout_seconds) onPrefs?.({ approval_timeout_seconds: timeout }); }} /><small className="themis-settings-hint">seconds</small></span>
      </label>
    </SettingsGroup>}
    {approval && <SettingsGroup title="Custom rules" hint="The first rule that matches decides; edit them in runtime.jsonc with Open below.">
      {approval.rules.map((rule, index) => <RuleRow key={index} rule={rule} />)}
      <div className="themis-settings-control themis-permission-rule"><span className="themis-permission-rule-match"><span>Anything else</span></span><span className="themis-permission-rule-action"><span className={`is-${approval.default}`}>{ACTION_LABEL[approval.default]}</span></span></div>
    </SettingsGroup>}
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
  </>);
}
