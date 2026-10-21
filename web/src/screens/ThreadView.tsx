import { open } from "@tauri-apps/plugin-dialog";
import type { Attachment } from "../lib/tauri";
import { usePromptSkills } from "../lib/usePromptSkills";
import { useEffect, useRef, useState } from "react";
import { Button } from "../components";
import type { GoModel } from "../lib/types";
import { attachFiles, editGoal, enqueueMessage, getGoal, getThread, getWorkspace, listDiff, listGoModels, listQueue, pauseGoal, removeGoal, removeQueuedMessage, resumeGoal, sendMessage, setProvider, setThreadEffort, setThreadApprovalMode, steerQueuedMessage, stopThread, updateQueuedMessage } from "../lib/tauri";
import { describeError, isConcurrencyLimitError, newId, toast, useActiveProject, useActiveThread, useApp } from "../state/store";
import { readSession, writeSession } from "../state/session";
import { useNewThread } from "./actions";
import { ThreadComposer } from "./ThreadComposer";
import { ThreadConversation } from "./ThreadConversation";
import { ThreadHeader } from "./ThreadHeader";
import { GoalBar } from "./GoalBar";
import { QueueStack } from "./QueueStack";
import { CompactionNotice } from "./ContextMeter";
import { WorkspacePanel } from "./WorkspacePanel";
import { useProjectWorkspace } from "./useProjectWorkspace";
import { useThreadTasksSync } from "./threadTasks";
import { skillToken } from "../lib/prompt";
import "./ThreadView.css";


const EMPTY_MESSAGES: never[] = [];

