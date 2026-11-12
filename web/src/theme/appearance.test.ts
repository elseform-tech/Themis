// @vitest-environment jsdom
import { expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { createElement, useEffect } from "react";
import { act, render } from "@testing-library/react";
import { appearancePreset, applyAppearance, skyFor, useAutoSky } from "./appearance";

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

it("glass is off when the system asks to reduce transparency", () => {
  const asked: string[] = [];
  withBrowser(true, "Macintosh", () => {
    vi.stubGlobal("matchMedia", (query: string) => { asked.push(query); return { matches: query === "(prefers-reduced-transparency: reduce)" }; });
    applyAppearance({ ...base, theme_palette: "morning" });
    expect(document.documentElement.dataset.glass).toBe("off");
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
