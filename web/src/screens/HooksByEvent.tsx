import { useState } from "react";
import type { Plugin } from "../lib/types";

/** A hook row of the Installed list. */
export interface HookRow {
  key: string;
  id: string;
  plugin: Plugin;
  enabled: boolean;
}

/** Hook events in the order a run reaches them (board 24). */
export const HOOK_EVENTS: Array<{ title: string; events: string[]; onlyWhenUsed?: boolean }> = [
  { title: "Session starts", events: ["SessionStart"], onlyWhenUsed: true },
  { title: "Instructions loaded", events: ["InstructionsLoaded"], onlyWhenUsed: true },
  { title: "Configuration changed", events: ["ConfigChange"], onlyWhenUsed: true },
  { title: "Prompt submitted", events: ["PromptSubmit"] },
  { title: "Skill used in a prompt", events: ["UserPromptExpansion"], onlyWhenUsed: true },
  { title: "Run starts", events: ["RunStart"] },
  { title: "Approval about to be asked", events: ["PermissionRequest"], onlyWhenUsed: true },
  { title: "Approval refused", events: ["PermissionDenied"], onlyWhenUsed: true },
  { title: "Before a tool runs", events: ["BeforeTool", "BeforeToolCall"] },
  { title: "After a tool runs", events: ["AfterTool", "AfterToolCall", "ToolCallFailed"] },
  { title: "After a step's tools", events: ["PostToolBatch"], onlyWhenUsed: true },
  { title: "Subagent starts", events: ["SubagentStart"] },
  { title: "Subagent plan item added", events: ["TaskCreated"], onlyWhenUsed: true },
  { title: "Subagent plan item done", events: ["TaskCompleted"], onlyWhenUsed: true },
  { title: "Subagent ends", events: ["SubagentEnd"] },
  { title: "Before context is compacted", events: ["BeforeCompaction"] },
  { title: "After context is compacted", events: ["PostCompact"], onlyWhenUsed: true },
  { title: "Notified", events: ["Notification"], onlyWhenUsed: true },
  { title: "Turn would end", events: ["Stop"] },
  { title: "Run ends", events: ["RunEnd", "RunFinished"] },
  { title: "Session ends", events: ["SessionEnd"], onlyWhenUsed: true },
];

/** Claude hook events, so a plugin's skipped hooks can be told from its other notes. */
const CLAUDE_EVENTS = /^(PreToolUse|PostToolUse|PostToolUseFailure|UserPromptSubmit|SessionStart|SessionEnd|Stop|SubagentStart|SubagentStop|PreCompact|PostCompact|Notification|PermissionRequest|PermissionDenied|PostToolBatch|UserPromptExpansion|TaskCreated|TaskCompleted|ConfigChange|InstructionsLoaded)\b/;

/** One plain reason for a skipped hook, or null for a note that is not about hooks. */
export function skipReason(note: string): string | null {
  const event = /^Hook event (\S+)/.exec(note);
  if (event) return `unsupported event (${event[1]})`;
  if (!CLAUDE_EVENTS.test(note)) return null;
  const [name, detail = ""] = note.split(/:\s*/, 2);
  if (/^matcher .+ has no Themis tool$/.test(detail)) return detail;
  if (detail === "lifecycle matcher requires manual mapping") return `${name} matcher not supported`;
  return detail || name;
}

/** A matcher's tools, then the pattern it may end in for MCP tools. */
function matcherText(matcher: string): string {
  const at = matcher.startsWith("re:") ? -1 : matcher.indexOf("|re:");
  if (at < 0 && !matcher.startsWith("re:")) return matcher.split("|").join(", ");
  return at < 0 ? matcher.slice(3) : [...matcher.slice(0, at).split("|"), matcher.slice(at + 4)].join(", ");
}

