/**
 * Display-only markdown flattening for one-line previews. This is NOT a
 * sanitizer: the result is untrusted text and must still be rendered as a text
 * node (never as HTML or markdown). Input is capped and processed in a single
 * linear pass, so adversarial input cannot cause super-linear work.
 */
export const PLAIN_TEXT_INPUT_LIMIT = 4096;

const LINE_MARKER = /^ {0,3}(?:#{1,6}[ \t]+|>[ \t]?|[-*+][ \t]+|\d{1,9}[.)][ \t]+)/;
const isSpace = (ch: string | undefined): boolean => ch === undefined || /\s/.test(ch);
const isWord = (ch: string | undefined): boolean => ch !== undefined && /[\p{L}\p{N}]/u.test(ch);

function cap(text: string, limit: number): string {
  if (text.length <= limit) return text;
  let end = limit;
  const last = text.charCodeAt(end - 1);
  if (last >= 0xd800 && last <= 0xdbff) end -= 1; // never end on half a surrogate pair
  return text.slice(0, end);
}

function stripInline(line: string): string {
  let out = "";
  // Cached forward searches: each only moves right, so total work stays linear.
  let nextLink = -2;
  let nextClose = -2;
  const find = (needle: string, from: number, cached: number): number =>
    cached === -1 || cached >= from ? cached : line.indexOf(needle, from);
  for (let i = 0; i < line.length; i++) {
    const ch = line[i]!;
    const prev = line[i - 1];
    const next = line[i + 1];
    if (ch === "`") continue;
    if ((ch === "*" || ch === "_" || ch === "~") && next === ch) { i++; continue; }
    if (ch === "*" || ch === "_") {
      const opening = isSpace(prev) || prev === "(";
      const closing = isSpace(next) || (next !== undefined && /[).,;:!?]/.test(next));
      if ((opening && !isSpace(next)) || (closing && !isSpace(prev))) {
        if (ch === "*" || !(isWord(prev) && isWord(next))) continue;
      }
    }
    if (ch === "!" && next === "[") {
      nextLink = find("](", i, nextLink);
      if (nextLink !== -1) continue;
    }
    if (ch === "[") {
      nextLink = find("](", i, nextLink);
      if (nextLink !== -1) continue;
    }
    if (ch === "]" && next === "(") {
      nextClose = find(")", i, nextClose);
      if (nextClose !== -1) { i = nextClose; continue; }
    }
    out += ch;
  }
  return out;
}

/** Flattens markdown to one line of plain text. Display-only; see the module note. */
export function stripMarkdown(text: string, limit = PLAIN_TEXT_INPUT_LIMIT): string {
  const pieces: string[] = [];
  for (const raw of cap(text, limit).split("\n")) {
    if (/^\s*(?:```|~~~)/.test(raw)) continue; // fence lines: keep the code, drop the fence
    pieces.push(stripInline(raw.replace(LINE_MARKER, "")));
  }
  return pieces.join(" ").replace(/\s+/g, " ").trim();
}

/** Plain-text preview of at most `max` characters; cuts on a character boundary and adds an ellipsis. */
export function plainSnippet(text: string, max = 90): string {
  const chars = Array.from(stripMarkdown(text, Math.max(PLAIN_TEXT_INPUT_LIMIT, max * 4)));
  return chars.length > max ? `${chars.slice(0, max - 1).join("").trimEnd()}…` : chars.join("");
}
