// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ANSWER_MARKER, type Question } from "../lib/types";
import { initialState, reducer } from "../state/reducer";
import { OpenQuestions, QUESTION_OPEN_MS, QuestionBox } from "./QuestionBox";

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

  it("sends a written answer, and not an empty one", async () => {
    const answer = vi.fn().mockResolvedValue(undefined);
    render(<QuestionBox question={question({ choices: [] })} onAnswer={answer} />);
    const send = screen.getByRole("button", { name: "Send answer" });
    expect(send).toBeDisabled();
    fireEvent.change(screen.getByLabelText("Your answer"), { target: { value: "  Two spaces  " } });
    fireEvent.click(send);
    await waitFor(() => expect(answer).toHaveBeenCalledWith("Two spaces"));
  });

  it("stays open until answered, and shrinks to one line only after a while without an answer", () => {
    vi.useFakeTimers();
    render(<QuestionBox question={question()} onAnswer={vi.fn()} />);
    expect(screen.getByRole("region", { name: "Question from the agent" })).toBeInTheDocument();
    act(() => { vi.advanceTimersByTime(QUESTION_OPEN_MS + 100); });
    expect(screen.queryByRole("region", { name: "Question from the agent" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Open question/ }));
    expect(screen.getByRole("region", { name: "Question from the agent" })).toBeInTheDocument();
  });

  it("keeps a blocking question open however long it waits", () => {
    render(<QuestionBox question={question({ blocking: true, asked_at: Date.now() - 10 * QUESTION_OPEN_MS })} onAnswer={vi.fn()} />);
    expect(screen.getByRole("region", { name: "Question from the agent" })).toBeInTheDocument();
  });

  it("lists choices one per line, so a long choice reads in full", () => {
    const long = "Use the existing PostgreSQL instance and migrate the SQLite data during the next release window";
    render(<QuestionBox question={question({ choices: [long, "Keep SQLite"] })} onAnswer={vi.fn()} />);
    expect(screen.getByRole("button", { name: long })).toHaveClass("themis-question-choice");
  });

  it("shows an answered question as one quiet line with its answer", () => {
    render(<QuestionBox question={question({ answer: "Tabs" })} onAnswer={vi.fn()} />);
    expect(screen.getByRole("note")).toHaveTextContent("Tabs or spaces? — Tabs");
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  });

  it("counts the open questions that have shrunk, and nothing else", () => {
    const old = Date.now() - 2 * QUESTION_OPEN_MS;
    const { container, rerender } = render(<OpenQuestions questions={[question()]} />);
    expect(container).toBeEmptyDOMElement();
    rerender(<OpenQuestions questions={[question({ id: "a", asked_at: old }), question({ id: "b", asked_at: old }), question({ id: "c", asked_at: old, answer: "Tabs" }), question({ id: "d", asked_at: old, blocking: true })]} />);
    expect(screen.getByRole("button", { name: "2 open questions" })).toBeInTheDocument();
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
