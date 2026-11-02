import { readFileSync } from "node:fs";
import { expect, it } from "vitest";

const styles = readFileSync("src/components/ResponseBody.css", "utf8");

it("keeps table words whole: `anywhere` would let a column shrink to one letter", () => {
  expect(styles).toMatch(/\.themis-response th,\s*\.themis-response td\s*\{[^}]*overflow-wrap:\s*normal/);
  expect(styles).toMatch(/\.themis-response-table\s*\{[^}]*overflow-x:\s*auto/);
});
