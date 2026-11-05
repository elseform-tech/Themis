import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

describe("chat rows", () => {
  it("are always laid out, so following the latest message lands on it", () => {
    // `content-visibility: auto` on rows gave off-screen rows estimated heights: sending a message
    // no longer followed to the reply, Latest stopped short, and a chat reopened mid-way.
    const css = readFileSync(new URL("./ThreadView.css", import.meta.url), "utf8");
    expect(css).not.toMatch(/content-visibility/);
  });
});
