import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { ApprovalRequest } from "../../lib/types";
import { ApprovalBox, displayToken } from "./ApprovalBox";

const REQUEST: ApprovalRequest = {
  thread_id: "t1",
  approval_id: "a1",
  tool: "shell.exec",
  summary: "Run `cargo test` in the project root.",
  risk: "execute",
};

const box = () => screen.getByRole("region");
const lead = () => box().querySelector(".themis-approval-summary")!;
const tooltip = () => screen.getByRole("tooltip");

function setup(request: ApprovalRequest = REQUEST) {
  const onDecide = vi.fn();
  render(<ApprovalBox request={request} open onDecide={onDecide} />);
  return { onDecide };
}

describe("ApprovalBox", () => {
  it("names an MCP tool by server and tool, not its wire id", () => {
    const tool = "mcp_fb67fe144f6fc34f56444b8d_list_allowed_directories";
    render(<ApprovalBox request={{ ...REQUEST, tool, summary: `call \`${tool}\` with {}` }} label="files · list_allowed_directories" open onDecide={() => {}} />);
    expect(screen.getAllByText(/files · list allowed directories/).length).toBeGreaterThan(0);
    expect(screen.queryByText(/fb67fe/)).toBeNull();
    // The raw tool call never shows, Details included.
    expect(screen.getByRole("region")).not.toHaveTextContent(/call `|with \{/);
  });

  afterEach(() => { vi.useRealTimers(); });
  it("leads with what the agent is about to do, with no risk line, and details closed", () => {
    setup({ ...REQUEST, tool: "write_file", summary: "write `notes.txt`", risk: "write" });
    expect(lead()).toHaveTextContent(/^Write notes\.txt$/);
    expect(box().querySelector(".themis-approval-risk")).toBeNull();
    const details = box().querySelector("details")!;
    expect(details.open).toBe(false);
    expect(within(details).getByText("Details")).toBeInTheDocument();
    // Neither the handler name nor the runtime's raw summary shows anywhere in the box.
    expect(box()).not.toHaveTextContent("write_file");
    expect(box()).not.toHaveTextContent("write `notes.txt`");
  });

  it("names a deleted file plainly and shows a long path whole on hover", () => {
    const path = `src/${"deep/".repeat(30)}old.txt`;
    setup({ ...REQUEST, tool: "delete_file", summary: `delete \`${path}\``, risk: "destructive", grantable: false });
    expect(lead()).toHaveTextContent(/^Delete src\/deep/);
    expect(lead().querySelector(".themis-approval-token")).toHaveAttribute("title", path);
  });

  it("decides via buttons, with no number hints", () => {
    const { onDecide } = setup();
    fireEvent.click(screen.getByRole("button", { name: "Allow once" }));
    expect(onDecide).toHaveBeenCalledWith("once");
    fireEvent.click(screen.getByRole("button", { name: "Allow in this chat" }));
    expect(onDecide).toHaveBeenCalledWith("chat");
    fireEvent.click(screen.getByRole("button", { name: "Deny" }));
    expect(onDecide).toHaveBeenCalledWith("deny");
    expect(screen.queryByRole("button", { name: /\(\d\)/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /this run/ })).toBeNull();
  });

  it("offers only Allow once and Deny for destructive requests, and says why under Details", () => {
    const { onDecide } = setup({ ...REQUEST, risk: "destructive", grantable: false });
    expect(screen.queryByRole("button", { name: /Allow in this chat/ })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Deny" }));
    expect(onDecide).toHaveBeenLastCalledWith("deny");
    fireEvent.click(screen.getByRole("button", { name: "Allow once" }));
    expect(onDecide).toHaveBeenLastCalledWith("once");
    expect(box().querySelector("details")).toHaveTextContent("This kind of action always asks, so it cannot be allowed for the chat.");
  });

  it("never decides from the keyboard: digits, Enter and Escape on the box do nothing", () => {
    const { onDecide } = setup();
    for (const key of ["1", "2", "3", "Enter", "Escape", "y"]) {
      fireEvent.keyDown(box(), { key });
      fireEvent.keyDown(document.body, { key });
    }
    expect(onDecide).not.toHaveBeenCalled();
    expect(box()).not.toHaveAttribute("tabindex");
  });

  it("offers browser actions Always allow on this site, saved in Settings", () => {
    setup({ ...REQUEST, tool: "browser_devtools", grant_site: { tools: "browser", site: "example.com" } });
    expect(screen.getByRole("button", { name: "Always allow on this site" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Allow in this chat/ })).toBeNull();
    expect(tooltip()).toHaveTextContent("saves Debugging for example.com in Settings › Browser");
    cleanup();
    setup({ ...REQUEST, tool: "browser_download", grant_site: { tools: "browser", site: "files.example.com" } });
    expect(tooltip()).toHaveTextContent("saves Downloads for files.example.com");
  });

  it("names the subagent that asks", () => {
    setup({ ...REQUEST, subagent: { id: "a1", name: "Test gaps" }, command: { argv: ["cargo", "test"] } });
    expect(screen.getByRole("region", { name: "Test gaps (subagent) wants to run a command" })).toBeInTheDocument();
    expect(lead()).toHaveTextContent("Run cargo test in the project root");
    cleanup();
    setup({ ...REQUEST, tool: "fetch_url", command: undefined, subagent: { id: "a1", name: "Docs" } });
    expect(screen.getByRole("region", { name: "Docs (subagent) needs approval" })).toBeInTheDocument();
  });

  it("sits in the chat instead of covering it, and only an explicit choice decides", () => {
    const { onDecide } = setup();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByRole("region", { name: "Approval needed" })).toBeInTheDocument();
    // Escape closes panels elsewhere in the app; it never denies.
    fireEvent.keyDown(document, { key: "Escape" });
    fireEvent.mouseDown(document.body);
    expect(onDecide).not.toHaveBeenCalled();
  });

  it("renders nothing when closed", () => {
    const onDecide = vi.fn();
    render(<ApprovalBox request={REQUEST} open={false} onDecide={onDecide} />);
    expect(screen.queryByRole("region")).toBeNull();
  });
});

