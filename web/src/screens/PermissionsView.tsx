import { useEffect, useState } from "react";
import { getEffectivePermissions } from "../lib/tauri";
import type { EffectivePermissions, EffectiveRule, RuleSource } from "../lib/types";
import { describeError } from "../state/store";

const SOURCE: Record<RuleSource, string> = {
  default: "Built-in",
  user: "User",
  project: "Project",
  run: "Run",
};

const ACTION: Record<string, string> = { allow: "Allow", ask: "Ask", deny: "Deny" };

const SANDBOX_STATE: Record<string, string> = { off: "Off", unavailable: "Not available", blocked: "Commands blocked" };

/** A path or other file text people may want to copy: monospace, selectable, never an action. */
function Path({ value }: { value: string }) {
  return <code className="themis-permissions-path">{value}</code>;
}

function RuleRow({ rule }: { rule: EffectiveRule }) {
  return (
    <li className="themis-permissions-rule">
      <span className="themis-permissions-rule-text">
        {ACTION[rule.action] ?? rule.action} {rule.tool === "*" ? "any tool" : <code>{rule.tool}</code>}
        {rule.risk && <> · {rule.risk}</>}
        {rule.command && rule.command.length > 0 && <> · <code>{rule.command.join(" ")}</code></>}
      </span>
      <span className="themis-permissions-source">{SOURCE[rule.source] ?? rule.source}</span>
    </li>
  );
}

/**
 * Pure read-only rendering of `get_effective_permissions`. It shows only what
 * tells the reader something about this chat: details that never vary (what
 * the sandbox covers, backends of other systems, temp folders) live in the docs.
 */
export function PermissionsSummary({ view }: { view: EffectivePermissions }) {
  const approval = view.approval;
  const sandbox = view.sandbox;
  const yolo = view.mode === "yolo";
  const fileError = view.files.some(file => file.error);
  const grants = view.chat?.grants ?? [];
  return (
    <>
      <section className="themis-settings-section" aria-label="Permission mode">
        <h3 className="themis-settings-subtitle">Mode</h3>
        <p className="themis-permissions-line"><strong>{yolo ? "YOLO" : "Custom"}</strong>{view.chat && " in this chat"} · {yolo ? "No approval prompts; the sandbox still applies." : "The rules below decide; anything else asks you."}</p>
      </section>

      {view.validation_error && !fileError && <p role="alert" className="themis-form-error">Runs are refused until the configuration is fixed: {view.validation_error.replace(/^runtime configuration: (invalid runtime configuration: )?/, "")}</p>}

      {approval && <section className="themis-settings-section" aria-label="Rules">
        <h3 className="themis-settings-subtitle">Rules, first match wins</h3>
        <ol className="themis-permissions-rules">
          {approval.rules.map((rule, index) => <RuleRow key={index} rule={rule} />)}
          <li className="themis-permissions-rule">
            <span className="themis-permissions-rule-text">Otherwise {(ACTION[approval.default] ?? approval.default).toLowerCase()}</span>
            <span className="themis-permissions-source">{SOURCE[approval.default_source] ?? approval.default_source}</span>
          </li>
        </ol>
      </section>}

      {sandbox && <section className="themis-settings-section" aria-label="Sandbox">
        <h3 className="themis-settings-subtitle">Sandbox</h3>
        <dl className="themis-permissions">
          <dt>Status</dt>
          <dd>{sandbox.state === "on" ? "On" : SANDBOX_STATE[sandbox.state] ?? sandbox.state} · {sandbox.summary}{sandbox.state === "blocked" && <> · {sandbox.reason}</>}</dd>
          {sandbox.state === "on" && <>
            <dt>Network</dt>
            <dd>{sandbox.network === "block" ? "Blocked" : "Allowed"}{sandbox.network_note && <> ({sandbox.network_note})</>}</dd>
            <dt>Can write</dt>
            <dd>This project and temporary folders{sandbox.configured_writable_paths.length > 0 && <ul className="themis-permissions-paths">{sandbox.configured_writable_paths.map(path => <li key={path}><Path value={path} /></li>)}</ul>}</dd>
            <dt>Protected</dt>
            <dd>Themis settings and this project's <code>.themis</code> folder are never writable. <details><summary>{sandbox.secret_paths.length} secret folders are never readable</summary><ul className="themis-permissions-paths">{sandbox.secret_paths.map(path => <li key={path}><Path value={path} /></li>)}</ul></details></dd>
          </>}
        </dl>
      </section>}

      <section className="themis-settings-section" aria-label="Configuration files">
        <h3 className="themis-settings-subtitle">Configuration files</h3>
        <p className="themis-settings-hint">Edit these files to change permissions; changes apply from the next run.</p>
        <ul className="themis-permissions-files">
          {view.files.map(file => <li key={file.scope}>
            <span className="themis-permissions-file-head">{file.scope === "user" ? "User" : "Project"}{file.status === "missing" && <span className="themis-permissions-source">Not created</span>}</span>
            <Path value={file.path} />
            {file.error && <span role="alert" className="themis-permissions-error">{file.status === "invalid" ? "Invalid, runs are refused. " : ""}{file.error.line ? `Line ${file.error.line}${file.error.column ? `, column ${file.error.column}` : ""}: ` : ""}{file.error.message}</span>}
          </li>)}
        </ul>
      </section>

      {(grants.length > 0 || view.chat?.warning) && <section className="themis-settings-section" aria-label="Allowed in this chat">
        <h3 className="themis-settings-subtitle">Allowed in this chat</h3>
        {grants.length > 0 && <ul className="themis-permissions-paths">{grants.map((grant, index) => <li key={index}>
          {grant.server ? <>Every tool of MCP server <code>{grant.server}</code></> : grant.command && grant.command.length > 0 ? <><code>{grant.command.join(" ")}</code>{grant.tool === "shell_unsandboxed" ? " outside the sandbox" : grant.tool === "git" ? " (git tool)" : ""}</> : <code>{grant.tool}</code>}
        </li>)}</ul>}
        {view.chat?.warning && <p role="alert" className="themis-permissions-error">The grant file was ignored: {view.chat.warning}</p>}
        {view.chat?.grants_path && <><p className="themis-settings-hint">To undo, edit this file or start a new chat:</p><Path value={view.chat.grants_path} /></>}
      </section>}
    </>
  );
}

/** Settings → Permissions: re-reads the effective view each time the tab opens. */
export function PermissionsSettings({ projectRoot, threadId }: { projectRoot: string | null; threadId: string | null }) {
  const [view, setView] = useState<EffectivePermissions | null>(null);
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true;
    setView(null); setError("");
    void getEffectivePermissions(projectRoot, threadId)
      .then(next => { if (active) setView(next); })
      .catch((failure: unknown) => { if (active) setError(describeError(failure)); });
    return () => { active = false; };
  }, [projectRoot, threadId]);
  if (error) return <p role="alert" className="themis-form-error">Permissions could not be read: {error}</p>;
  if (!view) return <p role="status" className="themis-settings-hint">Reading permissions…</p>;
  return <PermissionsSummary view={view} />;
}
