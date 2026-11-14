// @vitest-environment jsdom
// The channel's thinking rows and arrivals: dots that rise and fall, a row kept at least a moment,
// shown from the person's post that woke a Vision until it answers or its run ends, and new
// posts arriving one after another. Reduced motion: plain text, no wait, no stagger.
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialState, type AppState } from "../../state/reducer";
import type { ChannelEntry, ChannelInfo, ChannelLine, VisionInfo } from "../../lib/visions";
import * as visionsBridge from "../../lib/visions";
import { ChannelView } from "./ChannelView";

const live = vi.hoisted(() => ({ value: null as unknown }));
vi.mock("../../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../../state/store")>(),
  useApp: () => live.value,
}));
vi.mock("../../lib/visions", async importOriginal => ({
  ...await importOriginal<typeof import("../../lib/visions")>(),
  postChannelMessage: vi.fn(async () => ({ seq: 1, woken: ["rook"] })),
  listQuestions: vi.fn(async () => []),
}));

const channel: ChannelInfo = { id: "c1", name: "release", topic: "", project_root: "/p", approval_mode: "custom", members: ["rook"], revision: "r" };
const rook = (running: boolean): VisionInfo => ({
  id: "rook", name: "Rook", role: "", shape: "flat", color: "teal", soul: "", paused: false, revision: "r",
  over_budget: false, needs: 0, sorry: false, vision_chats: running ? [{ thread_id: "own", channel_id: "c1", running: true }] : [],
});
const line = (seq: number, entry: ChannelEntry): ChannelLine => ({ seq, at_ms: 0, entry });
const asked = line(1, { type: "person", text: "@Rook check the build" });
const said = (seq: number, text: string) => line(seq, { type: "vision", vision_id: "rook", text });

function state(running: boolean, lines: ChannelLine[]): AppState {
  return { ...initialState, visions: { ...initialState.visions, list: [rook(running)], channels: { "/p": [channel] }, lines: { c1: lines }, nextBefore: { c1: null }, chats: { c1: [] } } };
}

function show(running: boolean, lines: ChannelLine[]) {
  live.value = { state: state(running, lines), dispatch: vi.fn(), loadOlderLines: vi.fn(async () => false) };
  return <ChannelView channel={channel} />;
}

const row = () => screen.queryByRole("status");
const reduceMotion = (on: boolean) => { window.matchMedia = vi.fn().mockReturnValue({ matches: on, addEventListener: vi.fn(), removeEventListener: vi.fn() }); };

beforeEach(() => {
  vi.useFakeTimers();
  reduceMotion(false);
  HTMLElement.prototype.scrollIntoView = vi.fn();
});
afterEach(() => { vi.useRealTimers(); vi.clearAllMocks(); });

