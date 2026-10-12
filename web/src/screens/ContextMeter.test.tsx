// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { ContextBudget } from "../lib/types";
import { ContextMeter } from "./ContextMeter";

const budget: ContextBudget = { budget_tokens: 200000, usable_tokens: 187000, used_tokens: 158000, tool_definition_tokens: 9800, output_reserve_tokens: 8192, safety_margin_tokens: 4000, maintenance_at_tokens: 155210, compaction_at_tokens: 166000, compaction_soon: true, breakdown: { system: 8400, checkpoint: 4100, attachments: 10600, conversation: 38500, tool_activity: 96400 } };
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
  it("renders nothing before the first budget arrives", () => {
    const { container } = render(<ContextMeter budget={undefined} running />);
    expect(container).toBeEmptyDOMElement();
  });
});
