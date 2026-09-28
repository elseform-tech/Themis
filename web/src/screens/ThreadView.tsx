import { useEffect, useState } from "react";
import { Button } from "../components";
import type { ProviderKind } from "../lib/types";
import { getThread, listGoModels, sendMessage, setProvider, stopThread } from "../lib/tauri";
import { describeError, isConcurrencyLimitError, newId, toast, useActiveProject, useActiveThread, useApp } from "../state/store";
import { readSession, writeSession } from "../state/session";
import { useNewThread } from "./actions";
import { ThreadComposer } from "./ThreadComposer";
import { ThreadConversation } from "./ThreadConversation";
import "./ThreadView.css";


const EMPTY_MESSAGES: never[] = [];

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
  const [models, setModels] = useState<string[]>([]);
  const [efforts, setEfforts] = useState<Record<string, string>>(() => readSession("efforts", {}));
  const [savedRunDurations, setSavedRunDurations] = useState<Record<string, number>>(() => readSession("runDurations", {}));
  const runDurations = { ...savedRunDurations, ...state.runDurations };
  const supportsEffort = !!thread && ["go", "openai"].includes(thread.provider) && /^(gpt-[56]|o3|o4)/.test(thread.model);
  const effort = thread && supportsEffort ? efforts[thread.id] ?? "" : "";
  const [stopping, setStopping] = useState(false);
  const [sending, setSending] = useState(false);
  const [switching, setSwitching] = useState(false);
  const running = thread === null ? false : (state.running[thread.id] ?? false);
  const messages = thread === null ? EMPTY_MESSAGES : (state.messages[thread.id] ?? EMPTY_MESSAGES);
  const stream = thread === null ? "" : (state.streams[thread.id] ?? "");

  const sendError = thread === null ? undefined : state.sendErrors[thread.id];
  const readOnly = project !== null && !project.is_git;
  const composerDisabled = thread === null || running || sending || readOnly || project === null;
  const composerBusy = running || sending;
  const trace = thread ? state.traces[thread.id] ?? [] : [];
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
      <ThreadConversation
        threadId={thread.id}
        messages={messages}
        stream={stream}
        running={running}
        sending={sending}
        stopping={stopping}
        runDurations={runDurations}
        activeRunId={state.activeRunIds[thread.id]}
        runStartedAt={state.runStartedAt[thread.id]}
        compacting={state.compacting[thread.id] ?? false}
        traces={trace}
      />
      <ThreadComposer
        thread={thread}
        draft={draft}
        models={models}
        supportsEffort={supportsEffort}
        effort={effort}
        composerDisabled={composerDisabled}
        composerBusy={composerBusy}
        readOnly={readOnly}
        running={running}
        stopping={stopping}
        switching={switching}
        sendError={sendError}
        secretConfigured={
          thread.provider === "ollama" || thread.provider === "custom"
            ? true
            : Boolean(state.secretStatus[thread.provider])
        }
        onDraftChange={setDraft}
        onSend={() => void send()}
        onStop={() => void stop()}
        onNewThread={() => void newThread()}
        onProviderChange={model => void applyProvider(thread.provider, model)}
        onEffortChange={value => {
          const next = { ...efforts, [thread.id]: value };
          setEfforts(next);
          writeSession("efforts", next);
        }}
        onOpenSettings={() => dispatch({ type: "ui/view", view: "settings" })}
      />

    </div>
  );
}