describe("thinking in a channel", () => {
  it("shows_its_name_and_three_rising_dots_with_the_thinking_cloud", () => {
    render(show(true, [asked]));
    expect(row()).toHaveTextContent(/^Rook is thinking$/);
    expect(row()!.querySelectorAll(".themis-thinking-dots > i")).toHaveLength(3);
    expect(row()!.querySelector(".themis-cloud")).toHaveClass("themis-cloud--think");
    expect(row()!.querySelector(".themis-shim")).toBeNull();
  });

  it("keeps_the_row_a_moment_after_a_fast_reply", () => {
    const { rerender } = render(show(true, [asked]));
    rerender(show(false, [asked, said(2, "Green.")]));
    expect(row()).toHaveTextContent("Rook is thinking");
    act(() => { vi.advanceTimersByTime(1000); });
    expect(row()).toBeNull();
  });

  it("with_reduced_motion_it_is_plain_text_and_goes_at_once", () => {
    reduceMotion(true);
    const { rerender } = render(show(true, [asked]));
    expect(row()).toHaveTextContent(/^Rook is thinking…$/);
    expect(row()!.querySelector(".themis-thinking-dots")).toBeNull();
    rerender(show(false, [asked, said(2, "Green.")]));
    expect(row()).toBeNull();
  });

  it("shows_from_the_post_that_woke_it_before_its_run_starts_until_it_answers", async () => {
    const { rerender } = render(show(false, []));
    fireEvent.change(screen.getByLabelText("Message #release"), { target: { value: "@Rook check the build" } });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Send" })); });
    expect(visionsBridge.postChannelMessage).toHaveBeenCalled();
    rerender(show(false, [asked]));
    expect(row()).toHaveTextContent("Rook is thinking");
    rerender(show(true, [asked]));
    rerender(show(true, [asked, said(2, "On it.")]));
    // Its run goes on after its post: still thinking.
    act(() => { vi.advanceTimersByTime(5000); });
    expect(row()).toHaveTextContent("Rook is thinking");
    rerender(show(false, [asked, said(2, "On it."), said(3, "Green.")]));
    act(() => { vi.advanceTimersByTime(1000); });
    expect(row()).toBeNull();
  });

  it("a_wake_whose_run_never_starts_clears_and_an_ended_run_clears", async () => {
    const { rerender } = render(show(false, []));
    fireEvent.change(screen.getByLabelText("Message #release"), { target: { value: "@Rook hi" } });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Send" })); });
    rerender(show(false, [asked]));
    expect(row()).toHaveTextContent("Rook is thinking");
    // Refused or never started: it does not stay on.
    act(() => { vi.advanceTimersByTime(21_000); });
    expect(row()).toBeNull();
    // Woken again, its run starts and ends without a post (a failure): the row goes.
    fireEvent.change(screen.getByLabelText("Message #release"), { target: { value: "@Rook again" } });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Send" })); });
    rerender(show(true, [asked]));
    rerender(show(false, [asked]));
    act(() => { vi.advanceTimersByTime(1000); });
    expect(row()).toBeNull();
  });

  it("a_post_that_woke_nobody_shows_no_row", async () => {
    vi.mocked(visionsBridge.postChannelMessage).mockResolvedValueOnce({ seq: 1, woken: [] });
    render(show(false, []));
    fireEvent.change(screen.getByLabelText("Message #release"), { target: { value: "hello" } });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Send" })); });
    expect(row()).toBeNull();
  });
});

describe("arrivals in a channel", () => {
  const texts = () => screen.queryAllByText(/^Post \d$/).map(each => each.textContent);

  it("new_posts_arrive_one_after_another", () => {
    const { rerender } = render(show(false, [asked]));
    rerender(show(false, [asked, said(2, "Post 1"), said(3, "Post 2"), said(4, "Post 3")]));
    expect(texts()).toEqual(["Post 1"]);
    expect(screen.getByText("Post 1").closest("li")).toHaveClass("is-fresh");
    act(() => { vi.advanceTimersByTime(250); });
    expect(texts()).toEqual(["Post 1", "Post 2"]);
    act(() => { vi.advanceTimersByTime(250); });
    expect(texts()).toEqual(["Post 1", "Post 2", "Post 3"]);
  });

  it("lines_there_when_it_opens_and_older_lines_appear_at_once", () => {
    const { rerender } = render(show(false, [said(5, "Post 5"), said(6, "Post 6")]));
    expect(screen.queryAllByText(/^Post \d$/)).toHaveLength(2);
    rerender(show(false, [said(3, "Post 3"), said(4, "Post 4"), said(5, "Post 5"), said(6, "Post 6")]));
    expect(screen.queryAllByText(/^Post \d$/)).toHaveLength(4);
    expect(within(screen.getByRole("region", { name: "# release" })).getByText("Post 3").closest("li")).not.toHaveClass("is-fresh");
  });

  it("with_reduced_motion_new_posts_arrive_together_without_fading", () => {
    reduceMotion(true);
    const { rerender } = render(show(false, [asked]));
    rerender(show(false, [asked, said(2, "Post 1"), said(3, "Post 2")]));
    expect(texts()).toEqual(["Post 1", "Post 2"]);
    expect(screen.getByText("Post 1").closest("li")).not.toHaveClass("is-fresh");
  });
});
