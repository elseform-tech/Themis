/** Changed files whose path has a dotted part (`.github/…`, `.gitignore`) carry `hidden` from
 *  the backend, except project skills (`.agents/skills/…`), which are shared with the team.
 *  Lists and counts leave hidden files out and fold them into one quiet line. */
export function visibleFiles<T extends { hidden?: boolean }>(files: T[]): T[] {
  return files.filter(file => !file.hidden);
}

/** "2 hidden files changed": the folded line's text and accessible name. */
export function hiddenFilesLabel(count: number): string {
  return `${count} hidden file${count === 1 ? "" : "s"} changed`;
}
