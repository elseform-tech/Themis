// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as bridge from "../lib/browser";
import type { BrowserState, BrowserUpdate } from "../lib/browser";
import { BrowserView } from "./BrowserView";

let push: (update: BrowserUpdate) => void = () => undefined;
vi.mock("../lib/browser", async importOriginal => ({
  ...await importOriginal<typeof import("../lib/browser")>(),
  browserState: vi.fn(),
  browserStream: vi.fn((_thread: string, onUpdate: (update: BrowserUpdate) => void) => { push = onUpdate; return vi.fn(); }),
  browserScreencast: vi.fn(async () => undefined),
  browserScreencastStop: vi.fn(async () => undefined),
  browserInput: vi.fn(async () => undefined),
  browserOpenUrl: vi.fn(async () => undefined),
  browserNavigate: vi.fn(async () => undefined),
  browserSwitchTab: vi.fn(async () => undefined),
  browserCloseTab: vi.fn(async () => undefined),
  browserDevtoolsUrl: vi.fn(async () => "http://127.0.0.1:9222/devtools/inspector.html?ws=127.0.0.1:9222/devtools/page/T"),
  browserSetMuted: vi.fn(async () => undefined),
  browserCapture: vi.fn(async () => ({ path: "/p/.themis/attachments/x/screenshot-1701-12.png", name: "screenshot-1701-12.png", size: 10 })),
  browserRecordStart: vi.fn(async () => null),
  browserRecordStop: vi.fn(async () => ({ path: "/p/.themis/attachments/y/recording-1702-00.mp4", name: "recording-1702-00.mp4", size: 20, seconds: 12 })),
}));

const page: BrowserState = {
  open: true, available: true, tabs: [{ title: "Sign up", url: "http://localhost:5173/signup" }], current: 0,
  url: "http://localhost:5173/signup", title: "Sign up", can_go_back: true, can_go_forward: false,
  controller: "agent", activity: "clicking “Create account”", console_errors: 1, loading: false, muted: true,
};

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  vi.mocked(bridge.browserState).mockResolvedValue(page);
});
afterEach(() => { cleanup(); vi.clearAllMocks(); vi.unstubAllGlobals(); });

