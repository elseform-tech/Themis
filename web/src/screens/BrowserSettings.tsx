import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Button, Switch } from "../components";
import type { BrowserHistoryEntry, BrowserSettings, BrowserSettingsView, SiteAccess, SiteException } from "../lib/types";
import { browserHistory, clearBrowsingData, getBrowserSettings, removeBrowserHistory, updateBrowserSettings } from "../lib/tauri";
import { browserOpenUrl } from "../lib/browser";
import { describeError, toast, useApp } from "../state/store";
import { ResetIcon, SettingsGroup, SettingsIconButton } from "./SettingsGroup";
import "./BrowserSettings.css";

const PERMISSIONS = [
  { key: "browsing", label: "Browsing", hint: "Open, click, type, screenshots and recording" },
  { key: "downloads", label: "Downloads", hint: "Save files a page downloads" },
  { key: "debugging", label: "Debugging", hint: "Console, network, storage, breakpoints, profiling" },
] as const;
type PermissionKey = (typeof PERMISSIONS)[number]["key"];
const ACCESS: SiteAccess[] = ["allow", "ask", "block"];
const label = (access: SiteAccess) => access[0].toUpperCase() + access.slice(1);

/** Settings › Browser: what the agent may do in its built-in browser, Codex-style. */
export function BrowserSettingsSection() {
  const { state, dispatch } = useApp();
  const [view, setView] = useState<BrowserSettingsView | null>(null);
  const [error, setError] = useState("");
  const [newSite, setNewSite] = useState("");
  // Re-read when the window comes back, so edits made in runtime.jsonc meanwhile show.
  useEffect(() => {
    const load = () => { void getBrowserSettings().then(setView).catch(reason => setError(describeError(reason))); };
    load();
    window.addEventListener("focus", load);
    return () => window.removeEventListener("focus", load);
  }, []);
  if (error && !view) return <p role="alert" className="themis-form-error">{error}</p>;
  if (!view) return null;
  const settings = view.settings;

  async function save(changes: Partial<BrowserSettings>) {
    if (!view) return;
    setError("");
    try {
      const saved = await updateBrowserSettings(changes, view.revision);
      setView(current => current && { ...current, ...saved });
      dispatch({ type: "browser/opens-workspace", value: saved.settings.open_workspace });
    } catch (reason) {
      setError(describeError(reason));
      // The message sits at the top of a long page; say it where the person is looking too.
      toast(dispatch, describeError(reason), "warning");
      // Changed in the file meanwhile (or refused): show what is saved now.
      void getBrowserSettings().then(setView).catch(() => {});
    }
  }
  const patch = (changes: Partial<BrowserSettings>) => void save(changes);
  const setException = (site: string, key: PermissionKey, value: SiteAccess | "") => {
    const exceptions = settings.site_exceptions.map(entry => {
      if (entry.site !== site) return entry;
      const next: SiteException = { ...entry };
      if (value === "") delete next[key];
      else next[key] = value;
      return next;
    });
    patch({ site_exceptions: exceptions });
  };
  const addSite = () => {
    const site = newSite.trim().toLowerCase().replace(/^https?:\/\//, "").replace(/\/.*$/, "");
    if (!site || settings.site_exceptions.some(entry => entry.site === site)) { setNewSite(""); return; }
    patch({ site_exceptions: [...settings.site_exceptions, { site, browsing: "block" }] });
    setNewSite("");
  };
  const starter = view.starter_sites.map(site => ({ site, browsing: "block" as SiteAccess }));
  const chooseBrowser = async () => {
    const picked = await open({ multiple: false, directory: false, title: "Choose a Chrome, Chromium, Edge or Brave program" });
    if (typeof picked === "string") patch({ executable: picked });
  };
  const chooseFolder = async () => {
    const picked = await open({ multiple: false, directory: true, title: "Choose where downloads are saved" });
    if (typeof picked === "string") patch({ downloads_directory: picked });
  };

  return <>
    {error && <p role="alert" className="themis-form-error">{error}</p>}
    <SettingsGroup title="Built-in browser">
      <label className="themis-settings-check">Use the built-in browser<Switch checked={settings.enabled} onChange={event => patch({ enabled: event.target.checked })} /></label>
      {settings.enabled && <label className="themis-settings-control">Browser
        <span className="themis-browser-pick">
          <select aria-label="Browser" value={settings.executable ?? ""} onChange={event => patch({ executable: event.target.value || undefined })}>
            <option value="">{view.browsers[0] ? `Automatic (${programName(view.browsers[0])})` : "None found"}</option>
            {view.browsers.map(path => <option key={path} value={path}>{programName(path)}</option>)}
            {settings.executable && !view.browsers.includes(settings.executable) && <option value={settings.executable}>{programName(settings.executable)}</option>}
          </select>
          <button type="button" className="themis-text-action" onClick={() => void chooseBrowser()}>Choose…</button>
        </span>
      </label>}
    </SettingsGroup>

    <SettingsGroup title="Agent permissions" hint="Allow runs without asking, Ask shows a prompt (YOLO allows it), Block is refused in every mode. Local apps are always allowed.">
      {PERMISSIONS.map(permission => {
        const disabled = permission.key === "debugging" && !settings.developer_mode;
        return <label key={permission.key} className="themis-settings-control" title={permission.hint}>{permission.label}
          <select aria-label={permission.label} disabled={disabled} value={settings.permissions[permission.key]} onChange={event => patch({ permissions: { ...settings.permissions, [permission.key]: event.target.value as SiteAccess } })}>
            {ACCESS.map(access => <option key={access} value={access}>{label(access)}</option>)}
          </select>
        </label>;
      })}
    </SettingsGroup>

    <SettingsGroup title="Site exceptions" hint="A site covers its subdomains. The starter list blocks a few sign-in and payment sites; it is not complete." action={<SettingsIconButton label="Restore defaults" onClick={() => patch({ site_exceptions: [...starter, ...settings.site_exceptions.filter(entry => !view.starter_sites.includes(entry.site))] })}><ResetIcon /></SettingsIconButton>}>
      {settings.site_exceptions.length > 0 && <table className="themis-site-exceptions" aria-label="Site exceptions">
        <thead><tr><th scope="col">Site</th>{PERMISSIONS.map(p => <th key={p.key} scope="col">{p.label}</th>)}<th scope="col"><span className="themis-visually-hidden">Remove</span></th></tr></thead>
        <tbody>
          {settings.site_exceptions.map(entry => <tr key={entry.site}>
            <th scope="row">{entry.site}</th>
            {PERMISSIONS.map(permission => <td key={permission.key}>
              <select aria-label={`${permission.label} on ${entry.site}`} className={entry[permission.key] ? undefined : "is-default"} value={entry[permission.key] ?? ""} onChange={event => setException(entry.site, permission.key, event.target.value as SiteAccess | "")}>
                <option value="">Default</option>
                {ACCESS.map(access => <option key={access} value={access}>{label(access)}</option>)}
              </select>
            </td>)}
            <td><button type="button" className="themis-site-remove" aria-label={`Remove ${entry.site}`} title="Remove" onClick={() => patch({ site_exceptions: settings.site_exceptions.filter(other => other.site !== entry.site) })}>×</button></td>
          </tr>)}
        </tbody>
      </table>}
      <div className="themis-site-add">
        <input aria-label="Add a site" placeholder="Add a site, e.g. example.com" value={newSite} onChange={event => setNewSite(event.target.value)} onKeyDown={event => { if (event.key === "Enter") { event.preventDefault(); addSite(); } }} />
        <button type="button" className="themis-text-action" disabled={!newSite.trim()} onClick={addSite}>Add site</button>
      </div>
    </SettingsGroup>

    <SettingsGroup title="Developer mode">
      <label className="themis-settings-check" title="The agent's console, elements, network, debugger, storage and profiling tools">Enable full CDP access<Switch checked={settings.developer_mode} onChange={event => patch({ developer_mode: event.target.checked })} /></label>
      <label className="themis-settings-check" title="Cookie values, auth headers and token-like values; httpOnly cookies are never shown">Show secrets to the agent<Switch checked={settings.show_secrets} onChange={event => patch({ show_secrets: event.target.checked })} /></label>
    </SettingsGroup>

    <SettingsGroup title="Downloads">
      <div className="themis-settings-control"><span className="themis-browser-label">Save to</span>
        <span className="themis-browser-pick">
          <span className="themis-settings-root" title={settings.downloads_directory ?? view.default_downloads_directory}>{settings.downloads_directory ?? view.default_downloads_directory}</span>
          <button type="button" className="themis-text-action" onClick={() => void chooseFolder()}>Change…</button>
          {settings.downloads_directory && <SettingsIconButton label="Reset" onClick={() => patch({ downloads_directory: undefined })}><ResetIcon /></SettingsIconButton>}
        </span>
      </div>
      <label className="themis-settings-check">Open Workspace when the agent starts browsing<Switch checked={settings.open_workspace} onChange={event => patch({ open_workspace: event.target.checked })} /></label>
    </SettingsGroup>

    <BrowserData projects={state.projects.map(project => ({ root: project.root, name: project.name }))} threadId={state.activeThreadId} onError={message => toast(dispatch, message, "danger")} />
  </>;
}

function programName(path: string) {
  const parts = path.split(/[\\/]/);
  const app = parts.find(part => part.endsWith(".app"));
  return (app ?? parts[parts.length - 1]).replace(/\.(app|exe)$/, "");
}

/** History search, reopen and remove, and clearing browsing data. */
function BrowserData({ projects, threadId, onError }: { projects: Array<{ root: string; name: string }>; threadId: string | null; onError: (message: string) => void }) {
  const [query, setQuery] = useState("");
  const [entries, setEntries] = useState<BrowserHistoryEntry[] | null>(null);
  const [project, setProject] = useState("");
  const [range, setRange] = useState<"hour" | "day" | "week" | "all">("hour");
  const [types, setTypes] = useState<Record<"history" | "cookies" | "cache", boolean>>({ history: true, cookies: false, cache: true });
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [managing, setManaging] = useState(false);
  const nameOf = (root: string) => projects.find(p => p.root === root)?.name ?? root.split(/[\\/]/).pop();
  const search = async () => {
    try { setEntries((await browserHistory(project || null, query)).entries); } catch (reason) { onError(describeError(reason)); }
  };
  const [removing, setRemoving] = useState<string | null>(null);
  // Removing closes that project's browser first, which takes a few seconds.
  const remove = async (entry: BrowserHistoryEntry) => {
    setRemoving(`${entry.project}:${entry.url}`);
    try { await removeBrowserHistory(entry.project, entry.url); setEntries(current => current?.filter(other => other !== entry) ?? null); } catch (reason) { onError(describeError(reason)); } finally { setRemoving(null); }
  };
  const clear = async () => {
    setBusy(true);
    try {
      await clearBrowsingData(project || null, range, (Object.keys(types) as Array<keyof typeof types>).filter(type => types[type]));
      setConfirming(false);
      if (entries) await search();
    } catch (reason) { onError(describeError(reason)); } finally { setBusy(false); }
  };
  return <SettingsGroup title="History and data">
    <div className="themis-settings-control"><span>Browsing history, cookies and cache</span><button type="button" className="themis-text-action" aria-expanded={managing} onClick={() => setManaging(value => !value)}>{managing ? "Done" : "Manage…"}</button></div>
    {managing && <>
    <label className="themis-settings-control">Project
      <select aria-label="Project" value={project} onChange={event => { setProject(event.target.value); setEntries(null); }}>
        <option value="">All projects</option>
        {projects.map(p => <option key={p.root} value={p.root}>{p.name}</option>)}
      </select>
    </label>
    <div className="themis-site-add">
      <input aria-label="Search history" placeholder="Search history" value={query} onChange={event => setQuery(event.target.value)} onKeyDown={event => { if (event.key === "Enter") { event.preventDefault(); void search(); } }} />
      <button type="button" className="themis-text-action" onClick={() => void search()}>{entries ? "Search" : "Show history"}</button>
    </div>
    {entries && (entries.length === 0 ? <p className="themis-settings-hint">No history.</p> : <ul className="themis-browser-history" aria-label="Browser history">
      {entries.map(entry => <li key={`${entry.project}:${entry.url}`}>
        <span className="themis-browser-history-text"><span>{entry.title || entry.url}</span><small title={entry.url}>{entry.url} · {new Date(entry.last_visit * 1000).toLocaleString()}{projects.length > 1 && !project ? ` · ${nameOf(entry.project)}` : ""}</small></span>
        {threadId && <button type="button" className="themis-text-action" onClick={() => void browserOpenUrl(threadId, entry.url).catch(reason => onError(describeError(reason)))}>Reopen</button>}
        <button type="button" className="themis-text-action" aria-label={`Remove ${entry.url}`} disabled={removing !== null} onClick={() => void remove(entry)}>{removing === `${entry.project}:${entry.url}` ? "Removing…" : "Remove"}</button>
      </li>)}
    </ul>)}
    <div className="themis-browser-clear">
      <select aria-label="Time range" value={range} onChange={event => setRange(event.target.value as typeof range)}>
        <option value="hour">Last hour</option><option value="day">Last day</option><option value="week">Last week</option><option value="all">All time</option>
      </select>
      {(["history", "cookies", "cache"] as const).map(type => <label key={type} className="themis-browser-clear-type"><input type="checkbox" checked={types[type]} onChange={event => setTypes(current => ({ ...current, [type]: event.target.checked }))} />{type === "history" ? "History" : type === "cookies" ? "Cookies and site data" : "Cached files"}</label>)}
      {confirming
        ? <span className="themis-browser-pick">{busy ? "Clearing…" : <>This closes {project ? nameOf(project) : "every project"}'s browser{types.cookies ? " and signs the agent out" : ""}.</>}<Button variant="danger" size="small" disabled={busy} onClick={() => void clear()}>Clear</Button><button type="button" className="themis-text-action" onClick={() => setConfirming(false)}>Cancel</button></span>
        : <button type="button" className="themis-text-action" disabled={!Object.values(types).some(Boolean)} onClick={() => setConfirming(true)}>Clear browsing data…</button>}
    </div>
    </>}
  </SettingsGroup>;
}
