import { useEffect, useState } from "react";
import { listEditors, mcpConfigStatus, openMcpConfig, updateSettings, type McpConfigStatus } from "../lib/tauri";
import { describeError, useApp } from "../state/store";

/** What the status says: only what changed or went wrong. */
function statusText(status: McpConfigStatus): string {
  switch (status.state) {
    case "open": return "Open · saves apply";
    case "applied": return status.changed?.length ? `Applied · ${status.changed.join(", ")}` : "Saved · no changes";
    case "invalid": return `Invalid${status.line ? ` · line ${status.line}` : ""}: ${status.message ?? ""}`;
    case "conflict": return status.message ?? "Changed elsewhere; open it again";
    case "error": return status.message ?? "Error";
    default: return "";
  }
}

/**
 * Integrations › Connections: your personal MCP servers as one mcp.json copy, opened in the chosen
 * editor and applied each time you save it (revision-checked; see docs/plugins.md).
 */
export function McpFileRow({ onApplied }: { onApplied: () => void }) {
  const { state, dispatch } = useApp();
  const [editors, setEditors] = useState<Array<{ id: string; name: string }>>([{ id: "", name: "Text editor" }]);
  const [status, setStatus] = useState<McpConfigStatus>({ state: "closed" });
  const [error, setError] = useState("");
  const editor = state.settings.config_editor ?? "";
  useEffect(() => {
    void Promise.resolve().then(() => listEditors()).then(found => { if (found?.editors?.length) setEditors(found.editors); }).catch(() => {});
    void Promise.resolve().then(() => mcpConfigStatus()).then(setStatus).catch(() => {});
  }, []);
  // While a copy is open, follow its saves.
  const watching = status.state !== "closed";
  const changedKey = status.state === "applied" ? (status.changed ?? []).join(",") : "";
  useEffect(() => {
    if (!watching) return;
    const timer = window.setInterval(() => { void mcpConfigStatus().then(setStatus).catch(() => {}); }, 1500);
    return () => window.clearInterval(timer);
  }, [watching]);
  useEffect(() => { if (changedKey) onApplied(); }, [changedKey, onApplied]);
  const chosen = editors.some(entry => entry.id === editor) ? editor : "";
  const open = async () => {
    setError("");
    try { setStatus(await openMcpConfig(chosen || undefined)); } catch (reason) { setError(describeError(reason)); }
  };
  const choose = async (id: string) => {
    try { dispatch({ type: "settings/patched", settings: await updateSettings({ config_editor: id }) }); } catch (reason) { setError(describeError(reason)); }
  };
  const tone = status.state === "invalid" || status.state === "conflict" || status.state === "error" ? "bad" : status.state === "applied" ? "ok" : "off";
  return <div className="themis-mcp-file">
    <span className="themis-mcp-file-label" title="Your personal MCP servers as one file, named integration/server. Each save is checked and applied; a changed command needs enabling again.">Edit as mcp.json</span>
    {status.state !== "closed" && <span role="status" className={`themis-settings-state is-${tone}`}>{statusText(status)}</span>}
    {error && <span role="alert" className="themis-settings-state is-bad">{error}</span>}
    <span className="themis-mcp-file-actions">
      {editors.length > 1 && <select aria-label="Open with" value={chosen} onChange={event => void choose(event.target.value)}>{editors.map(entry => <option key={entry.id} value={entry.id}>{entry.name}</option>)}</select>}
      <button type="button" onClick={() => void open()}>Open</button>
    </span>
  </div>;
}
