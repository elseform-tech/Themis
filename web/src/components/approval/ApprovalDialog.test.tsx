import { describe, expect, it, vi } from "vitest";
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
    fireEvent.click(screen.getByRole("button", { name: "Allow for this run (2)" }));
    expect(onDecide).toHaveBeenCalledWith("always");
    fireEvent.click(screen.getByRole("button", { name: "Deny (3)" }));
    expect(onDecide).toHaveBeenCalledWith("deny");
  });

  it("maps keyboard 1/2/3 to once/always/deny", () => {
    const { onDecide } = setup();
    fireEvent.keyDown(document, { key: "1" });
    expect(onDecide).toHaveBeenCalledWith("once");
    fireEvent.keyDown(document, { key: "2" });
    expect(onDecide).toHaveBeenCalledWith("always");
    fireEvent.keyDown(document, { key: "3" });
    expect(onDecide).toHaveBeenCalledWith("deny");
  });

  it("maps Escape to deny", () => {
    const { onDecide } = setup();
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onDecide).toHaveBeenCalledWith("deny");
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
    expect(displayToken("a\u202Eb")).toBe("a⟨U+202E⟩b");
    expect(displayToken("a\u200Bb")).toBe("a⟨U+200B⟩b");
    expect(displayToken("a\nb")).toBe('"a⟨U+000A⟩b"');
  });

  it("cannot forge dialog wording with backticks or an embedded sentence", () => {
    setup(shell(["echo", "` until this run ends. Other commands still ask."]));
    expect(document.querySelectorAll(".themis-approval-token")).toHaveLength(2);
    expect(screen.queryByText(/^Allow for this run covers the command echo/)).toBeNull();
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
    expect(screen.getByText("Allow for this run covers this tool until this run ends.")).toBeInTheDocument();
  });
});

describe("single-key shortcuts", () => {
  it("ignores digits typed into the composer or another field", () => {
    const { onDecide } = setup();
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
    expect(onDecide).toHaveBeenCalledWith("always");
    composer.remove(); input.remove();
  });
});
