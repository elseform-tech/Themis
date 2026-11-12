import { useEffect, useRef, useState } from "react";
import { Button, Dialog, Input } from "../components";
import { createProject, deleteProject, getSettings, renameProject, renameThread } from "../lib/tauri";
import { describeError, toast, useApp } from "../state/store";
import { useNewThread, useDiscardThread } from "./actions";
import type { ThreadInfo } from "../lib/types";
import { readSession, writeSession } from "../state/session";
import { MovingText } from "../components/MovingText";
import { Tooltip } from "../components/primitives/Tooltip";
import { DOT_GRID, DotSpinner } from "../components/DotSpinner";
import { SettingsNav } from "./SettingsNav";
import { Rail } from "./Rail";
import "./Sidebar.css";

export type SidebarIconName = "panel" | "chat" | "thread" | "skills" | "automations" | "queue" | "project" | "settings" | "help" | "search" | "rename" | "trash";

/** Below this the chat list is mostly cut-off titles. */
/** The canvas's 52px rail plus its 236px panel. */
const SIDEBAR_MIN = 288;
const SIDEBAR_MAX = 560;
const SIDEBAR_DEFAULT = 288;

// Settings takes over the whole sidebar with its section list; the toggle folds it the same way.
export function Sidebar({ collapsed }: { collapsed: boolean }) {
  const { state, dispatch } = useApp();
  const settingsOpen = state.mainView === "settings";
  const [width, setWidth] = useState(() => Math.max(SIDEBAR_MIN, Math.min(SIDEBAR_MAX, Number(readSession("sidebarWidth", SIDEBAR_DEFAULT)) || SIDEBAR_DEFAULT)));
  const resizeStart = useRef<{ x: number; width: number } | null>(null);
  const [resizing, setResizing] = useState(false);
  const slot = useRef<HTMLDivElement>(null);
  // Entering or leaving Settings swaps the sidebar's contents and removes the focused
  // control; keep keyboard focus in the sidebar on the current item instead of losing it.
  const wasSettings = useRef(settingsOpen);
  useEffect(() => {
    if (wasSettings.current === settingsOpen) return;
    wasSettings.current = settingsOpen;
    const active = document.activeElement;
    if (!active || active === document.body) slot.current?.querySelector<HTMLElement>('[aria-current="page"]')?.focus();
  }, [settingsOpen]);
  const clampSidebar = (value: number) => Math.round(Math.max(SIDEBAR_MIN, Math.min(SIDEBAR_MAX, value)));
  function resize(value: number) {
    const next = clampSidebar(value);
    setWidth(next);
    writeSession("sidebarWidth", next);
  }
  // While dragging, move only the CSS variable; React state and storage update once on release.
  const lastWidth = useRef<number | null>(null);
  function preview(value: number) { lastWidth.current = clampSidebar(value); slot.current?.style.setProperty("--themis-sidebar-width", `${lastWidth.current}px`); }
  function finishResize() {
    if (resizeStart.current && lastWidth.current !== null) resize(lastWidth.current);
    resizeStart.current = null; lastWidth.current = null; setResizing(false);
  }
  const newThread = useNewThread();
  const [name, setName] = useState("");
  const [renaming, setRenaming] = useState<{ root: string; name: string } | null>(null);
  const [deleting, setDeleting] = useState<{ root: string; name: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const { discard } = useDiscardThread();
  const [editing, setEditing] = useState<(ThreadInfo & { projectRoot: string }) | null>(null);
  const [removing, setRemoving] = useState<(ThreadInfo & { projectRoot: string }) | null>(null);
  const [editError, setEditError] = useState("");
  const [editBusy, setEditBusy] = useState(false);
  const [help, setHelp] = useState(false);
  const activeRoot = state.activeProjectRoot;
  const activeProject = state.projects.find(project => project.root === activeRoot) ?? null;
  const projects = [...state.projects].sort((a, b) => Number(!!b.is_default) - Number(!!a.is_default));
  const statusOf = (thread: ThreadInfo) => threadStatus(state, thread);
  const threads = [...(activeRoot ? state.threadsByProject[activeRoot] ?? [] : [])]
    .sort((a, b) => (b.last_activity_seq ?? 0) - (a.last_activity_seq ?? 0));
  // Chats waiting for you, then working ones, sit above the rest; each keeps newest first.
  const waiting = threads.filter(thread => isWaiting(statusOf(thread)));
  const live = threads.filter(thread => statusOf(thread) === "Working");
  const rest = threads.filter(thread => !isLive(statusOf(thread)));
  const [showAll, setShowAll] = useState(false);
  const shown = showAll ? rest : rest.slice(0, 12);
  const finished = useFreshlyFinished(state.running, state.mainView === "thread" ? state.activeThreadId : null);
  const projectBusy = (root: string) => (state.threadsByProject[root] ?? []).some(thread => isLive(statusOf(thread)));
  const busyElsewhere = projects.some(project => project.root !== activeRoot && projectBusy(project.root));

  async function create() {
    if (busy || !name.trim()) return;
    setBusy(true); setError("");
    try {
      const project = await createProject(name.trim());
      dispatch({ type: "project/opened", project });
      dispatch({ type: "ui/project-dialog", open: false });
      setName("");
      await newThread(project.root);
      dispatch({ type: "settings/patched", settings: await getSettings() });
      toast(dispatch, `Created ${project.name}`, "success");
    } catch (error: unknown) { setError(describeError(error)); }
    finally { setBusy(false); }
  }
  async function saveThread() {
    if (!editing || editBusy) return;
    setEditBusy(true); setEditError("");
    try {
      const updated = await renameThread(editing.id, editing.title);
      dispatch({ type: "thread/updated", projectRoot: editing.projectRoot, thread: updated });
      setEditing(null);
    } catch (error) { setEditError(describeError(error)); }
    finally { setEditBusy(false); }
  }
  function threadRow(thread: ThreadInfo) {
    const active = thread.id === state.activeThreadId && state.mainView === "thread";
    const locked = state.running[thread.id] ?? thread.running;
    const status = statusOf(thread);
    const unseen = finished.has(thread.id) && !isLive(status);
    return <li key={thread.id} className="themis-sidebar-thread"><button type="button" className={`themis-sidebar-row themis-thread-row ${active ? "themis-sidebar-row--active" : ""} ${unseen ? "is-unseen" : ""}`} title={thread.title} aria-label={thread.title} aria-current={active ? "true" : undefined} onClick={() => activeRoot && dispatch({ type: "thread/selected", projectRoot: activeRoot, threadId: thread.id })}><ThreadStatus status={status} /><MovingText>{thread.title}</MovingText>{TAG[status] && <span className="themis-thread-tag">{TAG[status]}</span>}</button><details className="themis-thread-row-actions"><summary aria-label={`Options for ${thread.title}`} title="Thread options"><svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><circle cx="12" cy="5" r="1.7" /><circle cx="12" cy="12" r="1.7" /><circle cx="12" cy="19" r="1.7" /></svg></summary><div className="themis-row-menu"><button type="button" aria-label={`Edit ${thread.title}`} disabled={locked} onClick={event => { event.currentTarget.closest("details")?.removeAttribute("open"); setEditError(""); setEditing({ ...thread, projectRoot: activeRoot! }); }}>Rename</button><button type="button" className="is-danger" aria-label={`Remove ${thread.title}`} disabled={locked} onClick={event => { event.currentTarget.closest("details")?.removeAttribute("open"); setRemoving({ ...thread, projectRoot: activeRoot! }); }}>Remove</button></div></details></li>;
  }
  return <>
    <div ref={slot} className={`themis-sidebar-slot ${collapsed ? "is-collapsed" : ""} ${resizing ? "is-resizing" : ""}`} style={{ "--themis-sidebar-width": `${width}px` } as import("react").CSSProperties}>
      {settingsOpen ? <SettingsNav id="themis-sidebar" /> : <>
      <Rail onHelp={() => setHelp(true)} />
      <aside id="themis-sidebar" className={`themis-sidebar ${collapsed ? "" : "is-visible"}`} inert={collapsed} aria-hidden={collapsed} aria-label="Workspace navigation">
        <div className="themis-sidebar-top">
          <details className="themis-project-switcher" data-popover>
            <summary aria-label={`Project: ${activeProject?.name ?? "none"}`} title={activeProject?.root}>
              <SidebarIcon name="project" /><MovingText>{activeProject?.name ?? "Choose a project"}</MovingText>
              {busyElsewhere && <span className="themis-project-busy" role="img" aria-label="Activity in another project" />}
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true"><path d="m6 9 6 6 6-6" /></svg>
            </summary>
            <div className="themis-project-menu" role="group" aria-label="Projects">
              {projects.length === 0 && <p className="themis-sidebar-note">No projects</p>}
              {projects.map(project => (
                <div key={project.root} className="themis-project-entry">
                  <button type="button" className="themis-project-option" title={project.root} aria-current={project.root === activeRoot ? "true" : undefined}
                    onClick={event => { event.currentTarget.closest("details")?.removeAttribute("open"); setShowAll(false); dispatch({ type: "project/selected", root: project.root }); }}>
                    <MovingText>{project.name}</MovingText>
                    {projectBusy(project.root) && <span className="themis-project-busy" aria-hidden="true" />}
                  </button>
                  <button type="button" className="themis-project-action" aria-label={`Rename ${project.name}`} title="Rename" onClick={event => { event.currentTarget.closest("details")?.removeAttribute("open"); setError(""); setRenaming({ root: project.root, name: project.name }); }}><SidebarIcon name="rename" /></button>
                  {project.is_default
                    ? <span className="themis-project-action" aria-hidden="true" />
                    : <button type="button" className="themis-project-action is-danger" aria-label={`Delete ${project.name}`} title="Delete" onClick={event => { event.currentTarget.closest("details")?.removeAttribute("open"); setError(""); setDeleting({ root: project.root, name: project.name }); }}><SidebarIcon name="trash" /></button>}
                  <span className="themis-project-count" aria-label={`${(state.threadsByProject[project.root] ?? []).length} chats`}>{(state.threadsByProject[project.root] ?? []).length}</span>
                </div>
              ))}
              <hr />
              <button type="button" className="themis-project-option" aria-label="New project" onClick={event => { event.currentTarget.closest("details")?.removeAttribute("open"); dispatch({ type: "ui/project-dialog", open: true }); }}>New project</button>
            </div>
          </details>
          <NewChatButton disabled={!state.activeProjectRoot} onClick={() => void newThread()} />
        </div>
        <div className="themis-sidebar-projects">
          {activeProject && <>
            {waiting.length > 0 && <section className="themis-thread-group" aria-label="Waiting for you">
              <h2 className="themis-sidebar-section">Waiting for you<span aria-hidden="true">{waiting.length}</span></h2>
              <ul className="themis-sidebar-list">{waiting.map(threadRow)}</ul>
            </section>}
            {live.length > 0 && <section className="themis-thread-group" aria-label="Running">
              <h2 className="themis-sidebar-section">Running<span aria-hidden="true">{live.length}</span></h2>
              <ul className="themis-sidebar-list">{live.map(threadRow)}</ul>
            </section>}
            {(rest.length > 0 || threads.length === 0) && <section className="themis-thread-group" aria-label="Chats">
              <h2 className="themis-sidebar-section">Chats{rest.length > 0 && <span aria-hidden="true">{rest.length}</span>}</h2>
              {rest.length ? <ul className="themis-sidebar-list">{shown.map(threadRow)}</ul> : <p className="themis-sidebar-note">No chats yet</p>}
            </section>}
            {rest.length > shown.length && <button type="button" className="themis-show-more" onClick={() => setShowAll(true)}>Show {rest.length - shown.length} more</button>}
          </>}
        </div>
      </aside>
      </>}
      {!collapsed && <div className="themis-sidebar-resize" role="separator" aria-label="Resize sidebar" aria-controls="themis-sidebar" aria-orientation="vertical" aria-valuemin={SIDEBAR_MIN} aria-valuemax={SIDEBAR_MAX} aria-valuenow={width} tabIndex={0}
        onPointerDown={event => { if (event.button !== 0) return; event.preventDefault(); event.currentTarget.focus(); resizeStart.current = { x: event.clientX, width: event.currentTarget.parentElement!.getBoundingClientRect().width }; event.currentTarget.setPointerCapture(event.pointerId); setResizing(true); }}
        onPointerMove={event => { if (resizeStart.current) preview(resizeStart.current.width + event.clientX - resizeStart.current.x); }}
        onPointerUp={event => { finishResize(); if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId); }}
        onLostPointerCapture={finishResize}
        onPointerCancel={() => { resizeStart.current = null; setResizing(false); preview(width); }}
        onDoubleClick={() => resize(SIDEBAR_DEFAULT)}
        onKeyDown={event => { if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) { event.preventDefault(); resize(event.key === "Home" ? SIDEBAR_MIN : event.key === "End" ? SIDEBAR_MAX : width + (event.key === "ArrowRight" ? 20 : -20)); } }}
      />}
    </div>
    <Dialog open={state.projectDialogOpen} title="Create project" onClose={() => { if (!busy) dispatch({ type: "ui/project-dialog", open: false }); }}>
      <form className="themis-sidebar-dialog" onSubmit={event => { event.preventDefault(); void create(); }}>
        <Input id="project-name" label="Project name" placeholder="My project" value={name} maxLength={80} disabled={busy} onChange={e => { setName(e.target.value); setError(""); }} />
        <p className="themis-sidebar-dialog-hint">A new folder and Git repository will be created for your project.</p>
        <p className="themis-project-location">{state.settings.projects_directory}/{name.trim() || "project-name"}</p>
        {error && <p role="alert" className="themis-form-error">{error}</p>}
        <div className="themis-sidebar-dialog-actions"><Button variant="ghost" disabled={busy} onClick={() => dispatch({ type: "ui/project-dialog", open: false })}>Cancel</Button><Button type="submit" variant="primary" disabled={busy || !name.trim()}>{busy ? "Creating…" : "Create project"}</Button></div>
      </form>
    </Dialog>
    <Dialog open={renaming !== null} title="Rename project" onClose={() => { if (!busy) setRenaming(null); }}>
      {renaming && <form className="themis-sidebar-dialog" onSubmit={async event => {
        event.preventDefault(); if (busy) return; setBusy(true); setError("");
        try { const project = await renameProject(renaming.root, renaming.name); dispatch({ type: "project/opened", project, select: false }); setRenaming(null); }
        catch (error) { setError(describeError(error)); } finally { setBusy(false); }
      }}>
        <Input id="rename-project" label="Name" value={renaming.name} maxLength={80} disabled={busy} onChange={event => setRenaming({ ...renaming, name: event.target.value })} />
        {error && <p role="alert" className="themis-form-error">{error}</p>}
        <div className="themis-sidebar-dialog-actions"><Button variant="ghost" disabled={busy} onClick={() => setRenaming(null)}>Cancel</Button><Button type="submit" disabled={busy || !renaming.name.trim()}>Save</Button></div>
      </form>}
    </Dialog>
    <Dialog open={deleting !== null} title="Delete project?" onClose={() => { if (!busy) setDeleting(null); }}>
      {deleting && <div className="themis-sidebar-dialog">
        <p className="themis-sidebar-dialog-hint">“{deleting.name}” and its chats will be removed from Themis. The project folder moves to the Trash, so its files can still be recovered there.</p>
        {error && <p role="alert" className="themis-form-error">{error}</p>}
        <div className="themis-sidebar-dialog-actions"><Button variant="ghost" disabled={busy} onClick={() => setDeleting(null)}>Cancel</Button><Button variant="danger" disabled={busy} onClick={async () => {
          setBusy(true); setError("");
          try { await deleteProject(deleting.root); dispatch({ type: "project/removed", root: deleting.root }); dispatch({ type: "settings/patched", settings: await getSettings() }); toast(dispatch, `Moved ${deleting.name} to the Trash`, "success"); setDeleting(null); }
          catch (reason) { setError(describeError(reason)); } finally { setBusy(false); }
        }}>{busy ? "Deleting…" : "Delete project"}</Button></div>
      </div>}
    </Dialog>
    <Dialog open={editing !== null} title="Edit thread" onClose={() => { if (!editBusy) setEditing(null); }}>
      {editing && <form className="themis-sidebar-dialog" onSubmit={event => { event.preventDefault(); void saveThread(); }}>
        <Input id="sidebar-thread-title" label="Name" value={editing.title} maxLength={120} onChange={event => setEditing({ ...editing, title: event.target.value })} />
        {editError && <p role="alert" className="themis-form-error">{editError}</p>}
        <div className="themis-sidebar-dialog-actions"><Button variant="ghost" disabled={editBusy} onClick={() => setEditing(null)}>Cancel</Button><Button type="submit" disabled={editBusy || !editing.title.trim()}>{editBusy ? "Saving…" : "Save"}</Button></div>
      </form>}
    </Dialog>
    <Dialog open={removing !== null} title="Remove thread?" onClose={() => { if (!editBusy) setRemoving(null); }}>
      <p>This permanently removes the chat and its history. Project files are not changed.</p>
      <div className="themis-sidebar-dialog-actions"><Button variant="ghost" onClick={() => setRemoving(null)} disabled={editBusy}>Cancel</Button><Button variant="danger" disabled={editBusy} onClick={async () => { if (!removing) return; setEditBusy(true); try { if (await discard(removing.projectRoot, removing.id)) setRemoving(null); } finally { setEditBusy(false); } }}>Remove thread</Button></div>
    </Dialog>
    <Dialog open={help} title="Shortcuts" onClose={() => setHelp(false)}><div className="themis-help"><dl>{SHORTCUTS.map(([label, keys]) => <div key={label}><dt>{label}</dt><dd>{keys.map(key => <kbd key={key}>{key}</kbd>)}</dd></div>)}</dl></div></Dialog>
  </>;
}

