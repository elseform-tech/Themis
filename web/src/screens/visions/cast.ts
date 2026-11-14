// The Visions cast (canvas board ChiefMoods, flat avatars from TeamChannel). A Vision's look is
// `{shape, color, accessory}`: the shape gives the body circles, eye positions and highlight, the
// colour a hi/mid/lo ramp (3D pictures) and a flat tint (small avatars), the accessory its geometry.
// The seven board characters are presets; every combination renders. All geometry is the board's,
// on its 48-unit square.
import { ADMIN_VISION_ID } from "../../lib/visions";

/** What a Vision wears and is shaped like, as saved in its profile. */
export interface Look { shape: string; color: string; accessory: string }

/** The shared body base every cloud sits on (the flat bottom). */
const BASE = "M12 40a8 8 0 0 1 0-16H36a8 8 0 0 1 0 16Z";
const circle = (cx: number, cy: number, r: number) => `M${cx - r} ${cy}a${r} ${r} 0 1 1 ${2 * r} 0a${r} ${r} 0 1 1 ${-2 * r} 0Z`;

export interface Shape {
  label: string;
  /** Body path: the flat base plus the bumps. */
  body: string;
  /** Eye centres and height for the shaded face. */
  eyes: { l: number; r: number; y: number };
  /** The highlight's centre. */
  hx: number; hy: number;
  /** Flat avatars' eyes: path and stroke width. */
  flatEyes: [string, number];
}

/** The shapes, by the id saved in a profile. `broad` is Atlas's. */
export const SHAPES: Record<string, Shape> = {
  tall: { label: "Tall cloud", body: BASE + circle(15, 23, 8) + circle(25, 16, 11) + circle(35, 23, 7), eyes: { l: 17.5, r: 27, y: 29 }, hx: 20, hy: 10.5, flatEyes: ["M14 30h.01M25 30h.01", 5] },
  broad: { label: "Broad cloud", body: BASE + circle(16, 22, 9) + circle(24, 17, 10.5) + circle(33, 22, 8.5), eyes: { l: 18.5, r: 29.5, y: 29.5 }, hx: 20, hy: 12, flatEyes: ["M18 30h.01M30 30h.01", 5] },
  flat: { label: "Flat cloud", body: BASE + circle(14, 22, 8) + circle(24, 20, 9) + circle(34, 22, 8), eyes: { l: 18, r: 30, y: 30.5 }, hx: 20, hy: 17.5, flatEyes: ["M15 31h6M27 31h6", 3.4] },
  "two-bump": { label: "Two-bump cloud", body: BASE + circle(18, 18, 10) + circle(31, 21, 9), eyes: { l: 17, r: 31, y: 30 }, hx: 16, hy: 11, flatEyes: ["M17 30h.01M31 30h.01", 3.4] },
  leaning: { label: "Leaning cloud", body: BASE + circle(14, 24, 7) + circle(22, 17, 9) + circle(32, 20, 8.5), eyes: { l: 19, r: 30, y: 30.5 }, hx: 20, hy: 12.5, flatEyes: ["M19 31h.01M30 31h.01", 4.6] },
  wide: { label: "Wide cloud", body: BASE + circle(13, 24, 7.5) + circle(24, 19, 10) + circle(36, 24, 7.5), eyes: { l: 18.5, r: 29.5, y: 30.5 }, hx: 20, hy: 13.5, flatEyes: ["M16 31h5M27 31h5", 3] },
  fluffy: { label: "Fluffy cloud", body: BASE + circle(12, 24, 6.5) + circle(19, 18, 7.5) + circle(28, 16, 8) + circle(36, 21, 7), eyes: { l: 16, r: 25.5, y: 30.5 }, hx: 24, hy: 10.5, flatEyes: ["M16 31h.01M25.5 31h.01", 4.4] },
};

export interface Color {
  label: string;
  /** The 3D ramp: highlight, body, shade. */
  hi: string; mid: string; lo: string;
  /** The flat tint of small avatars and swatches. */
  flat: string;
}

/** The colours, by the id saved in a profile. `indigo` and `coral`'s flat tint are not drawn on the boards. */
export const COLORS: Record<string, Color> = {
  blue: { label: "Blue", hi: "#D4F0FF", mid: "#5DBDF6", lo: "#1A74BE", flat: "#38A9F0" },
  slate: { label: "Slate", hi: "#E6ECF7", mid: "#93A2BE", lo: "#4E5B76", flat: "#7C8AA5" },
  teal: { label: "Teal", hi: "#C9FFF6", mid: "#2DD4BF", lo: "#0F766E", flat: "#14B8A6" },
  violet: { label: "Violet", hi: "#E9E4FF", mid: "#9D90FA", lo: "#5B4BD0", flat: "#8B7CF6" },
  amber: { label: "Amber", hi: "#FFF0C2", mid: "#FBBF24", lo: "#C2770A", flat: "#F59E0B" },
  rose: { label: "Rose", hi: "#FFE0EA", mid: "#F47A9A", lo: "#B83A60", flat: "#F0668A" },
  coral: { label: "Coral", hi: "#FFE1DA", mid: "#FB8A74", lo: "#C2462F", flat: "#F4795F" },
  indigo: { label: "Indigo", hi: "#DDE2FF", mid: "#8C98F6", lo: "#3F4DC4", flat: "#6C7BF2" },
};

