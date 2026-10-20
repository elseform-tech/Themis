import { useCallback, useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { DiffView, EmptyState, FileTree } from "../components";
import { CodeBlock } from "../components/CodeBlock";
import type { DirEntry, FileView, WorkspaceInfo } from "../lib/types";
import { getWorkspace, listDiff, listProjectDir, readProjectFile } from "../lib/tauri";
import { describeError, useApp } from "../state/store";
import { readSession, writeSession } from "../state/session";
import { BrowserView } from "./BrowserView";
import { TasksView } from "./TasksView";
import { countTasks } from "./threadTasks";
import "./WorkspacePanel.css";

type Tab = "changes" | "files" | "branches" | "pulls" | "tasks" | "browser";
const TABS: Array<[Tab, string]> = [["changes", "Changes"], ["files", "Files"], ["branches", "Branches"], ["pulls", "Pull requests"], ["tasks", "Tasks"], ["browser", "Browser"]];
const MIN_WIDTH = 300;
const MAX_WIDTH = 1600;
const DEFAULT_WIDTH = 560;
/** The Browser tab needs room for a page; it remembers its own width. */
const BROWSER_DEFAULT_WIDTH = 760;
/** The Changes file list beside the diff; the diff keeps at least DIFF_MIN_WIDTH (CSS). */
const LIST_MIN_WIDTH = 120;
const LIST_MAX_WIDTH = 640;
const LIST_DEFAULT_WIDTH = 200;

function clampListWidth(value: number) {
  return Math.round(Math.max(LIST_MIN_WIDTH, Math.min(LIST_MAX_WIDTH, value)));
}

function clampWidth(value: number) {
  return Math.round(Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, value)));
}

