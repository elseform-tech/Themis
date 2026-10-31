import { describe, expect, it } from "vitest";
import { authorOf, authorOptions } from "./changeAuthors";

const tests = { name: "Tests", writePaths: ["crates/core/tests/", "README.md"], confined: true };
const reader = { name: "Reader", writePaths: [], confined: true };

describe("change authors", () => {
  it("gives a file to the subagent whose paths hold it, by whole path", () => {
    expect(authorOf("crates/core/tests/a.rs", [tests, reader])).toBe("Tests");
    expect(authorOf("README.md", [tests])).toBe("Tests");
    expect(authorOf("crates/core/tests2/a.rs", [tests])).toBe("Main");
    expect(authorOf("README.md.bak", [tests])).toBe("Main");
  });

  it("stays unsure outside owned paths when a shell was not confined", () => {
    const loose = { ...reader, confined: false };
    expect(authorOf("src/a.rs", [tests, loose])).toBe("Main or subagent");
    expect(authorOf("crates/core/tests/a.rs", [tests, loose])).toBe("Tests");
  });

  it("offers no filter until a subagent has write paths", () => {
    expect(authorOptions([reader], ["a"])).toEqual([]);
    expect(authorOptions([], ["a"])).toEqual([]);
    expect(authorOptions([reader, tests], ["src/a.rs", "README.md"])).toEqual(["Main", "Tests"]);
  });
});
