// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { inputChunks, type TerminalUpdate } from "../lib/terminal";
import { TerminalView } from "./TerminalView";

const written = vi.hoisted(() => [] as string[]);
const typed = vi.hoisted(() => ({ send: (_: string) => {} }));
const disposed = vi.hoisted(() => ({ count: 0 }));

vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    cols = 90;
    rows = 30;
    options = {};
    loadAddon() {}
    open() {}
    focus() {}
    write(text: string) { written.push(text); }
    onData(listener: (data: string) => void) { typed.send = listener; return { dispose() {} }; }
    dispose() { disposed.count += 1; }
  },
}));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
vi.mock("@xterm/xterm/css/xterm.css", () => ({}));

const bridge = vi.hoisted(() => ({
  update: (_: TerminalUpdate) => {},
  stopped: 0,
  open: vi.fn(),
  input: vi.fn(),
  resize: vi.fn(),
}));

vi.mock("../lib/terminal", async original => ({
  ...(await original<typeof import("../lib/terminal")>()),
  terminalOpen: bridge.open,
  terminalInput: bridge.input,
  terminalResize: bridge.resize,
  terminalStream: (_: string, onUpdate: (update: TerminalUpdate) => void) => {
    bridge.update = onUpdate;
    return () => { bridge.stopped += 1; };
  },
}));

vi.mock("../state/store", () => ({ describeError: (reason: unknown) => String(reason) }));

beforeEach(() => {
  written.length = 0;
  disposed.count = 0;
  bridge.stopped = 0;
  bridge.open.mockReset().mockResolvedValue({ root: "/wt/one", running: true, exit_code: null });
  bridge.input.mockReset().mockResolvedValue(undefined);
  bridge.resize.mockReset().mockResolvedValue(undefined);
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
});

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

async function opened() {
  const view = render(<TerminalView projectRoot="/project" threadId="chat" />);
  await act(async () => {});
  return view;
}

describe("TerminalView", () => {
  it("opens the chat's terminal at the panel's size and shows what it kept", async () => {
    await opened();
    expect(bridge.open).toHaveBeenCalledWith("/project", "chat", 90, 30);
    act(() => bridge.update({ kind: "state", state: { root: "/wt/one", running: true, exit_code: null, scrollback: "kept" } }));
    act(() => bridge.update({ kind: "output", data: "more" }));
    expect(written).toEqual(["kept", "more"]);
  });

  it("sends what is typed to the folder the server named, in order", async () => {
    await opened();
    act(() => { typed.send("ls\r"); typed.send("pwd\r"); });
    await act(async () => {});
    expect(bridge.input.mock.calls).toEqual([["/wt/one", "ls\r"], ["/wt/one", "pwd\r"]]);
  });

  it("offers Restart once the shell has ended, and opens it again", async () => {
    await opened();
    expect(screen.queryByRole("button", { name: "Restart" })).toBeNull();
    act(() => bridge.update({ kind: "exit", code: 3 }));
    expect(screen.getByRole("status")).toHaveTextContent("The shell exited with code 3");
    fireEvent.click(screen.getByRole("button", { name: "Restart" }));
    await act(async () => {});
    expect(bridge.open).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("button", { name: "Restart" })).toBeNull();
  });

  it("says why when it can't open", async () => {
    bridge.open.mockRejectedValue("8 terminals are already running");
    await opened();
    expect(screen.getByRole("alert")).toHaveTextContent("8 terminals are already running");
  });

  it("leaving stops watching, and asks nothing that would end the shell", async () => {
    const view = await opened();
    view.unmount();
    expect(bridge.stopped).toBe(1);
    expect(disposed.count).toBe(1);
  });
});

describe("inputChunks", () => {
  it("keeps a long paste whole, in pieces the server accepts", () => {
    const paste = "é😀x".repeat(30_000);
    const chunks = inputChunks(paste);
    expect(chunks.join("")).toBe(paste);
    for (const chunk of chunks) {
      expect(new TextEncoder().encode(chunk).length).toBeLessThanOrEqual(64 * 1024);
      // A lone half of a surrogate pair wouldn't survive the trip.
      expect(new TextDecoder().decode(new TextEncoder().encode(chunk))).toBe(chunk);
    }
    expect(inputChunks("")).toEqual([]);
  });
});
