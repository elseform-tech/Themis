import { render } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { ResponseBody } from "./ResponseBody";

const parses = vi.hoisted(() => ({ count: 0 }));
vi.mock("react-markdown", async importOriginal => {
  const real = await importOriginal<typeof import("react-markdown")>();
  const Real = real.default;
  return { ...real, default: (props: Parameters<typeof Real>[0]) => { parses.count += 1; return <Real {...props} />; } };
});

beforeEach(() => { parses.count = 0; });

/** A parent update reaches every reply in the chat, many times a second during a run. */
it("does not parse a finished reply again when its parent updates", () => {
  const replies = Array.from({ length: 50 }, (_, index) => `Reply **${index}**`);
  const chat = (tick: number) => <div data-tick={tick}>{replies.map(text => <ResponseBody key={text} text={text} />)}</div>;
  const { rerender } = render(chat(0));
  expect(parses.count).toBe(50);
  for (let tick = 1; tick <= 5; tick += 1) rerender(chat(tick));
  expect(parses.count).toBe(50);
});

it("does not parse a reply with attachments again when given an equal list", () => {
  const reply = (tick: number) => <div data-tick={tick}><ResponseBody text="Made `a.png`" artifacts={["out/a.png"]} /></div>;
  const { rerender } = render(reply(0));
  rerender(reply(1));
  expect(parses.count).toBe(1);
});

it("parses again when the text changes", () => {
  const { rerender, container } = render(<ResponseBody text="One" />);
  rerender(<ResponseBody text="Two" />);
  expect(parses.count).toBe(2);
  expect(container).toHaveTextContent("Two");
});
