import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
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
  it("shows an indeterminate bar without progress", () => {
    render(<Loader label="Checking" />);
    expect(screen.getByRole("progressbar")).not.toHaveAttribute("aria-valuenow");
  });
  it("has no buttons", () => {
    render(<Loader label="Checking" />);
    expect(screen.queryByRole("button")).toBeNull();
  });
});
