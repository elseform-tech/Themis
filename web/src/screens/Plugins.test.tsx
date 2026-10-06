// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Plugins } from "./Plugins";
import { readSession, writeSession } from "../state/session";
import { initialState } from "../state/reducer";
import type { Plugin } from "../lib/types";
import { pluginAction } from "../lib/tauri";

vi.mock("../state/store", async original => ({ ...await original<typeof import("../state/store")>(), useApp: () => ({ state: { ...initialState, activeProjectRoot: "/project", activeThreadId }, dispatch: vi.fn() }) }));
vi.mock("../lib/tauri", () => ({ pluginAction: vi.fn() }));
let installed: Plugin[];
let activeThreadId: string | null;
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
beforeEach(() => {
  vi.clearAllMocks();
  activeThreadId = "test-chat";
  const storage = new Map<string, string>();
  vi.stubGlobal("localStorage", { getItem: (key: string) => storage.get(key) ?? null, setItem: (key: string, value: string) => storage.set(key, value) });
  installed = [{ scope: "global", revision: "v1", enabled: true, source: null, spec: { name: "docs", description: "Documents", version: "1", skills: [{ id: "draft", name: "Draft", description: "Draft a document", instructions: "Read the sources before drafting.", allowedTools: [], scripts: [] }], mcp: {}, hooks: [], files: {}, unsupported: [] } }];
  vi.mocked(pluginAction).mockImplementation(async args => {
    if (args.action === "list") return structuredClone(installed) as never;
    if (args.action === "marketplaces") return [] as never;
    if (args.action === "disable") installed[0].enabled = false;
    if (args.action === "delete") installed = [];
    return null as never;
  });
});
describe("Integrations management", () => {
  it("leaves a failed import editable and shows its error", async () => {
    vi.mocked(pluginAction).mockImplementation(async args => {
      if (args.action === "list") return installed as never;
      if (args.action === "marketplaces") return [] as never;
      if (args.action === "import_path") throw new Error("Skill folder unavailable");
      return null as never;
    });
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.click(screen.getByRole("button", { name: "Add plugins" }));
    fireEvent.change(screen.getByLabelText("Source"), { target: { value: "/missing/skill" } });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Import" })));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("Skill folder unavailable");
    expect(screen.getByLabelText("Source")).toHaveValue("/missing/skill");
    expect(screen.getByRole("button", { name: "Import" })).toBeEnabled();
  });

  it("requires a chat for agent help and keeps drafts untouched", async () => {
    activeThreadId = null;
    writeSession("drafts", { "other-chat": "Preserve me" });
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.click(screen.getByRole("button", { name: "Ask Themis" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Open a chat");
    expect(readSession("drafts", {})).toEqual({ "other-chat": "Preserve me" });
  });

  it("keeps disabled plugin children gated and replaces failed icons with a glyph", async () => {
    installed[0].enabled = false;
    installed[0].spec.icon = "https://example.com/missing.svg";
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.error(screen.getByAltText(""));
    expect(screen.queryByAltText("")).toBeNull();
    expect(screen.getByRole("button", { name: "View docs" }).closest("li")!.querySelector("svg")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Skills" }));
    expect(screen.getByRole("button", { name: "Enable Draft" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Enable Draft" })).toHaveTextContent("Plugin disabled");
  });
  it.each(["discovered", "imported"])("reenables a disabled %s standalone skill from Skills", async source => {
    installed[0] = { ...installed[0], enabled: false, source: source === "discovered" ? "discovered" : null, spec: { ...installed[0].spec, package_kind: "skill", disabled_skills: ["draft"] } };
    vi.mocked(pluginAction).mockImplementation(async args => {
      if (args.action === "list") return structuredClone(installed) as never;
      if (args.action === "marketplaces") return [] as never;
      if (args.action === "enable") installed[0].enabled = true;
      if (args.action === "set_component_enabled") installed[0].spec.disabled_skills = [];
      return null as never;
    });
    render(<Plugins />);
    await screen.findByText("No plugins installed");
    fireEvent.click(screen.getByRole("button", { name: "Skills" }));
    const enable = screen.getByRole("button", { name: "Enable Draft" });
    expect(enable).toBeEnabled();
    expect(enable).toHaveTextContent("Disabled");
    await act(async () => fireEvent.click(enable));
    expect(pluginAction).toHaveBeenCalledWith(expect.objectContaining({ action: "enable", name: "docs" }));
    expect(pluginAction).toHaveBeenCalledWith(expect.objectContaining({ action: "set_component_enabled", id: "draft", enabled: true }));
    expect(screen.getByRole("button", { name: "Disable Draft" })).toBeEnabled();
  });
  it("lists standalone capabilities in Skills instead of Plugins while retaining real one-skill packages", async () => {
    installed.push({ ...installed[0], source: "discovered", spec: { ...installed[0].spec, name: "discovered-1234", skills: [{ ...installed[0].spec.skills[0], name: "Discovered draft" }] } });
    installed.push({ ...installed[0], spec: { ...installed[0].spec, name: "standalone", package_kind: "skill", skills: [{ ...installed[0].spec.skills[0], name: "Imported draft" }] } });
    for (const kind of ["mcp", "hook"] as const) installed.push({ ...installed[0], spec: { ...installed[0].spec, name: `standalone-${kind}`, package_kind: kind, skills: [] } });
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    expect(screen.queryByRole("button", { name: "View Discovered draft" })).toBeNull();
    expect(screen.queryByRole("button", { name: "View standalone" })).toBeNull();
    expect(screen.queryByRole("button", { name: "View standalone-mcp" })).toBeNull();
    expect(screen.queryByRole("button", { name: "View standalone-hook" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Skills" }));
    expect(screen.getByRole("button", { name: "View Discovered draft" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "View Imported draft" })).toBeInTheDocument();
  });
  it("searches descriptions across categories and prepares a targeted agent draft", async () => {
    writeSession("drafts", { "test-chat": "Keep my existing request." });
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "document" } });
    expect(screen.getByRole("button", { name: "View docs" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Skills" }));
    expect(screen.getByRole("button", { name: "View Draft" })).toBeInTheDocument();
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "missing" } });
    expect(screen.getByText("No matching integrations")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Ask Themis" }));
    expect(readSession<Record<string, string>>("drafts", {})["test-chat"]).toContain("[[skill:manage-skills]]");
    expect(readSession<Record<string, string>>("drafts", {})["test-chat"]).toContain("Please [[skill:manage-skills]] for me.");
    expect(readSession<Record<string, string>>("drafts", {})["test-chat"]).not.toContain("Help me manage");
    expect(readSession<Record<string, string>>("drafts", {})["test-chat"]).toContain("Current search: missing");
    expect(readSession<Record<string, string>>("drafts", {})["test-chat"]).toMatch(/^Keep my existing request\.\n\n/);
    expect(pluginAction).not.toHaveBeenCalledWith(expect.objectContaining({ action: "save" }));
  });

  it("keeps direct import accessible and puts manual configuration behind Advanced", async () => {
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.click(screen.getByRole("button", { name: "Add plugins" }));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByLabelText("Source")).toBeVisible();
    fireEvent.change(within(dialog).getByLabelText("What would you like Themis to do?"), { target: { value: "Connect my document service" } });
    expect(within(dialog).getByText("Advanced").closest("details")).not.toHaveAttribute("open");
    fireEvent.click(within(dialog).getByRole("button", { name: "Ask Themis" }));
    expect(readSession<Record<string, string>>("drafts", {})["test-chat"]).toContain("Connect my document service");
  });

  it("searches MCP and hook names and collapses their configuration", async () => {
    installed[0].spec.mcp = { filesystem: { command: "node", args: [], env: {}, enabled: true } };
    installed[0].spec.hooks = [{ name: "startup-check", event: "RunStart", command: "true", enabled: true, blocking: false, timeout_seconds: 10 }];
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.click(screen.getByRole("button", { name: "MCP" }));
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "filesystem" } });
    fireEvent.click(screen.getByRole("button", { name: "View filesystem" }));
    expect(within(screen.getByRole("dialog")).getByText("Advanced configuration").closest("details")).not.toHaveAttribute("open");
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    fireEvent.click(screen.getByRole("button", { name: "Hooks" }));
    expect(screen.getByText("No matching integrations")).toBeInTheDocument();
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "startup" } });
    expect(screen.getByRole("button", { name: "View startup-check" })).toBeInTheDocument();
  });

  it("summarizes MCP test results with full details behind Advanced", async () => {
    installed[0].spec.mcp = { filesystem: { command: "node", args: [], env: {}, enabled: true } };
    vi.mocked(pluginAction).mockImplementation(async args => {
      if (args.action === "list") return installed as never;
      if (args.action === "marketplaces") return [] as never;
      if (args.action === "test_mcp") return { connected: true, tools: [{ name: "read_file", inputSchema: { hugeSchemaMarker: true } }] } as never;
      return null as never;
    });
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.click(screen.getByRole("button", { name: "MCP" }));
    fireEvent.click(screen.getByRole("button", { name: "More actions for filesystem" }));
    fireEvent.click(screen.getByRole("button", { name: "Test" }));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Run test" })));
    expect(screen.getByRole("status")).toHaveTextContent("Connected · 1 tools");
    expect(screen.getByText("read_file")).toBeVisible();
    expect(screen.getByText("Advanced result").closest("details")).not.toHaveAttribute("open");
  });

  it("creates a manual hook template that outputs valid JSON", async () => {
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.click(screen.getByRole("button", { name: "Hooks" }));
    fireEvent.click(screen.getByRole("button", { name: "Add hooks" }));
    fireEvent.click(screen.getByRole("button", { name: "Create with JSON", hidden: true }));
    const spec = JSON.parse((screen.getByLabelText("Configuration") as HTMLTextAreaElement).value);
    expect(spec.hooks[0].command).toBe("printf '{}'");
  });

  it("shows discovered skill names while retaining internal identities for actions", async () => {
    installed[0] = { ...installed[0], source: "discovered", spec: { ...installed[0].spec, name: "discovered-1234", origin: { kind: "discovered", location: "/project/.agents/skills/draft" } } };
    render(<Plugins />);
    await screen.findByText("No plugins installed");
    fireEvent.click(screen.getByRole("button", { name: "Skills" }));
    await screen.findByRole("button", { name: "View Draft" });
    expect(screen.queryByText("discovered-1234")).toBeNull();
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Disable Draft" })));
    expect(pluginAction).toHaveBeenCalledWith(expect.objectContaining({ action: "set_component_enabled", name: "discovered-1234", kind: "skill" }));
    expect(screen.getByText("Draft · User")).toBeInTheDocument();
    expect(screen.queryByText("discovered-1234")).toBeNull();
  });

  it("keeps unavailable discovered skills out of Plugins", async () => {
    installed[0] = { ...installed[0], source: "discovered", spec: { ...installed[0].spec, name: "discovered-1234", skills: [], origin: { kind: "discovered", location: "/project/.agents/skills/broken-skill/" } } };
    render(<Plugins />);
    await screen.findByText("No plugins installed");
    expect(screen.queryByText("discovered-1234")).toBeNull();
  });

  it("previews public bundled skills without installing and retains the associated icon", async () => {
    vi.mocked(pluginAction).mockImplementation(async args => {
      if (args.action === "list") return installed as never;
      if (args.action === "marketplaces") return [{ name: "official", source: "official-source" }] as never;
      if (args.action === "catalog") return { plugins: [{ name: "public-docs", icon: "https://example.com/icon.svg" }] } as never;
      if (args.action === "preview") return { ...installed[0].spec, name: "public-docs" } as never;
      return null as never;
    });
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Not installed" })));
    expect(screen.getByAltText("")).toHaveAttribute("src", "https://example.com/icon.svg");
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "View public-docs" })));
    const dialog = screen.getByRole("dialog", { name: "public-docs" });
    expect(within(dialog).getByText("Read the sources before drafting.", { exact: false })).toBeVisible();
    expect(pluginAction).toHaveBeenCalledWith(expect.objectContaining({ action: "preview", marketplace: "official", name: "public-docs" }));
    expect(pluginAction).not.toHaveBeenCalledWith(expect.objectContaining({ action: "install" }));
    expect(pluginAction).not.toHaveBeenCalledWith(expect.objectContaining({ action: "save" }));
  });

  it("passes the selected import source into an editable agent draft", async () => {
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.click(screen.getByRole("button", { name: "Add plugins" }));
    fireEvent.change(screen.getByLabelText("Import from"), { target: { value: "repository" } });
    fireEvent.change(screen.getByLabelText("Source"), { target: { value: "https://github.com/example/skills.git" } });
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Ask Themis" }));
    expect(readSession<Record<string, string>>("drafts", {})["test-chat"]).toContain("https://github.com/example/skills.git");
    expect(pluginAction).not.toHaveBeenCalledWith(expect.objectContaining({ action: "import_repository" }));
  });

  it("keeps marketplace entries associated with the source used to install them", async () => {
    let finish: (value: unknown) => void = () => {};
    vi.mocked(pluginAction).mockImplementation(async args => {
      if (args.action === "list") return installed as never;
      if (args.action === "marketplaces") return [{ name: "official", source: "official-source" }] as never;
      if (args.action === "catalog") return await new Promise<unknown>(resolve => { finish = resolve; }) as never;
      return null as never;
    });
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.click(screen.getByRole("button", { name: "Not installed" }));
    expect(screen.getByRole("combobox", { name: "Marketplace" })).toBeDisabled();
    await act(async () => finish({ plugins: [{ name: "review", description: "Review pull requests" }] }));
    expect(screen.getByRole("combobox", { name: "Marketplace" })).toBeEnabled();
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "pull official-source" } });
    expect(screen.getByText("review")).toBeInTheDocument();
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "missing" } });
    expect(screen.getByText("No matching integrations")).toBeInTheDocument();
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "" } });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Install" })));
    expect(pluginAction).toHaveBeenCalledWith(expect.objectContaining({ action: "install", name: "review", marketplace: "official" }));
  });

  it("keeps disabled installed packages out of Not installed", async () => {
    installed[0].source = "official";
    installed[0].enabled = false;
    vi.mocked(pluginAction).mockImplementation(async args => {
      if (args.action === "list") return installed as never;
      if (args.action === "marketplaces") return [{ name: "official", source: "official-source" }] as never;
      if (args.action === "catalog") return { plugins: [{ name: "docs" }, { name: "other", icon: "https://example.com/other.svg" }] } as never;
      return null as never;
    });
    render(<Plugins />);
    await screen.findByRole("button", { name: "Enable docs" });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Not installed" })));
    expect(screen.queryByRole("button", { name: "View docs" })).toBeNull();
    expect(screen.getByRole("button", { name: "View other" })).toBeInTheDocument();
    expect(screen.getByAltText("")).toHaveAttribute("src", "https://example.com/other.svg");
  });

  it("offers actual standalone skills with pinned sources and removes installed entries", async () => {
    vi.mocked(pluginAction).mockImplementation(async args => {
      if (args.action === "list") return structuredClone(installed) as never;
      if (args.action === "marketplaces") return [] as never;
      if (args.action === "import_repository") installed.push({ ...installed[0], enabled: false, spec: { ...installed[0].spec, name: "pdf", package_kind: "skill", origin: { kind: "repository", location: String(args.url), subdirectory: String(args.subdirectory) } } });
      return null as never;
    });
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.click(screen.getByRole("button", { name: "Skills" }));
    fireEvent.click(screen.getByRole("button", { name: "Not installed" }));
    expect(screen.getByRole("button", { name: "View Word documents" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "View Spreadsheets" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "View PDF" }));
    expect(screen.getByRole("link", { name: "Read SKILL.md" })).toHaveAttribute("href", "https://github.com/anthropics/skills/blob/683bc88e56f3e09ba94f7055977f3d3aa499f202/skills/pdf/SKILL.md");
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    const row = screen.getByRole("button", { name: "View PDF" }).closest("li")!;
    expect(row.querySelector("svg")).toBeInTheDocument();
    await act(async () => fireEvent.click(within(row).getByRole("button", { name: "Install" })));
    expect(pluginAction).toHaveBeenCalledWith(expect.objectContaining({ action: "import_repository", name: "pdf", url: "https://github.com/anthropics/skills.git", subdirectory: "skills/pdf", reference: "683bc88e56f3e09ba94f7055977f3d3aa499f202" }));
    expect(screen.queryByRole("button", { name: "View PDF" })).toBeNull();
  });

  it("opens plugin skills and Markdown in a dialog without inline configuration", async () => {
    render(<Plugins />);
    fireEvent.click(await screen.findByRole("button", { name: "View docs" }));
    const dialog = screen.getByRole("dialog", { name: "docs" });
    expect(within(dialog).getByText("Read the sources before drafting.", { exact: false })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "MCP" })).toBeInTheDocument();
  });
  it("shows actions before opening the uninstall dialog and removes only after confirmation", async () => {
    render(<Plugins />);
    fireEvent.click(await screen.findByRole("button", { name: "More actions for docs" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Uninstall" }));
    const dialog = screen.getByRole("dialog", { name: "Uninstall docs?" });
    expect(installed).toHaveLength(1);
    await act(async () => fireEvent.click(within(dialog).getByRole("button", { name: "Uninstall" })));
    expect(screen.queryByText("docs")).toBeNull();
  });
  it("disables a plugin directly without requiring a dialog", async () => {
    render(<Plugins />);
    const disable = await screen.findByRole("button", { name: "Disable docs" });
    await act(async () => fireEvent.click(disable));
    expect(screen.getByRole("button", { name: "Enable docs" })).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
