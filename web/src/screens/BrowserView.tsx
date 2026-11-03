import { useCallback, useEffect, useRef, useState } from "react";
import { EmptyState } from "../components";
import {
  browserCapture, browserRecordStart, browserSetMuted, browserRecordStop, browserDevtoolsUrl, browserInput, browserNavigate, browserOpenUrl, browserScreencast,
  browserScreencastStop, browserState, browserStream, keyText, modifierBits, MOUSE_BUTTONS,
  type BrowserInput, type BrowserState, type SavedMedia,
} from "../lib/browser";
import { readSession, writeSession } from "../state/session";
import { describeError } from "../state/store";
import "./BrowserView.css";

/** Adds a saved screenshot or recording to the message being written in this chat. */
function addToComposer(threadId: string, file: SavedMedia) {
  const pending = readSession<Record<string, SavedMedia[]>>("attachments", {});
  writeSession("attachments", { ...pending, [threadId]: [...(pending[threadId] ?? []), { path: file.path, name: file.name, size: file.size }] });
  window.dispatchEvent(new Event("themis-attachments-changed"));
}

const clock = (seconds: number) => `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;

/** Mouse moves without a pressed button are sent at most this often. */
const MOVE_INTERVAL_MS = 50;
/** The page is laid out again only once the person stops resizing the panel this long. */
const RESIZE_SETTLE_MS = 250;

type Size = { width: number; height: number };

/** Device pixels per CSS pixel of this screen, as the backend takes it (1 to 3). */
const pixelRatio = () => Math.min(3, Math.max(1, window.devicePixelRatio || 1));

/** Where the page sits in the panel: scaled down to fit when it is laid out larger than
 *  the panel (never up, so a frame is never stretched), centred across and at the top. */
export function pageRect(panel: Size | null, page: Size | null): { left: number; top: number; width: number; height: number; scale: number } {
  if (!panel || !page || page.width <= 0 || page.height <= 0) return { left: 0, top: 0, width: panel?.width ?? 0, height: panel?.height ?? 0, scale: 1 };
  const scale = Math.min(1, panel.width / page.width, panel.height / page.height);
  const tidy = (value: number) => Math.round(value * 100) / 100;
  const width = tidy(page.width * scale);
  return { left: tidy(Math.max(0, (panel.width - width) / 2)), top: 0, width, height: tidy(page.height * scale), scale };
}

/**
 * The chat's built-in browser: the same page the agent drives, streamed from
 * the backend. The person and the agent use it at the same time. Nothing here
 * changes project files.
 */
export function BrowserView({ threadId, running = false }: { threadId: string; running?: boolean }) {
  const [state, setState] = useState<BrowserState | null>(null);
  const [frame, setFrame] = useState<string | null>(null);
  // The page's CSS size (from its frames) and the panel's, to place and aim at the page.
  const [page, setPage] = useState<Size | null>(null);
  const [panel, setPanel] = useState<Size | null>(null);
  const runningNow = useRef(running);
  runningNow.current = running;
  const [address, setAddress] = useState("");
  const [editing, setEditing] = useState(false);
  const [devtools, setDevtools] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [navigating, setNavigating] = useState(false);
  const [recordedAt, setRecordedAt] = useState(0);
  const [now, setNow] = useState(Date.now());
  const viewport = useRef<HTMLDivElement>(null);
  const lastMove = useRef(0);

  const apply = useCallback((next: BrowserState) => {
    setState(previous => ({ ...next, available: next.available ?? previous?.available }));
    setRecordedAt(Date.now());
  }, []);

  const recording = state?.recording ?? null;
  // The recording line counts up between updates.
  useEffect(() => {
    if (!recording) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [recording]);

  useEffect(() => {
    let live = true;
    setState(null); setFrame(null); setPage(null); setDevtools(null); setError("");
    browserState(threadId).then(next => { if (live) apply(next); }).catch(reason => { if (live) setError(describeError(reason)); });
    const stop = browserStream(threadId, update => {
      if (!live) return;
      if (update.kind === "frame") {
        setFrame(`data:image/jpeg;base64,${update.data}`);
        if (update.width > 0 && update.height > 0) setPage(previous => previous?.width === update.width && previous.height === update.height ? previous : { width: update.width, height: update.height });
      } else apply(update.state);
    });
    return () => { live = false; stop(); void browserScreencastStop(threadId).catch(() => undefined); };
  }, [threadId, apply]);

  const open = state?.open ?? false;
  // The stream has no pointer in it. The person's own mouse takes over until the agent moves again.
  const [personAt, setPersonAt] = useState<string | null>(null);
  const pointerKey = state?.pointer ? state.pointer.join() : null;
  // A page holding the pointer lock has no pointer to show: the person's mouse turns the view.
  const pageLocked = state?.pointer_locked ?? false;
  const agentCursor = pointerKey && pointerKey !== personAt && !pageLocked ? state?.pointer ?? null : null;
  // Frames come at the panel's pixel density. The page is laid out at the panel's size
  // when the tab opens with no run going, and after the person resizes the panel; never
  // under an agent mid-task, whose coordinates must hold (the page is then scaled to fit).
  useEffect(() => {
    const element = viewport.current;
    if (!open || !element) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let first = true;
    const fit = () => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        const { width, height } = element.getBoundingClientRect();
        if (width <= 0 || height <= 0) return;
        setPanel({ width, height });
        const resize = !first || !runningNow.current;
        first = false;
        void browserScreencast(threadId, width, height, pixelRatio(), resize).catch(reason => setError(describeError(reason)));
      }, first ? 150 : RESIZE_SETTLE_MS);
    };
    fit();
    const observer = new ResizeObserver(fit);
    observer.observe(element);
    return () => { clearTimeout(timer); observer.disconnect(); };
  }, [threadId, open]);

  useEffect(() => { if (!editing && state) setAddress(state.url === "about:blank" ? "" : state.url); }, [state, editing]);

  const send = useCallback((input: BrowserInput) => {
    void browserInput(threadId, input).catch(reason => setError(describeError(reason)));
  }, [threadId]);

  // The person's mouse is locked to the view while the page holds the pointer lock: its
  // movement goes as relative moves, gathered once a frame. Escape (which the app's web view
  // keeps for itself) gives the mouse back and lets the page go too.
  const [mouseHeld, setMouseHeld] = useState(false);
  const pageLockedNow = useRef(pageLocked);
  pageLockedNow.current = pageLocked;
  const moved = useRef({ dx: 0, dy: 0, frame: 0 });
  useEffect(() => {
    const changed = () => {
      const held = !!viewport.current && document.pointerLockElement === viewport.current;
      setMouseHeld(held);
      if (!held && pageLockedNow.current) {
        for (const kind of ["down", "up"] as const) send({ type: "key", kind, key: "Escape", code: "Escape", key_code: 27, modifiers: 0 });
      }
    };
    document.addEventListener("pointerlockchange", changed);
    return () => document.removeEventListener("pointerlockchange", changed);
  }, [send]);
  useEffect(() => {
    if (!pageLocked && viewport.current && document.pointerLockElement === viewport.current) document.exitPointerLock();
  }, [pageLocked]);
  function turn(event: React.MouseEvent) {
    const scale = placed.current.scale || 1;
    moved.current.dx += event.movementX / scale;
    moved.current.dy += event.movementY / scale;
    if (moved.current.frame) return;
    moved.current.frame = requestAnimationFrame(() => {
      const { dx, dy } = moved.current;
      moved.current = { dx: 0, dy: 0, frame: 0 };
      if (dx || dy) send({ type: "mouse_by", dx, dy });
    });
  }

  // Scrolling goes to the page only: a non-passive listener (React's onWheel is passive)
  // cancels the gesture here, so the app does not scroll or bounce with it.
  useEffect(() => {
    const element = viewport.current;
    if (!element) return;
    const wheel = (event: WheelEvent) => {
      event.preventDefault();
      send({ type: "mouse", kind: "wheel", ...point(event), delta_x: event.deltaX, delta_y: event.deltaY, modifiers: modifierBits(event) });
    };
    element.addEventListener("wheel", wheel, { passive: false });
    return () => element.removeEventListener("wheel", wheel);
  }, [open, send]);

  const shown = pageRect(panel, page);
  // Read when an event comes (the wheel listener outlives a render).
  const placed = useRef(shown);
  placed.current = shown;
  /** A point of the panel in the page's own CSS pixels. */
  function point(event: { clientX: number; clientY: number }) {
    const rect = viewport.current?.getBoundingClientRect();
    const { left, top, scale } = placed.current;
    return { x: (event.clientX - (rect?.left ?? 0) - left) / scale, y: (event.clientY - (rect?.top ?? 0) - top) / scale };
  }

  function mouse(kind: "down" | "up" | "move", event: React.MouseEvent) {
    if (kind === "move") {
      const now = performance.now();
      if (event.buttons === 0 && now - lastMove.current < MOVE_INTERVAL_MS) return;
      lastMove.current = now;
    }
    if (kind === "down") {
      viewport.current?.focus();
      // A click on a page holding the pointer lock takes the person's mouse into the view.
      if (pageLocked && !mouseHeld) viewport.current?.requestPointerLock?.();
    }
    if (kind === "move" && mouseHeld) { turn(event); return; }
    send({ type: "mouse", kind, ...point(event), button: MOUSE_BUTTONS[event.button] ?? "left", clicks: kind === "move" ? 0 : event.detail || 1, modifiers: modifierBits(event) });
  }

  function key(kind: "down" | "up", event: React.KeyboardEvent) {
    // Keep the app's own shortcuts (Escape closes popovers) out of the page only while it has focus.
    event.preventDefault();
    send({ type: "key", kind, key: event.key, code: event.code, text: kind === "down" ? keyText(event) : undefined, key_code: event.keyCode, modifiers: modifierBits(event) });
  }

  async function act(work: () => Promise<unknown>) {
    setError("");
    try { await work(); } catch (reason) { setError(describeError(reason)); }
  }

  /** Runs a navigation the person started, showing the loading bar until it answers. */
  async function navigate(work: () => Promise<unknown>) {
    setNavigating(true);
    try { await act(work); } finally { setNavigating(false); }
  }

  async function capture() {
    await act(async () => addToComposer(threadId, await browserCapture(threadId)));
  }

  async function toggleRecording() {
    await act(async () => {
      if (!recording) { await browserRecordStart(threadId); return; }
      const saved = await browserRecordStop(threadId);
      addToComposer(threadId, saved);
    });
  }

  async function toggleDevtools() {
    if (devtools) { setDevtools(null); return; }
    await act(async () => setDevtools(await browserDevtoolsUrl(threadId)));
  }

  if (state === null) {
    if (error) return <EmptyState title="Browser unavailable" hint={error} />;
    // Starting can take a while the first time, while the browser engine downloads.
    return <div className="themis-browser-starting"><EmptyState title="Loading browser…" /><span className="themis-browser-progress" role="progressbar" aria-label="Loading browser" /></div>;
  }
  if (state.available === false && !open) {
    return <EmptyState title="No Chromium browser found" hint="Install Chrome, Chromium or Edge, or set browser.executable in runtime.jsonc." />;
  }
  // The page is loading, a navigation the person started has not answered yet, or the
  // agent is starting the browser (the first start can include downloading its engine).
  const loading = state.loading || navigating || (state.controller === "agent" && !open);
  return (
    <div className={`themis-browser${devtools ? " has-devtools" : ""}`}>
      <div className="themis-browser-bar">
        <button type="button" className="themis-browser-icon" aria-label="Back" disabled={!state.can_go_back} onClick={() => void navigate(() => browserNavigate(threadId, "back"))}><Chevron left /></button>
        <button type="button" className="themis-browser-icon" aria-label="Forward" disabled={!state.can_go_forward} onClick={() => void navigate(() => browserNavigate(threadId, "forward"))}><Chevron /></button>
        <button type="button" className="themis-browser-icon" aria-label="Reload" disabled={!open} onClick={() => void navigate(() => browserNavigate(threadId, "reload"))}><Reload /></button>
        <form className="themis-browser-address" onSubmit={event => { event.preventDefault(); if (address.trim()) void navigate(() => browserOpenUrl(threadId, address)); (document.activeElement as HTMLElement | null)?.blur(); }}>
          <label className="themis-visually-hidden" htmlFor={`browser-address-${threadId}`}>Address</label>
          <input id={`browser-address-${threadId}`} value={address} placeholder="Enter an address" spellCheck={false} autoComplete="off"
            onFocus={() => setEditing(true)} onBlur={() => setEditing(false)} onChange={event => setAddress(event.target.value)} />
          {loading && <span className="themis-browser-progress" role="progressbar" aria-label="Loading page" />}
        </form>
        <button type="button" className="themis-browser-icon" aria-label={state.muted ? "Turn sound on" : "Turn sound off"} title={state.muted ? "Sound off" : "Sound on"} disabled={!open} onClick={() => void act(() => browserSetMuted(threadId, !state.muted))}><Speaker muted={state.muted} /></button>
        <button type="button" className="themis-browser-icon" aria-label="Capture screenshot to the message" title="Capture" disabled={!open} onClick={() => void capture()}><Camera /></button>
        <button type="button" className={`themis-browser-icon${recording ? " is-recording" : ""}`} aria-pressed={recording !== null} aria-label={recording ? "Stop recording" : "Record the browser"} title={recording ? "Stop recording" : "Record"} disabled={!open} onClick={() => void toggleRecording()}><RecordIcon /></button>
        <button type="button" className={`themis-browser-icon${devtools ? " is-on" : ""}`} aria-pressed={devtools !== null} disabled={!open}
          aria-label={state.console_errors > 0 ? `DevTools, ${state.console_errors} console error${state.console_errors === 1 ? "" : "s"}` : "DevTools"} title="DevTools" onClick={() => void toggleDevtools()}>
          <Code />{state.console_errors > 0 && <span className="themis-browser-errors">{state.console_errors}</span>}
        </button>
      </div>
      {recording && (
        <div className="themis-browser-status is-recording" role="status">
          <span className="themis-recording-dot" aria-hidden="true" />
          <span className="themis-browser-status-text">Recording {clock(Math.min(recording.max_seconds, recording.seconds + Math.max(0, Math.floor((now - recordedAt) / 1000))))} · started by {recording.by === "user" ? "you" : "agent"} · stops at {clock(recording.max_seconds)}</span>
          <button type="button" className="themis-browser-action is-danger" onClick={() => void toggleRecording()}>Stop</button>
        </div>
      )}
      {state.controller === "agent" && state.activity && (
        <div className="themis-browser-status" role="status">
          <span className="themis-browser-spinner" aria-hidden="true" />
          <span className="themis-browser-status-text">Agent is {state.activity}</span>
        </div>
      )}
      {error && <p className="themis-workspace-note" role="alert">{error}</p>}
      {open ? (
        <div ref={viewport} className={`themis-browser-page${recording ? " is-recording" : ""}`} tabIndex={0} aria-label={`Page: ${state.title || state.url}`}
          onMouseDown={event => { event.preventDefault(); mouse("down", event); }} onMouseUp={event => mouse("up", event)} onMouseMove={event => { setPersonAt(pointerKey); mouse("move", event); }}
          onContextMenu={event => event.preventDefault()}
          onKeyDown={event => key("down", event)} onKeyUp={event => key("up", event)}
          onPaste={event => { event.preventDefault(); const text = event.clipboardData.getData("text"); if (text) send({ type: "text", text }); }}>
          {frame && <img src={frame} alt="" draggable={false} style={page && panel ? { left: shown.left, top: shown.top, width: shown.width, height: shown.height } : undefined} />}
          {pageLocked && <span className="themis-browser-lock-hint" role="status">{mouseHeld ? "Esc to release the mouse" : "Click to control the view"}</span>}
          {agentCursor && <svg className="themis-browser-cursor" data-testid="agent-cursor" style={{ transform: `translate(${shown.left + agentCursor[0] * shown.scale}px, ${shown.top + agentCursor[1] * shown.scale}px)` }} width="18" height="18" viewBox="0 0 18 18" aria-hidden="true"><path d="M2 1.5v13l3.6-3.2 2.4 5.2 2.6-1.2-2.4-5.1h4.8Z" /></svg>}
        </div>
      ) : (
        <EmptyState title="No page open" hint="Pages the agent opens show here too." />
      )}
      {devtools && <iframe className="themis-browser-devtools" title="DevTools" src={devtools} />}
    </div>
  );
}

function Chevron({ left = false }: { left?: boolean }) {
  return <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.9" aria-hidden="true"><path d={left ? "m15 6-6 6 6 6" : "m9 6 6 6-6 6"} /></svg>;
}

function Reload() {
  return <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.9" aria-hidden="true"><path d="M20 11a8 8 0 1 0-2.3 5.7M20 4v7h-7" /></svg>;
}

function Speaker({ muted }: { muted: boolean }) {
  return <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M4 9.5h3.5L12 5.5v13l-4.5-4H4z" />{muted ? <path d="m16 9.5 5 5m0-5-5 5" /> : <path d="M16 9a4.5 4.5 0 0 1 0 6m2.5-8.5a8 8 0 0 1 0 11" />}</svg>;
}

function Camera() {
  return <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" aria-hidden="true"><path d="M4 8.5A1.5 1.5 0 0 1 5.5 7h2l1.5-2h6l1.5 2h2A1.5 1.5 0 0 1 20 8.5v9a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 17.5z" /><circle cx="12" cy="13" r="3.5" /></svg>;
}

function RecordIcon() {
  return <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" aria-hidden="true"><circle cx="12" cy="12" r="8" /><circle cx="12" cy="12" r="3.5" fill="currentColor" /></svg>;
}

function Code() {
  return <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" aria-hidden="true"><path d="m8 8-4 4 4 4M16 8l4 4-4 4M13.5 5l-3 14" /></svg>;
}
