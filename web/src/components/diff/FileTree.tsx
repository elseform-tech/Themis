import { useState } from "react";
import { hiddenFilesLabel } from "../../lib/hiddenFiles";
import type { DiffFile, DiffStatus } from "../../lib/types";
import { EmptyState } from "../primitives/EmptyState";
import "./FileTree.css";

export interface FileTreeProps {
  files: DiffFile[];
  selectedPath: string | null;
  onSelect: (path: string) => void;
  /** Omit both for a read-only tree without accept/discard actions. */
  onAccept?: (path: string) => void;
  onDiscard?: (path: string) => void;
  disabled?: boolean;
}

const STATUS_GLYPH: Record<DiffStatus, string> = {
  modified: "M",
  added: "A",
  deleted: "D",
  renamed: "R",
};

function directoryOf(path: string): string {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? "" : path.slice(0, slash);
}

export function FileTree({
  files,
  selectedPath,
  onSelect,
  onAccept,
  onDiscard,
  disabled = false,
}: FileTreeProps) {
  const [query, setQuery] = useState("");
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const hidden = files.filter(file => file.hidden);
  const selectedHidden = hidden.some(file => file.path === selectedPath);
  // Hidden files fold into one line, opened by hand or when one of them is selected.
  const [hiddenOpen, setHiddenOpen] = useState(selectedHidden);
  const [seenSelection, setSeenSelection] = useState(selectedPath);
  if (seenSelection !== selectedPath) {
    setSeenSelection(selectedPath);
    if (selectedHidden) setHiddenOpen(true);
  }
  if (files.length === 0) {
    return <EmptyState title="No changed files" hint="There is nothing to review yet." />;
  }
  const shown = files.filter(file => !file.hidden);
  const needle = query.trim().toLowerCase();
  const matches = (file: DiffFile) => needle === "" || file.path.toLowerCase().includes(needle);
  const visible = shown.filter(matches);
  const hiddenVisible = hidden.filter(matches);
  const groups = new Map<string, DiffFile[]>();
  for (const file of visible) {
    const directory = directoryOf(file.path);
    groups.set(directory, [...(groups.get(directory) ?? []), file]);
  }

  const row = (file: DiffFile, label: string) => {
    const selected = file.path === selectedPath;
    const discardDisabled = disabled || file.preexisting;
    return (
      <li
        key={file.path}
        className={`themis-filetree-row${selected ? " themis-filetree-row--selected" : ""}`}
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
            {label}
          </span>
          {(file.added > 0 || file.removed > 0) && (
            <span className="themis-filetree-counts" aria-label={`${file.added} added, ${file.removed} removed`}>
              {file.added > 0 && <span className="themis-diff-count-add">+{file.added}</span>}
              {file.removed > 0 && <span className="themis-diff-count-del">-{file.removed}</span>}
            </span>
          )}
        </button>
        {onAccept && (
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
                ? "Changed before this chat started, so it can't be discarded here"
                : `Discard ${file.path}`
            }
            className="themis-filetree-btn themis-filetree-btn--discard"
            disabled={discardDisabled}
            onClick={() => onDiscard?.(file.path)}
          >
            ✕
          </button>
        </span>
        )}
      </li>
    );
  };

  return (
    <div className="themis-filetree-wrap">
      {shown.length > 12 && (
        <div className="themis-filetree-tools">
          <input
            className="themis-filetree-filter"
            type="search"
            aria-label="Filter changed files"
            placeholder={`Filter ${shown.length} files`}
            value={query}
            onChange={event => setQuery(event.target.value)}
          />
        </div>
      )}
      <ul aria-label="Changed files" className="themis-filetree">
        {[...groups].map(([directory, entries]) => (
          <li key={directory || "."} className="themis-filetree-group">
            {groups.size > 1 && (
              <button type="button" className="themis-filetree-dir" aria-expanded={!collapsed[directory]}
                onClick={() => setCollapsed(previous => ({ ...previous, [directory]: !previous[directory] }))}>
                <span aria-hidden="true">{collapsed[directory] ? "›" : "⌄"}</span>
                <span className="themis-filetree-dirname">{directory || "(root)"}</span>
                <span className="themis-filetree-count">{entries.length}</span>
              </button>
            )}
            {!collapsed[directory] && (
              <ul className="themis-filetree-files">
                {entries.map(file => row(file, groups.size > 1 ? file.path.slice(directory.length + (directory ? 1 : 0)) : file.path))}
              </ul>
            )}
          </li>
        ))}
        {hiddenVisible.length > 0 && (
          <li className="themis-filetree-group themis-filetree-hidden">
            <button type="button" className="themis-filetree-dir" aria-expanded={hiddenOpen}
              aria-label={hiddenFilesLabel(hidden.length)} onClick={() => setHiddenOpen(open => !open)}>
              <span aria-hidden="true">{hiddenOpen ? "⌄" : "›"}</span>
              <span className="themis-filetree-hidden-label">{hiddenFilesLabel(hidden.length)}</span>
            </button>
            {hiddenOpen && <ul className="themis-filetree-files">{hiddenVisible.map(file => row(file, file.path))}</ul>}
          </li>
        )}
        {needle !== "" && visible.length === 0 && hiddenVisible.length === 0 && <li className="themis-filetree-none">No files match.</li>}
      </ul>
    </div>
  );
}
