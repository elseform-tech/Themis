import { useCallback, useEffect, useState } from "react";
import { Button, Dialog } from "../components";
import { pluginAction } from "../lib/tauri";

export interface SourceRow {
  name: string;
  source: string;
  enabled: boolean;
  official: boolean;
  status: string;
  completed: number;
  total: number;
  checked_at?: number | null;
  error?: string | null;
}
interface SourcePreview { name: string; publisher: string; plugins: number; source: string }
type View = { kind: "list" } | { kind: "add" } | { kind: "remove"; source: SourceRow };

const plain = (error: unknown) => (error instanceof Error ? error.message : String(error)).split("\nDiagnostic operation:")[0];
export const sourceLabel = (name: string) => name === "claude-plugins-official" ? "Official" : name;

function checkedAgo(at: number) {
  const minutes = Math.floor((Date.now() - at) / 60000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  return hours < 24 ? `${hours} h ago` : `${Math.floor(hours / 24)} d ago`;
}
function sourceMeta(source: SourceRow) {
  if (source.status === "scanning") return source.total ? `Checking ${source.completed}/${source.total}` : "Checking…";
  if (source.status === "failed") return "Check failed";
  if (source.status !== "ready") return "Not checked yet";
  return `${source.total} plugin${source.total === 1 ? "" : "s"}${source.checked_at ? ` · Checked ${checkedAgo(source.checked_at)}` : ""}`;
}

/** Discover › Sources (board 22): every marketplace with its state, adding one after reading its
 *  manifest, hiding one from Discover, and removing one. Installed plugins stay. */
export function SourcesDialog({ open, disabled, run, onChanged, onClose }: {
  open: boolean;
  disabled: boolean;
  run: (args: Record<string, unknown>) => Promise<void>;
  onChanged: (added?: string) => void;
  onClose: () => void;
}) {
  const [view, setView] = useState<View>({ kind: "list" });
  const [sources, setSources] = useState<SourceRow[]>([]);
  const [input, setInput] = useState(""), [preview, setPreview] = useState<SourcePreview | null>(null);
  const [problem, setProblem] = useState(""), [checking, setChecking] = useState(false);
  const refresh = useCallback(() => pluginAction<SourceRow[]>({ action: "sources" }).then(setSources).catch(() => {}), []);
  const scanning = sources.some(source => source.status === "scanning");
  useEffect(() => {
    if (!open) return;
    setView({ kind: "list" });
    void refresh();
  }, [open, refresh]);
  useEffect(() => {
    if (!open || !scanning) return;
    const timer = setInterval(() => void refresh(), 2000);
    return () => clearInterval(timer);
  }, [open, scanning, refresh]);
  const startAdd = () => { setInput(""); setPreview(null); setProblem(""); setView({ kind: "add" }); };
  async function submit() {
    if (checking || disabled || !input.trim()) return;
    if (!preview) {
      setChecking(true); setProblem("");
      try { setPreview(await pluginAction<SourcePreview>({ action: "preview_source", source: input.trim() })); }
      catch (error) { setProblem(plain(error)); }
      finally { setChecking(false); }
      return;
    }
    try {
      await run({ action: "add_marketplace", source: preview.source });
      onChanged(preview.name);
      await refresh();
      setView({ kind: "list" });
    } catch { /* The error toast is shown; the source stays here to correct. */ }
  }
  async function toggle(source: SourceRow) {
    try { await run({ action: "set_marketplace_enabled", name: source.name, enabled: !source.enabled }); onChanged(); await refresh(); } catch { /* toast shown */ }
  }
  async function remove(source: SourceRow) {
    try { await run({ action: "remove_marketplace", name: source.name }); onChanged(); await refresh(); setView({ kind: "list" }); } catch { /* toast shown */ }
  }
  if (view.kind === "add") return <Dialog open={open} title="Add marketplace" onClose={() => { if (!checking) setView({ kind: "list" }); }}>
    <form className="themis-marketplace-form" onSubmit={event => { event.preventDefault(); void submit(); }}>
      <label>Source<input value={input} aria-invalid={!!problem} aria-describedby={problem ? "themis-source-problem" : undefined} placeholder="owner/repo, HTTPS URL or local folder" onChange={event => { setInput(event.target.value); setPreview(null); setProblem(""); }} disabled={checking} autoFocus /></label>
      {problem && <p id="themis-source-problem" className="themis-source-problem" role="alert">{problem}</p>}
      {preview && <>
        <div className="themis-source-preview"><span className="themis-source-tile" aria-hidden="true">{preview.name.charAt(0).toUpperCase()}</span><span><strong>{preview.name}</strong><span>{[preview.publisher, `${preview.plugins} plugin${preview.plugins === 1 ? "" : "s"}`].filter(Boolean).join(" · ")}</span></span></div>
        <p className="themis-source-trust">Plugins from this source can run commands once enabled.</p>
      </>}
      <div><Button type="button" variant="ghost" disabled={checking} onClick={() => setView({ kind: "list" })}>Cancel</Button><Button type="submit" disabled={checking || disabled || !input.trim() || !!problem}>{checking ? "Checking…" : preview ? "Add" : "Check"}</Button></div>
    </form>
  </Dialog>;
  if (view.kind === "remove") return <Dialog open={open} title={`Remove ${sourceLabel(view.source.name)}?`} onClose={() => setView({ kind: "list" })}>
    <p>Plugins installed from it stay installed, but can no longer be updated.</p>
    <Button variant="ghost" disabled={disabled} onClick={() => setView({ kind: "list" })}>Cancel</Button>
    <Button variant="danger" disabled={disabled} onClick={() => void remove(view.source)}>Remove</Button>
  </Dialog>;
  return <Dialog open={open} title="Sources" onClose={onClose}>
    <ul className="themis-sources">{sources.map(source => <li key={source.name}>
      <span className="themis-source-tile" aria-hidden="true">{sourceLabel(source.name).charAt(0).toUpperCase()}</span>
      <span className="themis-source-text"><strong>{sourceLabel(source.name)}</strong><span className={source.status === "failed" ? "is-failed" : undefined} title={source.error ?? undefined}>{sourceMeta(source)}</span></span>
      <button type="button" className="themis-integration-toggle" role="switch" aria-label={`Show ${sourceLabel(source.name)} in Discover`} aria-checked={source.enabled} disabled={disabled} onClick={() => void toggle(source)}><span className="themis-toggle-track"><span /></span></button>
      {!source.official && <button type="button" className="themis-source-remove" aria-label={`Remove ${source.name}`} title="Remove source" disabled={disabled} onClick={() => setView({ kind: "remove", source })}><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" aria-hidden="true"><path d="M5 7h14M10 7V4h4v3M7 7l1 13h8l1-13" /></svg></button>}
    </li>)}</ul>
    <div className="themis-sources-actions"><Button variant="ghost" onClick={onClose}>Close</Button><Button onClick={startAdd}>Add marketplace</Button></div>
  </Dialog>;
}