/** Read-only monitor of what agents change (diffs, files, branches, PRs), Stop and Pause for this chat's tasks, and the chat's built-in browser. */
export function WorkspacePanel({ projectRoot, threadId, workspace }: { projectRoot: string; threadId: string; workspace: WorkspaceInfo | null }) {
  const { state, dispatch } = useApp();
  const [tab, setTab] = useState<Tab>(() => state.workspaceTabRequest ? state.workspaceTabRequest.tab : readSession<Tab>("workspaceTab", "changes"));
  const widthKey = tab === "browser" ? "workspaceBrowserWidth" : "workspacePanelWidth";
  const defaultWidth = tab === "browser" ? BROWSER_DEFAULT_WIDTH : DEFAULT_WIDTH;
  const [widths, setWidths] = useState<Record<string, number>>({});
  const width = widths[widthKey] ?? (clampWidth(Number(readSession(widthKey, defaultWidth)) || defaultWidth));
  const setWidth = (value: number) => setWidths(previous => ({ ...previous, [widthKey]: value }));
  const panel = useRef<HTMLElement>(null);
  const drag = useRef<{ x: number; width: number } | null>(null);
  const [dragging, setDragging] = useState(false);
  function resize(next: number) { const value = clampWidth(next); setWidth(value); writeSession(widthKey, value); }
  // While dragging only the element width changes; state and storage update on release.
  const lastWidth = useRef<number | null>(null);
  function preview(clientX: number) { if (!drag.current || !panel.current) return; lastWidth.current = clampWidth(drag.current.width + drag.current.x - clientX); panel.current.style.width = `${lastWidth.current}px`; }
  // Commit what is visible: CSS keeps room for the conversation, so store the rendered width.
  function finish() { if (drag.current && lastWidth.current !== null) resize(Math.min(lastWidth.current, panel.current?.getBoundingClientRect().width || lastWidth.current)); drag.current = null; lastWidth.current = null; setDragging(false); }
  function choose(next: Tab) { setTab(next); writeSession("workspaceTab", next); }
  // A request to show a changed file switches to Changes.
  const focusAt = state.diffFocus?.at;
  useEffect(() => { if (focusAt) setTab("changes"); }, [focusAt]);
  // The header's task indicator asks for the Tasks tab.
  const tabRequestAt = state.workspaceTabRequest?.at;
  // The request is applied once, so the remembered tab wins the next time the panel opens.
  useEffect(() => { if (tabRequestAt && state.workspaceTabRequest) { choose(state.workspaceTabRequest.tab); dispatch({ type: "ui/tab-request-consumed" }); } }, [tabRequestAt]); // eslint-disable-line react-hooks/exhaustive-deps
  const diff = state.diffs[threadId];
  const changed = diff?.available ? diff.files.length : 0;
  const counts = countTasks(state.tasks[threadId], state.settings.automations_enabled);
  const taskCount = counts.attention + counts.running + counts.scheduled;
  return (
    <aside ref={panel} className={`themis-workspace-panel${dragging ? " is-resizing" : ""}`} aria-label="Workspace (read-only)" style={{ width }}>
      <div className="themis-workspace-resize" role="separator" aria-label="Resize workspace panel" aria-orientation="vertical" aria-valuemin={MIN_WIDTH} aria-valuemax={MAX_WIDTH} aria-valuenow={width} tabIndex={0}
        // WebKit starts a text selection on mousedown even when pointerdown is cancelled.
        onMouseDown={event => event.preventDefault()}
        onPointerDown={event => { if (event.button !== 0) return; event.preventDefault(); drag.current = { x: event.clientX, width: panel.current?.getBoundingClientRect().width ?? width }; setDragging(true); event.currentTarget.setPointerCapture(event.pointerId); }}
        onPointerMove={event => preview(event.clientX)}
        onPointerUp={finish}
        onLostPointerCapture={finish}
        onDoubleClick={() => resize(defaultWidth)}
        onKeyDown={event => { if (event.key === "ArrowLeft") { event.preventDefault(); resize(width + 40); } else if (event.key === "ArrowRight") { event.preventDefault(); resize(width - 40); } }} />
      <header className="themis-workspace-head">
        <div className="themis-workspace-where">
          <strong>{workspace?.name ?? "Workspace"}</strong>
          {workspace?.branch && <span className="themis-workspace-branch"><BranchIcon />{workspace.branch}</span>}
        </div>
        <button type="button" className="themis-link-button" aria-label="Close workspace" onClick={() => dispatch({ type: "ui/diff-panel", open: false })}>✕</button>
      </header>
      <nav className="themis-workspace-tabs" aria-label="Workspace views">
        {TABS.map(([value, label]) => (
          <button key={value} type="button" aria-pressed={tab === value} onClick={() => choose(value)}>
            {label}{value === "changes" && changed > 0 ? <span className="themis-workspace-count">{changed}</span> : null}
            {value === "tasks" && taskCount > 0 ? <span className="themis-workspace-count themis-workspace-count--tasks">{taskCount}</span> : null}
          </button>
        ))}
      </nav>
      <div className="themis-workspace-body">
        {tab === "changes" && <ChangesView threadId={threadId} />}
        {tab === "files" && <FilesView projectRoot={projectRoot} />}
        {tab === "branches" && <BranchesView workspace={workspace} />}
        {tab === "pulls" && <PullRequestsView projectRoot={projectRoot} />}
        {tab === "tasks" && <TasksView threadId={threadId} />}
        {tab === "browser" && <BrowserView threadId={threadId} />}
      </div>
    </aside>
  );
}

export function BranchIcon() {
  return <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" aria-hidden="true"><circle cx="6" cy="5" r="2" /><circle cx="6" cy="19" r="2" /><circle cx="18" cy="7" r="2" /><path d="M6 7v10M18 9c0 5-6 4-11 8" /></svg>;
}

