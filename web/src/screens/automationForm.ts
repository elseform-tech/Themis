// Pure automation-form helpers: parsing, validation, and the save flow.
// The backend API is injected so unit tests can pass mocks; this module
// never imports ../lib/tauri.
import type {
  Automation,
  AutomationInput,
  ProviderKind,
} from "../lib/types";
import { newId, type AppAction } from "../state/reducer";

export interface AutomationFormState {
  name: string;
  projectRoot: string;
  provider: ProviderKind;
  model: string;
  skillIds: string[];
  /** Raw interval input; parsed to minutes on save. */
  intervalMinsRaw: string;
  task: string;
  enabled: boolean;
}

export type AutomationFormDispatch = (action: AppAction) => void;

export interface AutomationSaveApi {
  createAutomation(input: AutomationInput): Promise<Automation>;
  updateAutomation(
    automationId: string,
    input: AutomationInput,
  ): Promise<Automation>;
}

export function emptyAutomationForm(
  defaultProvider: ProviderKind,
): AutomationFormState {
  return {
    name: "",
    projectRoot: "",
    provider: defaultProvider,
    model: "",
    skillIds: [],
    intervalMinsRaw: "60",
    task: "",
    enabled: true,
  };
}

export function automationToForm(
  automation: Automation,
): AutomationFormState {
  return {
    name: automation.name,
    projectRoot: automation.project_root,
    provider: automation.provider,
    model: automation.model,
    skillIds: [...automation.skill_ids],
    intervalMinsRaw: String(automation.interval_mins),
    task: automation.task,
    enabled: automation.enabled,
  };
}

/** Client-side validation; null when the form may be submitted. */
export function validateAutomationForm(
  form: AutomationFormState,
): string | null {
  if (form.name.trim() === "") return "Name is required.";
  if (form.projectRoot.trim() === "") return "Project root is required.";
  const mins = Number.parseInt(form.intervalMinsRaw, 10);
  if (!Number.isFinite(mins) || mins < 1) {
    return "Interval must be at least 1 minute.";
  }
  if (form.task.trim() === "") return "Task is required.";
  return null;
}

export function toAutomationInput(
  form: AutomationFormState,
): AutomationInput {
  return {
    name: form.name.trim(),
    project_root: form.projectRoot.trim(),
    provider: form.provider,
    model: form.model.trim(),
    skill_ids: [...form.skillIds],
    interval_mins: Math.max(1, Math.floor(Number(form.intervalMinsRaw))),
    task: form.task,
    enabled: form.enabled,
  };
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
  dispatch: AutomationFormDispatch,
  message: string,
  tone: "success" | "danger",
): void {
  dispatch({
    type: "toast/push",
    toast: { id: newId("toast"), message, tone },
  });
}

/**
 * Persist an automation via create or update. On failure the backend error
 * is returned VERBATIM for dialog display, in addition to a danger toast.
 */
export async function saveAutomation(
  api: AutomationSaveApi,
  dispatch: AutomationFormDispatch,
  editingId: string | null,
  input: AutomationInput,
): Promise<{ ok: true; automation: Automation } | { ok: false; error: string }> {
  try {
    if (editingId === null) {
      const automation = await api.createAutomation(input);
      dispatch({ type: "automation/added", automation });
      pushToast(dispatch, `Created automation ${automation.name}`, "success");
      return { ok: true, automation };
    }
    const automation = await api.updateAutomation(editingId, input);
    dispatch({ type: "automation/updated", automation });
    pushToast(dispatch, `Updated automation ${automation.name}`, "success");
    return { ok: true, automation };
  } catch (error: unknown) {
    const detail = failureMessage(error);
    pushToast(dispatch, `Save failed: ${detail}`, "danger");
    return { ok: false, error: detail };
  }
}
