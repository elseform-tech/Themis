// A Vision's small flat avatar (board TeamChannel): its body in its flat colour, simple eyes and
// its accessory. `mood` only drives the quiet motion when `live` (Visions.css); message avatars are
// never live, and reduced motion stops every avatar.
import type { Mood } from "../../state/visions";
import { resolveLook, type Look } from "./cast";
import "./Visions.css";

/** `accessory={false}` draws the cloud without what it wears (the small sidebar and panel clouds). */
export function CharacterAvatar({ look, size = 18, mood = "calm", live = false, accessory: wears = true }: { look: Look; size?: number; mood?: Mood; live?: boolean; accessory?: boolean }) {
  const { shape, color, accessory, accessoryId } = resolveLook(look);
  const [eyes, eyeWidth] = shape.flatEyes;
  return (
    <svg className={`themis-cloud themis-cloud--${mood}${live ? " is-live" : ""}`} width={size} height={size} viewBox={wears && accessoryId === "crown" ? "0 -9 48 57" : "0 0 48 48"} aria-hidden="true" focusable="false" data-accessory={wears ? accessoryId : undefined}>
      <path d={shape.body} fill={color.flat} />
      <path className="themis-cloud-eyes" d={eyes} strokeWidth={eyeWidth} />
      {wears && <>
        <path d={accessory.line} fill="none" stroke={accessory.lineColor} strokeWidth={accessory.lineWidth} strokeLinecap="round" strokeLinejoin="round" />
        <path d={accessory.solid} fill={accessory.fill} stroke={accessory.stroke} strokeWidth="1.1" strokeLinejoin="round" />
        <path d={accessory.inner} fill="none" stroke={accessory.ink} strokeWidth={accessory.innerWidth} strokeLinecap="round" strokeLinejoin="round" />
      </>}
    </svg>
  );
}
