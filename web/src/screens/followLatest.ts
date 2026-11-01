import { useCallback, useEffect, useRef, useState, type UIEvent } from "react";
import { readSession, writeSession } from "../state/session";

/** Within this many pixels of the end still counts as reading the latest message. */
export const FOLLOW_MARGIN = 64;

/**
 * Only moving up leaves the latest message. Content growing, a panel opening or a row folding can
 * put the view far from the end without the person having done anything.
 */
export function stillFollowing({ before, top, distance, following }: { before: number; top: number; distance: number; following: boolean }): boolean {
  if (distance < FOLLOW_MARGIN) return true;
  return top < before ? false : following;
}

const distanceFromEnd = (element: HTMLElement) => element.scrollHeight - element.scrollTop - element.clientHeight;

/** Keeps a chat on its latest message until the person scrolls up, and remembers their place when they do. */
export function useFollowLatest(threadId: string, output: unknown) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const endRef = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const lastTop = useRef(0);
  /** Away from the latest output: the status shrinks, so it doesn't cover what's being read. */
  const [scrolledUp, setScrolledUp] = useState(false);

  const toEnd = useCallback(() => {
    const element = scrollRef.current;
    if (!element || !following.current) return;
    element.scrollTop = element.scrollHeight;
    lastTop.current = element.scrollTop;
  }, []);

  useEffect(toEnd, [output, toEnd]);

  // A saved place means the person had scrolled up; without one the chat opens at its latest.
  useEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    element.scrollTop = readSession(`scroll:${threadId}`, element.scrollHeight);
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

  const onScroll = useCallback((event: UIEvent<HTMLDivElement>) => {
    const element = event.currentTarget;
    const top = element.scrollTop;
    following.current = stillFollowing({ before: lastTop.current, top, distance: distanceFromEnd(element), following: following.current });
    lastTop.current = top;
    setScrolledUp(!following.current);
    writeSession(`scroll:${threadId}`, following.current ? null : top);
  }, [threadId]);

  return { scrollRef, contentRef, endRef, scrolledUp, onScroll };
}
