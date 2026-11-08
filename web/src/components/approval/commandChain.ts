/** Reading a command the agent wants to run: the exact text to show and copy, a short preview, and, for a chain, its parts.
 *  Everything here is untrusted input (model output): it only splits and labels, and the UI renders it escaped. */

/** One command of a chain: its text as written, and its words for the destructive hint. */
export interface CommandPart { text: string; words: string[] }

export interface CommandView {
  /** The exact command, shell-quoted: shown whole in Details and copied. */
  copy: string;
  /** At most `PREVIEW_CHARS` characters of `copy`, ending with "…" when cut. */
  preview: string;
  /** The parts of a chain (two or more), for the count and the destructive hint, or null for a single command. */
  parts: CommandPart[] | null;
}

export const PREVIEW_CHARS = 160;
const SAFE_WORD = /^[A-Za-z0-9_@%+=:,./-]+$/;
const CHAIN_TOKENS = new Set(["&&", "||", ";", "|"]);
const SHELLS = /^(?:.*\/)?(?:sh|bash|zsh|dash)$/;

/** POSIX shell quoting: plain words stay as they are, anything else goes in single quotes. */
export function shellJoin(argv: string[]): string {
  return argv.map(word => word !== "" && SAFE_WORD.test(word) ? word : `'${word.replace(/'/g, "'\\''")}'`).join(" ");
}

interface Lexer { parts: CommandPart[]; words: string[]; word: string; inWord: boolean; start: number; quote: string | null }

function endWord(lex: Lexer) {
  if (lex.inWord) lex.words.push(lex.word);
  lex.word = "";
  lex.inWord = false;
}

function endPart(lex: Lexer, script: string, end: number) {
  endWord(lex);
  const text = script.slice(lex.start, end).trim();
  if (text) lex.parts.push({ text, words: lex.words });
  lex.words = [];
}

/** One character inside quotes; -1 when a substitution makes the split unsure. */
function inQuote(lex: Lexer, script: string, i: number): number {
  const ch = script[i];
  if (ch === lex.quote) { lex.quote = null; return i + 1; }
  if (lex.quote === '"' && (ch === "`" || (ch === "$" && script[i + 1] === "("))) return -1;
  if (lex.quote === '"' && ch === "\\") { lex.word += script[i + 1] ?? ""; return i + 2; }
  lex.word += ch;
  return i + 1;
}

const OPERATOR = /&&|\|\||[;|\n]/y;
/** Subshells, substitutions, heredocs and background jobs: not split, shown as written. */
const UNSURE = /[`()&]|\$\(|<</y;

/** One step of the reader; returns the next index, or -1 when it is unsure. */
function step(lex: Lexer, script: string, i: number): number {
  const ch = script[i];
  if (lex.quote) return inQuote(lex, script, i);
  if (ch === "'" || ch === '"') { lex.quote = ch; lex.inWord = true; return i + 1; }
  if (ch === "\\") { lex.word += script[i + 1] ?? ""; lex.inWord = true; return i + 2; }
  OPERATOR.lastIndex = i;
  const op = OPERATOR.exec(script);
  if (op) { endPart(lex, script, i); lex.start = i + op[0].length; return lex.start; }
  UNSURE.lastIndex = i;
  if (UNSURE.test(script)) return -1;
  if (/\s/.test(ch)) { endWord(lex); return i + 1; }
  lex.word += ch;
  lex.inWord = true;
  return i + 1;
}

/** Splits a shell script at top-level `&&`, `||`, `;`, `|` and newlines, respecting quotes; null when unsure. */
export function splitScript(script: string): CommandPart[] | null {
  const lex: Lexer = { parts: [], words: [], word: "", inWord: false, start: 0, quote: null };
  for (let i = 0; i < script.length;) {
    i = step(lex, script, i);
    if (i < 0) return null;
  }
  if (lex.quote) return null;
  endPart(lex, script, script.length);
  return lex.parts;
}

/** An argv that carries chain operators as their own tokens, split into its commands. */
function splitArgv(argv: string[]): CommandPart[] {
  const groups: string[][] = [[]];
  for (const word of argv) {
    if (CHAIN_TOKENS.has(word)) groups.push([]); else groups[groups.length - 1].push(word);
  }
  return groups.filter(group => group.length > 0).map(words => ({ text: shellJoin(words), words }));
}

export function commandView(argv: string[]): CommandView {
  const copy = shellJoin(argv);
  const isScript = argv.length >= 3 && SHELLS.test(argv[0]) && /^-[a-z]*c$/.test(argv[1]);
  const parts = isScript ? splitScript(argv[2]) : splitArgv(argv);
  const chars = Array.from(copy);
  const preview = chars.length > PREVIEW_CHARS ? `${chars.slice(0, PREVIEW_CHARS).join("")}…` : copy;
  return { copy, preview, parts: parts && parts.length > 1 ? parts : null };
}

const RM_FLAG = /^-[a-zA-Z]*[rRf]/;
const PUBLISH = new Set(["publish", "unpublish", "deprecate"]);
const GIT_VALUE_OPTIONS = new Set(["-C", "-c", "--git-dir", "--work-tree", "--namespace"]);

type Has = (longs: string[], shorts: string) => boolean;
/** Git subcommands that discard work or rewrite history, mirroring the runtime's classifier in spirit. */
const GIT: Record<string, (has: Has, rest: string[]) => boolean> = {
  push: has => has(["force", "force-with-lease", "force-if-includes", "mirror", "delete", "prune"], "fd"),
  clean: has => has(["force"], "fdxX"),
  reset: has => has(["hard", "merge"], ""),
  checkout: (has, rest) => has(["force", "discard-changes"], "f") || rest.includes("."),
  switch: has => has(["force", "discard-changes"], "f"),
  restore: () => true,
  branch: has => has(["force"], "DfMC"),
  tag: has => has(["delete", "force"], "df"),
  commit: has => has(["amend"], ""),
  stash: (_, rest) => rest[0] === "drop" || rest[0] === "clear",
  rebase: () => true,
  "filter-branch": () => true,
  "update-ref": () => true,
};

function gitDestructive(args: string[]): boolean {
  let i = 0;
  while (i < args.length && args[i].startsWith("-")) i += GIT_VALUE_OPTIONS.has(args[i]) ? 2 : 1;
  const [sub, ...rest] = args.slice(i);
  const has: Has = (longs, shorts) => rest.some(arg => arg.startsWith("--")
    ? longs.includes(arg.slice(2).split("=")[0])
    : arg.startsWith("-") && Array.from(arg.slice(1)).some(ch => shorts.includes(ch)));
  return GIT[sub]?.(has, rest) ?? false;
}

/** A hint, not the runtime's decision: does this command look like it discards work? */
export function looksDestructive(words: string[]): boolean {
  let i = 0;
  while (i < words.length && (/^[A-Za-z_][A-Za-z0-9_]*=/.test(words[i]) || words[i] === "sudo" || words[i] === "env")) i += 1;
  const [program, ...rest] = words.slice(i);
  const name = program?.split("/").pop();
  if (name === "rm") return rest.some(arg => RM_FLAG.test(arg) || arg === "--recursive" || arg === "--force");
  if (name === "find") return rest.includes("-delete");
  if (name === "git") return gitDestructive(rest);
  if (name === "npm" || name === "pnpm" || name === "yarn" || name === "cargo") return PUBLISH.has(rest.find(arg => !arg.startsWith("-")) ?? "");
  return false;
}
