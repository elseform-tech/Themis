// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import type { Goal, TodoItem } from "../lib/types";
import { sortTodos, TodoWindow, todoWindow } from "./TodoList";
import { TodoView } from "./TodoView";

afterEach(cleanup);

const todoCss = readFileSync("src/screens/TodoList.css", "utf8");

const goal = (rules?: Goal["rules"], text = "Build Trailboard"): Goal => ({ text, status: "running", started_at: 0, active_ms: 0, turns: 1, stalls: 0, failures: 0, rules });
const items: TodoItem[] = [
  { id: "1", title: "Read the docs and plan", status: "completed", outcome: "Plan in PLAN.md" },
  { id: "2", title: "Fix drag in Safari", status: "in_progress", details: "Drag drops the card one column left in Safari." },
  { id: "3", title: "Add CSV export", status: "pending", source: "person", details: "Can you also add a CSV export?" },
  { id: "4", title: "Verify the UI in the browser", status: "pending", reopened: "Import with a wrong file was never tried." },
  { id: "5", title: "Cover the timeline with tests", status: "pending", source: "check" },
  { id: "6", title: "Write the README", status: "pending" },
];
const todo = { revision: 4, items };

it("shows the goal, a tally and the whole list", () => {
  render(<TodoView goal={goal()} todo={todo} />);
  expect(screen.getByText("Build Trailboard")).toBeInTheDocument();
  expect(document.querySelector(".themis-todo-tally")).toHaveTextContent("1 of 6 done");
  const list = screen.getByRole("list", { name: "To-do" });
  expect(within(list).getAllByRole("listitem")).toHaveLength(6);
  // Status is said in words too, never by colour or a mark alone.
  expect(within(list).getByText("Read the docs and plan").closest("li")).toHaveTextContent(/^Done: Read the docs and plan/);
  expect(within(list).getByText("Fix drag in Safari").closest("li")).toHaveTextContent(/^In progress: Fix drag in Safari/);
});

it("lists the item in progress first, then pending ones in plan order, finished ones last", () => {
  const mixed: TodoItem[] = [
    { id: "a", title: "A", status: "completed" },
    { id: "b", title: "B", status: "pending" },
    { id: "c", title: "C", status: "completed", closed: "cancelled" },
    { id: "d", title: "D", status: "in_progress" },
    { id: "e", title: "E", status: "pending" },
    { id: "f", title: "F", status: "completed", closed: "confirmed" },
  ];
  expect(sortTodos(mixed).map(item => item.id)).toEqual(["d", "b", "e", "a", "c", "f"]);
  render(<TodoView goal={null} todo={{ revision: 1, items: mixed }} />);
  const titles = within(screen.getByRole("list", { name: "To-do" })).getAllByRole("listitem").map(row => row.querySelector(".themis-todo-title")!.textContent);
  expect(titles).toEqual(["D", "B", "E", "A", "C", "F"]);
});

it("spins only on the current item and fades finished ones, keeping their tags, with no tick or strike-through", () => {
  render(<TodoView goal={null} todo={todo} />);
  const rows = within(screen.getByRole("list", { name: "To-do" })).getAllByRole("listitem");
  // Sorted: in progress, the four pending in plan order, then the finished one.
  expect(rows.map(row => row.querySelector(".themis-todo-title")!.textContent)).toEqual([
    "Fix drag in Safari", "Add CSV export", "Verify the UI in the browser", "Cover the timeline with tests", "Write the README", "Read the docs and plan",
  ]);
  expect(rows.map(row => row.querySelector(".themis-dot-spinner") !== null)).toEqual([true, false, false, false, false, false]);
  expect(rows[5]).toHaveClass("is-done");
  expect(rows[5]!.querySelector("svg")).toBeNull();
  expect(rows[1]!.querySelector(".themis-todo-pending")).not.toBeNull();
  // Faded, never crossed off: the stylesheet has no line-through left.
  expect(todoCss).not.toMatch(/line-through/);
  expect(todoCss).toMatch(/\.themis-todo-item\.is-done\s*\{[^}]*opacity:\s*\.55/);
});

it("tags requests from the person, the check's additions and reopened items in words", () => {
  render(<TodoView goal={goal()} todo={todo} />);
  expect(screen.getByText("Add CSV export").closest("li")).toHaveTextContent("· you asked");
  expect(screen.getByText("Cover the timeline with tests").closest("li")).toHaveTextContent("· added by the check");
  expect(screen.getByText("Verify the UI in the browser").closest("li")).toHaveTextContent("· reopened by the check");
  expect(screen.getByText("Write the README").closest("li")).not.toHaveTextContent("·");
});

