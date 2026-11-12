// @vitest-environment jsdom
import { render } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { useRef } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { useFitSelect } from "./fitSelect";

function Picker({ value }: { value: string }) {
  const ref = useRef<HTMLSelectElement>(null);
  useFitSelect(ref);
  return <select ref={ref} aria-label="Show changes from" value={value} onChange={() => {}}>
    <option value="">Uncommitted changes on a-very-long-branch-name-indeed</option>
    <option value="feature-dates">feature-dates</option>
  </select>;
}

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

it("sizes the select to the chosen option where field-sizing is missing (older WebKit, WebKitGTK)", () => {
  vi.stubGlobal("CSS", { supports: () => false });
  // The probe measures the chosen label; jsdom has no layout, so give the probe a width.
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
    return { width: this.textContent === "feature-dates" ? 80 : 300 } as DOMRect;
  });
  const { getByRole, rerender } = render(<Picker value="feature-dates" />);
  expect(getByRole("combobox")).toHaveStyle({ width: "82px" });
  rerender(<Picker value="" />);
  expect(getByRole("combobox")).toHaveStyle({ width: "302px" });
});

it("leaves the width to CSS where field-sizing: content works", () => {
  vi.stubGlobal("CSS", { supports: (property: string, value: string) => property === "field-sizing" && value === "content" });
  const { getByRole } = render(<Picker value="feature-dates" />);
  expect(getByRole("combobox").style.width).toBe("");
});

it("draws the Changes switcher and Author filter as text and a chevron: no box at rest, on hover or pressed, the app's ring on keyboard focus only", () => {
  const css = readFileSync("src/screens/WorkspacePanel.css", "utf8");
  const rule = (selector: string) => { const at = css.indexOf(`\n${selector} {`); expect(at, selector).toBeGreaterThanOrEqual(0); return css.slice(at, css.indexOf("}", at)); };
  const select = rule(".themis-workspace-source select");
  for (const part of ["appearance: none", "field-sizing: content", "border: 0", "background-color: transparent", "box-shadow: none"]) expect(select).toContain(part);
  expect(rule(".themis-workspace-source select:hover, .themis-workspace-source select:active, .themis-workspace-source select:focus")).toMatch(/color: var\(--themis-text\);[^}]*background-color: transparent;[^}]*box-shadow: none/);
  expect(rule(".themis-workspace-source select:focus:not(:focus-visible)")).toContain("outline: none");
  // No override of the app's ring (shell.css: 2px accent) for keyboard focus.
  expect(css).not.toMatch(/\.themis-workspace-source select:focus-visible\s*\{/);
});
