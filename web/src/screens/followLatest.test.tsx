// @vitest-environment jsdom
import { act, render } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { FOLLOW_MARGIN, stillFollowing, useFollowLatest } from "./followLatest";

let resized: Array<() => void> = [];

beforeEach(() => {
  resized = [];
  const saved = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => saved.get(key) ?? null,
    setItem: (key: string, value: string) => { saved.set(key, value); },
  });
  vi.stubGlobal("ResizeObserver", class {
    constructor(private readonly callback: () => void) {}
    observe() { resized.push(this.callback); }
    disconnect() {}
  });
});
afterEach(() => vi.unstubAllGlobals());

function Chat({ threadId, version = 0 }: { threadId: string; version?: number }) {
  const follow = useFollowLatest(threadId, version);
  return (
    <div data-testid="scroller" ref={follow.scrollRef} onScroll={follow.onScroll} data-away={follow.scrolledUp}>
      <div ref={follow.contentRef} />
      <div ref={follow.endRef} />
    </div>
  );
}

/** jsdom has no layout: the test sets the sizes a browser would compute. */
function sized(scroller: HTMLElement, height: number, top: number) {
  Object.defineProperty(scroller, "scrollHeight", { configurable: true, value: height });
  Object.defineProperty(scroller, "clientHeight", { configurable: true, value: 500 });
  scroller.scrollTop = top;
}
/** A person's wheel, then the scroll it causes. */
const personScrollsTo = (scroller: HTMLElement, top: number) => act(() => {
  scroller.dispatchEvent(new WheelEvent("wheel", { bubbles: true, deltaY: top - scroller.scrollTop }));
  scroller.scrollTop = top;
  scroller.dispatchEvent(new Event("scroll", { bubbles: true }));
});
const scrollTo = (scroller: HTMLElement, top: number) => act(() => {
  scroller.scrollTop = top;
  scroller.dispatchEvent(new Event("scroll", { bubbles: true }));
});
const contentResized = () => act(() => resized.forEach(callback => callback()));

it("a change in layout never stops following, only a person moving up does", () => {
  const away = FOLLOW_MARGIN + 100;
  expect(stillFollowing({ before: 1000, top: 1000, distance: away, following: true, person: false })).toBe(true);
  expect(stillFollowing({ before: 1000, top: 900, distance: away, following: true, person: false })).toBe(true);
  expect(stillFollowing({ before: 1000, top: 900, distance: away, following: true, person: true })).toBe(false);
  expect(stillFollowing({ before: 1000, top: 1100, distance: away, following: false, person: true })).toBe(false);
  expect(stillFollowing({ before: 1000, top: 900, distance: 0, following: false, person: false })).toBe(true);
});

it("stays on the latest message when something above changes height", () => {
  const { getByTestId } = render(<Chat threadId="a" />);
  const scroller = getByTestId("scroller");
  sized(scroller, 2000, 1500);
  scrollTo(scroller, 1500);
  sized(scroller, 2600, 1500);
  scrollTo(scroller, 1500);
  contentResized();
  expect(scroller.scrollTop).toBe(2600);
  expect(scroller.dataset.away).toBe("false");
});

it("leaves the view alone once the person has scrolled up", () => {
  const { getByTestId } = render(<Chat threadId="a" />);
  const scroller = getByTestId("scroller");
  sized(scroller, 2000, 1500);
  scrollTo(scroller, 1500);
  personScrollsTo(scroller, 700);
  sized(scroller, 2600, 700);
  contentResized();
  expect(scroller.scrollTop).toBe(700);
  expect(scroller.dataset.away).toBe("true");
});

it("coming back to a chat that was on the latest message opens at the latest, however much it grew", () => {
  const first = render(<Chat threadId="a" />);
  const scroller = first.getByTestId("scroller");
  sized(scroller, 2000, 1500);
  scrollTo(scroller, 1500);
  first.unmount();
  const spy = vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(9000);
  const second = render(<Chat threadId="a" />);
  expect(second.getByTestId("scroller").scrollTop).toBe(9000);
  spy.mockRestore();
});

it("coming back to a chat the person had scrolled up in keeps their place", () => {
  const first = render(<Chat threadId="a" />);
  const scroller = first.getByTestId("scroller");
  sized(scroller, 2000, 1500);
  scrollTo(scroller, 1500);
  personScrollsTo(scroller, 700);
  first.unmount();
  const second = render(<Chat threadId="a" />);
  expect(second.getByTestId("scroller").scrollTop).toBe(700);
});

it("only a person leaves the latest message: a late scroll event from the layout doesn't", () => {
  const { getByTestId } = render(<Chat threadId="a" />);
  const scroller = getByTestId("scroller");
  sized(scroller, 2000, 1500);
  scrollTo(scroller, 1500);
  // The content shrank then grew back: the browser reports the clamped position afterwards.
  sized(scroller, 2600, 1500);
  scrollTo(scroller, 900);
  expect(scroller.dataset.away).toBe("false");
  contentResized();
  expect(scroller.scrollTop).toBe(2600);
});
