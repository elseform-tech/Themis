import type { Settings } from "../lib/types";

export type Appearance = Pick<Settings, "theme_palette" | "font_family" | "text_size">;

export const FONT_OPTIONS = [
  ["system", "System"], ["sans", "Arial"], ["serif", "Georgia"], ["mono", "Mono"],
  ["rounded", "Rounded"], ["avenir", "Avenir"], ["helvetica", "Helvetica"],
  ["verdana", "Verdana"], ["trebuchet", "Trebuchet"], ["palatino", "Palatino"],
  ["charter", "Charter"], ["menlo", "Menlo"],
] as const;

// Shell, canvas, surface, raised, border, text, secondary text, accent.
export const APPEARANCE_PRESETS = [
  { id: "system", name: "Themis", font: "system", colors: ["#181a20", "#22242b", "#262830", "#30323b", "#3b3d46", "#e8eaf0", "#a1aaba", "#d9a73e"] },
  { id: "github-dark", name: "GitHub Dark", font: "system", colors: ["#010409", "#0d1117", "#161b22", "#21262d", "#30363d", "#e6edf3", "#9da7b3", "#79c0ff"] },
  { id: "github-dimmed", name: "GitHub Dimmed", font: "system", colors: ["#1c2128", "#22272e", "#2d333b", "#373e47", "#444c56", "#d1d9e0", "#a1acb9", "#8ddbff"] },
  { id: "midnight", name: "Midnight", font: "sans", colors: ["#0b1020", "#10182a", "#121a2d", "#1c2942", "#33435e", "#e2e9f6", "#a4b3cb", "#a6b9ff"] },
  { id: "nordic", name: "Nordic", font: "system", colors: ["#242933", "#292f3a", "#2e3440", "#3b4252", "#4c566a", "#eceff4", "#b2bdce", "#88c0d0"] },
  { id: "graphite", name: "Graphite", font: "system", colors: ["#151515", "#191919", "#1e1e1e", "#2b2b2b", "#414141", "#eeeeee", "#aaaaaa", "#d4d4d4"] },
  { id: "ink", name: "Ink", font: "serif", colors: ["#10121a", "#141822", "#191d29", "#252c3d", "#394158", "#e4e8f5", "#a6aec6", "#9aaeff"] },
  { id: "forest", name: "Forest", font: "system", colors: ["#101a16", "#14211a", "#18271f", "#243b2c", "#3e5746", "#e2eee5", "#a6bdae", "#92d9a8"] },
  { id: "copper", name: "Copper", font: "serif", colors: ["#1d1714", "#231b17", "#2a211b", "#3b2d24", "#594336", "#f2e6db", "#c2ad9b", "#e8b18a"] },
  { id: "plum", name: "Plum", font: "rounded", colors: ["#1c1423", "#221929", "#291e32", "#392943", "#543b61", "#f0e3f5", "#c3a9cd", "#d3a2ec"] },
  { id: "ocean", name: "Ocean", font: "sans", colors: ["#0e1922", "#11212b", "#152733", "#203b4c", "#36556a", "#e0eef5", "#9fbcca", "#7dcced"] },
  { id: "rosewood", name: "Rosewood", font: "serif", colors: ["#211519", "#281a20", "#2e2025", "#422d34", "#5d4049", "#f4e4e9", "#c9abb6", "#e7a6b9"] },
  { id: "olive", name: "Olive", font: "system", colors: ["#181b13", "#1d2217", "#23281c", "#343b29", "#4c583d", "#e9eedc", "#b3bda0", "#bfce85"] },
  { id: "amethyst", name: "Amethyst", font: "rounded", colors: ["#17142b", "#1d1933", "#241f3b", "#332c51", "#4f4572", "#eee8ff", "#b9abd7", "#baa4f4"] },
  { id: "terminal", name: "Terminal", font: "mono", colors: ["#0c1710", "#101d15", "#15221a", "#21372a", "#365443", "#dcefe3", "#a0bca8", "#87dfa2"] },
  { id: "sandstone", name: "Sandstone", font: "serif", colors: ["#201c17", "#26211b", "#2c261f", "#3f362c", "#594c3d", "#f1eadd", "#c4b59e", "#e1c18b"] },
  { id: "carbon", name: "Carbon", font: "system", colors: ["#111518", "#161b20", "#1a2228", "#28343d", "#3c4f5c", "#e0eaf0", "#a5bac6", "#9dc5df"] },
] as const;

export function appearancePreset(id: string) {
  const legacy = { warm: "copper", violet: "amethyst" };
  return APPEARANCE_PRESETS.find(preset => preset.id === (legacy[id as keyof typeof legacy] ?? id)) ?? APPEARANCE_PRESETS[0];
}

export function applyAppearance(settings: Appearance) {
  const root = document.documentElement;
  const preset = appearancePreset(settings.theme_palette);
  root.dataset.theme = "dark";
  root.dataset.palette = preset.id;
  root.dataset.font = settings.font_family;
  root.style.fontSize = `${16 * settings.text_size / 14}px`;
  ["shell-bg", "canvas", "surface", "raised", "border", "text", "text-2", "accent"].forEach((token, i) => {
    root.style.setProperty(`--themis-${token}`, preset.colors[i]);
  });
  root.style.setProperty("--themis-text-3", preset.colors[6]);
  root.style.setProperty("--themis-border-subtle", preset.colors[4]);
  root.style.setProperty("--themis-accent-ink", preset.colors[0]);
  root.style.setProperty("--themis-accent-hover", preset.colors[7]);
}
