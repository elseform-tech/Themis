import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ApprovalRequest } from "../../lib/types";
import { ApprovalDialog, displayToken } from "./ApprovalDialog";

const REQUEST: ApprovalRequest = {
  thread_id: "t1",
  approval_id: "a1",
  tool: "shell.exec",
  summary: "Run `cargo test` in the project root.",
  risk: "execute",
};

function setup(request: ApprovalRequest = REQUEST) {
  const onDecide = vi.fn();
  render(<ApprovalDialog request={request} open onDecide={onDecide} />);
  return { onDecide };
}

describe("ApprovalDialog", () => {
  it("names an MCP tool by server and tool, not its wire id", () => {
    const tool = "mcp_fb67fe144f6fc34f56444b8d_list_allowed_directories";
    render(<ApprovalDialog request={{ ...REQUEST, tool, summary: `call \`${tool}\` with {}` }} label="files · list_allowed_directories" open onDecide={() => {}} />);
    expect(screen.getAllByText(/files · list allowed directories/).length).toBeGreaterThan(0);
    expect(screen.queryByText(/fb67fe/)).toBeNull();
  });

  afterEach(() => { vi.useRealTimers(); });
  it("shows tool, summary, and risk badge", () => {
    setup();
    expect(screen.getByText("shell.exec")).toBeInTheDocument();
    expect(
      screen.getByText("Run `cargo test` in the project root."),
    ).toBeInTheDocument();
    expect(screen.getByText("execute")).toBeInTheDocument();
  });

  it("maps destructive risk to the danger tone", () => {
    setup({ ...REQUEST, risk: "destructive" });
    expect(screen.getByText("destructive").className).toContain(
      "themis-badge--danger",
    );
  });

  it("decides via buttons", () => {
    const { onDecide } = setup();
    fireEvent.click(screen.getByRole("button", { name: "Allow once (1)" }));
    expect(onDecide).toHaveBeenCalledWith("once");
    fireEvent.click(screen.getByRole("button", { name: "Allow in this chat (2)" }));
    expect(onDecide).toHaveBeenCalledWith("chat");
    fireEvent.click(screen.getByRole("button", { name: "Deny (3)" }));
    expect(onDecide).toHaveBeenCalledWith("deny");
    expect(screen.getAllByRole("button").map(button => button.textContent).filter(text => /\(\d\)$/.test(text ?? ""))).toEqual(["Allow once (1)", "Allow in this chat (2)", "Deny (3)"]);
    expect(screen.queryByRole("button", { name: /this run/ })).toBeNull();
  });

  it("offers only Allow once and Deny for destructive requests", () => {
    const { onDecide } = setup({ ...REQUEST, risk: "destructive", grantable: false });
    expect(screen.queryByRole("button", { name: /Allow in this chat/ })).toBeNull();
    expect(screen.getByText("This kind of action always asks, so it cannot be allowed for the chat.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Deny (3)" }));
    expect(onDecide).toHaveBeenLastCalledWith("deny");
    fireEvent.click(screen.getByRole("button", { name: "Allow once (1)" }));
    expect(onDecide).toHaveBeenLastCalledWith("once");
  });

  it("keeps the numbering stable for destructive requests: 1 once, 3 deny, 2 does nothing", () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    const { onDecide } = setup({ ...REQUEST, risk: "destructive", grantable: false });
    vi.advanceTimersByTime(600);
    fireEvent.keyDown(document, { key: "2" });
    expect(onDecide).not.toHaveBeenCalled();
    fireEvent.keyDown(document, { key: "3" });
    expect(onDecide).toHaveBeenLastCalledWith("deny");
    fireEvent.keyDown(document, { key: "1" });
    expect(onDecide).toHaveBeenLastCalledWith("once");
  });

  it("maps keyboard 1/2/3 to once/chat/deny", () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    const { onDecide } = setup();
    vi.advanceTimersByTime(600);
    fireEvent.keyDown(document, { key: "1" });
    expect(onDecide).toHaveBeenCalledWith("once");
    fireEvent.keyDown(document, { key: "2" });
    expect(onDecide).toHaveBeenCalledWith("chat");
    fireEvent.keyDown(document, { key: "3" });
    expect(onDecide).toHaveBeenCalledWith("deny");
  });

  it("keeps a deciding digit out of the composer the focus returns to", () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    const { onDecide } = setup();
    vi.advanceTimersByTime(600);
    // fireEvent returns false when the handler cancelled the key's default (typing it).
    expect(fireEvent.keyDown(document, { key: "3" })).toBe(false);
    expect(onDecide).toHaveBeenCalledWith("deny");
    // Other keys keep their default.
    expect(fireEvent.keyDown(document, { key: "7" })).toBe(true);
  });

  it("decides by digit while its own button has focus, as it does on opening", () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    const { onDecide } = setup();
    vi.advanceTimersByTime(600);
    const allow = screen.getByRole("button", { name: "Allow once (1)" });
    allow.focus();
    fireEvent.keyDown(allow, { key: "1" });
    expect(onDecide).toHaveBeenCalledWith("once");
  });

  it("still decides by digit while the app keeps re-rendering it with a new callback", () => {
    // Seen live: during a run the app re-rendered the dialog every few hundred ms with a
    // fresh onDecide, which restarted the 500 ms pause, so 1/2/3 never decided.
    vi.useFakeTimers({ toFake: ["Date"] });
    const decided = vi.fn();
    const { rerender } = render(<ApprovalDialog request={REQUEST} open onDecide={decision => decided(decision)} />);
    for (let tick = 0; tick < 6; tick += 1) {
      vi.advanceTimersByTime(200);
      rerender(<ApprovalDialog request={REQUEST} open onDecide={decision => decided(decision)} />);
    }
    fireEvent.keyDown(document, { key: "3" });
    expect(decided).toHaveBeenCalledWith("deny");
  });

  it("offers browser actions Always allow on this site, saved in Settings", () => {
    setup({ ...REQUEST, tool: "browser_devtools", grant_site: { tools: "browser", site: "example.com" } });
    expect(screen.getByRole("button", { name: "Always allow on this site (2)" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Allow in this chat/ })).toBeNull();
    expect(document.body).toHaveTextContent("saves Debugging for example.com in Settings › Browser");
    cleanup();
    setup({ ...REQUEST, tool: "browser_download", grant_site: { tools: "browser", site: "files.example.com" } });
    expect(document.body).toHaveTextContent("saves Downloads for files.example.com");
  });

  it("maps Escape to deny", () => {
    const { onDecide } = setup();
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onDecide).toHaveBeenCalledWith("deny");
  });

  it("ignores a click outside the dialog: only an explicit choice decides", () => {
    const { onDecide } = setup();
    const overlay = document.querySelector(".themis-dialog-overlay")!;
    fireEvent.mouseDown(overlay);
    expect(onDecide).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  it("renders nothing when closed", () => {
    const onDecide = vi.fn();
    render(<ApprovalDialog request={REQUEST} open={false} onDecide={onDecide} />);
    expect(screen.queryByText("shell.exec")).not.toBeInTheDocument();
  });
});

