// @vitest-environment jsdom
import { expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { createElement, useEffect } from "react";
import { act, render } from "@testing-library/react";
import { APPEARANCE_PRESETS, appearancePreset, applyAppearance, skyFor, useApplyAppearance, useAutoSky } from "./appearance";

it("sets body text to the chosen size, and no label below 9px even at the smallest size", () => {
  const shell = readFileSync("src/screens/shell.css", "utf8");
  expect(shell).toMatch(/\.themis-shell \{[^}]*font-size: \.8333rem/);
  expect(shell).toContain("-webkit-font-smoothing: antialiased");
  const source = readFileSync("src/theme/appearance.ts", "utf8");
  expect(source).toContain("settings.text_size * 1.2");
  // At 10, a rem is 12px: .75rem is 9px.
  expect(12 * .8333).toBeCloseTo(10, 0);
  for (const file of ["src/components/ResponseBody.css", "src/screens/WorkspacePanel.css", "src/components/diff/FileTree.css"]) {
    const smallest = [...readFileSync(file, "utf8").matchAll(/font-size:\s*0?(\.\d+)rem/g)].map(found => Number(found[1]));
    expect(Math.min(...smallest) * 12).toBeGreaterThanOrEqual(9);
  }
});

const base = { font_family: "system", text_size: 12 } as const;

function withBrowser(supports: boolean, userAgent: string, run: () => void) {
  const original = globalThis.CSS;
  const agent = vi.spyOn(navigator, "userAgent", "get").mockReturnValue(userAgent);
  vi.stubGlobal("CSS", { ...original, supports: () => supports });
  try { run(); } finally { vi.unstubAllGlobals(); agent.mockRestore(); }
}

it("appearancePreset falls back to classic for unknown and removed ids", () => {
  expect(appearancePreset("nonsense").id).toBe("classic");
  expect(appearancePreset("system").id).toBe("classic");
  expect(appearancePreset("ocean").id).toBe("classic");
  expect(appearancePreset("warm").id).toBe("classic");
  expect(appearancePreset("noon").id).toBe("noon");
  expect(appearancePreset("classic").name).toBe("Classic");
});

it("ships only Classic and the three skies", () => {
  expect(APPEARANCE_PRESETS.map(preset => preset.id)).toEqual(["classic", "morning", "noon", "night"]);
});

it("sets light or dark from the preset and paints the sky behind the frame", () => {
  const root = document.documentElement;
  withBrowser(true, "Macintosh", () => {
    applyAppearance({ ...base, theme_palette: "morning" });
    expect(root.dataset.theme).toBe("light");
    expect(root.dataset.glass).toBe("on");
    const morning = appearancePreset("morning");
    expect(root.style.getPropertyValue("--themis-sky")).toContain("#BFDDFB 0%, #DCEBFA 48%, #FFE2CC 100%");
    expect(root.style.getPropertyValue("--themis-loader-sky")).toContain("at 18% 100%");
    expect(root.style.getPropertyValue("--themis-panel-bg")).toBe(morning.glass?.panel);
    expect(root.style.getPropertyValue("--themis-stage-bg")).toBe(morning.glass?.stage);
    expect(root.style.getPropertyValue("--themis-panel-blur")).toBe("blur(36px) saturate(190%)");
    applyAppearance({ ...base, theme_palette: "night" });
    expect(root.dataset.theme).toBe("dark");
    applyAppearance({ ...base, theme_palette: "classic" });
    expect(root.dataset.theme).toBe("dark");
    expect(root.style.getPropertyValue("--themis-sky")).toBe("");
    expect(root.style.getPropertyValue("--themis-panel-bg")).toBe("");
    expect(root.style.getPropertyValue("--themis-panel-blur")).toBe("");
  });
});

it("every preset sets the panel ring, highlight, shadow and stage edge, and the panels draw them", () => {
  const root = document.documentElement;
  for (const preset of APPEARANCE_PRESETS) {
    applyAppearance({ ...base, theme_palette: preset.id });
    expect(root.style.getPropertyValue("--themis-panel-highlight")).toBe(preset.edge.highlight);
    expect(root.style.getPropertyValue("--themis-panel-ring")).toBe(preset.edge.ring);
    expect(root.style.getPropertyValue("--themis-panel-shadow")).toBe(preset.edge.shadow);
    expect(root.style.getPropertyValue("--themis-stage-edge")).toBe(preset.edge.stage);
  }
  const tokens = readFileSync("src/theme/tokens.css", "utf8");
  expect(tokens).toContain("--themis-panel-sheen: linear-gradient(140deg, var(--themis-panel-highlight) -45%, transparent 28%)");
  expect(tokens).toContain("--themis-panel-edge: inset 0 1px 0 var(--themis-panel-highlight), inset 0 0 0 1px var(--themis-panel-ring), 0 10px 36px var(--themis-panel-shadow)");
  expect(tokens).toContain("--themis-stage-edge-shadow: inset 0 0 0 1px var(--themis-stage-edge), inset 0 1px 0 var(--themis-panel-highlight)");
  const panels: Array<[string, string]> = [
    ["src/screens/Sidebar.css", ".themis-sidebar {"],
    ["src/screens/WorkspacePanel.css", ".themis-workspace-panel {"],
    ["src/screens/Sidebar.css", ".themis-settings-nav {"],
  ];
  for (const [file, selector] of panels) {
    const rule = readFileSync(file, "utf8").split(selector)[1].split("}")[0];
    // The Workspace panel is the canvas's inspector: its own tint, the same sheen and edge.
    expect(rule, file).toContain(`background: var(--themis-panel-sheen), var(--themis-${file.includes("Workspace") ? "inspector-bg" : "panel-bg"})`);
    expect(rule, file).toContain("box-shadow: var(--themis-panel-edge)");
  }
  const stages: Array<[string, string]> = [
    ["src/screens/shell.css", ".themis-main { margin"],
    ["src/screens/ThreadView.css", ".themis-thread-layout.is-reviewing > .themis-thread {"],
  ];
  for (const [file, selector] of stages) {
    const rule = readFileSync(file, "utf8").split(selector)[1].split("}")[0];
    expect(rule, file).toContain("background: var(--themis-stage-bg)");
    expect(rule, file).toContain("box-shadow: var(--themis-stage-edge-shadow)");
  }
});

it("glass falls back to the same simulated glass without blur when backdrop-filter is unsupported", () => {
  withBrowser(false, "Macintosh", () => {
    applyAppearance({ ...base, theme_palette: "noon" });
    expect(document.documentElement.dataset.glass).toBe("on");
    expect(document.documentElement.style.getPropertyValue("--themis-panel-blur")).toBe("");
  });
  const tokens = readFileSync("src/theme/tokens.css", "utf8");
  expect(tokens).toMatch(/\[data-glass="off"\][^{]*\{[^}]*--themis-panel-bg: var\(--themis-sidebar-bg\) !important/);
});

it("glass is on without blur on Linux", () => {
  withBrowser(true, "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15", () => {
    applyAppearance({ ...base, theme_palette: "night" });
    expect(document.documentElement.dataset.glass).toBe("on");
    expect(document.documentElement.style.getPropertyValue("--themis-panel-bg")).toBe(appearancePreset("night").glass?.panel);
    expect(document.documentElement.style.getPropertyValue("--themis-panel-blur")).toBe("");
  });
});

it("glass is on without blur on Windows", () => {
  withBrowser(true, "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Edg/120.0", () => {
    applyAppearance({ ...base, theme_palette: "night" });
    expect(document.documentElement.dataset.glass).toBe("on");
    expect(document.documentElement.style.getPropertyValue("--themis-panel-blur")).toBe("");
  });
});

it("glass is off, with ring and shadow kept, when the system asks to reduce transparency", () => {
  const asked: string[] = [];
  withBrowser(true, "Macintosh", () => {
    vi.stubGlobal("matchMedia", (query: string) => { asked.push(query); return { matches: query === "(prefers-reduced-transparency: reduce)" }; });
    applyAppearance({ ...base, theme_palette: "morning" });
    expect(document.documentElement.dataset.glass).toBe("off");
    expect(document.documentElement.style.getPropertyValue("--themis-panel-blur")).toBe("");
    expect(document.documentElement.style.getPropertyValue("--themis-panel-ring")).toBe(appearancePreset("morning").edge.ring);
    expect(document.documentElement.style.getPropertyValue("--themis-panel-shadow")).toBe(appearancePreset("morning").edge.shadow);
  });
  expect(asked).toContain("(prefers-reduced-transparency: reduce)");
});

it("skyFor maps the boundaries", () => {
  const at = (h: number, m: number) => skyFor(new Date(2026, 10, 12, h, m));
  expect([at(4, 59), at(5, 0), at(10, 59), at(11, 0), at(18, 59), at(19, 0)]).toEqual(["night", "morning", "morning", "noon", "noon", "night"]);
});

it("auto switches once at the boundary", () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(2026, 10, 12, 18, 58));
  const apply = vi.fn();
  function Harness() {
    const palette = useAutoSky("auto");
    useEffect(() => { apply(palette); }, [palette]);
    return null;
  }
  try {
    render(createElement(Harness));
    act(() => { vi.advanceTimersByTime(3 * 60_000); });
    expect(apply.mock.calls.filter(([id]) => id === "night")).toHaveLength(1);
    expect(apply.mock.calls.map(([id]) => id)).toEqual(["noon", "night"]);
  } finally { vi.useRealTimers(); }
});

