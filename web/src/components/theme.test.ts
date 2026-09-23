import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

// Resolve from the working directory (vitest runs with cwd = web/) so the
// scan works regardless of how import.meta.url is rewritten per environment.
function componentsDir(): string {
  const candidates = [
    join(process.cwd(), "src", "components"),
    join(process.cwd(), "web", "src", "components"),
  ];
  for (const candidate of candidates) {
    if (existsSync(candidate)) return candidate;
  }
  throw new Error("cannot locate src/components from " + process.cwd());
}

function collectSourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) {
      out.push(...collectSourceFiles(full));
    } else if (/\.(css|tsx?)$/.test(entry)) {
      out.push(full);
    }
  }
  return out;
}

// Built via constructor so the pattern source cannot self-match the scan.
const HEX_PATTERN = new RegExp("#[0-9a-fA-F]{3,8}\\b");
const RGB_PATTERN = new RegExp("rgba?\\s*\\(");
const HSL_PATTERN = new RegExp("hsla?\\s*\\(");

describe("themis theme discipline", () => {
  const files = collectSourceFiles(componentsDir());

  it("scans at least one component file", () => {
    expect(files.length).toBeGreaterThan(0);
  });

  it("uses no hardcoded hex colors in components/**", () => {
    const offenders = files.filter((file) =>
      HEX_PATTERN.test(readFileSync(file, "utf8")),
    );
    expect(offenders).toEqual([]);
  });

  it("uses no hardcoded rgb or hsl colors in components/**", () => {
    const offenders = files.filter((file) => {
      const text = readFileSync(file, "utf8");
      return RGB_PATTERN.test(text) || HSL_PATTERN.test(text);
    });
    expect(offenders).toEqual([]);
  });

  it("components stay pure: no ../lib/tauri imports", () => {
    const offenders = files
      .filter((file) => file.endsWith(".tsx"))
      .filter((file) => readFileSync(file, "utf8").includes("lib/tauri"));
    expect(offenders).toEqual([]);
  });
});
