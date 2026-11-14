// @vitest-environment jsdom
// The scene runtime with a fake WebGL renderer (real three.js objects otherwise): the loop really
// stops, everything it made is freed, a failed setup releases the renderer, the canvas size comes
// from the options, and the geometry and blink helpers.
import * as THREE from "three";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PRESETS } from "../cast";

const renderer = vi.hoisted(() => ({ instances: [] as Array<Record<string, ReturnType<typeof vi.fn>>> }));
const failBuild = vi.hoisted(() => ({ on: false }));
vi.mock("three", async importOriginal => {
  const actual = await importOriginal<typeof import("three")>();
  class FakeRenderer {
    setPixelRatio = vi.fn(); setSize = vi.fn(); render = vi.fn(); dispose = vi.fn(); forceContextLoss = vi.fn();
    constructor() { renderer.instances.push(this as unknown as Record<string, ReturnType<typeof vi.fn>>); }
  }
  return { ...actual, WebGLRenderer: FakeRenderer };
});
vi.mock("./buildCharacter", async importOriginal => {
  const actual = await importOriginal<typeof import("./buildCharacter")>();
  return { ...actual, buildCharacter: (...args: Parameters<typeof actual.buildCharacter>) => { if (failBuild.on) throw new Error("build failed"); return actual.buildCharacter(...args); } };
});
import { frontZ } from "./buildCharacter";
import { blinking } from "./faces";
import { startScene } from "./sceneRuntime";

const options = { look: PRESETS[0]!.look, expression: "calm" as const, size: 90, onLost: vi.fn() };
let frames: Array<(now: number) => void>;
const cancel = vi.fn();

beforeEach(() => {
  renderer.instances.length = 0; failBuild.on = false; frames = []; cancel.mockClear();
  const context = new Proxy({}, { get: (_target, key) => (key === "createRadialGradient" ? () => ({ addColorStop() {} }) : () => {}), set: () => true });
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(context as unknown as CanvasRenderingContext2D);
  vi.stubGlobal("Path2D", class {});
  vi.stubGlobal("requestAnimationFrame", (callback: (now: number) => void) => frames.push(callback));
  vi.stubGlobal("cancelAnimationFrame", cancel);
});
afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });

describe("the scene runtime", () => {
  it("the_canvas_size_comes_from_the_options_not_the_element", () => {
    const canvas = document.createElement("canvas");
    Object.defineProperty(canvas, "clientWidth", { value: 0 });
    startScene(canvas, options);
    expect(renderer.instances[0]!.setSize).toHaveBeenCalledWith(90, 90, false);
  });

  it("setRunning_false_cancels_the_animation_frame_and_true_starts_it_once", () => {
    const handle = startScene(document.createElement("canvas"), options);
    handle.setRunning(true); handle.setRunning(true);
    expect(frames).toHaveLength(1);
    handle.setRunning(false);
    expect(cancel).toHaveBeenCalledTimes(1);
    handle.setRunning(false);
    expect(cancel).toHaveBeenCalledTimes(1);
  });

  it("dispose_stops_the_loop_and_frees_geometries_materials_textures_and_the_renderer", () => {
    const geometries = vi.spyOn(THREE.BufferGeometry.prototype, "dispose");
    const materials = vi.spyOn(THREE.Material.prototype, "dispose");
    const textures = vi.spyOn(THREE.Texture.prototype, "dispose");
    const handle = startScene(document.createElement("canvas"), options);
    handle.setRunning(true);
    textures.mockClear();
    handle.dispose();
    expect(cancel).toHaveBeenCalled();
    expect(geometries.mock.calls.length).toBeGreaterThanOrEqual(5);
    expect(materials).toHaveBeenCalled();
    expect(textures.mock.calls.length).toBeGreaterThanOrEqual(2); // the face and the shadow
    expect(renderer.instances[0]!.dispose).toHaveBeenCalled();
    expect(renderer.instances[0]!.forceContextLoss).toHaveBeenCalled();
  });

  it("a_setup_that_throws_releases_the_renderer", () => {
    failBuild.on = true;
    expect(() => startScene(document.createElement("canvas"), options)).toThrow("build failed");
    expect(renderer.instances[0]!.dispose).toHaveBeenCalled();
    expect(renderer.instances[0]!.forceContextLoss).toHaveBeenCalled();
  });
});

describe("scene helpers", () => {
  it("frontZ_is_the_sphere_height_over_the_body_and_zero_outside_it", () => {
    const bumps: Array<[number, number, number]> = [[24, 16, 10]];
    expect(frontZ(bumps, 24, 16)).toBeCloseTo(10, 5);
    expect(frontZ(bumps, 24 + 6, 16 + 8)).toBeCloseTo(0, 5); // on the rim
    expect(frontZ(bumps, 24, 32)).toBeCloseTo(8, 5); // the base capsule's front
    expect(frontZ(bumps, 2, 2)).toBe(0);
    expect(frontZ(bumps, 24, 22)).toBeCloseTo(8, 5); // sqrt(100 - 36)
  });

  it("only_calm_and_done_faces_blink_and_only_briefly", () => {
    expect(blinking("calm", 4.8)).toBe(true);
    expect(blinking("done", 4.8)).toBe(true);
    expect(blinking("calm", 1)).toBe(false);
    expect(blinking("calm", 5 + 4.8)).toBe(true);
    for (const expression of ["working", "needs", "stuck", "paused"] as const) expect(blinking(expression, 4.8)).toBe(false);
  });
});
