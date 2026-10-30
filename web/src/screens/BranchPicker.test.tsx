// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { BranchPicker } from "./ThreadComposer";

afterEach(cleanup);

describe("composer branch chip", () => {
  it("lists the other branches and switches to the chosen one", () => {
    const onSwitch = vi.fn();
    render(<BranchPicker branch="main" branches={["main", "feature"]} busy={false} onSwitch={onSwitch} />);
    expect(screen.getByLabelText("Branch main. Switch branch")).toHaveTextContent("main");
    expect(screen.getByLabelText("Branch main. Switch branch").querySelector(".themis-branch-chevron")).toBeInTheDocument();
    const menu = screen.getByRole("menu", { name: "Switch branch", hidden: true });
    expect(menu).toHaveTextContent("feature");
    expect(menu).not.toHaveTextContent("main");
    fireEvent.click(screen.getByRole("menuitem", { name: "feature", hidden: true }));
    expect(onSwitch).toHaveBeenCalledWith("feature");
  });
  it("is plain text during a run, or when there is nothing to switch to", () => {
    const { rerender } = render(<BranchPicker branch="main" branches={["main", "feature"]} busy onSwitch={vi.fn()} />);
    expect(screen.getByText("main")).toBeVisible();
    expect(screen.queryByRole("menu", { hidden: true })).toBeNull();
    expect(document.querySelector(".themis-branch-chevron")).toBeNull();
    rerender(<BranchPicker branch="main" branches={["main"]} busy={false} onSwitch={vi.fn()} />);
    expect(screen.queryByRole("menu", { hidden: true })).toBeNull();
  });
});
