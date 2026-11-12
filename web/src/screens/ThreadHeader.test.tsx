// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { existsSync, readFileSync } from "node:fs";
import { expect, it, vi } from "vitest";
import type { ThreadInfo } from "../lib/types";
import { ThreadHeader } from "./ThreadHeader";

const thread = { id: "t", title: "Fix", provider: "go", model: "beam-free", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] } as unknown as ThreadInfo;

function header(changedFiles?: number, onToggleReview = vi.fn()) {
  return render(<ThreadHeader thread={thread} models={[]} effortLevels={["low", "high"]} effort="high" busy={false} switching={false}
    onProviderChange={vi.fn()} onEffortChange={vi.fn()} changedFiles={changedFiles} reviewOpen={false} onToggleReview={onToggleReview} />);
}

it("shows only the model pill on the left, as the canvas's `beam-free · high ⌄`", () => {
  header();
  const pill = screen.getByLabelText("Model beam-free, reasoning effort high");
  expect(pill).toHaveTextContent(/^beam-free · high$/);
  expect(screen.queryByText("Themis")).toBeNull();
});

it("shows Workspace as its icon and the changed-file count, named in full for assistive tech and in its tooltip", () => {
  const onToggle = vi.fn();
  header(2, onToggle);
  const toggle = screen.getByRole("button", { name: "Workspace, 2 files changed" });
  expect(toggle).toHaveTextContent(/^2$/);
  expect(toggle).toHaveAttribute("title", "Workspace");
  fireEvent.click(toggle);
  expect(onToggle).toHaveBeenCalled();
});

it("names a single change and no change plainly", () => {
  const { unmount } = header(1);
  expect(screen.getByRole("button", { name: "Workspace, 1 file changed" })).toHaveTextContent(/^1$/);
  unmount();
  header(0);
  expect(screen.getByRole("button", { name: "Workspace" })).toHaveTextContent(/^$/);
});

it("has no overview card floating over the chat: the header count and the Workspace replace it", () => {
  expect(existsSync("src/screens/OverviewBox.tsx")).toBe(false);
  expect(readFileSync("src/screens/ThreadConversation.tsx", "utf8")).not.toMatch(/Overview/);
});
