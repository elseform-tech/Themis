// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen, within } from "@testing-library/react";
import type { EffectivePermissions } from "../lib/types";

const getEffectivePermissions = vi.fn();
vi.mock("../lib/tauri", () => ({ getEffectivePermissions: (...args: unknown[]) => getEffectivePermissions(...args) }));

import { PermissionsSettings, PermissionsSummary } from "./PermissionsView";

const VIEW: EffectivePermissions = {
  project_root: "/work/app",
  mode: "yolo",
  validation_error: null,
  files: [
    { scope: "user", path: "/data/runtime.jsonc", status: "loaded", error: null },
    { scope: "project", path: "/work/app/.themis/config.jsonc", status: "invalid", error: { message: "unknown field `sandboxx`", line: 3, column: 5 } },
  ],
  approval: {
    default: "ask",
    default_source: "default",
    project_rule_count: 2,
    rules: [
      { tool: "shell", action: "deny", command: ["git", "push"], source: "project" },
      { tool: "git", action: "deny", risk: "write", source: "project" },
      { tool: "shell", action: "allow", command: ["cargo", "test"], source: "user" },
      { tool: "*", action: "allow", risk: "read", source: "default" },
    ],
  },
  sandbox: {
    enabled: true,
    state: "on",
    backend: "Seatbelt",
    summary: "Seatbelt, chosen automatically for macOS",
    reason: "chosen automatically for macOS",
    network: "allow",
    network_enforced: true,
    writable_paths: ["/work/app", "/private/tmp"],
    configured_writable_paths: [],
    protected_paths: ["/data", "/work/app/.themis"],
    secret_paths: ["~/.ssh", "~/.aws"],
    backends: [
      { id: "seatbelt", label: "Seatbelt", os: "macOS", supported_here: true, available: true, detail: "works" },
      { id: "bubblewrap", label: "bubblewrap", os: "Linux", supported_here: false, available: false, detail: "not available on macOS" },
    ],
    covers: ["shell commands", "background tasks"],
    not_covered: ["MCP servers", "plugin hooks"],
  },
  chat: { thread_id: "t1", grants_path: "/data/chat_grants/t1.json", grants: [{ tool: "shell", command: ["cargo", "test"], risk: "execute" }, { tool: "shell_unsandboxed", command: ["npm", "install"], risk: "execute" }], warning: null },
};

afterEach(() => { cleanup(); getEffectivePermissions.mockReset(); });

