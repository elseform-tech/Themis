import { useEffect, useState } from "react";
import type { ApprovalRequest, SubagentEntry } from "../lib/types";
import { useApp } from "../state/store";
import { TaskIcon } from "./TaskIcon";
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

function StateIcon({ state }: { state: SubagentState }) {
  if (state === "stopped" || state === "lost") {
    return <span className="themis-subagent-icon is-ended" aria-hidden="true"><svg viewBox="0 0 16 16"><rect x="5" y="5" width="6" height="6" rx="1" /></svg></span>;
  }
  const kind = state === "running" ? "running" : state === "finished" ? "finished" : state === "incomplete" ? "scheduled" : "attention";
  return <span className={`themis-subagent-icon themis-task-row-icon themis-task-row-icon--${kind} is-${kind}`} aria-hidden="true"><TaskIcon kind={kind} /></span>;
}

function useNow(ticking: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!ticking) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [ticking]);
  return now;
}

/** The subagents started together in a turn: one row each, opening its detail in Workspace › Tasks. */
export function SubagentCard({ agents, runLive }: { agents: SubagentEntry[]; runLive: boolean }) {
  const { state, dispatch } = useApp();
  const states = agents.map(agent => subagentState(agent, runLive, state.approvals));
  const active = states.some(s => s === "running" || s === "approval");
  const now = useNow(active);
  const times = agents.map(agent => elapsed(agent, now)).filter((time): time is number => time !== undefined);
  const total = times.length ? clock(Math.max(...times)) : "";
  const calls = agents.reduce((sum, agent) => sum + agent.toolCalls, 0);
  const count = (wanted: SubagentState) => states.filter(s => s === wanted).length;
  const summary = [
    count("finished") && `${count("finished")} done`,
    count("running") && `${count("running")} running`,
    count("approval") && `${count("approval")} needs approval`,
    count("stopped") + count("lost") && `${count("stopped") + count("lost")} stopped`,
    count("failed") && `${count("failed")} failed`,
  ].filter(Boolean);
  const allFinished = states.every(s => s === "finished");
  const noun = agents.length === 1 ? "subagent" : "subagents";
  const title = active ? `${agents.length} ${noun}` : allFinished ? `${agents.length} ${noun} finished` : `${agents.length} ${noun} ended`;
  const meta = active ? [...summary, total].filter(Boolean).join(" · ") : [total, `${calls} tool ${calls === 1 ? "call" : "calls"}`].filter(Boolean).join(" · ");
  return (
    <section className={`themis-subagents${active ? "" : " is-ended"}`} aria-label={title}>
      <header className="themis-subagents-head">
        {active ? <SubagentsGlyph /> : <StateIcon state={allFinished ? "finished" : "stopped"} />}
        <strong>{title}</strong>
        <span className="themis-subagents-meta">{meta}</span>
      </header>
      {agents.map((agent, index) => {
        const agentState = states[index];
        const time = elapsed(agent, now);
        const open = () => dispatch({ type: "ui/diff-panel", open: true, tab: "tasks", subagent: agent.id });
        if (!active) {
          return (
            <button key={agent.id} type="button" className={`themis-subagent-row is-folded is-${agentState}`} onClick={open}>
              <strong className="themis-subagent-name">{agent.name}</strong>
              <span className="themis-subagent-now">{subagentLine(agent, agentState, state.approvals)}</span>
            </button>
          );
        }
        return (
          <button key={agent.id} type="button" className={`themis-subagent-row is-${agentState}`} onClick={open}>
            <StateIcon state={agentState} />
            <span className="themis-subagent-body">
              <span className="themis-subagent-title"><strong className="themis-subagent-name">{agent.name}</strong><span className="themis-subagent-brief">{agent.brief}</span></span>
              <span className="themis-subagent-now">{subagentLine(agent, agentState, state.approvals)}</span>
            </span>
            <span className="themis-subagent-stats">
              <span>{[`${agent.toolCalls} tool ${agent.toolCalls === 1 ? "call" : "calls"}`, time === undefined ? "" : clock(time)].filter(Boolean).join(" · ")}</span>
              <span>Read only</span>
            </span>
          </button>
        );
      })}
    </section>
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