/** "Personal · shell, background_task · Only when it fails · Blocks on failure" */
export function hookMeta(row: HookRow, source: string): string {
  const hook = row.plugin.spec.hooks.find(item => item.name === row.id);
  if (!hook) return source;
  const blocks = hook.failure_policy === "abort" || (!hook.failure_policy && hook.blocking);
  return [
    source,
    hook.matcher ? matcherText(hook.matcher) : "",
    ["BeforeTool", "BeforeToolCall", "AfterTool", "AfterToolCall", "ToolCallFailed"].includes(hook.event) ? "Main and subagents" : "",
    hook.event === "ToolCallFailed" ? "Only when it fails" : "",
    blocks ? "Blocks on failure" : "",
  ].filter(Boolean).join(" · ");
}

/** Installed › Hooks: the events in run order with their hooks, empty ones dimmed (the rarer ones
 *  appear only with a hook), and one line per plugin for the hooks it could not bring. Read-only
 *  apart from each hook's switch. */
export function HooksByEvent({ rows, plugins, sourceOf, disabled, onToggle, onOpen }: {
  rows: HookRow[];
  plugins: Plugin[];
  sourceOf: (row: HookRow) => string;
  disabled: (row: HookRow) => boolean;
  onToggle: (row: HookRow) => void;
  onOpen: (row: HookRow) => void;
}) {
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set());
  const toggleEvent = (title: string) => setCollapsed(current => {
    const next = new Set(current);
    if (!next.delete(title)) next.add(title);
    return next;
  });
  const skipped = plugins.flatMap(plugin => {
    const reasons = plugin.spec.unsupported.map(skipReason).filter((reason): reason is string => reason !== null);
    if (!reasons.length) return [];
    const unique = [...new Set(reasons)];
    return [`${reasons.length} hook${reasons.length === 1 ? "" : "s"} skipped (${plugin.spec.name}): ${unique.join("; ")}.`];
  });
  const hooksOf = (events: string[]) => rows.filter(row => events.includes(row.plugin.spec.hooks.find(hook => hook.name === row.id)?.event ?? ""));
  return (
    <div className="themis-integrations-list">
      <ol className="themis-hook-events" aria-label="Hook events in run order">
        {HOOK_EVENTS.filter(step => !step.onlyWhenUsed || hooksOf(step.events).length > 0).map((step, index) => {
          const hooks = hooksOf(step.events);
          const open = !collapsed.has(step.title);
          const heading = <>
            <span className="themis-hook-step" aria-hidden="true">{index + 1}</span>
            <span className="themis-hook-title">{step.title}</span>
            <span className="themis-hook-count">{hooks.length ? `${hooks.length} hook${hooks.length === 1 ? "" : "s"}` : "None"}</span>
          </>;
          return (
            <li key={step.title} className={hooks.length ? "" : "is-empty"}>
              {hooks.length
                ? <button type="button" className="themis-hook-event" aria-expanded={open} onClick={() => toggleEvent(step.title)}>
                    {heading}
                    <svg className="themis-hook-chevron" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" aria-hidden="true"><path d="m6 9 6 6 6-6" /></svg>
                  </button>
                : <div className="themis-hook-event">{heading}</div>}
              {open && hooks.map(row => (
                <div key={row.key} className="themis-hook-row">
                  <span className="themis-hook-text">
                    <button type="button" className="themis-hook-name" aria-label={`View ${row.id}`} onClick={() => onOpen(row)}>{row.id}</button>
                    <span className="themis-hook-meta">{hookMeta(row, sourceOf(row))}</span>
                  </span>
                  <button className="themis-integration-toggle" role="switch" aria-label={`${row.id} enabled`} aria-checked={row.enabled} disabled={disabled(row)} onClick={() => onToggle(row)}><span className="themis-toggle-track"><span /></span></button>
                </div>
              ))}
            </li>
          );
        })}
      </ol>
      {skipped.map(line => <p key={line} className="themis-hook-skipped" role="note">{line}</p>)}
    </div>
  );
}
