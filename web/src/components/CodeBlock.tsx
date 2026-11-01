import { useMemo } from "react";
import { common, createLowlight } from "lowlight";
import "./CodeBlock.css";

export interface CodeBlockProps {
  code: string;
  language?: string;
  showLineNumbers?: boolean;
}

type TokenKind = "comment" | "string" | "keyword" | "number" | "title" | "type" | "meta" | "plain";

export interface Token {
  kind: TokenKind;
  text: string;
}

const lowlight = createLowlight(common);

/** Past this much text a block isn't coloured, so a huge file stays quick to open. */
const MAX_HIGHLIGHTED_CHARS = 200_000;

const KINDS: Record<string, TokenKind> = {
  comment: "comment", doctag: "comment", quote: "comment",
  string: "string", regexp: "string", symbol: "string",
  keyword: "keyword", literal: "keyword", "selector-tag": "keyword",
  number: "number",
  title: "title", section: "title", name: "title",
  type: "type", built_in: "type", class: "type",
  meta: "meta", attr: "meta", attribute: "meta", variable: "meta", params: "plain", subst: "plain",
};

function kindOf(classes: readonly unknown[] | undefined, inherited: TokenKind): TokenKind {
  for (const name of classes ?? []) {
    const known = KINDS[String(name).replace(/^hljs-/, "").replace(/_$/, "")];
    if (known) return known;
  }
  return inherited;
}

interface Tree {
  type: string;
  value?: string;
  children?: Tree[];
  properties?: { className?: readonly unknown[] };
}

/** The highlighter's tree as flat tokens, one list per line. */
function linesOf(root: Tree): Token[][] {
  const lines: Token[][] = [[]];
  const walk = (node: Tree, kind: TokenKind) => {
    if (node.type === "text") {
      (node.value ?? "").split("\n").forEach((part, index) => {
        if (index > 0) lines.push([]);
        if (part) lines[lines.length - 1]!.push({ kind, text: part });
      });
      return;
    }
    const own = node.type === "element" ? kindOf(node.properties?.className, kind) : kind;
    for (const child of node.children ?? []) walk(child, own);
  };
  walk(root, "plain");
  return lines;
}

/** The code's coloured tokens, line by line; plain for a language it doesn't know. The tree is
 *  turned into text nodes rather than HTML, so code can never become markup. */
export function highlightLines(code: string, language: string | undefined): Token[][] {
  const name = (language ?? "").trim().toLowerCase();
  if (!name || code.length > MAX_HIGHLIGHTED_CHARS || !lowlight.registered(name)) {
    return code.split("\n").map(line => (line ? [{ kind: "plain" as const, text: line }] : []));
  }
  return linesOf(lowlight.highlight(name, code) as unknown as Tree);
}

export function CodeBlock({
  code,
  language,
  showLineNumbers = false,
}: CodeBlockProps) {
  const tokenized = useMemo(() => highlightLines(code, language), [code, language]);

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
