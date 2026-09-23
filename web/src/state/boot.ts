// Phase 4 boot loading: skills, automations, and review items.
// Takes the backend API as a parameter so unit tests can inject mocks —
// this module never imports ../lib/tauri (same reason as ./reducer.ts).
import type { Automation, ReviewItem, Skill } from "../lib/types";
import { newId, type AppAction } from "./reducer";

export interface Phase4ListsApi {
  listSkills(): Promise<Skill[]>;
  listAutomations(): Promise<Automation[]>;
  listReviewItems(): Promise<ReviewItem[]>;
}

export type Phase4Dispatch = (action: AppAction) => void;

function failureMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
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
