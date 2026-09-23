// Pure skill-form helpers: parsing, validation, and the save flow.
// The backend API is injected so unit tests can pass mocks; this module
// never imports ../lib/tauri.
import type { Skill, SkillInput, SkillScript } from "../lib/types";
import { newId, type AppAction } from "../state/reducer";

export interface SkillFormState {
  name: string;
  description: string;
  instructions: string;
  /** Raw comma-separated tool allowlist. */
  allowedToolsRaw: string;
  scripts: SkillScript[];
}

export type SkillFormDispatch = (action: AppAction) => void;

export interface SkillSaveApi {
  createSkill(input: SkillInput): Promise<Skill>;
  updateSkill(skillId: string, input: SkillInput): Promise<Skill>;
}

export function emptySkillForm(): SkillFormState {
  return {
    name: "",
    description: "",
    instructions: "",
    allowedToolsRaw: "",
    scripts: [],
  };
}

export function skillToForm(skill: Skill): SkillFormState {
  return {
    name: skill.name,
    description: skill.description,
    instructions: skill.instructions,
    allowedToolsRaw: skill.allowed_tools.join(", "),
    scripts: skill.scripts.map((s) => ({ ...s })),
  };
}

/** Split a comma-separated allowlist, trimming and dropping blanks. */
export function parseAllowedTools(raw: string): string[] {
  return raw
    .split(",")
    .map((t) => t.trim())
    .filter((t) => t !== "");
}

/** Client-side validation; null when the form may be submitted. */
export function validateSkillForm(form: SkillFormState): string | null {
  if (form.name.trim() === "") return "Name is required.";
  for (let i = 0; i < form.scripts.length; i++) {
    const script = form.scripts[i];
    if (script === undefined) continue;
    if (script.name.trim() === "" && script.content.trim() !== "") {
      return `Script #${i + 1} needs a name.`;
    }
  }
  return null;
}

export function toSkillInput(form: SkillFormState): SkillInput {
  return {
    name: form.name.trim(),
    description: form.description.trim(),
    instructions: form.instructions,
    allowed_tools: parseAllowedTools(form.allowedToolsRaw),
    scripts: form.scripts
      .filter((s) => s.name.trim() !== "" || s.content.trim() !== "")
      .map((s) => ({ name: s.name.trim(), content: s.content })),
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
  dispatch: SkillFormDispatch,
  message: string,
  tone: "success" | "danger",
): void {
  dispatch({
    type: "toast/push",
    toast: { id: newId("toast"), message, tone },
  });
}

/**
 * Persist a skill via create or update. On success the store is updated and
 * a success toast is pushed; on failure a danger toast is pushed and the
 * backend error is returned VERBATIM so the dialog can display it
 * (especially unknown-tool validation errors).
 */
export async function saveSkill(
  api: SkillSaveApi,
  dispatch: SkillFormDispatch,
  editingId: string | null,
  input: SkillInput,
): Promise<{ ok: true; skill: Skill } | { ok: false; error: string }> {
  try {
    if (editingId === null) {
      const skill = await api.createSkill(input);
      dispatch({ type: "skill/added", skill });
      pushToast(dispatch, `Created skill ${skill.name}`, "success");
      return { ok: true, skill };
    }
    const skill = await api.updateSkill(editingId, input);
    dispatch({ type: "skill/updated", skill });
    pushToast(dispatch, `Updated skill ${skill.name}`, "success");
    return { ok: true, skill };
  } catch (error: unknown) {
    const detail = failureMessage(error);
    pushToast(dispatch, `Save failed: ${detail}`, "danger");
    return { ok: false, error: detail };
  }
}
