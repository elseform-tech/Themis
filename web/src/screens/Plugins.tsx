import { useCallback, useEffect, useRef, useState } from "react";
import { Button, Dialog, EmptyState } from "../components";
import { KnightLoading, readCompanionPreferences } from "../components/KnightCompanion";
import { ResponseBody } from "../components/ResponseBody";
import { pluginAction } from "../lib/tauri";
import { isPluginPackage } from "../lib/integrations";
import type { ImportPreview, Marketplace, McpConnections, Plugin, PluginSpec } from "../lib/types";
import { describeError, toast, useApp } from "../state/store";
import "./Plugins.css";
import { McpFileRow } from "./McpFile";
import { HooksByEvent, skipReason } from "./HooksByEvent";
import { SourcesDialog, sourceLabel } from "./Sources";

function integrationError(error: unknown): string {
  return describeError(error).replace(/\nDiagnostic operation: [0-9a-f-]{36}$/, "").replace("No such file or directory (os error 2)", "Plugin files are missing.");
}

type Category = "Plugins" | "Skills" | "MCP" | "Hooks";
type Item = { key: string; name: string; plugin: Plugin; kind: "plugin" | "skill" | "mcp" | "hook"; id: string; enabled: boolean; unavailable?: boolean };
type Panel = { item: Item; marketplace?: string; report?: ImportPreview["report"]; install?: boolean; check?: string };
const connectionLabels = { connecting: "Connecting…", connected: "Connected", failed: "Failed", needs_sign_in: "Sign in needed", needs_key: "Secret needed" };
const categories: Category[] = ["Plugins", "Skills", "MCP", "Hooks"];
type Glyph = "folder" | "plug" | "file" | "hook" | "pdf" | "docx" | "xlsx" | "eye" | "trash" | "plus" | "back" | "warning";
const skillGlyph = (id: string): Glyph => ["pdf", "docx", "xlsx"].includes(id) ? id as Glyph : "file";
const emptySpec = (): PluginSpec => ({ name: "personal", description: "", version: "", skills: [], mcp: {}, hooks: [], files: {}, unsupported: [] });
function mcpCommand(server: PluginSpec["mcp"][string]) {
  return server.url ?? [server.command ?? "", ...server.args].map(part => /^[\w@%+=:,./-]+$/.test(part) ? part : `'${part.replace(/'/g, `'"'"'`)}'`).join(" ");
}
function pluginDisplayName(plugin: Plugin) {
  return plugin.source === "discovered" || plugin.spec.origin?.kind === "discovered" ? plugin.spec.skills[0]?.name ?? plugin.spec.origin?.location.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? "Unavailable skill" : plugin.spec.name;
}
function items(plugins: Plugin[], category: Category): Item[] {
  return plugins.flatMap<Item>(plugin => {
    const base = { plugin, key: `${plugin.scope}:${plugin.spec.name}` };
    if (category === "Plugins") return isPluginPackage(plugin) ? [{ ...base, name: pluginDisplayName(plugin), kind: "plugin" as const, id: plugin.spec.name, enabled: plugin.enabled }] : [];
    if (category === "Skills" && !plugin.spec.skills.length && (plugin.source === "discovered" || plugin.spec.origin?.kind === "discovered")) return [{ ...base, name: pluginDisplayName(plugin), kind: "skill", id: plugin.spec.name, enabled: false, unavailable: true }];
    if (category === "Skills") return plugin.spec.skills.map(skill => ({ ...base, key: `${base.key}:skill:${skill.id}`, name: skill.name, id: skill.id, kind: "skill" as const, enabled: plugin.enabled && !(plugin.spec.disabled_skills ?? []).includes(skill.id) }));
    if (category === "MCP") return Object.entries(plugin.spec.mcp).map(([id, server]) => ({ ...base, key: `${base.key}:mcp:${id}`, name: id, id, kind: "mcp" as const, enabled: plugin.enabled && server.enabled }));
    return plugin.spec.hooks.map(hook => ({ ...base, key: `${base.key}:hook:${hook.name}`, name: hook.name, id: hook.name, kind: "hook" as const, enabled: plugin.enabled && hook.enabled }));
  });
}
function markdown(plugin: Plugin, id: string) {
  const skill = plugin.spec.skills.find(skill => skill.id === id);
  if (!skill) return "";
  const sourcePath = plugin.spec.skill_paths?.[id];
  const captured = sourcePath && plugin.spec.files[sourcePath];
  const root = plugin.spec.skills[0]?.id === id ? plugin.spec.files["SKILL.md"] : undefined;
  const nested = Object.entries(plugin.spec.files).find(([path]) => path.endsWith("/SKILL.md") && path.split("/").slice(-2)[0] === id)?.[1];
  return captured || root || plugin.spec.files[`skills/${id}/SKILL.md`] || nested || `---\nname: ${skill.name}\ndescription: ${skill.description}\n---\n\n${skill.instructions}`;
}
function Icon({ name }: { name: Glyph }) {
  return <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true">
    {name === "warning" && <><path d="m12 3 10 18H2Z" /><path d="M12 9v5m0 3v1" /></>}
    {name === "plus" && <path d="M12 5v14M5 12h14" />}
    {name === "back" && <path d="m14 5-7 7 7 7" />}
    {name === "trash" && <path d="M3 6h18M9 6V3h6v3M6 6l1 15h10l1-15M10 10v7M14 10v7" />}
    {name === "eye" && <><path d="M2 12s3-7 10-7 10 7 10 7-3 7-10 7-10-7-10-7Z" /><circle cx="12" cy="12" r="3" /></>}
    {name === "folder" && <path d="M3 7h7l2 2h9v11H3ZM3 7V4h7l2 3" />}
    {name === "plug" && <path d="M8 3v5M16 3v5M6 8h12v4a6 6 0 0 1-12 0ZM12 18v4" />}
    {name === "file" && <path d="M5 3h10l4 4v14H5ZM15 3v5h4M8 12h8M8 16h8" />}
    {["pdf", "docx", "xlsx"].includes(name) && <><path d="M5 3h10l4 4v14H5ZM15 3v5h4" /><text x="12" y="16" textAnchor="middle" fontSize="5.5" stroke="none" fill="currentColor">{name === "pdf" ? "PDF" : name === "docx" ? "DOC" : "XLS"}</text></>}
    {name === "hook" && <path d="M5 4v9a7 7 0 0 0 14 0v-3l-4 4M19 10l4 4" />}
  </svg>;
}
function PluginIcon({ plugin, fallback = "folder" }: { plugin: PluginSpec; fallback?: Glyph }) {
  const [failedIcon, setFailedIcon] = useState<string | null>(null);
  const icon = plugin.icon;
  if (icon?.startsWith("https://") && failedIcon !== icon) return <img className="themis-origin-icon" src={icon} alt="" referrerPolicy="no-referrer" onError={() => setFailedIcon(icon)} />;
  if (icon && failedIcon !== icon && plugin.files[icon]?.trim().startsWith("<svg")) return <img className="themis-origin-icon" src={`data:image/svg+xml,${encodeURIComponent(plugin.files[icon])}`} alt="" onError={() => setFailedIcon(icon)} />;
  return <Icon name={fallback} />;
}
/** A steady hue per name, so a plugin's letter tile keeps its colour. */
const tileHue = (name: string) => [...name].reduce((hash, char) => (hash * 31 + char.charCodeAt(0)) % 360, 7);
/** Card icon: the plugin's own image, else its initial on a tile coloured from its name. */
function CardIcon({ spec, name }: { spec: PluginSpec; name: string }) {
  const [failedIcon, setFailedIcon] = useState<string | null>(null);
  const icon = spec.icon && failedIcon !== spec.icon ? spec.icon : undefined;
  const svg = icon && spec.files[icon]?.trim().startsWith("<svg") ? spec.files[icon] : undefined;
  const source = icon?.startsWith("https://") ? icon : svg ? `data:image/svg+xml,${encodeURIComponent(svg)}` : undefined;
  if (source) return <span className="themis-card-tile"><img src={source} alt="" referrerPolicy="no-referrer" onError={() => setFailedIcon(icon ?? null)} /></span>;
  return <span className="themis-card-tile themis-card-monogram" style={{ ["--tile-hue" as string]: tileHue(name) }} aria-hidden="true">{name.charAt(0).toUpperCase()}</span>;
}
function capabilitySummary(spec: PluginSpec) {
  return [[spec.skills.length, "skill"], [Object.keys(spec.mcp).length, "MCP connection"], [spec.hooks.length, "hook"]]
    .filter(([count]) => Number(count) > 0).map(([count, label]) => `${count} ${label}${count === 1 ? "" : "s"}`).join(" · ");
}
const compatibilityLabels = { supported: "Compatible", partial: "Partially supported", unsupported: "Unsupported" };
/** One Discover entry: a catalog plugin of one source (`key` is `source/name`). */
type CatalogEntry = { key: string; market: string; name: string; description?: string; icon?: string; source?: unknown; author?: { name?: string } | string; group: string };
type Scan = { revision: string; status: string; completed: number; total: number; catalog?: { plugins: Array<Omit<CatalogEntry, "key" | "market" | "group">> } | null; checks: Record<string, ImportPreview>; errors: Record<string, string>; groups?: Record<string, string>; error?: string };
const groupOrder = ["Coding", "Cloud & deploy", "Data & databases", "Monitoring", "Security", "Work apps", "Design", "Other"];
const groupKey = "themis.discover.group";
// The last Discover group is a per-viewer convenience: storage may be unavailable.
function readGroup() { try { return localStorage.getItem(groupKey) ?? "All"; } catch { return "All"; } }
function saveGroup(group: string) { try { localStorage.setItem(groupKey, group); } catch { /* not remembered */ } }
const publisher = (entry: CatalogEntry) => typeof entry.author === "string" ? entry.author : entry.author?.name ?? "";
/** What installing adds, in plain words: "Connects to airtable · 8 skills". */
function addsSummary(spec: PluginSpec) {
  const servers = Object.keys(spec.mcp);
  const connections = servers.length === 1 ? `Connects to ${servers[0]}` : servers.length ? `${servers.length} connections` : "";
  const count = (n: number, label: string) => n ? `${n} ${label}${n === 1 ? "" : "s"}` : "";
  return [connections, count(spec.skills.length, "skill"), count(spec.hooks.length, "hook")].filter(Boolean).join(" · ");
}
const rank = (status?: string) => status === "supported" ? 0 : status === "partial" ? 1 : 2;
function compatibilityReasons(spec: PluginSpec) {
  return [...new Set([...(spec.import_issues ?? []).map(issue => [issue.field || issue.name, issue.reason, issue.remedy].filter(Boolean).join(": ")), ...spec.unsupported.map(feature => feature === "lspServers" ? "Language servers aren’t supported yet." : `Unsupported: ${feature}`)])];
}
function CompatibilityWarning({ reasons, label = "Compatibility details" }: { reasons: string[]; label?: string }) {
  return reasons.length ? <details className="themis-compatibility-reason">
    <summary><Icon name="warning" /><span>{label} ({reasons.length})</span></summary>
    <ul>{reasons.map(reason => <li key={reason}>{reason}</li>)}</ul>
  </details> : null;
}
function Compatibility({ spec, report }: { spec: PluginSpec; report?: ImportPreview["report"] }) {
  return <div className="themis-compatibility">
    {report && <strong className={`themis-compatibility-badge ${report.status}`} title="Import format checked; runtime not tested">{compatibilityLabels[report.status]}</strong>}
    <CompatibilityWarning reasons={compatibilityReasons(spec)} />
    {spec.notices?.length ? <details className="themis-compatibility-notes"><summary>Notes ({spec.notices.length})</summary><ul>{spec.notices.map(note => <li key={note}>{note}</li>)}</ul></details> : null}
    {(Object.keys(spec.mcp).length > 0 || spec.hooks.length > 0) && <p>Connections and hooks install disabled.</p>}
  </div>;
}
export function Plugins() {
  const { state, dispatch } = useApp();
  const root = state.activeProjectRoot;
  const activeRoot = useRef(root);
  activeRoot.current = root;
  const loadRequest = useRef(0), catalogRequest = useRef(0);
  const [plugins, setPlugins] = useState<Plugin[]>([]), [markets, setMarkets] = useState<Marketplace[]>([]);
  const [installedSearch, setInstalledSearch] = useState(""), [catalogSearch, setCatalogSearch] = useState("");
  const [installedOrigin, setInstalledOrigin] = useState("all"), [catalogOrigin, setCatalogOrigin] = useState("all");
  const [installing, setInstalling] = useState("");
  const mutationPending = useRef(false);
  const [mutating, setMutating] = useState(false);
  const [category, setCategory] = useState<Category>("Plugins"), [publicView, setPublicView] = useState(false);
  const [panel, setPanel] = useState<Panel | null>(null), [removing, setRemoving] = useState<Item | null>(null);
  const inspection = useRef(new Map<string, Promise<ImportPreview>>());
  const [checkErrors, setCheckErrors] = useState<Record<string, string>>({});
  const [checking, setChecking] = useState<Record<string, boolean>>({});
  const [checks, setChecks] = useState<Record<string, ImportPreview>>({});
  const [catalogStatus, setCatalogStatus] = useState("available"), [installedStatus, setInstalledStatus] = useState("all");
  const [catalogLoading, setCatalogLoading] = useState(false);
  const [scanProgress, setScanProgress] = useState("");
  const [scanPercent, setScanPercent] = useState<number>();
  const [group, setGroup] = useState(readGroup);
  const [busy, setBusy] = useState(false);
  const [file, setFile] = useState("");
  const [mcpTools, setMcpTools] = useState<Array<{ name: string; description: string }> | null>();
  const [sourcesOpen, setSourcesOpen] = useState(false);
  const previewRequest = useRef(0);
  const search = publicView ? catalogSearch : installedSearch;
  const setSearch = publicView ? setCatalogSearch : setInstalledSearch;
  const originFilter = publicView ? catalogOrigin : installedOrigin;
  const setOriginFilter = publicView ? setCatalogOrigin : setInstalledOrigin;
  const [catalog, setCatalog] = useState<CatalogEntry[]>([]);
  const load = useCallback(async () => {
    if (root !== activeRoot.current) return;
    const request = ++loadRequest.current;
    const [installed, sources] = await Promise.all([pluginAction<Plugin[]>({ action: "list", projectRoot: root }), pluginAction<Marketplace[]>({ action: "marketplaces" })]);
    if (request === loadRequest.current && root === activeRoot.current) { setPlugins(installed); setMarkets(sources); }
  }, [root]);
  // A save of the mcp.json copy changed servers: show them.
  const reloadAfterMcpEdit = useCallback(() => { void load().catch(() => {}); }, [load]);
  useEffect(() => {
    let cancelled = false;
    setPlugins([]); inspection.current.clear(); setChecks({}); setChecking({}); setCheckErrors({}); setInstalling(""); setSourcesOpen(false); setPanel(null); setRemoving(null); setCatalog([]); setPublicView(false); catalogRequest.current++; previewRequest.current++; setBusy(false);
    void load().catch(error => { if (!cancelled) toast(dispatch, integrationError(error), "danger"); });
    const invalidate = () => { catalogRequest.current++; previewRequest.current++; };
    return () => { cancelled = true; invalidate(); };
  }, [dispatch, load, root]);
  useEffect(() => {
    const refresh = () => void load().catch(error => toast(dispatch, integrationError(error), "danger"));
    window.addEventListener("themis-plugins-changed", refresh);
    return () => window.removeEventListener("themis-plugins-changed", refresh);
  }, [dispatch, load]);
  const mcpItem = panel?.item.kind === "mcp" ? panel.item : undefined;
  const mcpStatus = mcpItem && state.mcpConnections[mcpItem.key]?.status;
  useEffect(() => {
    if (!mcpItem || mcpStatus !== "connected") return;
    let cancelled = false;
    setMcpTools(undefined);
    void pluginAction<Array<{ name: string; description: string }>>({ action: "mcp_tools", projectRoot: root, scope: mcpItem.plugin.scope, name: mcpItem.plugin.spec.name, id: mcpItem.id })
      .then(tools => { if (!cancelled) setMcpTools(tools); })
      .catch(error => { if (!cancelled) { setMcpTools(null); toast(dispatch, integrationError(error), "danger"); } });
    return () => { cancelled = true; };
  }, [dispatch, mcpItem, mcpStatus, root]);
  async function action(args: Record<string, unknown>) {
    if (mutationPending.current) throw new Error("Another integration change is still running.");
    mutationPending.current = true; setMutating(true);
    setBusy(true);
    try {
      await pluginAction({ projectRoot: root, ...args });
      await load();
      window.dispatchEvent(new Event("themis-plugins-changed"));
    } catch (error) { toast(dispatch, integrationError(error), "danger"); throw error; }
    finally { mutationPending.current = false; setMutating(false); setBusy(false); }
  }
  // OAuth sign-in for a remote MCP server: the page opens in the person's browser.
  const [signing, setSigning] = useState("");
  async function signIn(key: string, url: string, out = false) {
    setSigning(key);
    try {
      const connections = await pluginAction<McpConnections>({ action: out ? "mcp_sign_out" : "mcp_sign_in", projectRoot: root, url });
      dispatch({ type: "mcp/status", connections });
    } catch (error) {
      // A sign-in you cancelled is not an error.
      if (!integrationError(error).includes("Sign-in cancelled")) toast(dispatch, integrationError(error), "danger");
    } finally {
      setSigning("");
    }
  }
  const cancelSignIn = (url: string) => void pluginAction({ action: "mcp_sign_in_cancel", url }).catch(() => {});

  async function toggle(item: Item) {
    const args = { scope: item.plugin.scope, name: item.plugin.spec.name };
    if (item.kind !== "plugin" && !item.plugin.enabled && !isPluginPackage(item.plugin)) await action({ ...args, action: "enable" });
    await action(item.kind === "plugin" ? { ...args, action: item.enabled ? "disable" : "enable" } : { ...args, action: "set_component_enabled", kind: item.kind, id: item.id, enabled: !item.enabled });
  }
  function open(item: Item) {
    previewRequest.current++;
    setBusy(false); setMcpTools(undefined); setPanel({ item });
    setFile(item.kind === "plugin" ? "" : item.id);
  }
  async function sourcesChanged(added?: string) {
    const list = await pluginAction<Marketplace[]>({ action: "marketplaces" });
    setMarkets(list);
    const shown = list.filter(source => source.enabled !== false).map(source => source.name);
    if (added) setCatalogOrigin(added);
    else if (catalogOrigin !== "all" && !shown.includes(catalogOrigin)) setCatalogOrigin("all");
    await browse(false, shown);
  }
  // Every source is checked at once; each one's plugins appear as soon as its check is ready.
  async function browse(refresh = false, sources = markets.filter(source => source.enabled !== false).map(source => source.name)) {
    const request = ++catalogRequest.current;
    previewRequest.current++;
    setPanel(null); setBusy(false); setInstalling(""); setScanProgress("Checking marketplaces…"); setScanPercent(undefined);
    setCatalog([]); inspection.current.clear(); setChecks({}); setChecking({}); setCheckErrors({}); setCatalogLoading(true);
    const current = () => request === catalogRequest.current && root === activeRoot.current;
    const progress: Record<string, [number, number]> = {};
    const scanOne = async (name: string) => {
      let rescan = refresh;
      while (current()) {
        const value = await pluginAction<Scan>({ action: "scan_marketplace", name, refresh: rescan });
        rescan = false;
        if (!current()) return;
        if (value.status === "failed") throw new Error(value.error || "Marketplace check failed. Refresh to retry.");
        // Checked plugins can be installed while the rest of their catalog is still being checked.
        if (value.catalog) {
          const entries = value.catalog.plugins.map(entry => ({ ...entry, key: `${name}/${entry.name}`, market: name, group: value.groups?.[entry.name] ?? "Other" }));
          const keyed = <T,>(record: Record<string, T>) => Object.fromEntries(Object.entries(record).map(([plugin, item]) => [`${name}/${plugin}`, item]));
          setCatalog(existing => [...existing.filter(entry => entry.market !== name), ...entries]);
          setChecks(existing => ({ ...existing, ...keyed(value.checks) })); setCheckErrors(existing => ({ ...existing, ...keyed(value.errors) }));
        }
        if (value.status === "ready") return;
        progress[name] = [value.completed, value.total];
        const [completed, total] = Object.values(progress).reduce(([a, b], [c, d]) => [a + c, b + d], [0, 0]);
        setScanPercent(total ? Math.min(100, Math.floor(completed / total * 100)) : undefined);
        setScanProgress(total ? `Checking marketplaces… ${completed}/${total}` : "Checking marketplaces…");
        await new Promise(resolve => setTimeout(resolve, 1000));
      }
    };
    await Promise.all(sources.map(name => scanOne(name).catch(error => { if (current()) toast(dispatch, `${name}: ${integrationError(error)}`, "danger"); })));
    if (current()) setCatalogLoading(false);
  }
  const inspect = useCallback((entry: CatalogEntry): Promise<ImportPreview> => {
    const key = entry.key;
    const existing = inspection.current.get(key);
    if (existing) return existing;
    const generation = catalogRequest.current;
    setChecking(current => ({ ...current, [key]: true }));
    const pending = pluginAction<ImportPreview>({ action: "inspect_marketplace", marketplace: entry.market, name: entry.name, projectRoot: root });
    inspection.current.set(key, pending);
    void pending.then(value => {
      if (generation === catalogRequest.current && root === activeRoot.current) setChecks(current => ({ ...current, [key]: value }));
    }, error => {
      if (generation === catalogRequest.current && root === activeRoot.current) setCheckErrors(current => ({ ...current, [key]: integrationError(error) }));
    }).finally(() => {
      if (generation === catalogRequest.current && root === activeRoot.current) setChecking(current => ({ ...current, [key]: false }));
    });
    return pending;
  }, [root]);
  async function preview(entry: CatalogEntry, install = false) {
    const { market: marketplace, name, icon } = entry;
    const spec = { ...emptySpec(), name, icon };
    const plugin: Plugin = { scope: "global", revision: "preview", enabled: false, source: marketplace, spec };
    const previewItem: Item = { key: `preview:${entry.key}`, name, id: name, kind: "plugin", enabled: false, plugin };
    if (install) { previewRequest.current++; setInstalling(entry.key); } else { open(previewItem); setPanel({ item: previewItem, marketplace }); }
    const request = previewRequest.current;
    setBusy(true);
    try {
      const checked = await inspect(entry);
      const value = checked.spec;
      if (request !== previewRequest.current || root !== activeRoot.current) return;
      setChecks(current => ({ ...current, [entry.key]: checked }));
      if (install && checked.report.status === "supported") {
        await action({ action: "install", name, marketplace, scope: "global", allowPartial: false, expectedCheck: checked.check_revision }).then(() => toast(dispatch, `${name} installed.`, "success")).catch(() => {});
      } else setPanel({ marketplace, install, check: checked.check_revision, report: checked.report, item: { ...previewItem, plugin: { ...plugin, spec: { ...value, icon: value.icon ?? icon } } } });
      setFile("");
    } catch (error) { if (request === previewRequest.current) toast(dispatch, integrationError(error), "danger"); } finally { if (request === previewRequest.current) { setBusy(false); setInstalling(""); } }
  }
  async function installPreview() {
    if (!panel?.report || !panel.marketplace || panel.report.status === "unsupported") return;
    const name = panel.item.plugin.spec.name;
    try {
      await action({ action: "install", name, marketplace: panel.marketplace, expectedCheck: panel.check, scope: "global", allowPartial: panel.report.status === "partial" });
      toast(dispatch, `${name} installed.`, "success");
      setPanel(null);
    } catch { /* The error toast is shown; keep the dialog available for retry. */ }
  }
  const item = panel?.item;
  const terms = search.trim().toLowerCase().split(/\s+/).filter(Boolean);
  const matches = (values: unknown[]) => { const text = values.filter(Boolean).join(" ").toLowerCase(); return terms.every(term => text.includes(term)); };
  const publicSource = (source?: string | null) => /^https?:\/\//.test(source ?? "");
  const marketplaceIsPublic = (name: string) => publicSource(markets.find(source => source.name === name)?.source);
  const isPublic = (plugin: Plugin) => publicSource(plugin.spec.origin?.location) || marketplaceIsPublic(plugin.source ?? plugin.spec.origin?.location ?? "");
  const matchesOrigin = (publicItem: boolean) => originFilter === "all" || (publicItem ? "public" : "personal") === originFilter;
  const catalogInstalled = (entry: Pick<CatalogEntry, "market" | "name">) => plugins.some(plugin => plugin.scope === "global" && plugin.source === entry.market && plugin.spec.name === entry.name);
  const rows = items(plugins, category).filter(row => matchesOrigin(isPublic(row.plugin)) && (installedStatus === "all" || (row.enabled ? "enabled" : "disabled") === installedStatus)).filter(row => matches([row.name, pluginDisplayName(row.plugin), row.plugin.source, row.plugin.spec.origin?.location, row.plugin.spec.description, ...row.plugin.spec.skills.map(skill => skill.description)]));
  const matchesStatus = (key: string, installed: boolean) => catalogStatus === "installed" ? installed : catalogStatus === "all" || (catalogStatus === "available" ? ["supported", "partial"].includes(checks[key]?.report.status) || (catalogLoading && !checks[key] && !checkErrors[key]) : (checks[key]?.report.status ?? (checkErrors[key] ? "failed" : "unchecked")) === catalogStatus);
  const sourceRemoved = (plugin: Plugin) => markets.length > 0 && !!plugin.source && plugin.source !== "discovered" && !markets.some(source => source.name === plugin.source);
  const sourceRows = catalog.filter(entry => (catalogOrigin === "all" || entry.market === catalogOrigin) && matchesStatus(entry.key, catalogInstalled(entry))).filter(entry => matches([entry.name, entry.description, publisher(entry), entry.market]));
  const groupCounts = groupOrder.map(name => [name, sourceRows.filter(entry => entry.group === name).length] as const).filter(([, count]) => count > 0);
  const shownGroup = group === "All" || groupCounts.some(([name]) => name === group) ? group : "All";
  // Cards keep their place while checks finish, so nothing moves under the pointer.
  const catalogRows = sourceRows.filter(entry => shownGroup === "All" || entry.group === shownGroup).sort((a, b) => (catalogLoading ? 0 : rank(checks[a.key]?.report.status) - rank(checks[b.key]?.report.status)) || a.name.localeCompare(b.name));
  const pickGroup = (name: string) => { setGroup(name); saveGroup(name); };
  const resultCount = publicView ? catalogRows.length : rows.length;
  const filtered = !!search.trim() || originFilter !== "all" || (publicView ? catalogStatus !== "available" || shownGroup !== "All" : installedStatus !== "all");
  const reason = (key: string) => {
    const checked = checks[key];
    const reasons = checkErrors[key] ? [`${integrationError(checkErrors[key])} Refresh to retry.`] : checked ? compatibilityReasons(checked.spec) : [];
    if (!reasons.length && checked?.report.status === "unsupported") reasons.push("No installable skills, connections or hooks.");
    const label = checkErrors[key] ? "Check failed" : checked?.report.status === "unsupported" ? "Unsupported" : "Some parts skipped";
    return <CompatibilityWarning reasons={reasons} label={label} />;
  };
  // Checked entries say what's wrong in their warning; only pending checks need a badge.
  const badge = (key: string) => checks[key] || checkErrors[key] ? null : <span className="themis-compatibility-badge unchecked" title="Compatibility check pending" aria-live="polite">{checking[key] || catalogLoading ? <><span className="themis-install-spinner" aria-hidden="true" />Checking…</> : "Not checked"}</span>;
  const text = item ? markdown(item.plugin, file) : "";
  const server = item?.kind === "mcp" ? item.plugin.spec.mcp[item.id] : undefined;
  const hook = item?.kind === "hook" ? item.plugin.spec.hooks.find(hook => hook.name === item.id) : undefined;
  return <div className="themis-integrations">
    <div className="themis-integrations-header"><h2>Integrations</h2><div className="themis-view-switch" aria-label="View"><button aria-pressed={publicView} onClick={() => { setCategory("Plugins"); setPublicView(true); void browse(); }}>Discover</button><button aria-pressed={!publicView} onClick={() => { previewRequest.current++; catalogRequest.current++; setPublicView(false); setPanel(null); setBusy(false); }}>Installed</button></div></div>
    {!publicView && <nav className="themis-integrations-tabs" aria-label="Integration categories">{categories.map(value => <button key={value} aria-pressed={category === value} onClick={() => { previewRequest.current++; catalogRequest.current++; setCategory(value); setPublicView(false); setPanel(null); setBusy(false); }}>{value}</button>)}</nav>}
    <div className="themis-integrations-toolbar"><div className="themis-integration-filters">
      <input className="themis-integrations-search" type="search" aria-label="Search integrations" placeholder={publicView ? "Search plugins" : "Search"} value={search} onChange={event => setSearch(event.target.value)} />
      {publicView && category === "Plugins" && <div className="themis-marketplace-picker"><button className="themis-sources-button" disabled={busy || mutating} onClick={() => setSourcesOpen(true)}>Sources</button></div>}
      <select aria-label="Status" value={publicView ? catalogStatus : installedStatus} onChange={event => publicView ? setCatalogStatus(event.target.value) : setInstalledStatus(event.target.value)}>
        {publicView ? <><option value="available">Compatible</option><option value="all">All statuses</option><option value="installed">Installed</option><option value="supported">Fully compatible</option><option value="partial">Partially supported</option><option value="unsupported">Unsupported</option><option value="failed">Check failed</option><option value="unchecked">Not checked</option></> : <><option value="all">All statuses</option><option value="enabled">Enabled</option><option value="disabled">Disabled</option></>}
      </select>
      <select aria-label="Source" value={originFilter} onChange={event => setOriginFilter(event.target.value)}><option value="all">All sources</option>{publicView && category === "Plugins" ? markets.filter(source => source.enabled !== false).map(source => <option key={source.name} value={source.name}>{sourceLabel(source.name)}</option>) : <><option value="personal">Personal</option><option value="public">Public</option></>}</select>
      <button aria-label="Refresh" title="Refresh" disabled={busy || mutating} onClick={() => { if (category === "MCP") window.dispatchEvent(new Event("themis-mcp-reconnect")); void (publicView && category === "Plugins" ? browse(true) : load()).catch(error => toast(dispatch, integrationError(error), "danger")); }}>↻</button></div></div>
    {category === "MCP" && !publicView && <McpFileRow onApplied={reloadAfterMcpEdit} />}
    {(filtered || (publicView && catalogLoading)) && <div className="themis-filter-summary"><span>{publicView && catalogLoading ? "Checking catalog" : `${resultCount} result${resultCount === 1 ? "" : "s"}`}</span>{filtered && <button onClick={() => { setSearch(""); setOriginFilter("all"); if (publicView) { setCatalogStatus("available"); pickGroup("All"); } else setInstalledStatus("all"); }}>Reset filters</button>}</div>}
    {publicView ? <>
      {catalogLoading && !catalog.length ? <div className="themis-marketplace-loading"><KnightLoading variant={readCompanionPreferences().variant} status={scanProgress} progress={scanPercent} progressLabel="Marketplace compatibility" /></div> : !catalogRows.length && <EmptyState title={(search.trim() || originFilter !== "all" || (publicView ? catalogStatus !== "available" : installedStatus !== "all")) ? "No matching integrations" : "No plugins to show"} />}
      {groupCounts.length > 1 && <nav className="themis-discover-groups" aria-label="Groups">{[["All", sourceRows.length] as const, ...groupCounts].map(([name, count]) => <button key={name} aria-pressed={shownGroup === name} onClick={() => pickGroup(name)}>{name}<span>{count}</span></button>)}</nav>}
      <ul className="themis-integrations-list themis-integration-cards">{catalogRows.map(entry => <li key={entry.key} data-check={entry.name}>
        <div className="themis-card-heading"><CardIcon spec={{ ...(checks[entry.key]?.spec ?? emptySpec()), icon: checks[entry.key]?.spec.icon ?? entry.icon }} name={entry.name} /><div><button className="themis-integration-name themis-integration-open" aria-label={`View ${entry.name}`} disabled={busy || mutating} onClick={() => void preview(entry)}>{entry.name}</button><span className="themis-integration-source">{[publisher(entry), sourceLabel(entry.market)].filter(Boolean).join(" · ")}</span></div></div>
        {entry.description && <p className="themis-integration-description">{entry.description}</p>}
        {reason(entry.key)}
        <div className="themis-card-footer"><span className="themis-integration-capabilities">{checks[entry.key] ? addsSummary(checks[entry.key].spec) : badge(entry.key)}</span><button className={catalogInstalled(entry) ? "themis-installed" : undefined} aria-live="polite" aria-busy={installing === entry.key} disabled={busy || mutating || !checks[entry.key] || catalogInstalled(entry) || checks[entry.key]?.report.status === "unsupported"} onClick={() => void preview(entry, true)}>{installing === entry.key ? <><span className="themis-install-spinner" aria-hidden="true" />Installing…</> : catalogInstalled(entry) ? "✓ Installed" : "Install"}</button></div>
      </li>)}</ul>
    </> : category === "Hooks" ? (rows.length || plugins.some(plugin => plugin.spec.unsupported.some(note => skipReason(note) !== null))
      ? <HooksByEvent rows={rows} plugins={plugins} sourceOf={row => pluginDisplayName(row.plugin)} disabled={row => busy || mutating || !row.plugin.enabled && isPluginPackage(row.plugin)} onToggle={row => void toggle(row as Item).catch(() => {})} onOpen={row => open(row as Item)} />
      : <EmptyState title={search.trim() || originFilter !== "all" || installedStatus !== "all" ? "No matching integrations" : "No hooks installed"} />
    ) : <>
      {!rows.length && <EmptyState title={(search.trim() || originFilter !== "all" || (publicView ? catalogStatus !== "available" : installedStatus !== "all")) ? "No matching integrations" : `No ${category.toLowerCase()} installed`} />}
      <ul className="themis-integrations-list themis-installed-integrations">{rows.map(row => <li key={row.key}>
        <PluginIcon plugin={row.plugin.spec} fallback={row.kind === "plugin" ? "folder" : row.kind === "mcp" ? "plug" : row.kind === "hook" ? "hook" : skillGlyph(row.id)} /><button className="themis-integration-name themis-integration-open" aria-label={`View ${row.name}`} onClick={() => open(row)}>{row.name}</button><span className="themis-integration-source">{row.kind === "plugin" ? row.plugin.spec.origin?.kind === "discovered" ? "Discovered" : row.plugin.source ? sourceLabel(row.plugin.source) : row.plugin.spec.origin?.kind ?? "Personal" : pluginDisplayName(row.plugin)} · {row.plugin.scope === "global" ? "User" : "Project"}{row.kind === "plugin" && sourceRemoved(row.plugin) ? " · Source removed" : ""}</span>
        <p className="themis-integration-description">{row.kind === "skill" ? row.plugin.spec.skills.find(skill => skill.id === row.id)?.description : row.kind === "mcp" ? <code>{mcpCommand(row.plugin.spec.mcp[row.id])}</code> : row.kind === "hook" ? row.plugin.spec.hooks.find(hook => hook.name === row.id)?.event : row.plugin.spec.description || capabilitySummary(row.plugin.spec)}</p>
        {row.unavailable && <CompatibilityWarning reasons={compatibilityReasons(row.plugin.spec)} />}
        {row.kind === "mcp" && row.enabled && state.mcpConnections[row.key]?.reason && !["needs_sign_in", "needs_key"].includes(state.mcpConnections[row.key]?.status) && <CompatibilityWarning label="Connection details" reasons={[state.mcpConnections[row.key].reason!]} />}
        <span className={`themis-integration-status themis-compatibility-badge ${row.kind === "mcp" && row.enabled ? state.mcpConnections[row.key]?.status === "connected" ? "supported" : state.mcpConnections[row.key]?.status === "failed" ? "failed" : ["needs_sign_in", "needs_key"].includes(state.mcpConnections[row.key]?.status) ? "partial" : "unchecked" : row.enabled ? "supported" : "unchecked"}`}>{row.unavailable ? "Unavailable" : row.kind === "mcp" && row.enabled ? connectionLabels[state.mcpConnections[row.key]?.status ?? "connecting"] : row.enabled ? "Enabled" : "Disabled"}</span>
        {row.kind === "mcp" && row.enabled && state.mcpConnections[row.key]?.status === "needs_key" && <button type="button" className="themis-text-action themis-integration-signin" title={state.mcpConnections[row.key].reason} onClick={() => dispatch({ type: "ui/view", view: "settings", settings: { section: "Secrets", key: state.mcpConnections[row.key].keys?.[0] } })}>Add secret</button>}
        {row.kind === "mcp" && row.enabled && state.mcpConnections[row.key]?.url && (state.mcpConnections[row.key]?.status === "needs_sign_in"
          ? signing === row.key
            ? <span className="themis-integration-signin themis-integration-waiting" role="status">Waiting for browser…<button type="button" className="themis-text-action" onClick={() => cancelSignIn(state.mcpConnections[row.key].url!)}>Cancel</button></span>
            : <Button className="themis-integration-signin" size="small" disabled={signing !== ""} onClick={() => void signIn(row.key, state.mcpConnections[row.key].url!)}>Sign in</Button>
          : state.mcpConnections[row.key]?.signed_in && <button className="themis-text-action themis-integration-signin" disabled={signing !== ""} onClick={() => void signIn(row.key, state.mcpConnections[row.key].url!, true)}>Sign out</button>)}
        {!row.unavailable && <button className="themis-integration-toggle" role="switch" aria-label={`${row.name} enabled`} aria-checked={row.enabled} title={!row.plugin.enabled && row.kind !== "plugin" && isPluginPackage(row.plugin) ? "Enable the parent plugin first" : `Turn ${row.enabled ? "off" : "on"} ${row.name}`} disabled={busy || mutating || row.kind !== "plugin" && !row.plugin.enabled && isPluginPackage(row.plugin)} onClick={() => void toggle(row).catch(() => {})}><span className="themis-toggle-track"><span /></span></button>}
        {row.plugin.source !== "discovered" && row.plugin.spec.origin?.kind !== "discovered" && <button aria-label={`Uninstall ${row.name}`} title={`Uninstall ${row.name}`} disabled={busy || mutating} onClick={() => { setRemoving(row); }}><Icon name="trash" /></button>}
      </li>)}</ul>
    </>}
    <Dialog open={panel !== null} title={item?.name} onClose={() => { if (!mutating) { previewRequest.current++; setBusy(false); setPanel(null); } }}>
      <div className="themis-skill-viewer" key={`${item?.key}:${file}`}>
        {panel?.marketplace && busy ? <p role="status">Loading skills…</p> : <>
          {item?.kind === "plugin" && !file && !panel?.install && <><ul className="themis-bundled-skills">{item.plugin.spec.skills.map(skill => <li key={skill.id}><button aria-label={`View ${skill.name}`} onClick={() => setFile(skill.id)}><span>{skill.name}</span><Icon name="eye" /></button></li>)}</ul>{!item.plugin.spec.skills.length && <p>No skills</p>}</>}
          {item?.kind === "plugin" && file && <button className="themis-skill-back" aria-label="Back to skills" title="Back to skills" onClick={() => setFile("")}><Icon name="back" /></button>}
          {server ? <dl className="themis-integration-summary"><dt>Transport</dt><dd>{server.url ? "HTTP" : "Local process"}</dd><dt>{server.url ? "Endpoint" : "Command"}</dt><dd><code>{mcpCommand(server)}</code></dd><dt>Status</dt><dd>{item?.enabled ? connectionLabels[state.mcpConnections[item.key]?.status ?? "connecting"] : "Disabled"}</dd><dt>Source</dt><dd>{item && pluginDisplayName(item.plugin)}</dd></dl> : hook ? <dl className="themis-integration-summary"><dt>Event</dt><dd>{hook.event}</dd><dt>Status</dt><dd>{item?.enabled ? "Enabled" : "Disabled"}</dd><dt>Source</dt><dd>{item && pluginDisplayName(item.plugin)}</dd></dl> : text ? <ResponseBody text={text.replace(/^---\r?\n[\s\S]*?\r?\n---(?:\r?\n|$)/, "")} /> : null}
          {server && <section className="themis-mcp-tools" aria-label="MCP tools"><h3>Tools{mcpStatus === "connected" && mcpTools ? ` (${mcpTools.length})` : ""}</h3>{mcpStatus !== "connected" ? <p>Tools appear when connected.</p> : mcpTools === undefined ? <p role="status">Loading tools…</p> : mcpTools && (mcpTools.length ? <dl>{mcpTools.map(tool => <div key={tool.name}><dt><code>{tool.name}</code></dt><dd>{tool.description}</dd></div>)}</dl> : <p>No tools exposed.</p>)}</section>}
          {item && (panel?.install || item.unavailable) && <Compatibility spec={item.plugin.spec} report={panel.report} />}
        </>}
      </div>
      {panel?.install && panel.report && panel.report.status !== "unsupported" && !catalogInstalled({ market: panel.marketplace ?? "", name: panel.item.id }) && <div className="themis-preview-install"><Button disabled={busy || mutating} onClick={() => void installPreview()}>{panel.report.status === "partial" ? "Install supported parts" : "Install"}</Button></div>}
      <Button variant="ghost" disabled={mutating} onClick={() => { previewRequest.current++; setBusy(false); setPanel(null); }}>Close</Button>
    </Dialog>
    <SourcesDialog open={sourcesOpen} disabled={busy || mutating} run={args => action(args)} onChanged={added => void sourcesChanged(added)} onClose={() => setSourcesOpen(false)} />
    <Dialog open={removing !== null} title={`Uninstall ${removing?.name ?? ""}?`} onClose={() => { if (!busy) setRemoving(null); }}>
      <p>{removing?.kind === "plugin" ? "Remove this plugin and its bundled capabilities?" : `Remove this ${removing?.kind} from ${removing?.plugin.spec.name}? Other capabilities remain installed.`}</p>
      <Button variant="ghost" disabled={busy || mutating} onClick={() => setRemoving(null)}>Cancel</Button>
      <Button variant="danger" disabled={busy || mutating} onClick={() => { if (removing) void action({ action: removing.kind === "plugin" ? "delete" : "remove_component", kind: removing.kind, id: removing.id, name: removing.plugin.spec.name, scope: removing.plugin.scope }).then(() => setRemoving(null)).catch(() => {}); }}>Uninstall</Button>
    </Dialog>
  </div>;
}
