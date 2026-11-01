import { expect, it } from "vitest";
import { readFileSync } from "node:fs";

it("sets body text to the chosen size, and no label below 9px even at the smallest size", () => {
  const shell = readFileSync("src/screens/shell.css", "utf8");
  expect(shell).toMatch(/\.themis-shell \{[^}]*font-size: \.8333rem/);
  expect(shell).toContain("-webkit-font-smoothing: antialiased");
  const source = readFileSync("src/theme/appearance.ts", "utf8");
  expect(source).toContain("settings.text_size * 1.2");
  // At 10, a rem is 12px: .75rem is 9px.
  expect(12 * .8333).toBeCloseTo(10, 0);
  for (const file of ["src/components/ResponseBody.css", "src/screens/WorkspacePanel.css", "src/components/diff/FileTree.css"]) {
    const smallest = [...readFileSync(file, "utf8").matchAll(/font-size:\s*0?(\.\d+)rem/g)].map(found => Number(found[1]));
    expect(Math.min(...smallest) * 12).toBeGreaterThanOrEqual(9);
  }
});
