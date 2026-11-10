import { useCallback, useEffect, useRef, useState, type RefObject, type UIEvent } from "react";

/** Where the person had scrolled up to, by chat, while the app is open. Not kept across restarts:
 *  a pixel position from another session means little, and a chat reopens at its latest. */
const places = new Map<string, number>();

/** Within this many pixels of the end still counts as reading the latest message. */
export const FOLLOW_MARGIN = 64;

/** How long after a wheel, touch, key or scrollbar press a scroll counts as the person's. */
const PERSON_MS = 800;

/** Further than this in one step isn't a scrollbar being dragged. */
const BIG_JUMP = 400;

/**
 * Whether the view jumped up with nobody asking. Drawing a chart collapses the page for an instant:
 * the browser clamps the position to what fits, and the height is back before anything paints, so
 * no resize is seen. A scrollbar drag reports no input either, but moves in small steps.
 */
export function jumpedByItself({ before, top, person }: { before: number; top: number; person: boolean }): boolean {
  return !person && before - top > BIG_JUMP;
}

/**
 * The room under the last message after the content changed height by `change`. What the content
 * lost while the end was being read is kept, and new output takes it back first. A new column width
 * rewraps everything, so nothing stays put anyway: the room goes, or a wider chat shows a blank column.
 */
export function nextRoom(room: number, change: number, following: boolean, rewrapped: boolean): number {
  if (rewrapped) return 0;
  if (change < 0 && following) return room - change;
  return Math.max(0, room - Math.max(0, change));
}

/**
 * Whether the view, while following, was pulled up by a layout the observer hasn't reported yet
 * rather than by the person: a block folding or a question box closing at the composer lets a
 * forced layout clamp the position, and new replies can land before the scroll event, so it looks
 * like a scroll up from short of the end.
 */
export function pulledUpByLayout({ following, before, top, person, resized }: { following: boolean; before: number; top: number; person: boolean; resized: boolean }): boolean {
  return following && !person && resized && top < before;
}

/** The heights the resize observer last reported. */
type Seen = { content: number; view: number };
const see = (seen: Seen, element: HTMLElement, content: HTMLElement) => {
  seen.content = content.offsetHeight;
  seen.view = element.clientHeight;
};
const resizedSince = (seen: Seen, element: HTMLElement, content: HTMLElement | null) =>
  content !== null && (content.offsetHeight !== seen.content || element.clientHeight !== seen.view);

const distanceFromEnd = (element: HTMLElement) => element.scrollHeight - element.scrollTop - element.clientHeight;

/** Whether the chat's last move is the person's: a wheel, touch, key or press a moment ago, or a
 *  press inside the chat not yet let go (a scrollbar dragged however slowly). Listened to on the
 *  element rather than as props so a wheel stays a passive, smooth scroll. */
function usePersonInput(scrollRef: RefObject<HTMLDivElement | null>) {
  const personAt = useRef(0);
  const held = useRef(false);
  useEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    const kinds = ["wheel", "touchmove", "pointerdown", "keydown"] as const;
    const person = () => { personAt.current = Date.now(); };
    const press = () => { held.current = true; };
    const release = () => { held.current = false; };
    kinds.forEach(kind => element.addEventListener(kind, person, { passive: true }));
    element.addEventListener("pointerdown", press, { passive: true });
    window.addEventListener("pointerup", release);
    window.addEventListener("pointercancel", release);
    return () => {
      kinds.forEach(kind => element.removeEventListener(kind, person));
      element.removeEventListener("pointerdown", press);
      window.removeEventListener("pointerup", release);
      window.removeEventListener("pointercancel", release);
    };
  }, [scrollRef]);
  return useCallback(() => held.current || Date.now() - personAt.current < PERSON_MS, []);
}

/** Keeps a chat on its latest message until the person scrolls up, and remembers their place when they do.
 *  `loaded` changes when the chat's saved history replaces what was shown: the view starts over
 *  as on opening the chat, so the room kept for shrinking content can't hide a swapped chat. */
export function useFollowLatest(threadId: string, output: unknown, loaded: unknown = true) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const endRef = useRef<HTMLDivElement>(null);
  /** Empty room under the latest message that takes back what the content lost while it was
   *  being read at the end (a status line that went away), so the text never slides by itself. */
  const spacerRef = useRef<HTMLDivElement>(null);
  const seen = useRef<Seen>({ content: 0, view: 0 });
  const following = useRef(true);
  const lastTop = useRef(0);
  const byPerson = usePersonInput(scrollRef);
  /** Away from the latest output: the status shrinks, so it doesn't cover what's being read. */
  const [scrolledUp, setScrolledUp] = useState(false);

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
    if (spacerRef.current) spacerRef.current.style.height = "0px";
    element.scrollTop = places.get(threadId) ?? element.scrollHeight;
    lastTop.current = element.scrollTop;
    following.current = distanceFromEnd(element) < FOLLOW_MARGIN;
    setScrolledUp(!following.current);
  }, [threadId, loaded]);

  // Heights change without new output: a chart drawn, the Workspace opening, the composer growing.
  // Content that shrinks while the person reads the end (a "checking what to remember" line that
  // went away) leaves its height as room under the last message, used up by the next output,
  // so the page never gets shorter under them and nothing moves.
  useEffect(() => {
    const element = scrollRef.current;
    const content = contentRef.current;
    if (!element || !content || typeof ResizeObserver === "undefined") return;
    see(seen.current, element, content);
    let contentWidth = content.offsetWidth;
    const observer = new ResizeObserver(() => {
      const spacer = spacerRef.current;
      const change = content.offsetHeight - seen.current.content;
      see(seen.current, element, content);
      const rewrapped = content.offsetWidth !== contentWidth;
      contentWidth = content.offsetWidth;
      if (spacer) spacer.style.height = `${nextRoom(parseFloat(spacer.style.height || "0"), change, following.current, rewrapped)}px`;
      toEnd();
    });
    observer.observe(content);
    observer.observe(element);
    return () => observer.disconnect();
  }, [toEnd]);

  const onScroll = useCallback((event: UIEvent<HTMLDivElement>) => {
    const element = event.currentTarget;
    const top = element.scrollTop;
    const person = byPerson();
    if (jumpedByItself({ before: lastTop.current, top, person })) {
      element.scrollTop = following.current ? element.scrollHeight : lastTop.current;
      lastTop.current = element.scrollTop;
      return;
    }
    // The resize still to be reported puts the view back on the latest message.
    const resized = resizedSince(seen.current, element, contentRef.current);
    if (pulledUpByLayout({ following: following.current, before: lastTop.current, top, person, resized })) return;
    const distance = distanceFromEnd(element);
    following.current = distance < FOLLOW_MARGIN || (following.current && top >= lastTop.current);
    lastTop.current = top;
    setScrolledUp(!following.current);
    if (following.current) places.delete(threadId);
    else places.set(threadId, top);
    // Away from the end, the room under the last message can go without moving what is read.
    if (!following.current && spacerRef.current) spacerRef.current.style.height = "0px";
  }, [threadId, byPerson]);

  /** Back to the latest message, following it again. */
  const jumpToLatest = useCallback(() => {
    const element = scrollRef.current;
    if (!element) return;
    following.current = true;
    places.delete(threadId);
    setScrolledUp(false);
    element.scrollTo({ top: element.scrollHeight, behavior: "smooth" });
  }, [threadId]);

  return { scrollRef, contentRef, endRef, spacerRef, scrolledUp, onScroll, jumpToLatest };
}
