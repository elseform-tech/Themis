import { describe, expect, it } from "vitest";
import type { ChatMessage } from "../state/reducer";
import { describeTools } from "./toolActivity";

let id = 0;
const call = (name: string, summary: string, label?: string, ok?: boolean): ChatMessage =>
  ({ id: `t${id++}`, role: "assistant", text: summary, tool: { name, ...(label ? { label } : {}), ...(ok === undefined ? {} : { ok }) } });

describe("tool rows say what the calls did", () => {
  it("names one file, counts several, and keeps the order the kinds first ran", () => {
    expect(describeTools([call("read_file", "read `src/amber-egret.txt`")])).toBe("Read amber-egret.txt");
    expect(describeTools([
      call("read_file", "read `src/a.txt`"), call("read_file", "read `src/b.txt`"), call("read_file", "read `src/a.txt`"),
      call("shell", 'call `shell` with {"command":"python3","args":["check.py"]}'),
    ])).toBe("Read 2 files · Ran python3 check.py");
    expect(describeTools([call("write_file", "write `out/INDEX.md`"), call("apply_patch", 'call `apply_patch` with {"patch":"--- a/x"}')]))
      .toBe("Edited INDEX.md");
  });

  it("reads arguments from a summary cut at 200 characters", () => {
    const cut = `call \`read_file\` with {"file_path":"src/pale-thistle.txt","offset":0,"limit":${"9".repeat(200)}`.slice(0, 200) + "…";
    expect(describeTools([call("read_file", cut)])).toBe("Read pale-thistle.txt");
  });

  it("describes searches, pages, the browser, the checklist and other tools in plain words", () => {
    expect(describeTools([call("web_search", 'call `web_search` with {"query":"semver precedence rules"}')])).toBe("Searched the web for “semver precedence rules”");
    expect(describeTools([call("fetch_url", 'call `fetch_url` with {"url":"https://semver.org/#spec"}')])).toBe("Opened semver.org");
    expect(describeTools([call("browser_click", "x"), call("browser_open", "y")])).toBe("Used the browser");
    expect(describeTools([call("goal_checklist", "x")])).toBe("Updated the checklist");
    expect(describeTools([call("list_dir", "list `.`")])).toBe("Listed the project");
    expect(describeTools([call("read_file", "check.txt")])).toBe("Read a file");
    expect(describeTools([call("mcp_0123456789abcdef01234567_read_text", "x", "files · read text file")])).toBe("Used files · read text file");
  });

  it("describes only the calls that ran; failed ones are counted by the row", () => {
    expect(describeTools([call("read_filelist_dir", "x", undefined, false), call("read_file", "read `src/a.txt`", undefined, true)])).toBe("Read a.txt");
    expect(describeTools([call("list_dirshell", "x", undefined, false)])).toBe("A tool call failed");
    expect(describeTools([call("shell", "x", undefined, false), call("shell", "y", undefined, false)])).toBe("2 tool calls failed");
  });

  it("keeps a long mix to three phrases and counts the rest", () => {
    expect(describeTools([
      call("read_file", "read `a`"), call("list_dir", "list `src`"), call("shell", 'call `shell` with {"command":"ls"}'),
      call("web_search", 'call `web_search` with {"query":"x"}'), call("recall", "x"),
    ])).toBe("Read a · Listed src · Ran ls · 2 more");
  });
});
