// @vitest-environment jsdom
import { expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { appearancePreset, applyAppearance } from "./appearance";

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

it("appearancePreset falls back to classic for unknown ids", () => {
  expect(appearancePreset("nonsense").id).toBe("classic");
  expect(appearancePreset("system").id).toBe("classic");
  expect(appearancePreset("noon").id).toBe("noon");
  expect(appearancePreset("classic").name).toBe("Classic");
});

it("sets light or dark from the preset and paints the sky behind the frame", () => {
  const root = document.documentElement;
  withBrowser(true, "Macintosh", () => {
    applyAppearance({ ...base, theme_palette: "morning" });
    expect(root.dataset.theme).toBe("light");
    expect(root.dataset.glass).toBe("on");
    const morning = appearancePreset("morning");
    expect(root.style.getPropertyValue("--themis-sky")).toContain("#BFDDFB");
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

it("glass falls back to solid panels when backdrop-filter is unsupported", () => {
  withBrowser(false, "Macintosh", () => {
    applyAppearance({ ...base, theme_palette: "noon" });
    expect(document.documentElement.dataset.glass).toBe("off");
  });
  const tokens = readFileSync("src/theme/tokens.css", "utf8");
  expect(tokens).toMatch(/\[data-glass="off"\][^{]*\{[^}]*--themis-panel-bg: var\(--themis-sidebar-bg\) !important/);
});

it("glass is off on Linux until its frame time is measured", () => {
  withBrowser(true, "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15", () => {
    applyAppearance({ ...base, theme_palette: "night" });
    expect(document.documentElement.dataset.glass).toBe("off");
  });
});
