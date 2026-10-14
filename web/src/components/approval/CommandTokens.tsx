import type { ReactNode } from "react";

export const MAX_TOKENS = 12;
export const MAX_TOKEN_CHARS = 80;

// Allowlist: only letters, numbers, punctuation and symbols are shown literally (plus a plain space).
// Everything else (controls, format/bidi/zero-width, combining marks, variation selectors, separators,
// private-use, unassigned, tags) is escaped. Blank-looking letters and symbols are denied explicitly.
const PRINTABLE = /^[\p{L}\p{N}\p{P}\p{S}]$/u;
const LOOKS_BLANK = new Set([0x3164, 0x115f, 0x1160, 0xffa0, 0x2800, 0x180e, 0x2800, 0x17b4, 0x17b5, 0x34f]);
// The escape brackets themselves are escaped when typed literally, so an escape always marks a real hidden character.
const BRACKETS = new Set([0x27e8, 0x27e9]);

export interface TokenPart { text: string; escaped: boolean }

const escapeOf = (ch: string): string => `⟨U+${ch.codePointAt(0)!.toString(16).toUpperCase().padStart(4, "0")}⟩`;
function isLiteral(ch: string): boolean {
  const code = ch.codePointAt(0)!;
  if (ch === " ") return true;
  return PRINTABLE.test(ch) && !LOOKS_BLANK.has(code) && !BRACKETS.has(code);
}

/** One argv token as display parts: literal text and visibly distinct escapes; long tokens are cut with a marker. */
export function tokenParts(token: string): TokenPart[] {
  const chars = Array.from(token);
  const parts: TokenPart[] = [];
  const push = (text: string, escaped: boolean) => {
    const last = parts[parts.length - 1];
    if (last && last.escaped === escaped) last.text += text; else parts.push({ text, escaped });
  };
  for (const ch of chars.slice(0, MAX_TOKEN_CHARS)) {
    if (isLiteral(ch)) push(ch, false); else push(escapeOf(ch), true);
  }
  const needsQuotes = token === "" || parts.some(part => part.escaped) || /[\s"'`]/u.test(token);
  if (needsQuotes) {
    for (const part of parts) if (!part.escaped) part.text = part.text.replace(/\\/g, "\\\\").replace(/"/g, '\\"');
    parts.unshift({ text: '"', escaped: false });
    parts.push({ text: '"', escaped: false });
  }
  if (chars.length > MAX_TOKEN_CHARS) parts.push({ text: `…(+${chars.length - MAX_TOKEN_CHARS} chars)`, escaped: false });
  return parts;
}

/** Plain-text form of [`tokenParts`], for tests and non-React callers. */
export const displayToken = (token: string): string => tokenParts(token).map(part => part.text).join("");

export function TokenChip({ token }: { token: string }) {
  return (
    <code className="themis-approval-token">
      {tokenParts(token).map((part, index) => part.escaped
        ? <span key={index} className="themis-approval-escape">{part.text}</span>
        : part.text)}
    </code>
  );
}

/** Argv as separate chips, capped, with an explicit "more" marker. */
export function TokenList({ argv }: { argv: string[] }): ReactNode {
  const shown = argv.slice(0, MAX_TOKENS);
  const hidden = argv.length - shown.length;
  return <>{shown.map((token, index) => <TokenChip key={index} token={token} />)}{hidden > 0 && <span> …(+{hidden} more)</span>}</>;
}
