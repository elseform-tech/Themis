import { useEffect, useState } from "react";
import type { Settings } from "../lib/types";

export type Appearance = Pick<Settings, "theme_palette" | "font_family" | "text_size">;

export const FONT_OPTIONS = [
  ["system", "System"], ["sans", "Arial"], ["serif", "Georgia"], ["mono", "Mono"],
  ["rounded", "Rounded"], ["avenir", "Avenir"], ["helvetica", "Helvetica"],
  ["verdana", "Verdana"], ["trebuchet", "Trebuchet"], ["palatino", "Palatino"],
  ["charter", "Charter"], ["menlo", "Menlo"],
] as const;

export type AppearancePreset = {
  id: string;
  name: string;
  mode: "light" | "dark";
  sidebar: string;
  colors: readonly [string, string, string, string, string, string, string, string];
  /** How the panels are drawn on the frame: the top highlight (also the side panels' sheen), the side panels'
   *  inset ring and drop shadow, and the stage's edge ring. */
  edge: { highlight: string; ring: string; shadow: string; stage: string };
  /** Chat and Workspace surfaces (canvas --cardh, --well, --insp): the bubble, composer and a selected row or tab (card),
   *  code and diffs (well) and the Workspace panel (inspector). Skies: rgba over the stage or frame; Classic: opaque. */
  surfaces: { card: string; well: string; inspector: string };
  /** Ink on the well (canvas --kw, --fn, --str, --ty, --com), a diff's lines (--add/--addh, --del/--delh) and the accent (--ona). */
  ink: { keyword: string; function: string; string: string; type: string; comment: string; add: string; addBg: string; del: string; delBg: string; onAccent: string };
  /** Hairlines (canvas --div; --themis-border-subtle) and the soft accent chip (--pill). */
  div: string;
  pill: string;
  /** The Themis Visions mark on the startup screen (canvas --ga → --gb, the lower V in --lv) and the bar's track (--div). */
  mark: { from: string; to: string; v: string; track: string };
  /** The card in Settings › Appearance. */
  preview: string;
  /** Skies: the background layers behind the frame (glows, then the sky gradient). */
  sky?: string;
  /** Skies: the startup screen's sky (canvas Main board: the same glows, placed at 18% 100% and 86% 6%). */
  loaderSky?: string;
  /** Skies: translucent frame tint, panel and stage, drawn over the sky. */
  glass?: { frame: string; panel: string; stage: string };
};

