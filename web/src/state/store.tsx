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
import {
  pluginAction,
  getSecretStatus,
  getPendingApprovals,
  onBackendResync,
  getSettings,
  getDefaultProject,
  getThread,
  getThreadHistory,
  importLegacyHistory,
  listAutomations,
  listReviewItems,
  listSkills,
  listThreads,
  onApprovalRequest,
  onReviewItemAdded,
  onThreadEvent,
  openProject,
} from "../lib/tauri";
import { failureMessage as describeError } from "../lib/errors";
import { readSession, writeSession } from "./session";
import { loadPhase4Lists, restoreRecentProjects } from "./boot";
import {
  initialState,
  newId,
  reducer,
  type AppState,
} from "./reducer";

export * from "./reducer";

interface AppContextValue {
  state: AppState;
  startup: string | null;
  dispatch: Dispatch<import("./reducer").AppAction>;
  armManualRun: (threadId: string) => void;
  cancelManualRun: (threadId: string) => void;
}

const AppContext = createContext<AppContextValue | null>(null);

export function useApp(): AppContextValue {
  const ctx = useContext(AppContext);
  if (ctx === null) throw new Error("useApp must be used inside <AppProvider>");
  return ctx;
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
  return (
    (state.threadsByProject[state.activeProjectRoot] ?? []).find(
      (t) => t.id === state.activeThreadId,
    ) ?? null
  );
}

export function useThreadRunning(threadId: string | null): boolean {
  const { state } = useApp();
  if (threadId === null) return false;
  return state.running[threadId] ?? false;
}

export { describeError };

export function AppProvider({ children }: { children: ReactNode }) {
  const [state, dispatch] = useReducer(reducer, initialState);
  const [startup, setStartup] = useState<string | null>("Loading preferences…");
  const legacyMessages = useRef(readSession<Record<string, PersistedMessage[]>>("messages", {}));
  const restoredSession = useRef(false);
  const savedSelection = useRef(readSession<{ root: string; threadId: string } | null>("selection", null));
  const currentState = useRef(state);
  currentState.current = state;
  const recoverState = useRef<() => void>(() => {});
  useEffect(() => { if (startup === null) recoverState.current(); }, [startup]);
  const manualRuns = useRef(new Set<string>());
  const settings = useRef(state.settings);
  settings.current = state.settings;
  const armManualRun = (threadId: string) => {
    manualRuns.current.add(threadId);
    if (settings.current.completion_sound) prepareCompletionSound();
  };
  const cancelManualRun = (threadId: string) => { manualRuns.current.delete(threadId); };
  useEffect(() => {
    if (restoredSession.current && state.activeProjectRoot && state.activeThreadId) writeSession("selection", { root: state.activeProjectRoot, threadId: state.activeThreadId });
  }, [state.activeProjectRoot, state.activeThreadId]);

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
            const history = await getThreadHistory(thread.id);
            const latest = await getThread(thread.id);
            if (cancelled) return;
            if (version !== eventVersion || latest.running) { pending = true; continue; }
            dispatch({ type: "thread/synced", projectRoot: project.root, thread: latest });
            dispatch({ type: "thread/history-loaded", threadId: thread.id, history });
            dispatch({ type: "thread/running-loaded", threadId: thread.id, running: false });
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
          if (kind === "started" || kind === "finished" || kind === "incomplete" || kind === "failed") {
            const version = eventVersion;
            metadataRequests.set(envelope.thread_id, version);
            const projectRoot = Object.keys(currentState.current.threadsByProject).find(root => currentState.current.threadsByProject[root].some(thread => thread.id === envelope.thread_id));
            if (projectRoot) void getThread(envelope.thread_id).then(thread => {
              if (!cancelled && thread?.id === envelope.thread_id && metadataRequests.get(envelope.thread_id) === version) dispatch({ type: "thread/synced", projectRoot, thread });
            }).catch(() => {});
          }
          if (kind === "finished" || kind === "incomplete" || kind === "failed") {
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
          dispatch({ type: "approval/enqueued", request });
        });
        const offReview = await onReviewItemAdded((item) => {
          if (cancelled) return;
          dispatch({ type: "review/added", review: item });
          toast(dispatch, `New review item: ${item.title}`, "info");
          void listAutomations().then(async automations => {
            if (cancelled) return;
            dispatch({ type: "automation/loaded", automations });
            const projectRoot = automations.find(automation => automation.id === item.automation_id)?.project_root;
            if (!projectRoot) return;
            const thread = await getThread(item.thread_id);
            if (!cancelled) dispatch({ type: "thread/synced", projectRoot, thread });
          }).catch(() => {});
        });
        const offRecovery = await onBackendResync(() => { clearTimeout(retry); window.dispatchEvent(new Event("themis-mcp-reconnect")); void recover(); });
        if (cancelled) {
          offRecovery();
          offThread();
          offApproval();
          offReview();
        } else {
          unlistens = [offThread, offApproval, offReview, offRecovery];
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
  }, []);

  // Load settings + secret status once on mount, then restore recent
  // projects and their threads.
  useEffect(() => {
    let cancelled = false;
    getSettings()
      .then(async (settings) => {
        if (cancelled) return;
        dispatch({ type: "settings/loaded", settings });
        setStartup("Opening workspace…");
        const defaultProject = await getDefaultProject().catch(error => {
          if (!cancelled) toast(dispatch, `Default workspace unavailable: ${describeError(error)}`, "danger");
          return null;
        });
        const roots = [...settings.recent_roots];
        if (defaultProject && !roots.includes(defaultProject.root)) roots.push(defaultProject.root);
        setStartup("Restoring conversations…");
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
    getSecretStatus()
      .then((status) => {
        if (!cancelled) dispatch({ type: "secrets/loaded", status });
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          toast(
            dispatch,
            `Failed to load secret status: ${describeError(error)}`,
            "warning",
          );
        }
      });
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

  const value = useMemo(() => ({ state, startup, dispatch, armManualRun, cancelManualRun }), [state, startup]);
  return <AppContext.Provider value={value}>{children}</AppContext.Provider>;
}
