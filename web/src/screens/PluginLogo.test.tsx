// @vitest-environment jsdom
import { readFileSync, statSync } from "node:fs";
import { cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import brandLogos from "../lib/brandLogos.json";
import { pluginAction } from "../lib/tauri";
import type { PluginSpec } from "../lib/types";
import { PluginLogo } from "./PluginLogo";

vi.mock("../lib/tauri", () => ({ pluginAction: vi.fn() }));
afterEach(() => { cleanup(); vi.mocked(pluginAction).mockReset(); });
const spec = (extra: Partial<PluginSpec>) => ({ name: "a", files: {}, ...extra }) as unknown as PluginSpec;

describe("Plugin logos", () => {
  it("prefers the plugin's own saved icon, without any network request", async () => {
    vi.mocked(pluginAction).mockResolvedValue("data:image/png;base64,AAAA" as never);
    const { container } = render(<PluginLogo spec={spec({ icon_asset: "abc.png", icon: "https://example.com/i.png" })} brand="stripe" name="pay" />);
    await waitFor(() => expect(container.querySelector("img")).toHaveAttribute("src", "data:image/png;base64,AAAA"));
    expect(pluginAction).toHaveBeenCalledWith({ action: "plugin_icon", asset: "abc.png" });
    expect(container.innerHTML).not.toContain("example.com");
  });
  it("never loads an icon URL that wasn't saved during a check", () => {
    const { container } = render(<PluginLogo spec={spec({ icon: "https://example.com/i.png" })} name="pay" />);
    expect(container.querySelector("img")).toBeNull();
    expect(container).toHaveTextContent("P");
  });
  it("falls back to a brand logo, then to a tile with the initial", () => {
    const logo = render(<PluginLogo brand="stripe" name="pay" />);
    expect(logo.container.querySelector("svg path")).toHaveAttribute("d", brandLogos.stripe.path);
    expect(logo.container).not.toHaveTextContent("P");
    logo.unmount();
    const tile = render(<PluginLogo brand="not-a-brand" name="pay" />);
    expect(tile.container.querySelector("svg")).toBeNull();
    expect(tile.container).toHaveTextContent("P");
  });
  it("keeps very dark marks visible on the neutral tile", () => {
    const dark = Object.entries(brandLogos).find(([, logo]) => logo.hex === "000000")!;
    const { container } = render(<PluginLogo brand={dark[0]} name="x" />);
    expect((container.firstChild as HTMLElement).style.background).toBe("");
  });
  it("keeps the bundled set small and every brand mapped to a logo", () => {
    expect(statSync("src/lib/brandLogos.json").size).toBeLessThan(300 * 1024);
    const brands = JSON.parse(readFileSync("../crates/core/builtins/plugin-brands.json", "utf8")) as Record<string, Record<string, string>>;
    const mapped = [...Object.values(brands.authors), ...Object.values(brands.owners)];
    expect(mapped.length).toBeGreaterThan(0);
    for (const slug of mapped) expect(brandLogos).toHaveProperty(slug);
  });
});
