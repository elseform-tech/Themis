// Builds one character in three.js: a union of spheres from the shape's circles on a capsule base,
// the face plane, and the accessory from simple primitives in the board's colours. Scene units are
// the board's SVG units (x right, y down, z toward the viewer); the root flips y so it reads upright.
import * as THREE from "three";
import { resolveLook, type Look } from "../cast";
import { drawFace, faceFor, FACE_SCALE, FACE_UNITS } from "./faces";
import type { Expression } from "../Expression";

export interface Character {
  /** The group to add to the scene (already flipped and centred on the board's frame). */
  root: THREE.Group;
  setExpression(expression: Expression, blink: boolean): void;
  dispose(): void;
}

/** Where the viewer sees the body's front at (x, y): the tallest sphere or the base's capsule. */
export function frontZ(bumps: Array<[number, number, number]>, x: number, y: number): number {
  const reach = (r: number, d: number) => (d < r ? Math.sqrt(r * r - d * d) : 0);
  const onAxis = Math.min(Math.max(x, 12), 36);
  let z = reach(8, Math.hypot(x - onAxis, y - 32));
  for (const [cx, cy, r] of bumps) z = Math.max(z, reach(r, Math.hypot(x - cx, y - cy)));
  return z;
}

const material = (color: string, extra: THREE.MeshStandardMaterialParameters = {}) => new THREE.MeshStandardMaterial({ color, roughness: 0.55, metalness: 0, ...extra });

function accessoryGroup(id: string, bumps: Array<[number, number, number]>, own: THREE.Object3D[]): THREE.Group {
  const group = new THREE.Group();
  const add = (mesh: THREE.Object3D) => { group.add(mesh); own.push(mesh); return mesh; };
  const at = (x: number, y: number, lift: number) => frontZ(bumps, x, y) + lift;
  const ring = (cx: number, cy: number, r: number, tube: number, color: string, z: number) => {
    const mesh = add(new THREE.Mesh(new THREE.TorusGeometry(r, tube, 12, 32), material(color))); mesh.position.set(cx, cy, z);
  };
  const glass = (cx: number, cy: number, r: number, color: string, opacity: number, z: number) => {
    const mesh = add(new THREE.Mesh(new THREE.CircleGeometry(r, 32), material(color, { transparent: true, opacity, depthWrite: false }))); mesh.position.set(cx, cy, z);
  };
  const rod = (x0: number, y0: number, x1: number, y1: number, radius: number, color: string, z: number) => {
    const mesh = add(new THREE.Mesh(new THREE.CylinderGeometry(radius, radius, Math.hypot(x1 - x0, y1 - y0), 10), material(color)));
    mesh.position.set((x0 + x1) / 2, (y0 + y1) / 2, z); mesh.rotation.z = Math.atan2(y1 - y0, x1 - x0) + Math.PI / 2;
  };
  const extruded = (points: Array<[number, number]>, color: string, depth: number, z: number) => {
    const shape = new THREE.Shape(points.map(([x, y]) => new THREE.Vector2(x, y)));
    const geometry = new THREE.ExtrudeGeometry(shape, { depth, bevelEnabled: true, bevelThickness: 0.6, bevelSize: 0.6, bevelSegments: 2 });
    geometry.translate(0, 0, z - depth / 2);
    add(new THREE.Mesh(geometry, material(color)));
  };
  switch (id) {
    case "crown": extruded([[15, 9], [15, -3], [19.5, 2], [24, -5], [28.5, 2], [33, -3], [33, 9]], "#F5C542", 3, 2); break;
    case "hard_hat": {
      const dome = add(new THREE.Mesh(new THREE.SphereGeometry(13, 32, 16, 0, Math.PI * 2, 0, Math.PI / 2), material("#F6C343")));
      dome.position.set(24, 14, 0); dome.rotation.x = Math.PI; dome.scale.set(1, 0.95, 1);
      rod(7, 14.5, 41, 14.5, 1.5, "#C9962C", at(24, 14.5, 0));
      break;
    }
    case "glasses": ring(17, 30, 5.5, 0.6, "#0B1220", at(17, 30, 1)); ring(31, 30, 5.5, 0.6, "#0B1220", at(31, 30, 1)); glass(17, 30, 5.5, "#FFFFFF", 0.3, at(17, 30, 0.8)); glass(31, 30, 5.5, "#FFFFFF", 0.3, at(31, 30, 0.8)); rod(22.5, 29, 25.5, 29, 0.6, "#0B1220", at(24, 29, 1)); break;
    case "goggles": rod(8, 20, 40, 20, 1.5, "#8A5A2B", at(24, 20, 0.5)); for (const x of [18, 30]) { ring(x, 20, 5, 0.8, "#8A5A2B", at(x, 20, 1.5)); glass(x, 20, 5, "#CFEBFF", 0.85, at(x, 20, 1.2)); } break;
    case "headphones": {
      const band = add(new THREE.Mesh(new THREE.TubeGeometry(new THREE.CubicBezierCurve3(new THREE.Vector3(5, 27, 0), new THREE.Vector3(5, 2, 0), new THREE.Vector3(43, 2, 0), new THREE.Vector3(43, 27, 0)), 40, 1.3, 8), material("#334155")));
      band.position.z = 0;
      for (const x of [4.5, 43.5]) { const cup = add(new THREE.Mesh(new THREE.CapsuleGeometry(3, 5, 6, 12), material("#334155"))); cup.position.set(x, 29.5, 0); }
      break;
    }
    case "magnifier": ring(38, 32, 5.5, 0.9, "#475569", 9); glass(38, 32, 5.5, "#CFEBFF", 0.55, 8.8); rod(42.3, 36.3, 47, 42, 1.4, "#7C5A3A", 9); break;
    case "clipboard": {
      const board = add(new THREE.Mesh(new THREE.BoxGeometry(13, 16, 2), material("#F8FAFC"))); board.position.set(40.5, 36, 9);
      const clip = add(new THREE.Mesh(new THREE.BoxGeometry(5, 2, 2.4), material("#64748B"))); clip.position.set(40.5, 28, 9.4);
      for (const y of [33.5, 39]) { const line = add(new THREE.Mesh(new THREE.BoxGeometry(8, 0.9, 0.4), material("#334155"))); line.position.set(40.5, y, 10.2); }
      break;
    }
    default: break;
  }
  return group;
}

