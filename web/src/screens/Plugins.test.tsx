// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Plugins } from "./Plugins";
import { readSession } from "../state/session";
import { initialState } from "../state/reducer";
import type { Plugin } from "../lib/types";
import { pluginAction } from "../lib/tauri";

vi.mock("../state/store", async original => ({ ...await original<typeof import("../state/store")>(), useApp: () => ({ state: { ...initialState, activeProjectRoot: "/project", activeThreadId: "test-chat" }, dispatch: vi.fn() }) }));
vi.mock("../lib/tauri", () => ({ pluginAction: vi.fn() }));
let installed: Plugin[];
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
beforeEach(() => {
  vi.clearAllMocks();
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
  it("searches descriptions across categories and prepares a targeted agent draft", async () => {
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
    expect(readSession<Record<string, string>>("drafts", {})["test-chat"]).toContain("Current search: missing");
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

  it("shows discovered skill names while retaining internal identities for actions", async () => {
    installed[0] = { ...installed[0], source: "discovered", spec: { ...installed[0].spec, name: "discovered-1234", origin: { kind: "discovered", location: "/project/.agents/skills/draft" } } };
    render(<Plugins />);
    await screen.findByRole("button", { name: "View Draft" });
    expect(screen.queryByText("discovered-1234")).toBeNull();
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Disable Draft" })));
    expect(pluginAction).toHaveBeenCalledWith(expect.objectContaining({ action: "disable", name: "discovered-1234" }));
    fireEvent.click(screen.getByRole("button", { name: "Skills" }));
    expect(screen.getByText("Draft · User")).toBeInTheDocument();
    expect(screen.queryByText("discovered-1234")).toBeNull();
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
    fireEvent.click(screen.getByRole("button", { name: "Public" }));
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
