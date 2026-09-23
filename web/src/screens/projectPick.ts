// Native folder picker with typed-path fallback. The typed-path dialog
// itself lives in the Sidebar; this module only resolves a directory path.
import { open } from "@tauri-apps/plugin-dialog";

export type PickResult =
  | { kind: "path"; path: string }
  | { kind: "cancelled" }
  | { kind: "unavailable" };

/**
 * Ask the OS for a project directory. Returns `cancelled` when the user
 * dismisses the dialog and `unavailable` when the native dialog cannot be
 * shown (e.g. missing backend plugin) so the caller can fall back to a
 * typed path.
 */
export async function pickProjectDirectory(): Promise<PickResult> {
  try {
    const selected = await open({
      directory: true,
      multiple: false,
      title: "Open project folder",
    });
    if (typeof selected === "string") return { kind: "path", path: selected };
    return { kind: "cancelled" };
  } catch {
    return { kind: "unavailable" };
  }
}
