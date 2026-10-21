// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import * as bridge from "../lib/tauri";
import { formatBytes, machineShare, ResourceMeter } from "./ResourceMeter";

vi.mock("../lib/tauri", () => ({ resourceUsage: vi.fn() }));
const MB = 1024 * 1024;
const usage = (over: Partial<bridge.ResourceUsage> = {}): bridge.ResourceUsage => ({
  sampled_at_ms: 1, cpu_percent: 160, memory_bytes: 1536 * MB, cores: 8,
  groups: [
    { group: "app", cpu_percent: 20, memory_bytes: 300 * MB, processes: 2 },
    { group: "browser", cpu_percent: 120, memory_bytes: 900 * MB, processes: 5 },
    { group: "tools", cpu_percent: 20, memory_bytes: 336 * MB, processes: 3 },
  ],
  ...over,
});
afterEach(() => { cleanup(); vi.useRealTimers(); });

describe("ResourceMeter", () => {
  it("formats memory and the machine share of CPU", () => {
    expect(formatBytes(512 * MB)).toBe("512 MB");
    expect(formatBytes(1536 * MB)).toBe("1.5 GB");
    expect(machineShare(usage())).toBe(20);
    expect(machineShare(usage({ cores: 0 }))).toBe(160);
  });

  it("stays hidden until the first sample, then shows CPU and memory and opens the breakdown", async () => {
    vi.mocked(bridge.resourceUsage).mockResolvedValueOnce(null).mockResolvedValue(usage());
    vi.useFakeTimers();
    const { container } = render(<ResourceMeter />);
    await act(async () => { await Promise.resolve(); });
    expect(container).toBeEmptyDOMElement();
    await act(async () => { vi.advanceTimersByTime(5000); await Promise.resolve(); });
    const button = screen.getByRole("button", { name: /Themis resource use: CPU 20%, memory 1.5 GB/ });
    expect(button).toHaveTextContent("20%");
    expect(button.querySelector(".is-warning")).toBeNull();
    fireEvent.click(button);
    const rows = screen.getAllByRole("row").map(row => row.textContent);
    expect(rows).toEqual(["PartCPUMemory", "App3%300 MB", "Built-in browser15%900 MB", "Tools and dev servers3%336 MB"]);
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("marks high CPU and memory that keeps growing", async () => {
    vi.mocked(bridge.resourceUsage).mockResolvedValue(usage({
      cpu_percent: 480,
      groups: [{ group: "browser", cpu_percent: 480, memory_bytes: 900 * MB, processes: 5, growing_bytes: 200 * MB }],
    }));
    render(<ResourceMeter />);
    const button = await screen.findByRole("button", { name: /CPU 60%.*memory keeps growing/ });
    expect(button.querySelectorAll(".is-warning")).toHaveLength(2);
    fireEvent.click(button);
    expect(screen.getByRole("dialog")).toHaveTextContent("Built-in browser memory grew 200 MB over the last 30 minutes");
  });
});
