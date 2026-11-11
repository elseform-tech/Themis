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
  font: Appearance["font_family"];
  sidebar: string;
  colors: readonly [string, string, string, string, string, string, string, string];
  /** Sky presets: the background layers behind the frame (glows, then the sky gradient). */
  sky?: string;
  /** Sky presets: translucent frame tint, panel and stage, drawn over the sky. */
  glass?: { frame: string; panel: string; stage: string };
};

// Frame, canvas, surface, raised, border, text, secondary text, accent.
// Solid presets: the sidebar sits between the frame and canvas in brightness.
// Skies: the colours are opaque stand-ins for the glass over the sky's middle stop.
export const APPEARANCE_PRESETS: readonly AppearancePreset[] = [
  { id: "classic", name: "Classic", mode: "dark", font: "system", sidebar: "#1c1d20", colors: ["#232427", "#131314", "#1f2023", "#282a2d", "#34363b", "#e6e6e6", "#a8abb2", "#d9a73e"] },
  { id: "morning", name: "Morning", mode: "light", font: "system", sidebar: "#e2f0fe", colors: ["#e6f2fd", "#f0f7fe", "#f8fbfe", "#fcfeff", "#d3dbe4", "#0f1d33", "#3b5273", "#005fc7"],
    sky: "radial-gradient(60% 55% at 10% 100%, rgba(255,209,179,.45), transparent 70%), radial-gradient(55% 50% at 90% 0%, rgba(166,216,255,.25), transparent 70%), linear-gradient(180deg, #BFDDFB 0%, #DCEBFA 48%, #FFE2CC 100%)",
    glass: { frame: "rgba(235,246,255,.65)", panel: "rgba(220,238,255,.45)", stage: "rgba(255,255,255,.4)" } },
  { id: "noon", name: "Noon", mode: "light", font: "system", sidebar: "#dceffe", colors: ["#d6ecfd", "#eef7fe", "#f8fcff", "#feffff", "#d1dbe4", "#0b1830", "#33496b", "#0052b3"],
    sky: "radial-gradient(60% 55% at 10% 100%, rgba(255,244,214,.5), transparent 70%), radial-gradient(55% 50% at 90% 0%, rgba(120,196,255,.34), transparent 70%), linear-gradient(180deg, #6DB6F7 0%, #A9D6FB 52%, #E4F3FF 100%)",
    glass: { frame: "rgba(244,250,255,.6)", panel: "rgba(226,241,255,.5)", stage: "rgba(255,255,255,.58)" } },
  { id: "night", name: "Night", mode: "dark", font: "system", sidebar: "#0e1325", colors: ["#0b0f1f", "#090c18", "#131a2e", "#1a233d", "#222d42", "#ebf3ff", "#94a9c9", "#38bdf8"],
    sky: "radial-gradient(60% 55% at 10% 100%, rgba(30,41,91,.55), transparent 70%), radial-gradient(55% 50% at 90% 0%, rgba(13,76,115,.3), transparent 70%), linear-gradient(180deg, #04060E 0%, #0A1024 55%, #141C44 100%)",
    glass: { frame: "rgba(11,15,29,.72)", panel: "rgba(16,22,42,.55)", stage: "rgba(7,10,20,.6)" } },
  { id: "github-dark", name: "GitHub Dark", mode: "dark", font: "system", sidebar: "#161b22", colors: ["#21262d", "#010409", "#161b22", "#21262d", "#30363d", "#e6edf3", "#9da7b3", "#79c0ff"] },
  { id: "github-dimmed", name: "GitHub Dimmed", mode: "dark", font: "system", sidebar: "#2d333b", colors: ["#373e47", "#1c2128", "#2d333b", "#373e47", "#444c56", "#d1d9e0", "#a1acb9", "#8ddbff"] },
  { id: "midnight", name: "Midnight", mode: "dark", font: "sans", sidebar: "#121a2d", colors: ["#1c2942", "#0b1020", "#121a2d", "#1c2942", "#33435e", "#e2e9f6", "#a4b3cb", "#a6b9ff"] },
  { id: "nordic", name: "Nordic", mode: "dark", font: "system", sidebar: "#2e3440", colors: ["#3b4252", "#242933", "#2e3440", "#3b4252", "#4c566a", "#eceff4", "#b2bdce", "#88c0d0"] },
  { id: "graphite", name: "Graphite", mode: "dark", font: "system", sidebar: "#1e1e1e", colors: ["#2b2b2b", "#151515", "#1e1e1e", "#2b2b2b", "#414141", "#eeeeee", "#aaaaaa", "#d4d4d4"] },
  { id: "ink", name: "Ink", mode: "dark", font: "serif", sidebar: "#191d29", colors: ["#252c3d", "#10121a", "#191d29", "#252c3d", "#394158", "#e4e8f5", "#a6aec6", "#9aaeff"] },
  { id: "forest", name: "Forest", mode: "dark", font: "system", sidebar: "#18271f", colors: ["#243b2c", "#101a16", "#18271f", "#243b2c", "#3e5746", "#e2eee5", "#a6bdae", "#92d9a8"] },
  { id: "copper", name: "Copper", mode: "dark", font: "serif", sidebar: "#2a211b", colors: ["#3b2d24", "#1d1714", "#2a211b", "#3b2d24", "#594336", "#f2e6db", "#c2ad9b", "#e8b18a"] },
  { id: "plum", name: "Plum", mode: "dark", font: "rounded", sidebar: "#291e32", colors: ["#392943", "#1c1423", "#291e32", "#392943", "#543b61", "#f0e3f5", "#c3a9cd", "#d3a2ec"] },
  { id: "ocean", name: "Ocean", mode: "dark", font: "sans", sidebar: "#152733", colors: ["#203b4c", "#0e1922", "#152733", "#203b4c", "#36556a", "#e0eef5", "#9fbcca", "#7dcced"] },
  { id: "rosewood", name: "Rosewood", mode: "dark", font: "serif", sidebar: "#2e2025", colors: ["#422d34", "#211519", "#2e2025", "#422d34", "#5d4049", "#f4e4e9", "#c9abb6", "#e7a6b9"] },
  { id: "olive", name: "Olive", mode: "dark", font: "system", sidebar: "#23281c", colors: ["#343b29", "#181b13", "#23281c", "#343b29", "#4c583d", "#e9eedc", "#b3bda0", "#bfce85"] },
  { id: "amethyst", name: "Amethyst", mode: "dark", font: "rounded", sidebar: "#241f3b", colors: ["#332c51", "#17142b", "#241f3b", "#332c51", "#4f4572", "#eee8ff", "#b9abd7", "#baa4f4"] },
  { id: "terminal", name: "Terminal", mode: "dark", font: "mono", sidebar: "#15221a", colors: ["#21372a", "#0c1710", "#15221a", "#21372a", "#365443", "#dcefe3", "#a0bca8", "#87dfa2"] },
  { id: "sandstone", name: "Sandstone", mode: "dark", font: "serif", sidebar: "#2c261f", colors: ["#3f362c", "#201c17", "#2c261f", "#3f362c", "#594c3d", "#f1eadd", "#c4b59e", "#e1c18b"] },
  { id: "carbon", name: "Carbon", mode: "dark", font: "system", sidebar: "#1a2228", colors: ["#28343d", "#111518", "#1a2228", "#28343d", "#3c4f5c", "#e0eaf0", "#a5bac6", "#9dc5df"] },
];

