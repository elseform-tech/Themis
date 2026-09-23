import type { DiffFile, DiffStatus } from "../../lib/types";
import { EmptyState } from "../primitives/EmptyState";
import "./FileTree.css";

export interface FileTreeProps {
  files: DiffFile[];
  selectedPath: string | null;
  onSelect: (path: string) => void;
  onAccept: (path: string) => void;
  onDiscard: (path: string) => void;
  disabled?: boolean;
}

const STATUS_GLYPH: Record<DiffStatus, string> = {
  modified: "M",
  added: "A",
  deleted: "D",
  renamed: "R",
};

export function FileTree({
  files,
  selectedPath,
  onSelect,
  onAccept,
  onDiscard,
  disabled = false,
}: FileTreeProps) {
  if (files.length === 0) {
    return <EmptyState title="No changed files" hint="There is nothing to review yet." />;
  }

  return (
    <ul aria-label="Changed files" className="themis-filetree">
      {files.map((file) => {
        const selected = file.path === selectedPath;
        const discardDisabled = disabled || file.preexisting;
        return (
          <li
            key={file.path}
            className={
              selected
                ? "themis-filetree-row themis-filetree-row--selected"
                : "themis-filetree-row"
            }
          >
            <button
              type="button"
              className="themis-filetree-select"
              aria-current={selected}
              onClick={() => onSelect(file.path)}
            >
              <span
                aria-hidden="true"
                className={`themis-filetree-status themis-filetree-status--${file.status}`}
              >
                {STATUS_GLYPH[file.status]}
              </span>
              <span className="themis-filetree-path" title={file.path}>
                {file.path}
              </span>
              {file.preexisting && (
                <span className="themis-filetree-pre" title="Changed before this thread started">
                  pre-existing
                </span>
              )}
            </button>
            <span className="themis-filetree-actions">
              <button
                type="button"
                aria-label={`Accept ${file.path}`}
                className="themis-filetree-btn themis-filetree-btn--accept"
                disabled={disabled}
                onClick={() => onAccept(file.path)}
              >
                ✓
              </button>
              <button
                type="button"
                aria-label={`Discard ${file.path}`}
                title={
                  file.preexisting
                    ? "Pre-existing changes cannot be discarded"
                    : `Discard ${file.path}`
                }
                className="themis-filetree-btn themis-filetree-btn--discard"
                disabled={discardDisabled}
                onClick={() => onDiscard(file.path)}
              >
                ✕
              </button>
            </span>
          </li>
        );
      })}
    </ul>
  );
}
