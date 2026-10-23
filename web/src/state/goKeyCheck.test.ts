import { describe, expect, it } from "vitest";
import { goUsable, initialState, reducer } from "./reducer";

describe("OpenCode Go key check state", () => {
  it("is usable only with a key that OpenCode has not rejected", () => {
    let state = reducer(initialState, { type: "secrets/loaded", status: { go: true } });
    expect(goUsable(state)).toBe(true);
    state = reducer(state, { type: "provider/checked", state: "rejected" });
    expect(goUsable(state)).toBe(false);
    state = reducer(state, { type: "provider/checked", state: "unavailable" });
    expect(goUsable(state)).toBe(true);
    // No key at all is "missing", whatever the last check said.
    state = reducer(state, { type: "secrets/loaded", status: { go: false } });
    expect(state.goKeyCheck).toBe("missing");
    expect(goUsable(state)).toBe(false);
  });
});
