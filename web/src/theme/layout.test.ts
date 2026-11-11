import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const css = readFileSync(new URL("./tokens.css", import.meta.url), "utf8");

describe("framed layout tokens", () => {
  it.each([
    ["--themis-panel-gap", "8px"],
    ["--themis-stage-radius", "18px"],
    ["--themis-panel-radius", "20px"],
    ["--themis-panel-bg", "var(--themis-sidebar-bg)"],
    ["--themis-stage-bg", "var(--themis-canvas)"],
    ["--themis-panel-blur", "none"],
  ])("defines %s as %s", (name, value) => {
    expect(new RegExp(`${name}\\s*:\\s*${value.replace(/[()]/g, "\\$&")}\\s*;`).test(css)).toBe(true);
  });
});
