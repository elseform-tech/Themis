import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
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
    expect(screen.getByText("Line 1")).toBeInTheDocument();
  });

  it("shows add/del counts", () => {
    render(<DiffView file={FILE} />);
    expect(screen.getByText("+1")).toBeInTheDocument();
    expect(screen.getByText("-1")).toBeInTheDocument();
  });

  it("renders a single read-only unified view without mode controls", () => {
    render(<DiffView file={FILE} />);
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.getByText("old line")).toBeInTheDocument();
    expect(screen.getByText("new line")).toBeInTheDocument();
  });

  it("shows an empty state when there are no hunks", () => {
    render(<DiffView file={{ ...FILE, hunks: [] }} />);
    expect(screen.getByText("No changes")).toBeInTheDocument();
  });
});
