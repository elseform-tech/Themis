import { useEffect, useState } from "react";
import { Badge } from "../components";
import { getEffectivePermissions } from "../lib/tauri";
import type { EffectivePermissions, EffectiveRule, RuleSource } from "../lib/types";
import { describeError } from "../state/store";

const SOURCE: Record<RuleSource, string> = {
  default: "Built-in",
  user: "User file",
  project: "Project file",
  run: "Run override",
};

const ACTION: Record<string, string> = { allow: "Allow", ask: "Ask", deny: "Deny" };

/** A path or other file text people may want to copy: monospace, selectable, never an action. */
function Path({ value }: { value: string }) {
  return <code className="themis-permissions-path">{value}</code>;
}

function RuleRow({ rule, index }: { rule: EffectiveRule; index: number }) {
  return (
    <li className="themis-permissions-rule">
      <span className="themis-permissions-rule-text">
        <span className="themis-permissions-index">{index + 1}.</span> {ACTION[rule.action] ?? rule.action}{" "}
        <code>{rule.tool === "*" ? "any tool" : rule.tool}</code>
        {rule.risk && <> · {rule.risk}</>}
        {rule.command && rule.command.length > 0 && <> · <code>{rule.command.join(" ")}</code></>}
      </span>
      <span className="themis-permissions-source">{SOURCE[rule.source] ?? rule.source}</span>
    </li>
  );
}

/** Pure read-only rendering of `get_effective_permissions`. No controls change anything here. */
export function PermissionsSummary({ view }: { view: EffectivePermissions }) {
  const approval = view.approval;
  const sandbox = view.sandbox;
  const projectRules = approval?.rules.filter(rule => rule.source === "project") ?? [];
  const otherRules = approval?.rules.filter(rule => rule.source !== "project") ?? [];
  const otherBackends = sandbox?.backends.filter(backend => backend.label !== sandbox.backend) ?? [];
  return (
    <>
      <section className="themis-settings-section" aria-label="Permission mode">
        <h3 className="themis-settings-subtitle">Mode</h3>
        <dl className="themis-permissions">
          <dt>Custom{view.mode === "custom" && <Badge tone="info">this chat</Badge>}</dt>
          <dd>The rules below decide; anything they do not decide asks you.</dd>
          <dt>YOLO{view.mode === "yolo" && <Badge tone="warning">this chat</Badge>}</dt>
          <dd>No approval prompts; the sandbox still applies.</dd>
          <dt>When changes apply</dt>
          <dd>Each run captures its mode and configuration at startup. Configuration lives only in the files below.</dd>
        </dl>
      </section>

      {view.validation_error && <p role="alert" className="themis-form-error">Configuration is invalid, so runs are refused until it is fixed: {view.validation_error}</p>}

      {approval && <section className="themis-settings-section" aria-label="Rules">
        <h3 className="themis-settings-subtitle">Rules, first match wins</h3>
        {projectRules.length > 0 && <>
          <p className="themis-settings-hint">+{projectRules.length} deny {projectRules.length === 1 ? "rule" : "rules"} from this project</p>
          <ol className="themis-permissions-rules">{projectRules.map((rule, index) => <RuleRow key={`p${index}`} rule={rule} index={index} />)}</ol>
        </>}
        <ol className="themis-permissions-rules" start={projectRules.length + 1}>
          {otherRules.map((rule, index) => <RuleRow key={`r${index}`} rule={rule} index={projectRules.length + index} />)}
          <li className="themis-permissions-rule">
            <span className="themis-permissions-rule-text">Otherwise {(ACTION[approval.default] ?? approval.default).toLowerCase()}</span>
            <span className="themis-permissions-source">{SOURCE[approval.default_source] ?? approval.default_source}</span>
          </li>
        </ol>
      </section>}

      {sandbox && <section className="themis-settings-section" aria-label="Sandbox">
        <h3 className="themis-settings-subtitle">Sandbox</h3>
        <dl className="themis-permissions">
          <dt>Status <Badge tone={sandbox.state === "on" ? "success" : sandbox.state === "blocked" ? "danger" : "warning"}>{sandbox.state === "on" ? "On" : sandbox.state === "off" ? "Off" : sandbox.state === "blocked" ? "Commands blocked" : "Not available"}</Badge></dt>
          <dd>{sandbox.summary}{sandbox.state === "blocked" && <> · {sandbox.reason}</>}</dd>
          <dt>Network</dt>
          <dd>{sandbox.network === "block" ? "Blocked" : "Allowed"}{sandbox.network_note && <> ({sandbox.network_note})</>}</dd>
          <dt>Writable</dt>
          <dd><ul className="themis-permissions-paths">{sandbox.writable_paths.map(path => <li key={path}><Path value={path} /></li>)}</ul></dd>
          <dt>Never writable</dt>
          <dd><ul className="themis-permissions-paths">{sandbox.protected_paths.map(path => <li key={path}><Path value={path} /></li>)}</ul></dd>
          <dt>Never readable</dt>
          <dd><details><summary>{sandbox.secret_paths.length} secret locations</summary><ul className="themis-permissions-paths">{sandbox.secret_paths.map(path => <li key={path}><Path value={path} /></li>)}</ul></details></dd>
          <dt>Applies to</dt>
          <dd>{sandbox.covers.join(", ")}. Not to {sandbox.not_covered.join(" or ")}.</dd>
          {otherBackends.length > 0 && <>
            <dt>Other backends</dt>
            <dd><ul className="themis-permissions-paths">{otherBackends.map(backend => <li key={backend.id}>{backend.label}: {backend.available ? "available" : backend.supported_here ? `not available (${backend.detail})` : `not available on this system (${backend.os} only)`}</li>)}</ul></dd>
          </>}
        </dl>
      </section>}

      <section className="themis-settings-section" aria-label="Configuration files">
        <h3 className="themis-settings-subtitle">Configuration files</h3>
        <ul className="themis-permissions-files">
          {view.files.map(file => <li key={file.scope}>
            <span className="themis-permissions-file-head">{file.scope === "user" ? "User" : "Project"} <Badge tone={file.status === "loaded" ? "success" : file.status === "missing" ? "default" : "danger"}>{file.status === "loaded" ? "Loaded" : file.status === "missing" ? "Not created" : file.status === "invalid" ? "Invalid" : "Unreadable"}</Badge></span>
            <Path value={file.path} />
            {file.error && <span role="alert" className="themis-permissions-error">{file.error.line ? `Line ${file.error.line}${file.error.column ? `, column ${file.error.column}` : ""}: ` : ""}{file.error.message}</span>}
          </li>)}
        </ul>
      </section>

      {view.chat && <section className="themis-settings-section" aria-label="Allowed in this chat">
        <h3 className="themis-settings-subtitle">Allowed in this chat</h3>
        {view.chat.grants.length === 0
          ? <p className="themis-settings-hint">Nothing yet.</p>
          : <ul className="themis-permissions-paths">{view.chat.grants.map((grant, index) => <li key={index}>
              {grant.server ? <>Every tool of MCP server <code>{grant.server}</code></> : grant.command && grant.command.length > 0 ? <><code>{grant.command.join(" ")}</code>{grant.tool === "shell_unsandboxed" ? " outside the sandbox" : grant.tool === "git" ? " (git tool)" : ""}</> : <code>{grant.tool}</code>} · {grant.risk}
            </li>)}</ul>}
        {view.chat.warning && <p role="alert" className="themis-permissions-error">The grant file was ignored: {view.chat.warning}</p>}
        {view.chat.grants_path && <Path value={view.chat.grants_path} />}
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
