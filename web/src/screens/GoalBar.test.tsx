// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Goal, QueuedMessage } from "../lib/types";
import { GoalBar, formatElapsed, goalElapsedMs } from "./GoalBar";
import { QueueStack } from "./QueueStack";
import { GoalNote } from "./ThreadConversation";

afterEach(() => { cleanup(); vi.useRealTimers(); try { window.localStorage?.clear(); } catch { /* no storage in this environment */ } });

const goal = (overrides: Partial<Goal> = {}): Goal => ({
  text: "Optimize this app to bring p95 under 200 ms", status: "running", started_at: 0,
  active_ms: 3_600_000, running_since: 1_000, turns: 3, stalls: 0, failures: 0, ...overrides,
});

describe("GoalBar", () => {
  it("shows the sentence and the elapsed time, ticking only while it runs", () => {
    vi.useFakeTimers();
    vi.setSystemTime(1_000 + 24_000);
    render(<GoalBar goal={goal()} onEdit={vi.fn()} onPause={vi.fn()} onResume={vi.fn()} onRemove={vi.fn()} />);
    expect(screen.getByRole("region", { name: "Goal" })).toHaveTextContent("Optimize this app to bring p95 under 200 ms");
    expect(screen.getByText("1:00:24")).toBeInTheDocument();
    act(() => { vi.advanceTimersByTime(2000); });
    expect(screen.getByText("1:00:26")).toBeInTheDocument();
    expect(formatElapsed(goalElapsedMs(goal({ running_since: undefined, active_ms: 59_999 }), 0))).toBe("0:00:59");
  });

  it("offers the sentence, edit, pause and more while running, and resume when paused", () => {
    const pause = vi.fn(), resume = vi.fn();
    const { rerender } = render(<GoalBar goal={goal()} onEdit={vi.fn()} onPause={pause} onResume={resume} onRemove={vi.fn()} />);
    expect(screen.getAllByRole("button").map(button => button.getAttribute("aria-label") ?? button.textContent)).toEqual(["Optimize this app to bring p95 under 200 ms", "Edit goal", "Pause goal after this turn", "More goal actions"]);
    fireEvent.click(screen.getByRole("button", { name: "Pause goal after this turn" }));
    expect(pause).toHaveBeenCalled();
    rerender(<GoalBar goal={goal({ status: "quota", running_since: undefined })} onEdit={vi.fn()} onPause={pause} onResume={resume} onRemove={vi.fn()} />);
    expect(screen.getByText("Paused · quota used up")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Resume goal" }));
    expect(resume).toHaveBeenCalled();
    // Every icon button names itself for the tooltip.
    expect(screen.getByRole("button", { name: "Edit goal" })).toHaveAttribute("data-tooltip", "Edit goal");
  });

  it("removes the goal from its More menu", () => {
    const remove = vi.fn();
    render(<GoalBar goal={goal({ status: "paused", running_since: undefined })} onEdit={vi.fn()} onPause={vi.fn()} onResume={vi.fn()} onRemove={remove} />);
    expect(screen.queryByRole("menuitem", { name: "Remove goal" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "More goal actions" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Remove goal" }));
    expect(remove).toHaveBeenCalled();
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("leaves the composer once the goal is reached", () => {
    const { container, rerender } = render(<GoalBar goal={goal()} onEdit={vi.fn()} onPause={vi.fn()} onResume={vi.fn()} onRemove={vi.fn()} />);
    expect(screen.getByRole("region", { name: "Goal" })).toBeInTheDocument();
    rerender(<GoalBar goal={goal({ status: "reached", running_since: undefined })} onEdit={vi.fn()} onPause={vi.fn()} onResume={vi.fn()} onRemove={vi.fn()} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("opens the sentence in full on a click, and edits it in place", async () => {
    const edit = vi.fn().mockResolvedValue(undefined);
    render(<GoalBar goal={goal()} onEdit={edit} onPause={vi.fn()} onResume={vi.fn()} onRemove={vi.fn()} />);
    const sentence = screen.getByRole("button", { name: "Optimize this app to bring p95 under 200 ms" });
    fireEvent.click(sentence);
    expect(sentence).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(screen.getByRole("button", { name: "Edit goal" }));
    const field = screen.getByLabelText("Goal");
    fireEvent.change(field, { target: { value: "  Bring p95 under 150 ms " } });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Save goal, applies next turn" })); });
    expect(edit).toHaveBeenCalledWith("Bring p95 under 150 ms");
    expect(screen.queryByRole("textbox")).toBeNull();
  });
});

describe("QueueStack", () => {
  const queue: QueuedMessage[] = [
    { id: "a", text: "Also add a download icon", attachments: [], held: false },
    { id: "b", text: "Look at this", attachments: ["/tmp/x.png"], held: false },
  ];

  it("lists waiting messages with steer, edit and remove", () => {
    const steer = vi.fn(), remove = vi.fn();
    render(<QueueStack queue={queue} running onSteer={steer} onHold={vi.fn()} onSave={vi.fn()} onRemove={remove} />);
    const rows = screen.getAllByRole("listitem");
    expect(rows).toHaveLength(2);
    fireEvent.click(screen.getAllByRole("button", { name: "Steer: send now" })[0]!);
    expect(steer).toHaveBeenCalledWith("a");
    // A message with files waits for the next turn: no steer.
    expect(screen.getAllByRole("button", { name: "Steer: send now" })).toHaveLength(1);
    fireEvent.click(screen.getAllByRole("button", { name: "Remove from queue" })[1]!);
    expect(remove).toHaveBeenCalledWith("b");
  });

  it("holds a message while it is edited and saves the new text", async () => {
    const hold = vi.fn(), save = vi.fn().mockResolvedValue(undefined);
    render(<QueueStack queue={queue} running={false} onSteer={vi.fn()} onHold={hold} onSave={save} onRemove={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "Steer: send now" })).toBeNull();
    fireEvent.click(screen.getAllByRole("button", { name: "Edit queued message" })[0]!);
    expect(hold).toHaveBeenCalledWith("a", true);
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "Add a download icon" } });
    await act(async () => { fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" }); });
    expect(save).toHaveBeenCalledWith("a", "Add a download icon");
  });

  it("marks a message the server could not send, with the reason, and offers no steer", () => {
    render(<QueueStack queue={[{ id: "x", text: "/goal", attachments: [], held: true, note: "Add the goal after /goal" }]} running onSteer={vi.fn()} onHold={vi.fn()} onSave={vi.fn()} onRemove={vi.fn()} />);
    const note = screen.getByLabelText("Not sent: Add the goal after /goal");
    expect(note).toHaveTextContent("Needs an edit");
    expect(note).toHaveAttribute("data-tooltip", "Add the goal after /goal");
    expect(screen.queryByRole("button", { name: "Steer: send now" })).toBeNull();
    expect(screen.getByRole("button", { name: "Edit queued message" })).toBeInTheDocument();
  });

  it("renders nothing for an empty queue", () => {
    const { container } = render(<QueueStack queue={[]} running onSteer={vi.fn()} onHold={vi.fn()} onSave={vi.fn()} onRemove={vi.fn()} />);
    expect(container).toBeEmptyDOMElement();
  });
});

describe("GoalNote", () => {
  it("shows the small-window warning as one plain line, and check outcomes with their title", () => {
    const reason = "Context window is small (32,768 tokens); long goals may stall.";
    const { container } = render(<GoalNote note={{ outcome: "small_window", reason }} />);
    const note = screen.getByRole("note");
    expect(note).toHaveTextContent(reason);
    expect(note).toHaveAttribute("data-outcome", "small_window");
    expect(container.querySelector("strong")).toBeNull();
    cleanup();
    render(<GoalNote note={{ outcome: "not_yet", reason: "Counter is 1." }} />);
    expect(screen.getByRole("note")).toHaveTextContent("Not reached yet. Counter is 1.");
  });
});
