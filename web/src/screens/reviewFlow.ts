// Pure review-queue flows: continue (open thread) and dismiss.
// The backend API and the state snapshot are injected so unit tests can
// pass mocks; this module never imports ../lib/tauri.
import type { Automation, ReviewItem, ThreadInfo } from "../lib/types";
import { newId, type AppAction } from "../state/reducer";

export type ReviewFlowDispatch = (action: AppAction) => void;

export interface ContinueReviewApi {
  continueReviewItem(reviewId: string): Promise<ThreadInfo>;
}

export interface DismissReviewApi {
  dismissReviewItem(reviewId: string): Promise<ReviewItem>;
}

/** Minimal state snapshot the flows need. */
export interface ReviewFlowState {
  threadsByProject: Record<string, ThreadInfo[]>;
  automations: Automation[];
}

/** Find the project that currently holds a thread, if any. */
export function findThreadProject(
  threadsByProject: Record<string, ThreadInfo[]>,
  threadId: string,
): string | null {
  for (const [root, threads] of Object.entries(threadsByProject)) {
    if (threads.some((t) => t.id === threadId)) return root;
  }
  return null;
}

export function automationName(
  automations: Automation[],
  automationId: string,
): string {
  return (
    automations.find((a) => a.id === automationId)?.name ?? "Unknown automation"
  );
}

function failureMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}

function pushToast(
  dispatch: ReviewFlowDispatch,
  message: string,
  tone: "success" | "warning" | "danger",
): void {
  dispatch({
    type: "toast/push",
    toast: { id: newId("toast"), message, tone },
  });
}

/**
 * Continue a review item: the backend returns the thread (already marked
 * continued there, so we record the status locally), we upsert the thread
 * and select it. The project root comes from the live thread list, falling
 * back to the owning automation's root; when neither knows it, the review
 * is still marked continued but no thread can be selected.
 */
export async function continueReview(
  api: ContinueReviewApi,
  dispatch: ReviewFlowDispatch,
  state: ReviewFlowState,
  review: ReviewItem,
): Promise<
  | { ok: true; projectRoot: string | null; thread: ThreadInfo }
  | { ok: false; error: string }
> {
  let thread: ThreadInfo;
  try {
    thread = await api.continueReviewItem(review.id);
  } catch (error: unknown) {
    const detail = failureMessage(error);
    pushToast(dispatch, `Open thread failed: ${detail}`, "danger");
    return { ok: false, error: detail };
  }
  dispatch({ type: "review/updated", review: { ...review, status: "continued" } });
  const knownRoot = findThreadProject(state.threadsByProject, thread.id);
  const projectRoot =
    knownRoot ??
    state.automations.find((a) => a.id === review.automation_id)
      ?.project_root ??
    null;
  if (projectRoot === null) {
    pushToast(
      dispatch,
      "Review continued, but the thread's project is not open — reopen it to view the thread.",
      "warning",
    );
    return { ok: true, projectRoot: null, thread };
  }
  if (knownRoot === null) {
    dispatch({ type: "thread/created", projectRoot, thread });
  } else {
    dispatch({ type: "thread/updated", projectRoot, thread });
  }
  dispatch({ type: "thread/selected", projectRoot, threadId: thread.id });
  pushToast(dispatch, `Opened thread ${thread.title}`, "success");
  return { ok: true, projectRoot, thread };
}

/** Dismiss a review item, recording the backend-returned item. */
export async function dismissReview(
  api: DismissReviewApi,
  dispatch: ReviewFlowDispatch,
  reviewId: string,
): Promise<{ ok: true; review: ReviewItem } | { ok: false; error: string }> {
  try {
    const review = await api.dismissReviewItem(reviewId);
    dispatch({ type: "review/updated", review });
    pushToast(dispatch, "Review dismissed", "success");
    return { ok: true, review };
  } catch (error: unknown) {
    const detail = failureMessage(error);
    pushToast(dispatch, `Dismiss failed: ${detail}`, "danger");
    return { ok: false, error: detail };
  }
}
