import { useEffect, useRef, useState } from "react";
import { Button } from "../components";
import type { ProviderKind } from "../lib/types";
import { getThread, listGoModels, sendMessage, setProvider, stopThread } from "../lib/tauri";
import { describeError, isConcurrencyLimitError, newId, toast, useActiveProject, useActiveThread, useApp } from "../state/store";
import { readSession, writeSession } from "../state/session";
import { useNewThread } from "./actions";
import { ResponseBody } from "../components/ResponseBody";
import { groupConversation, type ConversationRow } from "./conversation";
import type { ChatMessage } from "../state/reducer";
import "./ThreadView.css";


export function ThreadView() {
  const { state, dispatch } = useApp();
  const project = useActiveProject();
  const thread = useActiveThread();
  const [drafts, setDrafts] = useState<Record<string, string>>(() => readSession("drafts", {}));
  const draft = thread ? drafts[thread.id] ?? "" : "";
  function setDraft(value: string) {
    if (!thread) return;
    setDrafts(previous => { const next = { ...previous, [thread.id]: value }; writeSession("drafts", next); return next; });
  }
  const newThread = useNewThread();
  const scrollRef = useRef<HTMLDivElement>(null);
  const followOutput = useRef(true);
  const [models, setModels] = useState<string[]>([]);
  const [efforts, setEfforts] = useState<Record<string, string>>(() => readSession("efforts", {}));
  const [savedRunDurations, setSavedRunDurations] = useState<Record<string, number>>(() => readSession("runDurations", {}));
  const runDurations = { ...savedRunDurations, ...state.runDurations };
  const supportsEffort = !!thread && ["go", "openai"].includes(thread.provider) && /^(gpt-[56]|o3|o4)/.test(thread.model);
  const effort = thread && supportsEffort ? efforts[thread.id] ?? "" : "";
  const [stopping, setStopping] = useState(false);
  const [sending, setSending] = useState(false);
  const [switching, setSwitching] = useState(false);
  const [now, setNow] = useState(Date.now());
  const endRef = useRef<HTMLDivElement>(null);
  const running = thread === null ? false : (state.running[thread.id] ?? false);
  const messages = thread === null ? [] : (state.messages[thread.id] ?? []);
  const stream = thread === null ? "" : (state.streams[thread.id] ?? "");

  const sendError = thread === null ? undefined : state.sendErrors[thread.id];
  const readOnly = project !== null && !project.is_git;
  const composerDisabled = thread === null || running || sending || readOnly || project === null;
  const composerBusy = running || sending;
  const trace = thread ? state.traces[thread.id] ?? [] : [];
  const toolPending = trace.find(entry => entry.ok === undefined && !entry.summary.startsWith("approval "));
  const elapsed = thread ? Math.max(0, Math.floor((now - (state.runStartedAt[thread.id] ?? now)) / 1000)) : 0;
  useEffect(() => {
    if (!running) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [running, thread?.id]);
  useEffect(() => {
    function shortcut(event: KeyboardEvent) {
      if ((event.metaKey || event.ctrlKey) && event.shiftKey && event.key.toLowerCase() === "o") {
        event.preventDefault(); void newThread();
      }
    }
    document.addEventListener("keydown", shortcut);
    return () => document.removeEventListener("keydown", shortcut);
  }, [newThread]);
  useEffect(() => {
    let cancelled = false;
    setModels([]);
    if (thread?.provider === "go") listGoModels().then(items => { if (!cancelled) setModels(items); }).catch(() => {});
    return () => { cancelled = true; };
  }, [thread?.provider]);

  useEffect(() => {
    if (Object.keys(state.runDurations).length === 0) return;
    setSavedRunDurations(previous => {
      const next = { ...previous, ...state.runDurations };
      writeSession("runDurations", next);
      return next;
    });
  }, [state.runDurations]);

  useEffect(() => {
    if (followOutput.current) endRef.current?.scrollIntoView({ block: "end" });
  }, [messages, stream]);
  useEffect(() => {
    const element = scrollRef.current;
    if (element && thread) element.scrollTop = readSession(`scroll:${thread.id}`, element.scrollHeight);
  }, [thread?.id]);

  useEffect(() => { if (!running) setStopping(false); }, [running, thread?.id]);
  async function stop() {
    if (!thread || stopping) return;
    setStopping(true);
    try { await stopThread(thread.id); }
    catch (error: unknown) { setStopping(false); toast(dispatch, describeError(error), "danger"); }
  }

  async function send() {
    if (thread === null) return;
    const text = draft.trim();
    if (text === "" || composerDisabled) return;
    setDraft("");
    setSending(true);
    dispatch({
      type: "message/append",
      threadId: thread.id,
      message: { id: newId("msg"), role: "user", text },
    });
    try {
      await sendMessage(thread.id, text, effort);
      dispatch({ type: "thread/send-cleared", threadId: thread.id });
      getThread(thread.id).then(updated => dispatch({ type: "thread/updated", projectRoot: project!.root, thread: updated })).catch(() => {});
    } catch (error: unknown) {
      setDraft(text);
      const detail = describeError(error);
      // Records the error + pushes a toast (concurrency-aware) via reducer.
      dispatch({ type: "thread/send-failed", threadId: thread.id, error: detail });
      dispatch({
        type: "message/append",
        threadId: thread.id,
        message: {
          id: newId("msg"),
          role: "system",
          text: isConcurrencyLimitError(detail)
            ? `Send rejected: ${state.settings.concurrency_limit} runs already active — wait or raise the limit in Settings`
            : `Send failed: ${detail}`,
        },
      });
    } finally {
      setSending(false);
    }
  }

  async function applyProvider(provider: ProviderKind, selectedModel?: string) {
    if (thread === null || running) return;
    setSwitching(true);
    try {
      const model = (selectedModel ?? thread.model).trim();
      await setProvider(
        thread.id,
        provider,
        model === "" ? undefined : model,
      );
      if (state.activeProjectRoot !== null) {
        const updated = await getThread(thread.id);
        dispatch({
          type: "thread/updated",
          projectRoot: state.activeProjectRoot,
          thread: updated,
        });
      }
    } catch (error: unknown) {
      toast(dispatch, `Provider switch failed: ${describeError(error)}`, "danger");
    } finally {
      setSwitching(false);
    }
  }

  if (project === null) {
    return (
      <div className="themis-thread">
        <div className="themis-welcome"><h1>What would you like to build?</h1><p>Create a project to start working with Themis.</p><Button variant="primary" onClick={() => dispatch({ type: "ui/project-dialog", open: true })}>Create project</Button></div>
      </div>
    );
  }

  if (thread === null) {
    return (
      <div className="themis-thread">
        <div className="themis-welcome"><h1>{project.name}</h1><p>Start a thread to describe your next change.</p><Button variant="primary" onClick={() => void newThread()}>New thread</Button></div>
      </div>
    );
  }

  return (
    <div className="themis-thread">
      <div ref={scrollRef} className="themis-thread-stream" aria-live="polite" onScroll={event => { const element = event.currentTarget; followOutput.current = element.scrollHeight - element.scrollTop - element.clientHeight < 64; writeSession(`scroll:${thread.id}`, element.scrollTop); }}>
        {messages.length === 0 && stream === "" ? (
          <p className="themis-thread-start">Describe what you’d like to build or change.</p>
        ) : (
          groupConversation(messages, running, runDurations).map(row => {
            if (row.kind === "message") return <Message key={row.message.id} message={row.message} running={false} />;
            if (row.kind === "milestone") return <Milestone key={row.id} row={row} />;
            const durationMs = row.active ? elapsed * 1000 : row.durationMs;
            const durationSeconds = durationMs === undefined ? undefined : Math.floor(durationMs / 1000);
            const durationLabel = durationSeconds === undefined ? "" : durationSeconds < 60
              ? `${durationSeconds}s elapsed`
              : `${Math.floor(durationSeconds / 60)}m${durationSeconds % 60 === 0 ? "" : ` ${durationSeconds % 60}s`} elapsed`;
            return <details key={row.id} className="themis-run-bucket" open={row.active || undefined}>
              <summary><span className="themis-tool-chevron" aria-hidden="true">›</span><span>Activities</span><span className="themis-run-meta">{row.toolCount} tool call{row.toolCount === 1 ? "" : "s"}{durationLabel && ` · ${durationLabel}`}</span></summary>
              <div className="themis-run-bucket-body">{row.items.flatMap(item => item.items.map(message => <Message key={message.id} message={message} running={item.active} />))}</div>
            </details>;
          })
        )}
        {stream !== "" && (
          <div className="themis-thread-msg themis-thread-msg--assistant themis-thread-msg--live">
            <ResponseBody text={stream} streaming />
          </div>
        )}
        {(running || sending) && <div className="themis-run-progress" role="status">
          <div className="themis-run-status"><span className={stopping ? "" : "themis-shimmer"}>{stopping ? "Stopping…" : state.compacting[thread.id] ? "Compacting context…" : toolPending ? `Using ${toolPending.tool.replace(/_/g, " ")}…` : "Thinking…"}</span><time aria-label="Elapsed time">{Math.floor(elapsed / 60)}:{String(elapsed % 60).padStart(2, "0")}</time></div>

        </div>}
        <div ref={endRef} />
      </div>

      {sendError && <p className="themis-thread-senderror" role="alert">{sendError}</p>}
      <div className="themis-thread-composer">
        {thread.provider !== "ollama" && thread.provider !== "custom" && !state.secretStatus[thread.provider] && <p className="themis-thread-hint">Connect {thread.provider === "go" ? "OpenCode Go" : thread.provider} to send your first message. <Button variant="ghost" size="small" onClick={() => dispatch({ type: "ui/view", view: "settings" })}>Open settings</Button></p>}
        <label className="themis-sr-only" htmlFor="themis-composer">
          Message
        </label>
        <textarea
          id="themis-composer"
          className="themis-thread-input"
          placeholder={
            running ? "Run in progress…" : readOnly ? "Start a new thread to continue" : "Message Themis…"
          }
          value={draft}
          disabled={composerDisabled}
          rows={2}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
              event.preventDefault();
              void send();
            }
          }}
        />
        <div className="themis-thread-composer-actions">
          <Button variant="ghost" size="small" aria-label="New thread" title="New thread (⌘/Ctrl+Shift+O)" onClick={() => void newThread()}>+</Button>
          <div className="themis-composer-selectors">
            <select aria-label="Model" title="Model" value={thread.model} disabled={composerBusy || switching} onChange={event => void applyProvider(thread.provider, event.target.value)}>
              {[...new Set([thread.model, ...models])].map(model => <option key={model} value={model}>{model || "Default model"}</option>)}
            </select>
            <select aria-label="Reasoning effort" title={supportsEffort ? "Reasoning effort" : "This model manages its own reasoning"} value={effort} disabled={composerBusy || !supportsEffort} onChange={event => { const next = { ...efforts, [thread.id]: event.target.value }; setEfforts(next); writeSession("efforts", next); }}>
              <option value="">Default effort</option><option value="low">Low</option><option value="medium">Medium</option><option value="high">High</option>
            </select>
          </div>
          {running ? <Button className="themis-send" variant="ghost" aria-label={stopping ? "Stopping" : "Stop"} title={stopping ? "Stopping after current action" : "Stop"} disabled={stopping} onClick={() => void stop()}><svg width="14" height="14" viewBox="0 0 16 16" aria-hidden="true"><rect x="3" y="3" width="10" height="10" rx="2" fill="currentColor" /></svg></Button> : <Button
            className="themis-send" variant="primary" aria-label="Send" title="Send (Enter)"
            disabled={composerDisabled || draft.trim() === ""} onClick={() => void send()}
          ><svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true"><path d="M8 13V3M3 8l5-5 5 5" /></svg></Button>}
        </div>
        <div className="themis-composer-shortcuts"><span>↵ Send · ⇧↵ New line</span><span>⌘/Ctrl ⇧ O New thread</span></div>
      </div>

    </div>
  );
}

