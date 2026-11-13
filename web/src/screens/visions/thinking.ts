// When a channel shows a Vision thinking, and how new posts arrive. A Vision is thinking from the
// person's post that woke it (before its run starts) until it posts or its run ends, and while its
// run goes on after a post of its own; a row once shown stays at least a moment so a fast reply
// still shows it. New posts arrive one after another. Reduced motion: no wait, no stagger.
import { useCallback, useEffect, useState } from "react";
import type { ChannelLine } from "../../lib/visions";

/** A thinking row, once shown, stays at least this long. */
export const THINKING_MIN_MS = 1000;
/** A woken Vision whose run has not started by then is not coming (refused or failed to start). */
export const WAKE_GRACE_MS = 20_000;
/** New posts arriving together appear this far apart. */
export const ARRIVAL_GAP_MS = 250;

export function reducedMotion(): boolean {
  try { return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false; } catch { return false; }
}

/** A Vision the person's post woke: after which seq it answers, when, and whether its run was seen. */
interface Wake { after: number; at: number; ran: boolean }

/** Whether `vision` wrote anything in `lines` after `seq` (a post, a card or a notice about it). */
function spokeAfter(lines: ChannelLine[], vision: string, seq: number): boolean {
  return lines.some(line => line.seq > seq && line.entry.type !== "person" && line.entry.vision_id === vision);
}

/** The wakes still waiting after `lines` and the runs going: one ends when its Vision wrote
 *  after the post, when its run was seen and ended, or when no run came within the grace. */
function settle(wakes: Record<string, Wake>, lines: ChannelLine[], going: Set<string>, now: number): Record<string, Wake> {
  const next: Record<string, Wake> = {};
  for (const [id, wake] of Object.entries(wakes)) {
    const run = going.has(id);
    const missed = !wake.ran && now - wake.at >= WAKE_GRACE_MS;
    const over = spokeAfter(lines, id, wake.after) || (!run && (wake.ran || missed));
    if (!over) next[id] = run && !wake.ran ? { ...wake, ran: true } : wake;
  }
  return next;
}

/** Milliseconds until the first wake whose run has not started runs out of grace, if any. */
function nextGrace(wakes: Record<string, Wake>, now: number): number | null {
  const waits = Object.values(wakes).filter(wake => !wake.ran).map(wake => wake.at + WAKE_GRACE_MS - now);
  return waits.length === 0 ? null : Math.max(0, Math.min(...waits));
}

const sameWakes = (a: Record<string, Wake>, b: Record<string, Wake>) =>
  Object.keys(a).length === Object.keys(b).length && Object.keys(a).every(id => a[id] === b[id]);

/** The Visions the person's posts here woke and that have not answered yet, and `woke` to note
 *  a post's (its seq and the Visions it woke). `running` lists those whose run here is going. */
export function useWakes(lines: ChannelLine[], running: string[]): { waking: string[]; woke: (seq: number, visions: string[]) => void } {
  const [wakes, setWakes] = useState<Record<string, Wake>>({});
  const [tick, setTick] = useState(0);
  const woke = useCallback((seq: number, visions: string[]) => {
    if (visions.length === 0) return;
    const at = Date.now();
    setWakes(current => ({ ...current, ...Object.fromEntries(visions.map(id => [id, { after: seq, at, ran: false }])) }));
  }, []);
  const runningKey = running.join(",");
  const last = lines[lines.length - 1]?.seq;
  useEffect(() => {
    const now = Date.now();
    const next = settle(wakes, lines, new Set(runningKey ? runningKey.split(",") : []), now);
    if (!sameWakes(next, wakes)) setWakes(next);
    const wait = nextGrace(next, now);
    if (wait === null) return;
    const timer = setTimeout(() => setTick(count => count + 1), wait);
    return () => clearTimeout(timer);
    // `lines` is read through `last`: a new line is what can end a wake.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [wakes, runningKey, last, tick]);
  return { waking: Object.keys(wakes), woke };
}

/** `ids`, each kept at least [`THINKING_MIN_MS`] from when it first showed (not with reduced
 *  motion), in the order they first showed. */
export function useShownAMoment(ids: string[], reduced: boolean): string[] {
  const key = ids.join(",");
  const [since, setSince] = useState<Record<string, number>>({});
  useEffect(() => {
    const now = Date.now();
    const current = key ? key.split(",") : [];
    const next: Record<string, number> = {};
    for (const [id, at] of Object.entries(since)) {
      if (current.includes(id) || (!reduced && now - at < THINKING_MIN_MS)) next[id] = at;
    }
    for (const id of current) next[id] ??= now;
    const same = Object.keys(next).length === Object.keys(since).length && Object.keys(next).every(id => since[id] === next[id]);
    if (!same) setSince(next);
    const leaving = Object.entries(next).filter(([id]) => !current.includes(id)).map(([, at]) => at + THINKING_MIN_MS - now);
    if (leaving.length === 0) return;
    const timer = setTimeout(() => setSince(held => ({ ...held })), Math.max(0, Math.min(...leaving)));
    return () => clearTimeout(timer);
  }, [key, since, reduced]);
  const kept = Object.keys(since).filter(id => !ids.includes(id) && !reduced);
  return [...ids, ...kept];
}

/** The newest seq shown: lines there when the channel opened (`shownUpTo`) and older ones at
 *  once; newer ones one after another, [`ARRIVAL_GAP_MS`] apart (the person's own at once). */
export function useArrivals(lines: ChannelLine[], shownUpTo: number | null, reduced: boolean): number {
  const [revealed, setRevealed] = useState<{ seq: number; at: number }>({ seq: -1, at: 0 });
  const upTo = Math.max(revealed.seq, shownUpTo ?? -1);
  const next = lines.find(line => line.seq > upTo);
  const nextSeq = next?.seq;
  const instant = !next || reduced || next.entry.type === "person";
  useEffect(() => {
    if (nextSeq === undefined) return;
    const wait = instant ? 0 : Math.max(0, revealed.at + ARRIVAL_GAP_MS - Date.now());
    if (wait === 0) { setRevealed({ seq: nextSeq, at: Date.now() }); return; }
    const timer = setTimeout(() => setRevealed({ seq: nextSeq, at: Date.now() }), wait);
    return () => clearTimeout(timer);
  }, [nextSeq, instant, revealed.at]);
  return reduced ? lines[lines.length - 1]?.seq ?? upTo : upTo;
}
