import type { PromptSkill } from "../lib/prompt";
import { PromptText } from "../components/PromptInput";
import { usePromptSkills } from "../lib/usePromptSkills";
import { useActiveProject } from "../state/store";
import { useEffect, useRef, useState } from "react";
import { ResponseBody } from "../components/ResponseBody";
import type { ChatMessage, ToolTraceEntry } from "../state/reducer";
import { readSession, writeSession } from "../state/session";
import { groupConversation, type ConversationRow } from "./conversation";

interface ThreadConversationProps {
  threadId: string;
  messages: ChatMessage[];
  stream: string;
  running: boolean;
  sending: boolean;
  stopping: boolean;
  runDurations: Record<string, number>;
  activeRunId: string | undefined;
  runStartedAt: number | undefined;
  compacting: boolean;
  traces: ToolTraceEntry[];
}

export function ThreadConversation({
  threadId,
  messages,
  stream,
  running,
  sending,
  stopping,
  runDurations,
  activeRunId,
  runStartedAt,
  compacting,
  traces,
}: ThreadConversationProps) {
  const project = useActiveProject();
  const { skills } = usePromptSkills(project?.root, running);
  const scrollRef = useRef<HTMLDivElement>(null);
  const followOutput = useRef(true);
  const endRef = useRef<HTMLDivElement>(null);
  const [now, setNow] = useState(Date.now());
  const toolPending = traces.find(entry => entry.ok === undefined && !entry.summary.startsWith("approval "));
  const elapsed = Math.max(0, Math.floor((now - (runStartedAt ?? now)) / 1000));

  useEffect(() => {
    if (!running) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [running, threadId]);

  useEffect(() => {
    if (followOutput.current) endRef.current?.scrollIntoView({ block: "end" });
  }, [messages, stream]);

  useEffect(() => {
    const element = scrollRef.current;
    if (element) element.scrollTop = readSession(`scroll:${threadId}`, element.scrollHeight);
  }, [threadId]);

  return (
    <div
      ref={scrollRef}
      className="themis-thread-stream"
      aria-live="polite"
      onScroll={event => {
        const element = event.currentTarget;
        followOutput.current = element.scrollHeight - element.scrollTop - element.clientHeight < 64;
        writeSession(`scroll:${threadId}`, element.scrollTop);
      }}
    >
      {messages.length === 0 && stream === "" ? (
        <p className="themis-thread-start">Describe what you’d like to build or change.</p>
      ) : (
        groupConversation(messages, running, runDurations, activeRunId).map(row => (
          <ConversationRow
            key={row.kind === "message" ? row.message.id : row.id}
            row={row}
            skills={skills}
            activeRunId={activeRunId}
            elapsed={elapsed}
          />
        ))
      )}
      {stream !== "" && (
        <div className="themis-thread-msg themis-thread-msg--assistant themis-thread-msg--live">
          <ResponseBody text={stream} streaming />
        </div>
      )}
      {(running || sending) && (
        <div className="themis-run-progress" role="status">
          <div className="themis-run-status">
            <span className={stopping || compacting ? "" : "themis-shimmer"}>
              {stopping
                ? "Stopping…"
                : compacting
                  ? "Compacting context…"
                  : toolPending
                    ? `Using ${toolPending.tool.replace(/_/g, " ")}…`
                    : "Thinking…"}
            </span>
            <time aria-label="Elapsed time">{Math.floor(elapsed / 60)}:{String(elapsed % 60).padStart(2, "0")}</time>
          </div>
        </div>
      )}
      <div ref={endRef} />
    </div>
  );
}

function ConversationRow({
  row,
  activeRunId,
  elapsed,
  skills,
}: {
  row: ConversationRow;
  activeRunId: string | undefined;
  elapsed: number;
  skills: PromptSkill[];
}) {
  if (row.kind === "message") return <Message message={row.message} activeRunId={activeRunId} skills={skills} />;
  if (row.kind === "milestone") return <Milestone row={row} activeRunId={activeRunId} skills={skills} />;

  const durationMs = row.active ? elapsed * 1000 : row.durationMs;
  const durationSeconds = durationMs === undefined ? undefined : Math.floor(durationMs / 1000);
  const durationLabel = durationSeconds === undefined ? "" : durationSeconds < 60
    ? `${durationSeconds}s elapsed`
    : `${Math.floor(durationSeconds / 60)}m${durationSeconds % 60 === 0 ? "" : ` ${durationSeconds % 60}s`} elapsed`;

  return (
    <details className="themis-run-bucket" open={row.active || undefined}>
      <summary>
        <span className="themis-tool-chevron" aria-hidden="true">›</span>
        <span>Activities</span>
        <span className="themis-run-meta">
          {row.toolCount} tool call{row.toolCount === 1 ? "" : "s"}{durationLabel && ` · ${durationLabel}`}
        </span>
      </summary>
      <div className="themis-run-bucket-body">
        {row.items.flatMap(item => item.items.map(message => (
          <Message key={message.id} message={message} activeRunId={item.active ? activeRunId : undefined} skills={skills} />
        )))}
      </div>
    </details>
  );
}

function Milestone({
  row,
  activeRunId,
  skills,
}: {
  row: Extract<ConversationRow, { kind: "milestone" }>;
  skills: PromptSkill[];
  activeRunId: string | undefined;
}) {
  return (
    <details className="themis-milestone" open={row.active || undefined}>
      <summary>
        <span className="themis-tool-chevron" aria-hidden="true">›</span>
        <span>{row.title}</span>
      </summary>
      <div className="themis-milestone-body">
        {row.items.map(message => (
          <Message key={message.id} message={message} activeRunId={row.active ? activeRunId : undefined} skills={skills} />
        ))}
      </div>
    </details>
  );
}

function Message({ message, activeRunId, skills }: { message: ChatMessage; activeRunId: string | undefined; skills: PromptSkill[] }) {
  const isActiveRun = activeRunId !== undefined && message.runId === activeRunId;
  const actionWaiting = isActiveRun && message.role === "assistant" && !message.final && !message.tool;
  const completed = message.role === "assistant" && !isActiveRun;

  return (
    <div className={`themis-thread-msg themis-thread-msg--${message.role}${message.text.startsWith("Stopped by you.") ? " themis-thread-msg--stopped" : ""}${actionWaiting ? " themis-thread-msg--action-waiting" : ""}${completed ? " themis-thread-msg--completed" : ""}`}>
      {message.tool ? (
        <details className="themis-tool-message">
          <summary className={message.tool.ok === undefined && isActiveRun ? "themis-shimmer" : ""}>
            <span className="themis-tool-chevron" aria-hidden="true">›</span>
            <span aria-hidden="true">{message.tool.ok === undefined ? "◌" : message.tool.ok ? "✓" : "!"}</span>
            {message.tool.name.replace(/_/g, " ")}
            <span className="themis-tool-status">
              {message.tool.ok === undefined ? isActiveRun ? "Running" : "Interrupted" : message.tool.ok ? "Done" : "Failed"}
            </span>
          </summary>
          <div className="themis-tool-detail">
            <strong>Input</strong>
            <pre>{message.text}</pre>
            <strong>Output</strong>
            <pre>{message.tool.output ?? (message.tool.ok === undefined && isActiveRun ? "Waiting for output…" : "Output was not recorded for this older call.")}</pre>
          </div>
        </details>
      ) : message.text.startsWith("Run failed:") ? (
        <details className="themis-request-error">
          <summary>Request failed · Details</summary>
          <p className="themis-thread-text"><PromptText value={message.text} skills={skills} /></p>
        </details>
      ) : message.role === "assistant" ? (
        <>
          <ResponseBody text={message.text} />
          {(message.final || !message.runId) && <small className="themis-response-model">{message.model ?? "Model not recorded"}</small>}
        </>
      ) : (
        <p className="themis-thread-text"><PromptText value={message.text} skills={skills} /></p>
      )}
    </div>
  );
}
