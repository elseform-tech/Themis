import { useEffect, useRef, useState, type CSSProperties } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { cursorPosition, getCurrentWindow, monitorFromPoint, PhysicalPosition, LogicalSize, Window as NativeWindow } from "@tauri-apps/api/window";
import { KnightArtwork, readCompanionPreferences, type CompanionPreferences, type KnightMood } from "./KnightCompanion";
import { readSession, writeSession } from "../state/session";

// The native window uses the same local origin/preferences as the main window.
export function useDesktopCompanion(preferences: CompanionPreferences, mood: KnightMood, ready: boolean, onError: (error: unknown) => void): boolean {
  const native = isTauri();
  const placed = useRef(false);
  useEffect(() => { if (native) writeSession("companion-mood", mood); }, [native, mood]);
  useEffect(() => {
    if (!native) return;
    let cancelled = false;
    const pet = new NativeWindow("companion");
    void (async () => {
      if (!preferences.enabled || !ready) { await pet.hide(); return; }
      await pet.setSize(new LogicalSize(preferences.size + 20, preferences.size + 44));
      if (cancelled) return;
      if (!placed.current) {
        const saved = readSession<{ x: number; y: number } | null>("companion-desktop-position", null);
        if (saved && Number.isFinite(saved.x) && Number.isFinite(saved.y) && await monitorFromPoint(saved.x, saved.y)) {
          await pet.setPosition(new PhysicalPosition(saved.x, saved.y));
        } else {
          const main = getCurrentWindow();
          const [origin, size, scale] = await Promise.all([main.innerPosition(), main.innerSize(), main.scaleFactor()]);
          await pet.setPosition(new PhysicalPosition(origin.x + size.width - (preferences.size + 40) * scale, origin.y + size.height - (preferences.size + 164) * scale));
        }
        placed.current = true;
      }
      if (!cancelled && !await pet.isVisible()) {
        const main = getCurrentWindow();
        const restoreFocus = await main.isFocused();
        await pet.show();
        if (restoreFocus) await main.setFocus();
      }
    })().catch(error => { if (!cancelled) onError(error); });
    return () => { cancelled = true; };
    // onError only reports native failures; preference changes drive synchronization.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [native, preferences.enabled, preferences.size, ready]);
  return native;
}

export function DesktopCompanion() {
  const [preferences, setPreferences] = useState(readCompanionPreferences);
  const [mood, setMood] = useState<KnightMood>(() => readSession("companion-mood", "idle"));
  const [gaze, setGaze] = useState({ x: 0, y: 0 });
  const [poked, setPoked] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    document.documentElement.classList.add("themis-desktop-companion");
    function update(event: StorageEvent) {
      if (event.key === "themis:companion") setPreferences(readCompanionPreferences());
      if (event.key === "themis:companion-mood") setMood(readSession("companion-mood", "idle"));
    }
    window.addEventListener("storage", update);
    let off: (() => void) | undefined, cancelled = false;
    void getCurrentWindow().onMoved(({ payload }) => writeSession("companion-desktop-position", payload)).then(unlisten => { if (cancelled) unlisten(); else off = unlisten; });
    return () => { cancelled = true; off?.(); window.removeEventListener("storage", update); if (timer.current) clearTimeout(timer.current); };
  }, []);
  useEffect(() => {
    if (!preferences.enabled) return;
    const pet = getCurrentWindow();
    let cancelled = false;
    async function follow() {
      try {
        const [mouse, origin, size] = await Promise.all([cursorPosition(), pet.innerPosition(), pet.innerSize()]);
        if (!cancelled) setGaze({ x: Math.max(-5, Math.min(5, (mouse.x - origin.x - size.width / 2) / 70)), y: Math.max(-3, Math.min(3, (mouse.y - origin.y - size.height / 2) / 100)) });
      } catch { /* Pointer tracking is decorative; dragging remains available. */ }
      if (!cancelled) pending = setTimeout(follow, 80);
    }
    let pending = setTimeout(follow, 80);
    return () => { cancelled = true; clearTimeout(pending); };
  }, [preferences.enabled]);
  return <div className="themis-desktop-pet" style={{ "--knight-gaze": `${gaze.x}px`, "--knight-gaze-y": `${gaze.y}px` } as CSSProperties}>
    <button type="button" className="themis-desktop-character" aria-label="Greet knight" onClick={() => { setPoked(true); if (timer.current) clearTimeout(timer.current); timer.current = setTimeout(() => setPoked(false), 1600); }}><KnightArtwork variant={preferences.variant} mood={poked && mood === "idle" ? "complete" : mood} /></button>
    <button type="button" className="themis-knight-grip" aria-label="Drag companion anywhere" title="Drag anywhere · Arrow keys to move" onPointerDown={event => { if (event.button === 0) void getCurrentWindow().startDragging(); }} onKeyDown={event => {
      const steps: Record<string, [number, number]> = { ArrowLeft: [-20, 0], ArrowRight: [20, 0], ArrowUp: [0, -20], ArrowDown: [0, 20] };
      const step = steps[event.key]; if (!step) return; event.preventDefault();
      void getCurrentWindow().outerPosition().then(position => getCurrentWindow().setPosition(new PhysicalPosition(position.x + step[0], position.y + step[1])));
    }}>⠿</button>
  </div>;
}
