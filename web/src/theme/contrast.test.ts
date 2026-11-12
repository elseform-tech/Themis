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
    // Frame, stage, surface, raised and the side panel; the border colour is a line, never under text.
    for (const surface of [...preset.colors.slice(0, 4), preset.sidebar]) {
      expect(contrastRatio(preset.colors[5], surface)).toBeGreaterThanOrEqual(4.5);
      expect(contrastRatio(preset.colors[6], surface)).toBeGreaterThanOrEqual(4.5);
      expect(contrastRatio(preset.colors[7], surface)).toBeGreaterThanOrEqual(4.5);
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
      // The panel always sits on the frame tint, which covers the whole sky.
      const panel = compositeOver(glass.panel, compositeOver(glass.frame, stop));
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
      surfaces.push(frame, panel, sheenOver(preset.edge.highlight, panel), compositeOver(glass.stage, frame));
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
        // A changed line, its number included, is all its own ink (changed characters are underlined, not re-tinted).
        for (const [lineInk, line] of [[ink.add, added], [ink.del, removed]] as const) {
          expect(contrastRatio(lineInk, line), `${lineInk} on ${line}`).toBeGreaterThanOrEqual(AA_NORMAL);
        }
      }
    }
  });

  // The arrow is an icon: 3:1 on the circle, ready and dimmed (the accent at 70% on the composer when not ready).
  it(`${preset.name}: the send button's arrow reads on the accent, and on the dimmed circle when not ready`, () => {
    expect(contrastRatio(ink.onAccent, accent)).toBeGreaterThanOrEqual(SECONDARY_MIN);
    for (const { stage } of chatLayers(preset)) {
      const dimmed = compositeOver(withAlpha(accent, .7), compositeOver(card, stage));
      expect(contrastRatio(ink.onAccent, dimmed), `arrow on dimmed send ${dimmed}`).toBeGreaterThanOrEqual(SECONDARY_MIN);
    }
  });

}

function withAlpha(colour: string, alpha: number): string {
  const [r, g, b] = hexToRgb(colour);
  return `rgba(${r},${g},${b},${alpha})`;
}

// The canvas boards' tokens (Chat, Workspace, Main .aura blocks), verbatim per theme. The panels stand off the frame by
// their edge and shadow (chatSurfaces.test.ts), not by tints tuned for luminance. Only a text, accent, status or ink
// colour that misses 4.5 on its real composited surface may move, by the smallest step that passes, and each such
// move is listed in CONTRAST_DEPARTURES with the canvas value.
type CanvasTheme = Record<string, string>;
const CANVAS_MORNING: CanvasTheme = {
  win: "rgba(235,246,255,.65)", side: "rgba(220,238,255,.45)", stage: "rgba(255,255,255,.4)", cardh: "rgba(255,255,255,.82)",
  well: "rgba(215,232,248,.55)", insp: "rgba(230,242,255,.52)", hi: "rgba(255,255,255,.85)", lo: "rgba(255,255,255,.22)",
  sh: "rgba(8,20,45,.14)", edge: "rgba(15,29,51,.13)", div: "rgba(15,29,51,.08)", pill: "rgba(0,122,255,.1)",
  t1: "#0F1D33", t2: "#3B5273", acc: "#007AFF", ona: "#ffffff",
  kw: "#005CC5", fn: "#6F42C1", str: "#0A7B56", ty: "#C2410C", com: "#64748B", add: "#15803D", addh: "rgba(52,199,89,.18)", del: "#B91C1C", delh: "rgba(255,59,48,.15)",
  g1: "rgba(255,209,179,.45)", g2: "rgba(166,216,255,.25)", sky: "#BFDDFB 0%, #DCEBFA 48%, #FFE2CC 100%",
};
const CANVAS: Record<string, CanvasTheme> = {
  morning: CANVAS_MORNING,
  noon: {
    ...CANVAS_MORNING, win: "rgba(244,250,255,.6)", side: "rgba(226,241,255,.5)", stage: "rgba(255,255,255,.58)", cardh: "rgba(255,255,255,.94)",
    well: "rgba(222,236,250,.66)", insp: "rgba(236,246,255,.6)", hi: "rgba(255,255,255,.92)", lo: "rgba(255,255,255,.3)", pill: "rgba(0,102,224,.11)",
    t1: "#0B1830", t2: "#33496B", acc: "#0066E0", g1: "rgba(255,244,214,.5)", g2: "rgba(120,196,255,.34)", sky: "#6DB6F7 0%, #A9D6FB 52%, #E4F3FF 100%",
  },
  night: {
    win: "rgba(11,15,29,.72)", side: "rgba(16,22,42,.55)", stage: "rgba(7,10,20,.6)", cardh: "rgba(34,46,78,.68)",
    well: "rgba(5,8,16,.78)", insp: "rgba(14,20,38,.64)", hi: "rgba(165,216,255,.32)", lo: "rgba(165,216,255,.05)",
    sh: "rgba(0,0,0,.4)", edge: "rgba(148,197,255,.18)", div: "rgba(148,197,255,.1)", pill: "rgba(56,189,248,.14)",
    t1: "#EBF3FF", t2: "#94A9C9", acc: "#38BDF8", ona: "#06101E",
    kw: "#7DD3FC", fn: "#C4B5FD", str: "#6EE7B7", ty: "#FCD34D", com: "#64748B", add: "#4ADE80", addh: "rgba(48,209,88,.2)", del: "#F87171", delh: "rgba(255,69,58,.2)",
    g1: "rgba(30,41,91,.55)", g2: "rgba(13,76,115,.3)", sky: "#04060E 0%, #0A1024 55%, #141C44 100%",
  },
  classic: {
    win: "#232427", side: "#1c1d20", stage: "#131314", card: "#1f2023", cardh: "#282a2d", well: "#1f2023", insp: "#1c1d20",
    hi: "rgba(255,255,255,.05)", lo: "transparent", sh: "rgba(0,0,0,.28)", edge: "#34363b", div: "#2a2c30", pill: "rgba(217,167,62,.14)",
    t1: "#e6e6e6", t2: "#a8abb2", acc: "#d9a73e", ona: "#1a1405",
    kw: "#8ab4f8", fn: "#d7aefb", str: "#81c995", ty: "#e3b85a", com: "#8d9199", add: "#81c995", addh: "rgba(129,201,149,.13)", del: "#f28b82", delh: "rgba(242,139,130,.13)",
  },
};