export interface Accessory {
  label: string;
  /** A stroke drawn under the solid part (a band), its colour and width. */
  line: string; lineColor: string; lineWidth: number;
  /** The solid part: fill and outline. */
  solid: string; fill: string; stroke: string;
  /** Detail strokes on top, their ink and width. */
  inner: string; ink: string; innerWidth: number;
}

const accessory = (label: string, rest: Partial<Accessory>): Accessory => ({ label, line: "", lineColor: "none", lineWidth: 2.4, solid: "", fill: "none", stroke: "none", inner: "", ink: "none", innerWidth: 1.6, ...rest });

/** The accessories, by the id shared with the server. */
export const ACCESSORIES: Record<string, Accessory> = {
  none: accessory("None", {}),
  clipboard: accessory("Clipboard", { solid: "M36 28h9a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-9a2 2 0 0 1-2-2v-12a2 2 0 0 1 2-2Z", fill: "#F8FAFC", stroke: "#64748B", inner: "M38.6 26.8h4M36.5 33.5l1.2 1.2 2-2.4M42 33.5h2.5M36.5 39l1.2 1.2 2-2.4M42 39h2.5", ink: "#334155", innerWidth: 1.2 }),
  crown: accessory("Crown", { solid: "M15 9V-3l4.5 5 4.5-7 4.5 7 4.5-5V9Z", fill: "#F5C542", stroke: "#C9962C", inner: "M18.5 5.5h.01M24 5.5h.01M29.5 5.5h.01", ink: "#FFF4C2", innerWidth: 2 }),
  hard_hat: accessory("Hard hat", { line: "M7 14.5H41", lineColor: "#C9962C", lineWidth: 3, solid: "M11 14C11 -2 37 -2 37 14Z", fill: "#F6C343", stroke: "#C9962C", inner: "M24 2.5V8", ink: "#C9962C", innerWidth: 2 }),
  glasses: accessory("Glasses", { solid: "M11.5 30a5.5 5.5 0 1 1 11 0a5.5 5.5 0 1 1-11 0ZM25.5 30a5.5 5.5 0 1 1 11 0a5.5 5.5 0 1 1-11 0Z", fill: "rgba(255,255,255,.3)", stroke: "#0B1220", inner: "M22.5 29q1.5-1.5 3 0", ink: "#0B1220", innerWidth: 1.4 }),
  goggles: accessory("Goggles", { line: "M8 20H40", lineColor: "#8A5A2B", lineWidth: 3, solid: "M13 20a5 5 0 1 1 10 0a5 5 0 1 1-10 0ZM25 20a5 5 0 1 1 10 0a5 5 0 1 1-10 0Z", fill: "#CFEBFF", stroke: "#8A5A2B", inner: "M16 18.5l2-1.5M28 18.5l2-1.5", ink: "#FFFFFF", innerWidth: 1.4 }),
  headphones: accessory("Headphones", { line: "M5 27C5 2 43 2 43 27", lineColor: "#334155", lineWidth: 2.6, solid: "M1.5 24h6v11h-6ZM40.5 24h6v11h-6Z", fill: "#334155", stroke: "#1E293B" }),
  magnifier: accessory("Magnifier", { line: "M42.3 36.3L47 42", lineColor: "#7C5A3A", lineWidth: 3, solid: "M32.5 32a5.5 5.5 0 1 1 11 0a5.5 5.5 0 1 1-11 0Z", fill: "rgba(207,235,255,.55)", stroke: "#475569", inner: "M35 30.4q.9-1.6 2.6-2", ink: "#FFFFFF", innerWidth: 1.2 }),
};

/** The seven characters on the board: each a shape, colour and accessory of its own. */
export const PRESETS: Array<{ name: string; role: string; look: Look }> = [
  { name: "Juno", role: "Chief of staff", look: { shape: "tall", color: "blue", accessory: "clipboard" } },
  { name: "Atlas", role: "Admin", look: { shape: "broad", color: "slate", accessory: "crown" } },
  { name: "Rook", role: "Builder", look: { shape: "flat", color: "teal", accessory: "hard_hat" } },
  { name: "Wren", role: "Reviewer", look: { shape: "two-bump", color: "violet", accessory: "glasses" } },
  { name: "Pip", role: "Scout", look: { shape: "leaning", color: "amber", accessory: "goggles" } },
  { name: "Sol", role: "Watcher", look: { shape: "wide", color: "rose", accessory: "headphones" } },
  { name: "Tess", role: "UI tester", look: { shape: "fluffy", color: "coral", accessory: "magnifier" } },
];

/** The shape, colour and accessory to draw: an unknown id falls back to the first shape, blue and none. */
export function resolveLook(look: Look): { shape: Shape; color: Color; accessory: Accessory; accessoryId: string } {
  const accessoryId = look.accessory in ACCESSORIES ? look.accessory : "none";
  return { shape: SHAPES[look.shape] ?? SHAPES.tall!, color: COLORS[look.color] ?? COLORS.blue!, accessory: ACCESSORIES[accessoryId]!, accessoryId };
}

/** A Vision's look from its profile. Atlas, the built-in admin, always wears the crown until its
 *  profile says otherwise; any other Vision without an accessory wears none. */
export function lookOf(vision: { id?: string; shape: string; color: string; accessory?: string }): Look {
  return { shape: vision.shape, color: vision.color, accessory: vision.accessory ?? (vision.id === ADMIN_VISION_ID ? "crown" : "none") };
}