it.each([["manage_integrations_save_Some(Write)_123", "Manage integrations"], ["mcp_start_g--fixture--123--lookup", "Connect MCP"]])("gives %s a readable label without changing the decision", (tool, label) => {
  const { onDecide } = setup({ ...REQUEST, tool });
  expect(screen.getByText(label)).toBeInTheDocument();
  expect(screen.queryByText(tool)).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Allow once (1)" }));
  expect(onDecide).toHaveBeenCalledWith("once");
});

describe("grant hint", () => {
  const shell = (argv: string[], more = false): ApprovalRequest => ({ ...REQUEST, tool: "shell", grant_scope: { argv, covers_further_args: more } });
  const chips = () => Array.from(document.querySelectorAll(".themis-approval-token")).map(el => el.textContent);

  it("shows each shell token as its own chip and says further arguments are covered", () => {
    setup(shell(["cargo", "test"], true));
    expect(chips()).toEqual(["cargo", "test"]);
    expect(screen.getByText(/and any further arguments/)).toBeInTheDocument();
    expect(screen.getByText(/Other commands still ask/)).toBeInTheDocument();
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
    expect(document.querySelectorAll(".themis-approval-token")).toHaveLength(2);
    expect(screen.queryByText(/^Allow in this chat covers the command echo/)).toBeNull();
    // The forged text exists only inside a quoted chip, never as sentence text.
    const hint = document.querySelector(".themis-thread-hint")!;
    expect(Array.from(hint.childNodes).filter(n => n.nodeType === 3).map(n => n.textContent).join("")).not.toContain("`");
  });

  it("caps token count and length with explicit markers", () => {
    setup(shell(Array.from({ length: 20 }, (_, i) => `t${i}`)));
    expect(chips()).toHaveLength(12);
    expect(screen.getByText(/\(\+8 more\)/)).toBeInTheDocument();
    expect(displayToken("x".repeat(200))).toBe(`${"x".repeat(80)}…(+120 chars)`);
  });

  it("describes a per-tool grant for other tools", () => {
    setup({ ...REQUEST, tool: "write_file" });
    expect(screen.getByText("Allow in this chat covers this tool in later runs of this chat.")).toBeInTheDocument();
  });

  it("says a grant on an MCP tool covers the whole server", () => {
    setup({ ...REQUEST, tool: "mcp_00112233445566aa01020304_lookup" });
    expect(screen.getByText("Allow in this chat covers every tool of this MCP server in later runs of this chat.")).toBeInTheDocument();
  });

  it("names the site and the Settings permission a browser grant saves", () => {
    setup({ ...REQUEST, tool: "browser_click", risk: "network", grant_site: { tools: "browser", site: "example.org" } });
    expect(screen.getByText(/Always allow on this site saves/)).toHaveTextContent("Always allow on this site saves Browsing for example.org in Settings › Browser, where you can change it.");
  });

  it("names the site for a page read", () => {
    setup({ ...REQUEST, tool: "fetch_url", risk: "network", grant_site: { tools: "fetch_url", site: "docs.rs" } });
    expect(screen.getByText(/reading pages from/)).toHaveTextContent("docs.rs");
  });
});

