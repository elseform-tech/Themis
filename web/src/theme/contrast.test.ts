import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { APPEARANCE_PRESETS, compositeOver } from "./appearance";

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
    if (selector.includes("data-palette")) continue;
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

  for (const name of ["dark"] as const) {
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
      console.log(`[${name}] text-3/surface contrast: ${value.toFixed(2)}`);
      expect(value).toBeGreaterThanOrEqual(SECONDARY_MIN);
    });
  }
});

for (const preset of APPEARANCE_PRESETS) {
  it(`${preset.name}: readable text and actions on every surface`, () => {
    for (const surface of preset.colors.slice(0, 4)) {
      expect(contrastRatio(preset.colors[5], surface)).toBeGreaterThanOrEqual(4.5);
      expect(contrastRatio(preset.colors[6], surface)).toBeGreaterThanOrEqual(4.5);
      expect(contrastRatio(preset.colors[7], surface)).toBeGreaterThanOrEqual(4.5);
    }
  });
}

// The solid presets layer a lighter frame, a darker sidebar and the darkest canvas; skies take their layers from the glass instead.
for (const preset of APPEARANCE_PRESETS.filter(solid => solid.sky === undefined)) {
  it(`${preset.name}: frame, sidebar and canvas have descending brightness`, () => {
    expect(luminance(preset.colors[0])).toBeGreaterThan(luminance(preset.sidebar));
    expect(luminance(preset.sidebar)).toBeGreaterThan(luminance(preset.colors[1]));
    for (const text of [preset.colors[5], preset.colors[6], preset.colors[7]]) {
      expect(contrastRatio(text, preset.sidebar)).toBeGreaterThanOrEqual(4.5);
    }
  });
}

// Text also sits on the frame colour (header, rail, around the panels), colors[0] = --themis-shell-bg.
for (const preset of APPEARANCE_PRESETS) {
  it(`${preset.name}: text and text-2 are readable on the frame colour`, () => {
    expect(contrastRatio(preset.colors[5], preset.colors[0])).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(preset.colors[6], preset.colors[0])).toBeGreaterThanOrEqual(4.5);
  });
}

const SKIES = APPEARANCE_PRESETS.filter(preset => preset.sky !== undefined);

it("ships the Morning, Noon and Night skies, Morning and Noon light", () => {
  expect(SKIES.map(preset => [preset.id, preset.mode])).toEqual([["morning", "light"], ["noon", "light"], ["night", "dark"]]);
});

it("compositeOver blends an rgba colour over an opaque hex", () => {
  expect(compositeOver("rgba(255,255,255,.5)", "#000000")).toBe("#808080");
  expect(compositeOver("rgba(0, 0, 0, 0)", "#123456")).toBe("#123456");
  expect(compositeOver("#abcdef", "#000000")).toBe("#abcdef");
});

for (const preset of SKIES) {
  const sky = preset.sky ?? "";
  const glass = preset.glass ?? { frame: "", panel: "", stage: "" };
  const stops = sky.match(/#[0-9a-f]{6}/gi) ?? [];
  const glows = sky.match(/rgba\([^)]+\)/g) ?? [];
  const [text, text2, accent] = preset.colors.slice(5, 8);
  const theme = parseThemeVars(tokensCss)[preset.mode];
  const status = ["--themis-success", "--themis-warning", "--themis-danger", "--themis-info"].map(token => theme[token]);

  it(`${preset.name}: text, text-2 and accent reach 4.5 on the glass panel over every sky stop`, () => {
    expect(stops.length).toBeGreaterThanOrEqual(3);
    for (const stop of stops) {
      const panel = compositeOver(glass.panel, stop);
      for (const colour of [text, text2, accent]) expect(contrastRatio(colour, panel)).toBeGreaterThanOrEqual(AA_NORMAL);
    }
  });

  it(`${preset.name}: text, accent and status colours read on the frame, panels and stage, glows included`, () => {
    expect(glows.length).toBe(2);
    for (const stop of stops) {
      for (const glow of [undefined, ...glows]) {
        const frame = compositeOver(glass.frame, glow === undefined ? stop : compositeOver(glow, stop));
        for (const surface of [frame, compositeOver(glass.panel, frame), compositeOver(glass.stage, frame)]) {
          for (const colour of [text, text2, accent, ...status]) expect(contrastRatio(colour, surface)).toBeGreaterThanOrEqual(AA_NORMAL);
        }
      }
    }
  });

  it(`${preset.name}: status colours read on its opaque surfaces`, () => {
    for (const surface of preset.colors.slice(0, 4)) {
      for (const colour of status) expect(contrastRatio(colour, surface)).toBeGreaterThanOrEqual(AA_NORMAL);
    }
  });
}

for (const mode of ["dark", "light"] as const) {
  it(`${mode}: compatibility badge text reads on its badge`, () => {
    const vars = parseThemeVars(tokensCss)[mode];
    for (const kind of ["ok", "warn", "bad"]) {
      expect(contrastRatio(vars[`--themis-${kind}-ink`], vars[`--themis-${kind}-bg`])).toBeGreaterThanOrEqual(AA_NORMAL);
    }
  });
}

// The gold gradient is text (the greeting, the wordmark glint, shimmer) and the warning colour, so every stop reads on each sky.
for (const preset of SKIES) {
  const glass = preset.glass ?? { frame: "", panel: "", stage: "" };
  const vars = parseThemeVars(tokensCss)[preset.mode];
  const surfaces: string[] = [];
  for (const stop of (preset.sky ?? "").match(/#[0-9a-f]{6}/gi) ?? []) {
    for (const glow of [undefined, ...((preset.sky ?? "").match(/rgba\([^)]+\)/g) ?? [])]) {
      const frame = compositeOver(glass.frame, glow === undefined ? stop : compositeOver(glow, stop));
      surfaces.push(frame, compositeOver(glass.panel, frame), compositeOver(glass.stage, frame), compositeOver(glass.panel, stop));
    }
  }
  const accent = preset.colors[7].replace(/^#(..)(..)(..)$/, (_, r, g, b) => `rgba(${parseInt(r, 16)},${parseInt(g, 16)},${parseInt(b, 16)},.14)`);
  const worst = (colour: string, extra: string[] = []) => Math.min(...[...surfaces, ...extra].map(surface => contrastRatio(colour, surface)));

  it(`${preset.name}: every gradient stop and the warning text reach 4.5 on the sky's glass, worst stop`, () => {
    for (const token of ["--themis-gradient-1", "--themis-gradient-2", "--themis-gradient-3"]) {
      expect(worst(vars[token]), token).toBeGreaterThanOrEqual(AA_NORMAL);
    }
    // The context warning and compaction notice also sit on an accent tint over the surface.
    expect(worst(vars["--themis-warn-text"], [compositeOver(accent, preset.colors[2])])).toBeGreaterThanOrEqual(AA_NORMAL);
  });

  it(`${preset.name}: the working spinner reaches 3 on the sky's glass`, () => {
    expect(worst(vars["--themis-gradient-2"])).toBeGreaterThanOrEqual(SECONDARY_MIN);
  });
}
