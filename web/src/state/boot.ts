// Phase 4 boot loading: skills, automations, and review items.
// Takes the backend API as a parameter so unit tests can inject mocks —
// this module never imports ../lib/tauri (same reason as ./reducer.ts).
import type {
  Automation,
  HistoryItem,
  PersistedMessage,
  ProjectInfo,
  ReviewItem,
  Skill,
  ThreadInfo,
} from "../lib/types";
import { failureMessage } from "../lib/errors";
import { newId, type AppAction } from "./reducer";

export interface Phase4ListsApi {
  listSkills(): Promise<Skill[]>;
  listAutomations(): Promise<Automation[]>;
  listReviewItems(): Promise<ReviewItem[]>;
}

export type Phase4Dispatch = (action: AppAction) => void;

export interface RecentProjectApi {
  openProject(root: string): Promise<ProjectInfo>;
  listThreads(root: string): Promise<ThreadInfo[]>;
  getThreadHistory(threadId: string): Promise<HistoryItem[]>;
  importLegacyHistory(threadId: string, messages: PersistedMessage[]): Promise<void>;
}

interface RecentProjectRestore {
  api: RecentProjectApi;
  dispatch: Phase4Dispatch;
  isCancelled: () => boolean;
  legacyMessages: Record<string, PersistedMessage[]>;
  savedSelection: { root: string; threadId: string } | null;
  warn: (message: string) => void;
}

function warn(dispatch: Phase4Dispatch, message: string): void {
  dispatch({
    type: "toast/push",
    toast: { id: newId("toast"), message, tone: "warning" },
  });
}

/**
 * Load all three Phase 4 lists. Each list is independent: a failure toasts
 * a warning and leaves that slice empty while the others still load.
 */
export async function loadPhase4Lists(
  dispatch: Phase4Dispatch,
  api: Phase4ListsApi,
): Promise<void> {
  const skills = await api.listSkills().catch((error: unknown) => {
    warn(dispatch, `Failed to load skills: ${failureMessage(error)}`);
    return null;
  });
  if (skills !== null) dispatch({ type: "skill/loaded", skills });

  const automations = await api.listAutomations().catch((error: unknown) => {
    warn(dispatch, `Failed to load automations: ${failureMessage(error)}`);
    return null;
  });
  if (automations !== null) {
    dispatch({ type: "automation/loaded", automations });
  }

  const reviews = await api.listReviewItems().catch((error: unknown) => {
    warn(dispatch, `Failed to load review items: ${failureMessage(error)}`);
    return null;
  });
  if (reviews !== null) dispatch({ type: "review/loaded", reviews });
}

async function restoreThreadHistory(
  thread: ThreadInfo,
  context: RecentProjectRestore,
): Promise<void> {
  try {
    let history = await context.api.getThreadHistory(thread.id);
    const legacy = context.legacyMessages[thread.id];
    if (history.length === 0 && legacy?.length) {
      await context.api.importLegacyHistory(thread.id, legacy);
      history = await context.api.getThreadHistory(thread.id);
    }
    if (!context.isCancelled()) {
      context.dispatch({ type: "thread/history-loaded", threadId: thread.id, history });
    }
  } catch (error: unknown) {
    if (!context.isCancelled()) {
      context.warn(`Could not restore ${thread.title}: ${failureMessage(error)}`);
    }
  }
}

async function restoreProject(
  root: string,
  context: RecentProjectRestore,
): Promise<string | null> {
  let project: ProjectInfo;
  try {
    project = await context.api.openProject(root);
  } catch (error: unknown) {
    if (!context.isCancelled()) {
      context.warn(`Could not reopen ${root}: ${failureMessage(error)}`);
    }
    return null;
  }
  if (context.isCancelled()) return null;
  context.dispatch({ type: "project/opened", project });

  try {
    const threads = await context.api.listThreads(root);
    if (!context.isCancelled()) {
      context.dispatch({ type: "thread/listed", projectRoot: root, threads });
      await Promise.all(threads.map((thread) => restoreThreadHistory(thread, context)));
    }
  } catch (error: unknown) {
    if (!context.isCancelled()) {
      context.warn(`Could not list threads for ${root}: ${failureMessage(error)}`);
    }
  }
  return project.root;
}

/** Reopen recent projects, restore their histories, then reapply saved selection. */
export async function restoreRecentProjects(
  recentRoots: string[],
  context: RecentProjectRestore,
): Promise<boolean> {
  const restored: string[] = [];
  for (const root of recentRoots) {
    if (context.isCancelled()) return false;
    const restoredRoot = await restoreProject(root, context);
    if (restoredRoot !== null) restored.push(restoredRoot);
  }

  const first = restored[0];
  if (!context.isCancelled() && first !== undefined) {
    const selected = context.savedSelection;
    const root = selected && restored.includes(selected.root) ? selected.root : first;
    context.dispatch({ type: "project/selected", root });
    if (selected?.root === root) {
      try {
        const threads = await context.api.listThreads(root);
        if (!context.isCancelled() && threads.some((thread) => thread.id === selected.threadId)) {
          context.dispatch({ type: "thread/selected", projectRoot: root, threadId: selected.threadId });
        }
      } catch (error: unknown) {
        if (!context.isCancelled()) {
          context.warn(`Could not list threads for ${root}: ${failureMessage(error)}`);
        }
      }
    }
  }
  return true;
}
