import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import type { DiffFile } from "../../lib/types";
import { DiffView } from "./DiffView";

const FILE: DiffFile = {
  path: "src/main.ts",
  status: "modified",
  preexisting: false, added: 1, removed: 1,
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

/** Matches a whole diff line even when highlighting splits it into tokens. */
const line = (text: string) => (_: string, element: Element | null) => element?.classList.contains("themis-diff-text") === true && element.textContent === text;

describe("DiffView", () => {
  it("renders hunk content and hunk header", () => {
    render(<DiffView file={FILE} />);
    expect(screen.getByText(line("line one"))).toBeInTheDocument();
    expect(screen.getByText(line("old line"))).toBeInTheDocument();
    expect(screen.getByText(line("new line"))).toBeInTheDocument();
    expect(screen.getByText("Line 1")).toBeInTheDocument();
  });

  it("highlights code tokens and wraps instead of scrolling sideways", () => {
    const { container } = render(<DiffView file={FILE} />);
    expect(container.querySelector(".themis-codeblock-token--keyword")).toHaveTextContent("new");
    expect(container.querySelector(".themis-diff-text")).toBeInTheDocument();
  });

  it("says when a change is too large to show", () => {
    render(<DiffView file={{ path: "data/stars.csv", status: "added", preexisting: false, added: 0, removed: 0, hunks: [], too_large: true }} />);
    expect(screen.getByText("Too large to show")).toBeInTheDocument();
  });

  it("shows the first lines of a large change with a note", () => {
    render(<DiffView file={{ ...FILE, too_large: true }} />);
    expect(screen.getByText(/Showing the first lines only/)).toBeInTheDocument();
    expect(screen.getByText(line("new line"))).toBeInTheDocument();
  });

  it("shows add/del counts", () => {
    render(<DiffView file={FILE} />);
    expect(screen.getByText("+1")).toBeInTheDocument();
    expect(screen.getByText("-1")).toBeInTheDocument();
  });

  it("renders a single read-only unified view without mode controls", () => {
    render(<DiffView file={FILE} />);
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.getByText(line("old line"))).toBeInTheDocument();
    expect(screen.getByText(line("new line"))).toBeInTheDocument();
  });

  it("shows an empty state when there are no hunks", () => {
    render(<DiffView file={{ ...FILE, hunks: [] }} />);
    expect(screen.getByText("No changes")).toBeInTheDocument();
  });
});
