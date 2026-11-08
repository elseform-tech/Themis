// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ANSWER_MARKER, type Question } from "../lib/types";
import { initialState, reducer } from "../state/reducer";
import { BLOCKING_OPEN_MS, composerQueue, QUESTION_OPEN_MS, questionOnComposer } from "../state/composerSlot";
import { QuestionBox, QuestionNote } from "./QuestionBox";
import { QuestionPages } from "./QuestionPages";

afterEach(() => { cleanup(); vi.useRealTimers(); });

const question = (overrides: Partial<Question> = {}): Question => ({
  id: "q1", run_id: "r1", question: "Tabs or spaces?", choices: ["Tabs", "Spaces"], assumption: "Spaces",
  blocking: false, asked_at: Date.now(), ...overrides,
});

describe("QuestionBox", () => {
  it("shows the question with its choices, the recommended one marked, and answers with a click", async () => {
    const answer = vi.fn().mockResolvedValue(undefined);
    render(<QuestionBox question={question({ choices: ["Spaces", "Tabs"] })} onAnswer={answer} />);
    const box = screen.getByRole("region", { name: "Question from the agent" });
    expect(box).toHaveTextContent("Tabs or spaces?");
    expect(screen.getByRole("button", { name: /^Spaces/ })).toHaveTextContent("Recommended");
    expect(screen.getByRole("button", { name: "Tabs" })).not.toHaveTextContent("Recommended");
    fireEvent.click(screen.getByRole("button", { name: "Tabs" }));
    await waitFor(() => expect(answer).toHaveBeenCalledWith("Tabs"));
  });

  it("shows each option's hint under its label, two lines until opened, and opening picks nothing", async () => {
    const answer = vi.fn().mockResolvedValue(undefined);
    const long = "Re-indents every file with spaces. It matches the existing files and the formatter, but touches every line once; easy to undo with one revert.";
    render(<QuestionBox question={question({ choices: ["Spaces", "Tabs"], reasons: { Spaces: long, Tabs: "smaller diffs" } })} onAnswer={answer} />);
    // The label is the button; the hint is its own small control under it.
    expect(screen.getByRole("button", { name: /^Spaces/ })).not.toHaveTextContent(long);
    const hint = screen.getByRole("button", { name: long });
    expect(hint).toHaveClass("themis-question-hint");
    expect(hint).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(hint);
    expect(hint).toHaveAttribute("aria-expanded", "true");
    expect(hint).toHaveClass("is-open");
    expect(answer).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /^Tabs/ }));
    await waitFor(() => expect(answer).toHaveBeenCalledWith("Tabs"));
  });

  it("offers an open question's suggestion as one answer, beside a written one", async () => {
    const answer = vi.fn().mockResolvedValue(undefined);
    render(<QuestionBox question={question({ question: "What is the app called?", choices: [], assumption: "Pebble" })} onAnswer={answer} />);
    const suggestion = screen.getByRole("button", { name: /^Pebble/ });
    expect(suggestion).toHaveTextContent("Suggested");
    expect(screen.getByLabelText("Your answer")).toHaveAttribute("placeholder", "Or write your answer");
    fireEvent.click(suggestion);
    await waitFor(() => expect(answer).toHaveBeenCalledWith("Pebble"));
  });

  it("sends a written answer, offering Skip until something is written", async () => {
    const answer = vi.fn().mockResolvedValue(undefined);
    const skip = vi.fn();
    render(<QuestionBox question={question({ choices: [] })} onAnswer={answer} onSkip={skip} />);
    expect(screen.queryByRole("button", { name: "Send answer" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Skip" }));
    expect(skip).toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText("Your answer"), { target: { value: "  Two spaces  " } });
    expect(screen.queryByRole("button", { name: "Skip" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Send answer" }));
    await waitFor(() => expect(answer).toHaveBeenCalledWith("Two spaces"));
  });

  it("offers no Skip for a blocking question: the work waits for it", () => {
    render(<QuestionBox question={question({ blocking: true })} onAnswer={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "Skip" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^Spaces/ })).toHaveTextContent("Recommended");
  });

  it("lists choices one per line, so a long choice reads in full", () => {
    const long = "Use the existing PostgreSQL instance and migrate the SQLite data during the next release window";
    render(<QuestionBox question={question({ choices: [long, "Keep SQLite"] })} onAnswer={vi.fn()} />);
    expect(screen.getByRole("button", { name: long })).toHaveClass("themis-question-choice");
  });
});

describe("several questions in one box", () => {
  const pages = () => [
    question({ id: "p1", group: "g", question: "Tabs or spaces?" }),
    question({ id: "p2", group: "g", question: "Which license?", choices: ["MIT", "Apache-2.0"], assumption: "MIT" }),
    question({ id: "p3", group: "g", question: "Project name?", choices: [], assumption: "Pebble" }),
  ];

  it("shows one page at a time, then a review, and sends every answer together on Submit", async () => {
    const send = vi.fn().mockResolvedValue(undefined);
    const leave = vi.fn();
    render(<QuestionPages pages={pages()} onSend={send} onLeave={leave} />);
    expect(screen.getByRole("region", { name: "Question from the agent" })).toHaveTextContent("Tabs or spaces?");
    expect(screen.getByText("Question 1 of 3")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Back" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Next" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: /^Tabs/ }));
    await waitFor(() => expect(screen.getByText("Question 2 of 3")).toBeInTheDocument());
    expect(send).not.toHaveBeenCalled();
    // Back shows the answer given, and Next keeps it.
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(screen.getByRole("button", { name: /^Tabs/ })).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    fireEvent.click(screen.getByRole("button", { name: /^Apache-2.0/ }));
    await waitFor(() => expect(screen.getByText("Question 3 of 3")).toBeInTheDocument());
    fireEvent.change(screen.getByLabelText("Your answer"), { target: { value: "Rock" } });
    fireEvent.click(screen.getByRole("button", { name: "Send answer" }));
    // The review lists every answer; nothing is sent yet.
    const review = await screen.findByRole("region", { name: "Your answers" });
    expect(review).toHaveTextContent("Tabs or spaces?Tabs");
    expect(review).toHaveTextContent("Which license?Apache-2.0");
    expect(review).toHaveTextContent("Project name?Rock");
    expect(send).not.toHaveBeenCalled();
    // A click goes back to change one; answering it returns to the review.
    fireEvent.click(screen.getByRole("button", { name: "Change: Which license?" }));
    expect(screen.getByText("Question 2 of 3")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /^MIT/ }));
    expect(await screen.findByRole("region", { name: "Your answers" })).toHaveTextContent("Which license?MIT");
    fireEvent.click(screen.getByRole("button", { name: "Submit" }));
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    expect(send).toHaveBeenCalledWith([
      { questionId: "p1", answer: "Tabs" },
      { questionId: "p2", answer: "MIT" },
      { questionId: "p3", answer: "Rock" },
    ]);
    expect(leave).not.toHaveBeenCalled();
  });

  it("shows a skipped page as Skipped, leaves it open in its turn and sends the rest", async () => {
    const send = vi.fn().mockResolvedValue(undefined);
    const leave = vi.fn();
    render(<QuestionPages pages={pages().slice(0, 2)} onSend={send} onLeave={leave} />);
    fireEvent.click(screen.getByRole("button", { name: "Skip" }));
    await waitFor(() => expect(screen.getByText("Question 2 of 2")).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: /^MIT/ }));
    expect(await screen.findByRole("region", { name: "Your answers" })).toHaveTextContent("Tabs or spaces?Skipped");
    fireEvent.click(screen.getByRole("button", { name: "Submit" }));
    await waitFor(() => expect(send).toHaveBeenCalledWith([{ questionId: "p2", answer: "MIT" }]));
    expect(leave).toHaveBeenCalledWith(["p1"]);
  });

  it("offers no Skip on a page the work waits on", () => {
    render(<QuestionPages pages={[question({ id: "b", group: "g", blocking: true, assumption: undefined }), question({ id: "n", group: "g" })]} onSend={vi.fn()} onLeave={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "Skip" })).not.toBeInTheDocument();
  });

  it("moves keyboard focus to the next page's first answer", async () => {
    render(<QuestionPages pages={pages()} onSend={vi.fn()} onLeave={vi.fn()} />);
    const first = screen.getByRole("button", { name: /^Tabs/ });
    first.focus();
    fireEvent.click(first);
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("button", { name: /^MIT/ })));
  });

  it("is a plain question box for a single question: no page count, no review, sent at once", async () => {
    const send = vi.fn().mockResolvedValue(undefined);
    render(<QuestionPages pages={[question()]} onSend={send} onLeave={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "Back" })).not.toBeInTheDocument();
    expect(screen.queryByText(/Question 1 of/)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /^Tabs/ }));
    await waitFor(() => expect(send).toHaveBeenCalledWith([{ questionId: "q1", answer: "Tabs" }]));
    expect(screen.queryByRole("region", { name: "Your answers" })).not.toBeInTheDocument();
  });

  it("is one box on the composer, its questions counted", () => {
    const now = Date.now();
    const set = pages().map((page, i) => ({ ...page, asked_at: now - 10 + i }));
    const alone = question({ id: "alone", asked_at: now - 5 });
    const queue = composerQueue([], [alone, ...set], new Map(), now);
    expect(queue.current).toEqual({ kind: "question", questions: set });
    // A page reopened from its turn brings its box first; the pages keep the order asked.
    const reopened = composerQueue([], [alone, ...set], new Map([["p3", "composer" as const]]), now);
    expect(reopened.current).toEqual({ kind: "question", questions: set });
    const newer = question({ id: "newer", asked_at: now - 1 });
    expect(composerQueue([], [alone, newer], new Map([["newer", "composer" as const]]), now).current).toEqual({ kind: "question", questions: [newer] });
    expect(queue.waiting).toEqual({ approvals: 0, questions: 1 });
  });
});

