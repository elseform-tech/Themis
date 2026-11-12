// The Visions cast: each Vision is a filled cloud in one of six shapes and colours, with eyes that
// show its mood. Paths come from the canvas Cast (VisionsNew, VisionsEntry). A cloud moves only
// when `live`; message avatars are never live, and reduced motion stops every cloud (Visions.css).
import type { Mood } from "../../state/visions";
import "./Visions.css";

const BASE = "M12 40a8 8 0 0 1 0-16H36a8 8 0 0 1 0 16Z";

/** The shapes a Vision can take, by the id saved in its profile, with their names. */
export const SHAPES: Record<string, { label: string; body: string }> = {
  tall: { label: "Tall cloud", body: `${BASE}M7 23a8 8 0 1 1 16 0a8 8 0 1 1 -16 0ZM14 16a11 11 0 1 1 22 0a11 11 0 1 1 -22 0ZM28 23a7 7 0 1 1 14 0a7 7 0 1 1 -14 0Z` },
  flat: { label: "Flat cloud", body: `${BASE}M6 22a8 8 0 1 1 16 0a8 8 0 1 1 -16 0ZM15 20a9 9 0 1 1 18 0a9 9 0 1 1 -18 0ZM26 22a8 8 0 1 1 16 0a8 8 0 1 1 -16 0Z` },
  "two-bump": { label: "Two-bump cloud", body: `${BASE}M8 18a10 10 0 1 1 20 0a10 10 0 1 1 -20 0ZM22 21a9 9 0 1 1 18 0a9 9 0 1 1 -18 0Z` },
  leaning: { label: "Leaning cloud", body: `${BASE}M7 24a7 7 0 1 1 14 0a7 7 0 1 1 -14 0ZM13 17a9 9 0 1 1 18 0a9 9 0 1 1 -18 0ZM23.5 20a8.5 8.5 0 1 1 17 0a8.5 8.5 0 1 1 -17 0Z` },
  wide: { label: "Wide cloud", body: `${BASE}M5.5 24a7.5 7.5 0 1 1 15 0a7.5 7.5 0 1 1 -15 0ZM14 19a10 10 0 1 1 20 0a10 10 0 1 1 -20 0ZM28.5 24a7.5 7.5 0 1 1 15 0a7.5 7.5 0 1 1 -15 0Z` },
  fluffy: { label: "Fluffy cloud", body: `${BASE}M5.5 24a6.5 6.5 0 1 1 13 0a6.5 6.5 0 1 1 -13 0ZM11.5 18a7.5 7.5 0 1 1 15 0a7.5 7.5 0 1 1 -15 0ZM20 16a8 8 0 1 1 16 0a8 8 0 1 1 -16 0ZM29 21a7 7 0 1 1 14 0a7 7 0 1 1 -14 0Z` },
};

/** The colours a Vision can wear, by the id saved in its profile (the `--themis-cast-*` tokens). */
export const COLORS: Record<string, string> = { blue: "Blue", teal: "Teal", violet: "Violet", amber: "Amber", rose: "Rose", indigo: "Indigo" };

/** Eyes per mood on the cast's 48-unit cloud: path and stroke width. */
const EYES: Record<Mood, [string, number]> = {
  calm: ["M18 30h.01M30 30h.01", 5],
  think: ["M17 27h.01M29 27h.01", 5],
  work: ["M15 31h6M27 31h6", 3.4],
  needs: ["M18 27v6M30 27v6", 4.6],
  sorry: ["M21 28.5l-6 3M27 28.5l6 3", 3.4],
  sleep: ["M15 32h6M27 32h6", 3],
};

export function CloudIcon({ shape, color, mood, size = 18, live = false }: { shape: string; color: string; mood: Mood; size?: number; live?: boolean }) {
  const body = (SHAPES[shape] ?? SHAPES.tall!).body;
  const tint = color in COLORS ? color : "blue";
  const [eyes, width] = EYES[mood];
  return (
    <svg className={`themis-cloud themis-cloud--${tint} themis-cloud--${mood}${live ? " is-live" : ""}`} width={size} height={size} viewBox="0 0 48 48" aria-hidden="true" focusable="false">
      <path d={body} fill="currentColor" />
      <path className="themis-cloud-eyes" d={eyes} strokeWidth={width} />
    </svg>
  );
}

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