// The canvas boards' tokens (Chat, Workspace and Main .aura blocks), verbatim: frame tint (--win) over the sky and its
// glows, --side (panel), --stage, --cardh (card), --well, --insp (inspector), --hi/--lo/--sh/--edge, --div, --pill,
// --t1, --t2, --acc, --ona and the code and diff colours. The panels stand off the frame by their edge and shadow.
// colors: frame, canvas (stage), surface, raised, border, text, secondary text, accent. Skies: the opaque colours are
// the glass composited at the sky's middle stop (the solid panels under reduced transparency): frame = --win over the
// stop, canvas = --stage over that, sidebar = --side over it, surface = --cardh over the stage, raised = --cardh over the
// surface, border = --edge over the stage. Classic is the canvas's solid set (--card and --cardh are surface and raised).
export const APPEARANCE_PRESETS: readonly AppearancePreset[] = [
  { id: "classic", name: "Classic", mode: "dark", sidebar: "#1c1d20", colors: ["#232427", "#131314", "#1f2023", "#282a2d", "#34363b", "#e6e6e6", "#a8abb2", "#d9a73e"],
    edge: { highlight: "rgba(255,255,255,.05)", ring: "transparent", shadow: "rgba(0,0,0,.28)", stage: "#34363b" },
    surfaces: { card: "#282a2d", well: "#1f2023", inspector: "#1c1d20" },
    ink: { keyword: "#8ab4f8", function: "#d7aefb", string: "#81c995", type: "#e3b85a", comment: "#8d9199", add: "#81c995", addBg: "rgba(129,201,149,.13)", del: "#f28b82", delBg: "rgba(242,139,130,.13)", onAccent: "#1a1405" },
    div: "#2a2c30", pill: "rgba(217,167,62,.14)",
    mark: { from: "#c08a2e", to: "#f6e7bd", v: "#e6e6e6", track: "#2a2c30" },
    preview: "linear-gradient(180deg, #1c1d20 0 34%, #131314 34% 67%, #232427 67%)" },
  { id: "morning", name: "Morning", mode: "light", sidebar: "#e2f0fe", colors: ["#e6f2fd", "#f0f7fe", "#fcfeff", "#feffff", "#d3dbe4", "#0F1D33", "#3B5273", "#0066d6"],
    edge: { highlight: "rgba(255,255,255,.85)", ring: "rgba(255,255,255,.22)", shadow: "rgba(8,20,45,.14)", stage: "rgba(15,29,51,.13)" },
    surfaces: { card: "rgba(255,255,255,.82)", well: "rgba(215,232,248,.55)", inspector: "rgba(230,242,255,.52)" },
    ink: { keyword: "#005CC5", function: "#6F42C1", string: "#0a7753", type: "#ba3e0c", comment: "#5b6a7e", add: "#116a33", addBg: "rgba(52,199,89,.18)", del: "#b51b1b", delBg: "rgba(255,59,48,.15)", onAccent: "#ffffff" },
    div: "rgba(15,29,51,.08)", pill: "rgba(0,122,255,.1)",
    mark: { from: "#007AFF", to: "#6366F1", v: "#0F1D33", track: "rgba(15,29,51,.1)" },
    preview: "linear-gradient(180deg, #BFDDFB, #DCEBFA 48%, #FFE2CC)",
    sky: "radial-gradient(60% 55% at 10% 100%, rgba(255,209,179,.45), transparent 70%), radial-gradient(55% 50% at 90% 0%, rgba(166,216,255,.25), transparent 70%), linear-gradient(180deg, #BFDDFB 0%, #DCEBFA 48%, #FFE2CC 100%)",
    loaderSky: "radial-gradient(60% 55% at 18% 100%, rgba(255,209,179,.45), transparent 70%), radial-gradient(55% 50% at 86% 6%, rgba(166,216,255,.25), transparent 70%), linear-gradient(180deg, #BFDDFB 0%, #DCEBFA 48%, #FFE2CC 100%)",
    glass: { frame: "rgba(235,246,255,.65)", panel: "rgba(220,238,255,.45)", stage: "rgba(255,255,255,.4)" } },
  { id: "noon", name: "Noon", mode: "light", sidebar: "#dceffe", colors: ["#d6ecfd", "#eef7fe", "#feffff", "#ffffff", "#d1dbe4", "#0B1830", "#33496B", "#005bc7"],
    edge: { highlight: "rgba(255,255,255,.92)", ring: "rgba(255,255,255,.3)", shadow: "rgba(8,20,45,.14)", stage: "rgba(15,29,51,.13)" },
    surfaces: { card: "rgba(255,255,255,.94)", well: "rgba(222,236,250,.66)", inspector: "rgba(236,246,255,.6)" },
    ink: { keyword: "#005CC5", function: "#6F42C1", string: "#0a7954", type: "#bc3f0c", comment: "#5c6b80", add: "#116a33", addBg: "rgba(52,199,89,.18)", del: "#b71c1c", delBg: "rgba(255,59,48,.15)", onAccent: "#ffffff" },
    div: "rgba(15,29,51,.08)", pill: "rgba(0,102,224,.11)",
    mark: { from: "#0066E0", to: "#6366F1", v: "#0B1830", track: "rgba(15,29,51,.1)" },
    preview: "linear-gradient(180deg, #6DB6F7, #A9D6FB 52%, #E4F3FF)",
    sky: "radial-gradient(60% 55% at 10% 100%, rgba(255,244,214,.5), transparent 70%), radial-gradient(55% 50% at 90% 0%, rgba(120,196,255,.34), transparent 70%), linear-gradient(180deg, #6DB6F7 0%, #A9D6FB 52%, #E4F3FF 100%)",
    loaderSky: "radial-gradient(60% 55% at 18% 100%, rgba(255,244,214,.5), transparent 70%), radial-gradient(55% 50% at 86% 6%, rgba(120,196,255,.34), transparent 70%), linear-gradient(180deg, #6DB6F7 0%, #A9D6FB 52%, #E4F3FF 100%)",
    glass: { frame: "rgba(244,250,255,.6)", panel: "rgba(226,241,255,.5)", stage: "rgba(255,255,255,.58)" } },
  { id: "night", name: "Night", mode: "dark", sidebar: "#0e1325", colors: ["#0b0f1f", "#090c18", "#1a233d", "#1f2a49", "#222d42", "#EBF3FF", "#94A9C9", "#38BDF8"],
    edge: { highlight: "rgba(165,216,255,.32)", ring: "rgba(165,216,255,.05)", shadow: "rgba(0,0,0,.4)", stage: "rgba(148,197,255,.18)" },
    surfaces: { card: "rgba(34,46,78,.68)", well: "rgba(5,8,16,.78)", inspector: "rgba(14,20,38,.64)" },
    ink: { keyword: "#7DD3FC", function: "#C4B5FD", string: "#6EE7B7", type: "#FCD34D", comment: "#6c7b91", add: "#4ADE80", addBg: "rgba(48,209,88,.2)", del: "#F87171", delBg: "rgba(255,69,58,.2)", onAccent: "#06101E" },
    div: "rgba(148,197,255,.1)", pill: "rgba(56,189,248,.14)",
    mark: { from: "#38BDF8", to: "#818CF8", v: "#EBF3FF", track: "rgba(148,197,255,.14)" },
    preview: "linear-gradient(180deg, #04060E, #0A1024 55%, #141C44)",
    sky: "radial-gradient(60% 55% at 10% 100%, rgba(30,41,91,.55), transparent 70%), radial-gradient(55% 50% at 90% 0%, rgba(13,76,115,.3), transparent 70%), linear-gradient(180deg, #04060E 0%, #0A1024 55%, #141C44 100%)",
    loaderSky: "radial-gradient(60% 55% at 18% 100%, rgba(30,41,91,.55), transparent 70%), radial-gradient(55% 50% at 86% 6%, rgba(13,76,115,.3), transparent 70%), linear-gradient(180deg, #04060E 0%, #0A1024 55%, #141C44 100%)",
    glass: { frame: "rgba(11,15,29,.72)", panel: "rgba(16,22,42,.55)", stage: "rgba(7,10,20,.6)" } },
];

