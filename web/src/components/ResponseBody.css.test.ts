import { readFileSync } from "node:fs";
import { expect, it } from "vitest";

const styles = readFileSync("src/components/ResponseBody.css", "utf8");

it("keeps table words whole: `anywhere` would let a column shrink to one letter", () => {
  expect(styles).toMatch(/\.themis-response th,\s*\.themis-response td\s*\{[^}]*overflow-wrap:\s*normal/);
  expect(styles).toMatch(/\.themis-response-table\s*\{[^}]*overflow-x:\s*auto/);
});

// A narrow chat (Workspace dragged wide) never clips or overflows: code wraps in its well, tables scroll in their frame.
it("wraps code, diffs and inline code inside the chat column, with a hanging indent for continued lines", () => {
  const block = readFileSync("src/components/CodeBlock.css", "utf8");
  const line = block.slice(block.indexOf(".themis-codeblock-line {"), block.indexOf("}", block.indexOf(".themis-codeblock-line {")));
  for (const part of ["white-space: pre-wrap", "overflow-wrap: anywhere", "padding: 0 14px 0 calc(14px + 2ch)", "text-indent: -2ch"]) expect(line).toContain(part);
  expect(block).not.toMatch(/width: max-content/);
  expect(styles).toMatch(/\.themis-response :not\(pre\) > code \{[^}]*overflow-wrap: anywhere/);
  const diff = readFileSync("src/components/diff/DiffView.css", "utf8");
  expect(diff).toMatch(/\.themis-diff-text \{[^}]*white-space: pre-wrap;[^}]*overflow-wrap: anywhere/);
  // Line numbers and markers stay with a wrapped line's first visual line.
  expect(diff).toMatch(/\.themis-diff-no, \.themis-diff-sign \{ align-self: flex-start; \}/);
  const thread = readFileSync("src/screens/ThreadView.css", "utf8");
  expect(thread).toMatch(/\.themis-tool-detail pre \{[^}]*white-space: pre-wrap;[^}]*overflow-wrap: anywhere/);
  expect(styles).toMatch(/\.themis-response-table\s*\{[^}]*overflow-x:\s*auto/);
});
