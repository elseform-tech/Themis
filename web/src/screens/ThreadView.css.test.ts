import { expect, it } from "vitest";
import { readFileSync } from "node:fs";

const styles = readFileSync(new URL("./ThreadView.css", import.meta.url), "utf8");

it("keeps completed replies static while a run is active", () => {
  expect(styles).toMatch(/\.themis-thread-msg--completed \.themis-response > p\s*\{[^}]*animation:\s*none[^}]*background:\s*none[^}]*color:\s*var\(--themis-text\)/s);
});

it("does not shimmer streamed replies when no tool is pending", () => {
  expect(styles).toMatch(/\.themis-thread-msg--action-waiting \.themis-response > p/);
  expect(styles).not.toMatch(/\.themis-thread-msg--live \.themis-response > p/);
});