describe("BrowserView", () => {
  it("shows the agent's page and activity without a take-over control, and streams frames", async () => {
    await act(async () => { render(<BrowserView threadId="t1" />); });
    expect(screen.getByRole("status")).toHaveTextContent("Agent is clicking “Create account”");
    expect(screen.getByLabelText("Address")).toHaveValue("http://localhost:5173/signup");
    expect(screen.getByRole("button", { name: "Back" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Forward" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "DevTools, 1 console error" })).toBeInTheDocument();
    // A single tab needs no tab strip.
    expect(screen.queryByRole("tablist")).toBeNull();
    act(() => push({ kind: "frame", data: "AAAA", width: 800, height: 600 }));
    expect(document.querySelector(".themis-browser-page img")).toHaveAttribute("src", "data:image/jpeg;base64,AAAA");
    // The person and the agent use the page at the same time.
    expect(screen.queryByRole("button", { name: "Take over" })).toBeNull();
  });

  it("forwards input in page coordinates while the agent keeps working", async () => {
    await act(async () => { render(<BrowserView threadId="t1" />); });
    const view = screen.getByLabelText("Page: Sign up");
    view.getBoundingClientRect = () => ({ left: 100, top: 50, width: 800, height: 600, right: 900, bottom: 650, x: 100, y: 50, toJSON: () => ({}) });
    fireEvent.mouseDown(view, { clientX: 150, clientY: 80, button: 0, detail: 1 });
    expect(bridge.browserInput).toHaveBeenCalledWith("t1", expect.objectContaining({ type: "mouse", kind: "down", x: 50, y: 30, button: "left", clicks: 1 }));
    fireEvent.keyDown(view, { key: "a", code: "KeyA", keyCode: 65, metaKey: true });
    expect(bridge.browserInput).toHaveBeenCalledWith("t1", expect.objectContaining({ type: "key", kind: "down", key: "a", modifiers: 4, text: undefined }));
    fireEvent.keyDown(view, { key: "x", code: "KeyX", keyCode: 88 });
    expect(bridge.browserInput).toHaveBeenCalledWith("t1", expect.objectContaining({ key: "x", text: "x" }));
    expect(screen.getByRole("status")).toHaveTextContent("Agent is clicking");
    expect(screen.queryByRole("button", { name: "Hand back" })).toBeNull();
  });

  it("opens typed addresses, switches tabs and splits off DevTools", async () => {
    vi.mocked(bridge.browserState).mockResolvedValue({ ...page, controller: "idle", activity: null, tabs: [...page.tabs, { title: "Docs", url: "https://docs.rs" }] });
    await act(async () => { render(<BrowserView threadId="t1" />); });
    const address = screen.getByLabelText("Address");
    fireEvent.focus(address);
    fireEvent.change(address, { target: { value: "docs.rs/serde" } });
    await act(async () => fireEvent.submit(address));
    expect(bridge.browserOpenUrl).toHaveBeenCalledWith("t1", "docs.rs/serde");
    await act(async () => fireEvent.click(screen.getByRole("tab", { name: "Docs" })));
    expect(bridge.browserSwitchTab).toHaveBeenCalledWith("t1", 2);
    await act(async () => fireEvent.click(screen.getByRole("button", { name: /^DevTools/ })));
    expect(document.querySelector("iframe.themis-browser-devtools")).toHaveAttribute("src", expect.stringContaining("/devtools/inspector.html"));
  });

  it("captures to the composer, records with a visible line, and stops into the composer", async () => {
    const store = new Map<string, string>();
    vi.stubGlobal("localStorage", { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => store.set(k, v), removeItem: (k: string) => store.delete(k) });
    const changed = vi.fn();
    window.addEventListener("themis-attachments-changed", changed);
    await act(async () => { render(<BrowserView threadId="t1" />); });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Capture screenshot to the message" })));
    expect(bridge.browserCapture).toHaveBeenCalledWith("t1");
    expect(changed).toHaveBeenCalled();
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Record the browser" })));
    expect(bridge.browserRecordStart).toHaveBeenCalledWith("t1");
    act(() => push({ kind: "state", state: { ...page, recording: { by: "user", seconds: 12, max_seconds: 120 } } }));
    expect(screen.getAllByRole("status")[0]).toHaveTextContent("Recording 0:12 · started by you · stops at 2:00");
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Stop" })));
    expect(bridge.browserRecordStop).toHaveBeenCalledWith("t1");
    window.removeEventListener("themis-attachments-changed", changed);
  });

  it("explains a missing browser and an empty one", async () => {
    vi.mocked(bridge.browserState).mockResolvedValue({ ...page, open: false, available: false, tabs: [], url: "", title: "" });
    await act(async () => { render(<BrowserView threadId="t1" />); });
    expect(screen.getByText("No Chromium browser found")).toBeInTheDocument();
    cleanup();
    vi.mocked(bridge.browserState).mockResolvedValue({ ...page, open: false, tabs: [], url: "", title: "", controller: "idle", activity: null });
    await act(async () => { render(<BrowserView threadId="t2" />); });
    expect(screen.getByText("No page open")).toBeInTheDocument();
    expect(screen.getByLabelText("Address")).toHaveValue("");
  });

  it("starts with the sound off and turns it on and off", async () => {
    await act(async () => { render(<BrowserView threadId="t1" />); });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Turn sound on" })));
    expect(bridge.browserSetMuted).toHaveBeenCalledWith("t1", false);
    act(() => push({ kind: "state", state: { ...page, muted: false } }));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Turn sound off" })));
    expect(bridge.browserSetMuted).toHaveBeenCalledWith("t1", true);
  });
});
