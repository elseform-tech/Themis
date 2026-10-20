import { useCallback, useEffect, useRef, useState } from "react";
import { useDismiss } from "../lib/dismiss";
import { resourceUsage, type ResourceGroup, type ResourceUsage } from "../lib/tauri";
import "./ResourceMeter.css";

const POLL_MS = 5000;
const GROUP_LABEL: Record<ResourceGroup["group"], string> = { app: "App", browser: "Built-in browser", tools: "Tools and dev servers" };

export function formatBytes(bytes: number): string {
  const mb = bytes / (1024 * 1024);
  return mb >= 1024 ? `${(mb / 1024).toFixed(1)} GB` : `${Math.round(mb)} MB`;
}

/** Share of the whole machine, from percent-of-one-core summed over processes. */
export function machineShare(usage: ResourceUsage): number {
  return Math.round(usage.cpu_percent / Math.max(1, usage.cores));
}

/** Frames the window drew in the last second; 0 while it is hidden. */
function useFrameRate(): number {
  const [fps, setFps] = useState(0);
  useEffect(() => {
    let frames = 0;
    let handle = 0;
    const tick = () => { frames += 1; handle = requestAnimationFrame(tick); };
    handle = requestAnimationFrame(tick);
    const timer = window.setInterval(() => { setFps(frames); frames = 0; }, 1000);
    return () => { cancelAnimationFrame(handle); window.clearInterval(timer); };
  }, []);
  return fps;
}

/** Themis's resource use at the top right of the window: CPU, memory and the UI's frame rate; click for the breakdown. */
export function ResourceMeter() {
  const [usage, setUsage] = useState<ResourceUsage | null>(null);
  const [open, setOpen] = useState(false);
  const fps = useFrameRate();
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    let live = true;
    const poll = () => { if (document.visibilityState === "visible") void resourceUsage().then(next => { if (live && next) setUsage(next); }).catch(() => undefined); };
    poll();
    const timer = window.setInterval(poll, POLL_MS);
    return () => { live = false; window.clearInterval(timer); };
  }, []);
  const close = useCallback(() => setOpen(false), []);
  useDismiss(root, open, close);
  if (usage === null) return null;
  const share = machineShare(usage);
  const growing = usage.groups.filter(group => group.growing_bytes);
  const slow = fps > 0 && fps < 30;
  const summary = `Themis resource use: CPU ${share}%, memory ${formatBytes(usage.memory_bytes)}${fps ? `, ${fps} frames a second` : ""}${growing.length ? ", memory keeps growing" : ""}`;
  return (
    <div ref={root} className="themis-resources">
      <button type="button" className="themis-resources-button" aria-label={summary} aria-expanded={open} title="Themis resource use" onClick={() => setOpen(previous => !previous)}>
        <span className={`themis-resources-item${share >= 50 ? " is-warning" : ""}`}><CpuIcon />{share}%</span>
        <span className={`themis-resources-item${growing.length ? " is-warning" : ""}`}><MemoryIcon />{formatBytes(usage.memory_bytes)}</span>
        {fps > 0 && <span className={`themis-resources-item${slow ? " is-warning" : ""}`}><GaugeIcon />{fps} fps</span>}
      </button>
      {open && (
        <div className="themis-resources-popover" role="dialog" aria-label="Resource use">
          <table>
            <thead><tr><th scope="col">Part</th><th scope="col">CPU</th><th scope="col">Memory</th></tr></thead>
            <tbody>
              {usage.groups.filter(group => group.processes > 0).map(group => (
                <tr key={group.group}>
                  <th scope="row">{GROUP_LABEL[group.group]}</th>
                  <td>{Math.round(group.cpu_percent / Math.max(1, usage.cores))}%</td>
                  <td>{formatBytes(group.memory_bytes)}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {growing.map(group => <p key={group.group} className="themis-resources-note">{GROUP_LABEL[group.group]} memory grew {formatBytes(group.growing_bytes ?? 0)} over the last 30 minutes and has not come back down.</p>)}
          <p className="themis-resources-hint">CPU is the share of all {usage.cores} cores. The window itself is drawn by macOS and is not included.</p>
        </div>
      )}
    </div>
  );
}

function CpuIcon() {
  return <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" aria-hidden="true"><rect x="6" y="6" width="12" height="12" rx="2" /><path d="M9 2v4M15 2v4M9 18v4M15 18v4M2 9h4M2 15h4M18 9h4M18 15h4" /></svg>;
}

function MemoryIcon() {
  return <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" aria-hidden="true"><rect x="3" y="7" width="18" height="10" rx="1.5" /><path d="M7 11v2M11 11v2M15 11v2M6 17v3M10 17v3M14 17v3M18 17v3" /></svg>;
}

function GaugeIcon() {
  return <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" aria-hidden="true"><path d="M4 18a8 8 0 1 1 16 0" /><path d="m12 18 4-6" /></svg>;
}