describe("single-key shortcuts", () => {
  afterEach(() => { vi.useRealTimers(); });
  it("ignores digits typed during the first 500 ms and held-down repeats", () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    const { onDecide } = setup();
    fireEvent.keyDown(document.body, { key: "2" }); // typed into the composer just as the dialog opened
    vi.advanceTimersByTime(499);
    fireEvent.keyDown(document.body, { key: "2" });
    expect(onDecide).not.toHaveBeenCalled();
    vi.advanceTimersByTime(2);
    fireEvent.keyDown(document.body, { key: "2", repeat: true });
    expect(onDecide).not.toHaveBeenCalled();
    fireEvent.keyDown(document.body, { key: "2" });
    expect(onDecide).toHaveBeenCalledWith("chat");
  });
  it("ignores digits typed into the composer or another field", () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    const { onDecide } = setup();
    vi.advanceTimersByTime(600);
    const composer = document.createElement("div");
    composer.className = "themis-thread-composer";
    const editor = document.createElement("div");
    editor.setAttribute("role", "textbox");
    composer.append(editor);
    const input = document.createElement("input");
    document.body.append(composer, input);
    for (const target of [editor, composer, input]) fireEvent.keyDown(target, { key: "2" });
    expect(onDecide).not.toHaveBeenCalled();
    fireEvent.keyDown(document.body, { key: "2", metaKey: true });
    expect(onDecide).not.toHaveBeenCalled();
    fireEvent.keyDown(document.body, { key: "2" });
    expect(onDecide).toHaveBeenCalledWith("chat");
    composer.remove(); input.remove();
  });
});

