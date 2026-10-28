import { AttachmentPreview } from "./AttachmentPreview";
import { MediaGallery, MediaStrip, MediaViewer } from "./MediaStrip";
import type { PromptPlugin, PromptSkill } from "../lib/prompt";
import { PromptText } from "../components/PromptInput";
import { usePromptSkills } from "../lib/usePromptSkills";
import { describeError, toast, useActiveProject, useApp } from "../state/store";
import { browserOpenUrl } from "../lib/browser";
import { ANSWER_MARKER, type ChangedFile, type Question } from "../lib/types";
import { QuestionNote, useNowAt } from "./QuestionBox";
import { placeQuestion, questionOnComposer, QUESTION_OPEN_MS, usePlacements } from "../state/composerSlot";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ResponseBody, ResponseLinks } from "../components/ResponseBody";
import "./GoalBar.css";
import type { ChatMessage, ToolTraceEntry } from "../state/reducer";
import { readSession, writeSession } from "../state/session";
import { groupConversation, toolDisplayName, type ConversationRow } from "./conversation";
import { describeTools } from "./toolActivity";
import { OverviewBox, type Overview } from "./OverviewBox";

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
  /** The chat overview box, when it shows. */
  overview?: Overview;
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
  overview,
}: ThreadConversationProps) {
  const project = useActiveProject();
  const { skills, plugins } = usePromptSkills(project?.root, running);
  const scrollRef = useRef<HTMLDivElement>(null);
  const followOutput = useRef(true);
  const endRef = useRef<HTMLDivElement>(null);
  const [now, setNow] = useState(Date.now());
  const toolPending = traces.find(entry => entry.ok === undefined && !entry.summary.startsWith("approval "));
  const media = useMemo(() => messages.flatMap(message => message.media ?? []), [messages]);
  const [viewing, setViewing] = useState<number | null>(null);
  const openMedia = useCallback((path: string) => { const index = media.findIndex(file => file.path === path); if (index >= 0) setViewing(index); }, [media]);
  const gallery = useMemo(() => ({ threadId, items: media, open: openMedia }), [threadId, media, openMedia]);
  const { dispatch } = useApp();
  // Links in replies open in this chat's browser, shown in the Workspace Browser tab.
  const openLink = useCallback((url: string) => {
    dispatch({ type: "ui/diff-panel", open: true, tab: "browser" });
    browserOpenUrl(threadId, url).catch(error => toast(dispatch, `Could not open the link: ${describeError(error)}`, "danger"));
  }, [dispatch, threadId]);
  const elapsed = Math.max(0, Math.floor((now - (runStartedAt ?? now)) / 1000));

  useEffect(() => {
    if (!running) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [running, threadId]);

  // New output scrolls into view while following.
  useEffect(() => {
    if (followOutput.current) endRef.current?.scrollIntoView({ block: "end" });
  }, [messages, stream]);

  useEffect(() => {
    const element = scrollRef.current;
    if (element) element.scrollTop = readSession(`scroll:${threadId}`, element.scrollHeight);
  }, [threadId]);

  return (
    <MediaGallery.Provider value={gallery}>
    <ResponseLinks.Provider value={openLink}>
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
            threadId={threadId}
            skills={skills} plugins={plugins}
            activeRunId={activeRunId}
            runDurations={runDurations}
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
                    ? `Using ${toolDisplayName(toolPending.tool, toolPending.label)}…`
                    : "Thinking…"}
            </span>
            <time aria-label="Elapsed time">{Math.floor(elapsed / 60)}:{String(elapsed % 60).padStart(2, "0")}</time>
          </div>
        </div>
      )}
      <div ref={endRef} />
    </div>
    {overview && <OverviewBox threadId={threadId} overview={overview} onOpenMedia={openMedia} />}
    {viewing !== null && media[viewing] && <MediaViewer threadId={threadId} items={media} index={viewing} onIndex={setViewing} onClose={() => setViewing(null)} />}
    </ResponseLinks.Provider>
    </MediaGallery.Provider>
  );
}

function ConversationRow({
  row,
  threadId,
  activeRunId,
  runDurations,
  skills,
  plugins,
}: {
  row: ConversationRow;
  threadId: string;
  activeRunId: string | undefined;
  runDurations: Record<string, number>;
  skills: PromptSkill[];
  plugins: PromptPlugin[];
}) {
  if (row.kind === "message" && row.message.question) {
    return <QuestionRow question={row.message.question} />;
  }
  if (row.kind === "message") {
    const durationMs = row.message.final && row.message.runId ? runDurations[row.message.runId] : undefined;
    return <Message threadId={threadId} message={row.message} activeRunId={activeRunId} durationMs={durationMs} skills={skills} plugins={plugins} />;
  }
  if (row.kind === "milestone") return <Milestone row={row} activeRunId={activeRunId} skills={skills} plugins={plugins} />;

  // The agent's progress sentences stay on screen; each stretch of tool calls
  // between them folds into one quiet row.
  const parts: ({ kind: "text"; message: ChatMessage; active: boolean } | { kind: "tools"; messages: ChatMessage[]; active: boolean })[] = [];
  for (const item of row.items) {
    for (const message of item.items) {
      const last = parts[parts.length - 1];
      if (message.tool && last?.kind === "tools") last.messages.push(message);
      else if (message.tool) parts.push({ kind: "tools", messages: [message], active: item.active });
      else parts.push({ kind: "text", message, active: item.active });
    }
  }
  return (
    <div className="themis-run-activity">
      {parts.map(part => part.kind === "text"
        ? <Message key={part.message.id} message={part.message} activeRunId={part.active ? activeRunId : undefined} skills={skills} plugins={plugins} />
        : <ToolGroup key={part.messages[0]!.id} messages={part.messages} activeRunId={row.active ? activeRunId : undefined} skills={skills} plugins={plugins} />)}
    </div>
  );
}

