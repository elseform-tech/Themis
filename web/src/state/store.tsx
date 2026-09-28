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
  type Dispatch,
  type ReactNode,
} from "react";
import type { ToastTone } from "../components";
import type { PersistedMessage, ProjectInfo, ThreadInfo } from "../lib/types";
import {
  getSecretStatus,
  getSettings,
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
  dispatch: Dispatch<import("./reducer").AppAction>;
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
  const legacyMessages = useRef(readSession<Record<string, PersistedMessage[]>>("messages", {}));
  const restoredSession = useRef(false);
  const savedSelection = useRef(readSession<{ root: string; threadId: string } | null>("selection", null));
  useEffect(() => {
    if (restoredSession.current && state.activeProjectRoot && state.activeThreadId) writeSession("selection", { root: state.activeProjectRoot, threadId: state.activeThreadId });
  }, [state.activeProjectRoot, state.activeThreadId]);

  // Subscribe to backend events once on mount.
  useEffect(() => {
    let cancelled = false;
    let unlistens: Array<() => void> = [];
    (async () => {
      try {
        const offThread = await onThreadEvent((envelope) => {
          if (cancelled) return;
          dispatch({ type: "thread/event", envelope });
        });
        const offApproval = await onApprovalRequest((request) => {
          if (cancelled) return;
          dispatch({ type: "approval/enqueued", request });
        });
        const offReview = await onReviewItemAdded((item) => {
          if (cancelled) return;
          dispatch({ type: "review/added", review: item });
          toast(dispatch, `New review item: ${item.title}`, "info");
        });
        if (cancelled) {
          offThread();
          offApproval();
          offReview();
        } else {
          unlistens = [offThread, offApproval, offReview];
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
      for (const off of unlistens) off();
    };
  }, []);

  // Load settings + secret status once on mount, then restore recent
  // projects and their threads.
  useEffect(() => {
    let cancelled = false;
    getSettings()
      .then((settings) => {
        if (cancelled) return;
        dispatch({ type: "settings/loaded", settings });
        void restoreRecentProjects(settings.recent_roots, {
          api: { openProject, listThreads, getThreadHistory, importLegacyHistory },
          dispatch,
          isCancelled: () => cancelled,
          legacyMessages: legacyMessages.current,
          savedSelection: savedSelection.current,
          warn: (message) => toast(dispatch, message, "warning"),
        }).then((completed) => {
          if (completed) restoredSession.current = true;
        });
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          toast(dispatch, `Failed to load settings: ${describeError(error)}`, "danger");
          dispatch({
            type: "settings/loaded",
            settings: state.settings,
          });
        }
      });
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

  const value = useMemo(() => ({ state, dispatch }), [state]);
  return <AppContext.Provider value={value}>{children}</AppContext.Provider>;
}
