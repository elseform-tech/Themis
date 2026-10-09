import type { Plugin } from "./types";

export function isPluginPackage(plugin: Plugin): boolean {
  return plugin.source !== "discovered" && plugin.spec.origin?.kind !== "discovered"
    && (plugin.spec.package_kind == null || plugin.spec.package_kind === "plugin");
}
