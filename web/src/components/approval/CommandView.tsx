import { useState } from "react";
import { SafeText, TokenList } from "./CommandTokens";
import { commandView, looksDestructive, type CommandPart } from "./commandChain";

/** The lead's short preview: at most two lines, "…" when cut, and how many commands a chain holds. */
export function CommandPreview({ argv }: { argv: string[] }) {
  const view = commandView(argv);
  return (
    <>
      <code className="themis-approval-preview"><SafeText text={view.preview} max={Infinity} /></code>
      {view.parts && ` (${view.parts.length} commands)`}
    </>
  );
}

function DestructiveTag({ words }: { words: string[] }) {
  return looksDestructive(words) ? <span className="themis-approval-destructive" title="Looks like it discards work (a hint)">destructive</span> : null;
}

function ChainList({ parts }: { parts: CommandPart[] }) {
  return (
    <ol className="themis-approval-chain">
      {parts.map((part, index) => <li key={index}><code><SafeText text={part.text} max={Infinity} /></code><DestructiveTag words={part.words} /></li>)}
    </ol>
  );
}

/** The whole command in Details: never cut, wrapped and scrolling when very long, one command per line for a chain,
 *  a script it cannot split as written, and Copy for the exact command. */
export function CommandFull({ argv }: { argv: string[] }) {
  const view = commandView(argv);
  const [copied, setCopied] = useState<"" | "Copied" | "Select to copy">("");
  async function copy() {
    try { await navigator.clipboard.writeText(view.copy); setCopied("Copied"); } catch { setCopied("Select to copy"); }
  }
  let body = <div className="themis-approval-command"><code><TokenList argv={argv} full /></code><DestructiveTag words={argv} /></div>;
  if (view.parts) body = <ChainList parts={view.parts} />;
  else if (view.script !== null) body = <pre className="themis-approval-command"><SafeText text={view.script} max={Infinity} /></pre>;
  return (
    <div className="themis-approval-full">
      {body}
      <button type="button" className="themis-approval-copy" onClick={() => void copy()}>{copied || "Copy"}</button>
    </div>
  );
}

/** Details opens by itself when the lead could not show the whole command. */
export const commandNeedsDetails = (argv: string[]): boolean => {
  const view = commandView(argv);
  return view.cut || view.parts !== null || view.script !== null;
};
