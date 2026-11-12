// App shell state: useReducer + Context, plus backend subscriptions.
// This is the only state module that touches the Tauri bridge; the pure
// reducer lives in ./reducer.ts so unit tests never load Tauri APIs.
import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
  type Dispatch,
  type ReactNode,
} from "react";
import type { ToastTone } from "../components";
import { invoke } from "@tauri-apps/api/core";
import { playCompletionSound, prepareCompletionSound } from "../lib/completionSound";
import type { McpConnections, PersistedMessage, ProjectInfo, ThreadInfo } from "../lib/types";
import { getBrowserSettings,
  pluginAction,
  getSecretStatus,
  checkGoKey,
  getPendingApprovals,
  onBackendResync,
  onNotificationOpened,
  getSettings,
  getDefaultProject,
  getThread,
  getThreadHistory,
  listDiff,
  importLegacyHistory,
  listAutomations,
  getThreadTasks,
  listReviewItems,
  listSkills,
  listThreads,
  onApprovalExpired,
  onApprovalRequest,
  onReviewItemAdded,
  onSettingsChanged,
  onThreadChanged,
  onThreadEvent,
  onVisionsChanged,
  openProject,
} from "../lib/tauri";
import { getChannelPage, listChannels, listChannelThreads, listQuestions, listVisions } from "../lib/visions";
import { followVisions, visionsHoldsActive, type VisionsBridge } from "./visions";
import { failureMessage as describeError } from "../lib/errors";
import { reviewToast } from "../screens/reviewFlow";
import { readSession, writeSession } from "./session";
import { loadPhase4Lists, openProjectOfNewChat, restoreRecentProjects, STARTUP_STEPS } from "./boot";
import { settingsSync } from "./settingsSync";
import {
  appReducer,
  initialState,
  newId,
  type AppAction,
  type AppState,
} from "./reducer";

export * from "./reducer";

const visionsBridge: VisionsBridge = { listVisions, listChannels, getChannelPage: (channelId, before) => getChannelPage(channelId, before ?? null), listChannelThreads, listQuestions };
type VisionsFollower = ReturnType<typeof followVisions>;

interface AppContextValue {
  state: AppState;
  startup: string | null;
  dispatch: Dispatch<import("./reducer").AppAction>;
  armManualRun: (threadId: string) => void;
  cancelManualRun: (threadId: string) => void;
  /** Reads the page before a channel's window, for scrolling back; whether it added lines. */
  loadOlderLines: (channelId: string) => Promise<boolean>;
}

const AppContext = createContext<AppContextValue | null>(null);

export function useApp(): AppContextValue {
  const ctx = useContext(AppContext);
  if (ctx === null) throw new Error("useApp must be used inside <AppProvider>");
  return ctx;
}

/** Changes to runtime.jsonc or the stored keys announced since the window opened (0 outside
 *  the app): a Settings section lists it in the dependencies of the effect that loads it, so it
 *  reads its settings again when another client writes them. */
export function useSettingsChanges(scope: "runtime" | "secrets"): number {
  return useContext(AppContext)?.state.settingsChanges[scope] ?? 0;
}

export function useDispatch(): AppContextValue["dispatch"] {
  return useApp().dispatch;
}

/** Push a toast (convenience wrapper around the toast/push action). */
export function toast(
  dispatch: AppContextValue["dispatch"],
  message: string,
  tone: ToastTone,
): void {
  dispatch({ type: "toast/push", toast: { id: newId("toast"), message, tone } });
}

/** The plain chat to restore at the next launch; never the Vision's chat open in Visions. */
export function selectionToSave(state: AppState): { root: string; threadId: string } | null {
  const { activeProjectRoot: root, activeThreadId: threadId } = state;
  return root && threadId && !visionsHoldsActive(state) ? { root, threadId } : null;
}

/** Saves the plain chat to restore once the session was restored. */
function useSaveSelection(state: AppState, restored: { current: boolean }) {
  const selection = selectionToSave(state);
  const root = selection?.root;
  const threadId = selection?.threadId;
  useEffect(() => {
    if (restored.current && root && threadId) writeSession("selection", { root, threadId });
  }, [restored, root, threadId]);
}

export function useActiveProject(): ProjectInfo | null {
  const { state } = useApp();
  return (
    state.projects.find((p) => p.root === state.activeProjectRoot) ?? null
  );
}

