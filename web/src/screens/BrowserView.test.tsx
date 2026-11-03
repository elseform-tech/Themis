// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as bridge from "../lib/browser";
import type { BrowserState, BrowserUpdate } from "../lib/browser";
import { BrowserView, pageRect } from "./BrowserView";

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
  browserDevtoolsUrl: vi.fn(async () => "http://127.0.0.1:9222/devtools/devtools_app.html?ws=127.0.0.1:9222/devtools/page/T"),
  browserSetMuted: vi.fn(async () => undefined),
  browserCapture: vi.fn(async () => ({ path: "/p/.themis/attachments/x/screenshot-1701-12.png", name: "screenshot-1701-12.png", size: 10 })),
  browserRecordStart: vi.fn(async () => null),
  browserRecordStop: vi.fn(async () => ({ path: "/p/.themis/attachments/y/recording-1702-00.mp4", name: "recording-1702-00.mp4", size: 20, seconds: 12 })),
}));

const page: BrowserState = {
  open: true, available: true,
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

  it("shows where the agent's mouse is, and hides it while the person is in the page", async () => {
    vi.mocked(bridge.browserState).mockResolvedValue({ ...page, pointer: [120, 45] });
    render(<BrowserView threadId="t1" />);
    const cursor = await screen.findByTestId("agent-cursor");
    expect(cursor.style.transform).toBe("translate(120px, 45px)");
    fireEvent.mouseMove(screen.getByLabelText(/^Page:/), { clientX: 10, clientY: 10 });
    expect(screen.queryByTestId("agent-cursor")).toBeNull();
  });

  it("has no cursor before the agent has used the mouse", async () => {
    render(<BrowserView threadId="t1" />);
    await screen.findByLabelText(/^Page:/);
    expect(screen.queryByTestId("agent-cursor")).toBeNull();
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
    // A sideways swipe scrolls the page only: the app does not scroll or bounce with it.
    const swipe = new WheelEvent("wheel", { clientX: 300, clientY: 250, deltaX: 40, deltaY: 0, bubbles: true, cancelable: true });
    view.dispatchEvent(swipe);
    expect(swipe.defaultPrevented).toBe(true);
    expect(bridge.browserInput).toHaveBeenCalledWith("t1", expect.objectContaining({ type: "mouse", kind: "wheel", x: 200, y: 200, delta_x: 40, delta_y: 0 }));
  });

  it("opens typed addresses and splits off DevTools, with no tab strip", async () => {
    vi.mocked(bridge.browserState).mockResolvedValue({ ...page, controller: "idle", activity: null });
    await act(async () => { render(<BrowserView threadId="t1" />); });
    const address = screen.getByLabelText("Address");
    fireEvent.focus(address);
    fireEvent.change(address, { target: { value: "docs.rs/serde" } });
    await act(async () => fireEvent.submit(address));
    expect(bridge.browserOpenUrl).toHaveBeenCalledWith("t1", "docs.rs/serde");
    expect(screen.queryByRole("tablist", { name: "Browser tabs" })).toBeNull();
    await act(async () => fireEvent.click(screen.getByRole("button", { name: /^DevTools/ })));
    expect(document.querySelector("iframe.themis-browser-devtools")).toHaveAttribute("src", expect.stringContaining("/devtools/devtools_app.html"));
  });

  it("shows a loading bar while a page loads or a typed address is opening", async () => {
    const idle: BrowserState = { ...page, controller: "idle", activity: null };
    vi.mocked(bridge.browserState).mockResolvedValue(idle);
    await act(async () => { render(<BrowserView threadId="t1" />); });
    expect(screen.queryByRole("progressbar", { name: "Loading page" })).toBeNull();
    act(() => push({ kind: "state", state: { ...idle, loading: true } }));
    expect(screen.getByRole("progressbar", { name: "Loading page" })).toBeInTheDocument();
    act(() => push({ kind: "state", state: { ...idle, loading: false } }));
    expect(screen.queryByRole("progressbar")).toBeNull();

    // A typed address shows the bar at once, before the page reports loading.
    let answer: () => void = () => undefined;
    vi.mocked(bridge.browserOpenUrl).mockImplementationOnce(() => new Promise<void>(resolve => { answer = resolve; }));
    const address = screen.getByLabelText("Address");
    fireEvent.change(address, { target: { value: "docs.rs" } });
    act(() => { fireEvent.submit(address); });
    expect(screen.getByRole("progressbar", { name: "Loading page" })).toBeInTheDocument();
    await act(async () => answer());
    expect(screen.queryByRole("progressbar")).toBeNull();
  });

  it("shows the loading bar while the browser starts", async () => {
    vi.mocked(bridge.browserState).mockReturnValue(new Promise(() => undefined));
    await act(async () => { render(<BrowserView threadId="t1" />); });
    expect(screen.getByText("Loading browser…")).toBeInTheDocument();
    expect(screen.getByRole("progressbar", { name: "Loading browser" })).toBeInTheDocument();
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
    vi.mocked(bridge.browserState).mockResolvedValue({ ...page, open: false, available: false, url: "", title: "" });
    await act(async () => { render(<BrowserView threadId="t1" />); });
    expect(screen.getByText("No Chromium browser found")).toBeInTheDocument();
    cleanup();
    vi.mocked(bridge.browserState).mockResolvedValue({ ...page, open: false, url: "", title: "", controller: "idle", activity: null });
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

describe("a crisp Browser tab", () => {
  let resized: (() => void) | null = null;
  const panelRect = { left: 100, top: 50, width: 744, height: 560, right: 844, bottom: 610, x: 100, y: 50, toJSON: () => ({}) };
  beforeEach(() => {
    vi.stubGlobal("ResizeObserver", class { constructor(callback: () => void) { resized = callback; } observe() {} disconnect() {} });
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue(panelRect as DOMRect);
    vi.stubGlobal("devicePixelRatio", 2);
    vi.useFakeTimers();
  });
  afterEach(() => { vi.useRealTimers(); vi.restoreAllMocks(); resized = null; });

  it("streams at the panel's pixel density and leaves the agent's page its size mid-task", async () => {
    await act(async () => { render(<BrowserView threadId="t1" running />); });
    await act(async () => { vi.advanceTimersByTime(200); });
    expect(bridge.browserScreencast).toHaveBeenCalledTimes(1);
    expect(bridge.browserScreencast).toHaveBeenLastCalledWith("t1", 744, 560, 2, false);
    // The person drags the panel: once it settles, the page follows it.
    act(() => { resized?.(); resized?.(); });
    await act(async () => { vi.advanceTimersByTime(100); });
    expect(bridge.browserScreencast).toHaveBeenCalledTimes(1);
    await act(async () => { vi.advanceTimersByTime(200); });
    expect(bridge.browserScreencast).toHaveBeenCalledTimes(2);
    expect(bridge.browserScreencast).toHaveBeenLastCalledWith("t1", 744, 560, 2, true);
  });

  it("lays the page out at the panel's size when no run is going", async () => {
    vi.stubGlobal("devicePixelRatio", 4);
    await act(async () => { render(<BrowserView threadId="t1" running={false} />); });
    await act(async () => { vi.advanceTimersByTime(200); });
    // At most three device pixels per CSS pixel.
    expect(bridge.browserScreencast).toHaveBeenLastCalledWith("t1", 744, 560, 3, true);
  });

  it("draws a page laid out larger than the panel scaled to fit, and aims at the page's own pixels", async () => {
    vi.mocked(bridge.browserState).mockResolvedValue({ ...page, pointer: [640, 400] });
    await act(async () => { render(<BrowserView threadId="t1" running />); });
    await act(async () => { vi.advanceTimersByTime(200); });
    act(() => push({ kind: "frame", data: "BIG", width: 1280, height: 800 }));
    const img = document.querySelector<HTMLImageElement>(".themis-browser-page img")!;
    // 1280 × 800 into 744 × 560: 744 × 465 at the top, never stretched.
    expect([img.style.left, img.style.top, img.style.width, img.style.height]).toEqual(["0px", "0px", "744px", "465px"]);
    const [x, y] = screen.getByTestId("agent-cursor").style.transform.match(/[\d.]+/g)!.map(Number);
    expect(x).toBeCloseTo(372);
    expect(y).toBeCloseTo(232.5);
    const view = screen.getByLabelText("Page: Sign up");
    fireEvent.mouseDown(view, { clientX: 100 + 372, clientY: 50 + 232.5, button: 0, detail: 1 });
    const sent = vi.mocked(bridge.browserInput).mock.lastCall![1] as { kind: string; x: number; y: number };
    expect(sent.kind).toBe("down");
    expect(sent.x).toBeCloseTo(640);
    expect(sent.y).toBeCloseTo(400);
  });

  it("never draws a page larger than it is", () => {
    // A page smaller than the panel stays at its size, centred across, at the top.
    expect(pageRect({ width: 744, height: 560 }, { width: 600, height: 400 })).toEqual({ left: 72, top: 0, width: 600, height: 400, scale: 1 });
    // A tall page fits the height and is centred across.
    expect(pageRect({ width: 744, height: 560 }, { width: 800, height: 1120 })).toEqual({ left: 172, top: 0, width: 400, height: 560, scale: 0.5 });
    expect(pageRect(null, { width: 800, height: 600 }).scale).toBe(1);
  });
});
