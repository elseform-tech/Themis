import { useEffect, useId, useState } from "react";
import "./StartupScreen.css";
export { startupProgress } from "../state/boot";

/** How long the startup screen stays: at least `minimumMs` from launch, and `settleMs` after the last step so the
 *  bar is seen full. Tests set both to 0 (components/test-setup.ts). */
export const loaderTiming = { minimumMs: 900, settleMs: 600 };


/** True once startup has ended and the screen has had its minimum time and the bar its settle. Reduced motion skips
 *  the settle (the bar jumps), not the minimum. */
export function useStartupReveal(step: string | null): boolean {
  const [begun] = useState(() => Date.now());
  const [waited, setWaited] = useState(false);
  const instant = loaderTiming.minimumMs <= 0 && loaderTiming.settleMs <= 0;
  useEffect(() => {
    if (step !== null || instant) return;
    const still = typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    const wait = Math.max(loaderTiming.minimumMs - (Date.now() - begun), still ? 0 : loaderTiming.settleMs);
    const timer = window.setTimeout(() => setWaited(true), wait);
    return () => window.clearTimeout(timer);
  }, [step, instant, begun]);
  return step === null && (instant || waited);
}

/** The canvas Loading board: the T/V mark, the wordmark and a thin bar on the sky's frame. The step is read out to
 *  screen readers only. */
export function StartupScreen({ label, progress }: { label: string; progress: number }) {
  const gradient = `themis-mark-${useId().replace(/[^a-z0-9]/gi, "")}`;
  const value = Math.max(0, Math.min(100, progress));
  return (
    <div className="themis-startup" aria-busy="true">
      <svg width="72" height="72" viewBox="28 32 144 144" role="img" aria-label="Themis Visions">
        <defs>
          <linearGradient id={gradient} x1="0" y1="0" x2="1" y2="1">
            <stop offset="0" style={{ stopColor: "var(--themis-mark-from)" }} />
            <stop offset="1" style={{ stopColor: "var(--themis-mark-to)" }} />
          </linearGradient>
        </defs>
        <path d="M32 44H168L154 68H112V124L100 144L88 124V68H46Z" style={{ fill: `url(#${gradient}) var(--themis-mark-from)` }} />
        <path d="M39 58L63 58L100 122L137 58L161 58L100 164Z" style={{ fill: "var(--themis-mark-v)", fillOpacity: .9 }} />
      </svg>
      <div className="themis-startup-word" aria-hidden="true"><span>Themis</span><span>Visions</span></div>
      <div className="themis-startup-bar" role="progressbar" aria-label="Starting" aria-valuemin={0} aria-valuemax={100} aria-valuenow={value}>
        <div className="themis-startup-fill" style={{ width: `${value}%` }} />
      </div>
      <p className="themis-sr-only" role="status">{label}</p>
    </div>
  );
}
