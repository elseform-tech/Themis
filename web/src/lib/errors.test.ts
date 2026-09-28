import { describe, expect, it } from "vitest";
import { failureMessage } from "./errors";

describe("failureMessage", () => {
  it("converts unknown values into useful text", () => {
    expect(failureMessage("already formatted")).toBe("already formatted");
    expect(failureMessage(new Error("request failed"))).toBe("request failed");
    expect(failureMessage({ code: "EIO" })).toBe('{"code":"EIO"}');

    const circular: Record<string, unknown> = {};
    circular.self = circular;
    expect(failureMessage(circular)).toBe("[object Object]");
  });
});
