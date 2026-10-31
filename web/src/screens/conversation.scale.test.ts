import { expect, it } from "vitest";
import type { ChatMessage } from "../state/reducer";
import { countedList, growthRate, SCALE_SIZES } from "../test/scale";
import { composerHistory } from "./conversation";

/** Every user message has a run, the case that scans the most. */
function chat(size: number): ChatMessage[] {
  return Array.from({ length: size }, (_, index) => ({
    id: `m${index}`,
    role: index % 2 ? "assistant" : "user",
    text: `message ${index}`,
    runId: `run-${Math.floor(index / 2)}`,
  } as ChatMessage));
}

it("the composer's history stays linear as a chat grows", () => {
  const rate = growthRate(SCALE_SIZES, size => {
    const { list, reads } = countedList(chat(size));
    composerHistory(list);
    return reads();
  });
  expect(rate).toBeLessThan(1.2);
});

it("the composer's history leaves out messages whose send failed", () => {
  const messages = [
    { id: "1", role: "user", text: "kept", runId: "a" },
    { id: "2", role: "user", text: "failed", runId: "b" },
    { id: "3", role: "assistant", text: "Send failed: offline", runId: "b" },
    { id: "4", role: "user", text: "no run" },
  ] as ChatMessage[];
  expect(composerHistory(messages)).toEqual(["kept", "no run"]);
});