/** Consecutive tool calls as one folded row that says what they did ("Read 4 files ·
 *  Ran python3 check.py"); open while one of them runs. */
function ToolGroup({ messages, activeRunId, skills, plugins }: { messages: ChatMessage[]; activeRunId: string | undefined; skills: PromptSkill[]; plugins: PromptPlugin[] }) {
  const running = activeRunId !== undefined && messages.some(message => message.tool?.ok === undefined);
  const failed = messages.filter(message => message.tool?.ok === false).length;
  return (
    <details className="themis-tool-group" open={running || undefined}>
      <summary>
        <span className="themis-tool-chevron" aria-hidden="true">›</span>
        <span title={`${messages.length} tool call${messages.length === 1 ? "" : "s"}`}>{describeTools(messages)}</span>
        {failed > 0 && <span className="themis-run-meta">{failed} failed</span>}
      </summary>
      <div className="themis-tool-group-body">
        {messages.map(message => <Message key={message.id} message={message} activeRunId={activeRunId} skills={skills} plugins={plugins} />)}
      </div>
    </details>
  );
}

function Milestone({
  row,
  activeRunId,
  skills,
  plugins,
}: {
  row: Extract<ConversationRow, { kind: "milestone" }>;
  skills: PromptSkill[];
  plugins: PromptPlugin[];
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
          <Message key={message.id} message={message} activeRunId={row.active ? activeRunId : undefined} skills={skills} plugins={plugins} />
        ))}
      </div>
    </details>
  );
}

function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return <button type="button" className="themis-response-action" aria-label={copied ? "Copied" : "Copy response"} title={copied ? "Copied" : "Copy"}
    onClick={() => void navigator.clipboard?.writeText(text).then(() => { setCopied(true); setTimeout(() => setCopied(false), 1500); }).catch(() => {})}>
    {copied
      ? <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true"><path d="m5 12 5 5 9-10" /></svg>
      : <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true"><rect x="8" y="8" width="12" height="12" rx="2" /><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2" /></svg>}
  </button>;
}

const STATUS_LETTER: Record<ChangedFile["status"], string> = { added: "A", modified: "M", deleted: "D", renamed: "R" };

const EDITED_PREVIEW = 5;

/** The files one turn changed, in a card under its response; a row opens that file's diff. Long lists show five until expanded. */
function EditedFiles({ files }: { files: ChangedFile[] }) {
  const { dispatch } = useApp();
  const [expanded, setExpanded] = useState(false);
  const counts = (["added", "modified", "deleted", "renamed"] as const)
    .map(status => [status, files.filter(file => file.status === status).length] as const)
    .filter(([, count]) => count > 0);
  const shown = expanded ? files : files.slice(0, EDITED_PREVIEW);
  return <section className="themis-edited-files" aria-label="Files edited in this turn">
    <header>
      <h3>Edited {files.length} file{files.length === 1 ? "" : "s"}</h3>
      <span className="themis-edited-counts">{counts.map(([status, count]) => <span key={status} className={`themis-edited-status--${status}`}>{count} {status}</span>)}</span>
    </header>
    <ul className={expanded ? "is-expanded" : undefined}>{shown.map(file => {
      const slash = file.path.lastIndexOf("/");
      return <li key={file.path}><button type="button" title={`${file.path} · show diff`} onClick={() => dispatch({ type: "ui/diff-panel", open: true, focus: file.path })}>
        <span className={`themis-edited-status themis-edited-status--${file.status}`} aria-label={file.status}>{STATUS_LETTER[file.status]}</span>
        <span className="themis-edited-path">{slash >= 0 && <span>{file.path.slice(0, slash + 1)}</span>}{file.path.slice(slash + 1)}</span>
      </button></li>;
    })}</ul>
    {files.length > EDITED_PREVIEW && <button type="button" className="themis-edited-more" aria-expanded={expanded} onClick={() => setExpanded(value => !value)}>{expanded ? "Show fewer" : `Show all ${files.length}`}</button>}
  </section>;
}

function CheckpointDivider({ checkpoint }: { checkpoint: NonNullable<ChatMessage["checkpoint"]> }) {
  const change = checkpoint.before !== undefined && checkpoint.after !== undefined ? ` · ${checkpoint.before}% → ${checkpoint.after}%` : "";
  return <div className="themis-checkpoint-divider" role="note"><span>Context compacted{change}</span></div>;
}

