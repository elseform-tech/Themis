import { useCallback, useEffect, useRef, useState } from "react";
import { Button, Dialog, EmptyState } from "../components";
import { CodeBlock } from "../components/CodeBlock";
import { ResponseBody } from "../components/ResponseBody";
import { pluginAction } from "../lib/tauri";
import { isPluginPackage } from "../lib/integrations";
import type { Marketplace, Plugin, PluginSpec } from "../lib/types";
import { describeError, useApp } from "../state/store";
import "./Plugins.css";

type Category = "Plugins" | "Skills" | "MCP" | "Hooks";
type Item = { key: string; name: string; plugin: Plugin; kind: "plugin" | "skill" | "mcp" | "hook"; id: string; enabled: boolean };
type Panel = { mode: "view" | "edit" | "uninstall" | "import" | "create" | "market"; item?: Item; marketplace?: string; skillSource?: string };
const categories: Category[] = ["Plugins", "Skills", "MCP", "Hooks"];
const hookEvents = ["RunStart", "BeforeTool", "AfterTool", "BeforeCompaction", "RunEnd"];
const skillRepository = "https://github.com/anthropics/skills.git";
const skillReference = "683bc88e56f3e09ba94f7055977f3d3aa499f202";
const publicSkills = [
  { id: "pdf", name: "PDF", description: "Read, create, and work with PDF documents." },
  { id: "docx", name: "Word documents", description: "Create and edit Word documents." },
  { id: "xlsx", name: "Spreadsheets", description: "Create, edit, and analyze spreadsheets." },
];
type Glyph = "more" | "add" | "folder" | "plug" | "file" | "hook" | "pdf" | "docx" | "xlsx";
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
    {name === "more" && <><circle cx="5" cy="12" r="1" /><circle cx="12" cy="12" r="1" /><circle cx="19" cy="12" r="1" /></>}
    {name === "add" && <path d="M12 5v14M5 12h14" />}
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
  const { state } = useApp();
  const root = state.activeProjectRoot;
  const activeRoot = useRef(root);
  activeRoot.current = root;
  const loadRequest = useRef(0), catalogRequest = useRef(0);
  const [plugins, setPlugins] = useState<Plugin[]>([]), [markets, setMarkets] = useState<Marketplace[]>([]);
  const [category, setCategory] = useState<Category>("Plugins"), [publicView, setPublicView] = useState(false);
  const [panel, setPanel] = useState<Panel | null>(null), [menu, setMenu] = useState<string | null>(null);
  const [error, setError] = useState(""), [busy, setBusy] = useState(false), [draft, setDraft] = useState("");
  const [file, setFile] = useState(""), [scope, setScope] = useState<"local" | "global">(root ? "local" : "global");
  const [reference, setReference] = useState(""), [subdirectory, setSubdirectory] = useState("");
  const [sourceType, setSourceType] = useState("paste"), [source, setSource] = useState(""), [name, setName] = useState("");
  const [testResult, setTestResult] = useState<unknown>(null);
  const [search, setSearch] = useState("");
  const [readerMode, setReaderMode] = useState<"source" | "rendered">("source");
  const [catalog, setCatalog] = useState<Array<{ name: string; description?: string; icon?: string; source?: unknown }>>([]), [market, setMarket] = useState("");
  const load = useCallback(async () => {
    if (root !== activeRoot.current) return;
    const request = ++loadRequest.current;
    const [installed, sources] = await Promise.all([pluginAction<Plugin[]>({ action: "list", projectRoot: root }), pluginAction<Marketplace[]>({ action: "marketplaces" })]);
    if (request === loadRequest.current && root === activeRoot.current) { setPlugins(installed); setMarkets(sources); setError(""); }
  }, [root]);
  useEffect(() => {
    let cancelled = false;
    setPlugins([]); setPanel(null); setMenu(null);
    void load().catch(error => { if (!cancelled) setError(describeError(error)); });
    return () => { cancelled = true; };
  }, [load]);
  useEffect(() => {
    const refresh = () => void load().catch(error => setError(describeError(error)));
    window.addEventListener("themis-plugins-changed", refresh);
    return () => window.removeEventListener("themis-plugins-changed", refresh);
  }, [load]);
  useEffect(() => {
    if (!menu) return;
    const close = (event: Event) => { if (event instanceof KeyboardEvent && event.key === "Escape" || event instanceof MouseEvent && !(event.target as Element)?.closest(".themis-integration-more")) setMenu(null); };
    window.addEventListener("keydown", close); window.addEventListener("click", close);
    return () => { window.removeEventListener("keydown", close); window.removeEventListener("click", close); };
  }, [menu]);
  async function action(args: Record<string, unknown>) {
    setBusy(true); setError("");
    try { const value = await pluginAction<unknown>({ projectRoot: root, ...args }); await load(); window.dispatchEvent(new Event("themis-plugins-changed")); return value; }
    catch (error) { setError(describeError(error)); throw error; } finally { setBusy(false); }
  }
  function open(mode: Panel["mode"], item?: Item) {
    setMenu(null); setError(""); setTestResult(null); setReaderMode("source"); setPanel({ mode, item });
    setFile(item?.kind === "plugin" ? item.plugin.spec.skills[0]?.id ?? "" : item?.id ?? "");
    const template = emptySpec();
    let number = 1;
    while (plugins.some(plugin => plugin.spec.name === `personal-${number}`)) number++;
    template.name = `personal-${number}`;
    template.package_kind = category === "Plugins" ? "plugin" : category === "Skills" ? "skill" : category === "MCP" ? "mcp" : "hook";
    if (!item && category === "Skills") template.skills = [{ id: "my-skill", name: "My skill", description: "", instructions: "Describe the workflow here.", allowedTools: [], scripts: [] }];
    if (!item && category === "MCP") template.mcp = { server: { command: "npx", args: ["-y", "package-name"], env: {}, enabled: true } };
    if (!item && category === "Hooks") template.hooks = [{ name: "my-hook", event: "RunStart", command: "printf '{}'", enabled: false, timeout_seconds: 10, blocking: false }];
    setDraft(JSON.stringify(item?.plugin.spec ?? template, null, 2)); setName(""); setSource(""); setSourceType("path");
  }
  async function toggle(item: Item) {
    const args = { scope: item.plugin.scope, name: item.plugin.spec.name, expectedRevision: item.plugin.revision };
    if (item.kind !== "plugin" && !item.plugin.enabled && !isPluginPackage(item.plugin)) {
      await action({ ...args, action: "enable" });
    }
    await action(item.kind === "plugin" ? { ...args, action: item.enabled ? "disable" : "enable" } : { ...args, action: "set_component_enabled", kind: item.kind, id: item.id, enabled: !item.enabled });
  }
  function personalCopy(plugin: Plugin) {
    open("create");
    let number = 1;
    while (plugins.some(existing => existing.spec.name === `${plugin.spec.name}-copy-${number}`)) number++;
    setDraft(JSON.stringify({ ...plugin.spec, name: `${plugin.spec.name}-copy-${number}` }, null, 2));
  }
  async function saveDraft() {
    try { const spec = JSON.parse(draft) as PluginSpec; await action({ action: "save", scope: panel?.item?.plugin.scope ?? scope, spec, expectedRevision: panel?.item?.plugin.revision ?? null }); setPanel(null); }
    catch (error) { setError(describeError(error)); }
  }
  async function importSource() {
    const args = sourceType === "paste" ? { action: "import_json", content: source, name: name || null } : sourceType === "repository" ? { action: "import_repository", url: source, name: name || "", reference: reference || null, subdirectory: subdirectory || null } : { action: "import_path", path: source, name: name || null };
    await action({ ...args, scope }); setPanel(null);
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
    const plugin: Plugin = { scope, revision: "preview", enabled: false, source: marketplace, spec };
    const previewItem: Item = { key: `preview:${marketplace}:${name}`, name, id: name, kind: "plugin", enabled: false, plugin };
    open("view", previewItem);
    setPanel({ mode: "view", item: previewItem, marketplace });
    setBusy(true);
    try {
      const value = await pluginAction<PluginSpec>({ action: "preview", marketplace, name, projectRoot: root });
      setPanel(current => current?.marketplace === marketplace && current.item?.id === name ? { ...current, item: { ...previewItem, plugin: { ...plugin, spec: { ...value, icon: value.icon ?? icon } } } } : current);
      setFile(value.skills[0]?.id ?? "");
    } catch (error) { setError(describeError(error)); } finally { setBusy(false); }
  }
  async function previewSkill(skill: typeof publicSkills[number]) {
    const skillSource = `https://github.com/anthropics/skills/blob/${skillReference}/skills/${skill.id}/SKILL.md`;
    const spec = { ...emptySpec(), name: skill.id, package_kind: "skill" as const };
    const plugin: Plugin = { scope, revision: "preview", enabled: false, source: null, spec };
    const previewItem: Item = { key: `public-skill:${skill.id}`, name: skill.name, id: skill.id, kind: "skill", enabled: false, plugin };
    open("view", previewItem);
    setPanel(current => current ? { ...current, skillSource } : current);
    setBusy(true);
    try {
      const value = await pluginAction<PluginSpec>({ action: "preview_repository", url: skillRepository, name: skill.id, reference: skillReference, subdirectory: `skills/${skill.id}`, projectRoot: root });
      setPanel(current => current?.skillSource === skillSource ? { ...current, item: { ...previewItem, id: value.skills[0]?.id ?? skill.id, plugin: { ...plugin, spec: value } } } : current);
      setFile(value.skills[0]?.id ?? "");
    } catch (error) { setError(describeError(error)); } finally { setBusy(false); }
  }
  const item = panel?.item;
  const terms = search.trim().toLowerCase().split(/\s+/).filter(Boolean);
  const matches = (values: unknown[]) => { const text = values.filter(Boolean).join(" ").toLowerCase(); return terms.every(term => text.includes(term)); };
  const rows = items(plugins, category).filter(row => matches([row.name, pluginDisplayName(row.plugin), row.plugin.source, row.plugin.spec.origin?.location, row.plugin.spec.description, ...row.plugin.spec.skills.map(skill => skill.description)]));
  const catalogRows = catalog.filter(entry => !plugins.some(plugin => plugin.source === market && plugin.spec.name === entry.name)).filter(entry => matches([entry.name, entry.description, JSON.stringify(entry.source), market, markets.find(source => source.name === market)?.source]));
  const skillRows = publicSkills.filter(skill => !plugins.some(plugin => plugin.spec.origin
    ? plugin.spec.origin.location.replace(/\.git$/, "") === skillRepository.replace(/\.git$/, "") && plugin.spec.origin.subdirectory === `skills/${skill.id}`
    : plugin.spec.package_kind === "skill" && plugin.spec.skills.some(existing => existing.id === skill.id)))
    .filter(skill => matches([skill.id, skill.name, skill.description, "Anthropic", skillRepository]));
  const testedTools = testResult && typeof testResult === "object" && "tools" in testResult && Array.isArray(testResult.tools) ? testResult.tools as Array<{ name?: string }> : [];
  const text = item?.kind === "mcp" ? JSON.stringify(item.plugin.spec.mcp[item.id], null, 2) : item?.kind === "hook" ? JSON.stringify(item.plugin.spec.hooks.find(hook => hook.name === item.id), null, 2) : item ? markdown(item.plugin, file) : "";
  return <div className="themis-integrations">
    <h2>Integrations</h2>
    <nav className="themis-integrations-tabs" aria-label="Integration categories">{categories.map(value => <button key={value} aria-pressed={category === value} onClick={() => { setCategory(value); setPublicView(false); setMenu(null); }}>{value}</button>)}</nav>
    <div className="themis-integrations-toolbar"><div>{(category === "Plugins" || category === "Skills") && <><button aria-pressed={!publicView} onClick={() => setPublicView(false)}>Installed</button><button aria-pressed={publicView} onClick={() => { setPublicView(true); if (category === "Plugins" && markets[0]) void browse(markets[0].name); }}>Not installed</button></>}</div><div><button aria-label="Refresh" disabled={busy} onClick={() => void load().catch(error => setError(describeError(error)))}>↻</button><button aria-label={`Add ${category.toLowerCase()}`} onClick={() => open("import")}><Icon name="add" /></button></div></div>
    <input className="themis-integrations-search" type="search" aria-label="Search integrations" placeholder={`Search ${publicView ? `available ${category.toLowerCase()}` : category.toLowerCase()}`} value={search} onChange={event => setSearch(event.target.value)} />
    {error && !panel && <p role="alert">{error}</p>}
    {publicView && category === "Skills" ? <>
      {!skillRows.length && <EmptyState title={search.trim() ? "No matching integrations" : "All available skills installed"} />}
      <ul className="themis-integrations-list">{skillRows.map(skill => <li key={skill.id}><Icon name={skillGlyph(skill.id)} /><button className="themis-integration-name themis-integration-open" aria-label={`View ${skill.name}`} onClick={() => void previewSkill(skill)}>{skill.name}</button><span className="themis-integration-source">Anthropic</span><button disabled={busy} onClick={() => void action({ action: "import_repository", url: skillRepository, reference: skillReference, subdirectory: `skills/${skill.id}`, name: skill.id, scope }).catch(() => {})}>Install</button></li>)}</ul>
    </> : publicView ? <>
      <div className="themis-integrations-toolbar"><select aria-label="Marketplace" disabled={busy} value={market} onChange={event => void browse(event.target.value)}>{markets.map(m => <option key={m.name}>{m.name}</option>)}</select><button onClick={() => open("market")}>Add source</button></div>
      {!catalogRows.length && <EmptyState title={search.trim() ? "No matching integrations" : "No public plugins available"} />}
      <ul className="themis-integrations-list">{catalogRows.map(entry => <li key={entry.name}><PluginIcon plugin={{ ...emptySpec(), icon: entry.icon }} /><button className="themis-integration-name themis-integration-open" aria-label={`View ${entry.name}`} disabled={busy} onClick={() => void preview(entry.name, entry.icon)}>{entry.name}</button><button disabled={busy} onClick={() => void action({ action: "install", name: entry.name, marketplace: market, scope }).catch(() => {})}>Install</button></li>)}</ul>
    </> : <>
      {!rows.length && <EmptyState title={search.trim() ? "No matching integrations" : `No ${category.toLowerCase()} installed`} />}
      <ul className="themis-integrations-list">{rows.map(row => <li key={row.key}>
        <PluginIcon plugin={row.plugin.spec} fallback={row.kind === "plugin" ? "folder" : row.kind === "mcp" ? "plug" : row.kind === "hook" ? "hook" : skillGlyph(row.id)} /><button className="themis-integration-name themis-integration-open" aria-label={`View ${row.name}`} onClick={() => open("view", row)}>{row.name}</button><span className="themis-integration-source">{row.kind === "plugin" ? row.plugin.spec.origin?.kind === "discovered" ? "Discovered" : row.plugin.source ?? row.plugin.spec.origin?.kind ?? "Personal" : pluginDisplayName(row.plugin)} · {row.plugin.scope === "global" ? "User" : "Project"}</span>
        <button className="themis-integration-status" aria-label={`${row.enabled ? "Disable" : "Enable"} ${row.name}`} disabled={busy || row.kind !== "plugin" && !row.plugin.enabled && isPluginPackage(row.plugin)} onClick={() => void toggle(row).catch(() => {})}>{row.kind !== "plugin" && !row.plugin.enabled && isPluginPackage(row.plugin) ? "Plugin disabled" : row.enabled ? "Enabled" : "Disabled"}</button>
        <details className="themis-integration-more" open={menu === row.key}><summary role="button" aria-label={`More actions for ${row.name}`} onClick={event => { event.preventDefault(); setMenu(menu === row.key ? null : row.key); }}><Icon name="more" /></summary>{menu === row.key && <div className="themis-integration-menu"><button onClick={() => open("edit", row)}>Configure</button>{row.plugin.source && row.plugin.source !== "discovered" && <button disabled={busy} onClick={() => { setMenu(null); void action({ action: "update", name: row.plugin.spec.name, scope: row.plugin.scope, marketplace: row.plugin.source }).catch(() => {}); }}>Update</button>}<button onClick={() => open("uninstall", row)}>Uninstall</button></div>}</details>
      </li>)}</ul>
    </>}
    <Dialog open={panel !== null} title={panel?.mode === "view" ? item?.name : panel?.mode === "uninstall" ? `Uninstall ${item?.name}?` : panel?.mode === "edit" ? `Configure ${item?.name}` : panel?.mode === "market" ? "Add marketplace" : "Add integration"} onClose={() => { if (!busy || panel?.marketplace || panel?.skillSource) setPanel(null); }}>
      {panel?.mode === "view" && <div className="themis-skill-viewer">
        {(panel.marketplace || panel.skillSource) && busy ? <p role="status">Loading skills…</p> : <>
          {item?.kind === "plugin" && <><p className="themis-integration-skill-count">{item.plugin.spec.skills.length} {item.plugin.spec.skills.length === 1 ? "skill" : "skills"}</p><div className="themis-integration-files">{item.plugin.spec.skills.map(skill => <button key={skill.id} aria-pressed={file === skill.id} onClick={() => setFile(skill.id)}>{skill.name}</button>)}</div></>}
          {panel.skillSource && <a href={panel.skillSource} target="_blank" rel="noreferrer">Read SKILL.md</a>}
          {item?.kind === "mcp" || item?.kind === "hook" ? <details><summary>Advanced configuration</summary><CodeBlock code={text} language="json" /></details> : text ? <><div className="themis-integration-files" aria-label="Skill view"><button aria-pressed={readerMode === "source"} onClick={() => setReaderMode("source")}>Source</button><button aria-pressed={readerMode === "rendered"} onClick={() => setReaderMode("rendered")}>Rendered</button></div>{readerMode === "source" ? <CodeBlock code={text} language="markdown" /> : <ResponseBody text={text} />}</> : <p>No bundled skills.</p>}
          {!!item?.plugin.spec.unsupported.length && <details><summary>Compatibility notes</summary><ul>{item.plugin.spec.unsupported.map((warning, i) => <li key={i}>{warning}</li>)}</ul></details>}
        </>}
      </div>}
      {(panel?.mode === "edit" || panel?.mode === "create") && <>{panel?.mode === "create" && <label>Scope<select value={scope} onChange={event => setScope(event.target.value as "local" | "global")}><option value="global">User</option><option value="local" disabled={!root}>Project</option></select></label>}<details><summary>Advanced configuration</summary><label>Configuration<textarea aria-label="Configuration" readOnly={!!item?.plugin.source} value={draft} onChange={event => setDraft(event.target.value)} /></label>{item?.plugin.source && <Button variant="ghost" onClick={() => personalCopy(item.plugin)}>Create personal copy</Button>}<div className="themis-integrations-actions"><Button disabled={busy || !!item?.plugin.source} onClick={() => void saveDraft()}>Save</Button></div></details></>}
      {panel?.mode === "uninstall" && <><p>{item?.plugin.source === "discovered" ? `This skill is discovered from ${item.plugin.spec.origin?.location}. Disable it here, or remove it from its source directory.` : item?.kind === "plugin" ? "Remove this plugin and its bundled capabilities." : `Remove this ${item?.kind} from ${item?.plugin.spec.name}. Other capabilities remain installed.`}</p><div className="themis-integrations-actions"><Button variant="ghost" onClick={() => setPanel(null)}>Cancel</Button><Button variant="danger" disabled={busy || item?.plugin.source === "discovered"} onClick={() => { if (item) void action({ action: item.kind === "plugin" ? "delete" : "remove_component", kind: item.kind, id: item.id, scope: item.plugin.scope, name: item.plugin.spec.name }).then(() => setPanel(null)).catch(() => {}); }}>Uninstall</Button></div></>}
      {panel?.mode === "edit" && item && (item.kind === "mcp" || item.kind === "hook") && <><Button disabled={busy || draft !== JSON.stringify(item.plugin.spec, null, 2)} title={draft !== JSON.stringify(item.plugin.spec, null, 2) ? "Save changes before testing" : undefined} onClick={() => void action(item.kind === "mcp" ? { action: "test_mcp", server: item.plugin.spec.mcp[item.id] } : { action: "test_hook", hook: item.plugin.spec.hooks.find(hook => hook.name === item.id) }).then(setTestResult).catch(() => {})}>Run test</Button>{testResult !== null && <><p role="status">{item.kind === "mcp" ? `Connected · ${testedTools.length} tools` : "Hook completed"}</p>{item.kind === "mcp" && <p className="themis-integration-tool-names">{testedTools.map(tool => tool.name).filter(Boolean).join(" · ")}</p>}<details><summary>Advanced result</summary><pre className="themis-integration-markdown">{JSON.stringify(testResult, null, 2)}</pre></details></>}</>}
      {panel?.mode === "import" && <div className="themis-integrations-form"><label>Import from<select value={sourceType} onChange={event => setSourceType(event.target.value)}><option value="paste">Advanced: paste JSON</option><option value="path">File or folder</option><option value="repository">Repository URL</option></select></label><label>Scope<select value={scope} onChange={event => setScope(event.target.value as "local" | "global")}><option value="global">User</option><option value="local" disabled={!root}>Project</option></select></label>{sourceType === "paste" ? <details><summary>Advanced JSON</summary><label>JSON<textarea value={source} onChange={event => setSource(event.target.value)} /></label></details> : <label>Source<input value={source} onChange={event => setSource(event.target.value)} /></label>}<details><summary>Advanced</summary><Button variant="ghost" onClick={() => open("create")}>Create with JSON</Button><label>Name<input value={name} onChange={event => setName(event.target.value)} placeholder="Optional" /></label>{sourceType === "repository" && <><label>Git reference<input value={reference} onChange={event => setReference(event.target.value)} placeholder="Branch, tag, or commit" /></label><label>Subdirectory<input value={subdirectory} onChange={event => setSubdirectory(event.target.value)} placeholder="skills/my-skill" /></label></>}</details><Button disabled={busy || !source.trim()} onClick={() => void importSource().catch(() => {})}>Import</Button></div>}
      {panel?.mode === "market" && <div className="themis-integrations-form"><label>Name<input value={name} onChange={event => setName(event.target.value)} /></label><label>Repository or folder<input value={source} onChange={event => setSource(event.target.value)} /></label><Button disabled={busy || !name || !source} onClick={() => void action({ action: "add_marketplace", name, source }).then(() => setPanel(null)).catch(() => {})}>Add</Button></div>}
      {error && <p role="alert">{error}</p>}<Button variant="ghost" disabled={busy && !panel?.marketplace && !panel?.skillSource} onClick={() => setPanel(null)}>Close</Button>
    </Dialog>
    {category === "Hooks" && <p className="themis-integration-footnote">Events: {hookEvents.join(" · ")}</p>}
  </div>;
}
