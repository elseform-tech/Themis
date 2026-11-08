import { useState } from "react";
import { SafeText } from "./CommandTokens";
import { commandView, looksDestructive } from "./commandChain";

/** The lead's command, exactly as the agent wrote it: one line (pipes, `&&` and all, never split or wrapped) that
 *  scrolls sideways when long, how many commands a chain holds, a destructive hint, and Copy for the exact text. */
export function CommandPreview({ argv }: { argv: string[] }) {
  const view = commandView(argv);
  const [copied, setCopied] = useState<"" | "Copied" | "Select to copy">("");
  async function copy() {
    try { await navigator.clipboard.writeText(view.copy); setCopied("Copied"); } catch { setCopied("Select to copy"); }
  }
  const destructive = (view.parts ?? [{ words: argv }]).some(part => looksDestructive(part.words));
  return (
    <span className="themis-approval-full">
      <code className="themis-approval-preview" tabIndex={0}><SafeText text={view.copy} max={Infinity} /></code>
      {destructive && <span className="themis-approval-destructive" title="Looks like it discards work (a hint)">destructive</span>}
      <button type="button" className="themis-approval-copy" onClick={() => void copy()}>{copied || "Copy"}</button>
      {view.parts && <span className="themis-approval-count">{view.parts.length} commands</span>}
    </span>
  );
}
