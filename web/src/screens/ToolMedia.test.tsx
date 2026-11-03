// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AttachedMedia, ToolMedia } from "../lib/types";
import { initialState, reducer, type ChatMessage } from "../state/reducer";
import { MediaGallery } from "./MediaStrip";
import { GoalTurnDivider, ToolGroup } from "./ThreadConversation";

vi.mock("../lib/tauri", () => ({ attachmentFile: vi.fn(async (_thread: string, path: string) => `asset://${path}`) }));

const shotPath = "/p/.themis/attachments/u1/screenshot-1652-03.png";
const rawOutput = JSON.stringify({ height: 800, image_attached: true, note: "The image follows.", path: shotPath, width: 1280 });
const shot: ToolMedia = { path: shotPath, kind: "image", width: 1280, height: 800 };

function toolRow(media?: ToolMedia, name = "browser_screenshot", output = rawOutput): ChatMessage {
  return { id: "m1", role: "assistant", text: "call `browser_screenshot` with {}", runId: "r1", tool: { name, ok: true, output, ...(media ? { media } : {}) } };
}

function renderGroup(message: ChatMessage, show = vi.fn<(file: AttachedMedia) => void>()) {
  const view = render(
    <MediaGallery.Provider value={{ threadId: "t1", items: [], open: vi.fn(), show }}>
      <ToolGroup messages={[message]} activeRunId={undefined} skills={[]} plugins={[]} />
    </MediaGallery.Provider>,
  );
  return { ...view, show };
}

afterEach(() => cleanup());

describe("a screenshot step's output", () => {
  it("is a small thumbnail with its size, not the raw result", async () => {
    let container!: HTMLElement;
    await act(async () => { ({ container } = renderGroup(toolRow(shot))); });
    const detail = container.querySelector(".themis-tool-detail")!;
    const thumb = screen.getByRole("button", { name: "Open screenshot" });
    expect(thumb.querySelector("img")).toHaveAttribute("src", `asset://${shotPath}`);
    expect(detail).toHaveTextContent("1280 × 800");
    // The note and the JSON are for the model.
    expect(detail).not.toHaveTextContent("image_attached");
    expect(detail).not.toHaveTextContent("The image follows.");
    expect(detail.querySelectorAll("pre")).toHaveLength(1);
  });

  it("opens the media viewer on that screenshot alone", async () => {
    let show!: ReturnType<typeof vi.fn>;
    await act(async () => { ({ show } = renderGroup(toolRow(shot))); });
    fireEvent.click(screen.getByRole("button", { name: "Open screenshot" }));
    expect(show).toHaveBeenCalledWith({ path: shotPath, name: "screenshot-1652-03.png", kind: "image", caption: "Screenshot" });
  });

  it("shows a recording as a video thumbnail with a play mark", async () => {
    const video: ToolMedia = { path: "/p/.themis/attachments/u2/recording-1.mp4", kind: "video" };
    let container!: HTMLElement;
    let show!: ReturnType<typeof vi.fn>;
    await act(async () => { ({ container, show } = renderGroup(toolRow(video, "browser_record", JSON.stringify({ path: video.path, seconds: 4 })))); });
    const thumb = screen.getByRole("button", { name: "Play recording" });
    expect(thumb.querySelector("video")?.getAttribute("src")).toMatch(/recording-1\.mp4#t=0\.1$/);
    expect(thumb.querySelector(".themis-media-play")).not.toBeNull();
    expect(container.querySelector(".themis-tool-detail")).not.toHaveTextContent("seconds");
    fireEvent.click(thumb);
    expect(show).toHaveBeenCalledWith(expect.objectContaining({ kind: "video", caption: "Recording" }));
  });

  it("keeps the raw output for a step without media", async () => {
    let container!: HTMLElement;
    await act(async () => { ({ container } = renderGroup(toolRow(undefined))); });
    expect(screen.queryByRole("button", { name: "Open screenshot" })).toBeNull();
    expect(container.querySelector(".themis-tool-detail")).toHaveTextContent("image_attached");
  });
});

describe("tool media in the reducer", () => {
  const event = (kind: string, extra: object) => ({ type: "thread/event" as const, envelope: { thread_id: "t1", run_id: "r1", event: { kind, ...extra } as never } });

  it("keeps the step's media live and from history", () => {
    let state = reducer(initialState, event("tool_started", { tool: "browser_screenshot", summary: "shot" }));
    state = reducer(state, event("tool_finished", { tool: "browser_screenshot", ok: true, output: rawOutput, media: shot }));
    expect(state.messages.t1!.find(row => row.tool)!.tool!.media).toEqual(shot);
    const replayed = reducer(initialState, { type: "thread/history-loaded", threadId: "t1", history: [
      { kind: "user", run_id: "r1", text: "screenshot it" },
      { kind: "event", envelope: { thread_id: "t1", run_id: "r1", event: { kind: "tool_started", tool: "browser_screenshot", summary: "shot" } } },
      { kind: "event", envelope: { thread_id: "t1", run_id: "r1", event: { kind: "tool_finished", tool: "browser_screenshot", ok: true, output: rawOutput, media: shot } } },
    ] } as never);
    expect(replayed.messages.t1!.find(row => row.tool)!.tool!.media).toEqual(shot);
  });
});

describe("a goal turn", () => {
  it("starts after a quiet separator, not a bubble", () => {
    render(<GoalTurnDivider turn={3} />);
    const line = screen.getByRole("separator", { name: "Goal turn 3" });
    expect(line).toHaveClass("themis-goal-turn");
    expect(line).toHaveTextContent("");
  });
});

