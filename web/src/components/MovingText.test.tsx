import { render, screen } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { MovingText } from "./MovingText";

const css = readFileSync(resolve(process.cwd(), "src/components/MovingText.css"), "utf8");

describe("MovingText", () => {
  it("keeps the whole name readable", () => {
    render(<button type="button"><MovingText>A very long chat title</MovingText></button>);
    expect(screen.getByRole("button").textContent).toBe("A very long chat title");
  });

  it("moves only while pointed at or focused, in the rows that use it", () => {
    expect(css).not.toMatch(/infinite/);
    expect(css).toMatch(/button:hover \.is-overflowing/);
    expect(css).toMatch(/summary:hover \.is-overflowing/);
    expect(css).toMatch(/summary:focus-visible \.is-overflowing/);
    const rules = css.split("}").filter(rule => rule.includes("animation: themis-title"));
    for (const rule of rules) expect(rule).toMatch(/:(hover|focus-visible)/);
  });
});
