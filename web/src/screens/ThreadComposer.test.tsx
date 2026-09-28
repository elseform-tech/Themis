// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ThreadInfo } from "../lib/types";
import { ThreadComposer } from "./ThreadComposer";

const thread: ThreadInfo = {
  id: "thread-1",
  title: "Test thread",
  provider: "go",
  model: "gpt-5",
  running: false,
  worktree_path: null,
  branch: null,
  base_branch: null,
  recovered: false,
  skill_ids: [],
};

describe("ThreadComposer", () => {
  it("sends on Enter but preserves newlines and IME composition", () => {
    const onSend = vi.fn();
    render(
      <ThreadComposer
        thread={thread}
        draft="hello"
        models={[]}
        effortLevels={["low", "medium", "high"]}
        effort=""
        composerDisabled={false}
        composerBusy={false}
        readOnly={false}
        running={false}
        stopping={false}
        switching={false}
        sendError={undefined}
        secretConfigured
        onDraftChange={vi.fn()}
        onSend={onSend}
        onStop={vi.fn()}
        onNewThread={vi.fn()}
        onProviderChange={vi.fn()}
        onEffortChange={vi.fn()}
        onOpenSettings={vi.fn()}
      />,
    );

    const input = screen.getByRole("textbox", { name: "Message" });
    fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
    fireEvent.keyDown(input, { key: "Enter", isComposing: true });
    expect(onSend).not.toHaveBeenCalled();

    fireEvent.keyDown(input, { key: "Enter" });
    expect(onSend).toHaveBeenCalledTimes(1);
  });
});
