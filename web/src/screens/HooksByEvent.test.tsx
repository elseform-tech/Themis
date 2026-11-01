// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Plugin } from "../lib/types";
import { toolDisplayName } from "../lib/toolNames";
import { HooksByEvent, hookMeta, skipReason } from "./HooksByEvent";

afterEach(cleanup);

const plugin = (hooks: Plugin["spec"]["hooks"], unsupported: string[] = []): Plugin => ({
  scope: "global", enabled: true, revision: "r",
  spec: { name: "guards", description: "", version: "", skills: [], mcp: {}, hooks, files: {}, unsupported },
} as unknown as Plugin);
const hook = (name: string, event: string, extra: Partial<Plugin["spec"]["hooks"][number]> = {}) =>
  ({ name, event, command: "x", enabled: true, timeout_seconds: 10, blocking: false, ...extra });

it("names hooks by plugin and hook, never by their wire id", () => {
  expect(toolDisplayName("hook_g--ticketing--18e35169be252a40--before-shell_f6bb584aa0123456")).toBe("Hook · ticketing · before-shell");
  expect(toolDisplayName("hook_g--ticketing--18e35169be252a40--before-shell")).toBe("Hook · ticketing · before-shell");
  expect(toolDisplayName("hook_guard_f6bb584aa0123456")).toBe("Hook · guard");
});

describe("Hooks by event", () => {
  it("lists every event in run order, dims empty ones, and says what each hook covers", () => {
    const guards = plugin([
      hook("guard", "BeforeTool", { matcher: "shell|background_task", blocking: true }),
      hook("on-failure", "ToolCallFailed"),
      hook("ticket-context", "PromptSubmit"),
    ], ["Hook event SessionEnd", "PreToolUse: matcher NotebookEdit has no Themis tool", "Plugin has an LSP server"]);
    const rows = guards.spec.hooks.map(item => ({ key: item.name, id: item.name, plugin: guards, enabled: true }));
    const toggle = vi.fn();
    render(<HooksByEvent rows={rows} plugins={[guards]} sourceOf={() => "guards"} disabled={() => false} onToggle={toggle} onOpen={() => {}} />);
    const events = within(screen.getByRole("list", { name: "Hook events in run order" })).getAllByRole("listitem");
    expect(events.map(item => item.querySelector(".themis-hook-title")!.textContent)).toEqual([
      "Prompt submitted", "Run starts", "Before a tool runs", "After a tool runs", "Subagent starts", "Subagent ends", "Before context is compacted", "Turn would end", "Run ends",
    ]);
    expect(events[1]).toHaveClass("is-empty");
    expect(events[1]).toHaveTextContent("None");
    expect(events[0]).toHaveTextContent("ticket-context");
    expect(events[2]).toHaveTextContent("guards · shell, background_task · Main and subagents · Blocks on failure");
    expect(events[3]).toHaveTextContent("Only when it fails");
    // Skipped hooks: one plain line per plugin; other notes stay out.
    expect(screen.getByRole("note")).toHaveTextContent("2 hooks skipped (guards): unsupported event (SessionEnd); matcher NotebookEdit has no Themis tool.");
    fireEvent.click(screen.getByRole("switch", { name: "guard enabled" }));
    expect(toggle).toHaveBeenCalledWith(rows[0]);
    // The screen clips everything but this list, so events and skipped lines must scroll in it.
    const scroller = document.querySelector(".themis-integrations-list")!;
    expect(scroller).toContainElement(screen.getByRole("list", { name: "Hook events in run order" }));
    expect(scroller).toContainElement(screen.getByRole("note"));
  });

  it("collapses an event with hooks; an empty event has nothing to collapse", () => {
    const guards = plugin([hook("guard", "BeforeTool"), hook("ticket-context", "PromptSubmit")]);
    const rows = guards.spec.hooks.map(item => ({ key: item.name, id: item.name, plugin: guards, enabled: true }));
    render(<HooksByEvent rows={rows} plugins={[guards]} sourceOf={() => "guards"} disabled={() => false} onToggle={() => {}} onOpen={() => {}} />);
    const before = screen.getByRole("button", { name: /Before a tool runs/ });
    expect(before).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(before);
    expect(before).toHaveAttribute("aria-expanded", "false");
    expect(before).toHaveTextContent("1 hook");
    expect(screen.queryByRole("switch", { name: "guard enabled" })).toBeNull();
    expect(screen.getByRole("switch", { name: "ticket-context enabled" })).toBeInTheDocument();
    fireEvent.click(before);
    expect(screen.getByRole("switch", { name: "guard enabled" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Run starts/ })).toBeNull();
  });

  it("words import notes plainly and ignores ones that are not about hooks", () => {
    expect(skipReason("Stop: asyncRewake needs a way to wake the agent")).toBe("asyncRewake needs a way to wake the agent");
    expect(skipReason("SessionStart: lifecycle matcher requires manual mapping")).toBe("SessionStart matcher not supported");
    expect(skipReason("Plugin has an LSP server")).toBeNull();
    const guards = plugin([hook("guard", "BeforeTool")]);
    expect(hookMeta({ key: "k", id: "guard", plugin: guards, enabled: true }, "guards")).toBe("guards · Main and subagents");
  });
});
