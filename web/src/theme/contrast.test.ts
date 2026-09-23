import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// Read the stylesheet from disk: `?raw` CSS imports return an empty string
// when the @tailwindcss/vite plugin is active, so bypass the transform.
const tokensCss = readFileSync(new URL("./tokens.css", import.meta.url), "utf8");

type ThemeVars = Record<string, string>;

function parseThemeVars(css: string): { dark: ThemeVars; light: ThemeVars } {
  const dark: ThemeVars = {};
  const lightOverrides: ThemeVars = {};
  const rulePattern = /([^{}]+)\{([^{}]*)\}/g;
  let match: RegExpExecArray | null;
  while ((match = rulePattern.exec(css)) !== null) {
    const selector = match[1].trim();
    const body = match[2];
    const target = selector.includes("light")
      ? lightOverrides
      : selector.includes(":root") || selector.includes("dark")
        ? dark
        : null;
    if (target === null) continue;
    for (const decl of body.split(";")) {
      const declMatch = /(--[a-z0-9-]+)\s*:\s*([^;]+)/i.exec(decl);
      if (declMatch !== null) {
        target[declMatch[1].trim()] = declMatch[2].trim();
      }
    }
  }
  return { dark, light: { ...dark, ...lightOverrides } };
}

function hexToRgb(hex: string): [number, number, number] {
  const m = /^#([0-9a-f]{6})$/i.exec(hex.trim());
  if (m === null) throw new Error(`expected 6-digit hex color, got: ${hex}`);
  const v = parseInt(m[1], 16);
  return [(v >> 16) & 0xff, (v >> 8) & 0xff, v & 0xff];
}

function channelLuminance(c: number): number {
  const s = c / 255;
  return s <= 0.04045 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
}

// WCAG relative luminance.
function luminance(hex: string): number {
  const [r, g, b] = hexToRgb(hex);
  return (
    0.2126 * channelLuminance(r) +
    0.7152 * channelLuminance(g) +
    0.0722 * channelLuminance(b)
  );
}

function contrastRatio(aHex: string, bHex: string): number {
  const l1 = luminance(aHex);
  const l2 = luminance(bHex);
  const [hi, lo] = l1 >= l2 ? [l1, l2] : [l2, l1];
  return (hi + 0.05) / (lo + 0.05);
}

const AA_NORMAL = 4.5;
const SECONDARY_MIN = 3.0;

describe("themis theme token contrast", () => {
  const themes = parseThemeVars(tokensCss);

  for (const name of ["dark", "light"] as const) {
    const vars = themes[name];
    const ratio = (fg: string, bg: string) =>
      contrastRatio(vars[fg], vars[bg]);

    it(`${name}: text on canvas >= ${AA_NORMAL}`, () => {
      expect(ratio("--themis-text", "--themis-canvas")).toBeGreaterThanOrEqual(
        AA_NORMAL,
      );
    });

    it(`${name}: text on surface >= ${AA_NORMAL}`, () => {
      expect(ratio("--themis-text", "--themis-surface")).toBeGreaterThanOrEqual(
        AA_NORMAL,
      );
    });

    it(`${name}: text-2 on surface >= ${AA_NORMAL}`, () => {
      expect(
        ratio("--themis-text-2", "--themis-surface"),
      ).toBeGreaterThanOrEqual(AA_NORMAL);
    });

    it(`${name}: accent-ink on accent >= ${AA_NORMAL}`, () => {
      expect(
        ratio("--themis-accent-ink", "--themis-accent"),
      ).toBeGreaterThanOrEqual(AA_NORMAL);
    });

    it(`${name}: text-3 on surface reported (>= ${SECONDARY_MIN})`, () => {
      const value = ratio("--themis-text-3", "--themis-surface");
      // eslint-disable-next-line no-console
      console.log(`[${name}] text-3/surface contrast: ${value.toFixed(2)}`);
      expect(value).toBeGreaterThanOrEqual(SECONDARY_MIN);
    });
  }
});
