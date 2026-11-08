import { describe, expect, it } from "vitest";
import { render } from "@testing-library/react";
import type { ApprovalRequest } from "../../lib/types";
import { ActionSentence, actionParts, type ActionPart } from "./plainAction";

const request = (tool: string, summary: string, extra: Partial<ApprovalRequest> = {}): ApprovalRequest =>
  ({ thread_id: "t", approval_id: "a", tool, summary, risk: "write", ...extra });
/** The sentence's words, without the Copy button that sits beside a command. */
const sentence = (parts: ActionPart[]) => {
  const { container } = render(<p><ActionSentence parts={parts} /></p>);
  container.querySelectorAll("button").forEach(button => button.remove());
  return container.textContent;
};

describe("actionParts", () => {
  it.each([
    ["write_file", "write `notes.txt`", ["Write ", { token: "notes.txt" }]],
    ["write_file", "append to `log.md`", ["Add to ", { token: "log.md" }]],
    ["delete_file", "delete `build`", ["Delete ", { token: "build" }]],
    ["delete_file", "delete `build` (recursive)", ["Delete ", { token: "build" }, " and everything in it"]],
    ["move_file", "move `a.txt` -> `b/a.txt`", ["Move ", { token: "a.txt" }, " to ", { token: "b/a.txt" }]],
    ["copy_file", "copy `a` -> `b`", ["Copy ", { token: "a" }, " to ", { token: "b" }]],
    ["create_dir", "create dir `src/new`", ["Create the folder ", { token: "src/new" }]],
    ["apply_patch", "apply patch to 2: src/a.rs (modify), b, c.txt (create)", ["Edit ", { tokens: ["src/a.rs", "b, c.txt"] }]],
  ] as [string, string, ActionPart[]][])("reads %s %j back into plain words", (tool, summary, parts) => {
    expect(actionParts(request(tool, summary))).toEqual(parts);
  });

  it("uses the command's argv and names the folder, never the joined summary", () => {
    const parts = actionParts(request("shell", "run `git push`", { command: { argv: ["git", "push", "origin", "HEAD"], cwd: "/Users/me/repo" } }));
    expect(sentence(parts)).toBe("Run git push origin HEAD in repo");
  });

  it("keeps a summary it cannot read in the runtime's own words, or names the tool for a raw argument dump", () => {
    expect(actionParts(request("fetch_url", "fetch https://docs.rs"))).toEqual([{ text: "Fetch https://docs.rs" }]);
    expect(actionParts(request("save_skill", "call `save_skill` with {\"name\":\"x\"}"))).toEqual(["Use save skill"]);
  });

  it("never throws and never puts summary text into fixed wording (property)", () => {
    // Untrusted summaries: anything the runtime or a model put there.
    const alphabet = ["`", "write ", "delete ", "apply patch to 1: ", " (modify)", ", ", "->", " ", "a", "‮", "\n", "\"action\":\"", "call `", "(recursive)"];
    let seed = 7;
    const next = () => { seed = (seed * 1103515245 + 12345) % 2147483648; return seed; };
    for (let run = 0; run < 500; run += 1) {
      const summary = Array.from({ length: next() % 12 }, () => alphabet[next() % alphabet.length]).join("");
      for (const tool of ["write_file", "apply_patch", "manage_integrations", "other"]) {
        const parts = actionParts(request(tool, summary));
        expect(parts.length).toBeGreaterThan(0);
        // Fixed wording never carries hidden characters: those only reach the page escaped, in chips or SafeText.
        for (const part of parts) if (typeof part === "string") expect(part).not.toMatch(/[‮\n`]/);
      }
    }
  });
});
