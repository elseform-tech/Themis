import { useEffect, useState } from "react";
import { getEffectivePermissions, openRuntimeConfig } from "../lib/tauri";
import type { EffectivePermissions } from "../lib/types";
import { describeError } from "../state/store";

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

/** Settings → Permissions: the global file and any problem with it, re-read on every open. */
export function PermissionsSettings() {
  const [view, setView] = useState<EffectivePermissions | null>(null);
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true;
    void getEffectivePermissions(null, null)
      .then(next => { if (active) setView(next); })
      .catch((failure: unknown) => { if (active) setError(describeError(failure)); });
    return () => { active = false; };
  }, []);
  const open = () => { setError(""); void openRuntimeConfig().catch((failure: unknown) => setError(describeError(failure))); };
  const path = view?.files.find(entry => entry.scope === "user")?.path;
  return (
    <section className="themis-settings-section" aria-label="Permissions">
      <div className="themis-settings-control">
        <span title={path}>runtime.jsonc</span>
        <button type="button" className="themis-text-action" onClick={open}>Open</button>
      </div>
      {view && permissionProblems(view).map(problem => <p key={problem} role="alert" className="themis-permissions-error">{problem}</p>)}
      {error && <p role="alert" className="themis-form-error">{error}</p>}
    </section>
  );
}
