// @vitest-environment jsdom
import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import type { Goal } from "../lib/types";
import { GoalView } from "./GoalView";

afterEach(cleanup);

const goal = (proof: Goal["proof"]): Goal => ({ text: "Make the game", status: "running", started_at: 0, active_ms: 0, turns: 1, stalls: 0, failures: 0, proof });

it("walks the tasks in order and marks the one being worked on", () => {
  render(<GoalView goal={goal([
    { what: "Clicking starts the game", proven: true, how: "a real click locked the pointer" },
    { what: "Health drops", proven: false },
    { what: "Score rises", proven: false },
  ])} />);
  const tasks = screen.getByRole("list", { name: "Tasks, 1 of 3 complete" });
  const items = within(tasks).getAllByRole("listitem");
  expect(items.map(item => item.querySelector("span.themis-goal-what")?.textContent)).toEqual(["Clicking starts the game", "Health drops", "Score rises"]);
  expect(items[1]).toHaveAttribute("aria-current", "step");
  expect(within(items[1]).getByText("Now")).toBeInTheDocument();
  expect(items[2]).not.toHaveAttribute("aria-current");
  expect(within(items[0]).getByText("a real click locked the pointer")).toBeInTheDocument();
  expect(screen.getByText("Make the game")).toBeInTheDocument();
  expect(screen.queryByText(/proven/i)).not.toBeInTheDocument();
});

it("doesn't repeat why a task further along the trail isn't done", () => {
  render(<GoalView goal={goal([
    { what: "Health drops", proven: false, how: "no measured change" },
    { what: "Score rises", proven: false, how: "not started" },
  ])} />);
  expect(screen.getByText("no measured change")).toBeInTheDocument();
  expect(screen.queryByText("not started")).not.toBeInTheDocument();
});

it("says Next rather than Now while the goal isn't running", () => {
  render(<GoalView goal={{ ...goal([{ what: "Health drops", proven: false }]), status: "paused" } as Goal} />);
  expect(screen.getByText("Next")).toBeInTheDocument();
  expect(screen.queryByText("Now")).not.toBeInTheDocument();
});

it("keeps rules apart from the tasks, never ticked, and says plainly when one was broken", () => {
  render(<GoalView goal={goal([
    { what: "Board works", proven: true },
    { what: "No build step", proven: false, kind: "rule" },
    { what: "Subagents build the parts", proven: false, kind: "rule", broken: true, how: "the main agent wrote them all" },
  ])} />);
  expect(screen.getByRole("list", { name: "Tasks, 1 of 1 complete" })).not.toHaveTextContent("No build step");
  const rules = screen.getByRole("list", { name: "Rules" });
  expect(within(rules).getByText("No build step")).toBeInTheDocument();
  const broken = within(rules).getByText("Subagents build the parts").closest("li") as HTMLElement;
  expect(within(broken).getByText("Not followed: the main agent wrote them all")).toBeInTheDocument();
  expect(within(rules).getAllByText(/^Not followed/)).toHaveLength(1);
});

it("has no rules section without rules, and no current task when all are done", () => {
  render(<GoalView goal={goal([{ what: "Board works", proven: true }])} />);
  expect(screen.queryByRole("list", { name: "Rules" })).not.toBeInTheDocument();
  expect(document.querySelector("[aria-current]")).toBeNull();
});

it("says so before the checklist has come through, and without a goal", () => {
  const { rerender } = render(<GoalView goal={goal(undefined)} />);
  expect(screen.getByText("The checklist will appear here in a moment.")).toBeInTheDocument();
  rerender(<GoalView goal={null} />);
  expect(screen.getByText("This chat has no goal.")).toBeInTheDocument();
});