it("tags an item seeded from the goal subtly", () => {
  const seeded = { revision: 1, items: [{ id: "1", title: "CSV export downloads every task", status: "pending" as const, source: "goal" as const }] };
  render(<TodoView goal={goal()} todo={seeded} />);
  const item = screen.getByText("CSV export downloads every task").closest("li");
  expect(item).toHaveTextContent("· from the goal");
  expect(item?.querySelector(".themis-todo-tag")).toHaveTextContent("from the goal");
});

it("opens an item to show its details, outcome and why the check reopened it", () => {
  render(<TodoView goal={goal()} todo={todo} />);
  const done = screen.getByRole("button", { name: /Read the docs and plan/ });
  expect(done).toHaveAttribute("aria-expanded", "false");
  expect(screen.queryByText("Plan in PLAN.md")).toBeNull();
  fireEvent.click(done);
  expect(done).toHaveAttribute("aria-expanded", "true");
  expect(screen.getByText("Plan in PLAN.md")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: /Add CSV export/ }));
  expect(screen.getByText("Can you also add a CSV export?")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: /Verify the UI in the browser/ }));
  expect(screen.getByText("Import with a wrong file was never tried.")).toBeInTheDocument();
  fireEvent.click(done);
  expect(screen.queryByText("Plan in PLAN.md")).toBeNull();
  // An item with nothing more to show is not a button.
  expect(screen.queryByRole("button", { name: /Write the README/ })).toBeNull();
});

it("says when the check, not the agent, finished or closed an item, and why", () => {
  const closed: TodoItem[] = [
    { id: "t1", title: "Build the budget table", status: "completed", source: "person", closed: "confirmed", outcome: "table test: 1 passed" },
    { id: "t2", title: "Add a currency selector", status: "completed", source: "person", closed: "cancelled" },
    { id: "t3", title: "localStorage or sessionStorage?", status: "completed", source: "person", closed: "answered" },
    { id: "t4", title: "Write the README", status: "completed" },
  ];
  render(<TodoView goal={goal()} todo={{ revision: 2, items: closed }} />);
  const row = (title: string) => screen.getByText(title).closest("li");
  expect(row("Build the budget table")).toHaveTextContent(/^Done: Build the budget table · you asked · confirmed by the check$/);
  expect(row("Add a currency selector")).toHaveTextContent(/^Closed: Add a currency selector · you asked · cancelled by your later request$/);
  expect(row("localStorage or sessionStorage?")).toHaveTextContent(/^Closed: localStorage or sessionStorage\? · you asked · answered$/);
  expect(row("Write the README")).not.toHaveTextContent("check");
  for (const title of ["Build the budget table", "Add a currency selector", "localStorage or sessionStorage?"]) expect(row(title)).toHaveClass("is-done");
  // The check's outcome opens like the agent's.
  fireEvent.click(screen.getByRole("button", { name: /Build the budget table/ }));
  expect(screen.getByText("table test: 1 passed")).toBeInTheDocument();
  // The overview box counts closed items with the finished ones; only open items get rows.
  cleanup();
  render(<TodoWindow items={[closed[1]!, closed[2]!, { id: "t5", title: "Polish", status: "pending" }]} />);
  expect([...document.querySelectorAll(".themis-todo-item")].map(row => row.textContent)).toEqual(["Polish"]);
  expect(document.querySelector(".themis-todo-more")).toHaveTextContent("2 done");
});

it("lists the goal's rules read-only, only when it has some", () => {
  const { rerender } = render(<TodoView goal={goal([
    { what: "No build step" },
    { what: "Subagents build the parts", broken: true, how: "the main agent wrote them all" },
    { what: "Code review before finishing", pending: true },
  ])} todo={todo} />);
  const rules = screen.getByRole("list", { name: "Rules" });
  expect(within(rules).getByText("Subagents build the parts").closest("li")).toHaveTextContent("Not followed: the main agent wrote them all");
  expect(within(rules).getAllByText(/^Not followed/)).toHaveLength(1);
  expect(within(rules).getAllByText("Still to do")).toHaveLength(1);
  expect(within(rules).queryByRole("button")).toBeNull();
  rerender(<TodoView goal={goal([])} todo={todo} />);
  expect(screen.queryByRole("list", { name: "Rules" })).toBeNull();
});

it("shows no goal text in a normal chat, and folds a long goal", () => {
  const { rerender } = render(<TodoView goal={null} todo={todo} />);
  expect(document.querySelector(".themis-todo-goal")).toBeNull();
  expect(screen.queryByRole("list", { name: "Rules" })).toBeNull();
  rerender(<TodoView goal={goal(undefined, "Build it ".repeat(40))} todo={todo} />);
  const more = screen.getByRole("button", { name: "More" });
  expect(document.querySelector(".themis-todo-goal-text")).toHaveClass("is-folded");
  fireEvent.click(more);
  expect(document.querySelector(".themis-todo-goal-text")).not.toHaveClass("is-folded");
});

