// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { EffectivePermissions } from "../lib/types";

const getEffectivePermissions = vi.fn();
const openRuntimeConfig = vi.fn();
const listEditors = vi.fn();
vi.mock("../lib/tauri", () => ({
  getEffectivePermissions: (...args: unknown[]) => getEffectivePermissions(...args),
  openRuntimeConfig: (...args: unknown[]) => openRuntimeConfig(...args),
  listEditors: () => listEditors(),
}));

import { PermissionsSettings, fileState, permissionProblems } from "./PermissionsView";

const SANDBOX = {
  enabled: true, state: "on" as const, backend: "Seatbelt", summary: "Seatbelt, chosen automatically for macOS", reason: "chosen automatically for macOS",
  network: "allow" as const, network_enforced: true, writable_paths: [], configured_writable_paths: [], protected_paths: [], secret_paths: [], backends: [], covers: [], not_covered: [],
};
const VIEW: EffectivePermissions = {
  project_root: null,
  mode: null,
  validation_error: null,
  files: [{ scope: "user", path: "/data/runtime.jsonc", status: "loaded", error: null }],
  approval: { default: "ask", default_source: "default", rules: [{ tool: "*", action: "allow", risk: "read", source: "default" }], project_rule_count: 0 },
  sandbox: SANDBOX,
  chat: null,
};

afterEach(() => { cleanup(); getEffectivePermissions.mockReset(); openRuntimeConfig.mockReset(); listEditors.mockReset(); });

async function show(view: EffectivePermissions) {
  getEffectivePermissions.mockResolvedValue(view);
  render(<PermissionsSettings />);
  await act(async () => {});
}

describe("PermissionsSettings", () => {
  it("shows only the global file and an Open action when all is well", async () => {
    await show(VIEW);
    expect(getEffectivePermissions).toHaveBeenCalledWith(null, null);
    expect(screen.getByText("runtime.jsonc")).toHaveAttribute("title", "/data/runtime.jsonc");
    expect(screen.getAllByRole("button").map(button => button.textContent)).toEqual(["Open"]);
    expect(screen.queryByRole("alert")).toBeNull();
    // No rules, sandbox details, project or chat sections.
    expect(screen.queryByText(/Seatbelt|Rules|this chat|this project/)).toBeNull();
  });

  it("says whether the file was read", async () => {
    await show(VIEW);
    expect(screen.getByText("Loaded")).toBeInTheDocument();
    expect(fileState({ ...VIEW, files: [{ ...VIEW.files[0], status: "missing" }] })).toEqual({ tone: "off", label: "Not created · defaults" });
    expect(fileState({ ...VIEW, files: [{ ...VIEW.files[0], status: "invalid", error: { message: "bad", line: 4, column: 1 } }] })).toEqual({ tone: "bad", label: "Invalid · line 4" });
    expect(fileState({ ...VIEW, validation_error: "runtime configuration: bad" })).toEqual({ tone: "bad", label: "Invalid" });
  });

  it("offers the installed editors and opens the file with the chosen one", async () => {
    listEditors.mockResolvedValue({ editors: [{ id: "", name: "Text editor" }, { id: "zed", name: "Zed" }] });
    openRuntimeConfig.mockResolvedValue(undefined);
    const onEditor = vi.fn();
    getEffectivePermissions.mockResolvedValue(VIEW);
    render(<PermissionsSettings editor="zed" onEditor={onEditor} />);
    await act(async () => {});
    const select = screen.getByRole("combobox", { name: "Open with" });
    expect(select).toHaveValue("zed");
    fireEvent.change(select, { target: { value: "" } });
    expect(onEditor).toHaveBeenCalledWith("");
    fireEvent.click(screen.getByRole("button", { name: "Open" }));
    expect(openRuntimeConfig).toHaveBeenCalledWith("zed");
  });

  it("opens the global file", async () => {
    openRuntimeConfig.mockResolvedValue(undefined);
    await show(VIEW);
    fireEvent.click(screen.getByRole("button", { name: "Open" }));
    expect(openRuntimeConfig).toHaveBeenCalledTimes(1);
  });

  it("reports a failure to open", async () => {
    openRuntimeConfig.mockRejectedValue(new Error("no editor"));
    await show(VIEW);
    fireEvent.click(screen.getByRole("button", { name: "Open" }));
    await act(async () => {});
    expect(screen.getByRole("alert")).toHaveTextContent("no editor");
  });

  it("re-reads on every open", async () => {
    await show(VIEW);
    cleanup();
    await show(VIEW);
    expect(getEffectivePermissions).toHaveBeenCalledTimes(2);
  });
});

describe("permissionProblems", () => {
  it("is empty in the normal case", () => {
    expect(permissionProblems(VIEW)).toEqual([]);
  });

  it("names an invalid file with its line", () => {
    const files = [{ scope: "user" as const, path: "/data/runtime.jsonc", status: "invalid" as const, error: { message: "unknown field `sandbx`, expected one of `skills`, `approval`, `sandbox`", line: 3, column: 5 } }];
    expect(permissionProblems({ ...VIEW, files, validation_error: "runtime configuration: invalid runtime configuration: unknown field `sandbx`" }))
      .toEqual(["Invalid · line 3: unknown field `sandbx`. Runs are refused until it is fixed."]);
  });

  it("explains a missing named sandbox, no sandbox and a sandbox turned off", () => {
    expect(permissionProblems({ ...VIEW, sandbox: { ...SANDBOX, state: "blocked", reason: "bubblewrap is not installed" } }))
      .toEqual(["Sandbox not available: bubblewrap is not installed. Commands are refused."]);
    expect(permissionProblems({ ...VIEW, sandbox: { ...SANDBOX, state: "unavailable" } })).toEqual(["No sandbox on this system."]);
    expect(permissionProblems({ ...VIEW, sandbox: { ...SANDBOX, state: "off" } })).toEqual(["Sandbox off."]);
  });
});