function ChangesView({ threadId }: { threadId: string }) {
  const { state, dispatch } = useApp();
  const diff = state.diffs[threadId];
  const running = state.running[threadId] ?? false;
  const toolsDone = (state.traces[threadId] ?? []).filter(entry => entry.ok !== undefined).length;
  const [selected, setSelected] = useState<string | null>(null);
  // An edited-files row asks for its file; follow each new request.
  const focus = state.diffFocus;
  useEffect(() => { if (focus) setSelected(focus.path); }, [focus]);
  const refresh = useCallback(async () => {
    // A failed refresh keeps the last diff on screen.
    try { dispatch({ type: "diff/set", threadId, diff: await listDiff(threadId) }); } catch { /* keep last */ }
  }, [dispatch, threadId]);
  // Live: refresh on open, after each completed tool call and when a run ends.
  useEffect(() => { void refresh(); }, [refresh, toolsDone, running]);
  // Shell commands can change files without a file-tool event, so poll during a run.
  useEffect(() => {
    if (!running) return;
    const timer = setInterval(() => void refresh(), 4000);
    return () => clearInterval(timer);
  }, [running, refresh]);
  if (diff === undefined) return <EmptyState title="Loading changes…" />;
  if (diff === null || !diff.available) return <EmptyState title="Changes unavailable" hint={diff?.reason ?? "This folder has no Git diff."} />;
  const files = diff.files;
  const file = files.find(entry => entry.path === selected) ?? files[0] ?? null;
  return (
    <div className="themis-workspace-changes">
      {diff.reason && <p className="themis-workspace-note">{diff.reason}</p>}
      {file === null ? <EmptyState title="No changes" hint="The checkout matches its last commit." /> : (
        <DiffSplit>
          <FileTree files={files} selectedPath={file.path} onSelect={setSelected} />
          <DiffView file={file} />
        </DiffSplit>
      )}
    </div>
  );
}

/** File list | diff, with a draggable divider whose position is remembered. */
export function DiffSplit({ children }: { children: [ReactNode, ReactNode] }) {
  const [width, setWidth] = useState(() => clampListWidth(Number(readSession("workspaceDiffListWidth", LIST_DEFAULT_WIDTH)) || LIST_DEFAULT_WIDTH));
  const split = useRef<HTMLDivElement>(null);
  const drag = useRef<{ x: number; width: number } | null>(null);
  const [dragging, setDragging] = useState(false);
  function resize(next: number) { const value = clampListWidth(next); setWidth(value); writeSession("workspaceDiffListWidth", value); }
  // Store what is visible: CSS keeps room for the diff, so a wide value may render narrower.
  function rendered() { return split.current?.firstElementChild?.getBoundingClientRect().width || width; }
  return (
    <div ref={split} className={`themis-workspace-split${dragging ? " is-resizing" : ""}`} style={{ "--themis-diff-list": `${width}px` } as CSSProperties}>
      {children[0]}
      <div className="themis-split-resize" role="separator" aria-label="Resize file list" aria-orientation="vertical" aria-valuemin={LIST_MIN_WIDTH} aria-valuemax={LIST_MAX_WIDTH} aria-valuenow={width} tabIndex={0}
        // WebKit starts a text selection on mousedown even when pointerdown is cancelled.
        onMouseDown={event => event.preventDefault()}
        onPointerDown={event => { if (event.button !== 0) return; event.preventDefault(); drag.current = { x: event.clientX, width: rendered() }; setDragging(true); event.currentTarget.setPointerCapture(event.pointerId); }}
        onPointerMove={event => { if (drag.current) setWidth(clampListWidth(drag.current.width + event.clientX - drag.current.x)); }}
        onPointerUp={() => { if (drag.current) resize(Math.min(width, rendered())); drag.current = null; setDragging(false); }}
        onLostPointerCapture={() => { drag.current = null; setDragging(false); }}
        onDoubleClick={() => resize(LIST_DEFAULT_WIDTH)}
        onKeyDown={event => { if (event.key === "ArrowLeft") { event.preventDefault(); resize(width - 40); } else if (event.key === "ArrowRight") { event.preventDefault(); resize(width + 40); } }} />
      {children[1]}
    </div>
  );
}