it("re-applies the appearance when the reduced-transparency setting changes", () => {
  let reduced = false;
  const listeners: Array<() => void> = [];
  const agent = vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Macintosh");
  vi.stubGlobal("CSS", { supports: () => true });
  vi.stubGlobal("matchMedia", () => ({
    get matches() { return reduced; },
    addEventListener: (_: string, fn: () => void) => { listeners.push(fn); },
    removeEventListener: (_: string, fn: () => void) => { listeners.splice(listeners.indexOf(fn), 1); },
  }));
  function Harness() { useApplyAppearance({ ...base, theme_palette: "morning" }); return null; }
  try {
    const view = render(createElement(Harness));
    expect(document.documentElement.dataset.glass).toBe("on");
    reduced = true;
    act(() => listeners.forEach(fn => fn()));
    expect(document.documentElement.dataset.glass).toBe("off");
    view.unmount();
    expect(listeners).toHaveLength(0);
  } finally { vi.unstubAllGlobals(); agent.mockRestore(); }
});

it("every preset sets the chat and Workspace surfaces and the code ink from the canvas tokens", () => {
  const root = document.documentElement;
  for (const preset of APPEARANCE_PRESETS) {
    applyAppearance({ ...base, theme_palette: preset.id });
    const expected: Array<[string, string]> = [
      ["--themis-card", preset.surfaces.card], ["--themis-well", preset.surfaces.well], ["--themis-inspector-bg", preset.surfaces.inspector],
      ["--themis-code-keyword", preset.ink.keyword], ["--themis-code-function", preset.ink.function], ["--themis-code-string", preset.ink.string],
      ["--themis-code-type", preset.ink.type], ["--themis-code-comment", preset.ink.comment], ["--themis-diff-add", preset.ink.add],
      ["--themis-add-bg", preset.ink.addBg], ["--themis-diff-del", preset.ink.del], ["--themis-del-bg", preset.ink.delBg], ["--themis-accent-ink", preset.ink.onAccent],
    ];
    for (const [token, value] of expected) expect(root.style.getPropertyValue(token), `${preset.id} ${token}`).toBe(value);
  }
  // Under reduced transparency the Workspace panel is solid like the sidebar.
  const tokens = readFileSync("src/theme/tokens.css", "utf8");
  expect(tokens).toMatch(/\[data-glass="off"\][^{]*\{[^}]*--themis-inspector-bg: var\(--themis-sidebar-bg\) !important/);
});
