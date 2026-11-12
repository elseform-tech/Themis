// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { expect, it } from "vitest";
import { StartupScreen } from "./StartupScreen";

it("shows the Themis Visions mark, the wordmark and the startup bar, as the canvas Loading board does", () => {
  render(<StartupScreen label="Opening workspace…" progress={52} />);
  const mark = screen.getByRole("img", { name: "Themis Visions" });
  expect(mark).toHaveAttribute("width", "72");
  expect(mark).toHaveAttribute("height", "72");
  expect(mark.querySelectorAll("path")).toHaveLength(2);
  expect(screen.getByText("Themis")).toBeInTheDocument();
  expect(screen.getByText("Visions")).toBeInTheDocument();
  const bar = screen.getByRole("progressbar", { name: "Starting" });
  expect(bar).toHaveAttribute("aria-valuemin", "0");
  expect(bar).toHaveAttribute("aria-valuemax", "100");
  expect(bar).toHaveAttribute("aria-valuenow", "52");
  expect(bar.firstElementChild).toHaveStyle({ width: "52%" });
});

it("keeps the step for screen readers only: the canvas shows no status line", () => {
  render(<StartupScreen label="Restoring conversations…" progress={84} />);
  const status = screen.getByRole("status");
  expect(status).toHaveTextContent("Restoring conversations…");
  expect(status).toHaveClass("themis-sr-only");
});

it("draws on the sky's frame with the canvas's sizes, and calms its motion when asked", () => {
  const css = readFileSync("src/screens/StartupScreen.css", "utf8");
  expect(css).toMatch(/\.themis-startup \{[^}]*gap: 20px[^}]*background: var\(--themis-sky\)/);
  expect(css).toMatch(/\.themis-startup-word \{[^}]*font-size: 19px[^}]*letter-spacing: \.04em[^}]*gap: 7px/);
  expect(css).toMatch(/\.themis-startup-bar \{[^}]*width: 140px; height: 2px; border-radius: 3px; background: var\(--themis-mark-track\)/);
  expect(css).toMatch(/\.themis-startup-fill \{[^}]*background: var\(--themis-accent\); transition: width \.9s cubic-bezier\(\.3, \.7, \.2, 1\)/);
  expect(css).toMatch(/prefers-reduced-motion: reduce\)[^}]*\.themis-startup-fill \{ transition: none; \}/);
  expect(css).not.toMatch(/Manrope/);
  const shell = readFileSync("src/screens/shell.css", "utf8");
  expect(shell).toMatch(/\.themis-shell \{ animation: themis-shell-reveal 240ms ease-out; \}/);
  expect(shell).toMatch(/prefers-reduced-motion: reduce\) \{ \*, \*::before, \*::after \{[^}]*animation: none !important/);
});
