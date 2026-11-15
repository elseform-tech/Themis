// A team's level (board TwoLevels) as one segmented switch: Auto review or Full access, the latter
// drawn in the danger colour while it is on. Settings › Visions sets the level of new channels with
// it; channel details and the chief's profile set one channel's. Only the person sets a level.
import type { Level } from "../../lib/visions";

/** The levels in switch order, with their names. */
export const LEVELS: Array<[Level, string]> = [["auto_review", "Auto review"], ["full_access", "Full access"]];

/** `level` is undefined while it is not read (nothing shows as on). Choosing the level that is on
 *  still calls `onChange`; the caller skips a write that changes nothing. */
export function LevelSwitch({ label, level, onChange }: { label: string; level: Level | undefined; onChange: (level: Level) => void }) {
  return <div className="themis-visions-level" role="radiogroup" aria-label={label}>
    {LEVELS.map(([value, name]) => <button key={value} type="button" role="radio" aria-checked={level === value}
      className={value === "full_access" ? "is-danger" : undefined} onClick={() => onChange(value)}>{name}</button>)}
  </div>;
}
