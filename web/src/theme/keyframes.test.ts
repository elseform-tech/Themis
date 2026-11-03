import { describe, expect, it } from "vitest";
import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

// Every stylesheet under src/, as text.
function sheets(dir: string): [string, string][] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return sheets(path);
    return entry.name.endsWith(".css") ? [[path, readFileSync(path, "utf8")] as [string, string]] : [];
  });
}

describe("CSS animations", () => {
  it("name only keyframes that exist", () => {
    const all = sheets(fileURLToPath(new URL("..", import.meta.url)));
    const defined = new Set(all.flatMap(([, text]) => [...text.matchAll(/@keyframes\s+([\w-]+)/g)].map((match) => match[1])));
    const missing: string[] = [];
    for (const [file, text] of all) {
      for (const match of text.matchAll(/animation(?:-name)?\s*:\s*([^;}]+)/g)) {
        for (const name of match[1].split(",").map((part) => part.trim().split(/\s+/)[0])) {
          if (name.startsWith("themis-") && !defined.has(name)) missing.push(`${file.split("/src/")[1]}: ${name}`);
        }
      }
    }
    expect(all.length).toBeGreaterThan(10);
    expect(missing).toEqual([]);
  });
});
