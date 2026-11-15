// A Vision's small flat avatar (board TeamChannel): the one cloud outline in its flat colour,
// simple eyes and its accessory (`accessory={false}` leaves it off, for the smallest sizes). `mood`
// only drives the quiet motion when `live` (Visions.css); message avatars are never live, and
// reduced motion stops every avatar.
import type { Mood } from "../../state/visions";
import { FLAT_EYES, resolveLook, type Look } from "./cast";
import "./Visions.css";

export function CharacterAvatar({ look, size = 18, mood = "calm", live = false, accessory: worn = true }: { look: Look; size?: number; mood?: Mood; live?: boolean; accessory?: boolean }) {
  const { shape, color, accessory, accessoryId } = resolveLook(look);
  const [eyes, eyeWidth] = FLAT_EYES;
  return (
    <svg className={`themis-cloud themis-cloud--${mood}${live ? " is-live" : ""}`} width={size} height={size} viewBox="0 0 48 48" aria-hidden="true" focusable="false" data-accessory={worn ? accessoryId : "none"}>
      <path d={shape.body} fill={color.flat} />
      <path className="themis-cloud-eyes" d={eyes} strokeWidth={eyeWidth} />
      {worn && (
        <>
          <path d={accessory.line} fill="none" stroke={accessory.lineColor} strokeWidth={accessory.lineWidth} strokeLinecap="round" strokeLinejoin="round" />
          <path d={accessory.solid} fill={accessory.fill} stroke={accessory.stroke} strokeWidth=".9" strokeLinejoin="round" />
          <path d={accessory.inner} fill="none" stroke={accessory.ink} strokeWidth={accessory.innerWidth} strokeLinecap="round" strokeLinejoin="round" />
        </>
      )}
    </svg>
  );
}
