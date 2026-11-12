// @vitest-environment node
import { readFileSync } from "node:fs";
import { expect, it } from "vitest";

// The chat column, composer, bubble, code wells, sidebar rows and Workspace panel as the canvas boards draw them
// (Chat.dc.html, Workspace.dc.html), in tokens. Colours come from appearance.ts; contrast.test.ts checks them.
const read = (file: string) => readFileSync(`src/${file}`, "utf8");
/** The rule whose selector starts a line, exactly. */
const rule = (css: string, selector: string) => {
  const start = `\n${css}`.indexOf(`\n${selector} {`);
  expect(start, selector).toBeGreaterThanOrEqual(0);
  return css.slice(start, css.indexOf("}", start));
};

it("lays the chat out in a 700px column with 16px between items and a 28px gutter", () => {
  const thread = read("screens/ThreadView.css");
  expect(rule(thread, ".themis-thread")).toMatch(/--themis-thread-gutter: 28px; --themis-thread-column: 700px;/);
  expect(rule(thread, ".themis-thread-stream-content")).toContain("gap: 16px");
  expect(rule(thread, ".themis-tool-group > summary")).toContain("color: var(--themis-text-2)");
});

it("draws the user bubble as a lifted card with four round corners", () => {
  const bubble = rule(read("screens/ThreadView.css"), ".themis-thread-msg--user");
  for (const part of ["max-width: 80%", "padding: 9px 15px", "border-radius: 18px;", "background: var(--themis-card)", "box-shadow: var(--themis-card-lift)"]) expect(bubble).toContain(part);
  expect(read("theme/tokens.css")).toContain("--themis-card-lift: 0 1px 2px var(--themis-panel-shadow), inset 0 1px 0 var(--themis-panel-highlight);");
});

it("makes the composer (and the greeting's) a glass card with the panel edge", () => {
  const thread = read("screens/ThreadView.css");
  const composer = rule(thread, ".themis-thread-composer");
  for (const part of ["gap: 10px", "padding: 14px 14px 10px 18px", "border-radius: var(--themis-radius-composer)", "background: var(--themis-panel-sheen), var(--themis-card)", "box-shadow: var(--themis-panel-edge)", "backdrop-filter: var(--themis-panel-blur)"]) expect(composer).toContain(part);
  expect(read("theme/tokens.css")).toContain("--themis-radius-composer: 26px;");
  expect(rule(thread, ".themis-thread-composer:focus-within")).toContain("box-shadow: var(--themis-panel-edge), 0 0 0 1px var(--themis-border)");
  expect(rule(thread, ".themis-thread-composer-actions")).toContain("gap: 16px");
  // The running ring is masked to the edge: a translucent composer must not show the sweep through its middle.
  expect(rule(thread, ".themis-composer-ring")).toMatch(/mask-composite: exclude/);
  expect(thread).not.toContain(".themis-composer-ring::after");
});

it("draws send and stop as one filled accent circle", () => {
  const send = rule(read("screens/ThreadView.css"), ".themis-button.themis-send");
  for (const part of ["width: 32px", "height: 32px", "border-radius: 50%", "background: var(--themis-accent)", "color: var(--themis-accent-ink)"]) expect(send).toContain(part);
  // Not ready: the accent dimmed under a full-strength arrow, never a faded whole button (muddy on Classic, grey on light skies).
  const disabled = rule(read("screens/ThreadView.css"), ".themis-button.themis-send:disabled");
  expect(disabled).toContain("background: color-mix(in srgb, var(--themis-accent) 65%, transparent)");
  expect(disabled).toContain("opacity: 1");
});

it("keeps the diff's hunk header a quiet row inside the well, never an opaque band", () => {
  const header = rule(read("components/diff/DiffView.css"), ".themis-diff-hunk-header");
  expect(header).toContain("background: color-mix(in srgb, var(--themis-text) 4%, transparent)");
  expect(header).toContain("color: var(--themis-text-2)");
  expect(header).not.toMatch(/raised|sticky/);
});