describe("a question in its turn", () => {
  it("is one quiet line, question → answer, once answered", () => {
    render(<QuestionNote question={question({ answer: "Tabs" })} onComposer={false} onOpen={vi.fn()} />);
    expect(screen.getByRole("note")).toHaveTextContent("Tabs or spaces? → Tabs");
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  });

  it("shows nothing while the question is on the composer, and a line that brings it back after", () => {
    const open = vi.fn();
    const { container, rerender } = render(<QuestionNote question={question()} onComposer onOpen={open} />);
    expect(container).toBeEmptyDOMElement();
    rerender(<QuestionNote question={question()} onComposer={false} onOpen={open} />);
    fireEvent.click(screen.getByRole("button", { name: /Open question · Tabs or spaces\?/ }));
    expect(open).toHaveBeenCalled();
  });
});

describe("what the composer shows", () => {
  const approval = (id: string) => ({ approval_id: id, thread_id: "t1", tool: "shell", summary: "run", risk: "execute" as const });
  const none = new Map();

  it("puts approvals before questions, oldest question first, and counts the rest", () => {
    const now = Date.now();
    const newer = question({ id: "new", asked_at: now - 10 });
    const older = question({ id: "old", asked_at: now - 20 });
    const first = composerQueue([approval("a1"), approval("a2")] as never, [newer, older], none, now);
    expect(first.current).toEqual({ kind: "approval", request: approval("a1") });
    expect(first.waiting).toEqual({ approvals: 1, questions: 2 });
    const next = composerQueue([], [newer, older], none, now);
    expect(next.current).toEqual({ kind: "question", questions: [older] });
    expect(next.waiting).toEqual({ approvals: 0, questions: 1 });
    expect(composerQueue([], [question({ answer: "Tabs" })], none, now).current).toBeNull();
  });

  it("shows a question opened from its turn first, even with an older one still on the composer", () => {
    const now = Date.now();
    const older = question({ id: "old", asked_at: now - 20 });
    const opened = question({ id: "opened", asked_at: now - 10 * QUESTION_OPEN_MS });
    // Left its turn long ago; the person clicks it while the older question waits on the composer.
    const queue = composerQueue([], [older, opened], new Map([["opened", "composer"]]), now);
    expect(queue.current).toEqual({ kind: "question", questions: [opened] });
    expect(queue.waiting).toEqual({ approvals: 0, questions: 1 });
    // The one opened last comes first; approvals still lead.
    const both = new Map([["old", "composer"], ["opened", "composer"]]) as Map<string, "composer">;
    expect(composerQueue([], [older, opened], both, now).current).toEqual({ kind: "question", questions: [opened] });
    both.delete("old"); both.set("old", "composer");
    expect(composerQueue([], [older, opened], both, now).current).toEqual({ kind: "question", questions: [older] });
    expect(composerQueue([approval("a1")] as never, [older, opened], both, now).current?.kind).toBe("approval");
  });

  it("moves a question back into its turn when skipped or after a while", () => {
    const now = Date.now();
    expect(questionOnComposer(question(), none, now)).toBe(true);
    expect(questionOnComposer(question(), new Map([["q1", "turn"]]), now)).toBe(false);
    expect(questionOnComposer(question({ asked_at: now - QUESTION_OPEN_MS - 1 }), none, now)).toBe(false);
    // Opened again from its turn, it stays until answered.
    expect(questionOnComposer(question({ asked_at: now - 10 * QUESTION_OPEN_MS }), new Map([["q1", "composer"]]), now)).toBe(true);
  });

  it("keeps a blocking question on the composer for the approval timeout, then moves it into its turn", () => {
    const now = Date.now();
    const blocking = (asked_at: number) => question({ blocking: true, asked_at });
    // Past the 5 minutes other questions get, but within the approval timeout: still there.
    expect(questionOnComposer(blocking(now - QUESTION_OPEN_MS - 1), none, now)).toBe(true);
    expect(questionOnComposer(blocking(now - BLOCKING_OPEN_MS - 1), none, now)).toBe(false);
    // The wait follows Settings › Permissions › Approval timeout.
    expect(questionOnComposer(blocking(now - 61_000), none, now, 60_000)).toBe(false);
    expect(composerQueue([], [blocking(now - 61_000)], none, now, 60_000).current).toBeNull();
    // Opened again from its turn, it stays until answered.
    expect(questionOnComposer(blocking(now - 10 * BLOCKING_OPEN_MS), new Map([["q1", "composer"]]), now)).toBe(true);
  });
});

