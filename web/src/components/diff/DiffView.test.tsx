import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import type { DiffFile } from "../../lib/types";
import { DiffView } from "./DiffView";

beforeEach(() => {
  const store = new Map<string, string>();
  vi.stubGlobal("localStorage", { getItem: (key: string) => store.get(key) ?? null, setItem: (key: string, value: string) => { store.set(key, value); } });
});

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

  it("starts unified, with no controls that change anything but the layout", () => {
    render(<DiffView file={FILE} />);
    expect(screen.getAllByRole("button").map(button => button.textContent)).toEqual(["Unified", "Split"]);
    expect(screen.getByText(line("old line"))).toBeInTheDocument();
    expect(screen.getByText(line("new line"))).toBeInTheDocument();
  });

  it("shows an empty state when there are no hunks", () => {
    render(<DiffView file={{ ...FILE, hunks: [] }} />);
    expect(screen.getByText("No changes")).toBeInTheDocument();
  });

  it("shows the same change side by side, and remembers the choice", () => {
    const file = { path: "a.py", status: "modified", preexisting: false, added: 2, removed: 1, hunks: [{ old_start: 1, old_lines: 2, new_start: 1, new_lines: 3, lines: [
      { kind: "context", text: "keep" }, { kind: "del", text: "old" }, { kind: "add", text: "new" }, { kind: "add", text: "extra" },
    ] }] } as never;
    const { container, unmount } = render(<DiffView file={file} />);
    expect(screen.getByRole("button", { name: "Unified" })).toHaveAttribute("aria-pressed", "true");
    expect(container.querySelector(".themis-diff-side-row")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Split" }));
    const rows = [...container.querySelectorAll(".themis-diff-side-row")].map(row => [...row.querySelectorAll(".themis-diff-cell")].map(cell => cell.querySelector(".themis-diff-text")?.textContent ?? ""));
    expect(rows).toEqual([["keep", "keep"], ["old", "new"], ["", "extra"]]);
    unmount();
    render(<DiffView file={file} />);
    expect(screen.getByRole("button", { name: "Split" })).toHaveAttribute("aria-pressed", "true");
  });
});
