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
  it("passes the selected import source into an editable agent draft", async () => {
    render(<Plugins />);
    await screen.findByRole("button", { name: "View docs" });
    fireEvent.click(screen.getByRole("button", { name: "Add plugins" }));
    fireEvent.change(screen.getByLabelText("Import from"), { target: { value: "repository" } });
    fireEvent.change(screen.getByLabelText("Source"), { target: { value: "https://github.com/example/skills.git" } });
    fireEvent.click(screen.getByRole("button", { name: "Ask Themis" }));
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
    await act(async () => finish({ plugins: [{ name: "review" }] }));
    expect(screen.getByRole("combobox", { name: "Marketplace" })).toBeEnabled();
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
