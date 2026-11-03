/** Changed files whose path has a dotted part (`.github/…`, `.gitignore`) carry `hidden` from
 *  the backend. Lists and counts leave them out and fold them into one quiet line. */
export function visibleFiles<T extends { hidden?: boolean }>(files: T[]): T[] {
  return files.filter(file => !file.hidden);
}

/** "2 hidden files changed": the folded line's text and accessible name. */
export function hiddenFilesLabel(count: number): string {
  return `${count} hidden file${count === 1 ? "" : "s"} changed`;
}
