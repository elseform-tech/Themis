import type { DiffFile } from "../../lib/types";
import { EmptyState } from "../primitives/EmptyState";
import { normalizeLanguage, tokenizeLine } from "../CodeBlock";
import "../CodeBlock.css";
import "./DiffView.css";

export interface DiffViewProps {
  file: DiffFile;
}

interface NumberedLine {
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

/** Read-only unified diff of one file. */
export function DiffView({ file }: DiffViewProps) {
  const language = normalizeLanguage(file.path.split(".").pop());
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
      </div>

      {file.too_large && file.hunks.length === 0 ? (
        <EmptyState title="Too large to show" hint={`${file.path} changed too much to show line by line.`} />
      ) : file.hunks.length === 0 ? (
        <EmptyState title="No changes" hint={`${file.path} has no hunks to display.`} />
      ) : (
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
                  <span className="themis-diff-text">{tokenizeLine(line.text, language).map((token, index) => token.kind === "plain" ? token.text : <span key={index} className={`themis-codeblock-token--${token.kind}`}>{token.text}</span>)}</span>
                </div>
              ))}
            </div>
          ))}
          {file.too_large && <p className="themis-diff-note">Showing the first lines only; the rest of this change is too large to show.</p>}
        </div>
      )}
    </div>
  );
}