describe("PermissionsSummary", () => {
  it("is read-only: no buttons, inputs, links or switches", () => {
    render(<PermissionsSummary view={VIEW} />);
    expect(screen.queryAllByRole("button")).toHaveLength(0);
    expect(screen.queryAllByRole("textbox")).toHaveLength(0);
    expect(screen.queryAllByRole("link")).toHaveLength(0);
    expect(screen.queryAllByRole("switch")).toHaveLength(0);
    expect(screen.queryAllByRole("checkbox")).toHaveLength(0);
  });

  it("states only the current mode", () => {
    render(<PermissionsSummary view={VIEW} />);
    const mode = within(screen.getByRole("region", { name: "Permission mode" }));
    expect(mode.getByText(/No approval prompts; the sandbox still applies\./)).toBeInTheDocument();
    expect(mode.getByText(/in this chat/)).toBeInTheDocument();
    expect(mode.queryByText(/The rules below decide/)).toBeNull();
  });

  it("lists rules in one ordered list with short source labels", () => {
    render(<PermissionsSummary view={VIEW} />);
    const rules = within(screen.getByRole("region", { name: "Rules" }));
    expect(rules.queryByText(/from this project/)).toBeNull();
    const items = rules.getAllByRole("listitem").map(item => item.textContent);
    expect(items[0]).toMatch(/^Deny shell · git push.*Project$/);
    expect(items[1]).toMatch(/^Deny git · write.*Project$/);
    expect(items[2]).toMatch(/^Allow shell · cargo test.*User$/);
    expect(items[3]).toMatch(/^Allow any tool · read.*Built-in$/);
    expect(items[4]).toMatch(/^Otherwise ask.*Built-in$/);
  });

  it("summarises the sandbox without fixed or irrelevant detail", () => {
    render(<PermissionsSummary view={{ ...VIEW, sandbox: { ...VIEW.sandbox!, configured_writable_paths: ["/opt/cache"] } }} />);
    const sandbox = within(screen.getByRole("region", { name: "Sandbox" }));
    expect(sandbox.getByText("On · Seatbelt, chosen automatically for macOS")).toBeInTheDocument();
    expect(sandbox.getByText("Allowed")).toBeInTheDocument();
    expect(sandbox.getByText("/opt/cache")).toBeInTheDocument();
    expect(sandbox.getByText("2 secret folders are never readable")).toBeInTheDocument();
    // Temp folders, protected absolute paths, coverage and other systems' backends are documented, not shown.
    expect(sandbox.queryByText("/private/tmp")).toBeNull();
    expect(sandbox.queryByText("/work/app/.themis")).toBeNull();
    expect(sandbox.queryByText(/bubblewrap/)).toBeNull();
    expect(sandbox.queryByText(/MCP servers/)).toBeNull();
  });

  it("shows config file paths as copyable text and marks only problems", () => {
    render(<PermissionsSummary view={VIEW} />);
    const files = within(screen.getByRole("region", { name: "Configuration files" }));
    expect(files.getByText("/data/runtime.jsonc").tagName).toBe("CODE");
    expect(files.queryByText("Loaded")).toBeNull();
    expect(files.getByRole("alert")).toHaveTextContent("Invalid, runs are refused. Line 3, column 5: unknown field `sandboxx`");
  });

  it("does not repeat a file problem in a separate banner", () => {
    render(<PermissionsSummary view={{ ...VIEW, validation_error: "runtime configuration: invalid runtime configuration: unknown field `sandboxx`" }} />);
    expect(screen.getAllByRole("alert")).toHaveLength(1);
  });

  it("shows a validation error without a file problem once, without the doubled prefix", () => {
    const files = VIEW.files.map(file => ({ ...file, status: "loaded" as const, error: null }));
    render(<PermissionsSummary view={{ ...VIEW, files, validation_error: "runtime configuration: invalid runtime configuration: bad override" }} />);
    expect(screen.getByRole("alert")).toHaveTextContent("Runs are refused until the configuration is fixed: bad override");
  });

  it("lists the chat's grants with the file to edit", () => {
    render(<PermissionsSummary view={VIEW} />);
    const chat = within(screen.getByRole("region", { name: "Allowed in this chat" }));
    expect(chat.getByText("cargo test")).toBeInTheDocument();
    expect(chat.getByText(/outside the sandbox/)).toBeInTheDocument();
    expect(chat.getByText("/data/chat_grants/t1.json")).toBeInTheDocument();
  });

  it("hides the chat section when nothing is allowed", () => {
    render(<PermissionsSummary view={{ ...VIEW, chat: { ...VIEW.chat!, grants: [] } }} />);
    expect(screen.queryByRole("region", { name: "Allowed in this chat" })).toBeNull();
  });

  it("explains an unavailable sandbox without listing what it would do", () => {
    render(<PermissionsSummary view={{ ...VIEW, sandbox: { ...VIEW.sandbox!, state: "unavailable", backend: null, summary: "Not available on this system (Windows)" } }} />);
    const sandbox = within(screen.getByRole("region", { name: "Sandbox" }));
    expect(sandbox.getByText("Not available · Not available on this system (Windows)")).toBeInTheDocument();
    expect(sandbox.queryByText("Network")).toBeNull();
  });
});

describe("PermissionsSettings", () => {
  it("re-reads the view on every open", async () => {
    getEffectivePermissions.mockResolvedValue(VIEW);
    const first = render(<PermissionsSettings projectRoot="/work/app" threadId="t1" />);
    await act(async () => {});
    expect(getEffectivePermissions).toHaveBeenCalledWith("/work/app", "t1");
    first.unmount();
    render(<PermissionsSettings projectRoot="/work/app" threadId="t1" />);
    await act(async () => {});
    expect(getEffectivePermissions).toHaveBeenCalledTimes(2);
    expect(screen.getByRole("region", { name: "Sandbox" })).toBeInTheDocument();
  });

  it("reports a failed read", async () => {
    getEffectivePermissions.mockRejectedValue(new Error("server down"));
    render(<PermissionsSettings projectRoot={null} threadId={null} />);
    await act(async () => {});
    expect(screen.getByRole("alert")).toHaveTextContent("Permissions could not be read");
  });
});
