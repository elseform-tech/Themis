import { useEffect, useState } from "react";
import { getEffectivePermissions } from "./tauri";
import type { EffectivePermissions } from "./types";

/** What still holds in a YOLO chat because a configuration wrote it: YOLO removes
 *  Themis' default walls, never the ones a person or project chose. A blocked network
 *  is not listed here: the shield says it once, under the list, in every mode. */
export function yoloKeeps(view: EffectivePermissions): string[] {
  const keeps: string[] = [];
  const sandbox = view.sandbox;
  // The Keychain only exists on macOS (Seatbelt).
  if (sandbox?.open && sandbox.backend === "Seatbelt" && sandbox.allow_keychain === false) keeps.push("Keychain off");
  const denies = view.approval?.rules.filter(rule => rule.action === "deny").length ?? 0;
  if (denies > 0) keeps.push(denies === 1 ? "1 deny rule" : `${denies} deny rules`);
  return keeps;
}

/** The configuration blocks the network (`sandbox.network: "block"`), which holds in every mode, YOLO included. */
export const networkBlocked = (view: EffectivePermissions): boolean => view.sandbox?.network === "block";

export interface ShieldFacts { keeps: string[]; networkBlocked: boolean }
const NOTHING: ShieldFacts = { keeps: [], networkBlocked: false };

/** What the composer shield says about the chat's effective configuration; refreshed when the chat or its mode changes. */
export function useShieldFacts(projectRoot: string | undefined, threadId: string | undefined, mode: string | undefined): ShieldFacts {
  const [facts, setFacts] = useState<ShieldFacts>(NOTHING);
  useEffect(() => {
    if (projectRoot === undefined || threadId === undefined) { setFacts(NOTHING); return; }
    let current = true;
    getEffectivePermissions(projectRoot, threadId)
      .then(view => { if (current) setFacts({ keeps: mode === "yolo" ? yoloKeeps(view) : [], networkBlocked: networkBlocked(view) }); })
      .catch(() => { if (current) setFacts(NOTHING); });
    return () => { current = false; };
  }, [projectRoot, threadId, mode]);
  return facts;
}
