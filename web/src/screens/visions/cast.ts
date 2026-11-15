// The Visions cast (canvas boards Cloud3D and TeamChannel). A Vision's look is `{shape, color,
// accessory}`. Every Vision has the same body, the rail icon's outline, with eyes only; the colour
// (a hi/mid/lo ramp for shaded pictures, a flat tint for small avatars) and the accessory tell them
// apart. The seven board characters are presets; every combination renders. All geometry is the
// board's, on its 48-unit square.
import { ADMIN_VISION_ID } from "../../lib/visions";

/** What a Vision wears and is shaped like, as saved in its profile. */
export interface Look { shape: string; color: string; accessory: string }

/** The one body outline (the rail icon's shape on the 48 box), for every Vision. */
export const OUTLINE = "M13 39h22a8 8 0 0 0 .8-15.96A12 12 0 0 0 13.2 20.2A9.4 9.4 0 0 0 13 39Z";
/** Eye centres and height on the outline. */
export const EYES = { l: 19.5, r: 28.5, y: 29 };
/** The shaded body's highlight centre. */
export const HIGHLIGHT = { x: 20.5, y: 17.4 };
/** Small avatars' eyes: path and stroke width. */
export const FLAT_EYES: [string, number] = [`M${EYES.l} ${EYES.y}h.01M${EYES.r} ${EYES.y}h.01`, 4.4];

export interface Shape {
  label: string;
  /** Body path: the one outline. */
  body: string;
  /** Eye centres and height for the shaded face. */
  eyes: { l: number; r: number; y: number };
}

/** The shape ids a profile may hold; each draws the one outline. `broad` is Atlas's. */
const SHAPE_LABELS: Record<string, string> = { tall: "Tall cloud", broad: "Broad cloud", flat: "Flat cloud", "two-bump": "Two-bump cloud", leaning: "Leaning cloud", wide: "Wide cloud", fluffy: "Fluffy cloud" };

export const SHAPES: Record<string, Shape> = Object.fromEntries(Object.entries(SHAPE_LABELS).map(([id, label]) => [id, { label, body: OUTLINE, eyes: EYES }]));

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

const ring = (cx: number, cy: number, rx: number, ry: number) => `M${cx - rx} ${cy}a${rx} ${ry} 0 1 0 ${2 * rx} 0a${rx} ${ry} 0 1 0 ${-2 * rx} 0Z`;

/** The accessories, by the id shared with the server (Cloud3D's geometry; the magnifier is not on the board). */
export const ACCESSORIES: Record<string, Accessory> = {
  none: accessory("None", {}),
  clipboard: accessory("Clipboard", { solid: "M36 30h8a1.8 1.8 0 0 1 1.8 1.8v10.4a1.8 1.8 0 0 1-1.8 1.8h-8a1.8 1.8 0 0 1-1.8-1.8V31.8A1.8 1.8 0 0 1 36 30Z", fill: "#F8FAFC", stroke: "#64748B", inner: "M36.6 34.4l1 1 1.7-2M40.8 34.4h2.6M36.6 39.4l1 1 1.7-2M40.8 39.4h2.6", ink: "#334155", innerWidth: 1 }),
  crown: accessory("Crown", { solid: "M17.5 16V8.6l3.2 3.6 3.3-5.6 3.3 5.6 3.2-3.6V16Z", fill: "#F5C542", stroke: "#B8862A" }),
  hard_hat: accessory("Hard hat", { line: "M7.5 18.6H39.5", lineColor: "#B8862A", lineWidth: 2.6, solid: "M11.5 18.2C11.5 6.8 35.5 6.8 35.5 18.2Z", fill: "#F6C343", stroke: "#B8862A", inner: "M23.5 8.6V13.6", ink: "#B8862A", innerWidth: 1 }),
  glasses: accessory("Glasses", { solid: ring(EYES.l, EYES.y, 4, 4) + ring(EYES.r, EYES.y, 4, 4), fill: "rgba(255,255,255,.28)", stroke: "#0B1220", inner: "M23.5 28.4q.5-.6 1 0", ink: "#0B1220", innerWidth: 1 }),
  goggles: accessory("Goggles", { line: "M5.5 21.6Q23.5 17 41.5 21.6", lineColor: "#7A4A1E", lineWidth: 2.4, solid: ring(EYES.l, 20.6, 3.6, 3.2) + ring(EYES.r, 20.6, 3.6, 3.2), fill: "#CFEBFF", stroke: "#7A4A1E", inner: "M17.8 19.6l1.6-1.1M26.8 19.6l1.6-1.1", ink: "#FFFFFF", innerWidth: 1 }),
  headphones: accessory("Headphones", { line: "M6 28C6 6.5 41 6.5 41 28", lineColor: "#334155", lineWidth: 2.4, solid: "M2.6 24.5h5.4v10h-5.4ZM39.4 24.5h5.4v10h-5.4Z", fill: "#334155", stroke: "#1E293B" }),
  magnifier: accessory("Magnifier", { line: "M42.3 36.3L47 42", lineColor: "#7C5A3A", lineWidth: 3, solid: ring(38, 32, 5.5, 5.5), fill: "rgba(207,235,255,.55)", stroke: "#475569", inner: "M35 30.4q.9-1.6 2.6-2", ink: "#FFFFFF", innerWidth: 1.2 }),
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
