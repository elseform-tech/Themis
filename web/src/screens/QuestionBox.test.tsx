// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ANSWER_MARKER, type Question } from "../lib/types";
import { initialState, reducer } from "../state/reducer";
import { composerQueue, QUESTION_OPEN_MS, questionOnComposer } from "../state/composerSlot";
import { QuestionBox, QuestionNote } from "./QuestionBox";

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

  it("shows each option's one-line reason under it", () => {
    render(<QuestionBox question={question({ choices: ["Spaces", "Tabs"], reasons: { Spaces: "matches the existing files", Tabs: "smaller diffs" } })} onAnswer={vi.fn()} />);
    expect(screen.getByRole("button", { name: /^Spaces/ })).toHaveTextContent("matches the existing files");
    expect(screen.getByRole("button", { name: /^Tabs/ })).toHaveTextContent("smaller diffs");
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
    expect(screen.getByRole("button", { name: /^Spaces/ })).not.toHaveTextContent("working on this");
  });

  it("lists choices one per line, so a long choice reads in full", () => {
    const long = "Use the existing PostgreSQL instance and migrate the SQLite data during the next release window";
    render(<QuestionBox question={question({ choices: [long, "Keep SQLite"] })} onAnswer={vi.fn()} />);
    expect(screen.getByRole("button", { name: long })).toHaveClass("themis-question-choice");
  });
});

describe("a question in its turn", () => {
  it("is one quiet line with its answer once answered", () => {
    render(<QuestionNote question={question({ answer: "Tabs" })} onComposer={false} onOpen={vi.fn()} />);
    expect(screen.getByRole("note")).toHaveTextContent("Tabs or spaces? — Tabs");
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
    expect(next.current).toEqual({ kind: "question", question: older });
    expect(next.waiting).toEqual({ approvals: 0, questions: 1 });
    expect(composerQueue([], [question({ answer: "Tabs" })], none, now).current).toBeNull();
  });

  it("moves a question back into its turn when skipped or after a while, but never a blocking one", () => {
    const now = Date.now();
    expect(questionOnComposer(question(), none, now)).toBe(true);
    expect(questionOnComposer(question(), new Map([["q1", "turn"]]), now)).toBe(false);
    expect(questionOnComposer(question({ asked_at: now - QUESTION_OPEN_MS - 1 }), none, now)).toBe(false);
    // Opened again from its turn, it stays until answered.
    expect(questionOnComposer(question({ asked_at: now - 10 * QUESTION_OPEN_MS }), new Map([["q1", "composer"]]), now)).toBe(true);
    expect(questionOnComposer(question({ blocking: true, asked_at: now - 10 * QUESTION_OPEN_MS }), new Map([["q1", "turn"]]), now)).toBe(true);
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
    state = reducer(state, envelope({ kind: "question_answered", id: "q1", answer: "Tabs" }, ""));
    rows = state.messages[thread]!.filter(row => row.question);
    expect(rows[0]!.question!.answer).toBe("Tabs");
    // The same history replayed on load gives the same box.
    const loaded = reducer(initialState, { type: "thread/history-loaded", threadId: thread, history: [
      { kind: "event", envelope: { thread_id: thread, run_id: "r1", event: { kind: "question_asked", question: question() } } },
      { kind: "event", envelope: { thread_id: thread, run_id: "", event: { kind: "question_answered", id: "q1", answer: "Tabs" } } },
      { kind: "user", run_id: "r2", text: `${ANSWER_MARKER}\nQuestion: Tabs or spaces?\nAnswer: Tabs` },
    ] });
    expect(loaded.messages[thread]![0]!.question!.answer).toBe("Tabs");
  });
});