describe("question events", () => {
  const thread = "t1";
  const envelope = (event: object, run_id = "r1") => ({ type: "thread/event" as const, envelope: { thread_id: thread, run_id, event: event as never } });

  it("adds a box, updates it in place when asked again, and records the answer", () => {
    let state = reducer(initialState, envelope({ kind: "question_asked", question: question() }));
    state = reducer(state, envelope({ kind: "assistant_text", text: "Working." }));
    state = reducer(state, envelope({ kind: "question_asked", question: question({ question: "Tabs or spaces for indentation?" }) }));
    let rows = state.messages[thread]!.filter(row => row.question);
    expect(rows.map(row => row.question!.question)).toEqual(["Tabs or spaces for indentation?"]);
    state = reducer(state, envelope({ kind: "assistant_text", text: "Still working." }));
    state = reducer(state, envelope({ kind: "question_answered", id: "q1", answer: "Tabs" }, ""));
    rows = state.messages[thread]!.filter(row => row.question);
    expect(rows[0]!.question!.answer).toBe("Tabs");
    // The answer joins the chat where it was given: after what came in between.
    const all = state.messages[thread]!;
    expect(all[all.length - 1]).toMatchObject({ answeredHere: true, question: { id: "q1", answer: "Tabs" } });
    // The same history replayed on load gives the same box.
    const loaded = reducer(initialState, { type: "thread/history-loaded", threadId: thread, history: [
      { kind: "event", envelope: { thread_id: thread, run_id: "r1", event: { kind: "question_asked", question: question() } } },
      { kind: "event", envelope: { thread_id: thread, run_id: "", event: { kind: "question_answered", id: "q1", answer: "Tabs" } } },
      { kind: "user", run_id: "r2", text: `${ANSWER_MARKER}\nQuestion: Tabs or spaces?\nAnswer: Tabs` },
    ] });
    expect(loaded.messages[thread]![0]!.question!.answer).toBe("Tabs");
    expect(loaded.messages[thread]!.filter(row => row.answeredHere)).toHaveLength(1);
  });
});
