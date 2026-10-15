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

  it("explains YOLO as no prompts with the sandbox still on", () => {
    render(<PermissionsSummary view={VIEW} />);
    expect(screen.getByText("No approval prompts; the sandbox still applies.")).toBeInTheDocument();
    expect(screen.getByText("this chat")).toBeInTheDocument();
  });

  it("lists rules in order with sources and project additions separately", () => {
    render(<PermissionsSummary view={VIEW} />);
    const rules = within(screen.getByRole("region", { name: "Rules" }));
    expect(rules.getByText("+2 deny rules from this project")).toBeInTheDocument();
    const items = rules.getAllByRole("listitem").map(item => item.textContent);
    expect(items[0]).toMatch(/^1\. Deny shell · git push.*Project file$/);
    expect(items[1]).toMatch(/^2\. Deny git · write.*Project file$/);
    expect(items[2]).toMatch(/^3\. Allow shell · cargo test.*User file$/);
    expect(items[3]).toMatch(/^4\. Allow any tool · read.*Built-in$/);
    expect(items[4]).toMatch(/^Otherwise ask.*Built-in$/);
  });

  it("shows sandbox status, network, paths and other backends", () => {
    render(<PermissionsSummary view={VIEW} />);
    const sandbox = within(screen.getByRole("region", { name: "Sandbox" }));
    expect(sandbox.getByText("On")).toBeInTheDocument();
    expect(sandbox.getByText("Seatbelt, chosen automatically for macOS")).toBeInTheDocument();
    expect(sandbox.getByText("Allowed")).toBeInTheDocument();
    expect(sandbox.getByText("/private/tmp")).toBeInTheDocument();
    expect(sandbox.getByText("/work/app/.themis")).toBeInTheDocument();
    expect(sandbox.getByText("bubblewrap: not available on this system (Linux only)")).toBeInTheDocument();
  });

  it("shows config file paths as copyable text with load status and the error line", () => {
    render(<PermissionsSummary view={VIEW} />);
    const files = within(screen.getByRole("region", { name: "Configuration files" }));
    expect(files.getByText("/data/runtime.jsonc").tagName).toBe("CODE");
    expect(files.getByText("Loaded")).toBeInTheDocument();
    expect(files.getByText("Invalid")).toBeInTheDocument();
    expect(files.getByRole("alert")).toHaveTextContent("Line 3, column 5: unknown field `sandboxx`");
  });

  it("lists the chat's grants with the grants file path", () => {
    render(<PermissionsSummary view={VIEW} />);
    const chat = within(screen.getByRole("region", { name: "Allowed in this chat" }));
    expect(chat.getByText("cargo test")).toBeInTheDocument();
    expect(chat.getByText(/outside the sandbox/)).toBeInTheDocument();
    expect(chat.getByText("/data/chat_grants/t1.json")).toBeInTheDocument();
  });

  it("explains an unavailable sandbox and a blocked network", () => {
    render(<PermissionsSummary view={{ ...VIEW, sandbox: { ...VIEW.sandbox!, state: "unavailable", backend: null, summary: "Not available on this system (Windows)", network: "block" } }} />);
    expect(screen.getByText("Not available")).toBeInTheDocument();
    expect(screen.getByText("Not available on this system (Windows)")).toBeInTheDocument();
    expect(screen.getByText("Blocked")).toBeInTheDocument();
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
