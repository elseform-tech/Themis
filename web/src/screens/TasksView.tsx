import { useEffect, useState } from "react";
import { EmptyState } from "../components";
import { setAutomationEnabled, stopBackgroundTask, stopSubagent, stopThread } from "../lib/tauri";
import type { SubagentEntry, ThreadTaskBackground } from "../lib/types";
import { readSession, writeSession } from "../state/session";
import { describeError, useApp } from "../state/store";
import { BackgroundTaskRow } from "./BackgroundTaskRow";
import { groupBackground } from "./backgroundTasks";
import { TaskIcon } from "./TaskIcon";
import { groupTasks, type TaskKind, type TaskRow } from "./threadTasks";
import { clock, modelNote, SubagentLetter, subagentLine, type SubagentState } from "./SubagentCard";
import { plainSnippet } from "../lib/plainText";
import { SubagentDetail } from "./SubagentDetail";
import { doneCount, doneGroups, subagentTasks, type SubagentTask } from "./taskSections";

const DONE_OPEN_KEY = "tasksDoneOpen";

const subagentIcon = (state: SubagentState): TaskKind =>
  state === "running" ? "running" : state === "finished" ? "finished" : "attention";
const iconKind = (row: TaskRow): TaskKind => row.failed ? "attention" : row.kind === "earlier" ? "finished" : row.kind;

/** This chat's work (board 29): what needs you, what runs (subagents under their run), what is
 *  scheduled, and finished work folded by request. Stop and Pause are the only actions;
 *  neither undoes work. */
