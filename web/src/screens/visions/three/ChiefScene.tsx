// The chief in 3D (boards ChiefMoods and TeamChannel): the board's look as a lit three.js scene
// with a soft glow and a ground shadow, animating its expression. It draws only while on screen
// and the tab is visible. With reduced motion, without WebGL, when the scene fails to start or
// its context is lost, the still picture shows instead.
import { useEffect, useRef, useState } from "react";
import type { Look } from "../cast";
import type { Expression } from "../Expression";
import { StillPicture } from "../StillPicture";
import { reducedMotion } from "../thinking";
import type { SceneHandle } from "./sceneRuntime";

/** What the browser can do; injected so tests can pick a world. */
export interface SceneEnv { reducedMotion(): boolean; hasWebGL(): boolean }

export const browserEnv: SceneEnv = {
  reducedMotion,
  hasWebGL: () => typeof WebGLRenderingContext !== "undefined",
};

/** Whether the scene is on screen and the tab is in front (the loop runs only then). */
function useShouldRun(box: React.RefObject<HTMLElement | null>, enabled: boolean): boolean {
  const [onScreen, setOnScreen] = useState(false);
  const [tabVisible, setTabVisible] = useState(() => document.visibilityState !== "hidden");
  useEffect(() => {
    if (!enabled || !box.current || typeof IntersectionObserver === "undefined") return;
    const observer = new IntersectionObserver(entries => setOnScreen(entries[entries.length - 1]?.isIntersecting ?? false));
    observer.observe(box.current);
    return () => observer.disconnect();
  }, [box, enabled]);
  useEffect(() => {
    if (!enabled) return;
    const change = () => setTabVisible(document.visibilityState !== "hidden");
    document.addEventListener("visibilitychange", change);
    return () => document.removeEventListener("visibilitychange", change);
  }, [enabled]);
  return enabled && onScreen && tabVisible;
}

/** Starts the scene on `canvas` while `enabled`; `failed` turns true when it cannot start or its context is lost. */
function useScene(canvas: React.RefObject<HTMLCanvasElement | null>, enabled: boolean, first: { look: Look; expression: Expression }) {
  const [failed, setFailed] = useState(false);
  const [ready, setReady] = useState(false);
  const handle = useRef<SceneHandle | null>(null);
  useEffect(() => {
    if (!enabled || !canvas.current) return;
    let cancelled = false;
    let started: SceneHandle | null = null;
    const giveUp = () => { started?.dispose(); started = null; handle.current = null; setReady(false); setFailed(true); };
    import("./sceneRuntime").then(({ startScene }) => {
      if (cancelled || !canvas.current) return;
      started = startScene(canvas.current, { ...first, onLost: giveUp });
      handle.current = started;
      setReady(true);
    }).catch(() => { if (!cancelled) giveUp(); });
    return () => { cancelled = true; started?.dispose(); handle.current = null; setReady(false); };
  }, [canvas, enabled, first]);
  return { failed, ready, handle };
}

export function ChiefScene({ look, expression, name, size = 130, env = browserEnv }: { look: Look; expression: Expression; name?: string; size?: number; env?: SceneEnv }) {
  const box = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const first = useRef({ look, expression }).current;
  const eligible = !env.reducedMotion() && env.hasWebGL();
  const { failed, ready, handle } = useScene(canvas, eligible, first);
  const enabled = eligible && !failed;
  const running = useShouldRun(box, enabled);

  useEffect(() => { handle.current?.setRunning(running); }, [handle, running, ready]);
  useEffect(() => { handle.current?.setLook(look); }, [handle, look.shape, look.color, look.accessory]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => { handle.current?.setExpression(expression); }, [handle, expression]);

  if (!enabled) return <StillPicture look={look} expression={expression} name={name} size={size} />;
  const label = ready ? `${name ? `${name}, ` : ""}${expression}` : undefined;
  return (
    <div ref={box} className="themis-chief-scene" style={{ width: size, height: size }}>
      {!ready && <StillPicture look={look} expression={expression} name={name} size={size} />}
      <canvas ref={canvas} className="themis-chief-canvas" width={size} height={size} role={ready ? "img" : undefined} aria-label={label} hidden={!ready} />
    </div>
  );
}
