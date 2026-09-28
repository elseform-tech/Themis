// Diagnostics copy flow, minus the DOM. The Settings screen wires `load`
// to getDiagnostics and `writeClipboard` to navigator.clipboard; unit tests
// inject mocks. Secret values never appear in diagnostics — the backend only
// sends a settings snapshot and recent errors (see docs/user-guide.md).
import type { Diagnostics } from "../lib/types";
import { failureMessage } from "../lib/errors";

export function formatDiagnosticsJson(diagnostics: Diagnostics): string {
  return JSON.stringify(diagnostics, null, 2);
}

export type DiagnosticsTone = "success" | "danger";

export interface CopyDiagnosticsDeps {
  load: () => Promise<Diagnostics>;
  writeClipboard: (text: string) => Promise<void>;
  notify: (message: string, tone: DiagnosticsTone) => void;
}

/**
 * Loads diagnostics, copies the pretty-printed JSON to the clipboard, and
 * notifies. Returns the payload on success (for inline display) and null
 * when either step fails.
 */
export async function copyDiagnostics(
  deps: CopyDiagnosticsDeps,
): Promise<Diagnostics | null> {
  let diagnostics: Diagnostics;
  try {
    diagnostics = await deps.load();
  } catch (error: unknown) {
    deps.notify(`Diagnostics failed: ${failureMessage(error)}`, "danger");
    return null;
  }
  try {
    await deps.writeClipboard(formatDiagnosticsJson(diagnostics));
  } catch (error: unknown) {
    deps.notify(`Copy failed: ${failureMessage(error)}`, "danger");
    return null;
  }
  deps.notify("Diagnostics copied to clipboard", "success");
  return diagnostics;
}
