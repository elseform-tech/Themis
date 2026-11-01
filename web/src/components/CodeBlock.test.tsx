import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { CodeBlock, highlightLines } from "./CodeBlock";

describe("CodeBlock", () => {
  it("renders code text", () => {
    render(<CodeBlock code="const x = 1;" language="ts" />);
    expect(screen.getByText("const", { exact: true })).toBeInTheDocument();
  });

  it("shows line numbers when enabled", () => {
    const { container } = render(
      <CodeBlock code={"a\nb\nc"} showLineNumbers />,
    );
    const numbers = container.querySelectorAll(".themis-codeblock-lineno");
    expect(numbers).toHaveLength(3);
    expect(numbers[0]?.textContent).toBe("1");
    expect(numbers[2]?.textContent).toBe("3");
  });

  it("hides line numbers by default", () => {
    const { container } = render(<CodeBlock code={"a\nb"} />);
    expect(
      container.querySelectorAll(".themis-codeblock-lineno"),
    ).toHaveLength(0);
  });

  it("highlights keywords, strings, comments, and numbers", () => {
    const { container } = render(
      <CodeBlock
        code={'const s = "hi"; // greet\nlet n = 42;'}
        language="ts"
      />,
    );
    const kinds = [...container.querySelectorAll("span[class*='token--']")].map(
      (el) => el.className,
    );
    expect(kinds).toContain("themis-codeblock-token--keyword");
    expect(kinds).toContain("themis-codeblock-token--string");
    expect(kinds).toContain("themis-codeblock-token--comment");
    expect(kinds).toContain("themis-codeblock-token--number");
  });

  it("colours what makes Python read as Python", () => {
    const code = [
      "@dataclass",
      "class Person:",
      '    """A docstring',
      '    on two lines."""',
      "    name: str = 'x'",
      "",
      "def greet(person: Person) -> str:",
      "    # a comment",
      '    return f"Hello {person.name} {1 + 2}"',
    ].join("\n");
    const lines = highlightLines(code, "python");
    expect(lines).toHaveLength(9);
    const has = (line: number, kind: string, text: string) =>
      lines[line]!.some(token => token.kind === kind && token.text.includes(text));
    expect(has(0, "meta", "@dataclass")).toBe(true);
    expect(has(1, "keyword", "class")).toBe(true);
    expect(has(1, "title", "Person")).toBe(true);
    // A docstring stays a string across lines, which a line at a time couldn't do.
    expect(has(2, "string", "A docstring")).toBe(true);
    expect(has(3, "string", "on two lines")).toBe(true);
    expect(has(6, "title", "greet")).toBe(true);
    expect(has(6, "type", "str")).toBe(true);
    expect(has(7, "comment", "a comment")).toBe(true);
    expect(has(8, "string", "Hello")).toBe(true);
    expect(lines.map(line => line.map(token => token.text).join("")).join("\n")).toBe(code);
  });

  it("knows the short names and other languages", () => {
    expect(highlightLines("x = 1  # done", "py")[0]!.some(t => t.kind === "comment")).toBe(true);
    expect(highlightLines("fn main() {", "rust")[0]!.some(t => t.kind === "keyword" && t.text === "fn")).toBe(true);
  });

  it("leaves a language it doesn't know, or a huge block, plain", () => {
    expect(highlightLines("hello 123", "not-a-language")).toEqual([[{ kind: "plain", text: "hello 123" }]]);
    expect(highlightLines("hello 123", undefined)).toEqual([[{ kind: "plain", text: "hello 123" }]]);
    const huge = "x = 1\n".repeat(50_000);
    expect(highlightLines(huge, "python").every(line => line.every(token => token.kind === "plain"))).toBe(true);
  });

  it("renders code as text, never as markup", () => {
    const { container } = render(<CodeBlock code={'x = "<img src=x onerror=alert(1)>"'} language="python" />);
    expect(container.querySelector("img")).toBeNull();
    expect(container.textContent).toContain("<img src=x");
  });
});
