import { useEffect, useId, useMemo, useRef, useState, type CSSProperties } from "react";
import honey from "../assets/knight-honey.svg?raw";
import pearl from "../assets/knight-pearl.svg?raw";
import ink from "../assets/knight-ink.svg?raw";
import { Dialog, Tooltip } from "./index";
import { readSession } from "../state/session";
import "./KnightCompanion.css";

const knights = { honey: { name: "Honey", svg: honey }, pearl: { name: "Pearl", svg: pearl }, ink: { name: "Ink", svg: ink } };
export type KnightVariant = keyof typeof knights;
export type KnightMood = "idle" | "working" | "complete" | "attention";
export interface CompanionPreferences {
  variant: KnightVariant;
  enabled: boolean;
  size: number;
  position: { x: number; y: number };
}
const clamp = (value: number) => Math.max(0, Math.min(1, value));
export function readCompanionPreferences(): CompanionPreferences {
  const saved = readSession<Partial<CompanionPreferences> | null>("companion", null);
  return {
    variant: saved?.variant && Object.prototype.hasOwnProperty.call(knights, saved.variant) ? saved.variant : "honey",
    enabled: typeof saved?.enabled === "boolean" ? saved.enabled : true,
    size: typeof saved?.size === "number" && Number.isFinite(saved.size) ? Math.max(56, Math.min(192, saved.size)) : 88,
    position: {
      x: typeof saved?.position?.x === "number" && Number.isFinite(saved.position.x) ? clamp(saved.position.x) : 1,
      y: typeof saved?.position?.y === "number" && Number.isFinite(saved.position.y) ? clamp(saved.position.y) : .65,
    },
  };
}

