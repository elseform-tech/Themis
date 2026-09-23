import { describe, expect, it } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import type { DiffFile } from "../../lib/types";
import { DiffView } from "./DiffView";

const FILE: DiffFile = {
  path: "src/main.ts",
  status: "modified",
  preexisting: false,
  hunks: [
    {
      old_start: 1,
      old_lines: 3,
      new_start: 1,
      new_lines: 3,
      lines: [
        { kind: "context", text: "line one" },
        { kind: "del", text: "old line" },
        { kind: "add", text: "new line" },
      ],
    },
  ],
};

describe("DiffView", () => {
  it("renders hunk content and hunk header", () => {
    render(<DiffView file={FILE} />);
    expect(screen.getByText("line one")).toBeInTheDocument();
    expect(screen.getByText("old line")).toBeInTheDocument();
    expect(screen.getByText("new line")).toBeInTheDocument();
    expect(screen.getByText("@@ -1,3 +1,3 @@")).toBeInTheDocument();
  });

  it("shows add/del counts", () => {
    render(<DiffView file={FILE} />);
    expect(screen.getByText("+1")).toBeInTheDocument();
    expect(screen.getByText("-1")).toBeInTheDocument();
  });

  it("toggles between unified and side-by-side modes", () => {
    render(<DiffView file={FILE} />);
    const sideButton = screen.getByRole("button", { name: "Side-by-side" });
    fireEvent.click(sideButton);
    expect(sideButton).toHaveAttribute("aria-pressed", "true");
    // Side-by-side renders paired cells; both texts still visible.
    expect(screen.getByText("old line")).toBeInTheDocument();
    expect(screen.getByText("new line")).toBeInTheDocument();
    const unifiedButton = screen.getByRole("button", { name: "Unified" });
    fireEvent.click(unifiedButton);
    expect(unifiedButton).toHaveAttribute("aria-pressed", "true");
  });

  it("respects defaultMode", () => {
    render(<DiffView file={FILE} defaultMode="side-by-side" />);
    expect(
      screen.getByRole("button", { name: "Side-by-side" }),
    ).toHaveAttribute("aria-pressed", "true");
  });

  it("shows an empty state when there are no hunks", () => {
    render(<DiffView file={{ ...FILE, hunks: [] }} />);
    expect(screen.getByText("No changes")).toBeInTheDocument();
  });
});
