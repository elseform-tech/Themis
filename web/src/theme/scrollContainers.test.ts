// @vitest-environment node
import { readFileSync } from "node:fs";
import { expect, it } from "vitest";

// macOS overlay scrollbars flash on every scroll container under the pointer. In Workspace and the chat only the
// panes that really scroll on their own may be one; a well (code, diff) clips. Note that `overflow-x: hidden` with
// `overflow-y: visible` computes to `auto`, so a well with either axis set to hidden/auto/scroll is a scroller.
const FILES = ["components/diff/DiffView.css", "components/diff/FileTree.css", "components/CodeBlock.css", "components/ResponseBody.css", "screens/WorkspacePanel.css", "screens/ThreadView.css"];

function scrollers(): string[] {
  const found: string[] = [];
  for (const file of FILES) {
    const css = readFileSync(`src/${file}`, "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
    for (const rule of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
      if (/overflow(-x|-y)?\s*:\s*(auto|scroll)/.test(rule[2]!)) found.push(rule[1]!.trim().replace(/\s+/g, " "));
    }
  }
  return found.sort();
}

it("lets only the panes that must scroll on their own be scroll containers in Workspace and the chat", () => {
  expect(scrollers()).toEqual([
    ".themis-background-log", // a background command's log in Tasks, capped at 240px
    ".themis-branch-menu", // the composer's branch list, a popover
    ".themis-changes-stack", // Workspace › Changes: the one scroll for all files and their wells
    ".themis-composer-attachments, .themis-message-attachments", // attachment rows (overridden to visible where they wrap)
    ".themis-diagram-scroll", // a chart too wide to read, sideways in its own frame
    ".themis-edited-files ul.is-expanded", // a long edited-files list under a reply
    ".themis-greeting", // the empty chat
    ".themis-response-table", // a wide table, sideways in its own frame
    ".themis-tasks", // Workspace › Tasks
    ".themis-thread-composer .themis-prompt-editor", // a long message being written
    ".themis-thread-stream", // the conversation
    ".themis-tool-detail pre", // a tool's output, capped at 320px
    ".themis-workspace-branches > .themis-workspace-list",
    ".themis-workspace-list", // pull requests and branches
    ".themis-workspace-split > :not(.themis-split-resize)", // the Files tab's tree and file pane
  ].sort());
});

it("clips the code and diff wells (and their panes) instead of making them scroll, keeping their rounded corners", () => {
  const diff = readFileSync("src/components/diff/DiffView.css", "utf8");
  expect(diff).toMatch(/\.themis-diff \{[^}]*overflow: clip;[^}]*border-radius: 12px|\.themis-diff \{[^}]*border-radius: 12px;[^}]*overflow: clip/);
  expect(diff).toMatch(/\.themis-diff-unified, \.themis-diff-side \{[^}]*overflow: clip;/);
  expect(readFileSync("src/components/CodeBlock.css", "utf8")).toMatch(/\.themis-codeblock \{[^}]*overflow: clip;/);
  const workspace = readFileSync("src/screens/WorkspacePanel.css", "utf8");
  expect(workspace).toMatch(/\.themis-files-view \.themis-codeblock \{ overflow: clip; \}/);
  expect(workspace).not.toMatch(/overflow-y: visible/);
});

// Inside Workspace › Changes nothing but the stack is a scroll container, `hidden` included (WebKit can flash an
// overlay bar on a hidden-overflow block that overflows by a pixel). The one exception is an ellipsis text span.
it("keeps every element inside the Changes stack from being a scroll container, but ellipsis text spans", () => {
  const offenders: string[] = [];
  for (const file of ["components/diff/FileTree.css", "components/diff/DiffView.css", "components/CodeBlock.css", "components/primitives/EmptyState.css"]) {
    const css = readFileSync(`src/${file}`, "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
    for (const rule of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
      const body = rule[2]!;
      if (!/overflow(-x|-y)?\s*:\s*(auto|scroll|hidden)/.test(body)) continue;
      if (/overflow\s*:\s*hidden/.test(body) && /text-overflow:\s*ellipsis/.test(body) && /white-space:\s*nowrap/.test(body)) continue;
      offenders.push(`${file}: ${rule[1]!.trim()}`);
    }
  }
  expect(offenders).toEqual([]);
});

// With overlay scrollbars the stack's bar sits over its content; the wells are positioned (their Unified/Split
// control), so unless the stack is its own stacking context they paint over its bar and leave slivers beside the rows.
it("paints the Changes stack's overlay scrollbar above its positioned wells", () => {
  const workspace = readFileSync("src/screens/WorkspacePanel.css", "utf8");
  const stack = workspace.slice(workspace.indexOf(".themis-changes-stack {"), workspace.indexOf("}", workspace.indexOf(".themis-changes-stack {")));
  // Its own stacking context and compositing layer, so WebKit paints its overlay bar over the opaque Classic wells…
  for (const part of ["overflow-y: auto", "isolation: isolate", "transform: translateZ(0)"]) expect(stack).toContain(part);
  // …and a gutter the width of an overlay bar, so no well or row ever sits under it.
  expect(stack).toContain("padding-right: 12px");
});
