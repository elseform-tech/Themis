// A Vision's work chat in plain chat's list: a small still cloud in its colour before the title.
import type { ThreadInfo } from "../../lib/types";
import { useApp } from "../../state/store";
import { CloudIcon } from "./CloudIcon";
import { visionLook } from "./HandBack";

export function VisionMark({ thread }: { thread: ThreadInfo }) {
  const { state } = useApp();
  const vision = thread.vision ? visionLook(state, thread.vision.id, thread.vision) : undefined;
  if (!vision) return null;
  return (
    <span className="themis-vision-mark" role="img" aria-label={`Run by ${vision.name}`}>
      <CloudIcon shape={vision.shape} color={vision.color} mood="calm" size={14} />
    </span>
  );
}
