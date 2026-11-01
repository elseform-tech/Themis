import { useState } from "react";
import type { DiffFile } from "../../lib/types";
import { readSession, writeSession } from "../../state/session";
import { EmptyState } from "../primitives/EmptyState";
import { highlightLines } from "../CodeBlock";
import "../CodeBlock.css";
import "./DiffView.css";

export interface DiffViewProps {
  file: DiffFile;
}

type DiffLayout = "unified" | "split";
const LAYOUT_KEY = "workspaceDiffLayout";
/** A longer line (minified code) stays plain. */
const MAX_COLOURED_LINE = 2000;

export interface NumberedLine {
  kind: "context" | "add" | "del";
  text: string;
  oldNo: number | null;
  newNo: number | null;
}

function hunkHeader(oldStart: number, oldLines: number, newStart: number, newLines: number): string {
  // Plain location instead of @@ hunk syntax; deletions-only hunks point at the old file.
  return newLines > 0 ? `Line ${newStart}` : `Line ${oldStart} (removed${oldLines > 1 ? ` ${oldLines} lines` : ""})`;
}

function numberHunk(
  oldStart: number,
  newStart: number,
  lines: NumberedLine[],
): NumberedLine[] {
  let oldNo = oldStart;
  let newNo = newStart;
  return lines.map((line) => {
    if (line.kind === "del") {
      const numbered = { ...line, oldNo, newNo: null };
      oldNo += 1;
      return numbered;
    }
    if (line.kind === "add") {
      const numbered = { ...line, oldNo: null, newNo };
      newNo += 1;
      return numbered;
    }
    const numbered = { ...line, oldNo, newNo };
    oldNo += 1;
    newNo += 1;
    return numbered;
  });
}

/** Rows of a side-by-side view: a removal lines up with the addition that replaced it. */
export function sideBySide(lines: NumberedLine[]): Array<[NumberedLine | null, NumberedLine | null]> {
  const rows: Array<[NumberedLine | null, NumberedLine | null]> = [];
  let at = 0;
  while (at < lines.length) {
    const line = lines[at]!;
    if (line.kind === "context") {
      rows.push([line, line]);
      at += 1;
      continue;
    }
    const removed: NumberedLine[] = [];
    const added: NumberedLine[] = [];
    while (lines[at]?.kind === "del") removed.push(lines[at++]!);
    while (lines[at]?.kind === "add") added.push(lines[at++]!);
    for (let row = 0; row < Math.max(removed.length, added.length); row += 1) rows.push([removed[row] ?? null, added[row] ?? null]);
  }
  return rows;
}

function LineText({ text, extension }: { text: string; extension: string | undefined }) {
  const tokens = text.length > MAX_COLOURED_LINE ? [{ kind: "plain" as const, text }] : highlightLines(text, extension)[0] ?? [];
  return <span className="themis-diff-text">{tokens.map((token, index) => token.kind === "plain" ? token.text : <span key={index} className={`themis-codeblock-token--${token.kind}`}>{token.text}</span>)}</span>;
}

/** Read-only diff of one file, unified or side by side; the choice is remembered. */
export function DiffView({ file }: DiffViewProps) {
  const extension = file.path.split(".").pop();
  const [layout, setLayoutState] = useState<DiffLayout>(() => readSession<DiffLayout>(LAYOUT_KEY, "unified"));
  const setLayout = (next: DiffLayout) => { setLayoutState(next); writeSession(LAYOUT_KEY, next); };

  return (
    <div className="themis-diff">
      <div className="themis-diff-head">
        <span className="themis-diff-path" title={file.path}>
          {file.path}
        </span>
        <span className="themis-diff-counts">
          <span className="themis-diff-count-add">+{file.added}</span>
          <span className="themis-diff-count-del">-{file.removed}</span>
        </span>
        <span className="themis-diff-toggle" role="group" aria-label="Diff layout">
          {(["unified", "split"] as const).map(option => (
            <button key={option} type="button" aria-pressed={layout === option} className={`themis-diff-toggle-btn${layout === option ? " themis-diff-toggle-btn--active" : ""}`} onClick={() => setLayout(option)}>
              {option === "unified" ? "Unified" : "Split"}
            </button>
          ))}
        </span>
      </div>

      {file.too_large && file.hunks.length === 0 ? (
        <EmptyState title="Too large to show" hint={`${file.path} changed too much to show line by line.`} />
      ) : file.hunks.length === 0 ? (
        <EmptyState title="No changes" hint={`${file.path} has no hunks to display.`} />
      ) : (
        <div className={layout === "split" ? "themis-diff-side" : "themis-diff-unified"}>
          {file.hunks.map((hunk, hunkIndex) => {
            const lines = numberHunk(hunk.old_start, hunk.new_start, hunk.lines.map((l) => ({ ...l, oldNo: null, newNo: null })));
            return (
              <div key={hunkIndex} className="themis-diff-hunk">
                <div className="themis-diff-hunk-header">
                  {hunkHeader(hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines)}
                </div>
                {layout === "split"
                  ? sideBySide(lines).map(([left, right], rowIndex) => (
                    <div key={rowIndex} className="themis-diff-side-row">
                      {[left, right].map((line, side) => (
                        <div key={side} className={`themis-diff-cell themis-diff-line--${line ? line.kind : "empty"}`}>
                          {line && <>
                            <span className="themis-diff-no">{side === 0 ? line.oldNo : line.newNo}</span>
                            <LineText text={line.text} extension={extension} />
                          </>}
                        </div>
                      ))}
                    </div>
                  ))
                  : lines.map((line, lineIndex) => (
                    <div key={lineIndex} className={`themis-diff-line themis-diff-line--${line.kind}`}>
                      <span className="themis-diff-no">{line.oldNo ?? ""}</span>
                      <span className="themis-diff-no">{line.newNo ?? ""}</span>
                      <span className="themis-diff-sign">{line.kind === "add" ? "+" : line.kind === "del" ? "-" : " "}</span>
                      <LineText text={line.text} extension={extension} />
                    </div>
                  ))}
              </div>
            );
          })}
          {file.too_large && <p className="themis-diff-note">Showing the first lines only; the rest of this change is too large to show.</p>}
        </div>
      )}
    </div>
  );
}
