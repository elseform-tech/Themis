import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, fireEvent } from "@testing-library/react";
import type { DiffFile } from "../../lib/types";
import { changedRange, DiffView, hunkGaps, SPLIT_MIN_WIDTH } from "./DiffView";

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
  it("renders hunk content in the well with no header bar: hunks part by a hairline, named for screen readers", () => {
    const { container } = render(<DiffView file={FILE} />);
    expect(screen.getByText(line("line one"))).toBeInTheDocument();
    expect(screen.getByText(line("old line"))).toBeInTheDocument();
    expect(screen.getByText(line("new line"))).toBeInTheDocument();
    // The canvas well: no "Line 1" row, no path or counts bar (the file's row above shows those).
    expect(screen.queryByText("Line 1")).toBeNull();
    expect(screen.getByRole("separator", { name: "Line 1" })).toHaveTextContent("");
    expect(container.querySelector(".themis-diff-head, .themis-diff-path, .themis-diff-hunk-header")).toBeNull();
    expect(screen.getByRole("region", { name: `Changes in ${FILE.path}` })).toBeInTheDocument();
    // Unified and Split stay, as a compact control over the well's top right.
    expect(screen.getByRole("group", { name: "Diff layout" }).closest(".themis-diff")).not.toBeNull();
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

  it("leaves the add/del counts to the file's row above the well", () => {
    render(<DiffView file={FILE} />);
    expect(screen.queryByText("+1")).toBeNull();
    expect(screen.queryByText("-1")).toBeNull();
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

  it("marks just what changed inside a replaced line", () => {
    expect(changedRange("const total = 1;", "const total = 22;")).toEqual({ oldStart: 14, oldEnd: 15, newStart: 14, newEnd: 16 });
    expect(changedRange("same", "same")).toBeNull();
    // Nothing in common isn't worth marking: the whole line is already tinted.
    expect(changedRange("alpha", "zzzzz")).toBeNull();
    const file = { path: "a.txt", status: "modified", preexisting: false, added: 1, removed: 1, hunks: [{ old_start: 1, old_lines: 1, new_start: 1, new_lines: 1, lines: [
      { kind: "del", text: "const total = 1;" }, { kind: "add", text: "const total = 22;" },
    ] }] } as never;
    localStorage.setItem("themis:workspaceDiffLayout", JSON.stringify("split"));
    const { container } = render(<DiffView file={file} />);
    expect([...container.querySelectorAll(".themis-diff-word")].map(mark => mark.textContent)).toEqual(["1", "22"]);
  });

  it("uses one column for a file that only has one side", () => {
    localStorage.setItem("themis:workspaceDiffLayout", JSON.stringify("split"));
    const file = { path: "new.py", status: "added", preexisting: false, added: 1, removed: 0, hunks: [{ old_start: 0, old_lines: 0, new_start: 1, new_lines: 1, lines: [{ kind: "add", text: "x = 1" }] }] } as never;
    const { container } = render(<DiffView file={file} />);
    expect(container.querySelector(".themis-diff-side-row")).toBeNull();
    expect(container.querySelector(".themis-diff-unified")).not.toBeNull();
    expect(screen.getByRole("button", { name: "Split" })).toBeDisabled();
  });

  it("asks for more room when Split is chosen", () => {
    const onWantWidth = vi.fn();
    render(<DiffView file={FILE} onWantWidth={onWantWidth} />);
    fireEvent.click(screen.getByRole("button", { name: "Split" }));
    expect(onWantWidth).toHaveBeenCalledWith(SPLIT_MIN_WIDTH);
    onWantWidth.mockClear();
    fireEvent.click(screen.getByRole("button", { name: "Unified" }));
    expect(onWantWidth).not.toHaveBeenCalled();
  });
});

// Two hunks of a 100-line file: lines 10-12 changed at 11, lines 60-62 changed at 61. Unchanged between them: 13-59.
const LONG: DiffFile = {
  path: "src/long.ts", status: "modified", preexisting: false, added: 2, removed: 2,
  hunks: [
    { old_start: 10, old_lines: 3, new_start: 10, new_lines: 3, lines: [{ kind: "context", text: "l10" }, { kind: "del", text: "old 11" }, { kind: "add", text: "l11" }, { kind: "context", text: "l12" }] },
    { old_start: 60, old_lines: 3, new_start: 60, new_lines: 3, lines: [{ kind: "context", text: "l60" }, { kind: "del", text: "old 61" }, { kind: "add", text: "l61" }, { kind: "context", text: "l62" }] },
  ],
};
const TEXT = Array.from({ length: 100 }, (_, i) => `l${i + 1}`).join("\n");

describe("DiffView hidden lines", () => {
  it("counts the unchanged lines before and between hunks", () => {
    expect(hunkGaps(LONG.hunks)).toEqual([{ before: 0, from: 1, to: 9, offset: 0 }, { before: 1, from: 13, to: 59, offset: 0 }]);
    // A hunk that only adds moves the old numbering on.
    expect(hunkGaps([{ old_start: 3, old_lines: 0, new_start: 4, new_lines: 2, lines: [] }, { old_start: 10, old_lines: 1, new_start: 12, new_lines: 1, lines: [] }]))
      .toEqual([{ before: 0, from: 1, to: 3, offset: 0 }, { before: 1, from: 6, to: 11, offset: 2 }]);
  });

  it("shows each gap as a quiet row, reveals 20 lines nearest the hunk below, and all on Shift- or Alt-click", async () => {
    const loadText = vi.fn(async () => TEXT);
    render(<DiffView file={LONG} loadText={loadText} />);
    expect(screen.getByRole("button", { name: "⋯ 9 unchanged lines" })).toBeInTheDocument();
    const between = screen.getByRole("button", { name: "⋯ 47 unchanged lines" });
    await act(async () => { fireEvent.click(between); });
    // Lines 40-59, just above the second hunk, now show; 27 stay hidden.
    expect(screen.getByRole("button", { name: "⋯ 27 unchanged lines" })).toBeInTheDocument();
    expect(screen.getByText(line("l40"))).toBeInTheDocument();
    expect(screen.getByText(line("l59"))).toBeInTheDocument();
    expect(screen.queryByText(line("l39"))).toBeNull();
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "⋯ 27 unchanged lines" }), { shiftKey: true }); });
    expect(screen.queryByRole("button", { name: /27 unchanged/ })).toBeNull();
    expect(screen.getByText(line("l13"))).toBeInTheDocument();
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "⋯ 9 unchanged lines" }), { altKey: true }); });
    expect(screen.getByText(line("l1"))).toBeInTheDocument();
    // The file text is read once.
    expect(loadText).toHaveBeenCalledTimes(1);
  });

  it("reveals the same lines in Split, on both sides", async () => {
    localStorage.setItem("themis:workspaceDiffLayout", JSON.stringify("split"));
    render(<DiffView file={LONG} loadText={async () => TEXT} />);
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "⋯ 47 unchanged lines" })); });
    expect(screen.getAllByText(line("l50"))).toHaveLength(2);
  });

  it("says so when the file can't be read, and keeps the row", async () => {
    render(<DiffView file={LONG} loadText={async () => null} />);
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "⋯ 47 unchanged lines" })); });
    expect(screen.getByRole("button", { name: "⋯ 47 unchanged lines, can't be shown" })).toBeDisabled();
  });
});
