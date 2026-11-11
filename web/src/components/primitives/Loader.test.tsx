import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { Loader } from "./Loader";

describe("Loader", () => {
  it("shows label as status text", () => {
    render(<Loader label="Opening workspace" />);
    expect(screen.getByRole("status")).toHaveTextContent("Opening workspace");
  });
  it("shows progress when given", () => {
    render(<Loader label="Checking" progress={52} />);
    expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "52");
  });
  it("offers Wake only with onWake", () => {
    const { rerender } = render(<Loader label="Checking" />);
    expect(screen.queryByRole("button")).toBeNull();
    const onWake = vi.fn();
    rerender(<Loader label="Checking" onWake={onWake} />);
    fireEvent.click(screen.getByRole("button", { name: "Wake" }));
    expect(onWake).toHaveBeenCalledTimes(1);
  });
});
