import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import type { ApprovalRequest } from "../../lib/types";
import { ApprovalDialog } from "./ApprovalDialog";

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
