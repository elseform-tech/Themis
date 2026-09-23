import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { CodeBlock, tokenizeLine } from "./CodeBlock";

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

  it("tokenizes python comments and rust keywords", () => {
    const py = tokenizeLine("x = 1  # done", "python");
    expect(py.some((t) => t.kind === "comment")).toBe(true);
    const rs = tokenizeLine("fn main() {", "rust");
    expect(
      rs.some((t) => t.kind === "keyword" && t.text === "fn"),
    ).toBe(true);
  });

  it("falls back to plain text for unknown languages", () => {
    const tokens = tokenizeLine("hello world 123", "generic");
    expect(tokens.some((t) => t.kind === "number")).toBe(true);
    expect(tokens.every((t) => t.kind === "plain" || t.kind === "number")).toBe(
      true,
    );
  });
});
