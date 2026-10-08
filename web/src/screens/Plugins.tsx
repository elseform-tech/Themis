import { useCallback, useEffect, useRef, useState } from "react";
import { Badge, Button, Dialog, EmptyState } from "../components";
import { ResponseBody } from "../components/ResponseBody";
import { pluginAction } from "../lib/tauri";
import { isPluginPackage } from "../lib/integrations";
import type { Marketplace, Plugin, PluginSpec } from "../lib/types";
import { describeError, useApp } from "../state/store";
import "./Plugins.css";

type Category = "Plugins" | "Skills" | "MCP" | "Hooks";
type Item = { key: string; name: string; plugin: Plugin; kind: "plugin" | "skill" | "mcp" | "hook"; id: string; enabled: boolean };
interface ImportReport {status:string;components:Array<{kind:string;name:string;status:string;field?:string|null;reason?:string|null;remedy?:string|null}>}
interface ImportPreview {spec:PluginSpec;report:ImportReport}
function verificationKey(item:Item) {return `${item.key}:${item.plugin.revision}:${JSON.stringify(item.kind==="mcp"?item.plugin.spec.mcp[item.id]:item.plugin.spec.hooks.find(hook=>hook.name===item.id))}`;}
type ImportRequest={action:string;[key:string]:unknown};
type Panel = { item: Item; marketplace?: string; skillSource?: string };
type Status = "Ready" | "Disabled" | "Not tested" | "Limited support" | "Failed";
const statuses: Status[] = ["Ready", "Disabled", "Not tested", "Limited support", "Failed"];
const categories: Category[] = ["Plugins", "Skills", "MCP", "Hooks"];
const skillRepository = "https://github.com/anthropics/skills.git";
const skillReference = "683bc88e56f3e09ba94f7055977f3d3aa499f202";
const publicSkills = [
  { id: "pdf", name: "PDF", description: "Read, create, and work with PDF documents." },
  { id: "docx", name: "Word documents", description: "Create and edit Word documents." },
  { id: "xlsx", name: "Spreadsheets", description: "Create, edit, and analyze spreadsheets." },
];
type Glyph = "folder" | "plug" | "file" | "hook" | "pdf" | "docx" | "xlsx" | "eye" | "trash" | "plus" | "back";
const skillGlyph = (id: string): Glyph => ["pdf", "docx", "xlsx"].includes(id) ? id as Glyph : "file";
const emptySpec = (): PluginSpec => ({ name: "personal", description: "", version: "", skills: [], mcp: {}, hooks: [], files: {}, unsupported: [] });
function pluginDisplayName(plugin: Plugin) {
  return plugin.source === "discovered" || plugin.spec.origin?.kind === "discovered" ? plugin.spec.skills[0]?.name ?? plugin.spec.origin?.location.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? "Unavailable skill" : plugin.spec.name;
}
function items(plugins: Plugin[], category: Category): Item[] {
  return plugins.flatMap<Item>(plugin => {
    const base = { plugin, key: `${plugin.scope}:${plugin.spec.name}` };
    if (category === "Plugins") return isPluginPackage(plugin) ? [{ ...base, name: pluginDisplayName(plugin), kind: "plugin" as const, id: plugin.spec.name, enabled: plugin.enabled }] : [];
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
export function Plugins() {
  const { state, dispatch } = useApp();
  const root = state.activeProjectRoot;
  const activeRoot = useRef(root);
  activeRoot.current = root;
  const loadRequest = useRef(0), catalogRequest = useRef(0);
  const [plugins, setPlugins] = useState<Plugin[]>([]), [markets, setMarkets] = useState<Marketplace[]>([]);
  const [importScope, setImportScope] = useState<"local" | "global">(root ? "local" : "global");
  const [category, setCategory] = useState<Category>("Plugins"), [publicView, setPublicView] = useState(false);
  const [panel, setPanel] = useState<Panel | null>(null), [removing, setRemoving] = useState<Item | null>(null);
  const [error, setError] = useState(""), [busy, setBusy] = useState(false);
  const errorLines = error.split("\n");
  const operationLine = errorLines[errorLines.length - 1] ?? "";
  const diagnosticOperation = operationLine.startsWith("Diagnostic operation: ")
    ? operationLine.slice("Diagnostic operation: ".length) : "";
  const errorView = error ? <div><p role="alert">{error}</p>{/^[0-9a-f-]{36}$/.test(diagnosticOperation) && <Button variant="ghost" onClick={() => dispatch({type:"ui/view",view:"settings",settingsSection:"Diagnostics",diagnosticOperation})}>View diagnostics</Button>}</div> : null;
  const [file, setFile] = useState("");
  const [importRequest,setImportRequest]=useState<ImportRequest|null>(null),[importPreview,setImportPreview]=useState<ImportPreview|null>(null),[partial,setPartial]=useState(false);
  const [importing,setImporting]=useState(false),[importKind,setImportKind]=useState("path"),[importValue,setImportValue]=useState(""),[importName,setImportName]=useState("");
  async function inspectImport(request:ImportRequest) {
    const requestId=++previewRequest.current;
    setImportRequest(request);setImportPreview(null);setPartial(false);setBusy(true);setError("");
    const inspection=request.action==="install"?"inspect_marketplace":request.action==="import_repository"?"inspect_repository":request.action==="import_json"?"inspect_json":"inspect_path";
    try{const value=await pluginAction<ImportPreview>({...request,action:inspection,projectRoot:root});if(requestId===previewRequest.current&&root===activeRoot.current)setImportPreview(value);}catch(error){if(requestId===previewRequest.current){setError(describeError(error));if(request.action==="import_path"||request.action==="import_json")setImportRequest(null);}}finally{if(requestId===previewRequest.current)setBusy(false);}
  }
  async function completeImport(){if(!importRequest||!importPreview)return;await action({...importRequest,allowPartial:partial});setImportRequest(null);setImportPreview(null);setImporting(false);}

  const [addingMarket, setAddingMarket] = useState(false), [marketName, setMarketName] = useState(""), [marketSource, setMarketSource] = useState("");
  const previewRequest = useRef(0);
  const [search, setSearch] = useState("");
  const [statusFilter, setStatusFilter] = useState("all");
  const [verification, setVerification] = useState<Record<string, {ok:boolean;at:string;message:string}>>({});
  function status(item:Item):Status {
    if (!item.enabled) return "Disabled";
    if (verification[verificationKey(item)]?.ok === false) return "Failed";
    if (item.kind === "plugin" && item.plugin.spec.unsupported.length) return "Limited support";
    return item.kind === "mcp" || item.kind === "hook" ? verification[verificationKey(item)]?.ok ? "Ready" : "Not tested" : "Ready";
  }
  function badge(item:Item) { const value=status(item);return <Badge tone={value==="Ready"?"success":value==="Failed"?"danger":value==="Limited support"?"warning":"default"}>{value}</Badge>; }
  async function testConnection(item:Item) {
    setBusy(true);setError("");
    try {
      const value=await pluginAction<{tools:unknown[]}>({action:"test_mcp",projectRoot:root,server:item.plugin.spec.mcp[item.id]});
      if(root!==activeRoot.current)return;
      setVerification(current=>({...current,[verificationKey(item)]:{ok:true,at:new Date().toISOString(),message:`Connected · ${value.tools.length} tools discovered`}}));
    } catch(error) {if(root!==activeRoot.current)return;const message=describeError(error);setError(message);setVerification(current=>({...current,[verificationKey(item)]:{ok:false,at:new Date().toISOString(),message}}));}
    finally {if(root===activeRoot.current)setBusy(false);}
  }
  const [catalog, setCatalog] = useState<Array<{ name: string; description?: string; icon?: string; source?: unknown }>>([]), [market, setMarket] = useState("");
  const load = useCallback(async () => {
    if (root !== activeRoot.current) return;
    const request = ++loadRequest.current;
    const [installed, sources] = await Promise.all([pluginAction<Plugin[]>({ action: "list", projectRoot: root }), pluginAction<Marketplace[]>({ action: "marketplaces" })]);
    if (request === loadRequest.current && root === activeRoot.current) { setPlugins(installed); setMarkets(sources); setError(""); }
  }, [root]);
  useEffect(() => {
    let cancelled = false;
    setPlugins([]);setImportScope(root?"local":"global"); setVerification({}); setImportRequest(null);setImportPreview(null);setImporting(false);setAddingMarket(false); setPanel(null); setRemoving(null); setCatalog([]); setPublicView(false); catalogRequest.current++; previewRequest.current++; setBusy(false);
    void load().catch(error => { if (!cancelled) setError(describeError(error)); });
    return () => { cancelled = true; };
  }, [load, root]);
  useEffect(() => {
    const refresh = () => void load().catch(error => setError(describeError(error)));
    window.addEventListener("themis-plugins-changed", refresh);
    return () => window.removeEventListener("themis-plugins-changed", refresh);
  }, [load]);
  async function action(args: Record<string, unknown>) {
    setBusy(true); setError("");
    try {
      await pluginAction({ projectRoot: root, ...args });
      await load();
      window.dispatchEvent(new Event("themis-plugins-changed"));
    } catch (error) { setError(describeError(error)); throw error; }
    finally { setBusy(false); }
  }
  async function toggle(item: Item) {
    const args = { scope: item.plugin.scope, name: item.plugin.spec.name };
    if (item.kind !== "plugin" && !item.plugin.enabled && !isPluginPackage(item.plugin)) await action({ ...args, action: "enable" });
    await action(item.kind === "plugin" ? { ...args, action: item.enabled ? "disable" : "enable" } : { ...args, action: "set_component_enabled", kind: item.kind, id: item.id, enabled: !item.enabled });
  }
  function open(item: Item) {
    previewRequest.current++;
    setBusy(false); setError(""); setPanel({ item });
    setFile(item.kind === "plugin" ? "" : item.id);
  }
  async function browse(selected: string) {
    const request = ++catalogRequest.current;
    setMarket(selected); setCatalog([]); setBusy(true); setError("");
    try { const value = await pluginAction<{ plugins: typeof catalog }>({ action: "catalog", name: selected }); if (request === catalogRequest.current) setCatalog(value.plugins); }
    catch (error) { if (request === catalogRequest.current) setError(describeError(error)); } finally { if (request === catalogRequest.current) setBusy(false); }
  }
  async function preview(name: string, icon?: string) {
    const marketplace = market;
    const spec = { ...emptySpec(), name, icon };
    const plugin: Plugin = { scope: "global", revision: "preview", enabled: false, source: marketplace, spec };
    const previewItem: Item = { key: `preview:${marketplace}:${name}`, name, id: name, kind: "plugin", enabled: false, plugin };
    open(previewItem);
    const request = previewRequest.current;
    setPanel({ item: previewItem, marketplace });
    setBusy(true);
    try {
      const value = await pluginAction<PluginSpec>({ action: "preview", marketplace, name, projectRoot: root });
      if (request !== previewRequest.current || root !== activeRoot.current) return;
      setPanel(current => current?.marketplace === marketplace && current.item?.id === name ? { ...current, item: { ...previewItem, plugin: { ...plugin, spec: { ...value, icon: value.icon ?? icon } } } } : current);
      setFile("");
    } catch (error) { if (request === previewRequest.current) setError(describeError(error)); } finally { if (request === previewRequest.current) setBusy(false); }
  }
  async function previewSkill(skill: typeof publicSkills[number]) {
    const skillSource = `https://github.com/anthropics/skills/blob/${skillReference}/skills/${skill.id}/SKILL.md`;
    const spec = { ...emptySpec(), name: skill.id, package_kind: "skill" as const };
    const plugin: Plugin = { scope: "global", revision: "preview", enabled: false, source: null, spec };
    const previewItem: Item = { key: `public-skill:${skill.id}`, name: skill.name, id: skill.id, kind: "skill", enabled: false, plugin };
    open(previewItem);
    const request = previewRequest.current;
    setPanel(current => current ? { ...current, skillSource } : current);
    setBusy(true);
    try {
      const value = await pluginAction<PluginSpec>({ action: "preview_repository", url: skillRepository, name: skill.id, reference: skillReference, subdirectory: `skills/${skill.id}`, projectRoot: root });
      if (request !== previewRequest.current || root !== activeRoot.current) return;
      setPanel(current => current?.skillSource === skillSource ? { ...current, item: { ...previewItem, id: value.skills[0]?.id ?? skill.id, plugin: { ...plugin, spec: value } } } : current);
      setFile(value.skills[0]?.id ?? "");
    } catch (error) { if (request === previewRequest.current) setError(describeError(error)); } finally { if (request === previewRequest.current) setBusy(false); }
  }
  const item = panel?.item;
  const terms = search.trim().toLowerCase().split(/\s+/).filter(Boolean);
  const matches = (values: unknown[]) => { const text = values.filter(Boolean).join(" ").toLowerCase(); return terms.every(term => text.includes(term)); };
  const rows = items(plugins, category).filter(row => statusFilter === "all" || status(row) === statusFilter).filter(row => matches([row.name, pluginDisplayName(row.plugin), row.plugin.source, row.plugin.spec.origin?.location, row.plugin.spec.description, ...row.plugin.spec.skills.map(skill => skill.description)]));
  const catalogRows = catalog.filter(entry => !plugins.some(plugin => plugin.source === market && plugin.spec.name === entry.name)).filter(entry => matches([entry.name, entry.description, JSON.stringify(entry.source), market, markets.find(source => source.name === market)?.source]));
  const skillRows = publicSkills.filter(skill => !plugins.some(plugin => plugin.spec.skills.length > 0 && (plugin.spec.origin
    ? plugin.spec.origin.location.replace(/\.git$/, "") === skillRepository.replace(/\.git$/, "") && plugin.spec.origin.subdirectory === `skills/${skill.id}`
    : plugin.spec.package_kind === "skill" && plugin.spec.skills.some(existing => existing.id === skill.id))))
    .filter(skill => matches([skill.id, skill.name, skill.description, "Anthropic", skillRepository]));
  const text = item ? markdown(item.plugin, file) : "";
  const server = item?.kind === "mcp" ? item.plugin.spec.mcp[item.id] : undefined;
  const hook = item?.kind === "hook" ? item.plugin.spec.hooks.find(hook => hook.name === item.id) : undefined;
  return <div className="themis-integrations">
    <h2>Integrations</h2>
    <nav className="themis-integrations-tabs" aria-label="Integration categories">{categories.map(value => <button key={value} aria-pressed={category === value} onClick={() => { setCategory(value); setPublicView(false); }}>{value}</button>)}</nav>
    <div className="themis-integrations-toolbar"><div>{(category === "Plugins" || category === "Skills") && <><button aria-pressed={!publicView} onClick={() => setPublicView(false)}>Installed</button><button aria-pressed={publicView} onClick={() => { setPublicView(true); if (category === "Plugins" && markets[0]) void browse(markets[0].name); }}>Not installed</button></>}</div><div><button disabled={busy} onClick={()=>{setImporting(true);setImportRequest(null);setImportPreview(null);setError("");}}>Import</button><button aria-label="Refresh" disabled={busy} onClick={() => void load().catch(error => setError(describeError(error)))}>↻</button></div></div>
    <label className="themis-status-filter">Install for <select aria-label="Installation scope" value={importScope} disabled={busy} onChange={event=>setImportScope(event.target.value as "local"|"global")}><option value="global">User (Global)</option><option value="local" disabled={!root}>Project</option></select></label>
    <input className="themis-integrations-search" type="search" aria-label="Search integrations" placeholder={`Search ${publicView ? `available ${category.toLowerCase()}` : category.toLowerCase()}`} value={search} onChange={event => setSearch(event.target.value)} />
    {!publicView && <label className="themis-status-filter">Status <select aria-label="Integration status" value={statusFilter} onChange={event=>setStatusFilter(event.target.value)}><option value="all">All statuses</option>{statuses.map(value=><option key={value}>{value}</option>)}</select></label>}
    {error && !panel && !addingMarket && !importing && !importRequest && errorView}
    {publicView && category === "Skills" ? <>
      {!skillRows.length && <EmptyState title={search.trim() ? "No matching integrations" : "All available skills installed"} />}
      <ul className="themis-integrations-list">{skillRows.map(skill => <li key={skill.id}><Icon name={skillGlyph(skill.id)} /><button className="themis-integration-name themis-integration-open" aria-label={`View ${skill.name}`} onClick={() => void previewSkill(skill)}>{skill.name}</button><span className="themis-integration-source">Anthropic</span><button disabled={busy} onClick={() => void inspectImport({ action: "import_repository", url: skillRepository, reference: skillReference, subdirectory: `skills/${skill.id}`, name: skill.id, scope: importScope })}>Install</button></li>)}</ul>
    </> : publicView ? <>
      <div className="themis-integrations-toolbar"><div className="themis-marketplace-picker"><select aria-label="Marketplace" disabled={busy} value={market} onChange={event => void browse(event.target.value)}>{markets.map(m => <option key={m.name}>{m.name}</option>)}</select><button aria-label="Add marketplace" title="Add marketplace" disabled={busy} onClick={() => { setError(""); setMarketName(""); setMarketSource(""); setAddingMarket(true); }}><Icon name="plus" /></button></div></div>
      {!catalogRows.length && <EmptyState title={search.trim() ? "No matching integrations" : "No public plugins available"} />}
      <ul className="themis-integrations-list">{catalogRows.map(entry => <li key={entry.name}><PluginIcon plugin={{ ...emptySpec(), icon: entry.icon }} /><button className="themis-integration-name themis-integration-open" aria-label={`View ${entry.name}`} disabled={busy} onClick={() => void preview(entry.name, entry.icon)}>{entry.name}</button><button disabled={busy} onClick={() => void inspectImport({ action: "install", name: entry.name, marketplace: market, scope: importScope })}>Install</button></li>)}</ul>
    </> : <>
      {!rows.length && <EmptyState title={search.trim() || statusFilter !== "all" ? "No matching integrations" : `No ${category.toLowerCase()} installed`} />}
      <ul className="themis-integrations-list">{rows.map(row => <li key={row.key}>
        <PluginIcon plugin={row.plugin.spec} fallback={row.kind === "plugin" ? "folder" : row.kind === "mcp" ? "plug" : row.kind === "hook" ? "hook" : skillGlyph(row.id)} /><button className="themis-integration-name themis-integration-open" aria-label={`View ${row.name}`} onClick={() => open(row)}>{row.name}</button><span className="themis-integration-source">{row.kind === "plugin" ? row.plugin.spec.origin?.kind === "discovered" ? "Discovered" : row.plugin.source ?? row.plugin.spec.origin?.kind ?? "Personal" : pluginDisplayName(row.plugin)} · {row.plugin.scope === "global" ? "User" : "Project"}</span>
        <span className="themis-integration-status">{row.enabled && <span className="themis-sr-only">Enabled · </span>}{badge(row)}</span>
        <button className="themis-integration-toggle" role="switch" aria-label={`${row.name} enabled`} aria-checked={row.enabled} title={!row.plugin.enabled && row.kind !== "plugin" && isPluginPackage(row.plugin) ? "Enable the parent plugin first" : `Turn ${row.enabled ? "off" : "on"} ${row.name}`} disabled={busy || row.kind !== "plugin" && !row.plugin.enabled && isPluginPackage(row.plugin)} onClick={() => void toggle(row).catch(() => {})}><span className="themis-toggle-track"><span /></span></button>
        {row.plugin.source !== "discovered" && row.plugin.spec.origin?.kind !== "discovered" && <button aria-label={`Uninstall ${row.name}`} title={`Uninstall ${row.name}`} disabled={busy} onClick={() => { setError(""); setRemoving(row); }}><Icon name="trash" /></button>}
      </li>)}</ul>
    </>}
    <Dialog open={importing || importRequest !== null} title="Import integration" onClose={()=>{if(!busy){setImporting(false);setImportRequest(null);setImportPreview(null);}}}>
      {!importRequest && <form className="themis-marketplace-form" onSubmit={event=>{event.preventDefault();void inspectImport({action:importKind==="json"?"import_json":"import_path",[importKind==="json"?"content":"path"]:importValue,name:importName||undefined,scope:importScope});}}><label>Install for<select aria-label="Import installation scope" value={importScope} onChange={event=>setImportScope(event.target.value as "local"|"global")}><option value="global">User (Global)</option><option value="local" disabled={!root}>Project</option></select></label><label>Source format<select aria-label="Import source format" value={importKind} onChange={event=>setImportKind(event.target.value)}><option value="path">Local package or configuration path</option><option value="json">MCP configuration JSON</option></select></label><label>Name<input value={importName} onChange={event=>setImportName(event.target.value)} /></label><label>{importKind==="json"?"Configuration JSON":"Path"}<textarea aria-label="Import source" value={importValue} onChange={event=>setImportValue(event.target.value)} rows={importKind==="json"?8:2} /></label><Button disabled={busy||!importValue.trim()} type="submit">Inspect</Button></form>}
      {busy && <p role="status">Inspecting…</p>}
      {importPreview && <><p>Install for {importRequest?.scope==="global"?"User (Global)":"Project"}</p><h3>{importPreview.spec.name}</h3><p>Compatibility: {importPreview.report.status}</p><ul className="themis-import-report">{importPreview.report.components.map((component,index)=><li key={index}><strong>{component.kind} · {component.name}</strong><Badge tone={component.status==="supported"?"success":"warning"}>{component.status}</Badge>{component.field&&<code>{component.field}</code>}{component.reason&&<p>{component.reason}</p>}{component.remedy&&<p>{component.remedy}</p>}</li>)}</ul>{importPreview.report.status!=="supported"&&<label><input type="checkbox" checked={partial} onChange={event=>setPartial(event.target.checked)} />Install supported components only; leave incompatible components disabled</label>}<p className="themis-settings-hint">Import validation does not start servers, execute hooks or verify connections.</p><Button disabled={busy || importPreview.report.status!=="supported"&&!partial} onClick={()=>void completeImport().catch(()=>{})}>Install</Button></>}
      {errorView}<Button variant="ghost" disabled={busy} onClick={()=>{setImporting(false);setImportRequest(null);setImportPreview(null);}}>Cancel</Button>
    </Dialog>
    <Dialog open={panel !== null} title={item?.name} onClose={() => { previewRequest.current++; setBusy(false); setPanel(null); }}>
      <div className="themis-skill-viewer" key={`${item?.key}:${file}`}>
        {(panel?.marketplace || panel?.skillSource) && busy ? <p role="status">Loading skills…</p> : <>
          {item && <dl className="themis-integration-summary"><dt>Source</dt><dd>{item.plugin.spec.origin?.location ?? item.plugin.source ?? "Personal"}</dd><dt>Revision</dt><dd>{item.plugin.revision}</dd><dt>Readiness</dt><dd>{badge(item)}</dd><dt>Last connection test</dt><dd>{verification[verificationKey(item)] ? `${verification[verificationKey(item)].at} · ${verification[verificationKey(item)].message}` : "Not tested in this session"}</dd></dl>}
          {server && item && <Button variant="ghost" disabled={busy || !root || !!panel?.marketplace} onClick={()=>void testConnection(item)}>Test connection</Button>}
          {item?.kind === "plugin" && !file && <><p className="themis-integration-skill-count">{item.plugin.spec.skills.length} {item.plugin.spec.skills.length === 1 ? "skill" : "skills"}</p><ul className="themis-bundled-skills">{item.plugin.spec.skills.map(skill => <li key={skill.id}><button aria-label={`View ${skill.name}`} aria-pressed={file === skill.id} onClick={() => setFile(skill.id)}><span>{skill.name}</span><Icon name="eye" /></button></li>)}</ul></>}
          {item?.kind === "plugin" && file && <button className="themis-skill-back" aria-label="Back to skills" title="Back to skills" onClick={() => setFile("")}><Icon name="back" /></button>}
          {server ? <dl className="themis-integration-summary"><dt>Transport</dt><dd>{server.url ? "HTTP" : "Local process"}</dd><dt>{server.url ? "Endpoint" : "Command"}</dt><dd>{server.url ?? server.command}</dd><dt>Status</dt><dd>{item?.enabled ? "Enabled" : "Disabled"}</dd><dt>Source</dt><dd>{item && pluginDisplayName(item.plugin)}</dd></dl> : hook ? <dl className="themis-integration-summary"><dt>Event</dt><dd>{hook.event}</dd><dt>Status</dt><dd>{item?.enabled ? "Enabled" : "Disabled"}</dd><dt>Source</dt><dd>{item && pluginDisplayName(item.plugin)}</dd></dl> : text ? <ResponseBody text={text.replace(/^---\r?\n[\s\S]*?\r?\n---(?:\r?\n|$)/, "")} /> : !item?.plugin.spec.skills.length ? <p>No bundled skills.</p> : null}
          {!!item?.plugin.spec.import_issues?.length && <details><summary>Import issues</summary><ul>{item.plugin.spec.import_issues.map((issue,index)=><li key={index}><strong>{issue.kind} · {issue.name}</strong>{issue.field&&<code>{issue.field}</code>}<p>{issue.reason}</p>{issue.remedy&&<p>{issue.remedy}</p>}</li>)}</ul></details>}
          {!!item?.plugin.spec.unsupported.length && <details><summary>Compatibility notes</summary><ul>{item.plugin.spec.unsupported.map((warning, i) => <li key={i}>{warning}</li>)}</ul></details>}
        </>}
      </div>
      {errorView}<Button variant="ghost" onClick={() => { previewRequest.current++; setBusy(false); setPanel(null); }}>Close</Button>
    </Dialog>
    <Dialog open={addingMarket} title="Add marketplace" onClose={() => { if (!busy) setAddingMarket(false); }}>
      <form className="themis-marketplace-form" onSubmit={event => {
        event.preventDefault();
        if (busy || !marketName.trim() || !marketSource.trim()) return;
        void action({ action: "add_marketplace", name: marketName.trim(), source: marketSource.trim() }).then(async () => { setAddingMarket(false); await browse(marketName.trim()); }).catch(() => {});
      }}>
        <label>Name<input value={marketName} onChange={event => setMarketName(event.target.value)} required disabled={busy} /></label>
        <label>URL or local path<input value={marketSource} onChange={event => setMarketSource(event.target.value)} required disabled={busy} /></label>
        {errorView}
        <div><Button type="button" variant="ghost" disabled={busy} onClick={() => setAddingMarket(false)}>Cancel</Button><Button type="submit" disabled={busy || !marketName.trim() || !marketSource.trim()}>Add</Button></div>
      </form>
    </Dialog>
    <Dialog open={removing !== null} title={`Uninstall ${removing?.name ?? ""}?`} onClose={() => { if (!busy) setRemoving(null); }}>
      <p>{removing?.kind === "plugin" ? "Remove this plugin and its bundled capabilities?" : `Remove this ${removing?.kind} from ${removing?.plugin.spec.name}? Other capabilities remain installed.`}</p>
      {errorView}
      <Button variant="ghost" disabled={busy} onClick={() => setRemoving(null)}>Cancel</Button>
      <Button variant="danger" disabled={busy} onClick={() => { if (removing) void action({ action: removing.kind === "plugin" ? "delete" : "remove_component", kind: removing.kind, id: removing.id, name: removing.plugin.spec.name, scope: removing.plugin.scope }).then(() => setRemoving(null)).catch(() => {}); }}>Uninstall</Button>
    </Dialog>
  </div>;
}
