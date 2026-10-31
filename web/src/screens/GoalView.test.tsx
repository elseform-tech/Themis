// @vitest-environment jsdom
import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import type { Goal } from "../lib/types";
import { GoalView } from "./GoalView";

afterEach(cleanup);

const goal = (proof: Goal["proof"]): Goal => ({ text: "Make the game", status: "running", started_at: 0, active_ms: 0, turns: 1, stalls: 0, failures: 0, proof });

it("sorts the checklist into Pending and Complete columns", () => {
  render(<GoalView goal={goal([
    { what: "Clicking starts the game", proven: true, how: "a real click locked the pointer" },
    { what: "Health drops", proven: false },
  ])} />);
  const pending = screen.getByRole("list", { name: "Pending, 1" });
  const complete = screen.getByRole("list", { name: "Complete, 1" });
  expect(within(pending).getByText("Health drops")).toBeInTheDocument();
  expect(within(complete).getByText("Clicking starts the game")).toBeInTheDocument();
  expect(within(complete).getByText("a real click locked the pointer")).toBeInTheDocument();
  expect(screen.getByText("Make the game")).toBeInTheDocument();
  expect(screen.queryByText(/proven/i)).not.toBeInTheDocument();
});

it("says so before the first check has listed anything, and without a goal", () => {
  const { rerender } = render(<GoalView goal={goal(undefined)} />);
  expect(screen.getByText("The checklist appears after the first check.")).toBeInTheDocument();
  rerender(<GoalView goal={null} />);
  expect(screen.getByText("This chat has no goal.")).toBeInTheDocument();
});
