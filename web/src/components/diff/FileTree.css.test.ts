// @vitest-environment node
import { expect, it } from "vitest";
import { readFileSync } from "node:fs";

const styles = readFileSync(new URL("./FileTree.css", import.meta.url), "utf8");

it("lets a row shrink to the list so a long name ellipsizes and its line counts stay whole", () => {
  expect(styles).toMatch(/\.themis-filetree > \*, \.themis-filetree-files > \*\s*\{\s*min-width:\s*0;/);
  expect(styles).toMatch(/\.themis-filetree-counts\s*\{\s*flex:\s*none;/);
});
