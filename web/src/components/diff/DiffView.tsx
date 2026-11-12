import { useEffect, useMemo, useRef, useState, type MouseEvent, type ReactNode } from "react";
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
  /** The file as the diff's new side has it (the working tree, or a branch's tip), for its unchanged lines;
   *  null when it can't be read. Called once, on the first reveal. */
  loadText?: () => Promise<string | null>;
  /** Where the revealed unchanged lines (and the text read for them) live while the view stays open, so folding the
   *  file and unfolding it keeps them. */
  remember?: { initial?: HiddenLinesMemory; save: (memory: HiddenLinesMemory) => void };
}

/** What a diff has revealed of its unchanged lines: lines shown per gap, and the file's text once read. */
export interface HiddenLinesMemory { shown: Record<number, number>; lines: string[] | false | null }

/** Unchanged lines before a hunk, in the new file's numbering (`from`..`to`); `offset` is new minus old there. */
export interface HunkGap { before: number; from: number; to: number; offset: number }

/** The unchanged lines before the first hunk and between hunks. A hunk that only adds or only removes starts after
 *  its `start` line on that side, as Git counts. */
export function hunkGaps(hunks: DiffFile["hunks"]): HunkGap[] {
  const gaps: HunkGap[] = [];
  let next = 1;
  hunks.forEach((hunk, before) => {
    const start = hunk.new_lines > 0 ? hunk.new_start : hunk.new_start + 1;
    const oldStart = hunk.old_lines > 0 ? hunk.old_start : hunk.old_start + 1;
    if (start > next) gaps.push({ before, from: next, to: start - 1, offset: start - oldStart });
    next = start + hunk.new_lines;
  });
  return gaps;
}

/** A click shows this many more unchanged lines, nearest the hunk below; Shift or Alt shows them all. */
const REVEAL_STEP = 20;

/** How many lines of each gap show, and the file's lines once read (`false` when they can't be). */
/** The file's lines from `loadText`, or `false` when it can't be read. */
async function readLines(loadText: DiffViewProps["loadText"]): Promise<string[] | false> {
  const loaded = loadText ? await loadText().catch(() => null) : null;
  return loaded === null ? false : loaded.split("\n");
}

/** A gap can't be shown when the file can't be read, or was cut (256 KiB) before its last line. */
function isUnreadable(lines: string[] | false | null, gap: HunkGap): boolean {
  return lines === false || (Array.isArray(lines) && lines.length < gap.to);
}

/** The gap before a hunk, how many of its lines stay hidden, and the revealed ones (nearest the hunk) as context. */
function gapAround(gaps: HunkGap[], shown: Record<number, number>, lines: string[] | false | null, hunkIndex: number) {
  const gap = gaps.find(entry => entry.before === hunkIndex);
  if (!gap) return { gap: undefined, hidden: 0, revealed: [] as NumberedLine[] };
  const count = shown[hunkIndex] ?? 0;
  const revealed: NumberedLine[] = [];
  for (let no = gap.to - count + 1; no <= gap.to; no += 1) revealed.push({ kind: "context", text: lines ? lines[no - 1] ?? "" : "", oldNo: no - gap.offset, newNo: no });
  return { gap, hidden: gap.to - gap.from + 1 - count, revealed };
}

function useHiddenLines(file: DiffFile, loadText: DiffViewProps["loadText"], remember: DiffViewProps["remember"]) {
  const gaps = useMemo(() => hunkGaps(file.hunks), [file.hunks]);
  const [shown, setShown] = useState<Record<number, number>>(() => remember?.initial?.shown ?? {});
  const [lines, setLines] = useState<string[] | false | null>(() => remember?.initial?.lines ?? null);
  const save = remember?.save;
  useEffect(() => { save?.({ shown, lines }); }, [save, shown, lines]);
  const reveal = async (gap: HunkGap, all: boolean) => {
    let text = lines;
    if (text === null) { text = await readLines(loadText); setLines(text); }
    if (isUnreadable(text, gap)) return;
    const size = gap.to - gap.from + 1;
    setShown(previous => ({ ...previous, [gap.before]: all ? size : Math.min(size, (previous[gap.before] ?? 0) + REVEAL_STEP) }));
  };
  const around = (hunkIndex: number) => gapAround(gaps, shown, lines, hunkIndex);
  const unreadable = (gap: HunkGap) => isUnreadable(lines, gap);
  return { around, reveal, unreadable };
}

