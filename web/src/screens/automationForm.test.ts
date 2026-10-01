// Automation form helper tests: validation, input building, and the
// save flow with a mocked backend API. No DOM, no Tauri imports.
import { describe, expect, it, beforeEach } from "vitest";
import type { Automation, AutomationInput } from "../lib/types";
import { initialState, reducer, resetIds, type AppAction } from "../state/reducer";
import {
  automationToForm,
  emptyAutomationForm,
  saveAutomation,
  toAutomationInput,
  validateAutomationForm,
  type AutomationSaveApi,
} from "./automationForm";

const AUTOMATION: Automation = {
  id: "a1",
  name: "nightly",
  project_root: "/repo",
  provider: "go",
  model: "gpt",
  skill_ids: ["s1"],
  interval_mins: 30,
  task: "triage",
  enabled: true,
  last_run_at: null,
  next_run_at: "2026-01-01T00:00:00Z",
  run_count: 0,
};

const INPUT: AutomationInput = {
  name: "nightly",
  project_root: "/repo",
  provider: "go",
  model: "",
  skill_ids: [],
  interval_mins: 60,
  task: "triage",
  enabled: true,
};

beforeEach(() => {
  resetIds();
});

describe("validateAutomationForm", () => {
  it("requires name, project root, and task", () => {
    expect(validateAutomationForm(emptyAutomationForm("go"))).toBe(
      "Name is required.",
    );
    expect(
      validateAutomationForm({
        ...emptyAutomationForm("go"),
        name: "x",
      }),
    ).toBe("Choose a thread to continue.");
    expect(
      validateAutomationForm({
        ...emptyAutomationForm("go"),
        name: "x",
        targetMode: "new",
      }),
    ).toBe("Choose a project for new threads.");
    expect(
      validateAutomationForm({
        ...emptyAutomationForm("go"),
        name: "x",
        targetMode: "new",
        projectRoot: "/repo",
      }),
    ).toBe("Task is required.");
  });

  it("rejects intervals below 1 minute", () => {
    for (const raw of ["0", "-5", "abc", ""]) {
      expect(
        validateAutomationForm({
          ...automationToForm(AUTOMATION),
          intervalMinsRaw: raw,
        }),
      ).toBe("Interval must be at least 1 minute.");
    }
  });

  it("accepts a round-tripped automation", () => {
    expect(validateAutomationForm(automationToForm(AUTOMATION))).toBeNull();
  });
});

describe("toAutomationInput", () => {
  it("trims text fields and parses the interval", () => {
    const input = toAutomationInput({
      ...automationToForm(AUTOMATION),
      name: "  nightly  ",
      intervalMinsRaw: "15",
    });
    expect(input.name).toBe("nightly");
    expect(input.interval_mins).toBe(15);
    expect(input.skill_ids).toEqual([]);
  });
  it("round-trips the selected reasoning effort", () => {
    const form = automationToForm({ ...AUTOMATION, reasoning_effort: "medium" });
    expect(form.effort).toBe("medium");
    expect(toAutomationInput(form).reasoning_effort).toBe("medium");
    expect(toAutomationInput({ ...form, effort: "" }).reasoning_effort).toBeNull();
  });
  it("continues an existing thread without freezing its model or effort", () => {
    const form = automationToForm({ ...AUTOMATION, target_thread_id: "thread-1", reasoning_effort: "high" });
    expect(form.targetMode).toBe("continue");
    expect(toAutomationInput(form)).toMatchObject({ target_thread_id: "thread-1", model: "", reasoning_effort: null, skill_ids: [] });
  });
});

describe("saveAutomation flow", () => {
  function apiWith(overrides: Partial<AutomationSaveApi>): AutomationSaveApi {
    return {
      createAutomation: (input) =>
        Promise.resolve({ ...AUTOMATION, ...input, id: "a-new" }),
      updateAutomation: (id, input) =>
        Promise.resolve({ ...AUTOMATION, ...input, id }),
      ...overrides,
    };
  }

  it("create dispatches automation/added + success toast", async () => {
    const actions: AppAction[] = [];
    const result = await saveAutomation(
      apiWith({}),
      (a) => {
        actions.push(a);
      },
      null,
      INPUT,
    );
    expect(result.ok).toBe(true);
    const state = actions.reduce(reducer, initialState);
    expect(state.automations.map((a) => a.id)).toEqual(["a-new"]);
    expect(state.toasts[0]?.tone).toBe("success");
  });

  it("update dispatches automation/updated + success toast", async () => {
    const actions: AppAction[] = [];
    const seeded = reducer(initialState, {
      type: "automation/loaded",
      automations: [AUTOMATION],
    });
    const result = await saveAutomation(
      apiWith({}),
      (a) => {
        actions.push(a);
      },
      "a1",
      { ...INPUT, interval_mins: 5 },
    );
    expect(result.ok).toBe(true);
    const state = actions.reduce(reducer, seeded);
    expect(state.automations[0]?.interval_mins).toBe(5);
    expect(state.toasts[0]?.tone).toBe("success");
  });

  it("backend rejection returns the error verbatim + danger toast", async () => {
    const backendError = "unknown skill id(s): s-ghost";
    const actions: AppAction[] = [];
    const result = await saveAutomation(
      apiWith({
        createAutomation: () => Promise.reject(new Error(backendError)),
      }),
      (a) => {
        actions.push(a);
      },
      null,
      INPUT,
    );
    expect(result).toEqual({ ok: false, error: backendError });
    const state = actions.reduce(reducer, initialState);
    expect(state.automations).toEqual([]);
    expect(state.toasts).toHaveLength(1);
    expect(state.toasts[0]?.tone).toBe("danger");
    expect(state.toasts[0]?.message).toContain(backendError);
  });
});
