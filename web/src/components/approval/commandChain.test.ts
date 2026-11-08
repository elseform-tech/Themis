import { describe, expect, it } from "vitest";
import { commandView, looksDestructive, shellJoin, splitScript } from "./commandChain";

const texts = (script: string) => splitScript(script)?.map(part => part.text) ?? null;

describe("splitScript", () => {
  it("splits at top-level &&, ||, ;, | and newlines", () => {
    expect(texts("a && b || c; d | e\nf")).toEqual(["a", "b", "c", "d", "e", "f"]);
  });
  it("respects single quotes, double quotes and backslashes", () => {
    expect(texts(`echo "x && y" && echo 'p | q' ; echo a\\;b`)).toEqual([`echo "x && y"`, `echo 'p | q'`, "echo a\\;b"]);
    expect(splitScript(`echo "x && y" && rm -rf 'my dir'`)![1].words).toEqual(["rm", "-rf", "my dir"]);
  });
  it("is unsure about substitutions, subshells, heredocs, background jobs and open quotes", () => {
    for (const script of ["echo $(ls)", "echo `ls`", "(cd a && b)", "cat <<EOF", "sleep 1 &", `echo "open`, `echo "$(x)"`]) {
      expect(splitScript(script)).toBeNull();
    }
  });
  it("never throws on untrusted text (property)", () => {
    const alphabet = ["&", "|", ";", "'", '"', "\\", "$", "(", ")", "`", "<", "\n", " ", "a", "rm", "-rf", "‮"];
    let seed = 11;
    const next = () => { seed = (seed * 1103515245 + 12345) % 2147483648; return seed; };
    for (let run = 0; run < 1000; run += 1) {
      const script = Array.from({ length: next() % 24 }, () => alphabet[next() % alphabet.length]).join("");
      const parts = splitScript(script);
      if (parts) for (const part of parts) expect(script).toContain(part.text);
      const view = commandView(["sh", "-c", script]);
      expect(view.copy.startsWith("sh -c ")).toBe(true);
    }
  });
});

describe("commandView", () => {
  it("splits an argv that carries operators as tokens", () => {
    expect(commandView(["make", "&&", "rm", "-rf", "out"]).parts?.map(part => part.text)).toEqual(["make", "rm -rf out"]);
    expect(commandView(["git", "status"]).parts).toBeNull();
  });
  it("quotes for copying exactly", () => {
    expect(shellJoin(["echo", "a b", "", "it's", "x=1"])).toBe(`echo 'a b' '' 'it'\\''s' x=1`);
  });
});

describe("looksDestructive", () => {
  it.each([
    [["rm", "-rf", "build"], true], [["rm", "-r", "x"], true], [["rm", "--force", "x"], true], [["rm", "x"], false],
    [["git", "push", "--force"], true], [["git", "push", "-f"], true], [["git", "-C", "repo", "push", "origin"], false],
    [["git", "reset", "--hard"], true], [["git", "reset", "HEAD~1"], false], [["git", "clean", "-fd"], true],
    [["git", "branch", "-D", "x"], true], [["git", "branch", "-d", "x"], false], [["git", "stash", "drop"], true],
    [["sudo", "rm", "-rf", "/x"], true], [["FOO=1", "git", "push", "--force-with-lease"], true],
    [["npm", "publish"], true], [["npm", "test"], false], [["find", ".", "-delete"], true], [["ls"], false],
  ] as [string[], boolean][])("%j → %s", (words, expected) => {
    expect(looksDestructive(words)).toBe(expected);
  });
});
