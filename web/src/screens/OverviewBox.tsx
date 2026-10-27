import { useEffect, useState } from "react";
import { browserState, type BrowserState } from "../lib/browser";
import { getUsageSummary } from "../lib/tauri";
import type { AttachedMedia, DiffState } from "../lib/types";
import type { ChatMessage, WorkspaceTab } from "../state/reducer";
import { readSession, writeSession } from "../state/session";
import { useApp } from "../state/store";
import { countTasks } from "./threadTasks";
import "./OverviewBox.css";

const COLLAPSED_KEY = "overviewCollapsed";

export interface Overview {
  changes: { files: number; added: number; removed: number } | null;
  tasks: { running: number; attention: number; queued: number } | null;
  browser: { label: string; agent: boolean } | null;
  media: AttachedMedia[];
  cost: { usd?: number; free: boolean } | null;
  /** Something to show: the box (and the room the chat leaves for it) appears only then. */
  visible: boolean;
  collapsed: boolean;
  setCollapsed: (collapsed: boolean) => void;
}

/** Added and removed lines across the chat's changed files. */
export function diffTotals(diff: DiffState | null | undefined): Overview["changes"] {
  if (!diff?.available || diff.files.length === 0) return null;
  let added = 0;
  let removed = 0;
  for (const file of diff.files) for (const hunk of file.hunks) for (const line of hunk.lines) {
    if (line.kind === "add") added++;
    else if (line.kind === "del") removed++;
  }
  return { files: diff.files.length, added, removed };
}

function browserLabel(state: BrowserState): string {
  if (state.title) return state.title;
  try { return new URL(state.url).host || state.url; } catch { return state.url; }
}

/** What the chat's overview box shows, from data the app already keeps for this chat. */
export function useOverview(threadId: string, messages: ChatMessage[]): Overview {
  const { state } = useApp();
  const [collapsed, setCollapsedState] = useState(() => readSession<boolean>(COLLAPSED_KEY, false));
  const [browser, setBrowser] = useState<BrowserState | null>(null);
  const [cost, setCost] = useState<Overview["cost"]>(null);
  const running = state.running[threadId] ?? false;
  const browserAt = state.browsers[threadId]?.at;

  // The browser row follows the chat's browser: refreshed when its driver changes and when a run starts or ends.
  useEffect(() => {
    if (!threadId) return;
    let live = true;
    browserState(threadId).then(next => { if (live) setBrowser(next); }).catch(() => { if (live) setBrowser(null); });
    return () => { live = false; };
  }, [threadId, browserAt, running]);

  // Cost comes from the usage log; it changes only when a run ends.
  useEffect(() => {
    if (!threadId || running) return;
    let live = true;
    getUsageSummary().then(summary => {
      if (!live) return;
      const chat = summary.available ? summary.by_chat.find(entry => entry.key === threadId) : undefined;
      setCost(chat ? { usd: chat.totals.estimated_cost_usd, free: chat.free === true } : null);
    }).catch(() => { if (live) setCost(null); });
    return () => { live = false; };
  }, [threadId, running]);

  const counts = countTasks(state.tasks[threadId], state.settings.automations_enabled);
  const queued = state.queues[threadId]?.length ?? 0;
  const tasks = counts.running + counts.attention + queued > 0 ? { running: counts.running, attention: counts.attention, queued } : null;
  const changes = diffTotals(state.diffs[threadId]);
  const media = messages.flatMap(message => message.media ?? []);
  const shownBrowser = browser?.open && browser.url !== "about:blank" ? { label: browserLabel(browser), agent: browser.controller === "agent" } : null;
  const shownCost = cost && (cost.free || (cost.usd ?? 0) > 0) ? cost : null;
  const setCollapsed = (value: boolean) => { setCollapsedState(value); writeSession(COLLAPSED_KEY, value); };
  return {
    changes, tasks, browser: shownBrowser, media, cost: shownCost,
    visible: !!(changes || tasks || shownBrowser || media.length || shownCost),
    collapsed, setCollapsed,
  };
}

function formatUsd(usd: number): string {
  return usd < 0.01 ? "< $0.01" : `$${usd.toFixed(2)}`;
}

/** A compact summary of the chat's workspace over the top right of the chat. Every row opens
 *  the Workspace on its tab; the Workspace's own button and Close bring this box back. */