export function TasksView({ threadId }: { threadId: string }) {
  const { state, dispatch } = useApp();
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [doneOpen, setDoneOpenState] = useState(() => readSession<boolean>(DONE_OPEN_KEY, false));
  const [openGroups, setOpenGroups] = useState<Record<string, boolean>>({});
  const messages = state.messages[threadId] ?? [];
  const groups = groupTasks(state.tasks[threadId], state.settings.automations_enabled);
  const background = groupBackground(state.tasks[threadId]);
  const chatModel = Object.values(state.threadsByProject).flat().find(thread => thread.id === threadId)?.model;
  const subagents = subagentTasks(messages, state.activeRunIds[threadId], state.approvals);
  const focused = subagents.find(task => task.agent.id === state.subagentFocus);
  const waiting = subagents.filter(task => task.state === "approval");
  const working = subagents.filter(task => task.state === "running");
  const done = doneGroups(messages, subagents, groups.earlier, background.earlier);
  const doneTotal = doneCount(done);
  const setDoneOpen = (open: boolean) => { setDoneOpenState(open); writeSession(DONE_OPEN_KEY, open); };
  // The reply's row (board 29): titled by the request, with what it waits for and how long.
  const startedAt = state.runStartedAt[threadId];
  const replyRunning = groups.running.some(row => row.key === "run");
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!replyRunning) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [replyRunning]);
  const request = [...messages].reverse().find(row => row.role === "user")?.text ?? "";
  const reply = (row: TaskRow): TaskRow => row.key !== "run" ? row : {
    ...row,
    name: plainSnippet(request, 60) || row.name,
    tag: "Reply",
    meta: [working.length ? `Waiting for ${working.length} ${working.length === 1 ? "subagent" : "subagents"}` : "Running now", startedAt === undefined ? "" : clock((now - startedAt) / 1000)].filter(Boolean).join(" · "),
  };

  async function attempt(key: string, action: () => Promise<unknown>) {
    setBusy(key); setError("");
    try { await action(); } catch (reason) { setError(describeError(reason)); }
    finally { setBusy(null); }
  }
  const act = (row: TaskRow, what: "stop" | "pause") => attempt(`${row.key}:${what}`, async () => {
    if (what === "stop") await stopThread(threadId);
    else if (row.pause) dispatch({ type: "automation/updated", automation: await setAutomationEnabled(row.pause, false) });
  });
  const stopOne = (agent: SubagentEntry) => attempt(`subagent:${agent.id}`, () => stopSubagent(threadId, agent.id));
  const stopBackground = (task: ThreadTaskBackground) => attempt(`background:${task.id}`, () => stopBackgroundTask(threadId, task.id));

  if (focused) {
    return <SubagentDetail threadId={threadId} agent={focused.agent} runLive={focused.runLive} onBack={() => dispatch({ type: "ui/subagent-focus", id: null })} />;
  }

  const taskRow = (row: TaskRow) => {
    const icon = iconKind(row);
    return (
      <li key={row.key} className="themis-task-row">
        <span className={`themis-task-row-icon themis-task-row-icon--${icon}`} aria-hidden="true"><TaskIcon kind={icon} /></span>
        <span className="themis-task-row-text">
          <span className="themis-workspace-list-title">{row.name}<em>{row.tag}</em></span>
          <span className="themis-workspace-list-meta">{row.meta}</span>
        </span>
        <span className="themis-task-row-actions">
          {row.stop && <button type="button" aria-label={`Stop ${row.name}`} disabled={busy !== null} onClick={() => void act(row, "stop")}>Stop</button>}
          {row.pause && <button type="button" aria-label={`Pause ${row.name}`} disabled={busy !== null} onClick={() => void act(row, "pause")}>Pause</button>}
        </span>
      </li>
    );
  };
  const subagentRow = ({ agent, state: agentState, index }: SubagentTask, nested = false) => {
    const icon = subagentIcon(agentState);
    const live = agentState === "running" || agentState === "approval";
    return (
      <li key={`subagent-${agent.id}`} className="themis-task-row">
        {nested
          ? <SubagentLetter name={agent.name} index={index} working={agentState === "running"} />
          : <span className={`themis-task-row-icon themis-task-row-icon--${icon}`} aria-hidden="true"><TaskIcon kind={icon} /></span>}
        <button type="button" className="themis-task-row-text themis-task-row-open" onClick={() => dispatch({ type: "ui/subagent-focus", id: agent.id })}>
          <span className="themis-workspace-list-title">{agent.name}<em>Subagent</em></span>
          <span className="themis-workspace-list-meta">{[modelNote(agent, chatModel), subagentLine(agent, agentState, state.approvals)].filter(Boolean).join(" · ")}</span>
        </button>
        <span className="themis-task-row-actions">
          {live && <button type="button" aria-label={`Stop ${agent.name}`} disabled={busy !== null} onClick={() => void stopOne(agent)}>Stop</button>}
        </span>
      </li>
    );
  };
  const backgroundRow = (task: ThreadTaskBackground) => <BackgroundTaskRow key={`background-${task.id}`} threadId={threadId} task={task} busy={busy !== null} onStop={task => void stopBackground(task)} />;

  const needsYou = groups.attention.length + waiting.length + background.attention.length;
  const running = groups.running.length + working.length + background.running.length;
  const scheduled = groups.scheduled.length;
  const current = needsYou + running + scheduled;
  return (
    <div className="themis-tasks">
      {error && <p className="themis-workspace-note" role="alert">{error}</p>}
      {current + doneTotal === 0 && <EmptyState title="No tasks in this chat" hint="Runs, subagents, automations and background programs of this chat appear here." />}
      {needsYou > 0 && (
        <section aria-label="Needs you">
          <h3 className="themis-tasks-heading is-attention">Needs you</h3>
          <ul className="themis-workspace-list">
            {groups.attention.map(taskRow)}
            {waiting.map(task => subagentRow(task))}
            {background.attention.map(backgroundRow)}
          </ul>
        </section>
      )}
      {running > 0 && (
        <section aria-label="Running">
          <h3 className="themis-tasks-heading">Running</h3>
          <ul className="themis-workspace-list">
            {groups.running.map(row => taskRow(reply(row)))}
            {working.length > 0 && (
              <li className="themis-task-children" aria-label="Subagents of this reply">
                <ul className="themis-workspace-list">{working.map(task => subagentRow(task, true))}</ul>
              </li>
            )}
            {background.running.map(backgroundRow)}
          </ul>
        </section>
      )}
      {scheduled > 0 && (
        <section aria-label="Scheduled">
          <h3 className="themis-tasks-heading">Scheduled</h3>
          <ul className="themis-workspace-list">{groups.scheduled.map(taskRow)}</ul>
        </section>
      )}
      {current === 0 && doneTotal > 0 && <p className="themis-workspace-note">Nothing running</p>}
      {doneTotal > 0 && (
        <section aria-label="Done" className="themis-tasks-done">
          <button type="button" className="themis-tasks-fold" aria-expanded={doneOpen} onClick={() => setDoneOpen(!doneOpen)}>
            <svg viewBox="0 0 24 24" aria-hidden="true"><path d={doneOpen ? "m6 9 6 6 6-6" : "m9 6 6 6-6 6"} /></svg>
            Done · {doneTotal}
          </button>
          {doneOpen && done.map((group, index) => {
            const open = openGroups[group.key] ?? index === 0;
            return (
              <div key={group.key} className="themis-tasks-done-group">
                <button type="button" className="themis-tasks-fold is-group" aria-expanded={open} onClick={() => setOpenGroups(current => ({ ...current, [group.key]: !open }))}>
                  <svg viewBox="0 0 24 24" aria-hidden="true"><path d={open ? "m6 9 6 6 6-6" : "m9 6 6 6-6 6"} /></svg>
                  {group.label}
                </button>
                {open && (
                  <ul className="themis-workspace-list">
                    {group.subagents.map(task => subagentRow(task))}
                    {group.rows.map(taskRow)}
                    {group.background.map(backgroundRow)}
                  </ul>
                )}
              </div>
            );
          })}
        </section>
      )}
      <p className="themis-workspace-note themis-tasks-footer">Shows this chat only. Stopping never undoes work already done.</p>
    </div>
  );
}
