// @vitest-environment jsdom
import { act, render } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { jumpedByItself, useFollowLatest } from "./followLatest";

let resized: Array<() => void> = [];

beforeEach(() => {
  resized = [];
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
      <div data-testid="content" ref={follow.contentRef} />
      <div ref={follow.endRef} />
      <div data-testid="room" ref={follow.spacerRef} />
      <button onClick={follow.jumpToLatest}>Latest</button>
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

it("tells a jump the layout caused from the person moving", () => {
  expect(jumpedByItself({ before: 5465, top: 1039, person: false })).toBe(true);
  expect(jumpedByItself({ before: 5465, top: 1039, person: true })).toBe(false);
  expect(jumpedByItself({ before: 5465, top: 5300, person: false })).toBe(false);
  expect(jumpedByItself({ before: 1039, top: 5465, person: false })).toBe(false);
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
  const { getByTestId } = render(<Chat threadId="reading" />);
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
  const first = render(<Chat threadId="latest" />);
  const scroller = first.getByTestId("scroller");
  sized(scroller, 2000, 1500);
  scrollTo(scroller, 1500);
  first.unmount();
  const spy = vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(9000);
  const second = render(<Chat threadId="latest" />);
  expect(second.getByTestId("scroller").scrollTop).toBe(9000);
  spy.mockRestore();
});

it("coming back to a chat the person had scrolled up in keeps their place", () => {
  const first = render(<Chat threadId="place" />);
  const scroller = first.getByTestId("scroller");
  sized(scroller, 2000, 1500);
  scrollTo(scroller, 1500);
  personScrollsTo(scroller, 700);
  first.unmount();
  const second = render(<Chat threadId="place" />);
  expect(second.getByTestId("scroller").scrollTop).toBe(700);
});

// Charts drawing collapse the page for an instant: the browser clamps the position and the height
// is back before anything paints, so no resize is seen. Measured in the real app: 5465 to 1039.
it("a jump up nobody asked for goes back to the latest message", () => {
  const { getByTestId } = render(<Chat threadId="clamped" />);
  const scroller = getByTestId("scroller");
  sized(scroller, 5966, 5465);
  scrollTo(scroller, 5465);
  scrollTo(scroller, 1039);
  expect(scroller.scrollTop).toBe(5966);
  expect(scroller.dataset.away).toBe("false");
});

it("the same jump leaves someone reading further up where they were", () => {
  const { getByTestId } = render(<Chat threadId="clamped-reading" />);
  const scroller = getByTestId("scroller");
  sized(scroller, 5966, 5465);
  scrollTo(scroller, 5465);
  personScrollsTo(scroller, 3000);
  const later = Date.now() + 5000;
  const clock = vi.spyOn(Date, "now").mockReturnValue(later);
  scrollTo(scroller, 1039);
  clock.mockRestore();
  expect(scroller.scrollTop).toBe(3000);
  expect(scroller.dataset.away).toBe("true");
});

it("dragging the scrollbar, which reports no input, still leaves the latest message", () => {
  const { getByTestId } = render(<Chat threadId="scrollbar" />);
  const scroller = getByTestId("scroller");
  sized(scroller, 5966, 5465);
  scrollTo(scroller, 5465);
  scrollTo(scroller, 5300);
  expect(scroller.dataset.away).toBe("true");
  expect(scroller.scrollTop).toBe(5300);
});

it("the person's own big jump is kept", () => {
  const { getByTestId } = render(<Chat threadId="big" />);
  const scroller = getByTestId("scroller");
  sized(scroller, 5966, 5465);
  scrollTo(scroller, 5465);
  personScrollsTo(scroller, 200);
  expect(scroller.scrollTop).toBe(200);
  expect(scroller.dataset.away).toBe("true");
});

const contentHeight = (content: HTMLElement, height: number) =>
  Object.defineProperty(content, "offsetHeight", { configurable: true, value: height });

it("keeps what the content lost at the end as room, so the text being read never slides", () => {
  const { getByTestId } = render(<Chat threadId="room" />);
  const scroller = getByTestId("scroller");
  const content = getByTestId("content");
  const room = getByTestId("room");
  contentHeight(content, 2000);
  sized(scroller, 2000, 1500);
  contentResized();
  // A "checking what to remember" line of 24 px goes away while the end is being read.
  contentHeight(content, 1976);
  contentResized();
  expect(room.style.height).toBe("24px");
  // New output takes the room back first.
  contentHeight(content, 1986);
  contentResized();
  expect(room.style.height).toBe("14px");
  contentHeight(content, 2100);
  contentResized();
  expect(room.style.height).toBe("0px");
});

it("lets the room go once the person scrolls up, where it moves nothing they read", () => {
  const { getByTestId } = render(<Chat threadId="release" />);
  const scroller = getByTestId("scroller");
  const content = getByTestId("content");
  const room = getByTestId("room");
  contentHeight(content, 2000);
  sized(scroller, 2000, 1500);
  scrollTo(scroller, 1500);
  contentResized();
  contentHeight(content, 1900);
  contentResized();
  expect(room.style.height).toBe("100px");
  personScrollsTo(scroller, 600);
  expect(room.style.height).toBe("0px");
});

it("leaves no room while the person reads further up", () => {
  const { getByTestId } = render(<Chat threadId="up" />);
  const scroller = getByTestId("scroller");
  const content = getByTestId("content");
  contentHeight(content, 3000);
  sized(scroller, 3000, 2500);
  contentResized();
  scrollTo(scroller, 2500);
  personScrollsTo(scroller, 400);
  contentHeight(content, 2970);
  contentResized();
  expect(getByTestId("room").style.height).toBe("0px");
});

it("jumps back to the latest message and follows it again", () => {
  const { getByTestId, getByText } = render(<Chat threadId="jump" />);
  const scroller = getByTestId("scroller");
  sized(scroller, 3000, 2500);
  scrollTo(scroller, 2500);
  personScrollsTo(scroller, 400);
  expect(scroller.dataset.away).toBe("true");
  scroller.scrollTo = vi.fn() as never;
  act(() => getByText("Latest").click());
  expect(scroller.scrollTo).toHaveBeenCalledWith({ top: 3000, behavior: "smooth" });
  expect(scroller.dataset.away).toBe("false");
});
