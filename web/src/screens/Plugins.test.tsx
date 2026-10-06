// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Plugins } from "./Plugins";
import { initialState } from "../state/reducer";
import type { Plugin } from "../lib/types";
import { pluginAction } from "../lib/tauri";

let installed: Plugin[];
let projectRoot: string | null;
vi.mock("../state/store", async original => ({ ...await original<typeof import("../state/store")>(), useApp: () => ({ state: { ...initialState, activeProjectRoot: projectRoot }, dispatch: vi.fn() }) }));
vi.mock("../lib/tauri", () => ({ pluginAction: vi.fn() }));
afterEach(cleanup);
beforeEach(() => {
  vi.clearAllMocks();
  projectRoot = "/project";
  installed = [{ scope: "global", revision: "v1", enabled: true, source: null, spec: { name: "docs", description: "Documents", version: "1", skills: [{ id: "draft", name: "Draft", description: "Draft a document", instructions: "Read the sources before drafting.", allowedTools: [], scripts: [] }], mcp: { filesystem: { command: "node", args: [], env: { SECRET: "hidden-secret" }, enabled: true } }, hooks: [{ name: "startup-check", event: "RunStart", command: "printf '{}'", enabled: false, blocking: false, timeout_seconds: 10 }], files: {}, unsupported: [] } }];
  vi.mocked(pluginAction).mockImplementation(async args => {
    if (args.action === "list") return structuredClone(installed) as never;
    if (args.action === "marketplaces") return [{ name: "official", source: "official-source" }] as never;
    if (args.action === "catalog") return { plugins: [{ name: "public-docs", description: "Review documents", icon: "https://example.com/icon.svg" }] } as never;
    if (args.action === "preview") return { ...installed[0].spec, name: String(args.name) } as never;
    if (args.action === "preview_repository") return { ...installed[0].spec, name: "pdf", package_kind: "skill", skills: [{ ...installed[0].spec.skills[0], id: "pdf" }], files: { "SKILL.md": "---\nname: pdf\nlicense: Original-License\n---\n# PDF workflow\nUPSTREAM_FULL_TAIL" } } as never;
    if (args.action === "install") { installed.push({ ...installed[0], source: String(args.marketplace), spec: { ...installed[0].spec, name: String(args.name) } }); return null as never; }
    if (args.action === "import_repository") { installed.push({ ...installed[0], spec: { ...installed[0].spec, package_kind: "skill", name: String(args.name), skills: [{ ...installed[0].spec.skills[0], id: String(args.name), name: "PDF" }], origin: { kind: "repository", location: String(args.url), subdirectory: String(args.subdirectory) } } }); return null as never; }
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
  expect(vi.mocked(pluginAction).mock.calls.every(([args]) => ["list", "marketplaces", "catalog", "preview", "preview_repository", "enable", "disable", "set_component_enabled", "install", "import_repository", "delete", "remove_component"].includes(String(args.action)))).toBe(true);
}
describe("Integration browsing and lifecycle", () => {
  it("shows complete captured Markdown after selecting a bundled skill", async () => {
    const source = `---\nname: draft\nlicense: Original-License\n---\n# Full skill\n\n${"Complete source text. ".repeat(100)}\n\n## Final section\nUNTRUNCATED_TAIL`.replace(/\n/g, "\r\n");
    installed[0].spec.files = { "custom/notes/SKILL.md": source };
    installed[0].spec.skill_paths = { draft: "custom/notes/SKILL.md" };
    render(<Plugins />);
    fireEvent.click(await screen.findByRole("button", { name: "View docs" }));
    const dialog = screen.getByRole("dialog", { name: "docs" });
    expect(dialog).toHaveTextContent("1 skill");
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
    expect(dialog).toHaveTextContent("2 skills");
    expect(within(dialog).getByRole("button", { name: "View Draft" })).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "View Review" }));
    expect(dialog).toHaveTextContent("FULL_REVIEW_TAIL");
    expect(dialog).not.toHaveTextContent("Draft source");
  });
  it("replaces a long public bundle list with its selected body and returns to skills", async () => {
    installed[0].spec.skills = Array.from({ length: 23 }, (_, index) => ({ ...installed[0].spec.skills[0], id: `skill-${index}`, name: `Skill ${index}`, instructions: `# Body ${index}\nComplete instructions` }));
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Not installed" })));
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
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Not installed" })));
    const add = screen.getByRole("button", { name: "Add marketplace" });
    expect(add.querySelector("svg")).toBeInTheDocument();
    expect(add).toHaveAttribute("title", "Add marketplace");
    fireEvent.click(add);
    const dialog = screen.getByRole("dialog", { name: "Add marketplace" });
    fireEvent.change(within(dialog).getByRole("textbox", { name: "Name" }), { target: { value: "personal" } });
    fireEvent.change(within(dialog).getByRole("textbox", { name: "URL or local path" }), { target: { value: source } });
    await act(async () => fireEvent.click(within(dialog).getByRole("button", { name: "Add" })));
    expect(pluginAction).toHaveBeenCalledWith(expect.objectContaining({ action: "add_marketplace", name: "personal", source }));
    expect(pluginAction).toHaveBeenCalledWith({ action: "catalog", name: "personal" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(pluginAction).not.toHaveBeenCalledWith(expect.objectContaining({ action: "install" }));
  });
  it("keeps marketplace errors actionable and allows correction without installing", async () => {
    const previous = vi.mocked(pluginAction).getMockImplementation()!;
    vi.mocked(pluginAction).mockImplementation(async args => {
      if (args.action === "add_marketplace") throw new Error("Marketplace source could not be read");
      return previous(args) as never;
    });
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Not installed" })));
    fireEvent.click(screen.getByRole("button", { name: "Add marketplace" }));
    const dialog = screen.getByRole("dialog", { name: "Add marketplace" });
    expect(within(dialog).getByRole("button", { name: "Add" })).toBeDisabled();
    fireEvent.change(within(dialog).getByRole("textbox", { name: "Name" }), { target: { value: "personal" } });
    fireEvent.change(within(dialog).getByRole("textbox", { name: "URL or local path" }), { target: { value: "/missing" } });
    await act(async () => fireEvent.click(within(dialog).getByRole("button", { name: "Add" })));
    expect(within(dialog).getByRole("alert")).toHaveTextContent("Marketplace source could not be read");
    expect(within(dialog).getByRole("textbox", { name: "URL or local path" })).toHaveValue("/missing");
    expect(within(dialog).getByRole("button", { name: "Add" })).toBeEnabled();
    expect(pluginAction).not.toHaveBeenCalledWith(expect.objectContaining({ action: "install" }));
  });
  it("reports empty packages without inventing skills", async () => {
    installed[0].spec.skills = [];
    render(<Plugins />);
    fireEvent.click(await screen.findByRole("button", { name: "View docs" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("0 skills");
    expect(screen.getByRole("dialog")).toHaveTextContent("No bundled skills.");
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
    installed[0].spec.mcp.remote = { url: "https://example.com/mcp", args: [], env: {}, enabled: false };
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.click(screen.getByRole("button", { name: "MCP" }));
    fireEvent.click(screen.getByRole("button", { name: "View filesystem" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("Local process");
    expect(screen.getByRole("dialog")).toHaveTextContent("node");
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
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Not installed" })));
    expect(screen.getByAltText("")).toHaveAttribute("src", "https://example.com/icon.svg");
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "View public-docs" })));
    const dialog = screen.getByRole("dialog", { name: "public-docs" });
    fireEvent.click(within(dialog).getByRole("button", { name: "View Draft" }));
    expect(dialog).toHaveTextContent("Read the sources before drafting.");
    expectNoConfiguration();
  });
  it("excludes disabled installed packages from the matching source catalog", async () => {
    installed[0].source = "official";
    installed[0].spec.name = "public-docs";
    installed[0].enabled = false;
    render(<Plugins />);
    await screen.findByRole("button", { name: "View public-docs" });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Not installed" })));
    expect(screen.queryByRole("button", { name: "View public-docs" })).toBeNull();
    expect(screen.getByText("No public plugins available")).toBeInTheDocument();
  });
  it("previews actual pinned standalone skill Markdown and filters matching installed origins", async () => {
    installed.push({ ...installed[0], enabled: false, spec: { ...installed[0].spec, name: "pdf", package_kind: "skill", origin: { kind: "repository", location: "https://github.com/anthropics/skills.git", subdirectory: "skills/pdf" } } });
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.click(screen.getByRole("button", { name: "Skills" }));
    fireEvent.click(screen.getByRole("button", { name: "Not installed" }));
    expect(screen.queryByRole("button", { name: "View PDF" })).toBeNull();
    expect(screen.getByRole("button", { name: "View Word documents" }).closest("li")!.querySelector("svg")).toBeInTheDocument();
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "View Word documents" })));
    expect(screen.getByRole("dialog")).toHaveTextContent("UPSTREAM_FULL_TAIL");
    expect(pluginAction).toHaveBeenCalledWith(expect.objectContaining({ action: "preview_repository", url: "https://github.com/anthropics/skills.git", reference: "683bc88e56f3e09ba94f7055977f3d3aa499f202", subdirectory: "skills/docx" }));
    expectNoConfiguration();
  });
  it("refreshes after chat management and leaves unrelated skill IDs available", async () => {
    installed[0].spec.skills[0].id = "pdf";
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.click(screen.getByRole("button", { name: "Skills" }));
    fireEvent.click(screen.getByRole("button", { name: "Not installed" }));
    expect(screen.getByRole("button", { name: "View PDF" })).toBeInTheDocument();
    installed.push({ ...installed[0], spec: { ...installed[0].spec, name: "pdf", package_kind: "skill", origin: { kind: "repository", location: "https://github.com/anthropics/skills", subdirectory: "skills/pdf" } } });
    await act(async () => window.dispatchEvent(new Event("themis-plugins-changed")));
    expect(screen.queryByRole("button", { name: "View PDF" })).toBeNull();
    expectNoConfiguration();
  });
  it("ignores a closed preview's response when another skill is open", async () => {
    let finish: (value: unknown) => void = () => {};
    vi.mocked(pluginAction).mockImplementation(async args => {
      if (args.action === "list") return installed as never;
      if (args.action === "marketplaces") return [] as never;
      if (args.action === "preview_repository") return await new Promise<unknown>(resolve => { finish = resolve; }) as never;
      return null as never;
    });
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.click(screen.getByRole("button", { name: "Skills" }));
    fireEvent.click(screen.getByRole("button", { name: "Not installed" }));
    fireEvent.click(screen.getByRole("button", { name: "View PDF" }));
    expect(screen.getByRole("status")).toHaveTextContent("Loading skills");
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    fireEvent.click(screen.getByRole("button", { name: "Installed" }));
    fireEvent.click(screen.getByRole("button", { name: "View Draft" }));
    await act(async () => finish({ ...installed[0].spec, files: { "SKILL.md": "STALE_PREVIEW" } }));
    expect(screen.getByRole("dialog")).toHaveTextContent("Read the sources before drafting.");
    expect(screen.queryByText("STALE_PREVIEW")).toBeNull();
  });
  it.each(["Plugins", "Skills", "MCP", "Hooks"])("toggles %s with a switch and visible status", async category => {
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.click(screen.getByRole("button", { name: category }));
    const control = screen.getByRole("switch");
    const wasEnabled = control.getAttribute("aria-checked") === "true";
    expect(control.closest("li")).toHaveTextContent(wasEnabled ? "Enabled" : "Disabled");
    await act(async () => fireEvent.click(control));
    expect(screen.getByRole("switch")).toHaveAttribute("aria-checked", String(!wasEnabled));
    expect(screen.getByRole("switch").closest("li")).toHaveTextContent(wasEnabled ? "Disabled" : "Enabled");
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
  it("installs public packages and standalone skills and restores uninstalled skills to the catalog", async () => {
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Not installed" })));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Install" })));
    expect(pluginAction).toHaveBeenCalledWith(expect.objectContaining({ action: "install", name: "public-docs", marketplace: "official" }));
    expect(screen.queryByRole("button", { name: "View public-docs" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Skills" }));
    fireEvent.click(screen.getByRole("button", { name: "Not installed" }));
    const row = screen.getByRole("button", { name: "View PDF" }).closest("li")!;
    await act(async () => fireEvent.click(within(row).getByRole("button", { name: "Install" })));
    expect(pluginAction).toHaveBeenCalledWith(expect.objectContaining({ action: "import_repository", name: "pdf", url: "https://github.com/anthropics/skills.git", reference: "683bc88e56f3e09ba94f7055977f3d3aa499f202", subdirectory: "skills/pdf" }));
    expect(screen.queryByRole("button", { name: "View PDF" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Installed" }));
    fireEvent.click(screen.getByRole("button", { name: "Uninstall PDF" }));
    await act(async () => fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Uninstall" })));
    expect(installed.find(plugin => plugin.spec.name === "pdf")!.spec.skills).toHaveLength(0);
    fireEvent.click(screen.getByRole("button", { name: "Not installed" }));
    expect(screen.getByRole("button", { name: "View PDF" })).toBeInTheDocument();
    expectNoConfiguration();
  });

});
