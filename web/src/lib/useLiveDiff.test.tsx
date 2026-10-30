// @vitest-environment jsdom
import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DiffState } from "./types";
import { LIVE_DIFF_IDLE_MS, LIVE_DIFF_RUNNING_MS, useLiveDiff } from "./useLiveDiff";

const diff = (paths: string[]) => ({ available: true, files: paths.map(path => ({ path })) }) as unknown as DiffState;
const setHidden = (hidden: boolean) => Object.defineProperty(document, "hidden", { configurable: true, value: hidden });
const advance = (ms: number) => act(() => vi.advanceTimersByTimeAsync(ms));

describe("useLiveDiff", () => {
  beforeEach(() => { vi.useFakeTimers(); setHidden(false); });
  afterEach(() => { cleanup(); vi.useRealTimers(); setHidden(false); });

  it("picks up changes made between runs, such as in the person's own editor", async () => {
    let files = ["a.txt"];
    const load = vi.fn(async () => diff(files));
    const apply = vi.fn();
    renderHook(() => useLiveDiff("t", true, false, load, apply));
    await advance(0);
    expect(apply).toHaveBeenCalledTimes(1);
    files = ["a.txt", "b.txt"];
    await advance(LIVE_DIFF_IDLE_MS);
    expect(apply).toHaveBeenLastCalledWith("t", diff(["a.txt", "b.txt"]));
  });

  it("reads faster during a run, and reports only a diff that changed", async () => {
    const load = vi.fn(async () => diff(["a.txt"]));
    const apply = vi.fn();
    renderHook(() => useLiveDiff("t", true, true, load, apply));
    await advance(0);
    await advance(LIVE_DIFF_RUNNING_MS * 3);
    expect(load).toHaveBeenCalledTimes(4);
    expect(apply).toHaveBeenCalledTimes(1);
  });

  it("pauses while the app is hidden and catches up when it comes back", async () => {
    let files = ["a.txt"];
    const load = vi.fn(async () => diff(files));
    const apply = vi.fn();
    renderHook(() => useLiveDiff("t", true, false, load, apply));
    await advance(0);
    setHidden(true);
    load.mockClear();
    await advance(LIVE_DIFF_IDLE_MS * 5);
    expect(load).not.toHaveBeenCalled();
    files = ["b.txt"];
    setHidden(false);
    await act(async () => { window.dispatchEvent(new Event("focus")); await vi.advanceTimersByTimeAsync(0); });
    expect(apply).toHaveBeenLastCalledWith("t", diff(["b.txt"]));
  });

  it("never overlaps reads, keeps the last diff on failure and stops when closed", async () => {
    let finish!: (value: DiffState) => void;
    const load = vi.fn(() => new Promise<DiffState>(resolve => { finish = resolve; }));
    const apply = vi.fn();
    const view = renderHook(() => useLiveDiff("t", true, true, load, apply));
    await advance(LIVE_DIFF_RUNNING_MS * 4);
    expect(load).toHaveBeenCalledTimes(1);
    await act(async () => { finish(diff(["a.txt"])); await vi.advanceTimersByTimeAsync(0); });
    load.mockRejectedValueOnce(new Error("git busy"));
    await advance(LIVE_DIFF_RUNNING_MS);
    expect(apply).toHaveBeenCalledTimes(1);
    view.unmount();
    load.mockClear();
    await advance(LIVE_DIFF_RUNNING_MS * 4);
    expect(load).not.toHaveBeenCalled();
  });

  it("does nothing for a chat without a Git diff", async () => {
    const load = vi.fn(async () => diff([]));
    renderHook(() => useLiveDiff("t", false, true, load, vi.fn()));
    await advance(LIVE_DIFF_RUNNING_MS * 2);
    expect(load).not.toHaveBeenCalled();
  });
});