/** The character for `look`, wearing `expression`. */
export function buildCharacter(look: Look, expression: Expression): Character {
  const { shape, color, accessoryId } = resolveLook(look);
  const own: THREE.Object3D[] = [];
  const root = new THREE.Group();
  const body = new THREE.Group();
  const bodyMaterial = material(color.mid);
  const keep = <T extends THREE.Object3D>(object: T): T => { body.add(object); own.push(object); return object; };
  const base = keep(new THREE.Mesh(new THREE.CapsuleGeometry(8, 24, 8, 24), bodyMaterial));
  base.rotation.z = Math.PI / 2; base.position.set(24, 32, 0);
  for (const [cx, cy, r] of shape.bumps) keep(new THREE.Mesh(new THREE.SphereGeometry(r, 40, 24), bodyMaterial)).position.set(cx, cy, 0);
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = FACE_UNITS * FACE_SCALE;
  const ctx = canvas.getContext("2d")!;
  const texture = new THREE.CanvasTexture(canvas);
  texture.colorSpace = THREE.SRGBColorSpace; texture.flipY = false;
  const faceMaterial = new THREE.MeshBasicMaterial({ map: texture, transparent: true, depthWrite: false });
  const { l, r, y } = shape.eyes;
  const face = keep(new THREE.Mesh(new THREE.PlaneGeometry(FACE_UNITS, FACE_UNITS), faceMaterial));
  face.position.set(24, 24, frontZ(shape.bumps, (l + r) / 2, y) + 0.4);
  body.add(accessoryGroup(accessoryId, shape.bumps, own));
  root.add(body);
  root.scale.set(1, -1, 1); root.position.set(-24, 19.5, 0);
  const setExpression = (next: Expression, blink: boolean) => { drawFace(ctx, faceFor(next, l, r, y), y, blink); texture.needsUpdate = true; };
  setExpression(expression, false);
  return {
    root, setExpression,
    dispose() {
      for (const object of own) object.traverse(each => {
        const mesh = each as THREE.Mesh;
        mesh.geometry?.dispose();
        for (const m of ([] as THREE.Material[]).concat(mesh.material ?? [])) m.dispose();
      });
      texture.dispose();
    },
  };
}
