import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { Badge } from "./Badge";
import { Button } from "./Button";
import { StatusDot } from "./StatusDot";

describe("Button", () => {
  it("renders children and fires onClick", () => {
    const onClick = vi.fn();
    render(<Button onClick={onClick}>Save</Button>);
    const button = screen.getByRole("button", { name: "Save" });
    fireEvent.click(button);
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("applies variant and size classes", () => {
    render(
      <Button variant="danger" size="small">
        Delete
      </Button>,
    );
    const button = screen.getByRole("button", { name: "Delete" });
    expect(button.className).toContain("themis-button--danger");
    expect(button.className).toContain("themis-button--small");
  });

  it("disabled buttons do not fire onClick", () => {
    const onClick = vi.fn();
    render(
      <Button disabled onClick={onClick}>
        Blocked
      </Button>,
    );
    const button = screen.getByRole("button", { name: "Blocked" });
    expect(button).toBeDisabled();
    fireEvent.click(button);
    expect(onClick).not.toHaveBeenCalled();
  });
});

describe("StatusDot", () => {
  it.each([
    ["running", "themis-status-dot--running"],
    ["awaiting", "themis-status-dot--awaiting"],
    ["done", "themis-status-dot--done"],
    ["failed", "themis-status-dot--failed"],
    ["queued", "themis-status-dot--queued"],
  ] as const)("maps status %s to %s", (status, cls) => {
    const { container } = render(<StatusDot status={status} label={status} />);
    expect(screen.getByText(status)).toBeInTheDocument();
    const dot = container.querySelector("span[aria-hidden='true']");
    expect(dot?.className).toContain(cls);
  });
});

describe("Badge", () => {
  it("renders tone class and children", () => {
    render(<Badge tone="danger">destructive</Badge>);
    const badge = screen.getByText("destructive");
    expect(badge.className).toContain("themis-badge--danger");
  });
});
