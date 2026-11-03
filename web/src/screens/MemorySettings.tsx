import { useEffect, useState } from "react";
import { DOT_GRID, DotSpinner } from "../components/DotSpinner";
import { Switch } from "../components";
import { deleteMemory, getMemorySettings, listMemories, updateMemorySettings } from "../lib/tauri";
import type { Memory, MemoryChange, MemoryKind, MemoryList, MemoryScope, MemoryWaiting } from "../lib/types";
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

/** The memory mark: the sidebar's 3×3 dot grid with its top and bottom rows lit, muted. */
export function MemoryDots() {
  const lit = [0, 1, 2, 6, 7, 8];
  return <svg className="themis-memory-dots" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
    {DOT_GRID.map(([cx, cy], cell) => <circle key={cell} className={lit.includes(cell) ? "is-lit" : undefined} cx={cx} cy={cy} r="2.1" />)}
  </svg>;
}

const VERBS: Record<MemoryChange["action"], string> = { saved: "Saved", updated: "Updated", removed: "Removed" };

/** The Settings key that opens one memory: `<scope>:<name>`. */
export function memoryKey(change: Pick<MemoryChange, "scope" | "name">): string {
  return `${change.scope}:${change.name}`;
}

/** "Saved <name> · global": the name opens it in Settings › Memory (a removed one has nothing to
 *  open); hovering it shows its file and the memory model that made the change. */
function ChangeText({ change }: { change: MemoryChange }) {
  const { dispatch } = useApp();
  const verb = VERBS[change.action];
  const title = memoryTitle(change.name);
  const hint = change.model ? `memory/${change.name}.md · by ${change.model}` : `memory/${change.name}.md`;
  return <>
    <span>{verb}</span>{" "}
    {change.action === "removed"
      ? <span title={hint}>{title}</span>
      : <button type="button" title={hint} aria-label={`${verb} ${title}: open in Settings`} onClick={() => dispatch({ type: "ui/view", view: "settings", settings: { section: "Memory", key: memoryKey(change) } })}>{title}</button>}
    <span className="themis-memory-scope"> · {change.scope}</span>
  </>;
}

/** How long a line that just arrived shows what is being written before it settles. */
export const MEMORY_SETTLE_MS = 900;
/** How long "Memory will retry later" stays before it fades. */
export const MEMORY_WAITING_MS = 6000;

const WRITING: Record<MemoryChange["action"], string> = { saved: "Saving", updated: "Updating", removed: "Removing" };

/** "Saving uses-pnpm.md" for one change; "Saving 2 memories · updating index" for several. */
export function writingText(changes: MemoryChange[]): string {
  if (changes.length === 1) return `${WRITING[changes[0]!.action]} ${changes[0]!.name}.md`;
  return `Saving ${changes.length} memories · updating index`;
}

/** While the memory models work after a turn: a shimmering line under the latest reply. When
 *  every model failed it says so quietly for a moment (Settings › Memory keeps the details). */
export function MemoryWorkingLine({ step }: { step: "checking" | "waiting" }) {
  const [hidden, setHidden] = useState(false);
  useEffect(() => {
    setHidden(false);
    if (step !== "waiting") return;
    const timer = window.setTimeout(() => setHidden(true), MEMORY_WAITING_MS);
    return () => window.clearTimeout(timer);
  }, [step]);
  if (hidden) return null;
  return <div className="themis-memory-line is-working" role="status">
    {step === "checking" ? <DotSpinner className="themis-memory-dots" /> : <MemoryDots />}
    {step === "checking"
      ? <span className="themis-shimmer">Checking what to remember…</span>
      : <span>Memory will retry later</span>}
  </div>;
}

/** Under a reply: what the memory models changed after that turn, on one slim line. Several
 *  changes read "Saved 2 · updated 1", with each one listed on expand. A line that just arrived
 *  (`settling`) first shows what is being written, shimmering, then settles. */
