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

// The canvas Chat board's geometry (Chat.dc.html), kept below the app's own top bar (the window title row with the
// sidebar toggle, which the canvas leaves out).
describe("canvas chat geometry", () => {
  const read = (file: string) => readFileSync(new URL(`../${file}`, import.meta.url), "utf8");
  /** Every rule whose selector is exactly this, at a line start, joined. */
  const rule = (file: string, selector: string) => {
    const css = `\n${read(file)}`;
    const found: string[] = [];
    for (let start = css.indexOf(`\n${selector} {`); start >= 0; start = css.indexOf(`\n${selector} {`, start + 1)) found.push(css.slice(start, css.indexOf("}", start)));
    expect(found.length, selector).toBeGreaterThan(0);
    return found.join("\n");
  };
  it.each([
    ["rail: 52px wide, 14px top and bottom, 6px between buttons", "screens/Sidebar.css", ".themis-sidebar-rail", ["flex: 0 0 52px", "gap: 6px", "width: 52px", "padding: 14px 0"]],
    ["rail buttons: 36 by 36", "screens/Sidebar.css", ".themis-sidebar-rail .themis-sidebar-row", ["width: 36px", "min-height: 36px"]],
    ["rail and folded slot: 52px", "screens/Sidebar.css", ".themis-sidebar-slot.is-collapsed", ["width: 52px"]],
    ["sidebar panel: beside the rail, 8px padding, radius 20, 8px top and bottom", "screens/Sidebar.css", ".themis-sidebar", ["inset: var(--themis-panel-gap) 0 var(--themis-panel-gap) 52px", "padding: 8px", "border-radius: var(--themis-panel-radius)"]],
    ["sidebar rows: 34px, padding 0 10px, radius 10, gap 10", "screens/Sidebar.css", ".themis-sidebar-row", ["gap: 10px", "min-height: 34px", "padding: 0 10px", "border-radius: 10px"]],
    ["stage: 8px margin, radius 18", "screens/shell.css", ".themis-main", ["margin: var(--themis-panel-gap)", "border-radius: var(--themis-stage-radius)"]],
    ["chat header: 52px, the model's name and Workspace on the 28px gutter", "screens/ThreadView.css", ".themis-thread-head", ["min-height: 52px", "padding: 0 24px 0 14px"]],
    ["chat column: 700px, 28px gutter", "screens/ThreadView.css", ".themis-thread", ["--themis-thread-gutter: 28px", "--themis-thread-column: 700px"]],
    ["chat items 16px apart", "screens/ThreadView.css", ".themis-thread-stream-content", ["gap: 16px"]],
  ])("%s", (_, file, selector, parts) => {
    const found = rule(file, selector);
    for (const part of parts) expect(found, `${selector} ${part}`).toContain(part);
  });

  it("sizes rail icons at 18px and the sidebar panel at 236px beside the rail by default", () => {
    expect(rule("screens/Sidebar.css", ".themis-sidebar-icon")).toContain("width: 18px; height: 18px");
    expect(read("screens/Sidebar.tsx")).toMatch(/const SIDEBAR_MIN = 288;[\s\S]*const SIDEBAR_DEFAULT = 288;/);
    expect(read("screens/ThreadView.css")).toMatch(/\.themis-thread-composer \{[^}]*margin: 4px auto 18px/);
    expect(read("theme/tokens.css")).toMatch(/--themis-stage-radius: 18px;[\s\S]*--themis-panel-radius: 20px;/);
  });
});