// Lucide SVG paths from the approved Framed rail visualization (ISC license).
export function SidebarIcon({ name }: { name: SidebarIconName }) {
  return <svg className="themis-sidebar-icon" viewBox={name === "help" || name === "search" ? "0 0 20 20" : "0 0 24 24"} fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false">
    {name === "chat" && <path d="M7.9 20A9 9 0 1 0 4 16.1L2 22Z" />}
    {name === "panel" && <><rect width="18" height="18" x="3" y="3" rx="2" /><path d="M9 3v18" /></>}
    {name === "thread" && <><path d="M12 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7" /><path d="M18.375 2.625a1 1 0 0 1 3 3l-9.013 9.014a2 2 0 0 1-.853.505l-2.873.84a.5.5 0 0 1-.62-.62l.84-2.873a2 2 0 0 1 .506-.852z" /></>}
    {name === "skills" && <><path d="M8 3v5M16 3v5M6 8h12v4a6 6 0 0 1-12 0ZM12 18v4" /></>}
    {name === "automations" && <><circle cx="12" cy="12" r="10" /><path d="M12 6v6h4" /></>}
    {name === "queue" && <><circle cx="18" cy="18" r="3" /><circle cx="6" cy="6" r="3" /><path d="M13 6h3a2 2 0 0 1 2 2v7" /><line x1="6" x2="6" y1="9" y2="21" /></>}
    {name === "rename" && <><path d="M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z" /><path d="m15 5 4 4" /></>}
    {name === "trash" && <><path d="M3 6h18" /><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6" /><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2" /><path d="M10 11v6M14 11v6" /></>}
    {name === "project" && <path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" />}
    {name === "settings" && <><path d="M14 17H5M19 7h-9" /><circle cx="17" cy="17" r="3" /><circle cx="7" cy="7" r="3" /></>}
    {name === "help" && <><circle cx="10" cy="10" r="7.5"/><path d="M7.9 7.4a2.2 2.2 0 1 1 3.8 1.5c-1.1 1-1.7 1.2-1.7 2.6"/><path d="M10 14.6h.01"/></>}
    {name === "search" && <><circle cx="8.5" cy="8.5" r="5.5"/><path d="m12.5 12.5 4.2 4.2"/></>}
  </svg>;
}

type ThreadState = "Working" | "Needs approval" | "Question" | "Goal paused" | "Goal blocked" | "Goal waiting" | "Failed" | "Stopped" | "Done" | "Idle" | "Interrupted";

/** What waits on the person in this chat, if anything: an approval box, else a question box. A destructive step Auto or
 *  YOLO asks about first (`ask_first`) shows as a question box, so it reads as a question here too. */
function waitingOn(state: ReturnType<typeof useApp>["state"], thread: ThreadInfo): "Needs approval" | "Question" | null {
  const approvals = state.approvals.filter(approval => approval.thread_id === thread.id);
  if (approvals.some(approval => !approval.ask_first)) return "Needs approval";
  if (approvals.length > 0) return "Question";
  const asking = (state.messages[thread.id] ?? []).some(row => row.question && row.question.answer === undefined)
    || (!state.historyLoaded[thread.id] && (thread.open_questions ?? 0) > 0);
  return asking ? "Question" : null;
}

export function threadStatus(state: ReturnType<typeof useApp>["state"], thread: ThreadInfo): ThreadState {
  const messages = state.messages[thread.id] ?? [];
  const last = messages[messages.length - 1];
  const running = state.running[thread.id] ?? thread.running;
  // What waits on the person comes first: an approval, then a question box, then a held goal.
  const waiting = waitingOn(state, thread);
  if (waiting) return waiting;
  const goal = thread.id in state.goals ? state.goals[thread.id]?.status : thread.goal_status;
  if (goal === "paused") return "Goal paused";
  if (goal === "blocked") return "Goal blocked";
  if (goal === "waiting") return "Goal waiting";
  // A chat not opened yet has no history loaded: its last run's end comes with the list.
  if (!running && !state.historyLoaded[thread.id] && !state.sendErrors[thread.id]) {
    return thread.last_outcome === "failed" ? "Failed"
      : thread.last_outcome === "stopped" ? "Stopped"
      : thread.last_outcome === "done" ? "Done"
      : thread.recovered ? "Interrupted" : "Idle";
  }
  return running
    ? "Working"
    : state.sendErrors[thread.id] || (last?.role === "system" && /^(Run failed:|Send failed:|Send rejected:)/.test(last.text)) ? "Failed"
    : last?.role === "system" && last.text.startsWith("Stopped by you.") ? "Stopped"
    : last?.incomplete ? "Idle"
    : last?.final ? "Done"
    : thread.recovered ? "Interrupted" : "Idle";
}

const SHORTCUTS: Array<[string, string[]]> = [
  ["Search chats, projects and commands", ["⌘", "K"]],
  ["New chat", ["⌘", "⇧", "O"]],
  ["Show or hide the sidebar", ["⌘", "B"]],
  ["Send", ["↵"]],
  ["New line", ["⇧", "↵"]],
  ["Close a panel or dialog", ["Esc"]],
];

/** Threads whose run ended this session and that have not been opened since. */
function useFreshlyFinished(running: Record<string, boolean>, viewing: string | null) {
  const previous = useRef(running);
  const [fresh, setFresh] = useState<ReadonlySet<string>>(new Set());
  useEffect(() => {
    const ended = Object.keys(previous.current).filter(id => previous.current[id] && !running[id] && id !== viewing);
    previous.current = running;
    if (ended.length) setFresh(current => new Set([...current, ...ended]));
  }, [running, viewing]);
  useEffect(() => {
    if (viewing) setFresh(current => { if (!current.has(viewing)) return current; const next = new Set(current); next.delete(viewing); return next; });
  }, [viewing]);
  return fresh;
}

const WAITING: readonly ThreadState[] = ["Needs approval", "Question", "Goal paused", "Goal blocked", "Goal waiting"];
const isWaiting = (status: ThreadState) => WAITING.includes(status);
const isLive = (status: ThreadState) => status === "Working" || isWaiting(status);

/** Lit cells: a pause sign while held for you, a question mark for a question, a cross for a failure. */
const PAUSE = [0, 2, 3, 5, 6, 8] as const;
const LIT: Partial<Record<ThreadState, readonly number[]>> = { "Needs approval": PAUSE, Question: [0, 1, 2, 5, 4, 7], "Goal paused": PAUSE, "Goal blocked": PAUSE, "Goal waiting": PAUSE, Failed: [0, 2, 4, 6, 8] };
const LABEL: Partial<Record<ThreadState, string>> = { Working: "Working", "Needs approval": "Waiting for your approval", Question: "Waiting for your answer", "Goal paused": "Goal paused", "Goal blocked": "Goal blocked", "Goal waiting": "Goal waiting for you", Failed: "Failed" };
/** Why a chat waits for you, on its row. */
const TAG: Partial<Record<ThreadState, string>> = { "Needs approval": "Approval", Question: "Question", "Goal paused": "Paused", "Goal blocked": "Blocked", "Goal waiting": "Answer" };

/** The chat's icon: dots for live work, waiting for you and failure; the speech bubble otherwise. */
function ThreadStatus({ status }: { status: ThreadState }) {
  const label = LABEL[status];
  if (!label) return <SidebarIcon name="chat" />;
  const lit = LIT[status];
  return <span role="img" aria-label={label} title={label} className={`themis-thread-status themis-thread-status--${status.toLowerCase().replace(/ /g, "-")}`}>
    {lit
      ? <svg viewBox="0 0 24 24" aria-hidden="true">{DOT_GRID.map(([cx, cy], cell) => <circle key={cell} className={lit.includes(cell) ? "is-lit" : undefined} cx={cx} cy={cy} r="2.1" />)}</svg>
      : <DotSpinner />}
  </span>;
}

/** The canvas's top row ends in a pencil: a new chat in the open project (the project menu beside it switches, renames,
 *  deletes and creates projects). Its name and shortcut are in the tooltip. */
function NewChatButton({ disabled, onClick }: { disabled: boolean; onClick: () => void }) {
  return (
    <Tooltip content="New chat (⌘/Ctrl+Shift+O)" below>
      <button type="button" className="themis-new-chat" aria-label="New chat" disabled={disabled} onClick={onClick}>
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M12 20h9M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z" /></svg>
      </button>
    </Tooltip>
  );
}
