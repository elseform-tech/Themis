// The rail's cloud (canvas board VisionsEntry): an outline that sums up every Vision.
import type { Mood } from "../../state/visions";
import "./Visions.css";

/** Eyes per mood on the rail's outline cloud (VisionsEntry). */
const RAIL_EYES: Record<Mood, [string, number]> = {
  needs: ["M9.5 14v2M14.5 14v2", 1.8],
  sorry: ["M10.4 14.3l-1.8 1M13.6 14.3l1.8 1", 1.6],
  work: ["M8.6 15h1.8M13.6 15h1.8", 1.8],
  think: ["M8.6 13.5h.01M13.6 13.5h.01", 2.6],
  calm: ["M9.5 15h.01M14.5 15h.01", 2.6],
  sleep: ["M8.6 14.8q.9 1 1.8 0M13.6 14.8q.9 1 1.8 0", 1.6],
};

/** The rail's cloud: an outline that sums up every Vision. `hop` plays the hop once. */
export function RailCloud({ mood, hop }: { mood: Mood; hop: boolean }) {
  const [eyes, width] = RAIL_EYES[mood];
  return (
    <svg className={`themis-rail-cloud themis-rail-cloud--${mood}${hop ? " is-hopping" : ""}`} width="26" height="18" viewBox="1 5.5 22 15.2" aria-hidden="true" focusable="false">
      <path d="M6.5 19.5h11a4 4 0 0 0 .4-7.98A6 6 0 0 0 6.6 10.1 4.7 4.7 0 0 0 6.5 19.5Z" />
      <path className="themis-cloud-eyes" d={eyes} strokeWidth={width} />
    </svg>
  );
}
