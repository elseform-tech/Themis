// Builds one character in three.js (board Cloud3D): one body extruded from the rail icon's outline
// with rounded (bevelled) edges, the face plane (eyes and brows only), and the accessory from simple
// primitives in the board's colours. Scene units are the board's SVG units on its 48 box (x right,
// y down, z toward the viewer); the root flips y so it reads upright.
import * as THREE from "three";
import { SVGLoader } from "three/examples/jsm/loaders/SVGLoader.js";
import { EYES, OUTLINE, resolveLook, type Look } from "../cast";
import { drawFace, faceFor, FACE_SCALE, FACE_UNITS } from "./faces";
import type { Expression } from "../Expression";

export interface Character {
  /** The group to add to the scene (already flipped and centred on the board's frame). */
  root: THREE.Group;
  setExpression(expression: Expression, blink: boolean): void;
  dispose(): void;
}

/** The body's depth between its rounded edges, and how far the edges round. */
const DEPTH = 10;
const BEVEL = 3;
/** Where the body's flat front is, in scene units toward the viewer. */
export const FRONT_Z = DEPTH / 2 + BEVEL;

/** The outline as a three.js shape, read from the same SVG path the 2D avatars draw. */
function outlineShape(): THREE.Shape {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg"><path d="${OUTLINE}"/></svg>`;
  return SVGLoader.createShapes(new SVGLoader().parse(svg).paths[0]!)[0]!;
}

/** The body: the outline extruded and bevelled inward, so its silhouette is the outline itself. */
function bodyGeometry(): THREE.ExtrudeGeometry {
  const geometry = new THREE.ExtrudeGeometry(outlineShape(), { depth: DEPTH, bevelEnabled: true, bevelThickness: BEVEL, bevelSize: BEVEL, bevelOffset: -BEVEL, bevelSegments: 6, curveSegments: 24 });
  geometry.translate(0, 0, -DEPTH / 2);
  return geometry;
}

const material = (color: string, extra: THREE.MeshStandardMaterialParameters = {}) => new THREE.MeshStandardMaterial({ color, roughness: 0.55, metalness: 0, ...extra });

/** Small builders that add meshes to one accessory group. */
function parts(group: THREE.Group) {
  const mesh = (geometry: THREE.BufferGeometry, color: string, x: number, y: number, z: number, extra?: THREE.MeshStandardMaterialParameters) => {
    const made = new THREE.Mesh(geometry, material(color, extra));
    made.position.set(x, y, z); group.add(made);
    return made;
  };
  const scaled = (made: THREE.Mesh, x: number, y: number, z: number) => { made.scale.set(x, y, z); return made; };
  return {
    mesh, scaled,
    ring: (cx: number, cy: number, rx: number, ry: number, tube: number, color: string, z: number) => scaled(mesh(new THREE.TorusGeometry(1, tube / rx, 12, 32), color, cx, cy, z), rx, ry, rx),
    glass: (cx: number, cy: number, rx: number, ry: number, color: string, opacity: number, z: number) => scaled(mesh(new THREE.CircleGeometry(1, 32), color, cx, cy, z, { transparent: true, opacity, depthWrite: false }), rx, ry, 1),
    rod: (x0: number, y0: number, x1: number, y1: number, radius: number, color: string, z: number) => {
      const made = mesh(new THREE.CylinderGeometry(radius, radius, Math.hypot(x1 - x0, y1 - y0), 10), color, (x0 + x1) / 2, (y0 + y1) / 2, z);
      made.rotation.z = Math.atan2(y1 - y0, x1 - x0) + Math.PI / 2;
    },
    plate: (points: Array<[number, number]>, color: string, depth: number, z: number) => {
      const geometry = new THREE.ExtrudeGeometry(new THREE.Shape(points.map(([x, y]) => new THREE.Vector2(x, y))), { depth, bevelEnabled: true, bevelThickness: 0.5, bevelSize: 0.5, bevelSegments: 2 });
      geometry.translate(0, 0, z - depth / 2);
      mesh(geometry, color, 0, 0, 0);
    },
  };
}

type Parts = ReturnType<typeof parts>;
/** Just in front of the face. */
const ON_FRONT = FRONT_Z + 0.4;
const { l, r, y: eyeY } = EYES;

