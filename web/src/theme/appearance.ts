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
  /** The card in Settings › Appearance. */
  preview: string;
  /** Skies: the background layers behind the frame (glows, then the sky gradient). */
  sky?: string;
  /** Skies: translucent frame tint, panel and stage, drawn over the sky. */
  glass?: { frame: string; panel: string; stage: string };
};

// Frame, canvas, surface, raised, border, text, secondary text, accent.
// The frame sits furthest back; the sidebar and the stage (canvas) stand off it as lit panels.
// Skies: the colours are opaque stand-ins for the glass over the sky's middle stop (the solid panels under reduced transparency).
export const APPEARANCE_PRESETS: readonly AppearancePreset[] = [
  { id: "classic", name: "Classic", mode: "dark", sidebar: "#262729", colors: ["#0e0f10", "#212225", "#2a2b2f", "#323438", "#3a3c41", "#e6e6e6", "#a8abb2", "#d9a73e"],
    edge: { highlight: "rgba(255,255,255,.05)", ring: "transparent", shadow: "rgba(0,0,0,.28)", stage: "#34363b" },
    preview: "linear-gradient(180deg, #262729 0 34%, #212225 34% 67%, #0e0f10 67%)" },
  { id: "morning", name: "Morning", mode: "light", sidebar: "#ebf3fc", colors: ["#c3dbf6", "#f5f9fe", "#fbfdff", "#ffffff", "#d3dbe4", "#0f1d33", "#33496b", "#0052ab"],
    edge: { highlight: "rgba(255,255,255,.85)", ring: "rgba(255,255,255,.22)", shadow: "rgba(8,20,45,.14)", stage: "rgba(15,29,51,.13)" },
    preview: "linear-gradient(180deg, #A9CFF7, #BED8F5 50%, #FFD6B8)",
    sky: "radial-gradient(60% 55% at 10% 100%, rgba(255,190,150,.5), transparent 70%), radial-gradient(55% 50% at 90% 0%, rgba(166,216,255,.3), transparent 70%), linear-gradient(180deg, #A9CFF7 0%, #BED8F5 48%, #FFD6B8 100%)",
    glass: { frame: "rgba(235,246,255,.1)", panel: "rgba(255,255,255,.66)", stage: "rgba(255,255,255,.84)" } },
  { id: "noon", name: "Noon", mode: "light", sidebar: "#e2f1fd", colors: ["#b2dafb", "#eef7fe", "#f8fcff", "#feffff", "#d1dbe4", "#0b1830", "#24406a", "#004494"],
    edge: { highlight: "rgba(255,255,255,.92)", ring: "rgba(255,255,255,.3)", shadow: "rgba(8,20,45,.14)", stage: "rgba(15,29,51,.13)" },
    preview: "linear-gradient(180deg, #6DB6F7, #A9D6FB 55%, #E4F3FF)",
    sky: "radial-gradient(60% 55% at 10% 100%, rgba(255,244,214,.5), transparent 70%), radial-gradient(55% 50% at 90% 0%, rgba(120,196,255,.34), transparent 70%), linear-gradient(180deg, #6DB6F7 0%, #A9D6FB 52%, #E4F3FF 100%)",
    glass: { frame: "rgba(244,250,255,.12)", panel: "rgba(255,255,255,.62)", stage: "rgba(255,255,255,.78)" } },
  { id: "night", name: "Night", mode: "dark", sidebar: "#18223e", colors: ["#080d1c", "#192340", "#202b4d", "#283459", "#34405e", "#ebf3ff", "#a3b6d4", "#38bdf8"],
    edge: { highlight: "rgba(165,216,255,.32)", ring: "rgba(165,216,255,.05)", shadow: "rgba(0,0,0,.4)", stage: "rgba(148,197,255,.18)" },
    preview: "linear-gradient(180deg, #04060E, #0A1024 55%, #1B2558)",
    sky: "radial-gradient(60% 55% at 10% 100%, rgba(30,41,91,.55), transparent 70%), radial-gradient(55% 50% at 90% 0%, rgba(13,76,115,.3), transparent 70%), linear-gradient(180deg, #04060E 0%, #0A1024 55%, #141C44 100%)",
    glass: { frame: "rgba(4,6,14,.35)", panel: "rgba(40,54,96,.5)", stage: "rgba(32,43,78,.72)" } },
];

/** The Auto card: the day from morning to night. */
export const AUTO_PREVIEW = "linear-gradient(90deg, #FFD6B8, #A9CFF7 30%, #6DB6F7 55%, #141C44 80%, #04060E)";

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
    ["--themis-sky", "--themis-panel-bg", "--themis-stage-bg", "--themis-panel-blur"].forEach(token => root.style.removeProperty(token));
    return;
  }
  root.style.setProperty("--themis-sky", `linear-gradient(${glass.frame}, ${glass.frame}), ${preset.sky}`);
  root.style.setProperty("--themis-panel-bg", glass.panel);
  root.style.setProperty("--themis-stage-bg", glass.stage);
  if (on && backdropBlurAvailable()) root.style.setProperty("--themis-panel-blur", GLASS_BLUR);
  else root.style.removeProperty("--themis-panel-blur");
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
  root.style.setProperty("--themis-border-subtle", preset.colors[4]);
  root.style.setProperty("--themis-accent-ink", preset.colors[0]);
  root.style.setProperty("--themis-accent-hover", preset.colors[7]);
  applyEdge(root, preset);
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
