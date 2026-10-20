// The built-in browser's bridge: the Workspace Browser tab streams the chat's
// browser, forwards the person's input and drives take over and DevTools.
import { Channel, invoke as tauriInvoke } from "@tauri-apps/api/core";

export interface BrowserTab { title: string; url: string }

export interface BrowserState {
  /** False until the chat has a page open. */
  open: boolean;
  /** False when no Chromium browser is installed or configured. */
  available?: boolean;
  tabs: BrowserTab[];
  current: number;
  url: string;
  title: string;
  can_go_back: boolean;
  can_go_forward: boolean;
  /** Who drives the browser now. */
  controller: "agent" | "user" | "idle";
  /** What the agent is doing, e.g. `clicking “Create account”`. */
  activity: string | null;
  console_errors: number;
  loading: boolean;
}

export type BrowserUpdate =
  | { kind: "state"; state: BrowserState }
  | { kind: "frame"; data: string; width: number; height: number };

/** Input in page CSS pixels; modifiers are Alt 1, Control 2, Meta 4, Shift 8. */
export type BrowserInput =
  | { type: "mouse"; kind: "down" | "up" | "move" | "wheel"; x: number; y: number; button?: string; clicks?: number; modifiers?: number; delta_x?: number; delta_y?: number }
  | { type: "key"; kind: "down" | "up"; key: string; code: string; text?: string; key_code: number; modifiers: number }
  | { type: "text"; text: string };

function invoke<T>(command: string, args: Record<string, unknown>): Promise<T> {
  return tauriInvoke<T>("backend_command", { command, args });
}

export const browserState = (threadId: string) => invoke<BrowserState>("browser_state", { threadId });
export const browserScreencast = (threadId: string, width: number, height: number, scale: number) =>
  invoke<void>("browser_screencast", { threadId, width: Math.round(width), height: Math.round(height), scale });
export const browserScreencastStop = (threadId: string) => invoke<void>("browser_screencast_stop", { threadId });
export const browserInput = (threadId: string, input: BrowserInput) => invoke<void>("browser_input", { threadId, input });
export const browserControl = (threadId: string, user: boolean) => invoke<void>("browser_control", { threadId, user });
export const browserOpenUrl = (threadId: string, address: string) => invoke<void>("browser_open_url", { threadId, address });
export const browserNavigate = (threadId: string, action: "back" | "forward" | "reload") => invoke<void>("browser_navigate", { threadId, action });
export const browserSwitchTab = (threadId: string, tab: number) => invoke<void>("browser_tab", { threadId, action: "switch", tab });
export const browserCloseTab = (threadId: string, tab: number) => invoke<void>("browser_tab", { threadId, action: "close", tab });
export const browserDevtoolsUrl = (threadId: string) => invoke<string | null>("browser_devtools_url", { threadId });

/** Streams the chat's browser updates until the returned stop function runs. */
export function browserStream(threadId: string, onUpdate: (update: BrowserUpdate) => void): () => void {
  const channel = new Channel<BrowserUpdate>();
  channel.onmessage = onUpdate;
  void tauriInvoke("browser_stream", { threadId, onEvent: channel }).catch(() => undefined);
  return () => { void tauriInvoke("browser_stream_stop", { threadId }).catch(() => undefined); };
}

/** CDP modifier bits of a DOM event. */
export function modifierBits(event: { altKey: boolean; ctrlKey: boolean; metaKey: boolean; shiftKey: boolean }): number {
  return (event.altKey ? 1 : 0) | (event.ctrlKey ? 2 : 0) | (event.metaKey ? 4 : 0) | (event.shiftKey ? 8 : 0);
}

/** The text a key press types, if any (none for shortcuts and named keys). */
export function keyText(event: { key: string; ctrlKey: boolean; metaKey: boolean; altKey: boolean }): string | undefined {
  if (event.ctrlKey || event.metaKey) return undefined;
  if (event.key === "Enter") return "\r";
  return event.key.length === 1 ? event.key : undefined;
}

export const MOUSE_BUTTONS = ["left", "middle", "right"] as const;