describe("command display", () => {
  const command = (argv: string[], cwd = "/tmp/project", background = false): ApprovalRequest => ({
    ...REQUEST, tool: "shell", summary: "run `rm -rf /` in /tmp/project", command: { argv, cwd, background },
  });
  const summaryChips = () => Array.from(document.querySelectorAll(".themis-approval-summary .themis-approval-token")).map(el => el.textContent);

  it("renders shell and git commands from argv, never the joined summary", () => {
    setup(command(["rm", "-rf", "/"]));
    expect(summaryChips()).toEqual(["rm", "-rf", "/", "/tmp/project"]);
    expect(screen.queryByText(/run `rm -rf \/`/)).toBeNull();
  });

  it("keeps ['rm','-rf /'] and ['rm','-rf','/'] distinct in the summary", () => {
    setup(command(["rm", "-rf /"]));
    expect(summaryChips().slice(0, 2)).toEqual(["rm", '"-rf /"']);
  });

  it("makes newline, bidi and look-alike controls visible in the summary", () => {
    setup(command(["echo", "a\nb", "x\u202Ey", "q\u0338", "\u3164", "\u2800", "v\uFE0F"], "/tmp/pro\u202Eject"));
    const chips = summaryChips();
    expect(chips).toEqual(['echo', '"a⟨U+000A⟩b"', '"x⟨U+202E⟩y"', '"q⟨U+0338⟩"', '"⟨U+3164⟩"', '"⟨U+2800⟩"', '"v⟨U+FE0F⟩"', '"/tmp/pro⟨U+202E⟩ject"']);
  });

  it("escapes a literal escape-looking token so it cannot imitate a hidden character", () => {
    expect(displayToken("a⟨U+202E⟩b")).toBe('"a⟨U+27E8⟩U+202E⟨U+27E9⟩b"');
    setup(command(["echo", "x\u202Ey"]));
    expect(document.querySelectorAll(".themis-approval-escape")).toHaveLength(1);
  });

  it("shows the full `git push --force` command and no grant for it", () => {
    const argv = ["git", "push", "--force", "origin", "main"];
    setup({ ...command(argv, "/repo"), grant_scope: { argv, covers_further_args: false }, risk: "destructive", grantable: false });
    expect(summaryChips()).toEqual([...argv, "/repo"]);
    // Only the summary renders chips; the hint states why nothing can be allowed for the chat.
    const all = Array.from(document.querySelectorAll(".themis-approval-token")).map(el => el.textContent);
    expect(all).toEqual([...argv, "/repo"]);
    expect(screen.queryByRole("button", { name: /Allow in this chat/ })).toBeNull();
  });

  it("says the project root in words when the call has no cwd", () => {
    setup({ ...REQUEST, tool: "shell", summary: "call `shell` with {}", command: { argv: ["ls", "-la"] } });
    expect(summaryChips()).toEqual(["ls", "-la"]);
    expect(screen.getByText(/in the project root/)).toBeInTheDocument();
    expect(screen.queryByText(/call `shell` with/)).toBeNull();
  });

  it("shows git commands, the background note and the 12-token cap", () => {
    setup(command(["git", "commit", "-m", "msg"], "/repo", true));
    expect(screen.getByText(/in the background/)).toBeInTheDocument();
    cleanup();
    setup(command(Array.from({ length: 20 }, (_, i) => `a${i}`)));
    expect(summaryChips()).toHaveLength(13);
    expect(screen.getByText(/\(\+8 more\)/)).toBeInTheDocument();
  });

  it("asks to run a sandbox-blocked command outside the sandbox with the same three buttons", () => {
    const argv = ["touch", "/Users/me/x"];
    const { onDecide } = setup({
      ...REQUEST,
      tool: "shell_unsandboxed",
      summary: "Run outside the sandbox?",
      grant_scope: { argv, covers_further_args: false },
      command: { argv, cwd: "/repo", sandbox_blocked: "writing to /Users/me/x is blocked\u202E: only the project is writable" },
    });
    expect(screen.getByRole("dialog", { name: "Run outside the sandbox?" })).toBeInTheDocument();
    expect(screen.getByText("shell (outside the sandbox)")).toBeInTheDocument();
    expect(screen.getByText(/The sandbox blocked it/)).toHaveTextContent("writing to /Users/me/x is blocked⟨U+202E⟩: only the project is writable. Outside the sandbox");
    expect(screen.getByText(/run outside the sandbox in later runs of this chat/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Allow in this chat (2)" }));
    expect(onDecide).toHaveBeenLastCalledWith("chat");
    fireEvent.click(screen.getByRole("button", { name: "Allow once (1)" }));
    expect(onDecide).toHaveBeenLastCalledWith("once");
  });

  it("keeps destructive escalations to Allow once and Deny", () => {
    setup({ ...REQUEST, tool: "shell_unsandboxed", risk: "destructive", grantable: false, command: { argv: ["rm", "-rf", "/x"], sandbox_blocked: "writing to /x is blocked" } });
    expect(screen.queryByRole("button", { name: /Allow in this chat/ })).toBeNull();
    expect(screen.getByRole("button", { name: "Deny (3)" })).toBeInTheDocument();
  });
});
