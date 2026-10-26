import type { EffectivePermissions } from "./types";

/** What still holds in a YOLO chat because a configuration wrote it: YOLO removes
 *  Themis' default walls, never the ones a person or project chose. */
export function yoloKeeps(view: EffectivePermissions): string[] {
  const keeps: string[] = [];
  const sandbox = view.sandbox;
  if (sandbox?.open && sandbox.network === "block") keeps.push("network blocked");
  // The Keychain only exists on macOS (Seatbelt).
  if (sandbox?.open && sandbox.backend === "Seatbelt" && sandbox.allow_keychain === false) keeps.push("Keychain off");
  const denies = view.approval?.rules.filter(rule => rule.action === "deny").length ?? 0;
  if (denies > 0) keeps.push(denies === 1 ? "1 deny rule" : `${denies} deny rules`);
  return keeps;
}
