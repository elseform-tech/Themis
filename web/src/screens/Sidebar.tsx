import { useEffect, useRef, useState } from "react";
import { Badge, Button, Dialog, Input } from "../components";
import { createProject, getSettings, renameThread, setProvider, setThreadSkills } from "../lib/tauri";
import { describeError, toast, useApp, type MainView } from "../state/store";
import { useNewThread, useOpenProject, useDiscardThread } from "./actions";
import type { ThreadInfo } from "../lib/types";
import { readSession, writeSession } from "../state/session";
import { KnightBrand } from "../components/KnightBrand";
import "./Sidebar.css";

const views: Array<[MainView, string]> = [["thread", "Threads"], ["skills", "Skills"], ["automations", "Automations"], ["queue", "Review queue"]];
type SidebarIconName = "panel" | "thread" | "skills" | "automations" | "queue" | "project" | "settings" | "help" | "search";
const viewIcons: Record<MainView, SidebarIconName> = { thread: "thread", skills: "skills", automations: "automations", queue: "queue", settings: "settings" };

export function Sidebar({ collapsed, onToggle }: { collapsed: boolean; onToggle: () => void }) {
  const { state, dispatch } = useApp();
  const openProject = useOpenProject();
  const newThread = useNewThread();
  const [revealed, setRevealed] = useState(false);
  const [name, setName] = useState("");
  const [directory, setDirectory] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const { discard } = useDiscardThread();
  const [editing, setEditing] = useState<(ThreadInfo & { projectRoot: string }) | null>(null);
  const [removing, setRemoving] = useState<(ThreadInfo & { projectRoot: string }) | null>(null);
  const [editError, setEditError] = useState("");
  const [editBusy, setEditBusy] = useState(false);
  const [help, setHelp] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const panel = useRef<HTMLElement>(null);
  const inside = useRef(false);
  const visible = !collapsed || revealed;
  const pending = state.reviews.filter(r => r.status === "pending").length;
  const [collapsedProjects, setCollapsedProjects] = useState<Record<string, boolean>>(() => readSession("collapsedProjects", {}));
  function setProjectCollapsed(root: string, value: boolean) {
    setCollapsedProjects(previous => { const next = { ...previous, [root]: value }; writeSession("collapsedProjects", next); return next; });
  }

  function clearTimer() { clearTimeout(timer.current); }
  function reveal() {
    clearTimer();
    if (collapsed && state.settings.sidebar_hover) timer.current = setTimeout(() => setRevealed(true), 300);
  }
  function hideLater() {
    clearTimer();
    timer.current = setTimeout(() => {
      if (!inside.current && !panel.current?.contains(document.activeElement)) setRevealed(false);
    }, 700);
  }
  useEffect(() => { clearTimer(); setRevealed(false); }, [collapsed, state.settings.sidebar_hover]);
  useEffect(() => () => clearTimeout(timer.current), []);
  useEffect(() => {
    function escape(event: KeyboardEvent) {
      if (event.key === "Escape" && collapsed && !state.projectDialogOpen && !help && !state.paletteOpen) {
        setRevealed(false);
        document.getElementById("sidebar-toggle")?.focus();
      }
    }
    document.addEventListener("keydown", escape);
    return () => document.removeEventListener("keydown", escape);
  }, [collapsed, help, state.projectDialogOpen, state.paletteOpen]);

  async function create() {
    if (busy || !name.trim()) return;
    setBusy(true); setError("");
    try {
      const project = await createProject(name.trim(), directory.trim() || undefined);
      dispatch({ type: "project/opened", project });
      dispatch({ type: "ui/project-dialog", open: false });
      setName(""); setDirectory("");
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
      let updated = await renameThread(editing.id, editing.title);
      updated = await setProvider(editing.id, "go", editing.model);
      updated = await setThreadSkills(editing.id, editing.skill_ids);
      dispatch({ type: "thread/updated", projectRoot: editing.projectRoot, thread: updated });
      setEditing(null);
    } catch (error) { setEditError(describeError(error)); }
    finally { setEditBusy(false); }
  }
  function nav(view: MainView, label: string) {
    return <button key={view} type="button" className={`themis-sidebar-row ${state.mainView === view ? "themis-sidebar-row--active" : ""}`} aria-label={view === "queue" && pending > 0 ? `${label} ${pending}` : label} title={label} aria-current={state.mainView === view ? "page" : undefined} onClick={() => dispatch({ type: "ui/view", view })}>
      <SidebarIcon name={viewIcons[view]} /><span className="themis-sidebar-name">{label}</span>{view === "queue" && pending > 0 && <Badge tone="warning">{pending}</Badge>}
    </button>;
  }
  return <>
    {collapsed && <div className="themis-sidebar-edge" data-testid="sidebar-edge" onPointerEnter={reveal} onPointerLeave={hideLater} />}
    <div className={`themis-sidebar-slot ${collapsed ? "is-collapsed" : ""}`}>
      {collapsed && <nav className="themis-sidebar-rail" aria-label="Compact workspace navigation">
        <button type="button" className="themis-rail-brand" aria-label="Expand sidebar" onClick={onToggle}><KnightBrand /></button>
        {views.map(([view, label]) => nav(view, label))}
        <button type="button" className="themis-sidebar-row" aria-label="New project" title="New project" onClick={() => dispatch({ type: "ui/project-dialog", open: true })}><SidebarIcon name="project" /></button>
        <div className="themis-rail-footer">{nav("settings", "Settings")}<button type="button" className="themis-sidebar-row" aria-label="Search commands" title="Search commands" onClick={() => dispatch({ type: "ui/palette", open: true })}><SidebarIcon name="search" /></button></div>
      </nav>}
      <aside id="themis-sidebar" ref={panel} className={`themis-sidebar ${collapsed ? "is-overlay" : ""} ${visible ? "is-visible" : ""}`} inert={!visible} aria-hidden={!visible} aria-label="Workspace navigation"
        onPointerEnter={() => { inside.current = true; clearTimer(); }} onPointerLeave={() => { inside.current = false; hideLater(); }}
        onFocusCapture={() => { clearTimer(); setRevealed(true); }} onBlurCapture={hideLater}>
        <div className="themis-brand"><KnightBrand wordmark /></div>
        <nav className="themis-sidebar-nav" aria-label="Main views">{views.map(([view, label]) => nav(view, label))}</nav>
        <div className="themis-sidebar-projects">
          <div className="themis-sidebar-head"><h2 className="themis-sidebar-title themis-sidebar-title-line"><SidebarIcon name="project" /><span>Projects</span></h2><div className="themis-project-actions"><Button size="small" variant="ghost" aria-label="New project" title="New project" onClick={() => dispatch({ type: "ui/project-dialog", open: true })}>+</Button><details><summary aria-label="Project actions" title="Project actions">···</summary><div className="themis-project-menu"><Button variant="ghost" size="small" onClick={event => { event.currentTarget.closest("details")?.removeAttribute("open"); void openProject(); }}>Open existing folder…</Button></div></details></div></div>
          {state.projects.length === 0 ? <p className="themis-sidebar-note">Create your first project to begin.</p> : <ul className="themis-sidebar-list">{state.projects.map(project => <li className="themis-project" key={project.root}>
            <div className="themis-project-heading"><button type="button" className="themis-sidebar-row themis-project-row" title={project.root} aria-expanded={!collapsedProjects[project.root]} onClick={() => setProjectCollapsed(project.root, !collapsedProjects[project.root])}><span aria-hidden="true">{collapsedProjects[project.root] ? "›" : "⌄"}</span><SidebarIcon name="project" /><span className="themis-sidebar-name">{project.name}</span></button><Button variant="ghost" size="small" aria-label={`New thread in ${project.name}`} title="New thread" onClick={() => { setProjectCollapsed(project.root, false); void newThread(project.root); }}>+</Button></div>
            {!collapsedProjects[project.root] && <ul className="themis-sidebar-list themis-thread-list">{(state.threadsByProject[project.root] ?? []).map(thread => <li key={thread.id} className="themis-sidebar-thread"><button type="button" className={`themis-sidebar-row themis-thread-row ${thread.id === state.activeThreadId && state.mainView === "thread" ? "themis-sidebar-row--active" : ""}`} title={thread.title} aria-current={thread.id === state.activeThreadId && state.mainView === "thread" ? "true" : undefined} onClick={() => dispatch({ type: "thread/selected", projectRoot: project.root, threadId: thread.id })}><SidebarIcon name="thread" /><span className="themis-sidebar-name">{thread.title}</span><ThreadStatus thread={thread} /></button><div className="themis-thread-row-actions"><button aria-label={`Edit ${thread.title}`} title="Edit thread" disabled={state.running[thread.id] ?? thread.running} onClick={() => { setEditError(""); setEditing({ ...thread, projectRoot: project.root }); }}>✎</button><button aria-label={`Remove ${thread.title}`} title="Remove thread" disabled={state.running[thread.id] ?? thread.running} onClick={() => setRemoving({ ...thread, projectRoot: project.root })}><svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" aria-hidden="true"><path d="M3 4h10M6 4V2h4v2M4 4l1 10h6l1-10M7 6v5M9 6v5"/></svg></button></div></li>)}</ul>}
          </li>)}</ul>}
        </div>
        <div className="themis-sidebar-footer">{nav("settings", "Settings")}<button className="themis-sidebar-row" onClick={() => setHelp(true)}><SidebarIcon name="help" /><span className="themis-sidebar-name">Help & shortcuts</span></button><button className="themis-sidebar-row themis-sidebar-note" onClick={() => dispatch({ type: "ui/palette", open: true })}><SidebarIcon name="search" /><span className="themis-sidebar-name">Search commands</span><kbd>⌘ / Ctrl K</kbd></button></div>
      </aside>
    </div>
    <Dialog open={state.projectDialogOpen} title="Create project" onClose={() => { if (!busy) dispatch({ type: "ui/project-dialog", open: false }); }}>
      <form className="themis-sidebar-dialog" onSubmit={event => { event.preventDefault(); void create(); }}>
        <Input id="project-name" label="Project name" placeholder="My project" value={name} maxLength={80} disabled={busy} onChange={e => { setName(e.target.value); setError(""); }} />
        <p className="themis-sidebar-dialog-hint">A new folder and Git repository will be created for your project.</p>
        <details><summary>Change location</summary><Input id="project-location" label="Parent folder" value={directory} placeholder={state.settings.projects_directory} onChange={e => setDirectory(e.target.value)} disabled={busy} /></details>
        <p className="themis-project-location">{directory || state.settings.projects_directory}/{name.trim() || "project-name"}</p>
        {error && <p role="alert" className="themis-form-error">{error}</p>}
        <div className="themis-sidebar-dialog-actions"><Button variant="ghost" disabled={busy} onClick={() => dispatch({ type: "ui/project-dialog", open: false })}>Cancel</Button><Button type="submit" variant="primary" disabled={busy || !name.trim()}>{busy ? "Creating…" : "Create project"}</Button></div>
      </form>
    </Dialog>
    <Dialog open={editing !== null} title="Edit thread" onClose={() => { if (!editBusy) setEditing(null); }}>
      {editing && <form className="themis-sidebar-dialog" onSubmit={event => { event.preventDefault(); void saveThread(); }}>
        <Input id="sidebar-thread-title" label="Name" value={editing.title} maxLength={120} onChange={event => setEditing({ ...editing, title: event.target.value })} />
        <Input id="sidebar-thread-model" label="Model" value={editing.model} onChange={event => setEditing({ ...editing, model: event.target.value })} />
        {state.skills.length > 0 && <fieldset><legend>Skills</legend>{state.skills.map(skill => <label key={skill.id}><input type="checkbox" checked={editing.skill_ids.includes(skill.id)} onChange={event => setEditing({ ...editing, skill_ids: event.target.checked ? [...editing.skill_ids, skill.id] : editing.skill_ids.filter(id => id !== skill.id) })}/>{skill.name}</label>)}</fieldset>}
        {editError && <p role="alert" className="themis-form-error">{editError}</p>}
        <div className="themis-sidebar-dialog-actions"><Button variant="ghost" disabled={editBusy} onClick={() => setEditing(null)}>Cancel</Button><Button type="submit" disabled={editBusy || !editing.title.trim()}>{editBusy ? "Saving…" : "Save"}</Button></div>
      </form>}
    </Dialog>
    <Dialog open={removing !== null} title="Remove thread?" onClose={() => { if (!editBusy) setRemoving(null); }}>
      <p>This permanently removes the thread and its worktree. Unmerged changes will be lost.</p>
      <div className="themis-sidebar-dialog-actions"><Button variant="ghost" onClick={() => setRemoving(null)} disabled={editBusy}>Cancel</Button><Button variant="danger" disabled={editBusy} onClick={async () => { if (!removing) return; setEditBusy(true); try { if (await discard(removing.projectRoot, removing.id)) setRemoving(null); } finally { setEditBusy(false); } }}>Remove thread</Button></div>
    </Dialog>
    <Dialog open={help} title="Help & shortcuts" onClose={() => setHelp(false)}><div className="themis-help"><p>Create a project, describe what you want, then review the result.</p><dl><dt>Find a project, thread or command</dt><dd>⌘ / Ctrl + K</dd><dt>Show or hide sidebar</dt><dd>⌘ / Ctrl + B</dd><dt>Send a message</dt><dd>Enter or ⌘ / Ctrl + Enter · Shift + Enter for a new line</dd><dt>New thread</dt><dd>⌘ / Ctrl + Shift + O</dd><dt>Close a panel or dialog</dt><dd>Escape</dd></dl><p>Agent changes stay in a separate worktree until you choose Merge. Actions requiring permission pause for your decision.</p></div></Dialog>
  </>;
}

// Lucide SVG paths from the approved Framed rail visualization (ISC license).
export function SidebarIcon({ name }: { name: SidebarIconName }) {
  return <svg className="themis-sidebar-icon" viewBox={name === "help" || name === "search" ? "0 0 20 20" : "0 0 24 24"} fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false">
    {name === "panel" && <><rect width="18" height="18" x="3" y="3" rx="2" /><path d="M9 3v18" /></>}
    {name === "thread" && <><path d="M12 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7" /><path d="M18.375 2.625a1 1 0 0 1 3 3l-9.013 9.014a2 2 0 0 1-.853.505l-2.873.84a.5.5 0 0 1-.62-.62l.84-2.873a2 2 0 0 1 .506-.852z" /></>}
    {name === "skills" && <><path d="M11.017 2.814a1 1 0 0 1 1.966 0l1.051 5.558a2 2 0 0 0 1.594 1.594l5.558 1.051a1 1 0 0 1 0 1.966l-5.558 1.051a2 2 0 0 0-1.594 1.594l-1.051 5.558a1 1 0 0 1-1.966 0l-1.051-5.558a2 2 0 0 0-1.594-1.594l-5.558-1.051a1 1 0 0 1 0-1.966l5.558-1.051a2 2 0 0 0 1.594-1.594z" /><path d="M20 2v4M22 4h-4" /><circle cx="4" cy="20" r="2" /></>}
    {name === "automations" && <><circle cx="12" cy="12" r="10" /><path d="M12 6v6h4" /></>}
    {name === "queue" && <><circle cx="18" cy="18" r="3" /><circle cx="6" cy="6" r="3" /><path d="M13 6h3a2 2 0 0 1 2 2v7" /><line x1="6" x2="6" y1="9" y2="21" /></>}
    {name === "project" && <path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" />}
    {name === "settings" && <><path d="M14 17H5M19 7h-9" /><circle cx="17" cy="17" r="3" /><circle cx="7" cy="7" r="3" /></>}
    {name === "help" && <><circle cx="10" cy="10" r="7.5"/><path d="M7.9 7.4a2.2 2.2 0 1 1 3.8 1.5c-1.1 1-1.7 1.2-1.7 2.6"/><path d="M10 14.6h.01"/></>}
    {name === "search" && <><circle cx="8.5" cy="8.5" r="5.5"/><path d="m12.5 12.5 4.2 4.2"/></>}
  </svg>;
}

function ThreadStatus({ thread }: { thread: ThreadInfo }) {
  const { state } = useApp();
  const messages = state.messages[thread.id] ?? [];
  const last = messages[messages.length - 1];
  const running = state.running[thread.id] ?? thread.running;
  const status = running
    ? state.approvals.some(approval => approval.thread_id === thread.id) ? "Needs approval" : "Working"
    : state.sendErrors[thread.id] || (last?.role === "system" && /^(Run failed:|Send failed:|Send rejected:)/.test(last.text)) ? "Failed"
    : last?.role === "system" && last.text.startsWith("Stopped by you.") ? "Stopped"
    : last?.incomplete ? "Idle"
    : last?.final ? "Done"
    : thread.recovered ? "Interrupted" : "Idle";
  return <span role="img" aria-label={status} title={status} className={`themis-thread-status themis-thread-status--${status.toLowerCase().replace(/ /g, "-")}`}>
    {status === "Working" ? <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6" /></svg> : status === "Done" ? "✓" : status === "Failed" || status === "Needs approval" ? "!" : status === "Stopped" || status === "Interrupted" ? "■" : "·"}
  </span>;
}
