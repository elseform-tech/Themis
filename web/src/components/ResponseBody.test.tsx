import { expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { fileNamedIn, ResponseBody, ResponseFiles, ResponseLinks } from "./ResponseBody";
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
it("opens web links in the chat's browser, and in the system browser with Cmd", () => {
  const open = vi.fn();
  render(<ResponseLinks.Provider value={open}><ResponseBody text={"See [the page](http://127.0.0.1:8766/signup/) or [mail](mailto:a@b.test)."} /></ResponseLinks.Provider>);
  const link = screen.getByRole("link", { name: "the page" });
  expect(fireEvent.click(link)).toBe(false);
  expect(open).toHaveBeenCalledWith("http://127.0.0.1:8766/signup/");
  open.mockClear();
  link.addEventListener("click", event => event.preventDefault());
  fireEvent.click(link, { metaKey: true });
  fireEvent.click(screen.getByRole("link", { name: "mail" }));
  expect(open).not.toHaveBeenCalled();
});
it("knows which inline code cites a file's lines", () => {
  for (const [code, file] of [
    ["src/app.ts:12", "src/app.ts"],
    ["main.rs:10-40", "main.rs"],
    ["./web/src/index.tsx:3:7", "./web/src/index.tsx"],
  ]) expect(fileNamedIn(code!)).toBe(file);
  // A name in passing is not evidence, however much it looks like a file.
  for (const code of ["inventory.py", "src/app.ts", "docs/guide.md", "object.method", "v1.2.3", "cargo", "12:30", "list_items", "npm test", "https://x.dev/a.js:3", "--flag.js:3", "notes/:3"]) expect(fileNamedIn(code)).toBeNull();
});

it("links line references and addresses written as code, and leaves file names plain", () => {
  const openFile = vi.fn();
  const openLink = vi.fn();
  render(<ResponseLinks.Provider value={openLink}><ResponseFiles.Provider value={openFile}>
    <ResponseBody text={"See `src/app.ts:12`, `https://example.com/a` and `inventory.py`, then `object.method`."} />
  </ResponseFiles.Provider></ResponseLinks.Provider>);
  fireEvent.click(screen.getByRole("button", { name: "src/app.ts:12" }));
  expect(openFile).toHaveBeenCalledWith("src/app.ts");
  fireEvent.click(screen.getByRole("link", { name: "https://example.com/a" }));
  expect(openLink).toHaveBeenCalledWith("https://example.com/a");
  expect(screen.queryByRole("button", { name: "inventory.py" })).not.toBeInTheDocument();
  expect(screen.getByText("inventory.py").tagName).toBe("CODE");
  expect(screen.queryByRole("button", { name: "object.method" })).not.toBeInTheDocument();
});


it("links a file the reply attached as an artifact, by its path or its name", () => {
  const openFile = vi.fn();
  render(<ResponseFiles.Provider value={openFile}>
    <ResponseBody text={"The report is `report.pdf`, or `out/report.pdf`; the data is `data.csv`."} artifacts={["out/report.pdf"]} />
  </ResponseFiles.Provider>);
  fireEvent.click(screen.getByRole("button", { name: "report.pdf" }));
  fireEvent.click(screen.getByRole("button", { name: "out/report.pdf" }));
  expect(openFile.mock.calls).toEqual([["out/report.pdf"], ["out/report.pdf"]]);
  expect(screen.queryByRole("button", { name: "data.csv" })).not.toBeInTheDocument();
});
it("fits a diagram to the chat's column, in colours that tell groups apart", async () => {
  render(<ResponseBody text={'```mermaid\nflowchart TD\n A-->B\n```'} />);
  const frame = await screen.findByTitle("Mermaid diagram");
  expect(frame.style.width).toBe("100%");
  expect(frame.style.aspectRatio).not.toBe("");
  // The mock's chart is 120 wide and 80 tall, and the frame's floor makes it 320 by 160: it never
  // grows past its natural size or the height a reply can give it.
  expect(parseInt(frame.style.maxWidth, 10)).toBeLessThanOrEqual(344);
  expect(frame.closest(".themis-code-panel")).toHaveClass("themis-code-panel--diagram");
  expect(frame.getAttribute("srcdoc")).toContain("background:transparent");
  const options = diagram.initialize.mock.lastCall![0] as { theme: string; themeVariables: Record<string, string> };
  expect(options.theme).toBe("base");
  expect(options.themeVariables.primaryColor).toBeTruthy();
  expect(options.themeVariables.secondaryColor).not.toBe(options.themeVariables.primaryColor);
  expect(options.themeVariables.tertiaryColor).not.toBe(options.themeVariables.primaryColor);
});
it("keeps a tall chart readable instead of shrinking it to the height limit", async () => {
  diagram.render.mockResolvedValueOnce({ svg: '<svg viewBox="0 0 400 1600"></svg>' } as never);
  render(<ResponseBody text={'```mermaid\nflowchart TD\n A-->B\n```'} />);
  const frame = await screen.findByTitle("Mermaid diagram");
  // At least 70% of its own width, which is about where its labels stop being readable.
  expect(parseInt(frame.style.maxWidth, 10)).toBeGreaterThanOrEqual(280);
});
it("gives the frame the page's colour scheme, so a dark window has no white box", async () => {
  document.documentElement.dataset.theme = "dark";
  render(<ResponseBody text={'```mermaid\nflowchart TD\n A-->C\n```'} />);
  const frame = await screen.findByTitle("Mermaid diagram");
  expect(frame.getAttribute("srcdoc")).toContain("color-scheme:dark");
  delete document.documentElement.dataset.theme;
});
it("doesn't blow a small chart up past its own size", async () => {
  diagram.render.mockResolvedValueOnce({ svg: '<svg viewBox="0 0 150 500"></svg>' } as never);
  render(<ResponseBody text={'```mermaid\nflowchart TD\n A-->D\n```'} />);
  const frame = await screen.findByTitle("Mermaid diagram");
  expect(parseInt(frame.style.maxWidth, 10)).toBeLessThanOrEqual(174);
  expect(frame.style.aspectRatio).toBe("174 / 524");
});
it("centres a chart and labels it in small, plain text", async () => {
  render(<ResponseBody text={'```mermaid\nflowchart TD\n A-->E\n```'} />);
  const frame = await screen.findByTitle("Mermaid diagram");
  expect(frame.style.marginInline).toBe("auto");
  const options = diagram.initialize.mock.lastCall![0] as { themeVariables: Record<string, string> };
  expect(options.themeVariables.fontSize).toBe("10px");
  expect(options.themeVariables.fontFamily).toContain("system-ui");
});
it("shows the whole chart when enlarged, in its own shape", async () => {
  HTMLDialogElement.prototype.showModal = vi.fn();
  HTMLDialogElement.prototype.close = vi.fn();
  diagram.render.mockResolvedValueOnce({ svg: '<svg viewBox="0 0 150 500"></svg>' } as never);
  const sizes = vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(324);
  const widths = vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(1000);
  render(<ResponseBody text={'```mermaid\nflowchart TD\n A-->F\n```'} />);
  await screen.findByTitle("Mermaid diagram");
  fireEvent.click(screen.getByRole("button", { name: "Enlarge diagram" }));
  // A chart 500 tall in a 324 tall window, less margins, opens at half size.
  expect(await screen.findByText("50%")).toBeInTheDocument();
  const frames = screen.getAllByTitle("Mermaid diagram");
  const enlarged = frames[frames.length - 1]!;
  expect(enlarged.style.width).toBe("99px");
  expect(enlarged.style.height).toBe("274px");
  sizes.mockRestore();
  widths.mockRestore();
});
