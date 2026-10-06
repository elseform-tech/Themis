// @vitest-environment jsdom
import { act, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { AttachmentPreview } from "./AttachmentPreview";
import { attachmentFile } from "../lib/tauri";
vi.mock("../lib/tauri", () => ({ attachmentFile: vi.fn(async (_id, path) => `asset://localhost${path}`) }));
it("previews files with filenames and no open actions", async () => {
  for (const name of ["image.png", "document.pdf", "music.mp3", "video.mp4", "archive.zip"]) {
    await act(async () => { render(<AttachmentPreview threadId="t1" path={`/project/.themis/attachments/id/${name}`} />); });
    expect(screen.getByRole("figure", { name })).toBeInTheDocument();
    expect(screen.getByText(name)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: `Open ${name}` })).toBeNull();
  }
  expect(screen.getByAltText("image.png")).toHaveAttribute("src", expect.stringContaining("image.png"));
  expect(screen.getByTitle("Preview document.pdf")).toHaveAttribute("src", expect.stringContaining("document.pdf"));
  expect(screen.getByLabelText("Play music.mp3")).toHaveAttribute("controls");
  expect(screen.getByLabelText("Play video.mp4")).toHaveAttribute("controls");
  expect(attachmentFile).toHaveBeenCalledWith("t1", expect.stringContaining("document.pdf"));
});
