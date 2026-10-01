export interface PromptSkill { id: string; name: string; description: string; plugin: string; scope?: string; retired?: boolean }
export type PromptPart = { text: string } | { skill: string };
export function promptParts(value: string): PromptPart[] {
  const parts: PromptPart[] = []; let end = 0;
  for (const match of value.matchAll(/\[\[skill:([^\]\s]+)\]\]/g)) {
    if (match.index! > end) parts.push({ text: value.slice(end, match.index) });
    parts.push({ skill: match[1] }); end = match.index! + match[0].length;
  }
  if (end < value.length) parts.push({ text: value.slice(end) });
  return parts;
}
export function skillToken(id: string) { return `[[skill:${id}]]`; }