export function useActiveThread(): ThreadInfo | null {
  const { state } = useApp();
  if (state.activeProjectRoot === null || state.activeThreadId === null) {
    return null;
  }
  const id = state.activeThreadId;
  // A Vision's chat is not in the plain list: it is opened from its channel.
  return (state.threadsByProject[state.activeProjectRoot] ?? []).find(t => t.id === id)
    ?? Object.values(state.visions.chats).flat().find(t => t.id === id)
    ?? null;
}

export function useThreadRunning(threadId: string | null): boolean {
  const { state } = useApp();
  if (threadId === null) return false;
  return state.running[threadId] ?? false;
}

export { describeError };

/** Chats and settings changed through the shared server (the `themis` CLI, another client)
 *  follow into this window without a reload; settings written while the connection was down
 *  are read again when it comes back. */
async function listenOutsideChanges(dispatch: Dispatch<AppAction>, isCancelled: () => boolean, isOpen: (root: string) => boolean, refreshSecrets: () => void, visions: VisionsFollower): Promise<() => void> {
  const opening = new Set<string>();
  const offThreads = await onThreadChanged((change) => {
    if (isCancelled()) return;
    dispatch({ type: "thread/changed", change });
    if (opening.has(change.project_root)) return;
    opening.add(change.project_root);
    void openProjectOfNewChat(change, { isOpen, api: { openProject, listThreads }, dispatch: action => { if (!isCancelled()) dispatch(action); } })
      .catch(error => toast(dispatch, `Could not open the new chat's project: ${describeError(error)}`, "warning"))
      .finally(() => opening.delete(change.project_root));
  });
  const follow = settingsSync({ getSettings, getBrowserSettings, refreshSecrets }, dispatch, isCancelled);
  const offSettings = await onSettingsChanged(follow);
  const offResync = await onBackendResync(() => { follow({ scope: "app" }); follow({ scope: "runtime" }); void visions.changed({}); });
  // Visions follow `visions-changed`, reading again only what it names.
  const offVisions = await onVisionsChanged(change => { if (!isCancelled()) void visions.changed(change); });
  return () => { offThreads(); offSettings(); offResync(); offVisions(); };
}

/** The latest state for callbacks, and the Visions follower that reads it: the open projects'
 *  channels are read when a project opens (and everything with the first). */
function useLiveState(state: AppState, dispatch: Dispatch<AppAction>) {
  const current = useRef(state);
  current.current = state;
  const alive = useRef(true);
  const [visions] = useState(() => followVisions(visionsBridge, () => current.current, dispatch, () => !alive.current));
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const projectRoots = state.projects.map(project => project.root).join("\n");
  useEffect(() => {
    if (projectRoots) void visions.changed({}, projectRoots.split("\n"));
  }, [projectRoots, visions]);
  return { current, visions };
}

