import { promptParts, skillToken } from "../lib/prompt";
// Pure automation-form helpers: parsing, validation, and the save flow.
// The backend API is injected so unit tests can pass mocks; this module
// never imports ../lib/tauri.
import type {
  Automation,
  AutomationInput,
  ProviderKind,
  AutomationSchedule,
} from "../lib/types";
import { failureMessage } from "../lib/errors";
import { newId, type AppAction } from "../state/reducer";

export interface AutomationFormState {
  name: string;
  targetMode: "continue" | "new";
  targetThreadId: string;
  projectRoot: string;
  provider: ProviderKind;
  model: string;
  effort: string;
  skillIds: string[];
  /** Raw interval input; parsed to minutes on save. */
  intervalMinsRaw: string;
  repeat: AutomationSchedule["repeat"] | "interval";
  time: string;
  timezone: string;
  weekday: number;
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
    targetMode: "continue",
    targetThreadId: "",
    projectRoot: "",
    provider: defaultProvider,
    model: "",
    effort: "",
    skillIds: [],
    intervalMinsRaw: "60",
    repeat: "daily",
    time: "09:00",
    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || "Europe/Berlin",
    weekday: 0,
    task: "",
    enabled: true,
  };
}

export function automationToForm(
  automation: Automation,
): AutomationFormState {
  return {
    name: automation.name,
    targetMode: automation.target_thread_id ? "continue" : "new",
    targetThreadId: automation.target_thread_id ?? "",
    projectRoot: automation.project_root,
    provider: "go",
    model: automation.model,
    effort: automation.reasoning_effort ?? "",
    skillIds: [...automation.skill_ids],
    intervalMinsRaw: String(automation.interval_mins),
    repeat: automation.schedule?.repeat ?? "interval",
    time: automation.schedule?.time ?? "09:00",
    timezone: automation.schedule?.timezone ?? (Intl.DateTimeFormat().resolvedOptions().timeZone || "Europe/Berlin"),
    weekday: automation.schedule?.weekday ?? 0,
    task: automation.task + automation.skill_ids.filter(id => !automation.task.includes(skillToken(id))).map(id => ` ${skillToken(id)}`).join(""),
    enabled: automation.enabled,
  };
}

/** Client-side validation; null when the form may be submitted. */
export function validateAutomationForm(
  form: AutomationFormState,
): string | null {
  if (form.targetMode === "continue" && !form.targetThreadId) return "Choose a thread to continue.";
  if (form.targetMode === "new" && form.projectRoot.trim() === "") return "Choose a project for new threads.";
  if (form.repeat === "interval") {
    const mins = Number(form.intervalMinsRaw);
    if (!Number.isSafeInteger(mins) || mins < 1) return "Interval must be at least 1 minute.";
  } else {
    if (!/^([01]\d|2[0-3]):[0-5]\d$/.test(form.time)) return "Choose a time in HH:MM format.";
    try { new Intl.DateTimeFormat("en", { timeZone: form.timezone }); }
    catch { return "Choose a valid time zone."; }
    if (form.repeat === "weekly" && (!Number.isInteger(form.weekday) || form.weekday < 0 || form.weekday > 6)) return "Choose a weekday.";
  }
  if (form.task.trim() === "") return "Instructions are required.";
  return null;
}

export function toAutomationInput(
  form: AutomationFormState,
): AutomationInput {
  const inferredName = promptParts(form.task)
    .map(part => "text" in part ? part.text : "")
    .join("").trim().split("\n")[0].slice(0, 60);
  return {
    name: form.name.trim() || inferredName || "Scheduled task",
    project_root: form.projectRoot.trim(),
    target_thread_id: form.targetMode === "continue" ? form.targetThreadId : null,
    provider: form.provider,
    model: form.targetMode === "new" ? form.model.trim() : "",
    reasoning_effort: form.targetMode === "new" ? form.effort || null : null,
    skill_ids: [],
    interval_mins: form.repeat === "interval" ? Number(form.intervalMinsRaw) : 60,
    schedule: form.repeat === "interval" ? null : { repeat: form.repeat, time: form.time, timezone: form.timezone, weekday: form.weekday },
    task: form.task,
    enabled: form.enabled,
  };
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
