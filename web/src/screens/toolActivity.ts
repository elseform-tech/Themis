import type { ChatMessage } from "../state/reducer";
import { toolDisplayName } from "../lib/toolNames";

/** What one tool call worked on, read from its summary: a short form such as
 *  "read `src/a.txt`", or "call `tool` with {…}" (cut at 200 characters). */
function callTarget(summary: string): { path?: string; command?: string; query?: string; url?: string } {
  const short = /^(?:read|write|append to|list|create dir|delete|search `[^`]*` in) `([^`]+)`/.exec(summary);
  if (short) return { path: short[1] };
  const json = summary.indexOf(" with {");
  if (json < 0) return {};
  const text = summary.slice(json + 6);
  let args: Record<string, unknown> = {};
  try { args = JSON.parse(text) as Record<string, unknown>; } catch {
    // A cut summary is not JSON: pick the fields out by pattern.
    for (const key of ["file_path", "path", "directory_path", "command", "query", "url"]) {
      const match = new RegExp(`"${key}":"((?:[^"\\\\]|\\\\.)*)"`).exec(text);
      if (match) args[key] = match[1];
    }
  }
  const str = (key: string) => (typeof args[key] === "string" ? (args[key] as string) : undefined);
  const list = Array.isArray(args.args) ? (args.args as unknown[]).filter(arg => typeof arg === "string") as string[] : [];
  const command = str("command");
  return {
    path: str("file_path") ?? str("path") ?? str("directory_path"),
    command: command ? [command, ...list].join(" ") : undefined,
    query: str("query"),
    url: str("url"),
  };
}

const base = (path: string) => path.split("/").filter(Boolean).pop() ?? path;
const host = (url: string) => { try { return new URL(url).host; } catch { return url; } };
const count = (n: number, one: string, many: string) => (n === 1 ? `a ${one}` : `${n} ${many}`);
const clip = (text: string, max = 40) => (text.length > max ? `${text.slice(0, max - 1)}…` : text);

type Kind = "subagents" | "read" | "edit" | "list" | "search" | "run" | "git" | "web" | "fetch" | "browser" | "recall" | "checklist" | "other";

const KIND: Record<string, Kind> = {
  read_file: "read", write_file: "edit", apply_patch: "edit", copy_file: "edit", move_file: "edit",
  delete_file: "edit", create_dir: "edit", list_dir: "list", search_file: "search", shell: "run",
  spawn_subagents: "subagents", wait_subagents: "subagents", message_subagent: "subagents", stop_subagent: "subagents",
  git: "git", web_search: "web", fetch_url: "fetch", recall: "recall", goal_checklist: "checklist",
};

function kindOf(name: string): Kind {
  return KIND[name] ?? (name.startsWith("browser_") ? "browser" : "other");
}

/** One phrase for the calls of one kind, naming the thing when there is one. */
/** `pending` when none of the calls has finished yet (one may be waiting for an approval):
 *  edits and commands then read as under way, not done. */
function phrase(kind: Kind, calls: { name: string; summary: string; label?: string }[], pending = false): string {
  const targets = calls.map(call => callTarget(call.summary));
  const one = calls.length === 1;
  const unique = (values: (string | undefined)[]) => [...new Set(values.filter((value): value is string => !!value))];
  switch (kind) {
    case "read": {
      const files = unique(targets.map(target => target.path));
      return files.length === 1 ? `Read ${base(files[0]!)}` : `Read ${count(files.length || calls.length, "file", "files")}`;
    }
    case "edit": {
      const files = unique(targets.map(target => target.path));
      const verb = pending ? "Editing" : "Edited";
      return files.length === 1 ? `${verb} ${base(files[0]!)}` : `${verb} ${count(files.length || calls.length, "file", "files")}`;
    }
    case "list": {
      const dirs = unique(targets.map(target => target.path));
      return dirs.length === 1 ? `Listed ${dirs[0] === "." ? "the project" : dirs[0]}` : `Listed ${count(dirs.length || calls.length, "folder", "folders")}`;
    }
    case "search": return one ? `Searched ${targets[0]!.path ? base(targets[0]!.path) : "the files"}` : `Searched ${calls.length} times`;
    case "run": {
      const commands = unique(targets.map(target => target.command));
      const verb = pending ? "Running" : "Ran";
      return commands.length === 1 ? `${verb} ${clip(commands[0]!)}` : `${verb} ${count(calls.length, "command", "commands")}`;
    }
    case "subagents": {
      const words: Record<string, [string, string]> = {
        spawn_subagents: ["Starting subagents", "Started subagents"],
        wait_subagents: ["Waiting for subagents", "Waited for subagents"],
        message_subagent: ["Messaging a subagent", "Messaged a subagent"],
        stop_subagent: ["Stopping a subagent", "Stopped a subagent"],
      };
      return unique(calls.map(call => words[call.name]?.[pending ? 0 : 1] ?? call.name)).join(" · ");
    }
    case "git": return one ? "Used git" : `Used git ${calls.length} times`;
    case "web": {
      const queries = unique(targets.map(target => target.query));
      return queries.length === 1 ? `Searched the web for “${clip(queries[0]!, 32)}”` : one ? "Searched the web" : `Searched the web ${calls.length} times`;
    }
    case "fetch": {
      const hosts = unique(targets.map(target => target.url && host(target.url)));
      return hosts.length === 1 ? `Opened ${hosts[0]}` : `Opened ${count(calls.length, "page", "pages")}`;
    }
    case "browser": return "Used the browser";
    case "recall": return "Looked back at earlier work";
    case "checklist": return "Updated the checklist";
    case "other": {
      const names = unique(calls.map(call => toolDisplayName(call.name, call.label)));
      return names.length === 1 ? `Used ${names[0]}` : `Used ${names.length} tools`;
    }
  }
}

/** A stretch of tool calls in a few words, in the order they first ran: "Read 4 files ·
 *  Ran python3 check.py". At most three phrases; the rest is counted. Failed calls are
 *  left out (the row counts them); when every call failed, only their number is left.
 *  While the run is `live`, edits and commands none of whose calls has finished read as
 *  under way ("Editing", "Running"). */
export function describeTools(messages: ChatMessage[], live = false): string {
  const groups = new Map<Kind, { name: string; summary: string; label?: string }[]>();
  const finished = new Set<Kind>();
  const calls = messages.filter(message => message.tool);
  if (calls.length && calls.every(message => message.tool!.ok === false)) {
    return calls.length === 1 ? "1 tool call" : `${calls.length} tool calls`;
  }
  for (const message of calls) {
    if (!message.tool || message.tool.ok === false) continue;
    const kind = kindOf(message.tool.name);
    const group = groups.get(kind) ?? [];
    group.push({ name: message.tool.name, summary: message.text, ...(message.tool.label ? { label: message.tool.label } : {}) });
    if (message.tool.ok !== undefined) finished.add(kind);
    groups.set(kind, group);
  }
  const phrases = [...groups].map(([kind, calls]) => phrase(kind, calls, live && !finished.has(kind)));
  if (phrases.length <= 3) return phrases.join(" · ");
  return `${phrases.slice(0, 3).join(" · ")} · ${phrases.length - 3} more`;
}
