// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AttachedMedia } from "../lib/types";
import { MediaGallery, MediaStrip, MediaViewer } from "./MediaStrip";

vi.mock("../lib/tauri", () => ({ attachmentFile: vi.fn(async (_thread: string, path: string) => `asset://${path}`) }));

const file = (name: string, kind: AttachedMedia["kind"] = "image", caption?: string): AttachedMedia => ({ path: `/p/.themis/attachments/${name}`, name, kind, caption });

function renderStrip(files: AttachedMedia[], open = vi.fn()) {
  render(<MediaGallery.Provider value={{ threadId: "t1", items: files, open }}><MediaStrip files={files} /></MediaGallery.Provider>);
  return open;
}

afterEach(() => cleanup());

describe("MediaStrip", () => {
  it("shows one item as a large preview", async () => {
    await act(async () => { renderStrip([file("shot.png", "image", "Sign-up error")]); });
    const tile = screen.getByRole("button", { name: "Open Sign-up error" });
    expect(tile).toHaveClass("is-large");
    expect(tile.querySelector("img")).toHaveAttribute("src", "asset:///p/.themis/attachments/shot.png");
  });

  it("shows two to four as tiles with captions, videos with a play mark", async () => {
    await act(async () => { renderStrip([file("a.png", "image", "Before"), file("b.png", "image", "After"), file("flow.mp4", "video", "Sign-up flow")]); });
    expect(screen.getAllByRole("listitem")).toHaveLength(3);
    expect(screen.getByRole("button", { name: "Play Sign-up flow" }).querySelector("video")).toBeInTheDocument();
    expect(screen.getByText("Before")).toBeInTheDocument();
  });

  it("folds five or more into three tiles and +N that opens the rest", async () => {
    const files = ["1", "2", "3", "4", "5", "6"].map(n => file(`${n}.png`));
    const open = renderStrip(files);
    await act(async () => undefined);
    expect(screen.getAllByRole("listitem")).toHaveLength(4);
    fireEvent.click(screen.getByRole("button", { name: "Show 3 more" }));
    expect(open).toHaveBeenCalledWith(files[3].path);
  });
});

describe("MediaViewer", () => {
  it("steps through the chat's media with buttons and arrow keys", async () => {
    const items = [file("a.png", "image", "Before"), file("b.png", "image", "After"), file("c.mp4", "video")];
    const onIndex = vi.fn();
    await act(async () => { render(<MediaViewer threadId="t1" items={items} index={1} onIndex={onIndex} onClose={vi.fn()} />); });
    expect(screen.getByText("2 of 3")).toBeInTheDocument();
    expect(screen.getByText("b.png")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(onIndex).toHaveBeenLastCalledWith(2);
    fireEvent.keyDown(document, { key: "ArrowLeft" });
    expect(onIndex).toHaveBeenLastCalledWith(0);
  });
});