/** The Auto card: the day from morning to night. */
export const AUTO_PREVIEW = "linear-gradient(90deg, #FFE2CC, #BFDDFB 30%, #6DB6F7 55%, #141C44 80%, #04060E)";

export type Sky = "morning" | "noon" | "night";

/** The sky for a local time: morning 05:00-10:59, noon 11:00-18:59, night 19:00-04:59. */
export function skyFor(date: Date): Sky {
  const hour = date.getHours();
  if (hour >= 5 && hour < 11) return "morning";
  return hour >= 11 && hour < 19 ? "noon" : "night";
}

/** Resolves the `auto` palette to the sky for now; any other id is returned as is. */
export function resolveSky(palette: string, now: Date = new Date()): string {
  return palette === "auto" ? skyFor(now) : palette;
}

/** The palette to show: `auto` follows the clock, re-checked once a minute. The state only changes at a
 *  boundary, so the app repaints once there. */
export function useAutoSky(palette: string): string {
  const [resolved, setResolved] = useState(() => resolveSky(palette));
  useEffect(() => {
    setResolved(resolveSky(palette));
    if (palette !== "auto") return;
    const timer = window.setInterval(() => setResolved(resolveSky(palette)), 60_000);
    return () => window.clearInterval(timer);
  }, [palette]);
  return palette === "auto" ? resolved : palette;
}

const GLASS_BLUR = "blur(36px) saturate(190%)";

/** An unknown or retired id (the old "system" and the removed solid presets included) shows Classic. */
export function appearancePreset(id: string): AppearancePreset {
  return APPEARANCE_PRESETS.find(preset => preset.id === id) ?? APPEARANCE_PRESETS[0];
}

/** Blends a colour (rgba or hex) over an opaque hex, as the glass panels sit on the sky. */
export function compositeOver(fg: string, bg: string): string {
  const [r, g, b, a] = rgba(fg);
  const under = rgba(bg);
  return `#${[r, g, b].map((channel, i) => Math.round(channel * a + under[i] * (1 - a)).toString(16).padStart(2, "0")).join("")}`;
}

function rgba(colour: string): [number, number, number, number] {
  const hex = /^#([0-9a-f]{6})$/i.exec(colour.trim());
  if (hex) {
    const value = parseInt(hex[1], 16);
    return [(value >> 16) & 255, (value >> 8) & 255, value & 255, 1];
  }
  const parts = /^rgba?\(([^)]*)\)$/.exec(colour.trim())?.[1].split(/[\s,/]+/).filter(Boolean).map(Number) ?? [];
  if (parts.length < 3 || parts.some(Number.isNaN)) throw new Error(`Not a colour: ${colour}`);
  return [parts[0], parts[1], parts[2], parts[3] ?? 1];
}

function reducedTransparency(): boolean {
  return typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-transparency: reduce)").matches;
}

/** Real backdrop blur is an extra on macOS, the one platform where its frame time is measured (60 fps), and only
 *  where the engine has it. Every OS gets the same simulated glass: tint, ring, highlight, sheen and shadow. */
function backdropBlurAvailable(): boolean {
  const supports = typeof CSS !== "undefined" && typeof CSS.supports === "function"
    && (CSS.supports("backdrop-filter", "blur(1px)") || CSS.supports("-webkit-backdrop-filter", "blur(1px)"));
  return supports && /Macintosh|Mac OS X/.test(navigator.userAgent);
}

/** Glass is on for a sky on every OS, unless the person asked the system to reduce transparency: then the panels
 *  are solid (the `data-glass="off"` rule) and keep their ring and shadow. */