it("says so while a goal has no list yet", () => {
  render(<TodoView goal={goal()} todo={{ revision: 0, items: [] }} />);
  expect(screen.queryByRole("list", { name: "To-do" })).toBeNull();
  expect(document.querySelector(".themis-todo-tally")).toBeNull();
  expect(screen.getByText("The agent hasn't written its list yet.")).toBeInTheDocument();
});

it("keeps the whole finished list in the tab", () => {
  const done = items.map(item => ({ ...item, status: "completed" as const }));
  render(<TodoView goal={null} todo={{ revision: 5, items: done }} />);
  expect(within(screen.getByRole("list", { name: "To-do" })).getAllByRole("listitem")).toHaveLength(6);
});

it("the overview window accounts for every item of every list up to eight long", () => {
  const statuses = ["pending", "in_progress", "completed"] as const;
  for (let length = 0; length <= 8; length++) {
    for (let code = 0; code < 3 ** length; code++) {
      const list: TodoItem[] = Array.from({ length }, (_, index) => ({ id: `${index}`, title: `Item ${index}`, status: statuses[Math.floor(code / 3 ** index) % 3]! }));
      const { shown, more, done, allDone } = todoWindow(list);
      const finished = list.filter(item => item.status === "completed");
      const open = list.filter(item => item.status !== "completed");
      expect(allDone).toBe(length > 0 && open.length === 0);
      expect(done).toBe(finished.length);
      if (allDone) {
        expect(shown).toHaveLength(0);
        expect(more).toBe(0);
        continue;
      }
      // Only open items get rows: at most five, in progress first, then pending in plan order.
      expect(shown.length).toBe(Math.min(5, open.length));
      expect(shown).toEqual(sortTodos(open).slice(0, 5));
      expect(shown.length + more + done).toBe(length);
    }
  }
});

describe("reordering", () => {
  // jsdom has no layout: a row's top is its place among its siblings.
  const rect = vi.spyOn(Element.prototype, "getBoundingClientRect");
  const motion = (reduce: boolean) => { window.matchMedia = vi.fn().mockReturnValue({ matches: reduce, addEventListener: vi.fn(), removeEventListener: vi.fn() }); };
  beforeEach(() => {
    rect.mockImplementation(function (this: Element) {
      const at = this.parentElement ? [...this.parentElement.children].indexOf(this) : 0;
      return { top: at * 30, bottom: at * 30 + 30, left: 0, right: 100, width: 100, height: 30, x: 0, y: at * 30, toJSON: () => ({}) } as DOMRect;
    });
  });
  afterEach(() => { rect.mockReset(); });
  const first: TodoItem[] = [
    { id: "a", title: "A", status: "in_progress" },
    { id: "b", title: "B", status: "pending" },
    { id: "c", title: "C", status: "pending" },
  ];
  // A finishes and B starts: A sinks to the bottom, B and C rise.
  const next: TodoItem[] = [
    { id: "a", title: "A", status: "completed" },
    { id: "b", title: "B", status: "in_progress" },
    { id: "c", title: "C", status: "pending" },
  ];
  const shift = (title: string) => (screen.getByText(title).closest("[data-key]") as HTMLElement).style.transform;

  it("slides moved items from where they were in the tab", () => {
    motion(false);
    const { rerender } = render(<TodoView goal={null} todo={{ revision: 1, items: first }} />);
    rerender(<TodoView goal={null} todo={{ revision: 2, items: next }} />);
    expect(shift("A")).toBe("translateY(-60px)");
    expect(shift("B")).toBe("translateY(30px)");
    expect(shift("C")).toBe("translateY(30px)");
  });

  it("slides rows in the overview box too", () => {
    motion(false);
    const { rerender } = render(<TodoWindow items={first} />);
    // The agent starts B and puts A back to pending: B rises above A.
    rerender(<TodoWindow items={[{ id: "a", title: "A", status: "pending" }, next[1]!, next[2]!]} />);
    expect(shift("B")).toBe("translateY(30px)");
    expect(shift("A")).toBe("translateY(-30px)");
    expect(shift("C")).toBe("");
  });

  it("moves without animation under reduced motion", () => {
    motion(true);
    const { rerender } = render(<TodoView goal={null} todo={{ revision: 1, items: first }} />);
    rerender(<TodoView goal={null} todo={{ revision: 2, items: next }} />);
    expect(shift("A")).toBe("");
    expect(shift("B")).toBe("");
    expect(screen.getAllByRole("listitem").map(row => row.querySelector(".themis-todo-title")!.textContent)).toEqual(["B", "C", "A"]);
  });
});