export function MemoryLine({ changes, settling = false }: { changes: MemoryChange[]; settling?: boolean }) {
  const [open, setOpen] = useState(false);
  const [writing, setWriting] = useState(settling);
  useEffect(() => {
    if (!writing) return;
    const timer = window.setTimeout(() => setWriting(false), MEMORY_SETTLE_MS);
    return () => window.clearTimeout(timer);
  }, [writing]);
  if (writing) {
    return <div className="themis-memory-line is-working" role="status"><DotSpinner className="themis-memory-dots" /><span className="themis-shimmer">{writingText(changes)}</span></div>;
  }
  if (changes.length === 1) {
    return <div className="themis-memory-line" role="note"><MemoryDots /><span className="themis-memory-change"><ChangeText change={changes[0]!} /></span></div>;
  }
  const summary = (["saved", "updated", "removed"] as const)
    .map(action => [action, changes.filter(change => change.action === action).length] as const)
    .filter(([, count]) => count > 0)
    .map(([action, count], index) => `${index === 0 ? VERBS[action] : action} ${count}`)
    .join(" · ");
  return <div className="themis-memory-line is-several" role="note">
    <MemoryDots />
    <span className="themis-memory-group">
      <button type="button" className="themis-memory-summary" aria-expanded={open} onClick={() => setOpen(value => !value)}>{summary}</button>
      {open && <ul>{changes.map(change => <li key={`${change.scope}:${change.name}:${change.action}`}><ChangeText change={change} /></li>)}</ul>}
    </span>
  </div>;
}

const REASONS: Record<string, string> = {
  rate_limited: "rate limited",
  provider_error: "unavailable",
  timed_out: "timed out",
  unreadable: "unreadable answer",
  over_budget: "daily cap reached",
  unavailable: "can't be reached",
};

/** "Waiting for the memory models: 2 jobs (rate limited, daily cap reached)". */
function waitingText(waiting: MemoryWaiting): string {
  const reasons = [...new Set(waiting.reasons.map(reason => REASONS[reason] ?? reason.replace(/_/g, " ")))];
  const jobs = waiting.jobs === 1 ? "1 job" : `${waiting.jobs} jobs`;
  return `Waiting for the memory models: ${jobs}${reasons.length ? ` (${reasons.join(", ")})` : ""}`;
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

/** Settings › Memory: what is remembered for this project or everywhere, read and delete, the
 *  `memory.enabled` switch (saved in place in the user runtime.jsonc), and a status line while
 *  memory jobs wait for the memory models. */
export function MemorySettingsSection({ focusKey, onSaved }: { focusKey?: string; onSaved?: () => void } = {}) {
  const { state } = useApp();
  const project = useActiveProject();
  const focusScope = focusKey?.split(":")[0] as MemoryScope | undefined;
  const [scope, setScope] = useState<MemoryScope>(focusScope === "global" ? "global" : "project");
  const [open, setOpen] = useState<string | null>(focusKey?.split(":").slice(1).join(":") ?? null);
  const [settings, setSettings] = useState<{ enabled: boolean; revision: string; waiting: MemoryWaiting | null } | null>(null);
  const [list, setList] = useState<MemoryList | null>(null);
  const [error, setError] = useState("");
  const root = state.activeProjectRoot;
  const loadSettings = () => getMemorySettings().then(view => setSettings({ enabled: view.settings.enabled, revision: view.revision, waiting: view.waiting ?? null })).catch(reason => setError(describeError(reason)));
  const loadList = (which: MemoryScope) => {
    if (which === "project" && !root) { setList(null); return Promise.resolve(); }
    return listMemories(which, which === "project" ? root : null).then(setList).catch(reason => setError(describeError(reason)));
  };
  useEffect(() => { void loadSettings(); }, []);
  useEffect(() => { void loadList(scope); }, [scope, root]); // eslint-disable-line react-hooks/exhaustive-deps
  if (!settings) return error ? <p role="alert" className="themis-form-error">{error}</p> : null;

  const toggle = async (changes: { enabled: boolean }) => {
    setError("");
    try {
      const view = await updateMemorySettings(changes, settings.revision);
      setSettings({ ...settings, enabled: view.settings.enabled, revision: view.revision });
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
        <span className="themis-settings-name">Remember things across chats<small>After each reply, the memory models (Settings › Models) keep what's worth knowing about you and your projects, and new chats start with it. Nothing here is committed to the project.</small></span>
        <Switch checked={settings.enabled} onChange={event => void toggle({ enabled: event.target.checked })} />
      </label>
      {settings.enabled && settings.waiting && <p role="status" className="themis-settings-hint themis-memory-waiting">{waitingText(settings.waiting)}</p>}
      {settings.enabled && <div className="themis-settings-control themis-memory-showing">
        <span>Showing</span>
        <span className="themis-memory-scopes" role="group" aria-label="Which memories">
          <button type="button" aria-pressed={scope === "project"} onClick={() => pick("project")}>{projectLabel}</button>
          <button type="button" aria-pressed={scope === "global"} onClick={() => pick("global")}>Everywhere</button>
        </span>
      </div>}
    </SettingsGroup>
    {!settings.enabled && <SettingsGroup><p className="themis-memory-empty">Memory is off. Nothing is saved or read; what was saved is kept until you switch it back on.</p></SettingsGroup>}
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
