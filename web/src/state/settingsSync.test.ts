import { describe, expect, it, vi } from "vitest";
import { DEFAULT_SETTINGS, initialState, reducer } from "./reducer";
import { settingsSync } from "./settingsSync";
import type { Settings } from "../lib/types";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}

describe("settings/changed", () => {
  it("counts runtime.jsonc and key changes separately", () => {
    let state = reducer(initialState, { type: "settings/changed", scope: "runtime" });
    state = reducer(state, { type: "settings/changed", scope: "runtime" });
    state = reducer(state, { type: "settings/changed", scope: "secrets" });
    expect(state.settingsChanges).toEqual({ runtime: 2, secrets: 1 });
    expect(state.settings).toBe(initialState.settings);
  });
});

describe("settingsSync", () => {
  const api = () => ({
    getSettings: vi.fn<() => Promise<Settings>>(),
    getBrowserSettings: vi.fn(async () => ({ settings: { open_workspace: false } })),
    refreshSecrets: vi.fn(),
  });

  it("reads settings.json again on an app change and keeps only the latest read", async () => {
    const reads = api();
    const dispatch = vi.fn();
    const first = deferred<Settings>();
    const second = deferred<Settings>();
    reads.getSettings.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    const follow = settingsSync(reads, dispatch, () => false);
    follow({ scope: "app" });
    follow({ scope: "app" });
    const latest = { ...DEFAULT_SETTINGS, default_approval_mode: "custom" as const, approval_timeout_seconds: 30 };
    second.resolve(latest);
    first.resolve({ ...DEFAULT_SETTINGS, approval_timeout_seconds: 600 });
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(dispatch.mock.calls).toEqual([[{ type: "settings/loaded", settings: latest }]]);
  });

  it("bumps the runtime counter and the browser's Workspace choice on a runtime change", async () => {
    const reads = api();
    const dispatch = vi.fn();
    settingsSync(reads, dispatch, () => false)({ scope: "runtime" });
    await vi.waitFor(() => expect(dispatch).toHaveBeenCalledWith({ type: "browser/opens-workspace", value: false }));
    expect(dispatch).toHaveBeenCalledWith({ type: "settings/changed", scope: "runtime" });
    expect(reads.getSettings).not.toHaveBeenCalled();
  });

  it("refreshes the key status on a secrets change and does nothing once cancelled", () => {
    const reads = api();
    const dispatch = vi.fn();
    settingsSync(reads, dispatch, () => false)({ scope: "secrets" });
    expect(dispatch).toHaveBeenCalledWith({ type: "settings/changed", scope: "secrets" });
    expect(reads.refreshSecrets).toHaveBeenCalledTimes(1);
    const quiet = vi.fn();
    settingsSync(reads, quiet, () => true)({ scope: "app" });
    expect(quiet).not.toHaveBeenCalled();
    expect(reads.getSettings).not.toHaveBeenCalled();
  });

  it("keeps what is shown when the read fails", async () => {
    const reads = api();
    const dispatch = vi.fn();
    reads.getSettings.mockRejectedValueOnce(new Error("server gone"));
    settingsSync(reads, dispatch, () => false)({ scope: "app" });
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(dispatch).not.toHaveBeenCalled();
  });
});
