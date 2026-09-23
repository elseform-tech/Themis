// Shared backend-backed UI actions (used by Sidebar, Palette, Settings…).
import { useCallback } from "react";
import {
  createThread,
  discardThread,
  getThread,
  listDiff,
  listThreads,
  mergeThread,
  openProject,
  updateSettings,
} from "../lib/tauri";
import {
  describeError,
  toast,
  trackRecentRoot,
  useApp,
} from "../state/store";
import { pickProjectDirectory } from "./projectPick";

export { trackRecentRoot };

async function loadThreadsInto(
  dispatch: ReturnType<typeof useApp>["dispatch"],
  projectRoot: string,
): Promise<void> {
  try {
    const threads = await listThreads(projectRoot);
    dispatch({ type: "thread/listed", projectRoot, threads });
  } catch (error: unknown) {
    toast(dispatch, `Failed to list threads: ${describeError(error)}`, "warning");
  }
}

export function useOpenProject(): () => Promise<void> {
  const { state, dispatch } = useApp();
  return useCallback(async () => {
    const picked = await pickProjectDirectory();
    if (picked.kind === "cancelled") return;
    if (picked.kind === "unavailable") {
      toast(dispatch, "The folder picker is unavailable. Please retry opening the desktop app.", "warning");
      return;
    }
    try {
      const project = await openProject(picked.path);
      dispatch({ type: "project/opened", project });
      toast(dispatch, `Opened ${project.name}`, "success");
      await loadThreadsInto(dispatch, project.root);
      const recent_roots = trackRecentRoot(
        state.settings.recent_roots,
        project.root,
      );
      try {
        const settings = await updateSettings({ recent_roots });
        dispatch({ type: "settings/patched", settings });
      } catch {
        // Non-fatal: the project is open; recents just won't persist.
      }
    } catch (error: unknown) {
      toast(dispatch, `Failed to open project: ${describeError(error)}`, "danger");
    }
  }, [state.settings.recent_roots, dispatch]);
}

/** Reopen a project root directly (recent-roots list, restore flow). */
export function useReopenProject(): (root: string) => Promise<boolean> {
  const { state, dispatch } = useApp();
  return useCallback(
    async (root: string) => {
      try {
        const project = await openProject(root);
        dispatch({ type: "project/opened", project });
        await loadThreadsInto(dispatch, project.root);
        const recent_roots = trackRecentRoot(
          state.settings.recent_roots,
          project.root,
        );
        try {
          const settings = await updateSettings({ recent_roots });
          dispatch({ type: "settings/patched", settings });
        } catch {
          // Non-fatal.
        }
        return true;
      } catch (error: unknown) {
        toast(
          dispatch,
          `Failed to reopen ${root}: ${describeError(error)}`,
          "danger",
        );
        return false;
      }
    },
    [state.settings.recent_roots, dispatch],
  );
}

export function useNewThread(): (projectRoot?: string | null) => Promise<void> {
  const { state, dispatch } = useApp();
  return useCallback(
    async (projectRoot?: string | null) => {
      const root = projectRoot ?? state.activeProjectRoot;
      if (root === null || root === undefined) {
        toast(dispatch, "Open a project before creating a thread", "warning");
        return;
      }
      try {
        const thread = await createThread(
          root,
          state.settings.default_provider,
          state.settings.default_model || (state.settings.default_provider === "go" ? "minimax-m2.5" : undefined),
        );
        dispatch({ type: "thread/created", projectRoot: root, thread });
      } catch (error: unknown) {
        toast(dispatch, `Failed to create thread: ${describeError(error)}`, "danger");
      }
    },
    [state.activeProjectRoot, state.settings, dispatch],
  );
}

export function useMergeThread(): {
  merge: (projectRoot: string, threadId: string) => Promise<void>;
} {
  const { dispatch } = useApp();
  const merge = useCallback(
    async (projectRoot: string, threadId: string) => {
      let result;
      try {
        result = await mergeThread(threadId);
      } catch (error: unknown) {
        toast(dispatch, `Merge failed: ${describeError(error)}`, "danger");
        return;
      }
      dispatch({ type: "thread/merged", threadId, result });
      if (result.applied) {
        dispatch({ type: "ui/diff-panel", open: false });
        const count = result.applied_files.length;
        toast(
          dispatch,
          count === 0
            ? "Merge applied (no file changes)"
            : `Merge applied: ${result.applied_files.join(", ")}`,
          "success",
        );
        try {
          const diff = await listDiff(threadId);
          dispatch({ type: "diff/set", threadId, diff });
        } catch (error: unknown) {
          toast(dispatch, `Diff refresh failed: ${describeError(error)}`, "warning");
        }
      } else {
        toast(
          dispatch,
          `Merge blocked: ${result.conflicts.length} conflict(s) — see details`,
          "warning",
        );
      }
      try {
        const thread = await getThread(threadId);
        dispatch({ type: "thread/updated", projectRoot, thread });
      } catch {
        // Non-fatal: the merge result is already recorded.
      }
    },
    [dispatch],
  );
  return { merge };
}

export function useDiscardThread(): {
  discard: (projectRoot: string, threadId: string) => Promise<boolean>;
} {
  const { dispatch } = useApp();
  const discard = useCallback(
    async (projectRoot: string, threadId: string) => {
      try {
        await discardThread(threadId);
      } catch (error: unknown) {
        toast(dispatch, `Discard failed: ${describeError(error)}`, "danger");
        return false;
      }
      dispatch({ type: "thread/removed", projectRoot, threadId });
      toast(dispatch, "Thread discarded", "success");
      return true;
    },
    [dispatch],
  );
  return { discard };
}