// Only trusted, bundled SVG artwork is inlined. Instance IDs isolate gradients.
export function KnightArtwork({ variant, mood = "idle", icon = false }: { variant: KnightVariant; mood?: KnightMood; icon?: boolean }) {
  const id = useId().replace(/:/g, "");
  const markup = useMemo(() => knights[variant].svg
    .replace(/<style>[\s\S]*?<\/style>/, "")
    .replace(/id="([^"]+)"/g, (_, name: string) => `id="${id}-${name}"`)
    .replace(/url\(#([^)]+)\)/g, (_, name: string) => `url(#${id}-${name})`)
    .replace('aria-labelledby="title desc"', `aria-labelledby="${id}-title ${id}-desc"`), [id, variant]);
  return <span className={`themis-knight-art ${icon ? "is-icon" : ""}`} data-mood={mood} aria-hidden="true" dangerouslySetInnerHTML={{ __html: markup }} />;
}

export function KnightControl({ preferences, onChange }: { preferences: CompanionPreferences; onChange: (next: CompanionPreferences) => void }) {
  const [open, setOpen] = useState(false);
  return <div className="themis-knight-control">
    <Tooltip content="Knight companion"><button type="button" className="themis-sidebar-row" aria-label="Knight companion" aria-haspopup="dialog" aria-expanded={open} onClick={() => setOpen(value => !value)}><KnightArtwork variant={preferences.variant} icon /></button></Tooltip>
    <Dialog open={open} title="Knight companion" onClose={() => setOpen(false)}>
      <div className="themis-knight-choices" aria-label="Knight variants">{(Object.keys(knights) as KnightVariant[]).map(variant => <button type="button" key={variant} aria-label={knights[variant].name} aria-pressed={preferences.variant === variant} onClick={() => onChange({ ...preferences, variant })}><KnightArtwork variant={variant} icon /><span>{knights[variant].name}</span></button>)}</div>
      <label className="themis-knight-toggle"><span>Companion</span><input type="checkbox" checked={preferences.enabled} onChange={event => onChange({ ...preferences, enabled: event.target.checked })} /></label>
      <label className="themis-knight-size"><span>Size <output>{preferences.size}px</output></span><input type="range" min="56" max="192" step="4" aria-label="Size" value={preferences.size} onChange={event => onChange({ ...preferences, size: Number(event.target.value) })} /></label>
    </Dialog>
  </div>;
}

export function KnightLoading({ variant, status, progress, progressLabel = "Loading" }: { variant: KnightVariant; status: string; progress?: number; progressLabel?: string }) {
  const [look, setLook] = useState(0);
  return <div className="themis-knight-loading" aria-busy="true" onPointerMove={event => setLook(Math.max(-4, Math.min(4, (event.clientX - event.currentTarget.getBoundingClientRect().left - event.currentTarget.clientWidth / 2) / 80)))} onPointerLeave={() => setLook(0)}>
    <div className="themis-knight-progress">
    <svg className={progress === undefined ? "is-indeterminate" : ""} viewBox="0 0 180 180" role="progressbar" aria-label={progressLabel} aria-valuemin={0} aria-valuemax={100} aria-valuenow={progress}><circle className="themis-progress-track" cx="90" cy="90" r="84" /><circle className="themis-progress-value" cx="90" cy="90" r="84" pathLength="100" strokeDasharray={`${progress ?? 25} 100`} /></svg>
    <button type="button" className="themis-knight-wake" aria-label={`Wake ${knights[variant].name}`} onClick={() => setLook(value => value === 0 ? 4 : -value)} style={{ "--knight-gaze": `${look}px` } as CSSProperties}><KnightArtwork variant={variant} mood="working" /></button>
    </div>
    {progress !== undefined && <strong className="themis-loading-percent">{progress}%</strong>}
    <span className="themis-wordmark">Themis</span><p role="status">{status}</p>
  </div>;
}

export function KnightCompanion({ preferences, onChange, mood }: { preferences: CompanionPreferences; onChange: (next: CompanionPreferences) => void; mood: KnightMood }) {
  const area = useRef<HTMLDivElement>(null);
  const drag = useRef<{ pointer: number; startX: number; startY: number; x: number; y: number; width: number; height: number; moved: boolean } | null>(null);
  const [position, setPosition] = useState(preferences.position);
  const [poked, setPoked] = useState(false);
  const [look, setLook] = useState(0);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => { setPosition(preferences.position); }, [preferences.position]);
  useEffect(() => () => { if (timer.current) clearTimeout(timer.current); }, []);
  const displayedMood = mood === "idle" && poked ? "complete" : mood;
  function finishDrag(event: React.PointerEvent<HTMLButtonElement>) {
    const active = drag.current;
    if (!active || active.pointer !== event.pointerId) return;
    if (active.moved) {
      const next = position;
      setPosition(next); onChange({ ...preferences, position: next });
    }
    drag.current = null;
  }
  return <div className="themis-companion-area" ref={area}>
    <button type="button" className="themis-knight-companion" aria-label={`Move ${knights[preferences.variant].name} companion`} title="Drag to move · Arrow keys to reposition" style={{ width: preferences.size, height: preferences.size + 22, left: `${position.x * 100}%`, top: `${position.y * 100}%`, transform: `translate(${-position.x * 100}%, ${-position.y * 100}%)`, "--knight-gaze": `${look}px` } as CSSProperties}
      onPointerDown={event => {
        if (event.button !== 0 || !area.current) return;
        const bounds = area.current.getBoundingClientRect(), pet = event.currentTarget.getBoundingClientRect();
        drag.current = { pointer: event.pointerId, startX: event.clientX, startY: event.clientY, x: position.x, y: position.y, width: Math.max(1, bounds.width - pet.width), height: Math.max(1, bounds.height - pet.height), moved: false };
        event.currentTarget.setPointerCapture(event.pointerId);
      }}
      onPointerMove={event => {
        const active = drag.current;
        if (!active) { const rect = event.currentTarget.getBoundingClientRect(); setLook((event.clientX - rect.left - rect.width / 2) / 12); return; }
        if (active.pointer !== event.pointerId) return;
        const dx = event.clientX - active.startX, dy = event.clientY - active.startY;
        if (Math.abs(dx) + Math.abs(dy) > 5) active.moved = true;
        if (active.moved) setPosition({ x: clamp(active.x + dx / active.width), y: clamp(active.y + dy / active.height) });
      }}
      onPointerUp={event => {
        const moved = drag.current?.moved;
        finishDrag(event);
        if (!moved) { setPoked(true); if (timer.current) clearTimeout(timer.current); timer.current = setTimeout(() => setPoked(false), 1600); }
      }}
      onPointerCancel={event => { finishDrag(event); setLook(0); }}
      onPointerLeave={() => setLook(0)}
      onKeyDown={event => {
        const steps: Record<string, [number, number]> = { ArrowLeft: [-.1, 0], ArrowRight: [.1, 0], ArrowUp: [0, -.1], ArrowDown: [0, .1] };
        const step = steps[event.key];
        if (!step) return;
        event.preventDefault();
        const next = { x: clamp(position.x + step[0]), y: clamp(position.y + step[1]) };
        setPosition(next); onChange({ ...preferences, position: next });
      }}
      onClick={event => { if (event.detail === 0) { setPoked(true); if (timer.current) clearTimeout(timer.current); timer.current = setTimeout(() => setPoked(false), 1600); } }}>
      <KnightArtwork variant={preferences.variant} mood={displayedMood} /><span className="themis-knight-grip" aria-hidden="true">⠿</span><span className="themis-sr-only">{({ idle: "Idle", working: "Working", complete: "Complete", attention: "Needs attention" })[mood]}</span>
    </button>
  </div>;
}
