import { useEffect, useState } from "react";
import { Switch } from "../components";
import { getMemorySettings, updateMemorySettings } from "../lib/tauri";
import type { MemoryChange, MemoryWaiting } from "../lib/types";
import { describeError, useSettingsChanges } from "../state/store";
import { SectionIcon, SettingsGroup } from "./SettingsGroup";
import "./MemorySettings.css";

/** The memory mark: Settings › Memory's own icon, small and muted. */
export function MemoryIcon() {
  return <span className="themis-memory-icon" aria-hidden="true"><SectionIcon section="Memory" /></span>;
}

const VERBS: Record<MemoryChange["action"], string> = { saved: "Saved", updated: "Updated", removed: "Removed" };


/** "Saved <name>.md · global"; hovering the name shows its file and the memory model. */
function ChangeText({ change }: { change: MemoryChange }) {
  const verb = VERBS[change.action];
  const hint = change.model ? `memory/${change.name}.md · by ${change.model}` : `memory/${change.name}.md`;
  return <>
    <span>{verb}</span>{" "}<span title={hint}>{change.name}.md</span>
    <span className="themis-memory-scope"> · {change.scope}</span>
  </>;
}

/** How long a line that just arrived shows what is being written before it settles. */
export const MEMORY_SETTLE_MS = 900;
/** How long "Memory will retry later" stays before it fades. */
export const MEMORY_WAITING_MS = 6000;
/** How long the settled "Saved x.md" stays before it fades away (Settings › Memory keeps it). */
export const MEMORY_SHOWN_MS = 3000;
/** The fade itself. */
export const MEMORY_FADE_MS = 400;

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
    <MemoryIcon />
    {step === "checking"
      ? <span className="themis-shimmer">Checking what to remember…</span>
      : <span>Memory will retry later</span>}
  </div>;
}

/** Under a reply: what the memory models changed after that turn, on one slim line. Several
 *  changes read "Saved 2 · updated 1", with each one listed on expand. A line that just arrived
 *  (`settling`) first shows what is being written, shimmering, then settles. */
/** Live lines that already faded: coming back to the chat does not play them again. */
const FADED = new Set<string>();

export function MemoryLine({ changes, settling = false, fades = false, id }: { changes: MemoryChange[]; settling?: boolean; fades?: boolean; id?: string }) {
  const done = fades && id !== undefined && FADED.has(id);
  const [open, setOpen] = useState(false);
  const [writing, setWriting] = useState(settling && !done);
  // A live line goes once it has said what it saved: writing, then shown, then fading, then gone.
  const [phase, setPhase] = useState<"shown" | "fading" | "gone">(done ? "gone" : "shown");
  useEffect(() => { if (phase === "gone" && id !== undefined) FADED.add(id); }, [phase, id]);
  useEffect(() => {
    if (!writing) return;
    const timer = window.setTimeout(() => setWriting(false), MEMORY_SETTLE_MS);
    return () => window.clearTimeout(timer);
  }, [writing]);
  useEffect(() => {
    if (!fades || writing || open || phase === "gone") return;
    const timer = window.setTimeout(() => setPhase(phase === "shown" ? "fading" : "gone"), phase === "shown" ? MEMORY_SHOWN_MS : MEMORY_FADE_MS);
    return () => window.clearTimeout(timer);
  }, [fades, writing, open, phase]);
  if (phase === "gone") return null;
  const fading = phase === "fading" ? " is-fading" : "";
  if (writing) {
    return <div className="themis-memory-line is-working" role="status"><MemoryIcon /><span className="themis-shimmer">{writingText(changes)}</span></div>;
  }
  if (changes.length === 1) {
    return <div className={`themis-memory-line${fading}`} role="note"><MemoryIcon /><span className="themis-memory-change"><ChangeText change={changes[0]!} /></span></div>;
  }
  const summary = (["saved", "updated", "removed"] as const)
    .map(action => [action, changes.filter(change => change.action === action).length] as const)
    .filter(([, count]) => count > 0)
    .map(([action, count], index) => `${index === 0 ? VERBS[action] : action} ${count}`)
    .join(" · ");
  return <div className={`themis-memory-line is-several${fading}`} role="note">
    <MemoryIcon />
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


/** Settings › Memory: the `memory.enabled` switch (saved in place in the user runtime.jsonc),
 *  and a status line while memory jobs wait for the memory models. Memories themselves are
 *  kept by the memory models and not listed here. */
export function MemorySettingsSection({ onSaved }: { onSaved?: () => void } = {}) {
  const [settings, setSettings] = useState<{ enabled: boolean; revision: string; waiting: MemoryWaiting | null } | null>(null);
  const [error, setError] = useState("");
  const loadSettings = () => getMemorySettings().then(view => setSettings({ enabled: view.settings.enabled, revision: view.revision, waiting: view.waiting ?? null })).catch(reason => setError(describeError(reason)));
  const changes = useSettingsChanges("runtime");
  useEffect(() => { void loadSettings(); }, [changes]);
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

  return <>
    {error && <p role="alert" className="themis-form-error">{error}</p>}
    <SettingsGroup>
      <label className="themis-settings-check themis-memory-switch">
        <span className="themis-settings-name">Remember things across chats<small>The memory models (Settings › Models) keep what's worth knowing about you and your projects as you work, and every chat starts with it. Nothing here is committed to the project.</small></span>
        <Switch checked={settings.enabled} onChange={event => void toggle({ enabled: event.target.checked })} />
      </label>
      {settings.enabled && settings.waiting && <p role="status" className="themis-settings-hint themis-memory-waiting">{waitingText(settings.waiting)}</p>}
    </SettingsGroup>
    {!settings.enabled && <SettingsGroup><p className="themis-memory-empty">Memory is off. Nothing is saved or read; what was saved is kept until you switch it back on.</p></SettingsGroup>}
  </>;
}
