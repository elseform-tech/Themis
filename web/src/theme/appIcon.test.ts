// @vitest-environment node
import { existsSync, readFileSync } from "node:fs";
import { expect, it } from "vitest";

// The Themis Visions mark (canvas Logo board): the app icon is its dark tile, the favicon the same file.
const icons = "../crates/desktop/icons";

it("bundles the canvas's dark app icon: a #0B0F1D tile, the T in the Night gradient, the V in #EBF3FF at .9", () => {
  const svg = readFileSync(`${icons}/icon.svg`, "utf8");
  expect(svg).toContain('<rect width="200" height="200" rx="36" fill="#0B0F1D"/>');
  expect(svg).toMatch(/stop-color="#38BDF8"[\s\S]*stop-color="#818CF8"/);
  expect(svg).toContain('<path d="M32 44H168L154 68H112V124L100 144L88 124V68H46Z" fill="url(#tg-dark)"/>');
  expect(svg).toContain('<path d="M39 58L63 58L100 122L137 58L161 58L100 164Z" fill="#EBF3FF" fill-opacity=".9"/>');
  // The light tile is kept for later, unused: a bundle takes one icon.
  expect(readFileSync(`${icons}/icon-light.svg`, "utf8")).toContain('fill="#F4F8FD"');
  expect(readFileSync("../crates/desktop/tauri.conf.json", "utf8")).not.toContain("icon-light");
});

it("points the bundle at icon files that exist, and the favicon at the same mark", () => {
  const config = JSON.parse(readFileSync("../crates/desktop/tauri.conf.json", "utf8")) as { bundle: { icon: string[] } };
  expect(config.bundle.icon).toEqual(["icons/32x32.png", "icons/128x128.png", "icons/128x128@2x.png", "icons/icon.icns"]);
  for (const file of config.bundle.icon) expect(existsSync(`../crates/desktop/${file}`), file).toBe(true);
  expect(readFileSync("index.html", "utf8")).toContain('<link rel="icon" type="image/svg+xml" href="/src/assets/themis-visions.svg" />');
  expect(readFileSync("src/assets/themis-visions.svg", "utf8")).toBe(readFileSync(`${icons}/icon.svg`, "utf8"));
  expect(existsSync("src/assets/knight-trio.svg")).toBe(false);
});