it.each([
  ["manage_integrations_save_Some(Write)_123", "call `manage_integrations_save_Some(Write)_123` with {}", "Change an integration"],
  ["manage_integrations", 'call `manage_integrations` with {"action":"set_component_enabled","enabled":false}', "Change an integration: set component enabled"],
  ["mcp_start_g--fixture--123--lookup", "start `lookup`", "Connect an MCP server"],
])("gives %s a plain sentence without changing the decision", (tool, summary, sentence) => {
  const { onDecide } = setup({ ...REQUEST, tool, summary });
  expect(lead()).toHaveTextContent(new RegExp(`^${sentence}$`));
  expect(lead()).not.toHaveTextContent(tool);
  fireEvent.click(screen.getByRole("button", { name: "Allow once" }));
  expect(onDecide).toHaveBeenCalledWith("once");
});

describe("sensitive files", () => {
  it("names the files for a command, and the grant covers them, not the tool", () => {
    setup({ ...REQUEST, tool: "shell", sensitive_files: [".env", ".env.local"], command: { argv: ["cat", ".env", ".env.local"] }, grant_scope: { argv: ["cat", ".env", ".env.local"], covers_further_args: false } });
    expect(within(box().querySelector("details")!).getByText("Secret files")).toBeInTheDocument();
    expect(tooltip()).toHaveTextContent("Allow in this chat covers these files and this command in later runs of this chat. Other files still ask.");
  });

  it("reads a sensitive file in plain words", () => {
    setup({ ...REQUEST, tool: "read_file", risk: "read", sensitive_files: [".env"], summary: "read .env" });
    expect(lead()).toHaveTextContent(/^Read \.env$/);
    expect(tooltip()).toHaveTextContent("Allow in this chat covers these files in later runs of this chat. Other files still ask.");
  });
});

