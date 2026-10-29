import { useEffect, useState } from "react";
import { Badge } from "../components";
import { ResponseBody } from "../components/ResponseBody";
import { getSubagentTranscript, stopSubagent } from "../lib/tauri";
import type { SubagentEntry, ThreadEvent } from "../lib/types";
import { describeError, useApp } from "../state/store";
import { clock, elapsed, subagentLine, subagentState } from "./SubagentCard";
import { TaskIcon } from "./TaskIcon";
import "./SubagentCard.css";

/** A subagent's steps: its own words, and its tool calls folded between them. */
export type Step = { kind: "text"; text: string } | { kind: "tools"; calls: string[] };

export function stepsOf(events: ThreadEvent[]): Step[] {
  const steps: Step[] = [];
  for (const event of events) {
    const last = steps[steps.length - 1];
    if (event.kind === "assistant_text" && event.text.trim()) {
      if (last?.kind === "text") last.text += event.text;
      else steps.push({ kind: "text", text: event.text });
    } else if (event.kind === "tool_started") {
      if (last?.kind === "tools") last.calls.push(event.summary);
      else steps.push({ kind: "tools", calls: [event.summary] });
    } else if (event.kind === "finished" && !steps.some(step => step.kind === "text") && event.result.trim()) {
      steps.push({ kind: "text", text: event.result });
    }
  }
  return steps;
}

/** Workspace › Tasks detail of one subagent (board 26): read-only, with Stop while it runs. */
export function SubagentDetail({ threadId, agent, runLive, onBack }: { threadId: string; agent: SubagentEntry; runLive: boolean; onBack: () => void }) {
  const { state } = useApp();
  const [events, setEvents] = useState<ThreadEvent[]>([]);
  const [error, setError] = useState("");
  const [stopping, setStopping] = useState(false);
  const agentState = subagentState(agent, runLive, state.approvals);
  const live = agentState === "running" || agentState === "approval";
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    let cancelled = false;
    const load = () => getSubagentTranscript(threadId, agent.id)
      .then(found => { if (!cancelled) setEvents(found); })
      .catch(reason => { if (!cancelled) setError(describeError(reason)); });
    void load();
    if (!live) return () => { cancelled = true; };
    const timer = window.setInterval(() => { setNow(Date.now()); void load(); }, 1500);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [threadId, agent.id, live]);
  async function stop() {
    setStopping(true); setError("");
    try { await stopSubagent(threadId, agent.id); } catch (reason) { setError(describeError(reason)); }
    finally { setStopping(false); }
  }
  const time = elapsed(agent, now);
  const meta = [
    agent.model,
    "Read only",
    agent.confined ? "" : "Shell not confined",
    `${agent.toolCalls} tool ${agent.toolCalls === 1 ? "call" : "calls"}`,
    time === undefined ? "" : clock(time),
  ].filter(Boolean).join(" · ");
  const steps = stepsOf(events);
  const icon = agentState === "running" ? "running" : agentState === "finished" ? "finished" : "attention";
  return (
    <div className="themis-subagent-detail">
      <header className="themis-subagent-detail-head">
        <button type="button" className="themis-subagent-back" aria-label="Back to Tasks" onClick={onBack}>
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M15 6l-6 6 6 6" /></svg>
        </button>
        <span className="themis-subagent-detail-title">
          <span className="themis-subagent-detail-name">
            <span className={`themis-task-row-icon themis-task-row-icon--${icon}`} aria-hidden="true"><TaskIcon kind={icon} /></span>
            <strong>{agent.name}</strong>
            <Badge>Subagent</Badge>
          </span>
          <span className="themis-subagent-detail-meta">{meta}</span>
        </span>
        {live && <button type="button" className="themis-subagent-stop" disabled={stopping} onClick={() => void stop()}>Stop</button>}
      </header>
      {error && <p className="themis-workspace-note" role="alert">{error}</p>}
      <div className="themis-subagent-steps">
        <details className="themis-subagent-brief-box" open>
          <summary>Brief</summary>
          <p>{agent.brief}</p>
        </details>
        {steps.map((step, index) => step.kind === "text"
          ? <div key={index} className="themis-subagent-step-text"><ResponseBody text={step.text} /></div>
          : (
            <details key={index} className="themis-subagent-calls">
              <summary>{step.calls.length} tool {step.calls.length === 1 ? "call" : "calls"}</summary>
              <ul>{step.calls.map((call, at) => <li key={at}>{call}</li>)}</ul>
            </details>
          ))}
        {!live && agentState !== "finished" && <p className="themis-subagent-ending">{subagentLine(agent, agentState, state.approvals)}</p>}
      </div>
    </div>
  );
}
