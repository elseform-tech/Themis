import { expect, it } from "vitest";
import { readFileSync } from "node:fs";

const styles = readFileSync(new URL("./ThreadView.css", import.meta.url), "utf8");

it("keeps completed replies static while a run is active", () => {
  expect(styles).toMatch(/\.themis-thread-msg--completed \.themis-response > p\s*\{[^}]*animation:\s*none[^}]*background:\s*none[^}]*color:\s*var\(--themis-text\)/s);
});

it("never shimmers the agent's own text: progress lines read like its answers while their step runs", () => {
  expect(styles).not.toMatch(/\.themis-thread-msg--action-waiting \.themis-response > p/);
  expect(styles).not.toMatch(/\.themis-thread-msg--live \.themis-response > p/);
});

it("keeps inline code readable inside shimmering status text", () => {
  expect(styles).toMatch(/\.themis-shimmer :not\(pre\) > code[^{]*\{[^}]*color:\s*var\(--themis-text\)/s);
});

it("compacts the composer's labels to icons when the chat is narrow", () => {
  expect(styles).toMatch(/\.themis-thread-composer\s*\{\s*container:\s*themis-composer \/ inline-size;\s*\}/);
  expect(styles).toMatch(/@container themis-composer \(max-width: 380px\)\s*\{\s*\.themis-thread-composer \.themis-composer-label\s*\{\s*display:\s*none;/);
});

it("shrinks the header's Workspace button before the model name when the chat is narrow", () => {
  expect(styles).toMatch(/\.themis-thread-head\s*\{\s*container:\s*themis-head \/ inline-size;\s*\}/);
  expect(styles).toMatch(/@container themis-head \(max-width: 460px\)\s*\{\s*\.themis-thread-head \.themis-head-label\s*\{\s*display:\s*none;/);
  expect(styles).toMatch(/\.themis-model-pill-meta\s*\{\s*min-width:\s*6ch;/);
});

it("puts the goal sentence on its own line when the chat is too narrow to show it beside the controls", () => {
  const goal = readFileSync(new URL("./GoalBar.css", import.meta.url), "utf8");
  expect(goal).toMatch(/@container themis-composer \(max-width: 380px\)\s*\{\s*\.themis-goal-bar:not\(\.is-editing\)\s*\{[^}]*flex-wrap:\s*wrap/);
  expect(goal).toMatch(/\.themis-goal-bar:not\(\.is-editing\) \.themis-goal-text\s*\{[^}]*flex-basis:\s*100%/);
});
