// The three.js scene for one chief: renderer, soft light, ground shadow, the character and its
// loop. Loaded on demand by ChiefScene (the library stays out of the first paint and out of
// tests). The loop runs only between `setRunning(true)` and `setRunning(false)`.
import * as THREE from "three";
import type { Look } from "../cast";
import type { Expression } from "../Expression";
import { buildCharacter, type Character } from "./buildCharacter";
import { blinking, motionOffset } from "./faces";

export interface SceneHandle {
  setRunning(running: boolean): void;
  setLook(look: Look): void;
  setExpression(expression: Expression): void;
  dispose(): void;
}

/** `size` is the canvas edge in CSS pixels (a hidden canvas has no measurable size). */
export interface SceneOptions { look: Look; expression: Expression; size: number; onLost: () => void }

/** Frames per second the loop draws at; the motion is slow and a head is small. */
const FPS = 30;
/** Where the body's feet are in scene units: the pivot of its breathing. */
const FEET_Y = 24 - 39;

function groundShadow(): THREE.Mesh {
  const canvas = document.createElement("canvas");
  canvas.width = 128; canvas.height = 32;
  const ctx = canvas.getContext("2d")!;
  const gradient = ctx.createRadialGradient(64, 16, 0, 64, 16, 64);
  gradient.addColorStop(0, "rgba(8,20,45,.32)"); gradient.addColorStop(1, "rgba(8,20,45,0)");
  ctx.save(); ctx.scale(1, 0.25); ctx.fillStyle = gradient; ctx.fillRect(0, 0, 128, 128); ctx.restore();
  const texture = new THREE.CanvasTexture(canvas);
  texture.colorSpace = THREE.SRGBColorSpace;
  const mesh = new THREE.Mesh(new THREE.PlaneGeometry(40, 8), new THREE.MeshBasicMaterial({ map: texture, transparent: true, depthWrite: false }));
  mesh.position.set(-0.5, 24 - 43.6, -12);
  return mesh;
}

export function startScene(canvas: HTMLCanvasElement, options: SceneOptions): SceneHandle {
  const renderer = new THREE.WebGLRenderer({ canvas, antialias: true, alpha: true });
  try {
    return assemble(renderer, canvas, options);
  } catch (error) {
    renderer.dispose();
    renderer.forceContextLoss();
    throw error;
  }
}

function assemble(renderer: THREE.WebGLRenderer, canvas: HTMLCanvasElement, options: SceneOptions): SceneHandle {
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
  renderer.setSize(options.size, options.size, false);
  const scene = new THREE.Scene();
  // 48 units tall at the character's distance: the board's frame, accessories included.
  const camera = new THREE.PerspectiveCamera(18, 1, 1, 600);
  camera.position.set(0, 0, 24 / Math.tan((9 * Math.PI) / 180));
  scene.add(new THREE.HemisphereLight("#EAF4FF", "#7E8FB8", 1.6));
  const sun = new THREE.DirectionalLight("#FFFFFF", 1.8);
  sun.position.set(-30, 40, 80);
  scene.add(sun);
  const shadow = groundShadow();
  scene.add(shadow);
  const pivot = new THREE.Group();
  scene.add(pivot);

  let expression = options.expression;
  let character: Character = buildCharacter(options.look, expression);
  pivot.add(character.root);
  let frame = 0;
  let blink = false;
  let last = 0;
  const start = performance.now();

  const draw = (now: number) => {
    const t = (now - start) / 1000;
    const shut = blinking(expression, t);
    if (shut !== blink) { blink = shut; character.setExpression(expression, blink); }
    const { scale, dx, dy } = motionOffset(expression, t);
    pivot.scale.setScalar(scale);
    pivot.position.set(dx, dy + FEET_Y * (1 - scale), 0);
    renderer.render(scene, camera);
  };
  const tick = (now: number) => {
    frame = requestAnimationFrame(tick);
    if (now - last < 1000 / FPS - 1) return;
    last = now; draw(now);
  };
  const onLost = (event: Event) => { event.preventDefault(); stop(); options.onLost(); };
  const stop = () => { if (frame) cancelAnimationFrame(frame); frame = 0; };
  canvas.addEventListener("webglcontextlost", onLost);
  draw(start);

  return {
    setRunning(running) {
      if (!running) { stop(); return; }
      if (!frame) { last = 0; frame = requestAnimationFrame(tick); }
    },
    setLook(look) {
      pivot.remove(character.root); character.dispose();
      character = buildCharacter(look, expression); pivot.add(character.root);
      blink = false;
      if (!frame) draw(performance.now());
    },
    setExpression(next) {
      expression = next; blink = false; character.setExpression(next, false);
      if (!frame) draw(performance.now());
    },
    dispose() {
      stop();
      canvas.removeEventListener("webglcontextlost", onLost);
      character.dispose();
      shadow.geometry.dispose();
      const shadowMaterial = shadow.material as THREE.MeshBasicMaterial;
      shadowMaterial.map?.dispose(); shadowMaterial.dispose();
      renderer.dispose();
      renderer.forceContextLoss();
    },
  };
}