describe("grant hint", () => {
  const shell = (argv: string[], more = false): ApprovalRequest => ({ ...REQUEST, tool: "shell", grant_scope: { argv, covers_further_args: more } });
  const chips = () => Array.from(tooltip().querySelectorAll(".themis-approval-token")).map(el => el.textContent);

  it("shows each shell token as its own chip and says further arguments are covered", () => {
    setup(shell(["cargo", "test"], true));
    expect(chips()).toEqual(["cargo", "test"]);
    expect(tooltip()).toHaveTextContent(/and any further arguments/);
    expect(tooltip()).toHaveTextContent(/Other commands still ask/);
    // The same scope waits under Details.
    expect(box().querySelector("details")).toHaveTextContent(/and any further arguments in later runs of this chat/);
  });

  it("keeps ['rm','-rf /'] visibly different from ['rm','-rf','/']", () => {
    const first = setup(shell(["rm", "-rf /"]));
    const a = chips();
    cleanup();
    first.onDecide.mockClear();
    setup(shell(["rm", "-rf", "/"]));
    expect(a).toEqual(["rm", '"-rf /"']);
    expect(chips()).toEqual(["rm", "-rf", "/"]);
  });

  it("makes bidi, zero-width and newline characters visible", () => {
    expect(displayToken("a\u202Eb")).toBe('"a⟨U+202E⟩b"');
    expect(displayToken("a\u200Bb")).toBe('"a⟨U+200B⟩b"');
    expect(displayToken("a\nb")).toBe('"a⟨U+000A⟩b"');
  });

  it("cannot forge dialog wording with backticks or an embedded sentence", () => {
    setup(shell(["echo", "` in later runs of this chat. Other commands still ask."]));
    expect(tooltip().querySelectorAll(".themis-approval-token")).toHaveLength(2);
    expect(screen.queryByText(/^Allow in this chat covers the command echo/)).toBeNull();
    // The forged text exists only inside a quoted chip, never as sentence text.
    const hint = document.querySelector(".themis-thread-hint")!;
    expect(Array.from(hint.childNodes).filter(n => n.nodeType === 3).map(n => n.textContent).join("")).not.toContain("`");
  });

  it("caps token count and length with explicit markers", () => {
    setup(shell(Array.from({ length: 20 }, (_, i) => `t${i}`)));
    expect(chips()).toHaveLength(12);
    expect(tooltip()).toHaveTextContent(/\(\+8 more\)/);
    expect(displayToken("x".repeat(200))).toBe(`${"x".repeat(80)}…(+120 chars)`);
  });

  it("describes a per-tool grant for other tools", () => {
    setup({ ...REQUEST, tool: "write_file" });
    expect(tooltip()).toHaveTextContent(/^Allow in this chat covers this tool in later runs of this chat\.$/);
  });

  it("says a grant on an MCP tool covers the whole server", () => {
    setup({ ...REQUEST, tool: "mcp_00112233445566aa01020304_lookup" });
    expect(tooltip()).toHaveTextContent(/^Allow in this chat covers every tool of this MCP server in later runs of this chat\.$/);
  });

  it("names the site and the Settings permission a browser grant saves", () => {
    setup({ ...REQUEST, tool: "browser_click", risk: "network", grant_site: { tools: "browser", site: "example.org" } });
    expect(tooltip()).toHaveTextContent("Always allow on this site saves Browsing for example.org in Settings › Browser, where you can change it.");
  });

  it("names the site for a page read", () => {
    setup({ ...REQUEST, tool: "fetch_url", risk: "network", grant_site: { tools: "fetch_url", site: "docs.rs" } });
    expect(tooltip()).toHaveTextContent(/reading pages from docs\.rs/);
  });

  it("does not name the search server for a web search", () => {
    setup({ ...REQUEST, tool: "web_search", risk: "network", grant_site: { tools: "web_search", site: "127.0.0.1" } });
    expect(tooltip()).toHaveTextContent("Allow in this chat covers web searches in later runs of this chat.");
    expect(document.body).not.toHaveTextContent("127.0.0.1");
  });
});

