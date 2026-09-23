import { expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { ResponseBody } from "./ResponseBody";
const diagram = vi.hoisted(() => ({ initialize: vi.fn(), render: vi.fn(async () => ({ svg: '<svg viewBox="0 0 120 80"><text>Start</text></svg>' })) }));
vi.mock("mermaid", () => ({ default: diagram }));
it("formats markdown, tables and code, and copies the source", async () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  render(<ResponseBody text={'## Result\n\n| File | Status |\n| --- | --- |\n| a.py | Done |\n\n```python\nprint("hello")\n```'} />);
  expect(screen.getByRole("heading", { name: "Result" })).toBeInTheDocument();
  expect(screen.getByRole("table")).toHaveTextContent("a.py");
  fireEvent.click(screen.getByRole("button", { name: "Copy" }));
  await waitFor(() => expect(writeText).toHaveBeenCalledWith('print("hello")'));
});
it("does not render raw HTML or fetch response images", () => {
  const { container } = render(<ResponseBody text={'<script>alert(1)</script>\n\n![Image](https://example.com/a.png)\n\n[bad](javascript:alert(1))'} />);
  expect(container.querySelector("script, img")).toBeNull();
  expect(screen.getByText("bad").getAttribute("href")).not.toContain("javascript:");
});
it("renders completed Mermaid in a sandbox with expandable source", async () => {
  render(<ResponseBody text={'```mermaid\nflowchart LR\n A-->B\n```'} />);
  const frame = await screen.findByTitle("Mermaid diagram");
  expect(frame).toHaveAttribute("sandbox", "");
  expect(frame.getAttribute("srcdoc")).toContain("default-src 'none'");
  expect(screen.getByText("View source")).toBeInTheDocument();
});
it("enlarges Mermaid with zoom controls", async () => {
  HTMLDialogElement.prototype.showModal = vi.fn();
  HTMLDialogElement.prototype.close = vi.fn();
  HTMLElement.prototype.setPointerCapture = vi.fn();
  const { container } = render(<ResponseBody text={'```mermaid\nflowchart LR\n A-->B\n```'} />);
  await screen.findByTitle("Mermaid diagram");
  fireEvent.click(screen.getByRole("button", { name: "Enlarge diagram" }));
  expect(screen.getByRole("dialog", { hidden: true })).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Zoom in", hidden: true }));
  expect(screen.getByText("125%")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "100%", hidden: true }));
  expect(screen.getAllByText("100%")).toHaveLength(2);
  const canvas = container.querySelector(".themis-diagram-canvas")!;
  fireEvent.pointerDown(canvas, { button: 0, clientX: 10, clientY: 20, pointerId: 1 });
  fireEvent.pointerMove(canvas, { clientX: 50, clientY: 45, pointerId: 1 });
  expect(container.querySelector<HTMLElement>(".themis-diagram-stage")?.style.transform).toContain("translate(40px, 25px)");
  fireEvent.click(screen.getByRole("button", { name: "Fit", hidden: true }));
  expect(container.querySelector<HTMLElement>(".themis-diagram-stage")?.style.transform).toContain("translate(0px, 0px)");
});
it("keeps incomplete streaming diagrams as code", () => {
  render(<ResponseBody text={'```mermaid\nflowchart LR\n A--'} streaming />);
  expect(screen.queryByTitle("Mermaid diagram")).toBeNull();
});
it("falls back to source for untrusted diagram configuration", () => {
  render(<ResponseBody text={'```mermaid\n%%{init: {"securityLevel":"loose"}}%%\nflowchart LR\n A-->B\n```'} />);
  expect(screen.getByText("Diagram unavailable · View source")).toBeInTheDocument();
});
it("preserves expanded source across unrelated parent renders", async () => {
  const text = '```mermaid\nflowchart LR\n A-->B\n```';
  const { container, rerender } = render(<ResponseBody text={text} />);
  await screen.findByTitle("Mermaid diagram");
  const details = container.querySelector("details")!;
  details.open = true;
  rerender(<ResponseBody text={text} />);
  expect(container.querySelector("details")).toBe(details);
  expect(details.open).toBe(true);
});
