// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import * as bridge from "../lib/tauri";
import type { UsageBreakdown, UsageSummary, UsageTotals } from "../lib/types";
import { compactTokens, costLabel, UsageSettingsSection } from "./UsageSettings";

vi.mock("../lib/tauri", () => ({ getUsageSummary: vi.fn() }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });

const totals = (patch: Partial<UsageTotals> = {}): UsageTotals => ({ requests: 0, unreported_requests: 0, input_tokens: 0, cached_input_tokens: 0, output_tokens: 0, reasoning_tokens: 0, unpriced_requests: 0, ...patch });
const model = (key: string, input: number, free = false): UsageBreakdown => ({ key, provider: "opencode-go", free, totals: totals({ requests: 1, input_tokens: input }) });
const days = (models: Record<string, number> = {}) => Array.from({ length: 30 }, (_, index) => ({
  day: `2026-10-${String(index + 1).padStart(2, "0")}`,
  models: index === 29 ? models : {},
  requests: index === 29 && Object.keys(models).length > 0 ? 3 : 0,
}));

function summary(patch: Partial<UsageSummary> = {}): UsageSummary {
  return {
    available: true, scope: "themis_only", generated_at_ms: 0, week_starts: "monday",
    windows: { five_hours: totals(), today: totals(), week: totals(), month: totals() },
    days: days(), by_provider: [], by_model: [], by_chat: [], billing: [],
    ...patch,
  };
}

async function show(value: UsageSummary) {
  vi.mocked(bridge.getUsageSummary).mockResolvedValue(value);
  await act(async () => { render(<UsageSettingsSection />); });
}

describe("Settings › Usage", () => {
  it("shows the windows with cached share and reasoning, labelled from Themis only, and no money without prices", async () => {
    const week = totals({ requests: 12, input_tokens: 1_200_000, cached_input_tokens: 900_000, output_tokens: 20_100, reasoning_tokens: 5_000 });
    await show(summary({ windows: { five_hours: totals({ requests: 2, input_tokens: 950, output_tokens: 10 }), today: week, week, month: week } }));
    expect(screen.getByText("Tokens · from Themis only")).toBeInTheDocument();
    const rows = screen.getAllByRole("row");
    expect(rows.map(row => row.textContent)).toEqual([
      "WindowInputOutputRequests",
      "5 hours950102",
      "This week1.20M · 75% cached20.1k · 5.0k reasoning12",
      "This month1.20M · 75% cached20.1k · 5.0k reasoning12",
    ]);
    expect(screen.queryByText(/\$/)).not.toBeInTheDocument();
    expect(screen.queryByText(/%.*allowance|of plan/i)).not.toBeInTheDocument();
    // Nothing recorded in 30 days: no chart, no chats.
    expect(screen.queryByText("Last 30 days")).not.toBeInTheDocument();
    expect(screen.queryByText("Top chats · 30 days")).not.toBeInTheDocument();
  });

  it("estimates list-price cost with ≈, marks unpriced requests and free models", async () => {
    const month = totals({ requests: 4, input_tokens: 1000, output_tokens: 10, estimated_cost_usd: 1.234, unpriced_requests: 1 });
    await show(summary({
      windows: { five_hours: totals(), today: month, week: month, month },
      days: days({ paid: 900, "free-model": 110 }),
      by_model: [model("paid", 900), model("free-model", 110, true)],
    }));
    expect(screen.getByRole("columnheader", { name: "≈ List price" })).toBeInTheDocument();
    expect(screen.getAllByText("≈$1.23+")).toHaveLength(2);
    expect(screen.getByText("—")).toBeInTheDocument();
    const legend = screen.getByRole("list", { name: "Models" });
    expect(within(legend).getByText("Free")).toBeInTheDocument();
  });

  it("charts 30 days by model, folding models past the fourth into Other, with a hover breakdown", async () => {
    const models = { a: 500, b: 400, c: 300, d: 200, e: 100 };
    await show(summary({
      windows: { five_hours: totals(), today: totals({ requests: 3 }), week: totals({ requests: 3 }), month: totals({ requests: 3 }) },
      days: days(models),
      by_model: Object.entries(models).map(([key, input]) => model(key, input)),
    }));
    const legend = within(screen.getByRole("list", { name: "Models" })).getAllByRole("listitem").map(item => item.textContent);
    expect(legend).toEqual(["a500", "b400", "c300", "Other300"]);
    const bars = screen.getAllByRole("img");
    expect(bars).toHaveLength(30);
    const last = bars[29];
    expect(last).toHaveAccessibleName(/1\.5k tokens, a 500, b 400, c 300, Other 300$/);
    expect(last.querySelectorAll("span")).toHaveLength(4);
    fireEvent.mouseEnter(last);
    expect(screen.getByRole("status")).toHaveTextContent("1.5k tokens · 3 requests");
    fireEvent.mouseLeave(last.parentElement!.parentElement!);
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("lists top chats with deleted chats last", async () => {
    await show(summary({
      days: days({ m: 10 }),
      by_model: [model("m", 10)],
      by_chat: [
        { key: "t1", title: "Fix the parser", totals: totals({ requests: 7, input_tokens: 12_000, output_tokens: 300 }) },
        { key: null, totals: totals({ requests: 1, input_tokens: 10, output_tokens: 1 }) },
      ],
    }));
    expect(screen.getByText("Fix the parser")).toBeInTheDocument();
    expect(screen.getByText("12.3k tokens · 7 requests")).toBeInTheDocument();
    expect(screen.getByText("Deleted chats")).toBeInTheDocument();
  });

  it("says when usage cannot be recorded, and shows a load failure", async () => {
    await show(summary({ available: false }));
    expect(screen.getByRole("alert")).toHaveTextContent("new requests are not being counted");
    cleanup();
    vi.mocked(bridge.getUsageSummary).mockRejectedValue(new Error("server offline"));
    await act(async () => { render(<UsageSettingsSection />); });
    expect(screen.getByRole("alert")).toHaveTextContent("server offline");
  });

  it("refreshes on request", async () => {
    await show(summary());
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Refresh usage" })); });
    expect(bridge.getUsageSummary).toHaveBeenCalledTimes(2);
  });
});

describe("usage formatting", () => {
  it("compacts token counts and cost labels", () => {
    expect([999, 1000, 12_345, 4_560_000].map(compactTokens)).toEqual(["999", "1.0k", "12.3k", "4.56M"]);
    expect(costLabel(totals())).toBeNull();
    expect(costLabel(totals({ estimated_cost_usd: 0 }))).toBe("≈$0.00");
    expect(costLabel(totals({ estimated_cost_usd: 0.004 }))).toBe("≈<$0.01");
  });
});
