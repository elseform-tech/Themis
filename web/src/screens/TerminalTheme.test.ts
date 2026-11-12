// @vitest-environment jsdom
import { expect, it } from "vitest";
import { APPEARANCE_PRESETS } from "../theme/appearance";
import { LIGHT_ANSI } from "./TerminalView";

function luminance(hex: string): number {
  const v = parseInt(hex.slice(1), 16);
  return [(v >> 16) & 255, (v >> 8) & 255, v & 255]
    .map(c => { const s = c / 255; return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4; })
    .reduce((sum, c, i) => sum + c * [0.2126, 0.7152, 0.0722][i], 0);
}
function ratio(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

it("every ANSI colour of the light terminal theme reads at 4.5:1 on each light canvas", () => {
  const canvases = APPEARANCE_PRESETS.filter(preset => preset.mode === "light").map(preset => preset.colors[1]);
  expect(canvases.length).toBeGreaterThan(0);
  expect(Object.keys(LIGHT_ANSI)).toHaveLength(16);
  for (const canvas of canvases) {
    for (const [name, colour] of Object.entries(LIGHT_ANSI)) {
      expect(ratio(colour, canvas), `${name} on ${canvas}`).toBeGreaterThanOrEqual(4.5);
    }
  }
});