function FilesView({ projectRoot }: { projectRoot: string }) {
  const [listings, setListings] = useState<Record<string, DirEntry[] | string>>({});
  const [open, setOpen] = useState<Record<string, boolean>>({ "": true });
  const [file, setFile] = useState<FileView | null>(null);
  const [error, setError] = useState("");
  const load = useCallback(async (path: string) => {
    try { const listing = await listProjectDir(projectRoot, path); setListings(previous => ({ ...previous, [path]: listing.entries })); }
    catch (reason) { setListings(previous => ({ ...previous, [path]: describeError(reason) })); }
  }, [projectRoot]);
  useEffect(() => { setListings({}); setOpen({ "": true }); setFile(null); void load(""); }, [load]);
  async function view(path: string) {
    setError("");
    try { setFile(await readProjectFile(projectRoot, path)); }
    catch (reason) { setError(describeError(reason)); }
  }
  function toggle(path: string) {
    setOpen(previous => ({ ...previous, [path]: !previous[path] }));
    if (listings[path] === undefined) void load(path);
  }
  function renderDir(path: string, depth: number): React.ReactNode {
    const entries = listings[path];
    if (entries === undefined) return <li className="themis-files-note" style={{ paddingLeft: depth * 14 + 8 }}>Loading…</li>;
    if (typeof entries === "string") return <li className="themis-files-note" style={{ paddingLeft: depth * 14 + 8 }}>{entries}</li>;
    return entries.map(entry => (
      <li key={entry.path}>
        <button type="button" className={`themis-files-row${file?.path === entry.path ? " is-selected" : ""}`} style={{ paddingLeft: depth * 14 + 8 }}
          aria-expanded={entry.is_dir ? !!open[entry.path] : undefined}
          onClick={() => entry.is_dir ? toggle(entry.path) : void view(entry.path)}>
          <span aria-hidden="true" className="themis-files-glyph">{entry.is_dir ? (open[entry.path] ? "⌄" : "›") : ""}</span>
          <span className="themis-files-name">{entry.name}{entry.is_dir ? "/" : ""}</span>
        </button>
        {entry.is_dir && open[entry.path] && <ul>{renderDir(entry.path, depth + 1)}</ul>}
      </li>
    ));
  }
  const language = file?.path.split(".").pop();
  return (
    <div className="themis-workspace-split">
      <ul className="themis-files-tree" aria-label="Project files">{renderDir("", 0)}</ul>
      <div className="themis-files-view">
        {error && <p className="themis-workspace-note">{error}</p>}
        {file === null ? <EmptyState title="Pick a file" hint="Files open read-only." />
          : file.binary ? <EmptyState title={file.path} hint="Binary file; not shown." />
          : <>
              <div className="themis-files-path">{file.path}{file.truncated ? " · first 256 KB" : ""}</div>
              <CodeBlock code={file.content ?? ""} language={language} showLineNumbers />
            </>}
      </div>
    </div>
  );
}

function BranchesView({ workspace }: { workspace: WorkspaceInfo | null }) {
  if (workspace === null) return <EmptyState title="Loading branches…" />;
  if (!workspace.is_git) return <EmptyState title="Not a Git repository" />;
  if (workspace.branches.length === 0) return <EmptyState title="No branches yet" hint="Branches appear after the first commit." />;
  return (
    <ul className="themis-workspace-list" aria-label="Branches">
      {workspace.branches.map(branch => (
        <li key={branch.name} className={branch.current ? "is-current" : undefined}>
          <span className="themis-workspace-list-title"><BranchIcon />{branch.name}{branch.current && <em>current</em>}</span>
          <span className="themis-workspace-list-meta">{[branch.upstream, branch.track, branch.updated].filter(Boolean).join(" · ")}</span>
        </li>
      ))}
    </ul>
  );
}

function PullRequestsView({ projectRoot }: { projectRoot: string }) {
  const [workspace, setWorkspace] = useState<WorkspaceInfo | null>(null);
  const [error, setError] = useState("");
  useEffect(() => {
    let cancelled = false;
    setWorkspace(null); setError("");
    getWorkspace(projectRoot, true).then(info => { if (!cancelled) setWorkspace(info); }).catch(reason => { if (!cancelled) setError(describeError(reason)); });
    return () => { cancelled = true; };
  }, [projectRoot]);
  if (error) return <EmptyState title="Pull requests unavailable" hint={error} />;
  if (workspace === null) return <EmptyState title="Loading pull requests…" />;
  if (workspace.pull_requests_unavailable) return <EmptyState title="Pull requests unavailable" hint={workspace.pull_requests_unavailable} />;
  if (workspace.pull_requests.length === 0) return <EmptyState title="No open pull requests" />;
  return (
    <ul className="themis-workspace-list" aria-label="Pull requests">
      {workspace.pull_requests.map(pr => (
        <li key={pr.number}>
          <span className="themis-workspace-list-title">#{pr.number} {pr.title}{pr.draft && <em>draft</em>}</span>
          <span className="themis-workspace-list-meta"><BranchIcon />{pr.branch} · <span className="themis-workspace-url">{pr.url}</span></span>
        </li>
      ))}
    </ul>
  );
}
