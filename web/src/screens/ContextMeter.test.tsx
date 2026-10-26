// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import * as bridge from "../lib/tauri";
import type { ContextBudget } from "../lib/types";
import { ContextMeter } from "./ContextMeter";
import { readFileSync } from "node:fs";

const budget: ContextBudget = { budget_tokens: 200000, usable_tokens: 187000, used_tokens: 158000, tool_definition_tokens: 9800, output_reserve_tokens: 8192, safety_margin_tokens: 4000, maintenance_at_tokens: 155210, compaction_at_tokens: 166000, compaction_soon: true, breakdown: { system: 8400, checkpoint: 4100, attachments: 10600, conversation: 38500, tool_activity: 96400 } };
vi.mock("../lib/tauri", () => ({ getChatUsage: vi.fn(async () => ({ requests: 0 })) }));
afterEach(cleanup);

describe("ContextMeter", () => {
  it("shows usage and opens a breakdown with the runtime thresholds and reserves", () => {
    render(<ContextMeter budget={budget} running />);
    const chip = screen.getByRole("button", { name: "Context 84% full, close to auto-compact" });
    expect(chip).toHaveTextContent("84% · compacting soon");
    fireEvent.click(chip);
    const panel = screen.getByRole("dialog", { name: "Context" });
    expect(panel).toHaveTextContent("Tidy-up at 83%");
    expect(panel).toHaveTextContent("Auto-compact at 89%");
    expect(panel).toHaveTextContent("Tool calls & results96.4k");
    expect(panel).toHaveTextContent("9.8k tool definitions · 8.2k reply · 4.0k safety margin");
    expect(panel).toHaveTextContent("Close to auto-compact.");
  });
  it("stays quiet when idle and shows compaction progress while compacting", () => {
    const { rerender } = render(<ContextMeter budget={budget} />);
    expect(screen.getByRole("button", { name: "Context 84% full" })).toHaveTextContent(/^84%$/);
    rerender(<ContextMeter budget={budget} running compacting />);
    expect(screen.getByRole("button", { name: "Context 84% full" })).toHaveTextContent("Compacting…");
  });
  it("shows model reasoning only when a thinking model sends some back", () => {
    const { rerender } = render(<ContextMeter budget={budget} />);
    fireEvent.click(screen.getByRole("button", { name: "Context 84% full" }));
    expect(screen.getByRole("dialog", { name: "Context" })).not.toHaveTextContent("Model reasoning");
    rerender(<ContextMeter budget={{ ...budget, breakdown: { ...budget.breakdown, reasoning: 21300 } }} />);
    const panel = screen.getByRole("dialog", { name: "Context" });
    expect(panel).toHaveTextContent("Model reasoning21.3k");
    expect(screen.getByRole("img")).toHaveAccessibleName(expect.stringContaining("Model reasoning 21.3k"));
  });
  it("adds this chat's recorded tokens when the panel opens, and nothing before any", async () => {
    const usage = { requests: 54, unreported_requests: 0, input_tokens: 1_250_000, cached_input_tokens: 900_000, output_tokens: 40_200, reasoning_tokens: 0, unpriced_requests: 0 };
    vi.mocked(bridge.getChatUsage).mockResolvedValueOnce(usage).mockResolvedValueOnce({ ...usage, requests: 0 });
    const { unmount } = render(<ContextMeter budget={budget} threadId="t1" />);
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Context 84% full" })); });
    expect(bridge.getChatUsage).toHaveBeenCalledWith("t1");
    expect(screen.getByRole("dialog", { name: "Context" })).toHaveTextContent("This chat so far: 1.25M in · 40.2k out · 54 requests");
    unmount();
    render(<ContextMeter budget={budget} threadId="t2" />);
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Context 84% full" })); });
    expect(screen.getByRole("dialog", { name: "Context" })).not.toHaveTextContent("This chat");
  });
  it("warns when the budget leaves too little room to work in", () => {
    const small = { ...budget, budget_tokens: 16000, usable_tokens: 4100, used_tokens: 2200, compaction_soon: false, work_room_tokens: 1900, minimum_budget_tokens: 20100 };
    render(<ContextMeter budget={small} />);
    const chip = screen.getByRole("button", { name: "Context 54% full, budget too small" });
    expect(chip.className).toContain("themis-context-chip--warn");
    fireEvent.click(chip);
    expect(screen.getByRole("dialog", { name: "Context" })).toHaveTextContent(
      "This budget is too small to work in: instructions and tools leave about 1.9k for the task. Raise the context budget to at least 20.1k.");
  });
  it("says nothing about the floor when the budget is large enough or older events lack it", () => {
    const { rerender } = render(<ContextMeter budget={{ ...budget, work_room_tokens: 170000, minimum_budget_tokens: 30000 }} />);
    fireEvent.click(screen.getByRole("button", { name: "Context 84% full" }));
    expect(screen.getByRole("dialog", { name: "Context" })).not.toHaveTextContent("too small");
    rerender(<ContextMeter budget={budget} />);
    expect(screen.getByRole("dialog", { name: "Context" })).not.toHaveTextContent("too small");
  });
  it("renders nothing before the first budget arrives", () => {
    const { container } = render(<ContextMeter budget={undefined} running />);
    expect(container).toBeEmptyDOMElement();
  });
});

it("opens over the sticky run-progress row, which shows only during a run", () => {
  const css = readFileSync("src/screens/ThreadView.css", "utf8");
  const zIndex = (selector: string) => Number(new RegExp(`\\${selector} \\{[^}]*z-index: (\\d+)`).exec(css)?.[1]);
  expect(zIndex(".themis-thread-composer")).toBeGreaterThan(zIndex(".themis-run-progress"));
});
