import { useEffect, useRef, useState } from "react";
import { Button } from "../components";
import {
  getRuntimeConfiguration,
  saveRuntimeConfiguration,
  queryDiagnosticLogs,
  type RuntimeConfigurationView,
  type DiagnosticRecord,
} from "../lib/tauri";
import { describeError } from "../state/store";

export function RuntimeConfiguration({ projectRoot }: { projectRoot: string | null }) {
  const activeRoot = useRef(projectRoot);
  activeRoot.current = projectRoot;
  const [view, setView] = useState<RuntimeConfigurationView | null>(null);
  const [scope, setScope] = useState<"user" | "project">("user");
  const [json, setJson] = useState("{}");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState("");

  useEffect(() => {
    let active = true;
    setView(null);
    setBusy(false);
    setError("");
    setSaved("");
    void getRuntimeConfiguration(projectRoot).then(value => {
      if (!active) return;
      setView(value);
      setError(value.validation_error ?? "");
      setJson(value.user_json ?? "{}");
      setScope("user");
    }).catch(error => {
      if (active) setError(describeError(error));
    });
    return () => { active = false; };
  }, [projectRoot]);

  async function save() {
    setBusy(true);
    setError("");
    setSaved("");
    try {
      const value = await saveRuntimeConfiguration(scope, json, projectRoot);
      if (projectRoot === activeRoot.current) {
        setView(value);
        setSaved("Saved · applies to subsequent runs");
      }
    } catch (error) {
      if (projectRoot === activeRoot.current) setError(describeError(error));
    } finally {
      if (projectRoot === activeRoot.current) setBusy(false);
    }
  }

  function changeScope(next: "user" | "project") {
    setScope(next);
    setJson((next === "user" ? view?.user_json : view?.project_json) ?? "{}");
    setSaved("");
  }

  return (
    <section className="themis-settings-section" aria-label="Runtime configuration">
      <h3>Runtime configuration</h3>
      <p className="themis-settings-hint">
        Defaults → user → project → run overrides. Active runs keep their configuration.
      </p>
      <label>
        Scope{" "}
        <select aria-label="Configuration scope" disabled={busy || !view} value={scope}
          onChange={event => changeScope(event.target.value as "user" | "project")}>
          <option value="user">User</option>
          <option value="project" disabled={!projectRoot}>Project</option>
        </select>
      </label>
      <p className="themis-settings-mono">{scope === "user" ? view?.user_path : view?.project_path}</p>
      <textarea aria-label="Configuration JSON" rows={14} spellCheck={false} value={json}
        disabled={!view || busy} onChange={event => { setJson(event.target.value); setSaved(""); }} />
      <Button disabled={!view || busy} onClick={() => void save()}>
        {busy ? "Saving…" : "Validate and save"}
      </Button>
      {error && <p role="alert">{error}</p>}
      {saved && <p role="status">{saved}</p>}
      <details>
        <summary>Effective configuration and sources</summary>
        <pre>{JSON.stringify({ config: view?.config, provenance: view?.provenance }, null, 2)}</pre>
      </details>
      <details>
        <summary>Configuration schema</summary>
        <pre>{JSON.stringify(view?.schema, null, 2)}</pre>
      </details>
    </section>
  );
}

export function DiagnosticLogs({ operationId = "" }: { operationId?: string }) {
  const [records, setRecords] = useState<DiagnosticRecord[]>([]);
  const [level, setLevel] = useState("");
  const [service, setService] = useState("");
  const [operation, setOperation] = useState(operationId);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);
  const jsonl = records.map(record => JSON.stringify(record)).join("\n");

  useEffect(() => {
    if (!operationId) return;
    let active = true;
    setBusy(true);
    void queryDiagnosticLogs({ operationId, limit: 200 }).then(value => {
      if (active) setRecords(value);
    }).catch(error => {
      if (active) setError(describeError(error));
    }).finally(() => {
      if (active) setBusy(false);
    });
    return () => { active = false; };
  }, [operationId]);

  async function refresh() {
    setBusy(true);
    setError("");
    setCopied(false);
    try {
      setRecords(await queryDiagnosticLogs({
        level: level || undefined,
        service: service || undefined,
        operationId: operation || undefined,
        limit: 200,
      }));
    } catch (error) {
      setError(describeError(error));
    } finally {
      setBusy(false);
    }
  }

  async function copy() {
    try {
      await navigator.clipboard.writeText(jsonl);
      setCopied(true);
    } catch (error) {
      setError(describeError(error));
    }
  }

  function download() {
    const url = URL.createObjectURL(new Blob([jsonl + "\n"], { type: "application/x-ndjson" }));
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = "themis-diagnostics.jsonl";
    anchor.click();
    URL.revokeObjectURL(url);
  }

  return (
    <section className="themis-settings-section" aria-label="Diagnostic logs">
      <h3>Diagnostic logs</h3>
      <div className="themis-log-filters">
        <label>Severity
          <select aria-label="Log severity" value={level} onChange={event => setLevel(event.target.value)}>
            <option value="">All levels</option>
            {["debug", "info", "warn", "error"].map(value => <option key={value}>{value}</option>)}
          </select>
        </label>
        <label>Service
          <input aria-label="Log service" value={service} onChange={event => setService(event.target.value)}
            placeholder="e.g. integration" />
        </label>
        <label>Operation
          <input aria-label="Log operation" value={operation} onChange={event => setOperation(event.target.value)} />
        </label>
      </div>
      <Button disabled={busy} onClick={() => void refresh()}>{busy ? "Loading…" : "Load logs"}</Button>
      <Button variant="ghost" disabled={!records.length} onClick={() => void copy()}>Copy logs</Button>
      <Button variant="ghost" disabled={!records.length} onClick={download}>Export JSONL</Button>
      {error && <p role="alert">{error}</p>}
      {copied && <p role="status">Copied</p>}
      <p className="themis-settings-hint">{records.length} records · up to 200 per query</p>
      <pre className="themis-diagnostic-records">{jsonl}</pre>
    </section>
  );
}
