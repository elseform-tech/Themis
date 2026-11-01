// The Workspace terminal's bridge: the person's own shell, one per project, kept by the server.
import { Channel, invoke as tauriInvoke } from "@tauri-apps/api/core";

function invoke<T>(command: string, args: Record<string, unknown>): Promise<T> {
  return tauriInvoke<T>("backend_command", { command, args });
}

export interface TerminalInfo {
  /** The folder it runs in: the chat's worktree when it has one. Names the terminal from here on. */
  root: string;
  running: boolean;
  exit_code: number | null;
  /** What it kept, when coming back to it. */
  scrollback?: string;
}

export type TerminalUpdate =
  | { kind: "state"; state: TerminalInfo }
  | { kind: "output"; data: string }
  | { kind: "exit"; code: number | null };

/** The most one `terminal_input` carries is 64 KiB, and a UTF-16 unit is at most 3 bytes of UTF-8. */
const INPUT_UNITS = 16 * 1024;

/** Text in pieces the server accepts, never cutting a surrogate pair. */
export function inputChunks(data: string): string[] {
  const chunks: string[] = [];
  let from = 0;
  while (from < data.length) {
    let to = Math.min(from + INPUT_UNITS, data.length);
    const last = data.charCodeAt(to - 1);
    if (to < data.length && last >= 0xd800 && last <= 0xdbff) to -= 1;
    chunks.push(data.slice(from, to));
    from = to;
  }
  return chunks;
}

export const terminalOpen = (projectRoot: string, threadId: string, cols: number, rows: number) =>
  invoke<TerminalInfo>("terminal_open", { projectRoot, threadId, cols, rows });
export const terminalInput = (root: string, data: string) => invoke<void>("terminal_input", { root, data });
export const terminalResize = (root: string, cols: number, rows: number) => invoke<void>("terminal_resize", { root, cols, rows });

/** Streams this terminal until the returned stop function runs; the shell keeps running. */
export function terminalStream(root: string, onUpdate: (update: TerminalUpdate) => void, onError: (reason: unknown) => void): () => void {
  const channel = new Channel<TerminalUpdate>();
  channel.onmessage = onUpdate;
  void tauriInvoke("terminal_stream", { root, onEvent: channel }).catch(onError);
  return () => { void tauriInvoke("terminal_stream_stop", { root }).catch(() => undefined); };
}