const GLASS_BLUR = "blur(36px) saturate(190%)";

/** An unknown or retired id (the old "system" included) shows Classic. */
export function appearancePreset(id: string): AppearancePreset {
  const legacy = { warm: "copper", violet: "amethyst" };
  const wanted = legacy[id as keyof typeof legacy] ?? id;
  return APPEARANCE_PRESETS.find(preset => preset.id === wanted) ?? APPEARANCE_PRESETS[0];
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

/** Blur costs frame time: off where the engine lacks it, and on Linux (WebKitGTK) until its frame time is measured. */
function glassBlurAvailable(): boolean {
  const supports = typeof CSS !== "undefined" && typeof CSS.supports === "function"
    && (CSS.supports("backdrop-filter", "blur(1px)") || CSS.supports("-webkit-backdrop-filter", "blur(1px)"));
  return supports && !navigator.userAgent.includes("Linux");
}

function applyGlass(root: HTMLElement, preset: AppearancePreset) {
  const glass = preset.glass;
  root.dataset.glass = glass && glassBlurAvailable() ? "on" : "off";
  if (!glass || !preset.sky) {
    ["--themis-sky", "--themis-panel-bg", "--themis-stage-bg", "--themis-panel-blur"].forEach(token => root.style.removeProperty(token));
    return;
  }
  root.style.setProperty("--themis-sky", `linear-gradient(${glass.frame}, ${glass.frame}), ${preset.sky}`);
  root.style.setProperty("--themis-panel-bg", glass.panel);
  root.style.setProperty("--themis-stage-bg", glass.stage);
  root.style.setProperty("--themis-panel-blur", GLASS_BLUR);
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
  applyGlass(root, preset);
}
