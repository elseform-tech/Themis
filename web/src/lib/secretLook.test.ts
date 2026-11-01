import { describe, expect, it, vi } from "vitest";
import samples from "./secretSamples.json";
import { looksLikeSecret } from "./secretLook";

describe("looksLikeSecret", () => {
  it.each(samples.secrets.map(pieces => [pieces.join("")]))("finds %s", text => {
    expect(looksLikeSecret(text)).toBe(true);
  });

  it.each(samples.ordinary.map(text => [text]))("leaves %s alone", text => {
    expect(looksLikeSecret(text)).toBe(false);
  });

  it("does not depend on where in the message the key is", () => {
    const key = samples.secrets[4].join("");
    expect(looksLikeSecret(`first line\n\nhere it is: ${key}\nthanks`)).toBe(true);
  });

  it("copes with hostile and huge input quickly", () => {
    const started = performance.now();
    expect(looksLikeSecret("KEY=".repeat(50_000))).toBe(false);
    expect(looksLikeSecret("-".repeat(1_000_000))).toBe(false);
    expect(looksLikeSecret("sk-".repeat(100_000))).toBe(false);
    expect(looksLikeSecret(`${"a".repeat(300_000)}.eyJ`)).toBe(false);
    expect(performance.now() - started).toBeLessThan(2000);
  });

  it("never logs what it sees", () => {
    const logged = (["log", "info", "warn", "error", "debug"] as const).map(method => vi.spyOn(console, method));
    looksLikeSecret(samples.secrets[0].join(""));
    for (const spy of logged) expect(spy).not.toHaveBeenCalled();
    vi.restoreAllMocks();
  });
});
