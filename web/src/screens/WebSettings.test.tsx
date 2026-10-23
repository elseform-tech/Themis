// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import * as bridge from "../lib/tauri";
import { WebSettingsSection } from "./WebSettings";

vi.mock("../lib/tauri", () => ({
  getWebSettings: vi.fn(),
  updateWebSettings: vi.fn(async (searchUrl: string) => ({ search_url: searchUrl || null })),
  checkSearchServer: vi.fn(),
}));
afterEach(() => { cleanup(); vi.clearAllMocks(); });

async function show(searchUrl: string | null) {
  vi.mocked(bridge.getWebSettings).mockResolvedValue({ search_url: searchUrl });
  await act(async () => { render(<WebSettingsSection />); });
}

describe("Settings › Web", () => {
  it("uses and checks the built-in server when none of your own is set", async () => {
    vi.mocked(bridge.checkSearchServer).mockResolvedValue({ state: "working", results: 30, failed_engines: [] });
    vi.mocked(bridge.getWebSettings).mockResolvedValue({ search_url: null, built_in: { available: true } });
    await act(async () => { render(<WebSettingsSection />); });
    expect(bridge.checkSearchServer).toHaveBeenCalledWith(undefined);
    expect(screen.getByText("Built-in · Working · 30 results")).toBeInTheDocument();
  });

  it("checks the saved server on open and names failing engines", async () => {
    vi.mocked(bridge.checkSearchServer).mockResolvedValue({ state: "working", results: 4, failed_engines: [{ engine: "google", reason: "CAPTCHA" }] });
    await show("http://127.0.0.1:8888");
    expect(bridge.checkSearchServer).toHaveBeenCalledWith(undefined);
    expect(screen.getByText("Working · 4 results")).toBeInTheDocument();
    expect(screen.getByText("google: CAPTCHA")).toBeInTheDocument();
  });

  it("says web_search is off without a server, then saves one and checks it", async () => {
    vi.mocked(bridge.checkSearchServer).mockResolvedValue({ state: "working", results: 10, failed_engines: [] });
    await show(null);
    expect(screen.getByText("Not set · web_search is off")).toBeInTheDocument();
    expect(bridge.checkSearchServer).not.toHaveBeenCalled();
    const field = screen.getByRole("textbox", { name: "Search server" });
    fireEvent.change(field, { target: { value: " http://127.0.0.1:8888 " } });
    await act(async () => { fireEvent.blur(field); });
    expect(bridge.updateWebSettings).toHaveBeenCalledWith("http://127.0.0.1:8888");
    expect(screen.getByText("Working · 10 results")).toBeInTheDocument();
  });

  it("explains a server that refuses JSON", async () => {
    vi.mocked(bridge.checkSearchServer).mockResolvedValue({ state: "json_off", message: "JSON output is off: add json to search.formats in its settings.yml" });
    await show("http://127.0.0.1:8888");
    expect(screen.getByText("JSON output off")).toBeInTheDocument();
    expect(screen.getByText(/add json to search.formats/)).toBeInTheDocument();
  });
});
