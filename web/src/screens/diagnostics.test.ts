// Diagnostics copy-flow tests. The bridge and clipboard are injected, so
// no Tauri or DOM is needed.
import { describe, expect, it, vi } from "vitest";
import type { Diagnostics } from "../lib/types";
import { DEFAULT_SETTINGS } from "../state/reducer";
import {
  copyDiagnostics,
  formatDiagnosticsJson,
  type DiagnosticsTone,
} from "./diagnostics";

const PAYLOAD: Diagnostics = {
  app_version: "0.1.0",
  os: "macos-aarch64",
  settings: { ...DEFAULT_SETTINGS, onboarded: true },
  recent_errors: [{ at: "2026-01-01T00:00:00Z", command: "ping", message: "ok" }],
};

describe("formatDiagnosticsJson", () => {
  it("pretty-prints with indentation and round-trips", () => {
    const text = formatDiagnosticsJson(PAYLOAD);
    expect(text).toContain("\n");
    expect(text).toContain('  "app_version"');
    expect(JSON.parse(text)).toEqual(JSON.parse(JSON.stringify(PAYLOAD)));
  });

  it("never contains secret values, only the settings snapshot", () => {
    const text = formatDiagnosticsJson(PAYLOAD);
    expect(text).not.toContain("sk-");
    expect(text).toContain("onboarded");
  });
});

describe("copyDiagnostics", () => {
  it("loads, copies pretty JSON, notifies success, returns the payload", async () => {
    const load = vi.fn(async () => PAYLOAD);
    const writeClipboard = vi.fn(async (_text: string) => {});
    const notes: Array<[string, DiagnosticsTone]> = [];
    const result = await copyDiagnostics({
      load,
      writeClipboard,
      notify: (message, tone) => {
        notes.push([message, tone]);
      },
    });
    expect(load).toHaveBeenCalledTimes(1);
    expect(result).toEqual(PAYLOAD);
    expect(writeClipboard).toHaveBeenCalledTimes(1);
    const copied = writeClipboard.mock.calls[0]![0] as string;
    expect(copied).toBe(formatDiagnosticsJson(PAYLOAD));
    expect(notes).toEqual([["Diagnostics copied to clipboard", "success"]]);
  });

  it("a load failure notifies danger and copies nothing", async () => {
    const load = vi.fn(async () => {
      throw new Error("backend down");
    });
    const writeClipboard = vi.fn(async (_text: string) => {});
    const notes: Array<[string, DiagnosticsTone]> = [];
    const result = await copyDiagnostics({
      load,
      writeClipboard,
      notify: (message, tone) => {
        notes.push([message, tone]);
      },
    });
    expect(result).toBeNull();
    expect(writeClipboard).not.toHaveBeenCalled();
    expect(notes).toHaveLength(1);
    expect(notes[0]![1]).toBe("danger");
    expect(notes[0]![0]).toContain("backend down");
  });

  it("a clipboard failure notifies danger and returns null", async () => {
    const load = vi.fn(async () => PAYLOAD);
    const writeClipboard = vi.fn(async (_text: string) => {
      throw new Error("denied");
    });
    const notes: Array<[string, DiagnosticsTone]> = [];
    const result = await copyDiagnostics({
      load,
      writeClipboard,
      notify: (message, tone) => {
        notes.push([message, tone]);
      },
    });
    expect(result).toBeNull();
    expect(notes).toHaveLength(1);
    expect(notes[0]![1]).toBe("danger");
    expect(notes[0]![0]).toContain("denied");
  });
});
