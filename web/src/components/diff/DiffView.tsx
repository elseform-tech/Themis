import { useEffect, useRef, useState, type ReactNode } from "react";
import type { DiffFile } from "../../lib/types";
import { readSession, writeSession } from "../../state/session";
import { EmptyState } from "../primitives/EmptyState";
import { highlightLines } from "../CodeBlock";
import "../CodeBlock.css";
import "./DiffView.css";

export interface DiffViewProps {
  file: DiffFile;
  /** Asks for this much room when Split is chosen and it hasn't got it. */
  onWantWidth?: (width: number) => void;
}

type DiffLayout = "unified" | "split";
const LAYOUT_KEY = "workspaceDiffLayout";
/** A longer line (minified code) stays plain. */
const MAX_COLOURED_LINE = 2000;
/** What two columns of code want, and below what they aren't readable at all. */
export const SPLIT_MIN_WIDTH = 760;
const SPLIT_NARROWEST = 480;

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

/** Where a replaced line differs from what replaced it: what's left after their common start
 *  and end. `null` when they're the same or share nothing, since the whole line is tinted anyway. */
export function changedRange(before: string, after: string): { oldStart: number; oldEnd: number; newStart: number; newEnd: number } | null {
  if (before === after) return null;
  const longest = Math.min(before.length, after.length);
  let start = 0;
  while (start < longest && before[start] === after[start]) start += 1;
  let end = 0;
  while (end < longest - start && before[before.length - 1 - end] === after[after.length - 1 - end]) end += 1;
  if (start + end === 0) return null;
  return { oldStart: start, oldEnd: before.length - end, newStart: start, newEnd: after.length - end };
}

function LineText({ text, extension, mark }: { text: string; extension: string | undefined; mark?: [number, number] }) {
  const tokens = text.length > MAX_COLOURED_LINE ? [{ kind: "plain" as const, text }] : highlightLines(text, extension)[0] ?? [];
  const pieces: ReactNode[] = [];
  let at = 0;
  tokens.forEach((token, index) => {
    const from = at;
    at += token.text.length;
    // Cut at the mark, so colours and the mark can overlap freely.
    const cuts = [0, token.text.length];
    if (mark) for (const edge of mark) if (edge > from && edge < at) cuts.push(edge - from);
    cuts.sort((left, right) => left - right);
    cuts.slice(1).forEach((cut, part) => {
      const piece = token.text.slice(cuts[part], cut);
      const marked = mark !== undefined && from + cuts[part]! >= mark[0] && from + cut <= mark[1] && piece.length > 0;
      const className = [token.kind === "plain" ? "" : `themis-codeblock-token--${token.kind}`, marked ? "themis-diff-word" : ""].filter(Boolean).join(" ");
      pieces.push(className ? <span key={`${index}-${part}`} className={className}>{piece}</span> : piece);
    });
  });
  return <span className="themis-diff-text">{pieces}</span>;
}

/** One side of a split row. */
function SideCell({ line, side, extension, mark }: { line: NumberedLine | null; side: 0 | 1; extension: string | undefined; mark?: [number, number] }) {
  return (
    <div className={`themis-diff-cell themis-diff-line--${line ? line.kind : "empty"}`}>
      {line && <>
        <span className="themis-diff-no">{side === 0 ? line.oldNo : line.newNo}</span>
        <span className="themis-diff-sign">{line.kind === "add" ? "+" : line.kind === "del" ? "-" : " "}</span>
        <LineText text={line.text} extension={extension} mark={mark} />
      </>}
    </div>
  );
}

/** Read-only diff of one file, unified or side by side; the choice is remembered. A file with
 *  only one side, or too little room for two, reads better in one column. */
export function DiffView({ file, onWantWidth }: DiffViewProps) {
  const extension = file.path.split(".").pop();
  const [layout, setLayoutState] = useState<DiffLayout>(() => readSession<DiffLayout>(LAYOUT_KEY, "unified"));
  const root = useRef<HTMLDivElement>(null);
  const [cramped, setCramped] = useState(false);
  useEffect(() => {
    const element = root.current;
    if (!element || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => setCramped(element.clientWidth < SPLIT_NARROWEST));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  const oneSided = file.status === "added" || file.status === "deleted";
  const splitOff = oneSided || cramped;
  const showing = layout === "split" && !splitOff ? "split" : "unified";
  const setLayout = (next: DiffLayout) => {
    setLayoutState(next);
    writeSession(LAYOUT_KEY, next);
    if (next === "split" && (root.current?.clientWidth ?? 0) < SPLIT_MIN_WIDTH) onWantWidth?.(SPLIT_MIN_WIDTH);
  };

  return (
    <div className="themis-diff" ref={root}>
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
            <button key={option} type="button" aria-pressed={showing === option} disabled={option === "split" && splitOff}
              title={option === "split" && splitOff ? (oneSided ? "This file only has one side" : "Widen the panel for two columns") : undefined}
              className={`themis-diff-toggle-btn${showing === option ? " themis-diff-toggle-btn--active" : ""}`} onClick={() => setLayout(option)}>
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
        <div className={showing === "split" ? "themis-diff-side" : "themis-diff-unified"}>
          {file.hunks.map((hunk, hunkIndex) => {
            const lines = numberHunk(hunk.old_start, hunk.new_start, hunk.lines.map((l) => ({ ...l, oldNo: null, newNo: null })));
            return (
              <div key={hunkIndex} className="themis-diff-hunk">
                <div className="themis-diff-hunk-header">
                  {hunkHeader(hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines)}
                </div>
                {showing === "split"
                  ? sideBySide(lines).map(([left, right], rowIndex) => {
                    const range = left && right && left.kind === "del" && right.kind === "add" ? changedRange(left.text, right.text) : null;
                    return (
                      <div key={rowIndex} className="themis-diff-side-row">
                        <SideCell line={left} side={0} extension={extension} mark={range ? [range.oldStart, range.oldEnd] : undefined} />
                        <SideCell line={right} side={1} extension={extension} mark={range ? [range.newStart, range.newEnd] : undefined} />
                      </div>
                    );
                  })
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