/** Each accessory on the outline: Cloud3D's 2D geometry given depth. */
const ACCESSORY_BUILDERS: Record<string, (p: Parts) => void> = {
  crown: p => p.plate([[17.5, 16], [17.5, 8.6], [20.7, 12.2], [24, 6.6], [27.3, 12.2], [30.5, 8.6], [30.5, 16]], "#F5C542", 2, FRONT_Z - 0.4),
  hard_hat: p => {
    const dome = p.scaled(p.mesh(new THREE.SphereGeometry(12, 32, 16, 0, Math.PI * 2, 0, Math.PI / 2), "#F6C343", 23.5, 18.2, 0), 1, 0.78, 0.92);
    dome.rotation.x = Math.PI;
    p.scaled(p.mesh(new THREE.CylinderGeometry(16, 16, 1.2, 40), "#B8862A", 23.5, 18.6, 0), 1, 1, 0.78);
  },
  glasses: p => {
    for (const x of [l, r]) { p.ring(x, eyeY, 4, 4, 0.45, "#0B1220", ON_FRONT + 0.2); p.glass(x, eyeY, 4, 4, "#FFFFFF", 0.28, ON_FRONT); }
    p.rod(23.4, 28.6, 24.6, 28.6, 0.4, "#0B1220", ON_FRONT + 0.2);
  },
  goggles: p => {
    p.ring(22.7, 20.6, 12.6, 9.6, 1.1, "#7A4A1E", 0).rotation.x = Math.PI / 2;
    for (const x of [l, r]) { p.ring(x, 20.6, 3.6, 3.2, 0.7, "#7A4A1E", ON_FRONT + 0.6); p.glass(x, 20.6, 3.6, 3.2, "#CFEBFF", 0.85, ON_FRONT + 0.4); }
  },
  headphones: p => {
    const band = new THREE.CubicBezierCurve3(new THREE.Vector3(5, 28, 0), new THREE.Vector3(5, 6.5, 0), new THREE.Vector3(42, 6.5, 0), new THREE.Vector3(42, 28, 0));
    p.mesh(new THREE.TubeGeometry(band, 40, 1.2, 8), "#334155", 0, 0, 0);
    for (const x of [4.2, 42.8]) p.scaled(p.mesh(new THREE.SphereGeometry(1, 24, 16), "#334155", x, 29.5, 0), 2.7, 5, 9.6);
  },
  magnifier: p => { p.ring(38, 32, 5.5, 5.5, 0.9, "#475569", ON_FRONT + 0.6); p.glass(38, 32, 5.5, 5.5, "#CFEBFF", 0.55, ON_FRONT + 0.4); p.rod(42.3, 36.3, 47, 42, 1.4, "#7C5A3A", ON_FRONT + 0.6); },
  clipboard: p => {
    p.mesh(new THREE.BoxGeometry(11.6, 14, 1.2), "#F8FAFC", 40, 37, ON_FRONT + 0.6);
    for (const lineY of [34.4, 39.4]) {
      p.mesh(new THREE.BoxGeometry(2.6, 0.7, 0.3), "#334155", 42.1, lineY, ON_FRONT + 1.3);
      p.rod(36.6, lineY, 37.6, lineY + 1, 0.35, "#334155", ON_FRONT + 1.3);
      p.rod(37.6, lineY + 1, 39.3, lineY - 1, 0.35, "#334155", ON_FRONT + 1.3);
    }
  },
};

/** The character for `look`, wearing `expression`. */
export function buildCharacter(look: Look, expression: Expression): Character {
  const { color, accessoryId } = resolveLook(look);
  const root = new THREE.Group();
  const body = new THREE.Group();
  const shell = new THREE.Mesh(bodyGeometry(), material(color.mid));
  shell.name = "body";
  body.add(shell);
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = FACE_UNITS * FACE_SCALE;
  const ctx = canvas.getContext("2d")!;
  const texture = new THREE.CanvasTexture(canvas);
  texture.colorSpace = THREE.SRGBColorSpace; texture.flipY = false;
  const face = new THREE.Mesh(new THREE.PlaneGeometry(FACE_UNITS, FACE_UNITS), new THREE.MeshBasicMaterial({ map: texture, transparent: true, depthWrite: false }));
  face.position.set(24, 24, FRONT_Z + 0.05);
  body.add(face);
  const accessory = new THREE.Group();
  ACCESSORY_BUILDERS[accessoryId]?.(parts(accessory));
  body.add(accessory);
  root.add(body);
  root.scale.set(1, -1, 1); root.position.set(-24, 24, 0);
  const setExpression = (next: Expression, blink: boolean) => { drawFace(ctx, faceFor(next, l, r, eyeY), eyeY, blink); texture.needsUpdate = true; };
  setExpression(expression, false);
  return {
    root, setExpression,
    dispose() {
      body.traverse(each => {
        const mesh = each as THREE.Mesh;
        mesh.geometry?.dispose();
        for (const m of ([] as THREE.Material[]).concat(mesh.material ?? [])) m.dispose();
      });
      texture.dispose();
    },
  };
}