/** "⋯ N unchanged lines": a quiet row in place of the lines between hunks. */
function GapRow({ hidden, unreadable, onReveal }: { hidden: number; unreadable: boolean; onReveal: (all: boolean) => void }) {
  return (
    <button type="button" className="themis-diff-gap" disabled={unreadable} title={unreadable ? undefined : "Show 20 more · Shift- or Alt-click for all"}
      onClick={(event: MouseEvent) => onReveal(event.shiftKey || event.altKey)}>
      ⋯ {hidden} unchanged line{hidden === 1 ? "" : "s"}{unreadable ? ", can't be shown" : ""}
    </button>
  );
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
        <span className="themis-diff-sign">{line.kind === "add" ? "+" : line.kind === "del" ? "−" : " "}</span>
        <LineText text={line.text} extension={extension} mark={mark} />
      </>}
    </div>
  );
}

/** A hunk's lines (with any revealed unchanged lines before them), in one column or side by side. */
function HunkRows({ lines, split, extension }: { lines: NumberedLine[]; split: boolean; extension: string | undefined }) {
  if (split) {
    return <>{sideBySide(lines).map(([left, right], rowIndex) => {
      const range = left && right && left.kind === "del" && right.kind === "add" ? changedRange(left.text, right.text) : null;
      return (
        <div key={rowIndex} className="themis-diff-side-row">
          <SideCell line={left} side={0} extension={extension} mark={range ? [range.oldStart, range.oldEnd] : undefined} />
          <SideCell line={right} side={1} extension={extension} mark={range ? [range.newStart, range.newEnd] : undefined} />
        </div>
      );
    })}</>;
  }
  return <>{lines.map((line, lineIndex) => (
    <div key={lineIndex} className={`themis-diff-line themis-diff-line--${line.kind}`}>
      <span className="themis-diff-no">{line.oldNo ?? ""}</span>
      <span className="themis-diff-no">{line.newNo ?? ""}</span>
      <span className="themis-diff-sign">{line.kind === "add" ? "+" : line.kind === "del" ? "−" : " "}</span>
      <LineText text={line.text} extension={extension} />
    </div>
  ))}</>;
}

/** Read-only diff of one file, unified or side by side; the choice is remembered. A file with
 *  only one side, or too little room for two, reads better in one column. */
export function DiffView({ file, onWantWidth, loadText, remember }: DiffViewProps) {
  const extension = file.path.split(".").pop();
  const hiddenLines = useHiddenLines(file, loadText, remember);
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
    // The canvas well: no header bar (the file's row above shows its path and counts); Unified and Split sit over its top right.
    <div className="themis-diff" ref={root} role="region" aria-label={`Changes in ${file.path}`}>
        <span className="themis-diff-toggle" role="group" aria-label="Diff layout">
          {(["unified", "split"] as const).map(option => (
            <button key={option} type="button" aria-pressed={showing === option} disabled={option === "split" && splitOff}
              title={option === "split" && splitOff ? (oneSided ? "This file only has one side" : "Widen the panel for two columns") : undefined}
              className={`themis-diff-toggle-btn${showing === option ? " themis-diff-toggle-btn--active" : ""}`} onClick={() => setLayout(option)}>
              {option === "unified" ? "Unified" : "Split"}
            </button>
          ))}
        </span>

      {file.too_large && file.hunks.length === 0 ? (
        <EmptyState title="Too large to show" />
      ) : file.hunks.length === 0 ? (
        <EmptyState title="No changes" />
      ) : (
        <div className={showing === "split" ? "themis-diff-side" : "themis-diff-unified"}>
          {file.hunks.map((hunk, hunkIndex) => {
            const { gap, hidden, revealed } = hiddenLines.around(hunkIndex);
            const lines = [...revealed, ...numberHunk(hunk.old_start, hunk.new_start, hunk.lines.map((l) => ({ ...l, oldNo: null, newNo: null })))];
            return (
              <div key={hunkIndex} className="themis-diff-hunk">
                {/* Where each hunk starts is named for screen readers; the lines before it fold into one quiet row. */}
                <div className="themis-diff-hunk-gap" role="separator" aria-label={hunkHeader(hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines)} />
                {gap && hidden > 0 && <GapRow hidden={hidden} unreadable={hiddenLines.unreadable(gap)} onReveal={all => void hiddenLines.reveal(gap, all)} />}
                <HunkRows lines={lines} split={showing === "split"} extension={extension} />
              </div>
            );
          })}
          {file.too_large && <p className="themis-diff-note">Showing the first lines only; the rest of this change is too large to show.</p>}
        </div>
      )}
    </div>
  );
}
