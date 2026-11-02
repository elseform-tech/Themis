import { beforeEach, expect, it, vi } from "vitest";
import { markOffSeen, offParts, offSummary, unseenOff } from "./integrations";
import type { Plugin, PluginSpec } from "./types";

const plugin = (name: string, spec: Partial<PluginSpec>, enabled = true): Plugin => ({
  scope: "global", revision: "1", enabled, source: "official",
  spec: { name, description: "", version: "", skills: [], mcp: {}, hooks: [], files: {}, unsupported: [], ...spec },
});
const hook = (name: string, enabled: boolean) => ({ name, event: "SessionStart", command: "true", enabled }) as PluginSpec["hooks"][number];
const server = (enabled: boolean) => ({ args: [], enabled }) as unknown as PluginSpec["mcp"][string];

beforeEach(() => {
  const saved = new Map<string, string>();
  vi.stubGlobal("localStorage", { getItem: (key: string) => saved.get(key) ?? null, setItem: (key: string, value: string) => { saved.set(key, value); } });
});

it("lists the hooks and connections an installed plugin has off", () => {
  const parts = offParts(plugin("kit", { hooks: [hook("a", false), hook("b", true)], mcp: { c: server(false), d: server(true) } }));
  expect(parts).toEqual([{ kind: "mcp", id: "c" }, { kind: "hook", id: "a" }]);
  expect(offSummary(parts)).toBe("1 connection and 1 hook");
});

it("counts in plain words", () => {
  expect(offSummary([{ kind: "hook", id: "a" }, { kind: "hook", id: "b" }])).toBe("2 hooks");
  expect(offSummary([])).toBe("");
});

it("a plugin turned off as a whole has no parts to point out", () => {
  expect(offParts(plugin("kit", { hooks: [hook("a", false)] }, false))).toEqual([]);
});

it("the sidebar's mark stays until the Installed list has been seen, and returns for something new", () => {
  const one = plugin("kit", { hooks: [hook("a", false)] });
  expect(unseenOff([one])).toBe(true);
  markOffSeen([one]);
  expect(unseenOff([one])).toBe(false);
  expect(unseenOff([one, plugin("other", { hooks: [hook("b", false)] })])).toBe(true);
});