export function ThreadView() {
  const { state, dispatch, armManualRun, cancelManualRun } = useApp();
  const project = useActiveProject();
  const thread = useActiveThread();
  useThreadTasksSync(thread?.id ?? null);
  const [drafts, setDrafts] = useState<Record<string, string>>(() => readSession("drafts", {}));
  const [pendingFiles, setPendingFiles] = useState<Record<string, Attachment[]>>(() => readSession("attachments", {}));
  const attachments = thread ? pendingFiles[thread.id] ?? [] : [];
  const [attaching, setAttaching] = useState(false);
  const draft = thread ? drafts[thread.id] ?? "" : "";
  function setDraft(value: string) {
    if (!thread) return;
    setDrafts(previous => { const next = { ...previous, [thread.id]: value }; writeSession("drafts", next); return next; });
  }
  useEffect(() => {
    const reload = () => setDrafts(readSession("drafts", {}));
    window.addEventListener("themis-drafts-changed", reload);
    return () => window.removeEventListener("themis-drafts-changed", reload);
  }, []);
  useEffect(() => {
    const reload = () => setPendingFiles(readSession("attachments", {}));
    window.addEventListener("themis-attachments-changed", reload);
    return () => window.removeEventListener("themis-attachments-changed", reload);
  }, []);
  const newThread = useNewThread();
  const { skills: promptSkills, plugins: promptPlugins, error: skillsError } = usePromptSkills(project?.root, state.running[thread?.id ?? ""]);
  const [models, setModels] = useState<GoModel[]>([]);
  const threadId = thread?.id;
  const [savedRunDurations, setSavedRunDurations] = useState<Record<string, number>>(() => readSession("runDurations", {}));
  const runDurations = { ...savedRunDurations, ...state.runDurations };
  const effortLevels = models.find(model => model.id === thread?.model)?.effort_levels ?? [];
  const savedEffort = thread?.reasoning_effort ?? "";
  const effort = effortLevels.includes(savedEffort) ? savedEffort : "";
  const [stopping, setStopping] = useState(false);
  const [sending, setSending] = useState(false);
  const [switching, setSwitching] = useState(false);
  const [permissionSaving,setPermissionSaving]=useState(false);
  const [noticeDismissed, setNoticeDismissed] = useState<Record<string, boolean>>({});
  const soon = threadId !== undefined && state.contextBudgets[threadId]?.compaction_soon === true;
  // A dismissed heads-up returns the next time compaction is projected.
  useEffect(() => {
    if (threadId !== undefined && !soon) setNoticeDismissed(previous => previous[threadId] ? { ...previous, [threadId]: false } : previous);
  }, [threadId, soon]);
  const permissionPending=useRef(false);
  async function changeApprovalMode(mode:"custom"|"yolo") {
    if(!thread || !project || permissionPending.current)return;
    permissionPending.current=true;setPermissionSaving(true);
    try {const updated=await setThreadApprovalMode(thread.id,mode);dispatch({type:"thread/updated",projectRoot:project.root,thread:updated});}
    catch(error){toast(dispatch,`Permission change failed: ${describeError(error)}`,"danger");}
    finally {permissionPending.current=false;setPermissionSaving(false);}
  }
  const running = thread === null ? false : (state.running[thread.id] ?? false);
  const messages = thread === null ? EMPTY_MESSAGES : (state.messages[thread.id] ?? EMPTY_MESSAGES);
  const stream = thread === null ? "" : (state.streams[thread.id] ?? "");

  const sendError = thread === null ? undefined : state.sendErrors[thread.id];
  const readOnly = project !== null && !project.is_git && thread?.approval_mode !== "yolo";
  // A message sent while a run is going waits in the chat's queue.
  const composerDisabled = thread === null || thread.provider !== "go" || sending || attaching || permissionSaving || readOnly || project === null;
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
  const runningNow = threadId !== undefined && (state.running[threadId] ?? false);
  const workspace = useProjectWorkspace(project?.root, runningNow, getWorkspace);
  const diffKnown = threadId !== undefined && state.diffs[threadId] !== undefined;
  const gitProject = project?.is_git === true;
  useEffect(() => {
    if (threadId === undefined || diffKnown || !gitProject) return;
    let cancelled = false;
    listDiff(threadId).then(diff => { if (!cancelled) dispatch({ type: "diff/set", threadId, diff }); }).catch(() => {});
    return () => { cancelled = true; };
  }, [threadId, diffKnown, gitProject, dispatch]);
  // The chat's queue and goal; events keep them current afterwards.
  useEffect(() => {
    if (threadId === undefined) return;
    let cancelled = false;
    listQueue(threadId).then(queue => { if (!cancelled) dispatch({ type: "queue/loaded", threadId, queue }); }).catch(() => {});
    getGoal(threadId).then(goal => { if (!cancelled) dispatch({ type: "goal/loaded", threadId, goal }); }).catch(() => {});
    return () => { cancelled = true; };
  }, [threadId, dispatch]);
  useEffect(() => {
    let cancelled = false;
    setModels([]);
    if (threadId) listGoModels().then(items => { if (!cancelled) setModels(items); }).catch(() => {});
    return () => { cancelled = true; };
  }, [threadId]);

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

  function updateAttachments(id: string, update: (files: Attachment[]) => Attachment[]) {
    const previous = readSession<Record<string, Attachment[]>>("attachments", {});
    const next = { ...previous, [id]: update(previous[id] ?? []) };
    writeSession("attachments", next);
    window.dispatchEvent(new Event("themis-attachments-changed"));
  }
  async function attach() {
    if (!thread || composerDisabled) return;
    const id = thread.id;
    setAttaching(true);
    try {
      const paths = await open({ multiple: true, directory: false, title: "Attach files" });
      if (paths) {
        const files = await attachFiles(id, Array.isArray(paths) ? paths : [paths]);
        updateAttachments(id, previous => [...previous, ...files]);
      }
    } catch (error) { toast(dispatch, `Attachment failed: ${describeError(error)}`, "danger"); }
    finally { setAttaching(false); }
  }

  async function send() {
    if (thread === null || permissionPending.current) return;
    const text = draft.trim() || (attachments.length ? "Inspect the attached files." : "");
    if (text === "" || composerDisabled) return;
    setDraft("");
    setSending(true);
    if (running) {
      try {
        await enqueueMessage(thread.id, text, effort, attachments.map(file => file.path));
        updateAttachments(thread.id, files => files.filter(file => !attachments.some(sent => sent.path === file.path)));
      } catch (error: unknown) {
        setDraft(text);
        toast(dispatch, `Could not queue the message: ${describeError(error)}`, "danger");
      } finally {
        setSending(false);
      }
      return;
    }
    dispatch({
      type: "message/append",
      threadId: thread.id,
      message: { id: newId("msg"), role: "user", text, attachments: attachments.map(file => file.path) },
    });
    armManualRun(thread.id);
    try {
      await sendMessage(thread.id, text, effort, attachments.map(file => file.path));
      updateAttachments(thread.id, files => files.filter(file => !attachments.some(sent => sent.path === file.path)));
      dispatch({ type: "thread/send-cleared", threadId: thread.id });
      getThread(thread.id).then(updated => dispatch({ type: "thread/updated", projectRoot: project!.root, thread: updated })).catch(() => {});
    } catch (error: unknown) {
      cancelManualRun(thread.id);
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

  async function applyProvider(selectedModel: string) {
    if (thread === null) return;
    setSwitching(true);
    try {
      const model = selectedModel.trim();
      await setProvider(
        thread.id,
        "go",
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

  const budget = state.contextBudgets[thread.id];
  const goal = state.goals[thread.id] ?? null;
  const queue = state.queues[thread.id] ?? [];
  const compacting = state.compacting[thread.id] ?? false;
  const diff = state.diffs[thread.id];
  const changedFiles = diff?.available ? diff.files.length : undefined;
  const empty = messages.length === 0 && stream === "" && !running && !sending;
  const showNotice = budget?.compaction_soon === true && !compacting && running && !noticeDismissed[thread.id];
  const starters = [
    ...promptSkills.filter(skill => !skill.retired).slice(0, 2).map(skill => ({ label: `/ ${skill.name}`, text: `${skillToken(skill.id)} ` })),
    { label: "Review the current changes", text: "Review the current changes and point out problems." },
    { label: "Explain this project", text: "Explain how this project is structured." },
    { label: "Find and fix failing tests", text: "Run the tests, then find and fix any failures." },
  ].slice(0, 4);
  const composer = (
    <ThreadComposer
      thread={thread}
      key={thread.id}
      hero={empty}
      draft={draft}
      attachments={attachments}
      onAttach={() => void attach()}
      onRemoveAttachment={path => updateAttachments(thread.id, files => files.filter(file => file.path !== path))}
      skills={promptSkills}
      plugins={promptPlugins}
      history={messages.filter(m => m.role === "user" && !messages.some(error => m.runId !== undefined && error.runId === m.runId && error.text.startsWith("Send failed:"))).map(m => m.text)}
      composerDisabled={composerDisabled}
      readOnly={readOnly}
      running={running}
      stopping={stopping}
      sendError={sendError ?? (skillsError ? `Skills unavailable: ${skillsError}` : undefined)}
      secretConfigured={state.secretStatus.go}
      onDraftChange={setDraft}
      onSend={() => void send()}
      onStop={() => void stop()}
      onNewThread={() => void newThread()}
      permissionSaving={permissionSaving}
      onApprovalModeChange={mode => void changeApprovalMode(mode)}
      onOpenSettings={() => dispatch({ type: "ui/view", view: "settings" })}
      budget={budget}
      compacting={compacting}
      location={{ name: project.name, root: project.root, branch: workspace?.branch ?? null }}
      top={<>
        {goal && <GoalBar goal={goal}
          onEdit={async text => { try { await editGoal(thread.id, text); } catch (error) { toast(dispatch, `Could not save the goal: ${describeError(error)}`, "danger"); throw error; } }}
          onPause={() => void pauseGoal(thread.id).catch(error => toast(dispatch, describeError(error), "danger"))}
          onResume={() => void resumeGoal(thread.id).catch(error => toast(dispatch, describeError(error), "danger"))}
          onRemove={() => void removeGoal(thread.id).catch(error => toast(dispatch, describeError(error), "danger"))} />}
        <QueueStack queue={queue} running={running}
          onSteer={id => void steerQueuedMessage(thread.id, id).catch(error => toast(dispatch, describeError(error), "danger"))}
          onHold={(id, held) => void updateQueuedMessage(thread.id, id, { held }).catch(() => {})}
          onSave={async (id, text) => { try { await updateQueuedMessage(thread.id, id, { text, held: false }); } catch (error) { toast(dispatch, describeError(error), "danger"); } }}
          onRemove={id => void removeQueuedMessage(thread.id, id).catch(error => toast(dispatch, describeError(error), "danger"))} />
      </>}
    />
  );

  return (
    <div className={`themis-thread-layout${state.diffPanelOpen ? " is-reviewing" : ""}`}>
    <div className="themis-thread">
      <ThreadHeader
        thread={thread}
        models={models}
        effortLevels={effortLevels}
        effort={effort}
        busy={sending || attaching}
        running={running}
        switching={switching}
        onProviderChange={model => void applyProvider(model)}
        onEffortChange={value => {
          void setThreadEffort(thread.id, value)
            .then(updated => dispatch({ type: "thread/updated", projectRoot: project.root, thread: updated }))
            .catch(error => toast(dispatch, `Effort change failed: ${describeError(error)}`, "danger"));
        }}
        changedFiles={changedFiles}
        reviewOpen={state.diffPanelOpen}
        onToggleReview={() => dispatch({ type: "ui/diff-panel", open: !state.diffPanelOpen })}
        recording={state.browsers[thread.id]?.recording ? { ...state.browsers[thread.id].recording!, at: state.browsers[thread.id].at } : undefined}
        onOpenBrowser={() => dispatch({ type: "ui/diff-panel", open: true, tab: "browser" })}
      />
      {empty ? (
        <section className="themis-greeting" aria-label="New chat">
          <h1><span className="themis-gradient-text">Hi there.</span><br /><span className="themis-greeting-sub">What are we building in {project.name}?</span></h1>
          {composer}
          <div className="themis-starters" aria-label="Suggestions">
            {starters.map(starter => (
              <button key={starter.label} type="button" className="themis-starter" disabled={composerDisabled} onClick={() => setDraft(starter.text)}>{starter.label}</button>
            ))}
          </div>
        </section>
      ) : (
        <>
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
            compacting={compacting}
            traces={trace}
          />
          {showNotice && <CompactionNotice onDismiss={() => setNoticeDismissed(previous => ({ ...previous, [thread.id]: true }))} />}
          {composer}
        </>
      )}
    </div>
    {state.diffPanelOpen && <WorkspacePanel projectRoot={project.root} threadId={thread.id} workspace={workspace} />}
    </div>
  );
}
