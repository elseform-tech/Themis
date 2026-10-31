import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import type { DiffFile } from "../../lib/types";
import { FileTree } from "./FileTree";

const FILES: DiffFile[] = [
  { path: "src/a.ts", status: "modified", preexisting: false, added: 0, removed: 0, hunks: [] },
  { path: "src/b.ts", status: "added", preexisting: true, added: 0, removed: 0, hunks: [] },
];

function setup(files: DiffFile[] = FILES) {
  const onSelect = vi.fn();
  const onAccept = vi.fn();
  const onDiscard = vi.fn();
  render(
    <FileTree
      files={files}
      selectedPath={null}
      onSelect={onSelect}
      onAccept={onAccept}
      onDiscard={onDiscard}
    />,
  );
  return { onSelect, onAccept, onDiscard };
}

describe("FileTree", () => {
  it("selects a file on click", () => {
    const { onSelect } = setup();
    fireEvent.click(screen.getByTitle("src/a.ts"));
    expect(onSelect).toHaveBeenCalledWith("src/a.ts");
  });

  it("fires accept and discard callbacks", () => {
    const { onAccept, onDiscard } = setup();
    fireEvent.click(screen.getByRole("button", { name: "Accept src/a.ts" }));
    expect(onAccept).toHaveBeenCalledWith("src/a.ts");
    fireEvent.click(screen.getByRole("button", { name: "Discard src/a.ts" }));
    expect(onDiscard).toHaveBeenCalledWith("src/a.ts");
  });

  it("disables discard for preexisting files", () => {
    setup();
    expect(
      screen.getByRole("button", { name: "Discard src/b.ts" }),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "Accept src/b.ts" }),
    ).not.toBeDisabled();
  });

  it("shows status glyphs for each file", () => {
    setup();
    expect(screen.getByText("M")).toBeInTheDocument();
    expect(screen.getByText("A")).toBeInTheDocument();
  });

  it("shows an empty state when there are no files", () => {
    setup([]);
    expect(screen.getByText("No changed files")).toBeInTheDocument();
  });

  it("groups large change lists by folder and filters them", () => {
    const many: DiffFile[] = Array.from({ length: 14 }, (_, i) => ({ path: `${i % 2 ? "docs" : "src"}/file-${i}.ts`, status: "modified", preexisting: false, added: 0, removed: 0, hunks: [] }));
    render(<FileTree files={many} selectedPath={null} onSelect={vi.fn()} />);
    expect(screen.getByRole("button", { name: /src\s*7/ })).toHaveAttribute("aria-expanded", "true");
    expect(screen.queryByRole("checkbox")).toBeNull();
    expect(screen.queryByRole("button", { name: /^Accept / })).toBeNull();
    fireEvent.change(screen.getByRole("searchbox", { name: "Filter changed files" }), { target: { value: "file-3" } });
    expect(screen.getByTitle("docs/file-3.ts")).toBeInTheDocument();
    expect(screen.queryByTitle("src/file-2.ts")).toBeNull();
  });
});
