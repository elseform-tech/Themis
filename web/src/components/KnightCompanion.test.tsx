// @vitest-environment jsdom
import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { KnightCompanion, type CompanionPreferences } from "./KnightCompanion";

const preferences: CompanionPreferences = { variant: "honey", enabled: true, size: 88, position: { x: 1, y: 0.65 } };
afterEach(() => { cleanup(); vi.useRealTimers(); });

describe("Bubbles companion", () => {
  it("renders the bubble artwork and announces attention until it clears", () => {
    const view = render(<KnightCompanion preferences={preferences} onChange={vi.fn()} mood="attention" />);
    expect(view.container.querySelector("title")?.textContent).toBe("Themis Honey bubble");
    expect(screen.getByRole("status", { name: "" })).toHaveTextContent("Needs you");
    view.rerender(<KnightCompanion preferences={preferences} onChange={vi.fn()} mood="working" />);
    expect(screen.queryByText("Needs you")).toBeNull();
  });
  it("says all done briefly after completion", () => {
    vi.useFakeTimers();
    render(<KnightCompanion preferences={preferences} onChange={vi.fn()} mood="complete" />);
    expect(screen.getByText("All done!")).toBeInTheDocument();
    act(() => { vi.advanceTimersByTime(6000); });
    expect(screen.queryByText("All done!")).toBeNull();
  });
});
