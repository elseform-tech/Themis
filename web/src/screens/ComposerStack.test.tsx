// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ApprovalRequest, Goal, Question } from "../lib/types";
import { ComposerStack } from "./ComposerStack";

const dispatch = vi.fn();
vi.mock("../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../state/store")>(),
  useApp: () => ({ dispatch }),
}));
vi.mock("../lib/tauri", () => ({
  approveAction: vi.fn(async () => undefined),
  answerQuestion: vi.fn(async () => undefined),
}));

const approval: ApprovalRequest = { approval_id: "a1", thread_id: "t1", tool: "shell", summary: "run", risk: "execute", command: { argv: ["python3", "check.py"] } } as ApprovalRequest;
const question: Question = { id: "q1", run_id: "r1", question: "Retired items in INDEX.md only?", choices: ["INDEX.md only", "Own file too"], assumption: "INDEX.md only", blocking: false, asked_at: Date.now() };
const goal: Goal = { text: "Write every memo", status: "running", started_at: 0, active_ms: 0, turns: 2, stalls: 0, failures: 0, checklist: [{ text: "a", done: true, proof: "p" }, { text: "b", done: false }] };

function stack(props: { approvals?: ApprovalRequest[]; questions?: Question[]; goal?: Goal | null }) {
  return <ComposerStack threadId="t1" goal={props.goal ?? null} approvals={props.approvals ?? []} questions={props.questions ?? []} traces={[]}
    onEditGoal={vi.fn()} onPauseGoal={vi.fn()} onResumeGoal={vi.fn()} onRemoveGoal={vi.fn()} />;
}

beforeEach(() => {
  dispatch.mockClear();
  const store = new Map<string, string>();
  vi.stubGlobal("localStorage", { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => store.set(k, v) });
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

describe("composer stack", () => {
  it("shows the approval first, says a question waits, and lets nothing else in until it is decided", async () => {
    const { rerender } = render(stack({ goal, approvals: [approval], questions: [question] }));
    expect(screen.getByRole("region", { name: "Goal" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Approval needed" })).toHaveTextContent("1 question next");
    expect(screen.queryByRole("region", { name: "Question from the agent" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Allow once (1)" }));
    const { approveAction } = await import("../lib/tauri");
    await waitFor(() => expect(approveAction).toHaveBeenCalledWith("t1", "a1", "once"));
    expect(dispatch).toHaveBeenCalledWith({ type: "approval/dequeued", approvalId: "a1" });
    // Decided: the approval is gone and the question takes its place.
    rerender(stack({ goal, questions: [question] }));
    expect(screen.queryByRole("region", { name: "Approval needed" })).not.toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Question from the agent" })).toHaveTextContent("Recommended · working on this");
  });

  it("puts a skipped question back into its turn", () => {
    const { container } = render(stack({ questions: [{ ...question, id: "skip-me" }] }));
    fireEvent.click(screen.getByRole("button", { name: "Skip" }));
    expect(container).toBeEmptyDOMElement();
  });

  it("folds to pills that show the goal's progress, the waiting command and the questions, and remembers it", () => {
    render(stack({ goal, approvals: [approval], questions: [{ ...question, id: "fold" }] }));
    fireEvent.click(screen.getByRole("button", { name: "Fold to icons" }));
    expect(screen.getByRole("button", { name: "Goal: Write every memo, 1 of 2 done. Unfold" })).toHaveTextContent("1/2");
    expect(screen.getByRole("button", { name: "Approval waiting: python3 check.py. Unfold" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "1 question waiting. Unfold" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Approval needed" })).not.toBeInTheDocument();
    cleanup();
    render(stack({ goal }));
    expect(screen.getByRole("group", { name: /folded/ })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Unfold" }));
    expect(screen.getByRole("region", { name: "Goal" })).toBeInTheDocument();
  });

  it("is empty without a goal, approval or question", () => {
    const { container } = render(stack({ goal: { ...goal, status: "reached" } }));
    expect(container).toBeEmptyDOMElement();
  });
});
