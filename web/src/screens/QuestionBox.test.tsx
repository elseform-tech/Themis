// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ANSWER_MARKER, type Question } from "../lib/types";
import { initialState, reducer } from "../state/reducer";
import { OpenQuestions, QuestionBox } from "./QuestionBox";

afterEach(cleanup);

const question = (overrides: Partial<Question> = {}): Question => ({
  id: "q1", run_id: "r1", question: "Tabs or spaces?", choices: ["Tabs", "Spaces"], assumption: "Spaces",
  blocking: false, asked_at: 0, ...overrides,
});

describe("QuestionBox", () => {
  it("shows the question, the assumption and the choices while the agent has not moved on", async () => {
    const answer = vi.fn().mockResolvedValue(undefined);
    render(<QuestionBox question={question()} latest onAnswer={answer} />);
    const box = screen.getByRole("region", { name: "Question from the agent" });
    expect(box).toHaveTextContent("Tabs or spaces?");
    expect(box).toHaveTextContent("Working on: Spaces");
    fireEvent.click(screen.getByRole("button", { name: "Tabs" }));
    await waitFor(() => expect(answer).toHaveBeenCalledWith("Tabs"));
  });

  it("says nothing about working on an assumption for a blocking question", () => {
    render(<QuestionBox question={question({ blocking: true, assumption: "Nothing until you answer" })} latest onAnswer={vi.fn()} />);
    expect(screen.getByRole("region", { name: "Question from the agent" })).not.toHaveTextContent("Working on");
  });

  it("sends a written answer, and not an empty one", async () => {
    const answer = vi.fn().mockResolvedValue(undefined);
    render(<QuestionBox question={question({ choices: [] })} latest onAnswer={answer} />);
    const send = screen.getByRole("button", { name: "Send answer" });
    expect(send).toBeDisabled();
    fireEvent.change(screen.getByLabelText("Your answer"), { target: { value: "  Two spaces  " } });
    fireEvent.click(send);
    await waitFor(() => expect(answer).toHaveBeenCalledWith("Two spaces"));
  });

  it("collapses to one line once the agent moved on, and opens again to answer later", () => {
    render(<QuestionBox question={question()} latest={false} onAnswer={vi.fn()} />);
    const line = screen.getByRole("button", { name: /Open question/ });
    expect(screen.queryByRole("region", { name: "Question from the agent" })).not.toBeInTheDocument();
    fireEvent.click(line);
    expect(screen.getByRole("region", { name: "Question from the agent" })).toBeInTheDocument();
  });

  it("shows an answered question as one quiet line with its answer", () => {
    render(<QuestionBox question={question({ answer: "Tabs" })} latest onAnswer={vi.fn()} />);
    expect(screen.getByRole("note")).toHaveTextContent("Tabs or spaces? — Tabs");
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  });

  it("counts open questions and says nothing when there are none", () => {
    const { container, rerender } = render(<OpenQuestions ids={[]} />);
    expect(container).toBeEmptyDOMElement();
    rerender(<OpenQuestions ids={["a", "b"]} />);
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