export function OverviewBox({ overview, onOpenMedia }: { overview: Overview; onOpenMedia: (path: string) => void }) {
  const { dispatch } = useApp();
  const { changes, tasks, browser, media, cost, collapsed, setCollapsed } = overview;
  const open = (tab?: WorkspaceTab) => dispatch({ type: "ui/diff-panel", open: true, ...(tab ? { tab } : {}) });
  const taskText = tasks ? [tasks.running ? `${tasks.running} running` : "", tasks.attention ? `${tasks.attention} need you` : "", tasks.queued ? `${tasks.queued} queued` : ""].filter(Boolean).join(" · ") : "";
  return (
    <section className={`themis-overview${collapsed ? " is-collapsed" : ""}`} aria-label="Chat overview">
      <div className="themis-overview-head">
        <button type="button" className="themis-overview-icon themis-icon-tip" data-tooltip="Workspace" aria-label="Open Workspace" onClick={() => open()}>
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true"><rect x="3" y="4" width="18" height="16" rx="3" /><path d="M14 4v16" /></svg>
        </button>
        <button type="button" className="themis-overview-icon themis-icon-tip" data-tooltip={collapsed ? "Show details" : "Icons only"} aria-label={collapsed ? "Show overview details" : "Show only icons"} aria-expanded={!collapsed} onClick={() => setCollapsed(!collapsed)}>
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true"><path d={collapsed ? "m9 6 6 6-6 6" : "m15 6-6 6 6 6"} /></svg>
        </button>
      </div>
      {browser && (
        <button type="button" className="themis-overview-row themis-icon-tip" data-tooltip="Browser" aria-label={`Browser: ${browser.label}${browser.agent ? ", the agent is using it" : ""}`} onClick={() => open("browser")}>
          <svg className="themis-overview-glyph" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" aria-hidden="true"><circle cx="12" cy="12" r="9" /><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18" /></svg>
          <span className="themis-overview-value">{browser.label}</span>
          {browser.agent && <span className="themis-overview-dot" aria-hidden="true" />}
        </button>
      )}
      {changes && (
        <button type="button" className="themis-overview-row themis-icon-tip" data-tooltip="Changes" aria-label={`Changes: ${changes.files} file${changes.files === 1 ? "" : "s"}, ${changes.added} added and ${changes.removed} removed lines`} onClick={() => open("changes")}>
          <svg className="themis-overview-glyph" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" aria-hidden="true"><circle cx="6" cy="5" r="2" /><circle cx="6" cy="19" r="2" /><circle cx="18" cy="7" r="2" /><path d="M6 7v10M18 9c0 5-6 4-11 8" /></svg>
          <span className="themis-overview-value">{changes.files} file{changes.files === 1 ? "" : "s"}</span>
          <span className="themis-overview-meta"><span className="is-added">+{changes.added}</span> <span className="is-removed">−{changes.removed}</span></span>
        </button>
      )}
      {tasks && (
        <button type="button" className="themis-overview-row themis-icon-tip" data-tooltip="Tasks" aria-label={`Tasks: ${taskText}`} onClick={() => open("tasks")}>
          <svg className="themis-overview-glyph" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="m4 6 1.5 1.5L8 5M4 12l1.5 1.5L8 11M4 18l1.5 1.5L8 17M12 6h8M12 12h8M12 18h8" /></svg>
          <span className="themis-overview-value">{taskText}</span>
          {tasks.attention > 0 && <span className="themis-overview-dot is-attention" aria-hidden="true" />}
        </button>
      )}
      {media.length > 0 && (
        <button type="button" className="themis-overview-row themis-icon-tip" data-tooltip="Artifacts" aria-label={`Artifacts: ${media.length}. Opens the latest, ${media[media.length - 1]!.name}`} onClick={() => onOpenMedia(media[media.length - 1]!.path)}>
          <svg className="themis-overview-glyph" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><rect x="4" y="4" width="16" height="16" rx="2" /><circle cx="9" cy="9" r="1.5" /><path d="m20 15-5-5-9 9" /></svg>
          <span className="themis-overview-value">{media[media.length - 1]!.name}</span>
          <span className="themis-overview-meta">{media.length}</span>
        </button>
      )}
      {cost && (
        <button type="button" className="themis-overview-row themis-icon-tip" data-tooltip="Cost of this chat" aria-label={`Cost of this chat: ${cost.free ? "free model" : formatUsd(cost.usd ?? 0)}. Opens Settings › Usage`} onClick={() => dispatch({ type: "ui/view", view: "settings", settings: { section: "Usage" } })}>
          <svg className="themis-overview-glyph" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" aria-hidden="true"><circle cx="12" cy="12" r="9" /><path d="M14.5 9.5c-.5-1-1.5-1.5-2.5-1.5-1.5 0-2.5.8-2.5 2s1 1.6 2.5 2 2.5.8 2.5 2-1 2-2.5 2c-1 0-2-.5-2.5-1.5M12 6.5V8M12 16v1.5" /></svg>
          <span className="themis-overview-value">{cost.free ? "Free model" : formatUsd(cost.usd ?? 0)}</span>
        </button>
      )}
    </section>
  );
}