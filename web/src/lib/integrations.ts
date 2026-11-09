import type { Plugin } from "./types";
import { readSession, writeSession } from "../state/session";

export function isPluginPackage(plugin: Plugin): boolean {
  return plugin.source !== "discovered" && plugin.spec.origin?.kind !== "discovered" && !isOwn(plugin)
    && (plugin.spec.package_kind == null || plugin.spec.package_kind === "plugin");
}

/** The person's own container (`personal`, or one they named): skills, servers and hooks they or the
 *  agent saved, with no package, catalog or import behind it. It is a holder, not a plugin to show. */
export function isOwn(plugin: Plugin): boolean {
  const origin = plugin.spec.origin?.kind;
  return !plugin.source && (origin == null || origin === "mcp-json");
}

/** Where an item comes from, in a word: "Yours" (the `personal` holder; "Yours · NAME" for one the
 *  person named), "Project" for a skill folder in the repository, "Discovered" for one in a personal
 *  skill folder, else the plugin's name. */
export function itemSource(plugin: Plugin, pluginName: string): string {
  if (plugin.source === "discovered" || plugin.spec.origin?.kind === "discovered") return plugin.scope === "local" ? "Project" : "Discovered";
  if (isOwn(plugin)) return plugin.spec.name === "personal" ? "Yours" : `Yours · ${plugin.spec.name}`;
  return pluginName;
}

export interface OffPart { kind: "mcp" | "hook"; id: string }

/** What an installed plugin keeps off: connections and hooks don't start on, and without them many
 *  plugins do little. A plugin turned off as a whole has nothing to point out. */
export function offParts(plugin: Plugin): OffPart[] {
  if (!plugin.enabled) return [];
  return [
    ...Object.entries(plugin.spec.mcp).filter(([, server]) => !server.enabled).map(([id]) => ({ kind: "mcp" as const, id })),
    ...plugin.spec.hooks.filter(hook => !hook.enabled).map(hook => ({ kind: "hook" as const, id: hook.name })),
  ];
}

/** "1 connection and 2 hooks". */
export function offSummary(parts: OffPart[]): string {
  const count = (kind: OffPart["kind"], label: string) => {
    const n = parts.filter(part => part.kind === kind).length;
    return n ? `${n} ${label}${n === 1 ? "" : "s"}` : "";
  };
  return [count("mcp", "connection"), count("hook", "hook")].filter(Boolean).join(" and ");
}

const offKey = (plugin: Plugin, part: OffPart) => `${plugin.scope}:${plugin.spec.name}:${part.kind}:${part.id}`;
const allOff = (plugins: Plugin[]) => plugins.flatMap(plugin => offParts(plugin).map(part => offKey(plugin, part)));

/** Whether something is off that the person hasn't seen in the Installed list yet. Choosing to
 *  keep a part off shouldn't leave a mark on the sidebar for ever. */
export function unseenOff(plugins: Plugin[]): boolean {
  const seen = new Set(readSession<string[]>("seenOff", []));
  return allOff(plugins).some(key => !seen.has(key));
}

export function markOffSeen(plugins: Plugin[]): void {
  writeSession("seenOff", allOff(plugins));
}
