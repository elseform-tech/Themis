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
export function tokenParts(token: string, max = MAX_TOKEN_CHARS): TokenPart[] {
  const chars = Array.from(token);
  const parts: TokenPart[] = [];
  const push = (text: string, escaped: boolean) => {
    const last = parts[parts.length - 1];
    if (last && last.escaped === escaped) last.text += text; else parts.push({ text, escaped });
  };
  for (const ch of chars.slice(0, max)) {
    if (isLiteral(ch)) push(ch, false); else push(escapeOf(ch), true);
  }
  const needsQuotes = token === "" || parts.some(part => part.escaped) || /[\s"'`]/u.test(token);
  if (needsQuotes) {
    for (const part of parts) if (!part.escaped) part.text = part.text.replace(/\\/g, "\\\\").replace(/"/g, '\\"');
    parts.unshift({ text: '"', escaped: false });
    parts.push({ text: '"', escaped: false });
  }
  if (chars.length > max) parts.push({ text: `…(+${chars.length - max} chars)`, escaped: false });
  return parts;
}

/** Plain-text form of [`tokenParts`], for tests and non-React callers. */
export const displayToken = (token: string, max = MAX_TOKEN_CHARS): string => tokenParts(token, max).map(part => part.text).join("");

/** One token as a chip. `full` never cuts it; otherwise a cut token shows its whole (escaped) text on hover. */
export function TokenChip({ token, full = false }: { token: string; full?: boolean }) {
  const max = full ? Infinity : MAX_TOKEN_CHARS;
  const cut = Array.from(token).length > max;
  return (
    <code className="themis-approval-token" title={cut ? displayToken(token, Infinity) : undefined}>
      {tokenParts(token, max).map((part, index) => part.escaped
        ? <span key={index} className="themis-approval-escape">{part.text}</span>
        : part.text)}
    </code>
  );
}

/** Argv as separate chips, capped, with an explicit "more" marker that names the rest on hover. `full` shows every
 *  token whole, separated by spaces, so a command can be read (and wraps) end to end. */
export function TokenList({ argv, full = false }: { argv: string[]; full?: boolean }): ReactNode {
  if (full) return <>{argv.map((token, index) => <span key={index}>{index > 0 && " "}<TokenChip token={token} full /></span>)}</>;
  const shown = argv.slice(0, MAX_TOKENS);
  const rest = argv.slice(MAX_TOKENS);
  return <>{shown.map((token, index) => <TokenChip key={index} token={token} />)}{rest.length > 0 && <span title={rest.map(token => displayToken(token, Infinity)).join(" ")}> …(+{rest.length} more)</span>}</>;
}

/** Which line feeds of `text` may show as real line breaks: only one between two lines that each show something
 *  other than spaces, so a break never makes a blank-looking line that could push the rest of a command out of sight.
 *  Every other line feed (doubled, leading, trailing, around a spaces-only line) stays an escape. */
function realBreaks(text: string): boolean[] {
  const lines = text.split("\n");
  const shows = (line: string) => /[^ ]/u.test(line);
  return lines.slice(1).map((line, index) => shows(lines[index]!) && shows(line));
}

/** Untrusted plain text (for example a path from program output) with hidden characters escaped like tokens, unquoted
 *  and capped. With `lineBreaks`, a lone line feed between two non-blank lines shows as a line break instead of an escape. */
export function SafeText({ text, max = 300, lineBreaks = false }: { text: string; max?: number; lineBreaks?: boolean }) {
  const chars = Array.from(text);
  const shown = chars.slice(0, max);
  const breaks = lineBreaks ? realBreaks(shown.join("")) : [];
  let feed = 0;
  const parts: (TokenPart | "break")[] = [];
  for (const ch of shown) {
    if (ch === "\n" && breaks[feed++]) { parts.push("break"); continue; }
    const literal = isLiteral(ch);
    const piece = literal ? ch : escapeOf(ch);
    const last = parts[parts.length - 1];
    if (last && last !== "break" && last.escaped === !literal) last.text += piece; else parts.push({ text: piece, escaped: !literal });
  }
  return <>{parts.map((part, index) => part === "break" ? <br key={index} /> : part.escaped ? <span key={index} className="themis-approval-escape">{part.text}</span> : part.text)}{chars.length > max && `…(+${chars.length - max} chars)`}</>;
}
