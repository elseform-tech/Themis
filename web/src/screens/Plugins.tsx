import { useCallback, useEffect, useRef, useState } from "react";
import { Button, Dialog, EmptyState } from "../components";
import { KnightLoading, readCompanionPreferences } from "../components/KnightCompanion";
import { ResponseBody } from "../components/ResponseBody";
import { pluginAction } from "../lib/tauri";
import { isPluginPackage } from "../lib/integrations";
import type { ImportPreview, Marketplace, Plugin, PluginSpec } from "../lib/types";
import { describeError, toast, useApp } from "../state/store";
import "./Plugins.css";

function integrationError(error: unknown): string {
  return describeError(error).replace(/\nDiagnostic operation: [0-9a-f-]{36}$/, "").replace("No such file or directory (os error 2)", "Plugin files are missing.");
}

type Category = "Plugins" | "Skills" | "MCP" | "Hooks";
type Item = { key: string; name: string; plugin: Plugin; kind: "plugin" | "skill" | "mcp" | "hook"; id: string; enabled: boolean; unavailable?: boolean };
type Panel = { item: Item; marketplace?: string; report?: ImportPreview["report"]; install?: boolean; scan?: string };
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
function capabilitySummary(spec: PluginSpec) {
  return [[spec.skills.length, "skill"], [Object.keys(spec.mcp).length, "MCP connection"], [spec.hooks.length, "hook"]]
    .filter(([count]) => Number(count) > 0).map(([count, label]) => `${count} ${label}${count === 1 ? "" : "s"}`).join(" · ");
}
const compatibilityLabels = { supported: "Compatible", partial: "Partially supported", unsupported: "Unsupported" };
function compatibilityReasons(spec: PluginSpec) {
  return [...new Set([...(spec.import_issues ?? []).map(issue => [issue.field || issue.name, issue.reason, issue.remedy].filter(Boolean).join(": ")), ...spec.unsupported.map(feature => feature === "lspServers" ? "Language servers aren’t supported yet." : `Unsupported: ${feature}`)])];
}
function CompatibilityWarning({ reasons }: { reasons: string[] }) {
  return reasons.length ? <details className="themis-compatibility-reason">
    <summary><Icon name="warning" /><span>Compatibility details ({reasons.length})</span></summary>
    <ul>{reasons.map(reason => <li key={reason}>{reason}</li>)}</ul>
  </details> : null;
}
function Compatibility({ spec, report }: { spec: PluginSpec; report?: ImportPreview["report"] }) {
  return <div className="themis-compatibility">
    {report && <strong className={`themis-compatibility-badge ${report.status}`} title="Import format checked; runtime not tested">{compatibilityLabels[report.status]}</strong>}
    <CompatibilityWarning reasons={compatibilityReasons(spec)} />
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
  const [scanRevision, setScanRevision] = useState("");
  const [busy, setBusy] = useState(false);
  const [file, setFile] = useState("");
  const [addingMarket, setAddingMarket] = useState(false), [marketName, setMarketName] = useState(""), [marketSource, setMarketSource] = useState("");
  const previewRequest = useRef(0);
  const search = publicView ? catalogSearch : installedSearch;
  const setSearch = publicView ? setCatalogSearch : setInstalledSearch;
  const originFilter = publicView ? catalogOrigin : installedOrigin;
  const setOriginFilter = publicView ? setCatalogOrigin : setInstalledOrigin;
  const [catalog, setCatalog] = useState<Array<{ name: string; description?: string; icon?: string; source?: unknown }>>([]), [market, setMarket] = useState("");
  const load = useCallback(async () => {
    if (root !== activeRoot.current) return;
    const request = ++loadRequest.current;
    const [installed, sources] = await Promise.all([pluginAction<Plugin[]>({ action: "list", projectRoot: root }), pluginAction<Marketplace[]>({ action: "marketplaces" })]);
    if (request === loadRequest.current && root === activeRoot.current) { setPlugins(installed); setMarkets(sources); }
  }, [root]);
  useEffect(() => {
    let cancelled = false;
    setPlugins([]); inspection.current.clear(); setChecks({}); setChecking({}); setCheckErrors({}); setInstalling(""); setAddingMarket(false); setPanel(null); setRemoving(null); setCatalog([]); setPublicView(false); catalogRequest.current++; previewRequest.current++; setBusy(false);
    void load().catch(error => { if (!cancelled) toast(dispatch, integrationError(error), "danger"); });
    const invalidate = () => { catalogRequest.current++; previewRequest.current++; };
    return () => { cancelled = true; invalidate(); };
  }, [dispatch, load, root]);
  useEffect(() => {
    const refresh = () => void load().catch(error => toast(dispatch, integrationError(error), "danger"));
    window.addEventListener("themis-plugins-changed", refresh);
    return () => window.removeEventListener("themis-plugins-changed", refresh);
  }, [dispatch, load]);
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
  async function toggle(item: Item) {
    const args = { scope: item.plugin.scope, name: item.plugin.spec.name };
    if (item.kind !== "plugin" && !item.plugin.enabled && !isPluginPackage(item.plugin)) await action({ ...args, action: "enable" });
    await action(item.kind === "plugin" ? { ...args, action: item.enabled ? "disable" : "enable" } : { ...args, action: "set_component_enabled", kind: item.kind, id: item.id, enabled: !item.enabled });
  }
  function open(item: Item) {
    previewRequest.current++;
    setBusy(false); setPanel({ item });
    setFile(item.kind === "plugin" ? "" : item.id);
  }
  async function browse(selected: string, refresh = false) {
    const request = ++catalogRequest.current;
    previewRequest.current++;
    setPanel(null); setBusy(false); setInstalling(""); setScanRevision(""); setScanProgress("Checking marketplace…"); setScanPercent(undefined);
    setMarket(selected); setCatalog([]); inspection.current.clear(); setChecks({}); setChecking({}); setCheckErrors({}); setCatalogLoading(true);
    try {
      let rescan = refresh;
      while (request === catalogRequest.current && root === activeRoot.current) {
        const value = await pluginAction<{ revision: string; status: string; completed: number; total: number; catalog: { plugins: typeof catalog }; checks: Record<string, ImportPreview>; errors: Record<string, string>; error?: string }>({ action: "scan_marketplace", name: selected, refresh: rescan });
        rescan = false;
        if (request !== catalogRequest.current || root !== activeRoot.current) return;
        if (value.status === "failed") throw new Error(value.error || "Marketplace check failed. Refresh to retry.");
        if (value.status === "ready") {
          setCatalog(value.catalog.plugins); setChecks(value.checks); setCheckErrors(value.errors); setScanRevision(value.revision);
          break;
        }
        setScanPercent(value.total ? Math.min(100, Math.floor(value.completed / value.total * 100)) : undefined);
        setScanProgress(value.total ? `Checking marketplace… ${value.completed}/${value.total}` : "Checking marketplace…");
        await new Promise(resolve => setTimeout(resolve, 1000));
      }
    } catch (error) { if (request === catalogRequest.current) toast(dispatch, integrationError(error), "danger"); }
    finally { if (request === catalogRequest.current) setCatalogLoading(false); }
  }
  const inspect = useCallback((key: string): Promise<ImportPreview> => {
    const existing = inspection.current.get(key);
    if (existing) return existing;
    const generation = catalogRequest.current;
    setChecking(current => ({ ...current, [key]: true }));
    const pending = pluginAction<ImportPreview>({ action: "inspect_marketplace", marketplace: market, name: key, projectRoot: root });
    inspection.current.set(key, pending);
    void pending.then(value => {
      if (generation === catalogRequest.current && root === activeRoot.current) setChecks(current => ({ ...current, [key]: value }));
    }, error => {
      if (generation === catalogRequest.current && root === activeRoot.current) setCheckErrors(current => ({ ...current, [key]: integrationError(error) }));
    }).finally(() => {
      if (generation === catalogRequest.current && root === activeRoot.current) setChecking(current => ({ ...current, [key]: false }));
    });
    return pending;
  }, [market, root]);
  async function preview(name: string, icon?: string, install = false) {
    const marketplace = market;
    const spec = { ...emptySpec(), name, icon };
    const plugin: Plugin = { scope: "global", revision: "preview", enabled: false, source: marketplace, spec };
    const previewItem: Item = { key: `preview:${marketplace}:${name}`, name, id: name, kind: "plugin", enabled: false, plugin };
    if (install) { previewRequest.current++; setInstalling(name); } else { open(previewItem); setPanel({ item: previewItem, marketplace, scan: scanRevision }); }
    const request = previewRequest.current;
    setBusy(true);
    try {
      const checked = await inspect(name);
      const value = checked.spec;
      if (request !== previewRequest.current || root !== activeRoot.current) return;
      setChecks(current => ({ ...current, [name]: checked }));
      if (install && checked.report.status === "supported") {
        await action({ action: "install", name, marketplace, scope: "global", allowPartial: false, expectedScan: scanRevision }).then(() => toast(dispatch, `${name} installed.`, "success")).catch(() => {});
      } else setPanel({ marketplace, install, scan: scanRevision, report: checked.report, item: { ...previewItem, plugin: { ...plugin, spec: { ...value, icon: value.icon ?? icon } } } });
      setFile("");
    } catch (error) { if (request === previewRequest.current) toast(dispatch, integrationError(error), "danger"); } finally { if (request === previewRequest.current) { setBusy(false); setInstalling(""); } }
  }
  async function installPreview() {
    if (!panel?.report || !panel.marketplace || panel.report.status === "unsupported") return;
    const name = panel.item.plugin.spec.name;
    try {
      await action({ action: "install", name, marketplace: panel.marketplace, expectedScan: panel.scan, scope: "global", allowPartial: panel.report.status === "partial" });
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
  const catalogInstalled = (name: string) => plugins.some(plugin => plugin.scope === "global" && plugin.source === market && plugin.spec.name === name);
  const rows = items(plugins, category).filter(row => matchesOrigin(isPublic(row.plugin)) && (installedStatus === "all" || (row.enabled ? "enabled" : "disabled") === installedStatus)).filter(row => matches([row.name, pluginDisplayName(row.plugin), row.plugin.source, row.plugin.spec.origin?.location, row.plugin.spec.description, ...row.plugin.spec.skills.map(skill => skill.description)]));
  const matchesStatus = (key: string, installed: boolean) => catalogStatus === "installed" ? installed : catalogStatus === "all" || (catalogStatus === "available" ? ["supported", "partial"].includes(checks[key]?.report.status) : (checks[key]?.report.status ?? (checkErrors[key] ? "failed" : "unchecked")) === catalogStatus);
  const catalogRows = catalog.filter(entry => matchesOrigin(marketplaceIsPublic(market)) && matchesStatus(entry.name, catalogInstalled(entry.name))).filter(entry => matches([entry.name, entry.description, JSON.stringify(entry.source), market, markets.find(source => source.name === market)?.source]));
  const resultCount = publicView ? catalogRows.length : rows.length;
  const reason = (key: string) => {
    const checked = checks[key];
    const reasons = checkErrors[key] ? [`${integrationError(checkErrors[key])} Refresh to retry.`] : checked ? compatibilityReasons(checked.spec) : [];
    if (!reasons.length && checked?.report.status === "unsupported") reasons.push("No installable skills, connections or hooks.");
    return <CompatibilityWarning reasons={reasons} />;
  };
  const badge = (key: string) => <span className={`themis-compatibility-badge ${checks[key]?.report.status ?? (checkErrors[key] ? "failed" : "unchecked")}`} title={checkErrors[key] ? `${integrationError(checkErrors[key])} Refresh to retry.` : checks[key] ? "Import format checked; runtime not tested" : "Compatibility check pending"} aria-live="polite">{checks[key] ? compatibilityLabels[checks[key].report.status] : checking[key] ? <><span className="themis-install-spinner" aria-hidden="true" />Checking…</> : checkErrors[key] ? "Check failed" : "Not checked"}</span>;
  const text = item ? markdown(item.plugin, file) : "";
  const server = item?.kind === "mcp" ? item.plugin.spec.mcp[item.id] : undefined;
  const hook = item?.kind === "hook" ? item.plugin.spec.hooks.find(hook => hook.name === item.id) : undefined;
  return <div className="themis-integrations">
    <h2>Integrations</h2>
    <nav className="themis-integrations-tabs" aria-label="Integration categories">{categories.map(value => <button key={value} aria-pressed={category === value} onClick={() => { previewRequest.current++; catalogRequest.current++; setCategory(value); setPublicView(false); setPanel(null); setBusy(false); }}>{value}</button>)}</nav>
    <div className="themis-integrations-toolbar"><div>{category === "Plugins" && <><button aria-pressed={!publicView} onClick={() => { previewRequest.current++; catalogRequest.current++; setPublicView(false); setPanel(null); setBusy(false); }}>Installed</button><button aria-pressed={publicView} onClick={() => { setPublicView(true); if (category === "Plugins" && markets[0]) void browse(markets.find(source => source.name === market)?.name ?? markets[0].name); }}>Discover</button></>}</div><div><button aria-label="Refresh" disabled={busy || mutating} onClick={() => { void (publicView && category === "Plugins" ? browse(market, true) : load()).catch(error => toast(dispatch, integrationError(error), "danger")); }}>↻</button></div></div>
    <input className="themis-integrations-search" type="search" aria-label="Search integrations" placeholder={`Search ${category.toLowerCase()}`} value={search} onChange={event => setSearch(event.target.value)} />
    <div className="themis-integration-filters">
      {publicView && category === "Plugins" && <div className="themis-marketplace-picker"><select aria-label="Marketplace" disabled={busy || mutating} value={market} onChange={event => void browse(event.target.value)}>{markets.map(m => <option key={m.name}>{m.name}</option>)}</select><button aria-label="Add marketplace" title="Add marketplace" disabled={busy || mutating} onClick={() => { setMarketName(""); setMarketSource(""); setAddingMarket(true); }}><Icon name="plus" /></button></div>}
      <select aria-label="Status" value={publicView ? catalogStatus : installedStatus} onChange={event => publicView ? setCatalogStatus(event.target.value) : setInstalledStatus(event.target.value)}>
        {publicView ? <><option value="available">Available</option><option value="all">All statuses</option><option value="installed">Installed</option><option value="supported">Compatible</option><option value="partial">Partially supported</option><option value="unsupported">Unsupported</option><option value="failed">Check failed</option><option value="unchecked">Not checked</option></> : <><option value="all">All statuses</option><option value="enabled">Enabled</option><option value="disabled">Disabled</option></>}
      </select>
      <select aria-label="Source" value={originFilter} onChange={event => setOriginFilter(event.target.value)}><option value="all">All sources</option><option value="personal">Personal</option><option value="public">Public</option></select>
    </div>
    <div className="themis-filter-summary"><span>{publicView && catalogLoading ? "Checking catalog" : `${resultCount} result${resultCount === 1 ? "" : "s"}`}</span><button onClick={() => { setSearch(""); setOriginFilter("all"); if (publicView) setCatalogStatus("available"); else setInstalledStatus("all"); }}>Reset filters</button></div>
    {publicView ? <>
      {catalogLoading ? <div className="themis-marketplace-loading"><KnightLoading variant={readCompanionPreferences().variant} status={scanProgress} progress={scanPercent} progressLabel="Marketplace compatibility" /></div> : !catalogRows.length && <EmptyState title={(search.trim() || originFilter !== "all" || (publicView ? catalogStatus !== "available" : installedStatus !== "all")) ? "No matching integrations" : "No plugins to show"} />}
      <ul className="themis-integrations-list themis-integration-cards">{catalogRows.map(entry => <li key={entry.name} data-check={entry.name}>
        <div className="themis-card-heading"><PluginIcon plugin={{ ...(checks[entry.name]?.spec ?? emptySpec()), icon: checks[entry.name]?.spec.icon ?? entry.icon }} /><button className="themis-integration-name themis-integration-open" aria-label={`View ${entry.name}`} disabled={busy || mutating} onClick={() => void preview(entry.name, entry.icon)}>{entry.name}</button></div>
        <span className="themis-integration-source">{market} · Plugin</span>{entry.description && <p className="themis-integration-description">{entry.description}</p>}
        {checks[entry.name] && <p className="themis-integration-capabilities">{capabilitySummary(checks[entry.name].spec)}</p>}
        {reason(entry.name)}
        <div className="themis-card-footer">{badge(entry.name)}<button aria-live="polite" aria-busy={installing === entry.name} disabled={busy || mutating || !checks[entry.name] || catalogInstalled(entry.name) || checks[entry.name]?.report.status === "unsupported"} onClick={() => void preview(entry.name, entry.icon, true)}>{installing === entry.name ? <><span className="themis-install-spinner" aria-hidden="true" />Installing…</> : catalogInstalled(entry.name) ? "✓ Installed" : "Install"}</button></div>
      </li>)}</ul>
    </> : <>
      {!rows.length && <EmptyState title={(search.trim() || originFilter !== "all" || (publicView ? catalogStatus !== "available" : installedStatus !== "all")) ? "No matching integrations" : `No ${category.toLowerCase()} installed`} />}
      <ul className="themis-integrations-list themis-installed-integrations">{rows.map(row => <li key={row.key}>
        <PluginIcon plugin={row.plugin.spec} fallback={row.kind === "plugin" ? "folder" : row.kind === "mcp" ? "plug" : row.kind === "hook" ? "hook" : skillGlyph(row.id)} /><button className="themis-integration-name themis-integration-open" aria-label={`View ${row.name}`} onClick={() => open(row)}>{row.name}</button><span className="themis-integration-source">{row.kind === "plugin" ? row.plugin.spec.origin?.kind === "discovered" ? "Discovered" : row.plugin.source ?? row.plugin.spec.origin?.kind ?? "Personal" : pluginDisplayName(row.plugin)} · {row.plugin.scope === "global" ? "User" : "Project"}</span>
        <p className="themis-integration-description">{row.kind === "skill" ? row.plugin.spec.skills.find(skill => skill.id === row.id)?.description : row.kind === "mcp" ? <><code>{mcpCommand(row.plugin.spec.mcp[row.id])}</code><br />Connection not checked</> : row.kind === "hook" ? row.plugin.spec.hooks.find(hook => hook.name === row.id)?.event : row.plugin.spec.description || capabilitySummary(row.plugin.spec)}</p>
        {row.unavailable && <CompatibilityWarning reasons={compatibilityReasons(row.plugin.spec)} />}
        <span className={`themis-integration-status themis-compatibility-badge ${row.enabled ? "supported" : "unchecked"}`}>{row.unavailable ? "Unavailable" : row.enabled ? "Enabled" : "Disabled"}</span>
        {!row.unavailable && <button className="themis-integration-toggle" role="switch" aria-label={`${row.name} enabled`} aria-checked={row.enabled} title={!row.plugin.enabled && row.kind !== "plugin" && isPluginPackage(row.plugin) ? "Enable the parent plugin first" : `Turn ${row.enabled ? "off" : "on"} ${row.name}`} disabled={busy || mutating || row.kind !== "plugin" && !row.plugin.enabled && isPluginPackage(row.plugin)} onClick={() => void toggle(row).catch(() => {})}><span className="themis-toggle-track"><span /></span></button>}
        {row.plugin.source !== "discovered" && row.plugin.spec.origin?.kind !== "discovered" && <button aria-label={`Uninstall ${row.name}`} title={`Uninstall ${row.name}`} disabled={busy || mutating} onClick={() => { setRemoving(row); }}><Icon name="trash" /></button>}
      </li>)}</ul>
    </>}
    <Dialog open={panel !== null} title={item?.name} onClose={() => { if (!mutating) { previewRequest.current++; setBusy(false); setPanel(null); } }}>
      <div className="themis-skill-viewer" key={`${item?.key}:${file}`}>
        {panel?.marketplace && busy ? <p role="status">Loading skills…</p> : <>
          {item?.kind === "plugin" && !file && !panel?.install && <><ul className="themis-bundled-skills">{item.plugin.spec.skills.map(skill => <li key={skill.id}><button aria-label={`View ${skill.name}`} onClick={() => setFile(skill.id)}><span>{skill.name}</span><Icon name="eye" /></button></li>)}</ul>{!item.plugin.spec.skills.length && <p>No skills</p>}</>}
          {item?.kind === "plugin" && file && <button className="themis-skill-back" aria-label="Back to skills" title="Back to skills" onClick={() => setFile("")}><Icon name="back" /></button>}
          {server ? <dl className="themis-integration-summary"><dt>Transport</dt><dd>{server.url ? "HTTP" : "Local process"}</dd><dt>{server.url ? "Endpoint" : "Command"}</dt><dd><code>{mcpCommand(server)}</code></dd><dt>Status</dt><dd>{item?.enabled ? "Enabled" : "Disabled"}</dd><dt>Source</dt><dd>{item && pluginDisplayName(item.plugin)}</dd></dl> : hook ? <dl className="themis-integration-summary"><dt>Event</dt><dd>{hook.event}</dd><dt>Status</dt><dd>{item?.enabled ? "Enabled" : "Disabled"}</dd><dt>Source</dt><dd>{item && pluginDisplayName(item.plugin)}</dd></dl> : text ? <ResponseBody text={text.replace(/^---\r?\n[\s\S]*?\r?\n---(?:\r?\n|$)/, "")} /> : null}
          {item && (panel?.install || item.unavailable) && <Compatibility spec={item.plugin.spec} report={panel.report} />}
        </>}
      </div>
      {panel?.install && panel.report && panel.report.status !== "unsupported" && !catalogInstalled(panel.item.id) && <div className="themis-preview-install"><Button disabled={busy || mutating} onClick={() => void installPreview()}>{panel.report.status === "partial" ? "Install supported parts" : "Install"}</Button></div>}
      <Button variant="ghost" disabled={mutating} onClick={() => { previewRequest.current++; setBusy(false); setPanel(null); }}>Close</Button>
    </Dialog>
    <Dialog open={addingMarket} title="Add marketplace" onClose={() => { if (!busy) setAddingMarket(false); }}>
      <form className="themis-marketplace-form" onSubmit={event => {
        event.preventDefault();
        if (busy || !marketName.trim() || !marketSource.trim()) return;
        void action({ action: "add_marketplace", name: marketName.trim(), source: marketSource.trim() }).then(async () => { setAddingMarket(false); await browse(marketName.trim()); }).catch(() => {});
      }}>
        <label>Name<input value={marketName} onChange={event => setMarketName(event.target.value)} required disabled={busy || mutating} /></label>
        <label>URL or local path<input value={marketSource} onChange={event => setMarketSource(event.target.value)} required disabled={busy || mutating} /></label>
        <div><Button type="button" variant="ghost" disabled={busy || mutating} onClick={() => setAddingMarket(false)}>Cancel</Button><Button type="submit" disabled={busy || mutating || !marketName.trim() || !marketSource.trim()}>Add</Button></div>
      </form>
    </Dialog>
    <Dialog open={removing !== null} title={`Uninstall ${removing?.name ?? ""}?`} onClose={() => { if (!busy) setRemoving(null); }}>
      <p>{removing?.kind === "plugin" ? "Remove this plugin and its bundled capabilities?" : `Remove this ${removing?.kind} from ${removing?.plugin.spec.name}? Other capabilities remain installed.`}</p>
      <Button variant="ghost" disabled={busy || mutating} onClick={() => setRemoving(null)}>Cancel</Button>
      <Button variant="danger" disabled={busy || mutating} onClick={() => { if (removing) void action({ action: removing.kind === "plugin" ? "delete" : "remove_component", kind: removing.kind, id: removing.id, name: removing.plugin.spec.name, scope: removing.plugin.scope }).then(() => setRemoving(null)).catch(() => {}); }}>Uninstall</Button>
    </Dialog>
  </div>;
}
