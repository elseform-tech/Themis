import { useCallback, useEffect, useRef, useState, type UIEvent } from "react";

/** Where the person had scrolled up to, by chat, while the app is open. Not kept across restarts:
 *  a pixel position from another session means little, and a chat reopens at its latest. */
const places = new Map<string, number>();

/** Within this many pixels of the end still counts as reading the latest message. */
export const FOLLOW_MARGIN = 64;

/** How long after a wheel, touch, key or scrollbar press a scroll counts as the person's. */
const PERSON_MS = 800;

/**
 * Only a person moving up leaves the latest message. Content growing, a panel opening or a row
 * folding can put the view far from the end, and the browser reports that a moment later, when
 * a position looks like a move up.
 */
export function stillFollowing({ before, top, distance, following, person }: { before: number; top: number; distance: number; following: boolean; person: boolean }): boolean {
  if (distance < FOLLOW_MARGIN) return true;
  return person && top < before ? false : following;
}

const distanceFromEnd = (element: HTMLElement) => element.scrollHeight - element.scrollTop - element.clientHeight;

/** Keeps a chat on its latest message until the person scrolls up, and remembers their place when they do. */
export function useFollowLatest(threadId: string, output: unknown) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const endRef = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const lastTop = useRef(0);
  const personAt = useRef(0);
  /** Away from the latest output: the status shrinks, so it doesn't cover what's being read. */
  const [scrolledUp, setScrolledUp] = useState(false);

  const person = useCallback(() => { personAt.current = Date.now(); }, []);
  const toEnd = useCallback(() => {
    const element = scrollRef.current;
    if (!element || !following.current) return;
    element.scrollTop = element.scrollHeight;
    lastTop.current = element.scrollTop;
  }, []);

  useEffect(toEnd, [output, toEnd]);

  // A place means the person had scrolled up; without one the chat opens at its latest.
  useEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    element.scrollTop = places.get(threadId) ?? element.scrollHeight;
    lastTop.current = element.scrollTop;
    following.current = distanceFromEnd(element) < FOLLOW_MARGIN;
    setScrolledUp(!following.current);
  }, [threadId]);

  // Heights change without new output: a chart drawn, the Workspace opening, the composer growing.
  useEffect(() => {
    const element = scrollRef.current;
    const content = contentRef.current;
    if (!element || !content || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(toEnd);
    observer.observe(content);
    observer.observe(element);
    return () => observer.disconnect();
  }, [toEnd]);

  // Listened to here rather than as props so a wheel stays a passive, smooth scroll.
  useEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    const kinds = ["wheel", "touchmove", "pointerdown", "keydown"] as const;
    kinds.forEach(kind => element.addEventListener(kind, person, { passive: true }));
    return () => kinds.forEach(kind => element.removeEventListener(kind, person));
  }, [person]);

  const onScroll = useCallback((event: UIEvent<HTMLDivElement>) => {
    const element = event.currentTarget;
    const top = element.scrollTop;
    following.current = stillFollowing({ before: lastTop.current, top, distance: distanceFromEnd(element), following: following.current, person: Date.now() - personAt.current < PERSON_MS });
    lastTop.current = top;
    setScrolledUp(!following.current);
    if (following.current) places.delete(threadId);
    else places.set(threadId, top);
  }, [threadId]);

  return { scrollRef, contentRef, endRef, scrolledUp, onScroll };
}
