// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ANSWER_MARKER, VISION_ANSWER_PREFIX, type Question } from "../lib/types";
import type { VisionInfo } from "../lib/visions";
import { initialState, reducer } from "../state/reducer";
import { ThreadConversation } from "./ThreadConversation";

let visionList: VisionInfo[] = [];
vi.mock("../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../state/store")>(),
  useApp: () => ({ dispatch: vi.fn(), state: { ...initialState, historyLoaded: { t1: true }, settings: { approval_timeout_seconds: 600 }, visions: { ...initialState.visions, list: visionList } } }),
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

describe("a Vision's messages in the chat", () => {
  const wren: VisionInfo = { id: "v1", name: "Wren", role: "", shape: "cloud", color: "teal", soul: "", paused: false, revision: "r",
    over_budget: false, vision_chats: [], needs: 0, sorry: false };

  it("carry the Vision's name, sent, queued or steered; the person's own look as before", () => {
    visionList = [wren];
    const { container } = conversation([
      { kind: "user", run_id: "r1", text: "Please add a changelog." },
      { kind: "user", run_id: "r2", text: "Start on the release notes.", vision_id: "v1" },
      event({ kind: "steer_shown", text: "Use pnpm.", vision_id: "v1" }, "r2"),
      event({ kind: "steered", text: "Use pnpm.", vision_id: "v1" }, "r2"),
      event({ kind: "steer_shown", text: "And keep it short." }, "r2"),
    ]);
    const bubbles = [...container.querySelectorAll(".themis-thread-msg--user")];
    expect(bubbles.map(bubble => bubble.textContent)).toEqual([
      "Please add a changelog.", "WrenStart on the release notes.", "WrenUse pnpm.", "And keep it short.",
    ]);
    expect(bubbles[0]!.innerHTML).toBe(bubbles[3]!.innerHTML.replace("And keep it short.", "Please add a changelog."));
    visionList = [];
  });

  it("carry the name when one arrives live, too", () => {
    let state = reducer(initialState, { type: "thread/history-loaded", threadId: "t1", history: [] });
    state = reducer(state, { type: "thread/event", envelope: { thread_id: "t1", run_id: "r3", event: { kind: "user_message", text: "Go on.", attachments: [], vision_id: "v1" } as never } } as never);
    expect(state.messages.t1?.[0]?.visionId).toBe("v1");
  });
});