describe("command display", () => {
  const command = (argv: string[], cwd = "/tmp/project", background = false): ApprovalRequest => ({
    ...REQUEST, tool: "shell", summary: "run `rm -rf /` in /tmp/project", command: { argv, cwd, background },
  });
  const summaryChips = () => Array.from(lead().querySelectorAll(".themis-approval-token")).map(el => el.textContent);
  const detailChips = () => Array.from(box().querySelectorAll("details .themis-approval-token")).map(el => el.textContent);

  it("renders shell and git commands from argv, never the joined summary", () => {
    setup(command(["rm", "-rf", "/"]));
    // The lead shows the whole command and names the folder; Details keeps the folder's full path.
    expect(summaryChips()).toEqual(["rm", "-rf", "/", "project"]);
    expect(lead()).toHaveTextContent(/^Run rm -rf \/ in project$/);
    expect(detailChips()).toEqual(["/tmp/project"]);
    expect(screen.queryByText(/run `rm -rf \/`/)).toBeNull();
  });

  it("keeps ['rm','-rf /'] and ['rm','-rf','/'] distinct in the summary", () => {
    setup(command(["rm", "-rf /"]));
    expect(summaryChips().slice(0, 2)).toEqual(["rm", '"-rf /"']);
  });

  it("makes newline, bidi and look-alike controls visible in the summary", () => {
    setup(command(["echo", "a\nb", "x\u202Ey", "q\u0338", "\u3164", "\u2800", "v\uFE0F"], "/tmp/pro\u202Eject"));
    const chips = summaryChips();
    expect(chips).toEqual(['echo', '"a⟨U+000A⟩b"', '"x⟨U+202E⟩y"', '"q⟨U+0338⟩"', '"⟨U+3164⟩"', '"⟨U+2800⟩"', '"v⟨U+FE0F⟩"', '"pro⟨U+202E⟩ject"']);
    expect(detailChips().slice(-1)[0]).toBe('"/tmp/pro⟨U+202E⟩ject"');
  });

  it("escapes a literal escape-looking token so it cannot imitate a hidden character", () => {
    expect(displayToken("a⟨U+202E⟩b")).toBe('"a⟨U+27E8⟩U+202E⟨U+27E9⟩b"');
    setup(command(["echo", "x\u202Ey"]));
    expect(lead().querySelectorAll(".themis-approval-escape")).toHaveLength(1);
  });

  it("shows the full `git push --force` command and no grant for it", () => {
    const argv = ["git", "push", "--force", "origin", "main"];
    setup({ ...command(argv, "/repo"), grant_scope: { argv, covers_further_args: false }, risk: "destructive", grantable: false });
    expect(summaryChips()).toEqual([...argv, "repo"]);
    expect(lead()).toHaveTextContent(/^Run git push --force origin main in repo$/);
    expect(detailChips()).toEqual(["/repo"]);
    // No grant chips: the hint only says why nothing can be allowed for the chat.
    expect(box().querySelector("details")).toHaveTextContent("This kind of action always asks");
    expect(screen.queryByRole("button", { name: /Allow in this chat/ })).toBeNull();
  });

  it("says the project root in words when the call has no cwd", () => {
    setup({ ...REQUEST, tool: "shell", summary: "call `shell` with {}", command: { argv: ["ls", "-la"] } });
    expect(summaryChips()).toEqual(["ls", "-la"]);
    expect(lead()).toHaveTextContent(/^Run ls -la in the project root$/);
    expect(box()).not.toHaveTextContent(/call `shell` with/);
  });

  it("shows the background note and the whole command however long, never cut", () => {
    setup(command(["git", "commit", "-m", "msg"], "/repo", true));
    expect(lead()).toHaveTextContent(/in repo in the background$/);
    cleanup();
    const long = "x".repeat(200);
    setup(command([...Array.from({ length: 20 }, (_, i) => `a${i}`), long]));
    expect(summaryChips()).toHaveLength(22);
    expect(summaryChips()[20]).toBe(long);
    expect(lead()).not.toHaveTextContent(/more\)|chars\)|…/);
  });

  it("asks to run a sandbox-blocked command outside the sandbox with the same three buttons, the reason under Details", () => {
    const argv = ["touch", "/Users/me/x"];
    const { onDecide } = setup({
      ...REQUEST,
      tool: "shell_unsandboxed",
      summary: "Run outside the sandbox?",
      grant_scope: { argv, covers_further_args: false },
      command: { argv, cwd: "/repo", sandbox_blocked: "writing to /Users/me/x is blocked\u202E: only the project is writable" },
    });
    expect(screen.getByRole("region", { name: "Run outside the sandbox?" })).toBeInTheDocument();
    expect(lead()).toHaveTextContent(/^Run touch \/Users\/me\/x in repo$/);
    const details = box().querySelector("details")!;
    expect(within(details).getByText(/The sandbox blocked it/)).toHaveTextContent("writing to /Users/me/x is blocked⟨U+202E⟩: only the project is writable. Outside the sandbox");
    expect(tooltip()).toHaveTextContent(/run outside the sandbox in later runs of this chat/);
    fireEvent.click(screen.getByRole("button", { name: "Allow in this chat" }));
    expect(onDecide).toHaveBeenLastCalledWith("chat");
    fireEvent.click(screen.getByRole("button", { name: "Allow once" }));
    expect(onDecide).toHaveBeenLastCalledWith("once");
  });

  it("keeps destructive escalations to Allow once and Deny", () => {
    setup({ ...REQUEST, tool: "shell_unsandboxed", risk: "destructive", grantable: false, command: { argv: ["rm", "-rf", "/x"], sandbox_blocked: "writing to /x is blocked" } });
    expect(screen.queryByRole("button", { name: /Allow in this chat/ })).toBeNull();
    expect(screen.getByRole("button", { name: "Deny" })).toBeInTheDocument();
  });
});