function Milestone({ row }: { row: Extract<ConversationRow, { kind: "milestone" }> }) {
  const body = <div className="themis-milestone-body">{row.items.map(message => <Message key={message.id} message={message} running={row.active} />)}</div>;
  return <details className="themis-milestone" open={row.active || undefined}>
    <summary><span className="themis-tool-chevron" aria-hidden="true">›</span><span className={row.active ? "themis-shimmer" : ""}>{row.title}</span></summary>
    {body}
  </details>;
}

function Message({ message, running }: { message: ChatMessage; running: boolean }) {
  const actionWaiting = running && message.role === "assistant" && !message.final && !message.tool;
  return <div className={`themis-thread-msg themis-thread-msg--${message.role}${message.text.startsWith("Stopped by you.") ? " themis-thread-msg--stopped" : ""}${actionWaiting ? " themis-thread-msg--action-waiting" : ""}`}>
    {message.tool ? <details className="themis-tool-message">
      <summary className={message.tool.ok === undefined && running ? "themis-shimmer" : ""}><span className="themis-tool-chevron" aria-hidden="true">›</span><span aria-hidden="true">{message.tool.ok === undefined ? "◌" : message.tool.ok ? "✓" : "!"}</span> {message.tool.name.replace(/_/g, " ")}<span className="themis-tool-status">{message.tool.ok === undefined ? running ? "Running" : "Interrupted" : message.tool.ok ? "Done" : "Failed"}</span></summary>
      <div className="themis-tool-detail"><strong>Input</strong><pre>{message.text}</pre><strong>Output</strong><pre>{message.tool.output ?? (message.tool.ok === undefined && running ? "Waiting for output…" : "Output was not recorded for this older call.")}</pre></div>
    </details> : message.text.startsWith("Run failed:") ? <details className="themis-request-error"><summary>Request failed · Details</summary><p className="themis-thread-text">{message.text}</p></details> : message.role === "assistant" ? <ResponseBody text={message.text} /> : <p className="themis-thread-text">{message.text}</p>}
  </div>;
}
