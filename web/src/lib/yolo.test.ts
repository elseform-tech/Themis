import { describe, expect, it } from "vitest";
import type { EffectivePermissions } from "./types";
import { networkBlocked, yoloKeeps } from "./yolo";

const view = (sandbox: object, rules: object[] = []): EffectivePermissions =>
  ({ files: [], sandbox: sandbox as never, approval: { default: "ask", default_source: "default", rules: rules as never, project_rule_count: 0 } });

describe("yoloKeeps", () => {
  it("names only the blocks a configuration wrote", () => {
    expect(yoloKeeps(view({ open: true, network: "allow", backend: "Seatbelt", allow_keychain: true }))).toEqual([]);
    expect(yoloKeeps(view({ open: true, network: "block", backend: "Seatbelt", allow_keychain: false },
      [{ tool: "shell", action: "deny", source: "user" }, { tool: "*", action: "allow", source: "default" }, { tool: "git", action: "deny", source: "project" }])))
      .toEqual(["Keychain off", "2 deny rules"]);
    // Off macOS there is no Keychain to mention.
    expect(yoloKeeps(view({ open: true, network: "allow", backend: "bubblewrap", allow_keychain: false }))).toEqual([]);
  });
  it("leaves a blocked network to its own line, in every mode", () => {
    expect(networkBlocked(view({ open: true, network: "block" }))).toBe(true);
    expect(networkBlocked(view({ open: false, network: "block" }))).toBe(true);
    expect(networkBlocked(view({ network: "allow" }))).toBe(false);
    expect(networkBlocked({ files: [], sandbox: null } as never)).toBe(false);
  });
});
