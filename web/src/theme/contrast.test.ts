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
    for (const surface of [...preset.colors.slice(0, 4), preset.sidebar]) {
      expect(contrastRatio(preset.colors[5], surface)).toBeGreaterThanOrEqual(4.5);
      expect(contrastRatio(preset.colors[6], surface)).toBeGreaterThanOrEqual(4.5);
      expect(contrastRatio(preset.colors[7], surface)).toBeGreaterThanOrEqual(4.5);
    }
  });
}

// The frame sits behind; the sidebar and the stage must read as separate panels on it by their tint, not only by
// their ring and shadow. Measured at the sky's middle stop, glass and solid (reduced transparency) alike. The bar is a
// luminance contrast of 1.25 on light skies and 1.2 on dark ones, where WCAG's flare term squeezes every ratio
// (#0e0f10 to #212225 is 1.2); before this, the panels sat 1.02 to 1.09 off the frame and read as one colour.
for (const preset of APPEARANCE_PRESETS) {
  it(`${preset.name}: the panel, the stage and the Workspace panel stand clearly off the frame`, () => {
    const bar = preset.mode === "light" ? 1.25 : 1.2;
    // The Workspace panel (inspector) is a side panel too; solid, it is the sidebar colour.
    const layers: Array<[string, string, string, string]> = [[preset.colors[0], preset.sidebar, preset.colors[1], preset.sidebar]];
    if (preset.glass && preset.sky) {
      const middle = (preset.sky.match(/#[0-9a-f]{6}/gi) ?? [])[1];
      const frame = compositeOver(preset.glass.frame, middle);
      layers.push([frame, compositeOver(preset.glass.panel, frame), compositeOver(preset.glass.stage, frame), compositeOver(preset.surfaces.inspector, frame)]);
    }
    for (const [frame, panel, stage, inspector] of layers) {
      expect(contrastRatio(panel, frame), `panel ${panel} on ${frame}`).toBeGreaterThanOrEqual(bar);
      expect(contrastRatio(stage, frame), `stage ${stage} on ${frame}`).toBeGreaterThanOrEqual(bar);
      expect(contrastRatio(inspector, frame), `Workspace ${inspector} on ${frame}`).toBeGreaterThanOrEqual(bar);
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

// The side panels' sheen: the highlight fades from -45% to 28% of a 140deg gradient, so the corner gets 28/73 of it.
function sheenOver(highlight: string, panel: string, share = 28 / 73): string {
  const [, rgb, alpha] = /^rgba\((.*),\s*([\d.]+)\)$/.exec(highlight) ?? [];
  return rgb === undefined ? panel : compositeOver(`rgba(${rgb},${(Number(alpha) * share).toFixed(3)})`, panel);
}

// The composer's controls row (the shield's Auto and YOLO colours, the context warning) sits below the message, about
// 68px down and 18px in on a composer up to 760px wide: at 140deg that is 12% along the sheen, where 16/73 of the
// highlight is left. The message text and placeholder sit in the corner and are measured at the full 28/73.
const CONTROLS_SHEEN = 16 / 73;

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
        const panel = compositeOver(glass.panel, frame);
        for (const surface of [frame, panel, sheenOver(preset.edge.highlight, panel), compositeOver(glass.stage, frame)]) {
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
      const panel = compositeOver(glass.panel, frame);
      surfaces.push(frame, panel, sheenOver(preset.edge.highlight, panel), compositeOver(glass.stage, frame), compositeOver(glass.panel, stop));
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

// Chat and Workspace surfaces (canvas boards Chat and Workspace). Every layer is composited as it renders: the bubble
// and composer (card) on the stage, the Workspace panel (inspector) on the frame, a selected tab or file (card) on the
// panel, code (well) on the stage and on the panel, and the diff's tinted lines on the well. Skies are measured over
// every sky stop and glow, and solid (reduced transparency: the stand-ins, the panel as the sidebar colour).
type ChatLayers = { stage: string; inspector: string };

function chatLayers(preset: (typeof APPEARANCE_PRESETS)[number]): ChatLayers[] {
  const layers: ChatLayers[] = [{ stage: preset.colors[1], inspector: preset.sidebar }];
  const sky = preset.sky ?? "";
  for (const stop of sky.match(/#[0-9a-f]{6}/gi) ?? []) {
    for (const glow of [undefined, ...(sky.match(/rgba\([^)]+\)/g) ?? [])]) {
      if (!preset.glass) continue;
      const frame = compositeOver(preset.glass.frame, glow === undefined ? stop : compositeOver(glow, stop));
      layers.push({ stage: compositeOver(preset.glass.stage, frame), inspector: compositeOver(preset.surfaces.inspector, frame) });
    }
  }
  return layers;
}

for (const preset of APPEARANCE_PRESETS) {
  const { card, well } = preset.surfaces;
  const ink = preset.ink;
  const [text, text2, accent] = preset.colors.slice(5, 8);
  const status = ["--themis-success", "--themis-warning", "--themis-danger", "--themis-info"].map(token => parseThemeVars(tokensCss)[preset.mode][token]);

  it(`${preset.name}: text, accent, status and line counts reach 4.5 on the bubble, the composer and the Workspace panel`, () => {
    for (const { stage, inspector } of chatLayers(preset)) {
      const bubble = compositeOver(card, stage);
      const selected = compositeOver(card, inspector);
      for (const surface of [bubble, sheenOver(preset.edge.highlight, bubble, CONTROLS_SHEEN), inspector, sheenOver(preset.edge.highlight, inspector), selected]) {
        for (const colour of [text, text2, accent, ...status, ink.add, ink.del]) {
          expect(contrastRatio(colour, surface), `${colour} on ${surface}`).toBeGreaterThanOrEqual(AA_NORMAL);
        }
      }
      for (const colour of [text, text2]) {
        const corner = sheenOver(preset.edge.highlight, bubble);
        expect(contrastRatio(colour, corner), `${colour} on the composer's corner ${corner}`).toBeGreaterThanOrEqual(AA_NORMAL);
      }
    }
  });

  it(`${preset.name}: code, line numbers and diff ink reach 4.5 on the well, in the chat and in Workspace`, () => {
    for (const { stage, inspector } of chatLayers(preset)) {
      for (const under of [stage, inspector]) {
        const plain = compositeOver(well, under);
        const added = compositeOver(ink.addBg, plain);
        const removed = compositeOver(ink.delBg, plain);
        for (const colour of [text, text2, ink.keyword, ink.function, ink.string, ink.type, ink.comment, ink.add, ink.del]) {
          expect(contrastRatio(colour, plain), `${colour} on well ${plain}`).toBeGreaterThanOrEqual(AA_NORMAL);
        }
        // The diff's hunk header ("Line 1"): text-2 on a 4% tint of the text colour inside the well.
        const hunk = compositeOver(withAlpha(text, .04), plain);
        expect(contrastRatio(text2, hunk), `hunk header ${hunk}`).toBeGreaterThanOrEqual(AA_NORMAL);
        // Changed lines read as tinted, calmly: visible on the well, never a bright slab (the canvas's --addh/--delh).
        for (const line of [added, removed]) {
          expect(contrastRatio(line, plain), `changed line ${line} on ${plain}`).toBeGreaterThanOrEqual(1.08);
          expect(contrastRatio(line, plain), `changed line ${line} on ${plain}`).toBeLessThanOrEqual(1.3);
        }
        // A changed line is all its own ink; the characters that changed get the line's tint twice; numbers stay --com.
        for (const [lineInk, lineBg, line] of [[ink.add, ink.addBg, added], [ink.del, ink.delBg, removed]] as const) {
          expect(contrastRatio(lineInk, line), `${lineInk} on ${line}`).toBeGreaterThanOrEqual(AA_NORMAL);
          expect(contrastRatio(lineInk, compositeOver(lineBg, line)), `${lineInk} on its word tint`).toBeGreaterThanOrEqual(AA_NORMAL);
          expect(contrastRatio(ink.comment, line), `line number on ${line}`).toBeGreaterThanOrEqual(AA_NORMAL);
        }
      }
    }
  });

  it(`${preset.name}: the send button's arrow reads on the accent, and on the dimmed circle when not ready`, () => {
    expect(contrastRatio(ink.onAccent, accent)).toBeGreaterThanOrEqual(AA_NORMAL);
    // Not ready: the accent at 65% on the composer, the same arrow; a 3:1 control, still plainly the accent's circle.
    for (const { stage } of chatLayers(preset)) {
      const dimmed = compositeOver(withAlpha(accent, .65), compositeOver(card, stage));
      expect(contrastRatio(ink.onAccent, dimmed), `arrow on dimmed send ${dimmed}`).toBeGreaterThanOrEqual(SECONDARY_MIN);
      expect(contrastRatio(dimmed, accent), `dimmed ${dimmed} vs ready ${accent}`).toBeGreaterThanOrEqual(1.2);
    }
  });

  // The owner wants the bubble and composer to lift off the stage, and a selected tab or file to read as chosen. On
  // light skies the card is near white and the stage must stay 1.25 off the frame, so white caps the step at about
  // 1.15: the bar is 1.1 there, with the shadow and top highlight doing the rest. Dark skies use the frame bar, 1.2.
  it(`${preset.name}: the bubble and composer stand off the stage, a selected row off the Workspace panel, the well off both`, () => {
    const bar = preset.mode === "light" ? 1.1 : 1.2;
    for (const { stage, inspector } of [chatLayers(preset)[0], ...middleLayers(preset)]) {
      expect(contrastRatio(compositeOver(card, stage), stage), `card on stage ${stage}`).toBeGreaterThanOrEqual(bar);
      expect(contrastRatio(compositeOver(card, inspector), inspector), `card on panel ${inspector}`).toBeGreaterThanOrEqual(bar);
      // Code sits in a visible well, on the stage and on the Workspace panel; the inset shadow adds the edge.
      const wellBar = preset.mode === "light" ? 1.06 : 1.1;
      expect(contrastRatio(compositeOver(well, stage), stage), `well on stage ${stage}`).toBeGreaterThanOrEqual(wellBar);
      expect(contrastRatio(compositeOver(well, inspector), inspector), `well on panel ${inspector}`).toBeGreaterThanOrEqual(wellBar);
    }
  });
}

function withAlpha(colour: string, alpha: number): string {
  const [r, g, b] = hexToRgb(colour);
  return `rgba(${r},${g},${b},${alpha})`;
}

/** The glass layers at the sky's middle stop, as the frame distinction test measures. */
function middleLayers(preset: (typeof APPEARANCE_PRESETS)[number]): ChatLayers[] {
  if (!preset.glass || !preset.sky) return [];
  const frame = compositeOver(preset.glass.frame, (preset.sky.match(/#[0-9a-f]{6}/gi) ?? [])[1]);
  return [{ stage: compositeOver(preset.glass.stage, frame), inspector: compositeOver(preset.surfaces.inspector, frame) }];
}
