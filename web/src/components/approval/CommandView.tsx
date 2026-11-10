import { useState } from "react";
import { SafeText } from "./CommandTokens";
import { commandView, looksDestructive } from "./commandChain";

/** The lead's short preview: "…" when cut, and how many commands a chain holds (the whole command is in Details). */
export function CommandPreview({ argv }: { argv: string[] }) {
  const view = commandView(argv);
  return (
    <>
      <code className="themis-approval-preview"><SafeText text={view.preview} max={Infinity} /></code>
      {view.parts && ` (${view.parts.length} commands)`}
    </>
  );
}

/** The whole command in Details, exactly as written: one piece (pipes and `&&` kept, never split into lines),
 *  wrapping and scrolling top to bottom when long, a destructive hint, and Copy for the exact text. A line feed the
 *  command itself holds (a heredoc, a multi-line script) shows as a line break; the two-line preview keeps it escaped. */
export function CommandFull({ argv }: { argv: string[] }) {
  const view = commandView(argv);
  const [copied, setCopied] = useState<"" | "Copied" | "Select to copy">("");
  async function copy() {
    try { await navigator.clipboard.writeText(view.copy); setCopied("Copied"); } catch { setCopied("Select to copy"); }
  }
  const destructive = (view.parts ?? [{ words: argv }]).some(part => looksDestructive(part.words));
  return (
    <div className="themis-approval-full">
      <code className="themis-approval-command" tabIndex={0}><SafeText text={view.copy} max={Infinity} lineBreaks /></code>
      {destructive && <span className="themis-approval-destructive" title="Looks like it discards work (a hint)">destructive</span>}
      <button type="button" className="themis-approval-copy" onClick={() => void copy()}>{copied || "Copy"}</button>
    </div>
  );
}
