import type { ApprovalRequest, SubagentEntry } from "../lib/types";
import { useApp } from "../state/store";
import "./SubagentCard.css";

/** What a subagent's row says about it now. */
export type SubagentState = "running" | "approval" | "finished" | "stopped" | "failed" | "incomplete" | "lost";

/** A subagent's state, given whether its run is still going and the approvals it waits on. */
export function subagentState(agent: SubagentEntry, runLive: boolean, approvals: ApprovalRequest[]): SubagentState {
  if (agent.status !== "running") return agent.status;
  if (!runLive) return "lost";
  return approvals.some(approval => approval.subagent?.id === agent.id) ? "approval" : "running";
}

/** `0:48` or `12:05`. */
export function clock(seconds: number): string {
  const whole = Math.max(0, Math.floor(seconds));
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, "0")}`;
}

/** The first line of a result that says something (not one that only introduces a list),
 *  without Markdown marks. */
export function firstLine(text: string): string {
  const lines = text.split("\n").map(part => part.replace(/^([#>*-]+|\d+\.)\s*/, "").replace(/\*\*|`/g, "").trim()).filter(Boolean);
  return lines.find(line => !line.endsWith(":")) ?? lines[0] ?? "";
}

/** How long it has run: its final time once ended, else the time since it started. */
export function elapsed(agent: SubagentEntry, now: number): number | undefined {
  if (agent.seconds !== undefined) return agent.seconds;
  return agent.startedAt === undefined ? undefined : (now - agent.startedAt) / 1000;
}

const ENDING: Record<Exclude<SubagentState, "running" | "approval" | "finished">, string> = {
  stopped: "Stopped · partial result returned",
  incomplete: "Reached its step limit · partial result returned",
  failed: "Failed",
  lost: "Ended when Themis closed",
};

/** One line on what the subagent is doing, or how it ended. */
export function subagentLine(agent: SubagentEntry, state: SubagentState, approvals: ApprovalRequest[]): string {
  switch (state) {
    case "running": return agent.text || "Starting";
    case "approval": {
      const request = approvals.find(approval => approval.subagent?.id === agent.id);
      return `Approval needed: ${request?.command?.argv.join(" ") ?? request?.summary ?? request?.tool ?? ""}`;
    }
    case "finished": return firstLine(agent.result ?? "");
    case "failed": return agent.result ? `Failed: ${firstLine(agent.result)}` : ENDING.failed;
    default: return ENDING[state];
  }
}

/** Soft tones the circles cycle through, so neighbours differ. */
const TONES = ["info", "success", "accent", "neutral"] as const;

/** The subagents started together in a turn (board 25): overlapping letter circles, ringed
 *  while they work. Status lives in the overview box, the approval box and Workspace › Tasks;
 *  a circle opens that subagent there, the count opens the list. */
export function SubagentCard({ agents, runLive }: { agents: SubagentEntry[]; runLive: boolean }) {
  const { state, dispatch } = useApp();
  const open = (subagent?: string) => dispatch({ type: "ui/diff-panel", open: true, tab: "tasks", ...(subagent ? { subagent } : {}) });
  const label = `${agents.length} ${agents.length === 1 ? "subagent" : "subagents"}`;
  return (
    <div className="themis-subagents" role="group" aria-label={label}>
      <span className="themis-subagent-stack">
        {agents.map((agent, index) => {
          const agentState = subagentState(agent, runLive, state.approvals);
          const working = agentState === "running" || agentState === "approval";
          return (
            <button key={agent.id} type="button" title={agent.name} aria-label={`${agent.name}: opens it in Tasks`}
              className={`themis-subagent-dot is-${TONES[index % TONES.length]}${working ? " is-working" : ""}`}
              style={{ zIndex: agents.length - index }} onClick={() => open(agent.id)}>
              {agent.name.trim().charAt(0).toUpperCase() || "S"}
              {working && <svg className="themis-subagent-ring" viewBox="0 0 34 34" aria-hidden="true"><path d="M17 2a15 15 0 0 1 15 15" /></svg>}
            </button>
          );
        })}
      </span>
      <button type="button" className="themis-subagents-count" onClick={() => open()}>{label}</button>
    </div>
  );
}

export function SubagentsGlyph() {
  return (
    <svg className="themis-subagents-glyph" viewBox="0 0 24 24" aria-hidden="true">
      <circle cx="8" cy="9" r="3" /><circle cx="16" cy="9" r="3" />
      <path d="M3 19c.8-3 2.7-4.5 5-4.5s4.2 1.5 5 4.5M11 19c.8-3 2.7-4.5 5-4.5s4.2 1.5 5 4.5" />
    </svg>
  );
}
