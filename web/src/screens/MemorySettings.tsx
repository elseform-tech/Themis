import { useEffect, useState } from "react";
import { Switch } from "../components";
import { deleteMemory, getMemorySettings, listMemories, updateMemorySettings } from "../lib/tauri";
import type { Memory, MemoryKind, MemoryList, MemoryNote, MemoryScope } from "../lib/types";
import { describeError, useActiveProject, useApp } from "../state/store";
import { SettingsGroup } from "./SettingsGroup";
import "./MemorySettings.css";

const KINDS: [MemoryKind, string][] = [["user", "About you"], ["feedback", "Your feedback"], ["project", "Project facts"], ["reference", "Pointers"]];

/** A memory's name as people read it: `free-model-for-tests` → "Free model for tests". */
export function memoryTitle(name: string): string {
  const words = name.replace(/-/g, " ");
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/** "today", or a short date such as "Oct 29" (with the year when it is not this year's). */
export function memoryDate(saved: string, now = new Date()): string {
  const [year, month, day] = saved.split("-").map(Number);
  if (!year || !month || !day) return saved;
  const date = new Date(year, month - 1, day);
  if (date.toDateString() === now.toDateString()) return "today";
  return date.toLocaleDateString(undefined, year === now.getFullYear() ? { month: "short", day: "numeric" } : { month: "short", day: "numeric", year: "numeric" });
}

/** "a web page" or "a file", from what the agent said the memory was built from. */
function sourceKind(source: string): string {
  return /^(web|https?:|page|site)/i.test(source.trim()) ? "a web page" : "a file";
}

/** The brain mark used by Settings › Memory and the chat's memory lines. */
export function MemoryIcon({ size = 14 }: { size?: number }) {
  return <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true" focusable="false"><path d="M9.5 2A2.5 2.5 0 0 1 12 4.5v15a2.5 2.5 0 0 1-4.96.44 2.5 2.5 0 0 1-2.96-3.08 3 3 0 0 1-.34-5.58 2.5 2.5 0 0 1 1.32-4.24A2.5 2.5 0 0 1 9.5 2Z" /><path d="M14.5 2A2.5 2.5 0 0 0 12 4.5v15a2.5 2.5 0 0 0 4.96.44 2.5 2.5 0 0 0 2.96-3.08 3 3 0 0 0 .34-5.58 2.5 2.5 0 0 0-1.32-4.24A2.5 2.5 0 0 0 14.5 2Z" /></svg>;
}

/** "Remembered · <name>" (or "Forgot · <name>") where the agent changed a memory; the
 *  name opens it in Settings › Memory. */
export function MemoryLine({ note }: { note: MemoryNote }) {
  const { dispatch } = useApp();
  const verb = note.action === "forgot" ? "Forgot" : "Remembered";
  const title = memoryTitle(note.name);
  return <div className="themis-memory-line" role="note">
    <MemoryIcon />
    <span>{verb}</span>
    {note.action === "forgot"
      ? <span title={note.description}>{title}</span>
      : <button type="button" title={note.description} aria-label={`${verb} ${title}: open in Settings`} onClick={() => dispatch({ type: "ui/view", view: "settings", settings: { section: "Memory", key: memoryKey(note) } })}>{title}</button>}
  </div>;
}

/** The Settings key that opens one memory: `<scope>:<name>`. */
export function memoryKey(note: Pick<MemoryNote, "scope" | "name">): string {
  return `${note.scope}:${note.name}`;
}

function MemoryRow({ memory, open, onToggle, onDelete }: { memory: Memory; open: boolean; onToggle: (open: boolean) => void; onDelete: () => Promise<void> }) {
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const text = memory.source ? `${memory.body}\n\nSource: ${memory.source} (information only)` : memory.body;
  return <details className="themis-memory" open={open} onToggle={event => { const now = (event.currentTarget as HTMLDetailsElement).open; if (now !== open) onToggle(now); if (!now) setConfirming(false); }}>
    <summary>
      <span className="themis-memory-text">
        <b>{memoryTitle(memory.name)}</b>
        {memory.source && <span className="themis-memory-from" title={memory.source}>from {sourceKind(memory.source)}</span>}
        <small>{memory.description}</small>
      </span>
      <span className="themis-memory-when">{memoryDate(memory.saved)}</span>
    </summary>
    <div className="themis-memory-body">
      <pre>{text}</pre>
      <div className="themis-memory-actions">
        {confirming ? <>
          <span className="themis-memory-question">Delete this memory? The agent won't see it again.</span>
          <button type="button" className="themis-text-action" disabled={busy} onClick={() => setConfirming(false)}>Cancel</button>
          <button type="button" className="themis-text-action themis-danger-action" disabled={busy} onClick={() => { setBusy(true); void onDelete().finally(() => setBusy(false)); }}>{busy ? "Deleting…" : "Delete"}</button>
        </> : <button type="button" className="themis-text-action themis-danger-action" onClick={() => setConfirming(true)}>Delete</button>}
      </div>
    </div>
  </details>;
}

/** Settings › Memory: what the agent remembers for this project or everywhere, read and
 *  delete, and one switch (`memory.enabled`, saved in place in the user runtime.jsonc). */
export function MemorySettingsSection({ focusKey, onSaved }: { focusKey?: string; onSaved?: () => void } = {}) {
  const { state } = useApp();
  const project = useActiveProject();
  const focusScope = focusKey?.split(":")[0] as MemoryScope | undefined;
  const [scope, setScope] = useState<MemoryScope>(focusScope === "global" ? "global" : "project");
  const [open, setOpen] = useState<string | null>(focusKey?.split(":").slice(1).join(":") ?? null);
  const [settings, setSettings] = useState<{ enabled: boolean; revision: string } | null>(null);
  const [list, setList] = useState<MemoryList | null>(null);
  const [error, setError] = useState("");
  const root = state.activeProjectRoot;
  const loadSettings = () => getMemorySettings().then(view => setSettings({ enabled: view.settings.enabled, revision: view.revision })).catch(reason => setError(describeError(reason)));
  const loadList = (which: MemoryScope) => {
    if (which === "project" && !root) { setList(null); return Promise.resolve(); }
    return listMemories(which, which === "project" ? root : null).then(setList).catch(reason => setError(describeError(reason)));
  };
  useEffect(() => { void loadSettings(); }, []);
  useEffect(() => { void loadList(scope); }, [scope, root]); // eslint-disable-line react-hooks/exhaustive-deps
  if (!settings) return error ? <p role="alert" className="themis-form-error">{error}</p> : null;

  const toggle = async (enabled: boolean) => {
    setError("");
    try {
      const view = await updateMemorySettings(enabled, settings.revision);
      setSettings({ enabled: view.settings.enabled, revision: view.revision });
      onSaved?.();
    } catch (reason) {
      setError(describeError(reason));
      void loadSettings();
    }
  };
  const remove = async (memory: Memory) => {
    if (!list) return;
    setError("");
    try {
      await deleteMemory(scope, scope === "project" ? root : null, memory.name, list.revision);
    } catch (reason) {
      setError(describeError(reason));
    }
    await loadList(scope);
  };
  const pick = (which: MemoryScope) => { if (which !== scope) { setScope(which); setOpen(null); setList(null); } };
  const memories = list?.memories ?? [];
  const projectLabel = project ? `${project.name} project` : "This project";

  return <>
    {error && <p role="alert" className="themis-form-error">{error}</p>}
    <SettingsGroup>
      <label className="themis-settings-check themis-memory-switch">
        <span className="themis-settings-name">Remember things across chats<small>The agent saves what it learns about you and your projects, and reads it back in new chats. Nothing here is committed to the project.</small></span>
        <Switch checked={settings.enabled} onChange={event => void toggle(event.target.checked)} />
      </label>
      {settings.enabled && <div className="themis-settings-control themis-memory-showing">
        <span>Showing</span>
        <span className="themis-memory-scopes" role="group" aria-label="Which memories">
          <button type="button" aria-pressed={scope === "project"} onClick={() => pick("project")}>{projectLabel}</button>
          <button type="button" aria-pressed={scope === "global"} onClick={() => pick("global")}>Everywhere</button>
        </span>
      </div>}
    </SettingsGroup>
    {!settings.enabled && <SettingsGroup><p className="themis-memory-empty">Memory is off. The agent won't save or read memories; the ones it saved are kept until you switch it back on.</p></SettingsGroup>}
    {settings.enabled && list && <>
      <div className="themis-memory-head">
        <span>{memories.length === 1 ? "1 memory" : `${memories.length} memories`}</span>
        <span className="themis-memory-meter" title="The index the agent reads at the start of each chat">index {list.index_lines} of {list.max_lines} lines</span>
      </div>
      {memories.length === 0 && <SettingsGroup><p className="themis-memory-empty">{scope === "project" ? "Nothing remembered for this project yet." : "Nothing remembered everywhere yet."}</p></SettingsGroup>}
      {KINDS.map(([kind, title]) => {
        const rows = memories.filter(memory => memory.type === kind);
        if (rows.length === 0) return null;
        return <SettingsGroup key={kind} title={title}>
          {rows.map(memory => <MemoryRow key={memory.name} memory={memory} open={open === memory.name} onToggle={now => setOpen(now ? memory.name : null)} onDelete={() => remove(memory)} />)}
        </SettingsGroup>;
      })}
    </>}
  </>;
}
