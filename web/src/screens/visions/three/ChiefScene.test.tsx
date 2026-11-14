// @vitest-environment jsdom
// The 3D chief: the scene code (three.js) is mocked here; these tests pin when it runs and when
// the still picture shows instead (reduced motion, no WebGL, a lost context) and that it only
// renders while on screen.
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PRESETS } from "../cast";
import { ChiefScene, type SceneEnv } from "./ChiefScene";
import { motionOffset } from "./faces";

const handle = vi.hoisted(() => ({ setRunning: vi.fn(), setLook: vi.fn(), setExpression: vi.fn(), dispose: vi.fn() }));
const started = vi.hoisted(() => ({ calls: 0, onLost: null as null | (() => void), fail: false }));
vi.mock("./sceneRuntime", () => ({
  startScene: vi.fn((_canvas: HTMLCanvasElement, options: { onLost: () => void }) => {
    started.calls += 1; started.onLost = options.onLost;
    if (started.fail) throw new Error("no context");
    return handle;
  }),
}));

const look = PRESETS[0]!.look;
const capable: SceneEnv = { reducedMotion: () => false, hasWebGL: () => true };
let observer: { callback: (entries: Array<{ isIntersecting: boolean }>) => void; disconnect: ReturnType<typeof vi.fn> };

beforeEach(() => {
  vi.clearAllMocks(); started.calls = 0; started.onLost = null; started.fail = false;
  vi.stubGlobal("IntersectionObserver", class {
    disconnect = vi.fn();
    constructor(callback: (entries: Array<{ isIntersecting: boolean }>) => void) { observer = { callback, disconnect: this.disconnect }; }
    observe() {}
  });
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

const hide = (hidden: boolean) => {
  Object.defineProperty(document, "visibilityState", { configurable: true, value: hidden ? "hidden" : "visible" });
  document.dispatchEvent(new Event("visibilitychange"));
};

describe("the 3D chief", () => {
  it("no_webgl_falls_back_to_the_still_picture", () => {
    render(<ChiefScene look={look} expression="calm" env={{ ...capable, hasWebGL: () => false }} />);
    expect(screen.getByRole("img").getAttribute("data-expression")).toBe("calm");
    expect(document.querySelector("canvas")).toBeNull();
    expect(started.calls).toBe(0);
  });

  it("reduced_motion_keeps_the_still_picture", () => {
    render(<ChiefScene look={look} expression="working" env={{ ...capable, reducedMotion: () => true }} />);
    expect(screen.getByRole("img").getAttribute("data-expression")).toBe("working");
    expect(document.querySelector("canvas")).toBeNull();
    expect(started.calls).toBe(0);
  });

  it("the_scene_starts_for_a_capable_browser_and_replaces_the_still_picture_once_ready", async () => {
    render(<ChiefScene look={look} expression="calm" env={capable} />);
    await waitFor(() => expect(started.calls).toBe(1));
    await waitFor(() => expect(document.querySelector("svg.themis-still")).toBeNull());
    expect(document.querySelector("canvas")).not.toBeNull();
  });

  it("the_scene_stops_when_hidden", async () => {
    render(<ChiefScene look={look} expression="calm" env={capable} />);
    await waitFor(() => expect(started.calls).toBe(1));
    act(() => observer.callback([{ isIntersecting: true }]));
    expect(handle.setRunning).toHaveBeenLastCalledWith(true);
    act(() => observer.callback([{ isIntersecting: false }]));
    expect(handle.setRunning).toHaveBeenLastCalledWith(false);
    act(() => observer.callback([{ isIntersecting: true }]));
    expect(handle.setRunning).toHaveBeenLastCalledWith(true);
    act(() => hide(true));
    expect(handle.setRunning).toHaveBeenLastCalledWith(false);
    act(() => hide(false));
    expect(handle.setRunning).toHaveBeenLastCalledWith(true);
  });

  it("a_lost_context_or_a_failed_start_shows_the_still_picture", async () => {
    const lost = render(<ChiefScene look={look} expression="needs" env={capable} />);
    await waitFor(() => expect(started.calls).toBe(1));
    act(() => started.onLost?.());
    expect(screen.getByRole("img").getAttribute("data-expression")).toBe("needs");
    expect(handle.dispose).toHaveBeenCalled();
    lost.unmount();
    started.fail = true;
    render(<ChiefScene look={look} expression="needs" env={capable} />);
    await waitFor(() => expect(screen.getByRole("img")).toBeTruthy());
    expect(document.querySelector("canvas")).toBeNull();
  });

  it("a_new_expression_and_look_reach_the_running_scene_and_unmount_disposes_it", async () => {
    const view = render(<ChiefScene look={look} expression="calm" env={capable} />);
    await waitFor(() => expect(started.calls).toBe(1));
    view.rerender(<ChiefScene look={PRESETS[1]!.look} expression="done" env={capable} />);
    expect(handle.setExpression).toHaveBeenLastCalledWith("done");
    expect(handle.setLook).toHaveBeenLastCalledWith(PRESETS[1]!.look);
    view.unmount();
    expect(handle.dispose).toHaveBeenCalled();
    expect(observer.disconnect).toHaveBeenCalled();
  });
});

describe("the face motion", () => {
  it("each_expression_moves_as_the_board_describes_and_a_still_face_does_not", () => {
    expect(motionOffset("calm", 0)).toEqual({ scale: 1, dx: 0, dy: 0 });
    expect(motionOffset("calm", 4.5 / 2).scale).toBeCloseTo(1.04, 5);
    expect(motionOffset("working", 1.3 / 2).dy).toBeCloseTo(1.2, 5);
    expect(motionOffset("needs", 3 * 0.78).dy).toBeCloseTo(3, 1);
    expect(motionOffset("stuck", 2.6 * 0.4).dx).toBeCloseTo(0.4, 5);
    expect(motionOffset("paused", 7 / 2).scale).toBeCloseTo(1.04, 5);
  });
});
