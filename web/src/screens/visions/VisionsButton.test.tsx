// @vitest-environment jsdom
import { act, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_SETTINGS, initialState, type AppState } from "../../state/reducer";
import type { VisionInfo } from "../../lib/visions";
import { VisionsButton } from "./VisionsButton";

const dispatch = vi.fn();
let state: AppState = initialState;
vi.mock("../../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../../state/store")>(),
  useApp: () => ({ state, dispatch }),
}));

const vision = (id: string, extra: Partial<VisionInfo> = {}): VisionInfo => ({
  id, name: id, role: "", shape: "tall", color: "blue", soul: "", paused: false, revision: "r",
  over_budget: false, vision_chats: [], needs: 0, sorry: false, ...extra,
});
const withVisions = (list: VisionInfo[]): AppState => ({ ...initialState, settings: DEFAULT_SETTINGS, visions: { ...initialState.visions, list } });

beforeEach(() => { dispatch.mockClear(); state = withVisions([]); });

describe("VisionsButton", () => {
  it("the_cloud_says_what_needs_you", () => {
    state = withVisions([vision("a", { needs: 1 }), vision("b", { needs: 1 }), vision("c", { sorry: true })]);
    const { rerender } = render(<VisionsButton />);
    const button = screen.getByRole("button", { name: "Visions, 2 need you" });
    expect(button).toHaveTextContent("2");
    expect(button.querySelector(".themis-rail-cloud")).toHaveClass("themis-rail-cloud--needs");

    state = withVisions([vision("a", { needs: 1 })]);
    rerender(<VisionsButton />);
    expect(screen.getByRole("button", { name: "Visions, 1 needs you" })).toBeTruthy();

    // Sorry outranks the rest once nothing waits; no badge without needs.
    state = withVisions([vision("a", { sorry: true }), vision("b")]);
    rerender(<VisionsButton />);
    expect(screen.getByRole("button", { name: "Visions, a chat failed" })).toHaveTextContent(/^$/);

    state = withVisions([vision("a", { paused: true }), vision("b", { over_budget: true })]);
    rerender(<VisionsButton />);
    expect(screen.getByRole("button", { name: "Visions, all asleep" })).toBeTruthy();

    state = withVisions([vision("a")]);
    rerender(<VisionsButton />);
    expect(screen.getByRole("button", { name: "Visions" })).toBeTruthy();
  });

  it("hops once for each new thing that needs you", () => {
    state = withVisions([vision("a")]);
    const { rerender } = render(<VisionsButton />);
    const cloud = () => screen.getByRole("button").querySelector(".themis-rail-cloud")!;
    expect(cloud()).not.toHaveClass("is-hopping");
    state = withVisions([vision("a", { needs: 1 })]);
    rerender(<VisionsButton />);
    const first = cloud();
    expect(first).toHaveClass("is-hopping");
    // The same count again does not hop again; one more does (a fresh element replays it).
    rerender(<VisionsButton />);
    expect(cloud()).toBe(first);
    state = withVisions([vision("a", { needs: 2 })]);
    rerender(<VisionsButton />);
    expect(cloud()).not.toBe(first);
    expect(cloud()).toHaveClass("is-hopping");
  });

  it("opens Visions", () => {
    render(<VisionsButton />);
    act(() => { screen.getByRole("button").click(); });
    expect(dispatch).toHaveBeenCalledWith({ type: "ui/view", view: "visions" });
  });
});
