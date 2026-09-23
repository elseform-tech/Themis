// Skill form helper tests: parsing, validation, input building, and the
// save flow with a mocked backend API. No DOM, no Tauri imports.
import { describe, expect, it, beforeEach } from "vitest";
import type { Skill, SkillInput } from "../lib/types";
import { initialState, reducer, resetIds, type AppAction } from "../state/reducer";
import {
  emptySkillForm,
  parseAllowedTools,
  saveSkill,
  skillToForm,
  toSkillInput,
  validateSkillForm,
  type SkillSaveApi,
} from "./skillForm";

const SKILL: Skill = {
  id: "s1",
  name: "review",
  description: "d",
  instructions: "i",
  allowed_tools: ["read_file", "exec"],
  scripts: [{ name: "h.sh", content: "echo hi" }],
};

const INPUT: SkillInput = {
  name: "review",
  description: "d",
  instructions: "i",
  allowed_tools: [],
  scripts: [],
};

beforeEach(() => {
  resetIds();
});

describe("parseAllowedTools", () => {
  it("splits, trims, and drops blanks", () => {
    expect(parseAllowedTools("")).toEqual([]);
    expect(parseAllowedTools("  ")).toEqual([]);
    expect(parseAllowedTools("read_file, exec , ,write_file")).toEqual([
      "read_file",
      "exec",
      "write_file",
    ]);
  });
});

describe("skill form round-trip", () => {
  it("empty form has blank fields", () => {
    expect(emptySkillForm()).toEqual({
      name: "",
      description: "",
      instructions: "",
      allowedToolsRaw: "",
      scripts: [],
    });
  });

  it("skillToForm joins the allowlist", () => {
    const form = skillToForm(SKILL);
    expect(form.name).toBe("review");
    expect(form.allowedToolsRaw).toBe("read_file, exec");
    expect(form.scripts).toEqual([{ name: "h.sh", content: "echo hi" }]);
  });

  it("toSkillInput drops fully-blank scripts and trims names", () => {
    const input = toSkillInput({
      ...emptySkillForm(),
      name: "  review  ",
      scripts: [
        { name: "", content: "" },
        { name: "  h.sh ", content: "echo" },
      ],
    });
    expect(input.name).toBe("review");
    expect(input.scripts).toEqual([{ name: "h.sh", content: "echo" }]);
  });
});

describe("validateSkillForm", () => {
  it("requires a name", () => {
    expect(validateSkillForm(emptySkillForm())).toBe("Name is required.");
  });

  it("requires names for non-empty scripts", () => {
    expect(
      validateSkillForm({
        ...emptySkillForm(),
        name: "x",
        scripts: [{ name: "", content: "echo" }],
      }),
    ).toBe("Script #1 needs a name.");
  });

  it("accepts a valid form", () => {
    expect(validateSkillForm(skillToForm(SKILL))).toBeNull();
  });
});

describe("saveSkill flow", () => {
  function apiWith(overrides: Partial<SkillSaveApi>): SkillSaveApi {
    return {
      createSkill: (input) =>
        Promise.resolve({ ...SKILL, ...input, id: "s-new" }),
      updateSkill: (id, input) => Promise.resolve({ ...SKILL, ...input, id }),
      ...overrides,
    };
  }

  it("create dispatches skill/added + success toast", async () => {
    const actions: AppAction[] = [];
    const result = await saveSkill(
      apiWith({}),
      (a) => {
        actions.push(a);
      },
      null,
      INPUT,
    );
    expect(result.ok).toBe(true);
    const state = actions.reduce(reducer, initialState);
    expect(state.skills.map((s) => s.id)).toEqual(["s-new"]);
    expect(state.toasts).toHaveLength(1);
    expect(state.toasts[0]?.tone).toBe("success");
  });

  it("update dispatches skill/updated + success toast", async () => {
    const actions: AppAction[] = [];
    const seeded = reducer(initialState, { type: "skill/loaded", skills: [SKILL] });
    const result = await saveSkill(
      apiWith({}),
      (a) => {
        actions.push(a);
      },
      "s1",
      { ...INPUT, name: "renamed" },
    );
    expect(result.ok).toBe(true);
    const state = actions.reduce(reducer, seeded);
    expect(state.skills[0]?.name).toBe("renamed");
    expect(state.toasts[0]?.tone).toBe("success");
  });

  it("unknown-tool rejection returns the error verbatim + danger toast", async () => {
    const backendError = "unknown tool(s): frobnicate, wibble (valid: read_file, exec)";
    const actions: AppAction[] = [];
    const result = await saveSkill(
      apiWith({
        createSkill: () => Promise.reject(new Error(backendError)),
      }),
      (a) => {
        actions.push(a);
      },
      null,
      INPUT,
    );
    expect(result).toEqual({ ok: false, error: backendError });
    const state = actions.reduce(reducer, initialState);
    // Nothing was recorded in the skills slice…
    expect(state.skills).toEqual([]);
    // …but the failure is surfaced as a toast naming the backend detail.
    expect(state.toasts).toHaveLength(1);
    expect(state.toasts[0]?.tone).toBe("danger");
    expect(state.toasts[0]?.message).toContain(backendError);
  });

  it("string rejections are surfaced verbatim too", async () => {
    const actions: AppAction[] = [];
    const result = await saveSkill(
      apiWith({ createSkill: () => Promise.reject("raw failure") }),
      (a) => {
        actions.push(a);
      },
      null,
      INPUT,
    );
    expect(result).toEqual({ ok: false, error: "raw failure" });
  });
});
