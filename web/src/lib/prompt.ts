export interface PromptSkill { id: string; name: string; description: string; plugin: string; scope?: string; retired?: boolean }
export interface PromptPlugin { id: string; name: string; description: string; origin: string; icon?: string; enabled: boolean }
export type PromptPart = { text: string } | { skill: string } | { plugin: string };
export function promptParts(value: string): PromptPart[] {
  const parts: PromptPart[] = []; let end = 0;
  for (const match of value.matchAll(/\[\[(skill|plugin):([^\]\s]+)\]\]/g)) {
    if (match.index! > end) parts.push({ text: value.slice(end, match.index) });
    parts.push(match[1] === "plugin" ? { plugin: match[2] } : { skill: match[2] }); end = match.index! + match[0].length;
  }
  if (end < value.length) parts.push({ text: value.slice(end) });
  return parts;
}
export function skillToken(id: string) { return `[[skill:${id}]]`; }
export function pluginToken(id: string) { return `[[plugin:${id}]]`; }