function applyGlass(root: HTMLElement, preset: AppearancePreset) {
  const glass = preset.glass;
  const on = glass !== undefined && !reducedTransparency();
  root.dataset.glass = on ? "on" : "off";
  if (!glass || !preset.sky) {
    ["--themis-sky", "--themis-loader-sky", "--themis-panel-bg", "--themis-stage-bg", "--themis-panel-blur"].forEach(token => root.style.removeProperty(token));
    return;
  }
  root.style.setProperty("--themis-sky", `linear-gradient(${glass.frame}, ${glass.frame}), ${preset.sky}`);
  root.style.setProperty("--themis-loader-sky", `linear-gradient(${glass.frame}, ${glass.frame}), ${preset.loaderSky ?? preset.sky}`);
  root.style.setProperty("--themis-panel-bg", glass.panel);
  root.style.setProperty("--themis-stage-bg", glass.stage);
  if (on && backdropBlurAvailable()) root.style.setProperty("--themis-panel-blur", GLASS_BLUR);
  else root.style.removeProperty("--themis-panel-blur");
}

const MARK_TOKENS = [["mark-from", "from"], ["mark-to", "to"], ["mark-v", "v"], ["mark-track", "track"]] as const;
const SURFACE_TOKENS = [["card", "card"], ["well", "well"], ["inspector-bg", "inspector"]] as const;
const INK_TOKENS = [
  ["code-keyword", "keyword"], ["code-function", "function"], ["code-string", "string"], ["code-type", "type"], ["code-comment", "comment"],
  ["diff-add", "add"], ["add-bg", "addBg"], ["diff-del", "del"], ["del-bg", "delBg"], ["accent-ink", "onAccent"],
] as const;

/** The chat's card and well, the Workspace panel, and the code and diff ink (canvas --cardh, --well, --insp, --kw…). */
function applySurfaces(root: HTMLElement, preset: AppearancePreset) {
  SURFACE_TOKENS.forEach(([token, key]) => root.style.setProperty(`--themis-${token}`, preset.surfaces[key]));
  INK_TOKENS.forEach(([token, key]) => root.style.setProperty(`--themis-${token}`, preset.ink[key]));
  MARK_TOKENS.forEach(([token, key]) => root.style.setProperty(`--themis-${token}`, preset.mark[key]));
}

const PALETTE_KEY = "themis:sky";

/** The look the person chose last time (a sky, Auto or Classic), so the startup screen wears it before the settings
 *  arrive; Auto (the clock's sky) when nothing is remembered. A per-viewer convenience: storage may be unavailable. */
export function rememberedPalette(): string {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(PALETTE_KEY) ?? "null");
    return typeof value === "string" ? value : "auto";
  } catch { return "auto"; }
}

export function rememberPalette(palette: string) {
  try { localStorage.setItem(PALETTE_KEY, JSON.stringify(palette)); } catch { /* Storage may be unavailable. */ }
}

function applyEdge(root: HTMLElement, preset: AppearancePreset) {
  root.style.setProperty("--themis-panel-highlight", preset.edge.highlight);
  root.style.setProperty("--themis-panel-ring", preset.edge.ring);
  root.style.setProperty("--themis-panel-shadow", preset.edge.shadow);
  root.style.setProperty("--themis-stage-edge", preset.edge.stage);
}

export function applyAppearance(settings: Appearance) {
  const root = document.documentElement;
  const preset = appearancePreset(settings.theme_palette);
  root.dataset.theme = preset.mode;
  root.dataset.palette = preset.id;
  root.dataset.font = settings.font_family;
  // Body text is .8333rem, so the setting is its size in pixels.
  root.style.fontSize = `${settings.text_size * 1.2}px`;
  ["shell-bg", "canvas", "surface", "raised", "border", "text", "text-2", "accent"].forEach((token, i) => {
    root.style.setProperty(`--themis-${token}`, preset.colors[i]);
  });
  root.style.setProperty("--themis-sidebar-bg", preset.sidebar);
  root.style.setProperty("--themis-text-3", preset.colors[6]);
  root.style.setProperty("--themis-border-subtle", preset.div);
  root.style.setProperty("--themis-pill", preset.pill);
  root.style.setProperty("--themis-accent-hover", preset.colors[7]);
  applyEdge(root, preset);
  applySurfaces(root, preset);
  applyGlass(root, preset);
}

/** Applies the appearance, and again when the system's reduced-transparency choice changes. */
export function useApplyAppearance(settings: Appearance) {
  const { theme_palette, font_family, text_size } = settings;
  useEffect(() => {
    const apply = () => applyAppearance({ theme_palette, font_family, text_size });
    apply();
    const query = typeof window.matchMedia === "function" ? window.matchMedia("(prefers-reduced-transparency: reduce)") : null;
    query?.addEventListener?.("change", apply);
    return () => query?.removeEventListener?.("change", apply);
  }, [theme_palette, font_family, text_size]);
}
