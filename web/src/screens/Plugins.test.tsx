// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { toast } from "../state/store";
import { Plugins } from "./Plugins";
import { initialState } from "../state/reducer";
import type { ImportPreview, Plugin } from "../lib/types";
import { pluginAction } from "../lib/tauri";

const dispatch = vi.fn();
let installed: Plugin[];
let projectRoot: string | null;
let mcpConnections = initialState.mcpConnections;
vi.mock("../state/store", async original => ({ ...await original<typeof import("../state/store")>(), toast: vi.fn(), useApp: () => ({ state: { ...initialState, mcpConnections, activeProjectRoot: projectRoot }, dispatch }) }));
vi.mock("../lib/tauri", () => ({ pluginAction: vi.fn() }));
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
beforeEach(() => {
  vi.clearAllMocks();
  projectRoot = "/project";
  mcpConnections = {};
  installed = [{ scope: "global", revision: "v1", enabled: true, source: null, spec: { name: "docs", description: "Documents", version: "1", skills: [{ id: "draft", name: "Draft", description: "Draft a document", instructions: "Read the sources before drafting.", allowedTools: [], scripts: [] }], mcp: { filesystem: { command: "node", args: [], env: { SECRET: "hidden-secret" }, enabled: true } }, hooks: [{ name: "startup-check", event: "RunStart", command: "printf '{}'", enabled: false, blocking: false, timeout_seconds: 10 }], files: {}, unsupported: [] } }];
  vi.mocked(pluginAction).mockImplementation(async args => {
    if (args.action === "mcp_tools") return [{ name: "lookup", description: "Find a document" }] as never;
    if (args.action === "list") return structuredClone(installed) as never;
    if (args.action === "marketplaces") return [{ name: "official", source: "official-source" }] as never;
    if (args.action === "scan_marketplace") {
      const catalog = await pluginAction<{ plugins: Array<{ name: string }> }>({ action: "catalog" });
      const checks: Record<string, ImportPreview> = {};
      for (const entry of catalog.plugins) checks[entry.name] = await pluginAction<ImportPreview>({ action: "inspect_marketplace", name: entry.name });
      return { revision: "scan-1", status: "ready", catalog, checks, errors: {}, completed: catalog.plugins.length, total: catalog.plugins.length } as never;
    }
    if (args.action === "catalog") return { plugins: [{ name: "public-docs", description: "Review documents", icon: "https://example.com/icon.svg" }] } as never;
    if (args.action === "inspect_marketplace") return { spec: { ...installed[0].spec, name: String(args.name) }, report: { status: "supported", components: [] } } as never;
    if (args.action === "install") { installed.push({ ...installed[0], source: String(args.marketplace), spec: { ...installed[0].spec, name: String(args.name) } }); return null as never; }
    const plugin = installed.find(plugin => plugin.spec.name === args.name);
    if (args.action === "delete") { installed = installed.filter(existing => existing !== plugin); return null as never; }
    if (plugin && (args.action === "enable" || args.action === "disable")) { plugin.enabled = args.action === "enable"; return null as never; }
    if (plugin && args.action === "set_component_enabled") {
      if (args.kind === "skill") plugin.spec.disabled_skills = args.enabled ? [] : [String(args.id)];
      if (args.kind === "mcp") plugin.spec.mcp[String(args.id)].enabled = Boolean(args.enabled);
      if (args.kind === "hook") plugin.spec.hooks.find(hook => hook.name === args.id)!.enabled = Boolean(args.enabled);
      return null as never;
    }
    if (plugin && args.action === "remove_component") { plugin.spec.skills = plugin.spec.skills.filter(skill => skill.id !== args.id); return null as never; }
    throw new Error(`Unexpected management action: ${args.action}`);
  });
});
function expectNoConfiguration() {
  expect(screen.queryByRole("button", { name: /^(Add (plugin|skill|MCP|hook)|Import|Configure|Update|Test|Run test|Save|More actions|Ask Themis)( |$)/ })).toBeNull();
  expect(screen.queryByRole("textbox", { name: "Configuration" })).toBeNull();
  expect(vi.mocked(pluginAction).mock.calls.every(([args]) => ["mcp_tools", "list", "marketplaces", "scan_marketplace", "catalog", "inspect_marketplace", "inspect_repository", "enable", "disable", "set_component_enabled", "install", "import_repository", "delete", "remove_component"].includes(String(args.action)))).toBe(true);
}
describe("Integration browsing and lifecycle", () => {
  it("shows actual MCP connection state separately from its activation switch", async () => {
    mcpConnections = { "global:docs:mcp:filesystem": { status: "failed", reason: "Process could not start" } };
    const view = render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.click(screen.getByRole("button", { name: "MCP" }));
    expect(screen.getByRole("switch")).toHaveAttribute("aria-checked", "true");
    expect(screen.getByText("Failed")).toBeVisible();
    expect(screen.getByText("Process could not start").closest("details")).not.toHaveAttribute("open");
    mcpConnections = { "global:docs:mcp:filesystem": { status: "connected" } };
    view.rerender(<Plugins />);
    expect(screen.getByText("Connected")).toBeVisible();
    expect(screen.queryByText("Failed")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "View filesystem" }));
    expect(await screen.findByText("lookup")).toBeVisible();
    expect(screen.getByRole("region", { name: "MCP tools" })).toHaveTextContent("Find a document");
    expect(pluginAction).toHaveBeenCalledWith({ action: "mcp_tools", projectRoot: "/project", scope: "global", name: "docs", id: "filesystem" });
  });
  it("keeps Installed and Discover search and filters independent", async () => {
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "docs" } });
    fireEvent.change(screen.getByRole("combobox", { name: "Source" }), { target: { value: "personal" } });
    fireEvent.change(screen.getByRole("combobox", { name: "Status" }), { target: { value: "enabled" } });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Discover" })));
    expect(screen.getByRole("searchbox")).toHaveValue("");
    expect(screen.getByRole("combobox", { name: "Source" })).toHaveValue("all");
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "missing" } });
    fireEvent.click(screen.getByRole("button", { name: "Installed" }));
    expect(screen.getByRole("searchbox")).toHaveValue("docs");
    expect(screen.getByRole("combobox", { name: "Source" })).toHaveValue("personal");
    expect(screen.getByRole("combobox", { name: "Status" })).toHaveValue("enabled");
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Discover" })));
    expect(screen.getByRole("searchbox")).toHaveValue("missing");
    fireEvent.click(screen.getByRole("button", { name: "Reset filters" }));
    expect(screen.getByRole("button", { name: "View public-docs" })).toBeVisible();
  });
  it("shows rejected local skills with their reason and no activation control", async () => {
    installed.push({ ...installed[0], source: "discovered", enabled: false, spec: { ...installed[0].spec, name: "discovered-invalid", package_kind: "skill", origin: { kind: "discovered", location: "/skills/invalid" }, skills: [], mcp: {}, hooks: [], unsupported: ["Missing description"] } });
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.click(screen.getByRole("button", { name: "Skills" }));
    const row = screen.getByRole("button", { name: "View invalid" }).closest("li")!;
    const warning = row.querySelector("details")!;
    expect(warning).not.toBeNull();
    expect(warning).not.toHaveAttribute("open");
    fireEvent.click(within(warning).getByText(/Compatibility details/));
    expect(within(warning).getByText(/Missing description/)).toBeVisible();
    expect(row).toHaveTextContent("Unavailable");
    expect(within(row).queryByRole("switch")).toBeNull();
  });
  it("shows actual scan percentage with an interactive companion", async () => {
    vi.useFakeTimers();
    try {
      const previous = vi.mocked(pluginAction).getMockImplementation()!;
      vi.mocked(pluginAction).mockImplementation(async args => args.action === "scan_marketplace" ? { revision: "scan", status: "scanning", completed: 3, total: 12, catalog: { plugins: [] }, checks: {}, errors: {} } as never : previous(args) as never);
      await act(async () => { render(<Plugins />); });
      await act(async () => fireEvent.click(screen.getByRole("button", { name: "Discover" })));
      expect(screen.getByRole("progressbar", { name: "Marketplace compatibility" })).toHaveAttribute("aria-valuenow", "25");
      expect(screen.getByText("25%")).toBeVisible();
      fireEvent.click(screen.getByRole("button", { name: /Wake/ }));
      fireEvent.click(screen.getByRole("button", { name: "Installed" }));
      await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
      expect(screen.queryByRole("progressbar")).toBeNull();
    } finally { vi.useRealTimers(); }
  });
  it("explains partial support before allowing installation and hides unsupported packages", async () => {
    const previous = vi.mocked(pluginAction).getMockImplementation()!;
    vi.mocked(pluginAction).mockImplementation(async args => {
      if (args.action === "catalog") return { plugins: [{ name: "mixed", description: "Useful review tools" }, { name: "legacy", description: "Legacy tools" }] } as never;
      if (args.action === "inspect_marketplace") return { spec: { ...installed[0].spec, name: args.name, skills: args.name === "legacy" ? [] : installed[0].spec.skills, mcp: {}, hooks: [], unsupported: ["lspServers"] }, report: { status: args.name === "legacy" ? "unsupported" : "partial", components: [{ kind: "lsp", name: "server", status: "unsupported", reason: "LSP is not supported", remedy: "Use the bundled skill instead" }] } } as never;
      return previous(args) as never;
    });
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Discover" })));
    expect(screen.getByText("Useful review tools")).toBeVisible();
    expect(screen.getAllByRole("button", { name: "Install" })).toHaveLength(1);
    await act(async () => fireEvent.click(within(screen.getByRole("button", { name: "View mixed" }).closest("li")!).getByRole("button", { name: "Install" })));
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("Partially supported");
    expect(dialog).toHaveTextContent("Language servers aren’t supported yet.");
    expect(within(dialog).getByRole("button", { name: "Install supported parts" })).toBeEnabled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    fireEvent.change(screen.getByRole("combobox", { name: "Status" }), { target: { value: "unsupported" } });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "View legacy" })));
    expect(screen.getByRole("dialog")).toHaveTextContent("No skills");
    expect(screen.getByRole("dialog")).not.toHaveTextContent(/No supported capabilities|Not included|lspServers/);
    expect(within(screen.getByRole("dialog")).queryByRole("button", { name: /^Install/ })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    fireEvent.change(screen.getByRole("combobox", { name: "Status" }), { target: { value: "available" } });
    expect(screen.queryByRole("button", { name: "View legacy" })).toBeNull();
    fireEvent.change(screen.getByRole("combobox", { name: "Status" }), { target: { value: "unsupported" } });
    expect(screen.getByRole("button", { name: "View legacy" })).toBeVisible();
    const reason = within(screen.getByRole("button", { name: "View legacy" }).closest("li")!).getByText("Language servers aren’t supported yet.");
    const warning = reason.closest("details")!;
    expect(warning).not.toHaveAttribute("open");
    fireEvent.click(within(warning).getByText(/Compatibility details/));
    expect(warning).toHaveAttribute("open");
    expect(screen.queryByRole("button", { name: "View mixed" })).toBeNull();
    fireEvent.change(screen.getByRole("combobox", { name: "Status" }), { target: { value: "partial" } });
    expect(screen.getByRole("button", { name: "View mixed" })).toBeVisible();
    expect(screen.queryByRole("button", { name: "View legacy" })).toBeNull();
    fireEvent.change(screen.getByRole("combobox", { name: "Status" }), { target: { value: "unchecked" } });
    expect(screen.queryByRole("button", { name: "View mixed" })).toBeNull();
  });
  it("loads saved checks before opening and keeps the skills view quiet", async () => {
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Discover" })));
    const tile = screen.getByRole("button", { name: "View public-docs" }).closest("li")!;
    expect(tile).toHaveTextContent("Compatible");
    expect(screen.queryByRole("dialog")).toBeNull();
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "View public-docs" })));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByRole("button", { name: "View Draft" })).toBeVisible();
    expect(dialog).not.toHaveTextContent(/Compatible|Documents|MCP|hook|Install|Available in/);
    expect(vi.mocked(pluginAction).mock.calls.filter(([args]) => args.action === "inspect_marketplace")).toHaveLength(2);
  });
  it("keeps incomplete catalogs unavailable and ignores a scan after switching marketplaces", async () => {
    const previous = vi.mocked(pluginAction).getMockImplementation()!;
    let finish!: (value: unknown) => void;
    vi.mocked(pluginAction).mockImplementation(async args => {
      if (args.action === "marketplaces") return [{ name: "slow", source: "/slow" }, { name: "ready", source: "/ready" }] as never;
      if (args.action === "scan_marketplace" && args.name === "slow") return await new Promise<unknown>(resolve => { finish = resolve; }) as never;
      return previous(args) as never;
    });
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.click(screen.getByRole("button", { name: "Discover" }));
    expect(screen.getByRole("status")).toHaveTextContent("Checking marketplace");
    expect(screen.queryByRole("button", { name: "Install" })).toBeNull();
    expect(screen.getByRole("combobox", { name: "Marketplace" })).toBeEnabled();
    await act(async () => fireEvent.change(screen.getByRole("combobox", { name: "Marketplace" }), { target: { value: "ready" } }));
    expect(screen.getByRole("button", { name: "View public-docs" })).toBeVisible();
    await act(async () => finish({ status: "ready", revision: "old", catalog: { plugins: [{ name: "stale" }] }, checks: {}, errors: {} }));
    expect(screen.queryByRole("button", { name: "View stale" })).toBeNull();
    expect(screen.getByRole("button", { name: "View public-docs" })).toBeVisible();
  });
  it("does not reopen a closed preview when refresh overtakes its response", async () => {
    const previous = vi.mocked(pluginAction).getMockImplementation()!;
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Discover" })));
    let finish!: (value: unknown) => void;
    vi.mocked(pluginAction).mockImplementation(async args => args.action === "inspect_marketplace" ? await new Promise<unknown>(resolve => { finish = resolve; }) as never : previous(args) as never);
    fireEvent.click(screen.getByRole("button", { name: "View public-docs" }));
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    // A new scan uses its saved tile results and must not await the old preview.
    vi.mocked(pluginAction).mockImplementation(previous);
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Refresh" })));
    await act(async () => finish({ spec: { ...installed[0].spec, name: "stale-preview" }, report: { status: "supported", components: [] } }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(pluginAction).toHaveBeenCalledWith({ action: "scan_marketplace", name: "official", refresh: true });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Install" })));
    expect(pluginAction).toHaveBeenCalledWith(expect.objectContaining({ action: "install", expectedScan: "scan-1" }));
  });
  it("filters saved failures by their label and prevents installation", async () => {
    const previous = vi.mocked(pluginAction).getMockImplementation()!;
    vi.mocked(pluginAction).mockImplementation(async args => args.action === "scan_marketplace" ? { revision: "failed-entry", status: "ready", catalog: { plugins: [{ name: "missing" }] }, checks: {}, errors: { missing: "No such file or directory (os error 2)" } } as never : previous(args) as never);
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Discover" })));
    fireEvent.change(screen.getByRole("combobox", { name: "Status" }), { target: { value: "failed" } });
    expect(screen.getByRole("button", { name: "View missing" })).toBeVisible();
    fireEvent.click(screen.getByText(/Compatibility details/));
    expect(screen.getByText("Plugin files are missing. Refresh to retry.")).toBeVisible();
    expect(screen.getByRole("button", { name: "Install" })).toBeDisabled();
    fireEvent.change(screen.getByRole("combobox", { name: "Status" }), { target: { value: "supported" } });
    expect(screen.queryByRole("button", { name: "View missing" })).toBeNull();
  });
  it("filters installed items by their activation label", async () => {
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.change(screen.getByRole("combobox", { name: "Status" }), { target: { value: "disabled" } });
    expect(screen.queryByRole("button", { name: "View docs" })).toBeNull();
    fireEvent.change(screen.getByRole("combobox", { name: "Status" }), { target: { value: "enabled" } });
    expect(screen.getByRole("button", { name: "View docs" })).toBeVisible();
  });
  it("filters personal and public capabilities by recorded origin", async () => {
    installed.push({ ...installed[0], spec: { ...installed[0].spec, name: "public-tools", origin: { kind: "repository", location: "https://example.com/tools.git" } } });
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.change(screen.getByRole("combobox", { name: "Source" }), { target: { value: "personal" } });
    expect(screen.getByRole("button", { name: "View docs" })).toBeVisible();
    expect(screen.queryByRole("button", { name: "View public-tools" })).toBeNull();
    fireEvent.change(screen.getByRole("combobox", { name: "Source" }), { target: { value: "public" } });
    expect(screen.getByRole("button", { name: "View public-tools" })).toBeVisible();
    expect(screen.queryByRole("button", { name: "View docs" })).toBeNull();
  });
  it("shows tile progress, prevents duplicate installation and leaves a completion state", async () => {
    const previous = vi.mocked(pluginAction).getMockImplementation()!;
    let finish!: () => void;
    const pending = new Promise<void>(resolve => { finish = resolve; });
    vi.mocked(pluginAction).mockImplementation(async args => {
      if (args.action === "install") await pending;
      return previous(args) as never;
    });
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Discover" })));
    fireEvent.click(screen.getByRole("button", { name: "Install" }));
    expect(await screen.findByRole("button", { name: "Installing…" })).toBeDisabled();
    expect(screen.queryByRole("dialog")).toBeNull();
    await act(async () => finish());
    expect(screen.getByRole("button", { name: "✓ Installed" })).toBeDisabled();
    expect(toast).toHaveBeenCalledExactlyOnceWith(dispatch, "public-docs installed.", "success");
    expect(screen.queryByText("public-docs installed.")).toBeNull();
    expect(vi.mocked(pluginAction).mock.calls.filter(([args]) => args.action === "install")).toHaveLength(1);
  });
  it("shows complete captured Markdown after selecting a bundled skill", async () => {
    const source = `---\nname: draft\nlicense: Original-License\n---\n# Full skill\n\n${"Complete source text. ".repeat(100)}\n\n## Final section\nUNTRUNCATED_TAIL`.replace(/\n/g, "\r\n");
    installed[0].spec.files = { "custom/notes/SKILL.md": source };
    installed[0].spec.skill_paths = { draft: "custom/notes/SKILL.md" };
    render(<Plugins />);
    fireEvent.click(await screen.findByRole("button", { name: "View docs" }));
    const dialog = screen.getByRole("dialog", { name: "docs" });
    expect(dialog).not.toHaveTextContent(/1 skill|Documents|MCP connection|Connections and hooks|startup-check/);
    expect(dialog).not.toHaveTextContent("UNTRUNCATED_TAIL");
    const view = within(dialog).getByRole("button", { name: "View Draft" });
    expect(view.querySelector("svg")).toBeInTheDocument();
    fireEvent.click(view);
    expect(within(dialog).getByRole("heading", { name: "Full skill" })).toBeInTheDocument();
    expect(within(dialog).getByRole("heading", { name: "Final section" })).toBeInTheDocument();
    expect(dialog).toHaveTextContent("UNTRUNCATED_TAIL");
    expect(dialog).not.toHaveTextContent("Original-License");
    expect(within(dialog).queryByRole("button", { name: /Source|Rendered/ })).toBeNull();
    expectNoConfiguration();
  });
  it("lists every bundled skill and selects its original nested source", async () => {
    installed[0].spec.skills.push({ ...installed[0].spec.skills[0], id: "review", name: "Review" });
    installed[0].spec.files = { "skills/draft/SKILL.md": "# Draft source", "skills/review/SKILL.md": "# Review source\nFULL_REVIEW_TAIL" };
    render(<Plugins />);
    fireEvent.click(await screen.findByRole("button", { name: "View docs" }));
    const dialog = screen.getByRole("dialog", { name: "docs" });
    expect(within(dialog).getAllByRole("button", { name: /^View / })).toHaveLength(2);
    expect(within(dialog).getByRole("button", { name: "View Draft" })).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "View Review" }));
    expect(dialog).toHaveTextContent("FULL_REVIEW_TAIL");
    expect(dialog).not.toHaveTextContent("Draft source");
  });
  it("replaces a long public bundle list with its selected body and returns to skills", async () => {
    installed[0].spec.skills = Array.from({ length: 23 }, (_, index) => ({ ...installed[0].spec.skills[0], id: `skill-${index}`, name: `Skill ${index}`, instructions: `# Body ${index}\nComplete instructions` }));
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Discover" })));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "View public-docs" })));
    const dialog = screen.getByRole("dialog");
    const listViewer = dialog.querySelector(".themis-skill-viewer")!;
    listViewer.scrollTop = 600;
    fireEvent.click(within(dialog).getByRole("button", { name: "View Skill 22" }));
    expect(within(dialog).getByRole("heading", { name: "Body 22" })).toBeInTheDocument();
    expect(dialog.querySelector(".themis-bundled-skills")).toBeNull();
    expect(dialog.querySelector(".themis-skill-viewer")).not.toBe(listViewer);
    expect(dialog.querySelector(".themis-skill-viewer")!.scrollTop).toBe(0);
    fireEvent.click(within(dialog).getByRole("button", { name: "Back to skills" }));
    expect(within(dialog).queryByRole("heading", { name: "Body 22" })).toBeNull();
    expect(within(dialog).getAllByRole("button", { name: /^View Skill/ })).toHaveLength(23);
    expect(dialog.querySelector(".themis-skill-viewer")!.scrollTop).toBe(0);
    expect(pluginAction).not.toHaveBeenCalledWith(expect.objectContaining({ action: "install" }));
  });
  it.each(["https://example.com/marketplace.json", "/local/marketplace.json"])("adds marketplace source %s and browses without installing", async source => {
    const previous = vi.mocked(pluginAction).getMockImplementation()!;
    vi.mocked(pluginAction).mockImplementation(async args => args.action === "add_marketplace" ? null as never : previous(args) as never);
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Discover" })));
    const add = screen.getByRole("button", { name: "Add marketplace" });
    expect(add.querySelector("svg")).toBeInTheDocument();
    expect(add).toHaveAttribute("title", "Add marketplace");
    fireEvent.click(add);
    const dialog = screen.getByRole("dialog", { name: "Add marketplace" });
    fireEvent.change(within(dialog).getByRole("textbox", { name: "Name" }), { target: { value: "personal" } });
    fireEvent.change(within(dialog).getByRole("textbox", { name: "URL or local path" }), { target: { value: source } });
    await act(async () => fireEvent.click(within(dialog).getByRole("button", { name: "Add" })));
    expect(pluginAction).toHaveBeenCalledWith(expect.objectContaining({ action: "add_marketplace", name: "personal", source }));
    expect(pluginAction).toHaveBeenCalledWith({ action: "scan_marketplace", name: "personal", refresh: false });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(pluginAction).not.toHaveBeenCalledWith(expect.objectContaining({ action: "install" }));
  });
  it("keeps marketplace errors actionable and allows correction without installing", async () => {
    const previous = vi.mocked(pluginAction).getMockImplementation()!;
    vi.mocked(pluginAction).mockImplementation(async args => {
      if (args.action === "add_marketplace") throw new Error("Marketplace source could not be read\nDiagnostic operation: 12345678-1234-1234-1234-123456789abc");
      return previous(args) as never;
    });
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Discover" })));
    fireEvent.click(screen.getByRole("button", { name: "Add marketplace" }));
    const dialog = screen.getByRole("dialog", { name: "Add marketplace" });
    expect(within(dialog).getByRole("button", { name: "Add" })).toBeDisabled();
    fireEvent.change(within(dialog).getByRole("textbox", { name: "Name" }), { target: { value: "personal" } });
    fireEvent.change(within(dialog).getByRole("textbox", { name: "URL or local path" }), { target: { value: "/missing" } });
    await act(async () => fireEvent.click(within(dialog).getByRole("button", { name: "Add" })));
    expect(toast).toHaveBeenCalledWith(expect.any(Function), "Marketplace source could not be read", "danger");
    expect(within(dialog).queryByRole("alert")).toBeNull();
    expect(within(dialog).getByRole("textbox", { name: "URL or local path" })).toHaveValue("/missing");
    expect(within(dialog).getByRole("button", { name: "Add" })).toBeEnabled();
    expect(pluginAction).not.toHaveBeenCalledWith(expect.objectContaining({ action: "install" }));
  });
  it("reports empty packages without inventing skills", async () => {
    installed[0].spec.skills = [];
    render(<Plugins />);
    fireEvent.click(await screen.findByRole("button", { name: "View docs" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("No skills");
    expect(screen.getByRole("dialog")).not.toHaveTextContent(/MCP connection|hook|Documents/);
  });
  it("keeps standalone wrappers in Skills and shows disabled status without actions", async () => {
    installed.push({ ...installed[0], enabled: false, source: "discovered", spec: { ...installed[0].spec, name: "discovered-1234", skills: [{ ...installed[0].spec.skills[0], name: "Discovered draft" }] } });
    for (const kind of ["skill", "mcp", "hook"] as const) installed.push({ ...installed[0], spec: { ...installed[0].spec, name: `standalone-${kind}`, package_kind: kind } });
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    expect(screen.queryByRole("button", { name: /View standalone/ })).toBeNull();
    expect(screen.queryByText("discovered-1234")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Skills" }));
    const row = screen.getByRole("button", { name: "View Discovered draft" }).closest("li")!;
    expect(row).toHaveTextContent("Disabled");
    expect(row).not.toHaveTextContent("discovered-1234");
    expectNoConfiguration();
  });
  it("searches descriptions with recoverable empty results", async () => {
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "document" } });
    expect(screen.getByRole("button", { name: "View docs" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Skills" }));
    expect(screen.getByRole("button", { name: "View Draft" })).toBeInTheDocument();
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "missing" } });
    expect(screen.getByText("No matching integrations")).toBeInTheDocument();
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "View Draft" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("Read the sources before drafting.");
    expectNoConfiguration();
  });
  it("shows useful MCP and hook summaries without configuration or execution", async () => {
    installed[0].spec.mcp.filesystem.args = ["-y", "@example/server", "/a path/it's here", ""];
    installed[0].spec.mcp.remote = { url: "https://example.com/mcp", args: [], env: {}, enabled: false };
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.click(screen.getByRole("button", { name: "MCP" }));
    fireEvent.click(screen.getByRole("button", { name: "View filesystem" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("Local process");
    expect(screen.getByRole("dialog")).toHaveTextContent(`node -y @example/server '/a path/it'"'"'s here' ''`);
    expect(screen.getByRole("dialog")).not.toHaveTextContent("hidden-secret");
    expectNoConfiguration();
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    fireEvent.click(screen.getByRole("button", { name: "View remote" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("HTTP");
    expect(screen.getByRole("dialog")).toHaveTextContent("https://example.com/mcp");
    expect(screen.getByRole("dialog")).toHaveTextContent("Disabled");
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    fireEvent.click(screen.getByRole("button", { name: "Hooks" }));
    fireEvent.click(screen.getByRole("button", { name: "View startup-check" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("RunStart");
    expect(screen.getByRole("dialog")).toHaveTextContent("Disabled");
    expectNoConfiguration();
  });
  it("uses source icons and replaces failed images with a glyph", async () => {
    installed[0].spec.icon = "https://example.com/missing.svg";
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.error(screen.getByAltText(""));
    expect(screen.queryByAltText("")).toBeNull();
    expect(screen.getByRole("button", { name: "View docs" }).closest("li")!.querySelector("svg")).toBeInTheDocument();
  });
  it("previews public bundles without installing and retains source icons", async () => {
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Discover" })));
    expect(screen.getByAltText("")).toHaveAttribute("src", "https://example.com/icon.svg");
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "View public-docs" })));
    const dialog = screen.getByRole("dialog", { name: "public-docs" });
    fireEvent.click(within(dialog).getByRole("button", { name: "View Draft" }));
    expect(dialog).toHaveTextContent("Read the sources before drafting.");
    expectNoConfiguration();
  });
  it("marks disabled installed packages as installed in the catalog", async () => {
    installed[0].source = "official";
    installed[0].spec.name = "public-docs";
    installed[0].enabled = false;
    render(<Plugins />);
    await screen.findByRole("button", { name: "View public-docs" });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Discover" })));
    expect(within(screen.getByRole("button", { name: "View public-docs" }).closest("li")!).getByRole("button", { name: "✓ Installed" })).toBeDisabled();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(pluginAction).toHaveBeenCalledWith(expect.objectContaining({action:"scan_marketplace"}));

  });
  it("keeps installed skills and chat refresh without a separate discovery catalog", async () => {
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.click(screen.getByRole("button", { name: "Skills" }));
    expect(screen.queryByRole("button", { name: "Discover" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "View Draft" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("Read the sources before drafting.");
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    installed.push({ ...installed[0], spec: { ...installed[0].spec, name: "standalone", package_kind: "skill", skills: [{ ...installed[0].spec.skills[0], name: "Local review" }] } });
    await act(async () => window.dispatchEvent(new Event("themis-plugins-changed")));
    expect(screen.getByRole("button", { name: "View Local review" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Plugins" }));
    expect(screen.getByRole("button", { name: "Discover" })).toBeInTheDocument();
  });
  it.each(["Plugins", "Skills", "MCP", "Hooks"])("toggles %s with a switch and visible status", async category => {
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.click(screen.getByRole("button", { name: category }));
    const control = screen.getByRole("switch");
    const wasEnabled = control.getAttribute("aria-checked") === "true";
    expect(control.closest("li")).toHaveTextContent(wasEnabled ? category === "MCP" ? "Connecting…" : "Enabled" : "Disabled");
    await act(async () => fireEvent.click(control));
    expect(screen.getByRole("switch")).toHaveAttribute("aria-checked", String(!wasEnabled));
    expect(screen.getByRole("switch").closest("li")).toHaveTextContent(wasEnabled ? "Disabled" : category === "MCP" ? "Connecting…" : "Enabled");
    expectNoConfiguration();
  });
  it("keeps disabled bundle children gated but reenables disabled discovered standalone skills", async () => {
    installed[0].enabled = false;
    installed.push({ ...installed[0], source: "discovered", spec: { ...installed[0].spec, name: "discovered-draft", package_kind: "skill", skills: [{ ...installed[0].spec.skills[0], name: "Standalone" }], disabled_skills: ["draft"] } });
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.click(screen.getByRole("button", { name: "Skills" }));
    expect(screen.getByRole("switch", { name: "Draft enabled" })).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Uninstall Standalone" })).toBeNull();
    await act(async () => fireEvent.click(screen.getByRole("switch", { name: "Standalone enabled" })));
    expect(screen.getByRole("switch", { name: "Standalone enabled" })).toHaveAttribute("aria-checked", "true");
    expect(pluginAction).toHaveBeenCalledWith(expect.objectContaining({ action: "enable", name: "discovered-draft" }));
    expect(pluginAction).toHaveBeenCalledWith(expect.objectContaining({ action: "set_component_enabled", name: "discovered-draft", id: "draft", enabled: true }));
  });
  it("confirms component uninstall and preserves its siblings", async () => {
    installed[0].spec.skills.push({ ...installed[0].spec.skills[0], id: "review", name: "Review" });
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.click(screen.getByRole("button", { name: "Skills" }));
    fireEvent.click(screen.getByRole("button", { name: "Uninstall Draft" }));
    expect(pluginAction).not.toHaveBeenCalledWith(expect.objectContaining({ action: "remove_component" }));
    const dialog = screen.getByRole("dialog", { name: "Uninstall Draft?" });
    await act(async () => fireEvent.click(within(dialog).getByRole("button", { name: "Uninstall" })));
    expect(screen.queryByRole("button", { name: "View Draft" })).toBeNull();
    expect(screen.getByRole("button", { name: "View Review" })).toBeInTheDocument();
    expect(pluginAction).toHaveBeenCalledWith(expect.objectContaining({ action: "remove_component", name: "docs", id: "draft", kind: "skill" }));
  });
  it("cancels package uninstall before deleting only after confirmation", async () => {
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.click(screen.getByRole("button", { name: "Uninstall docs" }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(pluginAction).not.toHaveBeenCalledWith(expect.objectContaining({ action: "delete" }));
    fireEvent.click(screen.getByRole("button", { name: "Uninstall docs" }));
    await act(async () => fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Uninstall" })));
    expect(screen.getByText("No plugins installed")).toBeInTheDocument();
    expect(pluginAction).toHaveBeenCalledWith(expect.objectContaining({ action: "delete", name: "docs", scope: "global" }));
  });
  it("installs packages globally from their tiles", async () => {
    const scope = "global";
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Discover" })));
    expect(screen.queryByRole("combobox", { name: "Installation scope" })).toBeNull();
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Install" })));
    expect(pluginAction).toHaveBeenCalledWith(expect.objectContaining({ action: "install", name: "public-docs", marketplace: "official", scope, allowPartial: false }));
    expect(within(screen.getByRole("button", { name: "View public-docs" }).closest("li")!).getByRole("button", { name: "✓ Installed" })).toBeDisabled();
    expectNoConfiguration();
  });

});
