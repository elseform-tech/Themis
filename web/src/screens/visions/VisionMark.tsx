// A Vision's work chat in plain chat's list: a small still cloud in its colour before the title.
// The cloud is decoration; the row's name says who runs the chat (", run by Rook").
import type { ThreadInfo } from "../../lib/types";
import type { RowMark } from "../Sidebar";
import { CloudIcon } from "./CloudIcon";
import { visionLook } from "./HandBack";

type AppState = Parameters<typeof visionLook>[0];

export function visionRowMark(state: AppState, thread: ThreadInfo): RowMark | null {
  const vision = thread.vision ? visionLook(state, thread.vision.id, thread.vision) : undefined;
  if (!vision) return null;
  return {
    mark: <span className="themis-vision-mark" aria-hidden="true"><CloudIcon shape={vision.shape} color={vision.color} mood="calm" size={14} /></span>,
    said: `, run by ${vision.name}`,
  };
}
