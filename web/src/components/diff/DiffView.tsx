import { useState } from "react";
import type { DiffFile } from "../../lib/types";
import { EmptyState } from "../primitives/EmptyState";
import "./DiffView.css";

export type DiffMode = "unified" | "side-by-side";

export interface DiffViewProps {
  file: DiffFile;
  defaultMode?: DiffMode;
}

interface NumberedLine {
  kind: "context" | "add" | "del";
  text: string;
  oldNo: number | null;
  newNo: number | null;
}

interface SideRow {
  left: NumberedLine | null;
  right: NumberedLine | null;
}

function hunkHeader(oldStart: number, oldLines: number, newStart: number, newLines: number): string {
  return `@@ -${oldStart},${oldLines} +${newStart},${newLines} @@`;
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

/** Pair consecutive del runs with following add runs for side-by-side rows. */
function toSideRows(lines: NumberedLine[]): SideRow[] {
  const rows: SideRow[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (line === undefined) break;
    if (line.kind === "context") {
      rows.push({ left: line, right: line });
      i += 1;
      continue;
    }
    const dels: NumberedLine[] = [];
    const adds: NumberedLine[] = [];
    while (i < lines.length && lines[i]?.kind === "del") {
      dels.push(lines[i] as NumberedLine);
      i += 1;
    }
    while (i < lines.length && lines[i]?.kind === "add") {
      adds.push(lines[i] as NumberedLine);
      i += 1;
    }
    const count = Math.max(dels.length, adds.length);
    for (let k = 0; k < count; k++) {
      rows.push({ left: dels[k] ?? null, right: adds[k] ?? null });
    }
  }
  return rows;
}

export function DiffView({ file, defaultMode = "unified" }: DiffViewProps) {
  const [mode, setMode] = useState<DiffMode>(defaultMode);

  let addCount = 0;
  let delCount = 0;
  for (const hunk of file.hunks) {
    for (const line of hunk.lines) {
      if (line.kind === "add") addCount += 1;
      if (line.kind === "del") delCount += 1;
    }
  }

  return (
    <div className="themis-diff">
      <div className="themis-diff-head">
        <span className="themis-diff-path" title={file.path}>
          {file.path}
        </span>
        <span className="themis-diff-counts">
          <span className="themis-diff-count-add">+{addCount}</span>
          <span className="themis-diff-count-del">-{delCount}</span>
        </span>
        <div role="group" aria-label="Diff mode" className="themis-diff-toggle">
          <button
            type="button"
            aria-pressed={mode === "unified"}
            className={
              mode === "unified"
                ? "themis-diff-toggle-btn themis-diff-toggle-btn--active"
                : "themis-diff-toggle-btn"
            }
            onClick={() => setMode("unified")}
          >
            Unified
          </button>
          <button
            type="button"
            aria-pressed={mode === "side-by-side"}
            className={
              mode === "side-by-side"
                ? "themis-diff-toggle-btn themis-diff-toggle-btn--active"
                : "themis-diff-toggle-btn"
            }
            onClick={() => setMode("side-by-side")}
          >
            Side-by-side
          </button>
        </div>
      </div>

      {file.hunks.length === 0 ? (
        <EmptyState title="No changes" hint={`${file.path} has no hunks to display.`} />
      ) : mode === "unified" ? (
        <div className="themis-diff-unified">
          {file.hunks.map((hunk, hunkIndex) => (
            <div key={hunkIndex} className="themis-diff-hunk">
              <div className="themis-diff-hunk-header">
                {hunkHeader(hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines)}
              </div>
              {numberHunk(
                hunk.old_start,
                hunk.new_start,
                hunk.lines.map((l) => ({ ...l, oldNo: null, newNo: null })),
              ).map((line, lineIndex) => (
                <div
                  key={lineIndex}
                  className={`themis-diff-line themis-diff-line--${line.kind}`}
                >
                  <span className="themis-diff-no">
                    {line.oldNo ?? ""}
                  </span>
                  <span className="themis-diff-no">
                    {line.newNo ?? ""}
                  </span>
                  <span className="themis-diff-sign">
                    {line.kind === "add" ? "+" : line.kind === "del" ? "-" : " "}
                  </span>
                  <span className="themis-diff-text">{line.text}</span>
                </div>
              ))}
            </div>
          ))}
        </div>
      ) : (
        <div className="themis-diff-side">
          {file.hunks.map((hunk, hunkIndex) => (
            <div key={hunkIndex} className="themis-diff-hunk">
              <div className="themis-diff-hunk-header">
                {hunkHeader(hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines)}
              </div>
              <div className="themis-diff-side-grid">
                {toSideRows(
                  numberHunk(
                    hunk.old_start,
                    hunk.new_start,
                    hunk.lines.map((l) => ({ ...l, oldNo: null, newNo: null })),
                  ),
                ).map((row, rowIndex) => (
                  <div key={rowIndex} className="themis-diff-side-row">
                    <div
                      className={`themis-diff-cell themis-diff-line--${row.left?.kind ?? "empty"}`}
                    >
                      <span className="themis-diff-no">{row.left?.oldNo ?? ""}</span>
                      <span className="themis-diff-text">{row.left?.text ?? ""}</span>
                    </div>
                    <div
                      className={`themis-diff-cell themis-diff-line--${row.right?.kind ?? "empty"}`}
                    >
                      <span className="themis-diff-no">{row.right?.newNo ?? ""}</span>
                      <span className="themis-diff-text">{row.right?.text ?? ""}</span>
                    </div>
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
