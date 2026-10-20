import { useCallback, useEffect, useRef, useState } from "react";
import { EmptyState } from "../components";
import {
  browserCapture, browserCloseTab, browserRecordStart, browserSetMuted, browserRecordStop, browserDevtoolsUrl, browserInput, browserNavigate, browserOpenUrl, browserScreencast,
  browserScreencastStop, browserState, browserStream, browserSwitchTab, keyText, modifierBits, MOUSE_BUTTONS,
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

/**
 * The chat's built-in browser: the same page the agent drives, streamed from
 * the backend. Any input takes over; the agent's browser actions wait until
 * Hand back. Nothing here changes project files.
 */
export function BrowserView({ threadId }: { threadId: string }) {
  const [state, setState] = useState<BrowserState | null>(null);
  const [frame, setFrame] = useState<string | null>(null);
  const [address, setAddress] = useState("");
  const [editing, setEditing] = useState(false);
  const [devtools, setDevtools] = useState<string | null>(null);
  const [error, setError] = useState("");
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
    setState(null); setFrame(null); setDevtools(null); setError("");
    browserState(threadId).then(next => { if (live) apply(next); }).catch(reason => { if (live) setError(describeError(reason)); });
    const stop = browserStream(threadId, update => {
      if (!live) return;
      if (update.kind === "frame") setFrame(`data:image/jpeg;base64,${update.data}`);
      else apply(update.state);
    });
    return () => { live = false; stop(); void browserScreencastStop(threadId).catch(() => undefined); };
  }, [threadId, apply]);

  const open = state?.open ?? false;
  // The page is laid out at the view's size, so input coordinates map one to one.
  useEffect(() => {
    const element = viewport.current;
    if (!open || !element) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const fit = () => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        const { width, height } = element.getBoundingClientRect();
        if (width > 0 && height > 0) void browserScreencast(threadId, width, height, window.devicePixelRatio || 1).catch(reason => setError(describeError(reason)));
      }, 150);
    };
    fit();
    const observer = new ResizeObserver(fit);
    observer.observe(element);
    return () => { clearTimeout(timer); observer.disconnect(); };
  }, [threadId, open, devtools]);

  useEffect(() => { if (!editing && state) setAddress(state.url === "about:blank" ? "" : state.url); }, [state, editing]);

  const send = useCallback((input: BrowserInput) => {
    void browserInput(threadId, input).catch(reason => setError(describeError(reason)));
  }, [threadId]);

  function point(event: React.MouseEvent | React.WheelEvent) {
    const rect = viewport.current?.getBoundingClientRect();
    return { x: event.clientX - (rect?.left ?? 0), y: event.clientY - (rect?.top ?? 0) };
  }

  function mouse(kind: "down" | "up" | "move", event: React.MouseEvent) {
    if (kind === "move") {
      const now = performance.now();
      if (event.buttons === 0 && now - lastMove.current < MOVE_INTERVAL_MS) return;
      lastMove.current = now;
    }
    if (kind === "down") viewport.current?.focus();
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

  if (state === null) return error ? <EmptyState title="Browser unavailable" hint={error} /> : <EmptyState title="Loading browser…" />;
  if (state.available === false && !open) {
    return <EmptyState title="No Chromium browser found" hint="Install Chrome, Chromium or Edge, or set browser.executable in runtime.jsonc." />;
  }
  return (
    <div className={`themis-browser${devtools ? " has-devtools" : ""}`}>
      {state.tabs.length > 1 && (
        <div className="themis-browser-tabs" role="tablist" aria-label="Browser tabs">
          {state.tabs.map((tab, index) => (
            <span key={index} className={`themis-browser-tab${index === state.current ? " is-current" : ""}`}>
              <button type="button" role="tab" aria-selected={index === state.current} title={tab.url} onClick={() => void act(() => browserSwitchTab(threadId, index + 1))}>{tab.title || tab.url}</button>
              <button type="button" className="themis-browser-tab-close" aria-label={`Close ${tab.title || "tab"}`} onClick={() => void act(() => browserCloseTab(threadId, index + 1))}>✕</button>
            </span>
          ))}
        </div>
      )}
      <div className="themis-browser-bar">
        <button type="button" className="themis-browser-icon" aria-label="Back" disabled={!state.can_go_back} onClick={() => void act(() => browserNavigate(threadId, "back"))}><Chevron left /></button>
        <button type="button" className="themis-browser-icon" aria-label="Forward" disabled={!state.can_go_forward} onClick={() => void act(() => browserNavigate(threadId, "forward"))}><Chevron /></button>
        <button type="button" className="themis-browser-icon" aria-label="Reload" disabled={!open} onClick={() => void act(() => browserNavigate(threadId, "reload"))}><Reload /></button>
        <form className="themis-browser-address" onSubmit={event => { event.preventDefault(); if (address.trim()) void act(() => browserOpenUrl(threadId, address)); (document.activeElement as HTMLElement | null)?.blur(); }}>
          <label className="themis-visually-hidden" htmlFor={`browser-address-${threadId}`}>Address</label>
          <input id={`browser-address-${threadId}`} value={address} placeholder="Enter an address" spellCheck={false} autoComplete="off"
            onFocus={() => setEditing(true)} onBlur={() => setEditing(false)} onChange={event => setAddress(event.target.value)} />
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
          onMouseDown={event => { event.preventDefault(); mouse("down", event); }} onMouseUp={event => mouse("up", event)} onMouseMove={event => mouse("move", event)}
          onWheel={event => send({ type: "mouse", kind: "wheel", ...point(event), delta_x: event.deltaX, delta_y: event.deltaY, modifiers: modifierBits(event) })}
          onContextMenu={event => event.preventDefault()}
          onKeyDown={event => key("down", event)} onKeyUp={event => key("up", event)}
          onPaste={event => { event.preventDefault(); const text = event.clipboardData.getData("text"); if (text) send({ type: "text", text }); }}>
          {frame && <img src={frame} alt="" draggable={false} />}
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