const GOAL_NOTE_TITLE: Record<NonNullable<ChatMessage["goalNote"]>["outcome"], string> = {
  reached: "Goal reached.",
  not_yet: "Not reached yet.",
  stuck: "Paused — stuck.",
  waiting: "Paused — waiting for your reply.",
  quota: "Paused — quota used up.",
  failed: "Turn failed.",
  small_window: "",
};

/** What the goal check concluded after a turn, or a warning as the goal starts (no title). */
export function GoalNote({ note }: { note: NonNullable<ChatMessage["goalNote"]> }) {
  return (
    <div className="themis-goal-note" role="note" data-outcome={note.outcome}>
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true"><circle cx="12" cy="12" r="8" /><circle cx="12" cy="12" r="4" /></svg>
      <span>{GOAL_NOTE_TITLE[note.outcome] && <><strong>{GOAL_NOTE_TITLE[note.outcome]}</strong> </>}{note.reason}</span>
    </div>
  );
}

/** A question in its turn: one line with its answer, or, once it has left the composer
 *  unanswered, a line that puts it back there. */
function QuestionRow({ question }: { question: Question }) {
  const placements = usePlacements();
  const now = useNowAt(question.answer === undefined && !question.blocking ? question.asked_at + QUESTION_OPEN_MS : undefined);
  return <QuestionNote question={question} onComposer={questionOnComposer(question, placements, now)} onOpen={() => placeQuestion(question.id, "composer")} />;
}

/** `12s` or `2m 32s`. */
function formatRunDuration(ms: number): string {
  const seconds = Math.floor(ms / 1000);
  return seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m${seconds % 60 === 0 ? "" : ` ${seconds % 60}s`}`;
}

function Message({ message, threadId, activeRunId, durationMs, skills, plugins }: { message: ChatMessage; threadId?: string; activeRunId: string | undefined; durationMs?: number; skills: PromptSkill[]; plugins: PromptPlugin[] }) {
  if (message.checkpoint) return <CheckpointDivider checkpoint={message.checkpoint} />;
  // The answered question box already shows the answer the agent was sent.
  if (message.role === "user" && message.text.startsWith(ANSWER_MARKER)) return null;
  // A goal turn starts with a runtime message: a divider, not a bubble from the user.
  if (message.goalTurn !== undefined) return <div className="themis-goal-turn" role="separator" aria-label={`Goal turn ${message.goalTurn}`}>Turn {message.goalTurn}</div>;
  if (message.goalNote) return <GoalNote note={message.goalNote} />;
  const text = message.attachments?.length ? message.text.split("\n\nAttached files (local paths;")[0]! : message.text;
  const isActiveRun = activeRunId !== undefined && message.runId === activeRunId;
  const actionWaiting = isActiveRun && message.role === "assistant" && !message.final && !message.tool;
  const completed = message.role === "assistant" && !isActiveRun;

  return (
    <>
    {threadId && !!message.attachments?.length && <div className="themis-message-attachments" aria-label="Attached files">{message.attachments.map(path => <AttachmentPreview key={path} threadId={threadId} path={path} />)}</div>}
    <div className={`themis-thread-msg themis-thread-msg--${message.role}${message.text.startsWith("Stopped by you.") ? " themis-thread-msg--stopped" : ""}${actionWaiting ? " themis-thread-msg--action-waiting" : ""}${completed ? " themis-thread-msg--completed" : ""}`}>
      {message.tool ? (
        <details className="themis-tool-message">
          <summary className={message.tool.ok === undefined && isActiveRun ? "themis-shimmer" : ""}>
            <span className="themis-tool-chevron" aria-hidden="true">›</span>
            <span aria-hidden="true">{message.tool.ok === undefined ? "◌" : message.tool.ok ? "✓" : "!"}</span>
            {toolDisplayName(message.tool.name, message.tool.label)}
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
        <>
        <details className="themis-request-error">
          <summary>Request failed · Details</summary>
          <p className="themis-thread-text"><PromptText value={message.text} skills={skills} plugins={plugins} /></p>
        </details>
        {!!message.changedFiles?.length && <EditedFiles files={message.changedFiles} />}
        </>
      ) : message.role === "assistant" ? (
        <>
          <ResponseBody text={message.text} />
          {!!message.media?.length && <MediaStrip files={message.media} />}
          {!!message.changedFiles?.length && <EditedFiles files={message.changedFiles} />}
          {(message.final || !message.runId) && <div className="themis-response-actions">
            <CopyButton text={message.text} />
            <small className="themis-response-model">{message.interrupted === "stopped" ? "Stopped by you" : message.interrupted === "failed" ? "Interrupted by an error" : message.model ?? "Model not recorded"}{durationMs !== undefined && ` · ${formatRunDuration(durationMs)}`}</small>
          </div>}
        </>
      ) : (
        <>
          <p className="themis-thread-text"><PromptText value={text} skills={skills} plugins={plugins} /></p>
        </>
      )}
    </div>
    </>
  );
}