/** Colours moved off the canvas because they miss 4.5 (text) or 3 (icons) on their real composited surface. */
export const CONTRAST_DEPARTURES: Record<string, { canvas: string; value: string }> = {
  // Accent text (links, the Auto shield, primary actions) on the pale glass: 3.35 → 4.53 (Morning), 3.81 → 4.56 (Noon).
  "morning.acc": { canvas: "#007AFF", value: "#0066d6" },
  "noon.acc": { canvas: "#0066E0", value: "#005bc7" },
  // Code ink on the light wells (worst well over the stage or the Workspace panel, every sky stop and glow).
  "morning.str": { canvas: "#0A7B56", value: "#0a7753" },
  "morning.ty": { canvas: "#C2410C", value: "#ba3e0c" },
  "morning.com": { canvas: "#64748B", value: "#5b6a7e" },
  "noon.str": { canvas: "#0A7B56", value: "#0a7954" },
  "noon.ty": { canvas: "#C2410C", value: "#bc3f0c" },
  "noon.com": { canvas: "#64748B", value: "#5c6b80" },
  "night.com": { canvas: "#64748B", value: "#6c7b91" },
  // Diff ink on its own tinted line and as line counts on the panels.
  "morning.add": { canvas: "#15803D", value: "#116a33" },
  "morning.del": { canvas: "#B91C1C", value: "#b51b1b" },
  "noon.add": { canvas: "#15803D", value: "#116a33" },
  "noon.del": { canvas: "#B91C1C", value: "#b71c1c" },
};

function presetTokens(preset: (typeof APPEARANCE_PRESETS)[number]): CanvasTheme {
  const { colors, edge, surfaces, ink, glass } = preset;
  const tokens: CanvasTheme = {
    cardh: surfaces.card, well: surfaces.well, insp: surfaces.inspector, hi: edge.highlight, lo: edge.ring, sh: edge.shadow, edge: edge.stage,
    div: preset.div, pill: preset.pill, t1: colors[5], t2: colors[6], acc: colors[7], ona: ink.onAccent,
    kw: ink.keyword, fn: ink.function, str: ink.string, ty: ink.type, com: ink.comment, add: ink.add, addh: ink.addBg, del: ink.del, delh: ink.delBg,
  };
  if (glass && preset.sky) {
    const glows = preset.sky.match(/rgba\([^)]+\)/g) ?? [];
    Object.assign(tokens, { win: glass.frame, side: glass.panel, stage: glass.stage, g1: glows[0], g2: glows[1], sky: /linear-gradient\(180deg, ([^)]*)\)$/.exec(preset.sky)?.[1] ?? "" });
  } else {
    Object.assign(tokens, { win: colors[0], side: preset.sidebar, stage: colors[1], card: colors[2] });
  }
  return tokens;
}

for (const preset of APPEARANCE_PRESETS) {
  it(`${preset.name}: uses the canvas's tokens verbatim, but for the listed contrast departures`, () => {
    const canvas = CANVAS[preset.id];
    const ours = presetTokens(preset);
    for (const [token, value] of Object.entries(canvas)) {
      const departure = CONTRAST_DEPARTURES[`${preset.id}.${token}`];
      if (departure) expect(departure.canvas.toLowerCase(), `${token} departure records the canvas value`).toBe(value.toLowerCase());
      expect(ours[token]?.toLowerCase(), `${preset.id} --${token}`).toBe((departure?.value ?? value).toLowerCase());
    }
    // Classic's raised surface is the canvas's --cardh; the skies' frame tint covers the canvas sky and glows.
    if (preset.id === "classic") expect(preset.colors[3]).toBe(canvas.cardh);
  });
}

// The canvas sets the side panel and the stage in an order: on dark looks the chat (stage) is darker than the side
// panel (Night: lighter navy glass beside an ink chat area); on light skies the stage is the brighter white.
for (const preset of APPEARANCE_PRESETS) {
  it(`${preset.name}: the side panel and the stage keep the canvas's order`, () => {
    const middle = preset.glass && preset.sky ? compositeOver(preset.glass.frame, (preset.sky.match(/#[0-9a-f]{6}/gi) ?? [])[1]) : preset.colors[0];
    const side = preset.glass ? compositeOver(preset.glass.panel, middle) : preset.sidebar;
    const stage = preset.glass ? compositeOver(preset.glass.stage, middle) : preset.colors[1];
    if (preset.mode === "dark") expect(luminance(stage), `stage ${stage} darker than side ${side}`).toBeLessThan(luminance(side));
    else expect(luminance(stage), `stage ${stage} brighter than side ${side}`).toBeGreaterThan(luminance(side));
    // The opaque stand-ins are those composites.
    expect([preset.sidebar, preset.colors[1]]).toEqual([side, stage]);
  });
}
