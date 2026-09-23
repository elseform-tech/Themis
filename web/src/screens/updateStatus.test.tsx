// Update-status rendering for all four states (plus never-checked).
// Pure server-side render: no DOM, no bridge calls.
import { describe, expect, it, vi } from "vitest";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { UpdateStatus } from "../lib/types";

vi.mock("../lib/tauri", () => ({
  checkForUpdates: vi.fn(),
  clearSecret: vi.fn(),
  createThread: vi.fn(),
  discardThread: vi.fn(),
  getDiagnostics: vi.fn(),
  getSecretStatus: vi.fn(),
  getSettings: vi.fn(),
  getThread: vi.fn(),
  listAutomations: vi.fn(),
  listDiff: vi.fn(),
  listReviewItems: vi.fn(),
  listSkills: vi.fn(),
  listThreads: vi.fn(),
  mergeThread: vi.fn(),
  onApprovalRequest: vi.fn(),
  onReviewItemAdded: vi.fn(),
  onThreadEvent: vi.fn(),
  openProject: vi.fn(),
  setSecret: vi.fn(),
  updateSettings: vi.fn(),
}));

import { UpdateStatusView } from "./Settings";

function render(status: UpdateStatus | null): string {
  return renderToStaticMarkup(createElement(UpdateStatusView, { status }));
}

describe("UpdateStatusView", () => {
  it("renders the never-checked prompt when status is null", () => {
    const html = render(null);
    expect(html).toContain("Not checked yet");
    expect(html).toContain("Check for updates");
  });

  it("renders disabled with its message and a docs hint", () => {
    const html = render({ state: "disabled", message: "no endpoint configured" });
    expect(html).toContain("Updates are disabled");
    expect(html).toContain("no endpoint configured");
    expect(html).toContain("docs/user-guide.md");
  });

  it("renders up-to-date", () => {
    const html = render({ state: "up-to-date", message: "v0.1.0 is current" });
    expect(html).toContain("up to date");
    expect(html).toContain("v0.1.0 is current");
  });

  it("renders available with version and notes", () => {
    const html = render({
      state: "available",
      version: "0.2.0",
      notes: "Faster runs, new theme.",
    });
    expect(html).toContain("update available");
    expect(html).toContain("0.2.0");
    expect(html).toContain("Faster runs, new theme.");
  });

  it("renders error with its message", () => {
    const html = render({ state: "error", message: "network unreachable" });
    expect(html).toContain("check failed");
    expect(html).toContain("network unreachable");
  });
});