export function AppProvider({ children }: { children: ReactNode }) {
  const [state, dispatch] = useReducer(appReducer, initialState);
  const [startup, setStartup] = useState<string | null>(STARTUP_STEPS[0]);
  const legacyMessages = useRef(readSession<Record<string, PersistedMessage[]>>("messages", {}));
  const restoredSession = useRef(false);
  const savedSelection = useRef(readSession<{ root: string; threadId: string } | null>("selection", null));
  const { current: currentState, visions } = useLiveState(state, dispatch);

  /** The activity position each chat's history was loaded at, shared by the loader on opening
   *  and the recovery loop, so a chat is reloaded only when it changed since. */
  const loadedAtRef = useRef(new Map<string, number>());
  // A chat's history loads when it is opened, not at startup: memory holds the chats in use.
  const openedThread = state.activeThreadId;
  const openedLoaded = openedThread ? Boolean(state.historyLoaded[openedThread]) : true;
  useEffect(() => {
    if (!openedThread || openedLoaded) return;
    let live = true;
    const listed = Object.values(currentState.current.threadsByProject).flat().find(thread => thread.id === openedThread);
    const seq = listed?.last_activity_seq;
    getThreadHistory(openedThread)
      .then(history => {
        if (!live) return;
        if (seq !== undefined) loadedAtRef.current.set(openedThread, seq);
        // A chat opened mid-run is still running: without saying so, loading its history
        // would mark the run interrupted and drop it from Running while it goes on.
        const running = currentState.current.running[openedThread] ?? listed?.running ?? false;
        dispatch({ type: "thread/history-loaded", threadId: openedThread, history, ...(running ? { running: true } : {}) });
      })
      .catch((error: unknown) => { if (live) toast(dispatch, `Could not load this chat: ${describeError(error)}`, "warning"); });
    return () => { live = false; };
  }, [openedThread, openedLoaded, currentState]);
  const recoverState = useRef<() => void>(() => {});
  const refreshSecretStatus = useRef<() => void>(() => {});
  useEffect(() => { if (startup === null) recoverState.current(); }, [startup]);
  const manualRuns = useRef(new Set<string>());
  const settings = useRef(state.settings);
  settings.current = state.settings;
  const armManualRun = (threadId: string) => {
    manualRuns.current.add(threadId);
    if (settings.current.completion_sound) prepareCompletionSound();
  };
  const cancelManualRun = (threadId: string) => { manualRuns.current.delete(threadId); };
  useSaveSelection(state, restoredSession);

  // A clicked goal notification opens its chat (the app already came forward).
  useEffect(() => {
    let cancelled = false;
    let off: (() => void) | undefined;
    void Promise.resolve()
      .then(() => onNotificationOpened(threadId => {
        const { threadsByProject } = currentState.current;
        const projectRoot = Object.keys(threadsByProject).find(root => threadsByProject[root].some(thread => thread.id === threadId));
        if (projectRoot) dispatch({ type: "thread/selected", projectRoot, threadId });
      }))
      .then(unlisten => { if (cancelled) unlisten(); else off = unlisten; })
      .catch(() => {});
    return () => { cancelled = true; off?.(); };
  }, [currentState]);

  // Subscribe to backend events once on mount.
  useEffect(() => {
    let cancelled = false;
    let unlistens: Array<() => void> = [];
    let eventVersion = 0;
    const metadataRequests = new Map<string, number>();
    let recovering = false;
    let recoveryRequested = false;
    let recoveryWarning = false;
    let retry: ReturnType<typeof setTimeout> | undefined;
    const loadedAt = loadedAtRef.current;
    async function recover() {
      if (cancelled) return;
      if (recovering) { recoveryRequested = true; return; }
      recovering = true;
      let pending = false;
      try {
        for (const project of currentState.current.projects) {
          const listVersion = eventVersion;
          const threads = await listThreads(project.root);
          if (cancelled) return;
          if (listVersion !== eventVersion) { pending = true; continue; }
          for (const thread of threads) {
            const version = eventVersion;
            if (thread.running) {
              dispatch({ type: "thread/synced", projectRoot: project.root, thread });
              dispatch({ type: "thread/running-loaded", threadId: thread.id, running: true });
              pending = true; continue;
            }
            // A chat not opened yet keeps only its list entry; its history loads when opened.
            if (!currentState.current.historyLoaded[thread.id]) {
              dispatch({ type: "thread/synced", projectRoot: project.root, thread });
              continue;
            }
            // While another chat keeps this loop going, an idle chat that has not changed stays as loaded.
            if (thread.last_activity_seq !== undefined && loadedAt.get(thread.id) === thread.last_activity_seq) continue;
            const history = await getThreadHistory(thread.id);
            const latest = await getThread(thread.id);
            if (cancelled) return;
            if (version !== eventVersion || latest.running) { pending = true; continue; }
            dispatch({ type: "thread/synced", projectRoot: project.root, thread: latest });
            dispatch({ type: "thread/history-loaded", threadId: thread.id, history });
            dispatch({ type: "thread/running-loaded", threadId: thread.id, running: false });
            if (latest.last_activity_seq !== undefined) loadedAt.set(thread.id, latest.last_activity_seq);
          }
        }
        const version = eventVersion;
        const requests = await getPendingApprovals();
        if (!cancelled && version === eventVersion) dispatch({ type: "approval/loaded", requests });
        else pending = true;
        if (!cancelled) await loadPhase4Lists(action => { if (!cancelled) dispatch(action); }, { listSkills, listAutomations, listReviewItems });
      } catch (error) {
        pending = true;
        if (!cancelled && !recoveryWarning) toast(dispatch, `Connection recovery failed; retrying: ${describeError(error)}`, "warning");
        recoveryWarning = true;
      } finally {
        recovering = false;
        // Active streams keep their live text; reconcile durable history when they finish.
        if ((pending || recoveryRequested) && !cancelled) retry = setTimeout(() => void recover(), 2000);
        recoveryRequested = false;
      }
    }
    (async () => {
      try {
        const offThread = await onThreadEvent((envelope) => {
          if (cancelled) return;
          eventVersion++;
          dispatch({ type: "thread/event", envelope });
          const { kind } = envelope.event;
          // A background task started, exited or was stopped: refresh this chat's task feed.
          if (kind === "background_tasks_changed") {
            const threadId = envelope.thread_id;
            void getThreadTasks(threadId).then(tasks => { if (!cancelled) dispatch({ type: "tasks/loaded", threadId, tasks }); }).catch(() => {});
          }
          // The agent can create or change automations through this tool.
          if (envelope.event.kind === "tool_finished" && envelope.event.tool === "manage_automations") {
            void listAutomations().then(automations => { if (!cancelled) dispatch({ type: "automation/loaded", automations }); }).catch(() => {});
          }
          if (kind === "started" || kind === "finished" || kind === "incomplete" || kind === "failed") {
            const version = eventVersion;
            metadataRequests.set(envelope.thread_id, version);
            const projectRoot = Object.keys(currentState.current.threadsByProject).find(root => currentState.current.threadsByProject[root].some(thread => thread.id === envelope.thread_id));
            if (projectRoot) void getThread(envelope.thread_id).then(thread => {
              if (!cancelled && thread?.id === envelope.thread_id && metadataRequests.get(envelope.thread_id) === version) dispatch({ type: "thread/synced", projectRoot, thread });
            }).catch(() => {});
          }
          if (kind === "finished" || kind === "incomplete" || kind === "failed") {
            refreshSecretStatus.current();
            // Changed files drive the thread header's review toggle.
            void listDiff(envelope.thread_id).then(diff => { if (!cancelled) dispatch({ type: "diff/set", threadId: envelope.thread_id, diff }); }).catch(() => {});
            const manual = manualRuns.current.delete(envelope.thread_id);
            if (manual && kind === "finished") {
              if (settings.current.completion_sound) playCompletionSound();
              if (settings.current.completion_haptic) void invoke("completion_haptic").catch(() => {});
            }
          }
        });
        const offApproval = await onApprovalRequest((request) => {
          if (cancelled) return;
          eventVersion++;
          dispatch({ type: "approval/enqueued", request }); void visions.approvalChanged(request.thread_id);
        });
        const offExpired = await onApprovalExpired((expired) => {
          if (cancelled) return;
          eventVersion++;
          dispatch({ type: "approval/expired", approvalId: expired.approval_id, steered: expired.steered }); void visions.approvalChanged(expired.thread_id);
        });
        const offReview = await onReviewItemAdded((item) => {
          if (cancelled) return;
          dispatch({ type: "review/added", review: item });
          toast(dispatch, reviewToast(item), "info");
          void listAutomations().then(async automations => {
            if (cancelled) return;
            dispatch({ type: "automation/loaded", automations });
            const projectRoot = automations.find(automation => automation.id === item.automation_id)?.project_root;
            if (!projectRoot) return;
            const thread = await getThread(item.thread_id);
            if (!cancelled) dispatch({ type: "thread/synced", projectRoot, thread });
          }).catch(() => {});
        });
        const offRecovery = await onBackendResync(() => { clearTimeout(retry); refreshSecretStatus.current(); window.dispatchEvent(new Event("themis-mcp-reconnect")); void recover(); });
        const offThreadChanged = await listenOutsideChanges(dispatch, () => cancelled, root => currentState.current.threadsByProject[root] !== undefined, () => refreshSecretStatus.current(), visions);
        if (cancelled) {
          offRecovery();
          offThread();
          offApproval();
          offExpired();
          offReview();
          offThreadChanged();
        } else {
          unlistens = [offThread, offApproval, offExpired, offReview, offThreadChanged, offRecovery];
          recoverState.current = () => { clearTimeout(retry); void recover(); };
          if (restoredSession.current) recoverState.current();
        }
      } catch (error: unknown) {
        if (!cancelled) {
          toast(
            dispatch,
            `Event subscription failed: ${describeError(error)}`,
            "danger",
          );
        }
      }
    })();
    return () => {
      cancelled = true;
      recoverState.current = () => {};
      clearTimeout(retry);
      for (const off of unlistens) off();
    };
  }, [visions, currentState]);

  // Restore recent projects and refresh credential presence after external changes.
  useEffect(() => {
    let cancelled = false;
    let secretLoading = false, secretAgain = false, secretWarning = false, keyCheckedAt = 0;
    async function refreshSecrets() {
      if (cancelled) return;
      if (secretLoading) { secretAgain = true; return; }
      secretLoading = true;
      do {
        secretAgain = false;
        try {
          const status = await getSecretStatus();
          if (!cancelled) dispatch({ type: "secrets/loaded", status });
          // Whether OpenCode accepts the key: at startup, then at most every ten minutes
          // (Settings › Models checks on demand).
          if (status.go && Date.now() - keyCheckedAt > 10 * 60_000) {
            keyCheckedAt = Date.now();
            void checkGoKey().then(check => { if (!cancelled) dispatch({ type: "provider/checked", state: check.state }); }).catch(() => undefined);
          }
          secretWarning = false;
        } catch (error) {
          if (!cancelled && !secretWarning) toast(dispatch, `Failed to load secret status: ${describeError(error)}`, "warning");
          secretWarning = true;
        }
      } while (secretAgain && !cancelled);
      secretLoading = false;
    }
    const refresh = () => { void refreshSecrets(); };
    const visible = () => { if (document.visibilityState === "visible") refresh(); };
    refreshSecretStatus.current = refresh;
    window.addEventListener("focus", refresh);
    document.addEventListener("visibilitychange", visible);
    refresh();
    Promise.resolve()
      .then(() => getBrowserSettings())
      .then(view => { if (!cancelled) dispatch({ type: "browser/opens-workspace", value: view.settings.open_workspace }); })
      .catch(() => { /* the default (open) stays */ });
    getSettings()
      .then(async (settings) => {
        if (cancelled) return;
        dispatch({ type: "settings/loaded", settings });
        setStartup(STARTUP_STEPS[1]);
        const defaultProject = await getDefaultProject().catch(error => {
          if (!cancelled) toast(dispatch, `Default workspace unavailable: ${describeError(error)}`, "danger");
          return null;
        });
        const roots = [...settings.recent_roots];
        if (defaultProject && !roots.includes(defaultProject.root)) roots.push(defaultProject.root);
        setStartup(STARTUP_STEPS[2]);
        const completed = await restoreRecentProjects(roots, {
          api: { openProject: root => defaultProject?.root === root ? Promise.resolve(defaultProject) : openProject(root), listThreads, getThreadHistory, importLegacyHistory },
          dispatch,
          isCancelled: () => cancelled,
          legacyMessages: legacyMessages.current,
          savedSelection: savedSelection.current,
          warn: (message) => toast(dispatch, message, "warning"),
        });
        if (completed) restoredSession.current = true;
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          toast(dispatch, `Failed to load settings: ${describeError(error)}`, "danger");
          dispatch({
            type: "settings/loaded",
            settings: state.settings,
          });
        }
      })
      .finally(() => { if (!cancelled) setStartup(null); });
    // Phase 4 lists (skills, automations, reviews) load independently of the
    // project restore flow; failures toast per-list warnings.
    void loadPhase4Lists(
      (action) => {
        if (!cancelled) dispatch(action);
      },
      { listSkills, listAutomations, listReviewItems },
    );
    return () => {
      cancelled = true;
      refreshSecretStatus.current = () => {};
      window.removeEventListener("focus", refresh);
      document.removeEventListener("visibilitychange", visible);
    };
    // state.settings is the compiled default on first mount; intentionally once.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const root = state.activeProjectRoot;
    if (!root || startup !== null) return;
    let cancelled = false, generation = 0, unavailable = false;
    let timer: ReturnType<typeof setTimeout>;
    let previous: McpConnections = {};
    dispatch({ type: "mcp/status", connections: {} });
    async function refresh(connect = false, retry = false) {
      clearTimeout(timer);
      const request = ++generation;
      try {
        const connections = await pluginAction<McpConnections>({ action: "mcp_status", projectRoot: root, connect, retry });
        if (cancelled || request !== generation) return;
        for (const [key, value] of Object.entries(connections)) {
          if (value.status === "failed" && previous[key]?.status !== "failed") toast(dispatch, `MCP ${key.split(":mcp:")[1]}: ${value.reason}`, "warning");
        }
        previous = connections;
        unavailable = false;
        dispatch({ type: "mcp/status", connections });
      } catch (error) {
        if (!cancelled && request === generation && !unavailable) toast(dispatch, `MCP status unavailable: ${describeError(error)}`, "warning");
        unavailable = true;
      } finally {
        if (!cancelled && request === generation) timer = setTimeout(() => void refresh(unavailable), 3000);
      }
    }
    const changed = () => void refresh(true);
    const reconnect = () => void refresh(true, true);
    window.addEventListener("themis-plugins-changed", changed);
    window.addEventListener("themis-mcp-reconnect", reconnect);
    void refresh(true, true);
    return () => { cancelled = true; clearTimeout(timer); window.removeEventListener("themis-plugins-changed", changed); window.removeEventListener("themis-mcp-reconnect", reconnect); };
  }, [state.activeProjectRoot, startup]);

  const value = useMemo(() => ({ state, startup, dispatch, armManualRun, cancelManualRun, loadOlderLines: visions.loadOlder }), [state, startup, visions]);
  return <AppContext.Provider value={value}>{children}</AppContext.Provider>;
}
