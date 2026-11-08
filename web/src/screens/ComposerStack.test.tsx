// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ApprovalRequest, Goal, Question } from "../lib/types";
import { ComposerStack } from "./ComposerStack";

const dispatch = vi.fn();
vi.mock("../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../state/store")>(),
  useApp: () => ({ dispatch, state: { settings: { approval_timeout_seconds: 600 } } }),
}));
vi.mock("../lib/tauri", () => ({
  approveAction: vi.fn(async () => undefined),
  answerQuestion: vi.fn(async () => undefined),
  enqueueMessage: vi.fn(async () => ({ id: "m1" })),
  steerQueuedMessage: vi.fn(async () => undefined),
}));

const approval: ApprovalRequest = { approval_id: "a1", thread_id: "t1", tool: "shell", summary: "run", risk: "execute", command: { argv: ["python3", "check.py"] } } as ApprovalRequest;
const question: Question = { id: "q1", run_id: "r1", question: "Retired items in INDEX.md only?", choices: ["INDEX.md only", "Own file too"], assumption: "INDEX.md only", blocking: false, asked_at: Date.now() };
const goal: Goal = { text: "Write every memo", status: "running", started_at: 0, active_ms: 0, turns: 2, failures: 0 };

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
  it("asks about a destructive step in Auto or YOLO as a question box, never an approval box", async () => {
    const tauri = await import("../lib/tauri");
    const destructive = { ...approval, approval_id: "d1", risk: "destructive", command: { argv: ["rm", "-rf", "build"] }, ask_first: true } as ApprovalRequest;
    render(stack({ approvals: [destructive] }));
    expect(screen.getByRole("region", { name: "Question from the agent" })).toHaveTextContent("about to run `rm -rf build`");
    expect(screen.queryByRole("region", { name: "Approval needed" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /Skip it/ }));
    await waitFor(() => expect(tauri.approveAction).toHaveBeenCalledWith("t1", "d1", "deny"));
    expect(dispatch).toHaveBeenCalledWith({ type: "approval/dequeued", approvalId: "d1" });
    cleanup();
    // A written answer goes to the agent as a steered message, which releases the step unrun.
    render(stack({ approvals: [destructive] }));
    fireEvent.change(screen.getByLabelText("Your answer"), { target: { value: "use git clean -n first" } });
    fireEvent.click(screen.getByRole("button", { name: "Send answer" }));
    await waitFor(() => expect(tauri.steerQueuedMessage).toHaveBeenCalledWith("t1", "m1"));
    expect(tauri.enqueueMessage).toHaveBeenCalledWith("t1", "use git clean -n first");
  });

  it("shows the approval first, says a question waits, and lets nothing else in until it is decided", async () => {
    const { rerender } = render(stack({ goal, approvals: [approval], questions: [question] }));
    expect(screen.getByRole("region", { name: "Goal" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Approval needed" })).toHaveTextContent("1 question next");
    expect(screen.queryByRole("region", { name: "Question from the agent" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Allow once" }));
    const { approveAction } = await import("../lib/tauri");
    await waitFor(() => expect(approveAction).toHaveBeenCalledWith("t1", "a1", "once"));
    expect(dispatch).toHaveBeenCalledWith({ type: "approval/dequeued", approvalId: "a1" });
    // Decided: the approval is gone and the question takes its place.
    rerender(stack({ goal, questions: [question] }));
    expect(screen.queryByRole("region", { name: "Approval needed" })).not.toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Question from the agent" })).toHaveTextContent("Recommended");
  });

  it("puts a skipped question back into its turn", () => {
    const { container } = render(stack({ questions: [{ ...question, id: "skip-me" }] }));
    fireEvent.click(screen.getByRole("button", { name: "Skip" }));
    expect(container).toBeEmptyDOMElement();
  });

  it("folds to pills that show the goal, the waiting command and the questions, and remembers it", () => {
    render(stack({ goal, approvals: [approval], questions: [{ ...question, id: "fold" }] }));
    fireEvent.click(screen.getByRole("button", { name: "Fold to icons" }));
    expect(screen.getByRole("button", { name: "Goal: Write every memo. Unfold" })).toHaveTextContent("Write every memo");
    expect(screen.getByRole("button", { name: "Approval waiting: python3 check.py. Unfold" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "1 question waiting. Unfold" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Approval needed" })).not.toBeInTheDocument();
    cleanup();
    render(stack({ goal }));
    expect(screen.getByRole("group", { name: /folded/ })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Unfold" }));
    expect(screen.getByRole("region", { name: "Goal" })).toBeInTheDocument();
  });

  it("keeps a complete goal for its evidence, and is empty without a goal, approval or question", () => {
    render(stack({ goal: { ...goal, status: "reached" } }));
    expect(screen.getByRole("region", { name: "Goal" })).toHaveTextContent("Complete");
    cleanup();
    const { container } = render(stack({ goal: null }));
    expect(container).toBeEmptyDOMElement();
  });
});
