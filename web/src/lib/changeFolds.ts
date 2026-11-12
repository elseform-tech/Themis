/** Which changed files in Workspace › Changes are open. A choice the person made wins; otherwise a small change
 *  (five files or fewer) opens whole, so it reads as one document, and a larger one opens only its first file, so the
 *  list stays scannable. Hidden files (`.github/…`) start folded, as their group does. */
export type Folds = Record<string, boolean>;

interface FoldFile { path: string; hidden?: boolean }

const OPEN_ALL_UP_TO = 5;

export function isOpen(folds: Folds, files: FoldFile[], path: string): boolean {
  const chosen = folds[path];
  if (chosen !== undefined) return chosen;
  const shown = files.filter(file => !file.hidden);
  if (!shown.some(file => file.path === path)) return false;
  return shown.length <= OPEN_ALL_UP_TO || shown[0]?.path === path;
}

/** Collapse all or Expand all. */
export function setAll(files: FoldFile[], open: boolean): Folds {
  return Object.fromEntries(files.map(file => [file.path, open]));
}

/** Per chat, and per view (the uncommitted changes or another branch's). */
export function foldsKey(threadId: string, branch: string | null): string {
  return `themis:changeFolds:${threadId}:${branch ?? ""}`;
}

/** Session storage only (a per-window convenience); storage that is blocked or full is ignored. */
export function readFolds(key: string): Folds {
  try {
    const value: unknown = JSON.parse(sessionStorage.getItem(key) ?? "{}");
    return value && typeof value === "object" && !Array.isArray(value) ? value as Folds : {};
  } catch { return {}; }
}

export function writeFolds(key: string, folds: Folds): void {
  try { sessionStorage.setItem(key, JSON.stringify(folds)); } catch { /* Storage may be unavailable. */ }
}