it("rounds a chosen sidebar chat, Workspace tab and file row at the canvas's 10px", () => {
  expect(rule(read("screens/Sidebar.css"), ".themis-sidebar-row")).toContain("border-radius: 10px");
  expect(rule(read("screens/WorkspacePanel.css"), ".themis-workspace-tabs button")).toContain("border-radius: 10px");
  expect(rule(read("screens/WorkspacePanel.css"), ".themis-files-row")).toContain("border-radius: 10px");
  expect(rule(read("components/diff/FileTree.css"), ".themis-filetree-row")).toContain("border-radius: 10px");
  // Nothing else rounds those rows differently (the rail's, folded Settings' and the ⋮ menu's round icons aside).
  for (const file of ["screens/Sidebar.css", "screens/WorkspacePanel.css", "components/diff/FileTree.css"]) {
    for (const found of read(file).matchAll(/^([^{\n]*(?:thread-row|sidebar-row--active|filetree-row|files-row|workspace-tabs button)[^{]*)\{[^}]*border-radius:\s*([^;]+);/gm)) {
      if (!/sidebar-rail|is-collapsed|row-actions/.test(found[1])) expect(found[2], found[1]).toBe("10px");
    }
  }
});

it("puts code in a well: chat blocks, the Workspace file view and diffs", () => {
  const block = rule(read("components/CodeBlock.css"), ".themis-codeblock");
  for (const part of ["background: var(--themis-well)", "box-shadow: var(--themis-well-shadow)", "border-radius: 12px", "padding: 12px 0"]) expect(block).toContain(part);
  const code = read("components/CodeBlock.css");
  expect(rule(code, ".themis-codeblock-line")).toContain("padding: 0 14px");
  expect(rule(code, ".themis-codeblock-line--add")).toMatch(/background: var\(--themis-add-bg\); color: var\(--themis-diff-add\)/);
  expect(rule(code, ".themis-codeblock-line--del")).toMatch(/background: var\(--themis-del-bg\); color: var\(--themis-diff-del\)/);
  expect(read("theme/tokens.css")).toContain("--themis-well-shadow: inset 0 1px 3px var(--themis-panel-shadow);");
  expect(rule(read("components/ResponseBody.css"), ".themis-code-panel .themis-codeblock")).toContain("line-height: 1.7");
  // Inline code: a slim well, no taller than the line.
  expect(rule(read("components/ResponseBody.css"), ".themis-response :not(pre) > code")).toMatch(/background: var\(--themis-well\);[^}]*padding: 1px 5px/);
  const diff = read("components/diff/DiffView.css");
  for (const part of ["background: var(--themis-well)", "box-shadow: var(--themis-well-shadow)", "border-radius: 12px", "line-height: 1.8"]) expect(rule(diff, ".themis-diff")).toContain(part);
  expect(rule(diff, ".themis-diff-no")).toMatch(/min-width: 30px;[^}]*color: var\(--themis-code-comment\)/);
});

it("lists sidebar chats in 34px rounded rows, the chosen one on a card, under quiet section titles", () => {
  const sidebar = read("screens/Sidebar.css");
  expect(rule(sidebar, ".themis-sidebar-row")).toMatch(/gap: 10px;[^}]*min-height: 34px;[^}]*border-radius: 10px/);
  expect(rule(sidebar, ".themis-thread-row.themis-sidebar-row--active")).toContain("background: var(--themis-card)");
  const section = rule(sidebar, ".themis-sidebar-section");
  for (const part of ["letter-spacing: .06em", "color: var(--themis-text-2)", "padding: 12px 10px 4px"]) expect(section).toContain(part);
  expect(section).not.toContain("border-top");
});

it("draws the Workspace panel on its own glass with card tabs and file rows", () => {
  const workspace = read("screens/WorkspacePanel.css");
  const panel = rule(workspace, ".themis-workspace-panel");
  for (const part of ["gap: 8px", "padding: 10px", "background: var(--themis-panel-sheen), var(--themis-inspector-bg)", "box-shadow: var(--themis-panel-edge)"]) expect(panel).toContain(part);
  expect(rule(workspace, ".themis-workspace-head")).not.toContain("border-bottom");
  const tab = rule(workspace, ".themis-workspace-tabs button");
  for (const part of ["gap: 6px", "height: 32px", "padding: 0 9px", "border-radius: 10px", "color: var(--themis-text-2)"]) expect(tab).toContain(part);
  expect(rule(workspace, '.themis-workspace-tabs button[aria-pressed="true"]')).toMatch(/background: var\(--themis-card\); color: var\(--themis-text\)/);
  const tree = read("components/diff/FileTree.css");
  expect(rule(tree, ".themis-filetree-row--selected")).toMatch(/background: var\(--themis-card\)/);
  expect(rule(read("components/diff/DiffView.css"), ".themis-diff-count-add")).toContain("color: var(--themis-diff-add)");
});
