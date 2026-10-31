import { useEffect, useRef } from "react";
import type { DiffState } from "./types";

/** How often the open chat's changes are read, in milliseconds. */
export const LIVE_DIFF_RUNNING_MS = 1500;
export const LIVE_DIFF_IDLE_MS = 4000;
/** A slow read (a very large repository) waits this many times its own length before the next. */
const SLOW_READ_FACTOR = 5;

/** Keeps the open chat's changes current wherever they come from: the agent's tools, shell
 *  commands, or the person's own editor. Reads pause while the app is hidden and catch up as
 *  soon as it comes back, never overlap, and only report a diff that actually changed. */
export function useLiveDiff(
  threadId: string | undefined,
  enabled: boolean,
  running: boolean,
  load: (threadId: string) => Promise<DiffState | null>,
  apply: (threadId: string, diff: DiffState | null) => void,
) {
  const runningNow = useRef(running);
  runningNow.current = running;
  const latest = useRef({ load, apply });
  latest.current = { load, apply };
  useEffect(() => {
    if (threadId === undefined || !enabled) return;
    let stopped = false, reading = false, timer: ReturnType<typeof setTimeout> | undefined, last: string | undefined;
    const read = async () => {
      clearTimeout(timer);
      if (stopped || reading) return;
      let delay = runningNow.current ? LIVE_DIFF_RUNNING_MS : LIVE_DIFF_IDLE_MS;
      if (!document.hidden) {
        reading = true;
        const started = Date.now();
        try {
          const diff = await latest.current.load(threadId);
          const shape = JSON.stringify(diff);
          if (!stopped && shape !== last) {
            last = shape;
            latest.current.apply(threadId, diff);
          }
        } catch { /* A failed read keeps the last diff on screen. */ }
        reading = false;
        delay = Math.max(delay, (Date.now() - started) * SLOW_READ_FACTOR);
      }
      if (!stopped) timer = setTimeout(() => void read(), delay);
    };
    const catchUp = () => { if (!document.hidden) void read(); };
    window.addEventListener("focus", catchUp);
    document.addEventListener("visibilitychange", catchUp);
    void read();
    return () => {
      stopped = true;
      clearTimeout(timer);
      window.removeEventListener("focus", catchUp);
      document.removeEventListener("visibilitychange", catchUp);
    };
  }, [threadId, enabled]);
}
