import { useMemo } from "react";
import "./CodeBlock.css";

export interface CodeBlockProps {
  code: string;
  language?: string;
  showLineNumbers?: boolean;
}

type TokenKind = "comment" | "string" | "keyword" | "number" | "plain";

interface Token {
  kind: TokenKind;
  text: string;
}

const KEYWORDS: Record<string, readonly string[]> = {
  ts: [
    "const", "let", "var", "function", "return", "if", "else", "for", "while",
    "class", "interface", "type", "import", "from", "export", "default",
    "extends", "implements", "new", "async", "await", "try", "catch", "throw",
    "switch", "case", "break", "continue", "do", "of", "in", "as", "is",
    "null", "undefined", "true", "false", "this", "void", "enum", "public",
    "private", "readonly", "static",
  ],
  rust: [
    "fn", "let", "mut", "const", "struct", "enum", "impl", "trait", "pub",
    "use", "mod", "return", "if", "else", "for", "while", "loop", "match",
    "where", "type", "as", "in", "ref", "move", "async", "await", "try",
    "true", "false", "self", "Self", "crate", "super", "static", "dyn",
    "break", "continue",
  ],
  python: [
    "def", "class", "return", "if", "elif", "else", "for", "while", "import",
    "from", "as", "with", "try", "except", "raise", "pass", "lambda", "yield",
    "async", "await", "None", "True", "False", "and", "or", "not", "in", "is",
    "break", "continue", "global", "nonlocal", "assert", "del",
  ],
  bash: [
    "if", "then", "else", "elif", "fi", "for", "while", "do", "done", "case",
    "esac", "in", "function", "return", "exit", "export", "local", "readonly",
    "echo", "true", "false",
  ],
};

function normalizeLanguage(language: string | undefined): string {
  const lang = (language ?? "").trim().toLowerCase();
  if (lang === "js" || lang === "javascript" || lang === "jsx") return "ts";
  if (lang === "ts" || lang === "typescript" || lang === "tsx") return "ts";
  if (lang === "rs" || lang === "rust") return "rust";
  if (lang === "py" || lang === "python") return "python";
  if (lang === "sh" || lang === "bash" || lang === "shell") return "bash";
  return "generic";
}

function lineCommentStart(lang: string): string | null {
  if (lang === "python" || lang === "bash") return "#";
  if (lang === "generic") return null;
  return "//";
}

/** Lightweight tokenizer: comments, strings, keywords, numbers. */
export function tokenizeLine(line: string, lang: string): Token[] {
  const tokens: Token[] = [];
  const keywords = new Set(KEYWORDS[lang] ?? []);
  const commentStart = lineCommentStart(lang);
  let i = 0;
  let current = "";

  function flush() {
    if (current !== "") {
      tokens.push({ kind: "plain", text: current });
      current = "";
    }
  }

  while (i < line.length) {
    const ch = line[i] ?? "";
    const rest = line.slice(i);

    // Line comment: rest of the line.
    if (
      commentStart !== null &&
      (commentStart === "//"
        ? rest.startsWith("//")
        : ch === "#")
    ) {
      flush();
      tokens.push({ kind: "comment", text: rest });
      break;
    }

    // Block comment opener on one line (/* ... */); unterminated runs to EOL.
    if (
      (lang === "ts" || lang === "rust" || lang === "generic") &&
      rest.startsWith("/*")
    ) {
      flush();
      const end = line.indexOf("*/", i + 2);
      if (end === -1) {
        tokens.push({ kind: "comment", text: rest });
        break;
      }
      tokens.push({ kind: "comment", text: line.slice(i, end + 2) });
      i = end + 2;
      continue;
    }

    // Strings: '...' "..." `...` (no multiline tracking per line).
    if (ch === '"' || ch === "'" || ch === "`") {
      flush();
      let j = i + 1;
      while (j < line.length) {
        if (line[j] === "\\") {
          j += 2;
          continue;
        }
        if (line[j] === ch) {
          j += 1;
          break;
        }
        j += 1;
      }
      tokens.push({ kind: "string", text: line.slice(i, j) });
      i = j;
      continue;
    }

    // Numbers: digit runs, optionally with . and suffix letters.
    if (/[0-9]/.test(ch)) {
      flush();
      const match = /^[0-9][0-9a-zA-Z_.]*/.exec(rest);
      const text = match?.[0] ?? ch;
      tokens.push({ kind: "number", text });
      i += text.length;
      continue;
    }

    // Words: keyword or plain.
    if (/[A-Za-z_$]/.test(ch)) {
      flush();
      const match = /^[A-Za-z_$][A-Za-z0-9_$]*/.exec(rest);
      const text = match?.[0] ?? ch;
      tokens.push({
        kind: keywords.has(text) ? "keyword" : "plain",
        text,
      });
      i += text.length;
      continue;
    }

    current += ch;
    i += 1;
  }
  flush();
  return tokens;
}

export function CodeBlock({
  code,
  language,
  showLineNumbers = false,
}: CodeBlockProps) {
  const lang = normalizeLanguage(language);
  const lines = useMemo(() => code.split("\n"), [code]);
  const tokenized = useMemo(
    () => lines.map((line) => tokenizeLine(line, lang)),
    [lines, lang],
  );

  return (
    <pre className="themis-codeblock">
      <code>
        {tokenized.map((tokens, index) => (
          <span key={index} className="themis-codeblock-line">
            {showLineNumbers && (
              <span
                aria-hidden="true"
                className="themis-codeblock-lineno"
              >
                {index + 1}
              </span>
            )}
            {tokens.map((token, tokenIndex) =>
              token.kind === "plain" ? (
                <span key={tokenIndex}>{token.text}</span>
              ) : (
                <span
                  key={tokenIndex}
                  className={`themis-codeblock-token--${token.kind}`}
                >
                  {token.text}
                </span>
              ),
            )}
            {index < tokenized.length - 1 ? "\n" : ""}
          </span>
        ))}
      </code>
    </pre>
  );
}
