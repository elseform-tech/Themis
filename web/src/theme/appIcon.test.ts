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

// macOS draws app icons on Apple's template: an 824×824 body centred on 1024 (100px margin), radius about 185, a
// soft shadow in the margin. The other platforms keep the full tile.
it("gives macOS its own padded icon (icns, iconset and the runtime Dock icon), and the other platforms the full tile", () => {
  const svg = readFileSync(`${icons}/icon-macos.svg`, "utf8");
  expect(svg).toContain('<rect x="100" y="100" width="824" height="824" rx="185" fill="#0B0F1D" filter="url(#shadow)"/>');
  expect(svg).toContain('<g transform="translate(100 100) scale(4.12)">');
  const fullBleed = readFileSync(`${icons}/icon-source.png`);
  const icns = readFileSync(`${icons}/icon.icns`);
  expect(icns.subarray(0, 4).toString("ascii")).toBe("icns");
  expect(icns.includes(fullBleed)).toBe(false);
  // The iconset's largest image is the padded one, not the full tile; the Windows/Linux PNGs keep the full tile.
  expect(readFileSync(`${icons}/Themis.iconset/icon_512x512@2x.png`).equals(fullBleed)).toBe(false);
  expect(existsSync(`${icons}/icon.ico`) && existsSync(`${icons}/128x128@2x.png`)).toBe(true);
  // The Dock icon the app sets at run time on macOS is the padded one too.
  const lib = readFileSync("../crates/desktop/src/lib.rs", "utf8");
  expect(lib).toMatch(/include_bytes!\(\s*"..\/icons\/Themis.iconset\/icon_128x128@2x.png"\s*\)/);
  expect(JSON.parse(readFileSync("../crates/desktop/tauri.conf.json", "utf8")).bundle.icon).toContain("icons/icon.icns");
});
