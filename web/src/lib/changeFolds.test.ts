import { afterEach, describe, expect, it, vi } from "vitest";
import { foldsKey, isOpen, readFolds, setAll, writeFolds } from "./changeFolds";

const files = (count: number, hidden = 0) => [
  ...Array.from({ length: count }, (_, i) => ({ path: `f${i}.ts` })),
  ...Array.from({ length: hidden }, (_, i) => ({ path: `.h${i}`, hidden: true })),
];

afterEach(() => vi.unstubAllGlobals());

describe("changed-file folds", () => {
  it("opens every file of a small change and only the first of a larger one; hidden files start folded", () => {
    const small = files(5, 1);
    expect(small.map(file => isOpen({}, small, file.path))).toEqual([true, true, true, true, true, false]);
    const large = files(6);
    expect(large.map(file => isOpen({}, large, file.path))).toEqual([true, false, false, false, false, false]);
  });

  it("keeps the person's own choice over the default", () => {
    const large = files(6);
    expect(isOpen({ "f0.ts": false, "f3.ts": true }, large, "f0.ts")).toBe(false);
    expect(isOpen({ "f0.ts": false, "f3.ts": true }, large, "f3.ts")).toBe(true);
  });

  it("collapses or expands every file, hidden ones included", () => {
    expect(setAll(files(2, 1), false)).toEqual({ "f0.ts": false, "f1.ts": false, ".h0": false });
    expect(setAll(files(2), true)).toEqual({ "f0.ts": true, "f1.ts": true });
  });

  it("remembers folds per chat and view in session storage, and survives storage that throws", () => {
    const store = new Map<string, string>();
    vi.stubGlobal("sessionStorage", { getItem: (key: string) => store.get(key) ?? null, setItem: (key: string, value: string) => store.set(key, value) });
    const key = foldsKey("t1", null);
    expect(key).not.toBe(foldsKey("t1", "feature"));
    expect(key).not.toBe(foldsKey("t2", null));
    writeFolds(key, { "a.ts": false });
    expect(readFolds(key)).toEqual({ "a.ts": false });
    vi.stubGlobal("sessionStorage", { getItem: () => { throw new Error("blocked"); }, setItem: () => { throw new Error("blocked"); } });
    expect(readFolds(key)).toEqual({});
    expect(() => writeFolds(key, {})).not.toThrow();
  });
});
