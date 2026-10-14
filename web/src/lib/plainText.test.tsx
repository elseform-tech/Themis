// @vitest-environment jsdom
import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { plainSnippet, stripMarkdown } from "./plainText";

describe("stripMarkdown", () => {
  it("removes emphasis, code ticks, headings, lists and links", () => {
    expect(stripMarkdown("**DONE** - `status.txt` says [ok](http://x.test)")).toBe("DONE - status.txt says ok");
    expect(stripMarkdown("# Title\n- one\n- *two*\n> quote")).toBe("Title one two quote");
    expect(stripMarkdown("```sh\ncargo test\n```\nafter")).toBe("cargo test after");
  });
  it("keeps snake_case and lone asterisks", () => {
    expect(stripMarkdown("see my_file_name and 2 * 3")).toBe("see my_file_name and 2 * 3");
  });
});

describe("plainSnippet", () => {
  it("truncates cleanly after stripping, never inside markup or an emoji", () => {
    const out = plainSnippet(`**${"a".repeat(20)}** ${"😀".repeat(100)}`, 30);
    expect(Array.from(out)).toHaveLength(30);
    expect(out.endsWith("…")).toBe(true);
    expect(out).not.toContain("*");
    expect(out).not.toMatch(/[\ud800-\udbff](?![\udc00-\udfff])/);
  });
  it("leaves short text alone", () => {
    expect(plainSnippet("`ok`")).toBe("ok");
  });
});

describe("adversarial input", () => {
  const big = 200_000;
  const cases: Record<string, string> = {
    fences: "```\n".repeat(big / 4),
    bold: "**".repeat(big / 2),
    brackets: "[".repeat(big),
    bracketsAndParens: "[a](".repeat(big / 4),
    ticks: "`".repeat(big),
    underscores: "_a ".repeat(big / 3),
    images: "![".repeat(big / 2),
  };
  for (const [name, input] of Object.entries(cases)) {
    it(`handles 200 KB of ${name} quickly`, () => {
      const start = performance.now();
      const out = plainSnippet(input);
      stripMarkdown(input);
      expect(performance.now() - start).toBeLessThan(250);
      expect(Array.from(out).length).toBeLessThanOrEqual(90);
    });
  }
  it("scales linearly when the cap is lifted", () => {
    const time = (n: number, input: string) => Math.min(...[0, 1, 2].map(() => { const t = performance.now(); stripMarkdown(input.repeat(n), 1e9); return performance.now() - t; }));
    for (const unit of ["[", "![a](", "**_", "`"]) {
      const small = Math.max(time(10_000, unit), 1);
      // Four times the input: linear is about 4x, quadratic about 16x; allow generous CI noise.
      expect(time(40_000, unit)).toBeLessThan(small * 12 + 50);
    }
  });
  it("caps the input it processes", () => {
    expect(stripMarkdown("a".repeat(100_000)).length).toBe(4096);
  });
  it("returns markup-looking text unchanged as data, for text-node rendering only", () => {
    const html = "<img src=x onerror=alert(1)>";
    expect(stripMarkdown(html)).toBe(html);
    const { container } = render(<p>{plainSnippet(`**${html}**`)}</p>);
    expect(container.querySelector("img")).toBeNull();
    expect(container.textContent).toBe(html);
  });
});
