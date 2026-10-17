// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useRef, useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { useDismiss, usePopoverDismissal } from "./dismiss";

afterEach(cleanup);

function Menus() {
  usePopoverDismissal();
  return <div>
    <details data-popover open><summary>Model</summary><button type="button">Inside</button></details>
    <details open><summary>Inline section</summary></details>
    <p>Elsewhere</p>
  </div>;
}

function Panel() {
  const [open, setOpen] = useState(true);
  const root = useRef<HTMLDivElement>(null);
  useDismiss(root, open, () => setOpen(false));
  return <div><div ref={root}>{open && <div role="dialog" aria-label="Context"><button type="button">Inside</button></div>}</div><p>Elsewhere</p></div>;
}

describe("popover dismissal", () => {
  it("closes popover menus on a click outside or Escape, and leaves inline sections alone", () => {
    const { container } = render(<Menus />);
    const [menu, inline] = Array.from(container.querySelectorAll("details"));
    fireEvent.pointerDown(screen.getByRole("button", { name: "Inside" }));
    expect(menu).toHaveAttribute("open");
    fireEvent.pointerDown(screen.getByText("Elsewhere"));
    expect(menu).not.toHaveAttribute("open");
    expect(inline).toHaveAttribute("open");
    menu.setAttribute("open", "");
    fireEvent.keyDown(document, { key: "Escape" });
    expect(menu).not.toHaveAttribute("open");
  });
  it("closes a state-driven panel on a click outside or Escape, not inside", () => {
    render(<Panel />);
    fireEvent.pointerDown(screen.getByRole("button", { name: "Inside" }));
    expect(screen.getByRole("dialog", { name: "Context" })).toBeInTheDocument();
    fireEvent.pointerDown(screen.getByText("Elsewhere"));
    expect(screen.queryByRole("dialog", { name: "Context" })).toBeNull();
  });
});
