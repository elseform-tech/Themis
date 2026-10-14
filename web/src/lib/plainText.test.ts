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
