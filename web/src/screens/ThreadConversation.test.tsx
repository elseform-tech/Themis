// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ANSWER_MARKER, VISION_ANSWER_PREFIX, type Question } from "../lib/types";
import { initialState, reducer } from "../state/reducer";
import { ThreadConversation } from "./ThreadConversation";

vi.mock("../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../state/store")>(),
  useApp: () => ({ dispatch: vi.fn(), state: { ...initialState, historyLoaded: { t1: true }, settings: { approval_timeout_seconds: 600 } } }),
  useActiveProject: () => null,
}));
vi.mock("../lib/usePromptSkills", () => ({ usePromptSkills: () => ({ skills: [], plugins: [] }) }));

afterEach(cleanup);

const asked = (id: string, text: string): Question => ({ id, run_id: "r1", question: text, choices: ["Tabs", "Spaces"], assumption: "Spaces", blocking: false, asked_at: Date.now() });
const event = (event: object, run_id = "r1") => ({ kind: "event" as const, envelope: { thread_id: "t1", run_id, event: event as never } });

function conversation(history: object[]) {
  const state = reducer(initialState, { type: "thread/history-loaded", threadId: "t1", history: history as never });
  return render(<ThreadConversation threadId="t1" messages={state.messages.t1 ?? []} stream="" running={false} sending={false} stopping={false}
    runDurations={{}} activeRunId={undefined} runStartedAt={undefined} compacting={false} traces={[]} />);
}

describe("answered questions in the chat", () => {
  it("show as question → answer rows where they were answered, never as an answer bubble", () => {
    // A chat saved before this change: the answer went out as a marked user message.
    const { container } = conversation([
      event({ kind: "question_asked", question: asked("q1", "Tabs or spaces?") }),
      event({ kind: "question_asked", question: asked("q2", "Which license?") }),
      event({ kind: "assistant_text", text: "Working on my recommendations." }),
      event({ kind: "question_answered", id: "q1", answer: "Tabs" }, ""),
      event({ kind: "question_answered", id: "q2", answer: "MIT" }, ""),
      { kind: "user", run_id: "r2", text: `${ANSWER_MARKER}\nQuestion: Tabs or spaces?\nAnswer: Tabs\n\nQuestion: Which license?\nAnswer: MIT` },
    ]);
    const notes = screen.getAllByRole("note").filter(note => note.classList.contains("themis-question-note"));
    expect(notes.map(note => note.textContent)).toEqual(["Tabs or spaces? → Tabs", "Which license? → MIT"]);
    // After the reply that came in between, not up where they were asked.
    const text = container.textContent ?? "";
    expect(text.indexOf("Working on my recommendations.")).toBeLessThan(text.indexOf("Tabs or spaces? → Tabs"));
    expect(container.querySelector(".themis-thread-msg--user")).toBeNull();
    expect(text).not.toContain(ANSWER_MARKER);
  });

  it("hide a Vision's answer message the same way", () => {
    const { container } = conversation([
      event({ kind: "question_asked", question: asked("q1", "Tabs or spaces?") }),
      event({ kind: "question_answered", id: "q1", answer: "Tabs", vision_id: "v1" }, ""),
      { kind: "user", run_id: "r2", text: `${VISION_ANSWER_PREFIX}Wren]\nWren is a Vision.\nQuestion: Tabs or spaces?\nAnswer: Tabs` },
    ]);
    expect(container.querySelector(".themis-thread-msg--user")).toBeNull();
    expect(container.textContent ?? "").not.toContain(VISION_ANSWER_PREFIX);
  });

  it("keep an unanswered question as its open line where it was asked once its wait is over", () => {
    conversation([event({ kind: "question_asked", question: { ...asked("q1", "Tabs or spaces?"), asked_at: Date.now() - 60 * 60 * 1000 } })]);
    expect(screen.getByRole("button", { name: /Open question · Tabs or spaces\?/ })).toBeInTheDocument();
  });
});
