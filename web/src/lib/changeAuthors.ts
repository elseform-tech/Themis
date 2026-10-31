import type { SubagentEntry } from "./types";

export const MAIN_AUTHOR = "Main";
export const UNSURE_AUTHOR = "Main or subagent";

type Author = Pick<SubagentEntry, "name" | "writePaths" | "confined">;

function owns(writePath: string, file: string): boolean {
  return writePath.endsWith("/") ? file.startsWith(writePath) : file === writePath;
}

/** Who changed a file: the subagent whose write paths hold it, else the main agent. A subagent whose shell wasn't confined could have written anywhere, so the rest stays unsure. */
export function authorOf(file: string, agents: Author[]): string {
  const owner = agents.find(agent => agent.writePaths.some(path => owns(path, file)));
  if (owner) return owner.name;
  return agents.some(agent => !agent.confined) ? UNSURE_AUTHOR : MAIN_AUTHOR;
}

/** The author filter's choices: none until a subagent in the chat has write paths. */
export function authorOptions(agents: Author[], files: string[]): string[] {
  const writers = agents.filter(agent => agent.writePaths.length > 0);
  if (writers.length === 0) return [];
  const others = [...new Set(files.map(file => authorOf(file, agents)))].filter(name => !writers.some(agent => agent.name === name));
  return [...new Set([...others, ...writers.map(agent => agent.name)])];
}
