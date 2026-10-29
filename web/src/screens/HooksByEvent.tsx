import type { Plugin } from "../lib/types";

/** A hook row of the Installed list. */
export interface HookRow {
  key: string;
  id: string;
  plugin: Plugin;
  enabled: boolean;
}

/** Hook events in the order a run reaches them (board 24). */
export const HOOK_EVENTS: Array<{ title: string; events: string[] }> = [
  { title: "Prompt submitted", events: ["PromptSubmit"] },
  { title: "Run starts", events: ["RunStart"] },
  { title: "Before a tool runs", events: ["BeforeTool", "BeforeToolCall"] },
  { title: "After a tool runs", events: ["AfterTool", "AfterToolCall", "ToolCallFailed"] },
  { title: "Before context is compacted", events: ["BeforeCompaction"] },
  { title: "Run ends", events: ["RunEnd", "RunFinished"] },
];

/** Claude hook events, so a plugin's skipped hooks can be told from its other notes. */
const CLAUDE_EVENTS = /^(PreToolUse|PostToolUse|PostToolUseFailure|UserPromptSubmit|SessionStart|SessionEnd|Stop|SubagentStart|SubagentStop|PreCompact|Notification|PermissionRequest|PostToolBatch|UserPromptExpansion)\b/;

/** One plain reason for a skipped hook, or null for a note that is not about hooks. */
export function skipReason(note: string): string | null {
  const event = /^Hook event (\S+)/.exec(note);
  if (event) return `unsupported event (${event[1]})`;
  if (!CLAUDE_EVENTS.test(note)) return null;
  const [name, detail = ""] = note.split(/:\s*/, 2);
  const matcher = /^matcher (.+) has no Themis tool$/.exec(detail);
  if (matcher) return `${matcher[1]} has no Themis tool`;
  if (detail === "non-command hook") return "not a command";
  if (detail === "lifecycle matcher requires manual mapping") return `${name} matcher not supported`;
  if (detail === "hook options require manual mapping") return `${name} options not supported`;
  return detail || name;
}

/** "Personal · shell, background_task · Only when it fails · Blocks on failure" */
export function hookMeta(row: HookRow, source: string): string {
  const hook = row.plugin.spec.hooks.find(item => item.name === row.id);
  if (!hook) return source;
  const blocks = hook.failure_policy === "abort" || (!hook.failure_policy && hook.blocking);
  return [
    source,
    hook.matcher ? hook.matcher.split("|").join(", ") : "",
    hook.event === "ToolCallFailed" ? "Only when it fails" : "",
    blocks ? "Blocks on failure" : "",
  ].filter(Boolean).join(" · ");
}

/** Installed › Hooks: every event in run order with its hooks, empty ones dimmed, and one line
 *  per plugin for the hooks it could not bring. Read-only apart from each hook's switch. */
export function HooksByEvent({ rows, plugins, sourceOf, disabled, onToggle, onOpen }: {
  rows: HookRow[];
  plugins: Plugin[];
  sourceOf: (row: HookRow) => string;
  disabled: (row: HookRow) => boolean;
  onToggle: (row: HookRow) => void;
  onOpen: (row: HookRow) => void;
}) {
  const skipped = plugins.flatMap(plugin => {
    const reasons = plugin.spec.unsupported.map(skipReason).filter((reason): reason is string => reason !== null);
    if (!reasons.length) return [];
    const unique = [...new Set(reasons)];
    return [`${reasons.length} hook${reasons.length === 1 ? "" : "s"} skipped (${plugin.spec.name}): ${unique.join("; ")}.`];
  });
  return (
    <>
      <ol className="themis-hook-events" aria-label="Hook events in run order">
        {HOOK_EVENTS.map((step, index) => {
          const hooks = rows.filter(row => step.events.includes(row.plugin.spec.hooks.find(hook => hook.name === row.id)?.event ?? ""));
          return (
            <li key={step.title} className={hooks.length ? "" : "is-empty"}>
              <div className="themis-hook-event">
                <span className="themis-hook-step" aria-hidden="true">{index + 1}</span>
                <span className="themis-hook-title">{step.title}</span>
                <span className="themis-hook-count">{hooks.length ? `${hooks.length} hook${hooks.length === 1 ? "" : "s"}` : "None"}</span>
              </div>
              {hooks.map(row => (
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
    </>
  );
}
