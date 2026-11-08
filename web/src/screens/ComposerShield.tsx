import { useRef, type KeyboardEvent, type ReactNode } from "react";

export type ApprovalMode = "custom" | "auto" | "yolo";

const SHIELD = "M12 3 4 6.5V12c0 4.5 8 9 8 9s8-4.5 8-9V6.5Z";

/** Auto: shield with a check; Custom: plain shield; YOLO: lightning bolt. */
function ModeIcon({ mode }: { mode: ApprovalMode }) {
  return (
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      {mode === "yolo" ? <path d="M13 2 4 14h7l-1 8 9-12h-7Z" /> : <path d={SHIELD} />}
      {mode === "auto" && <path d="m9 12 2 2 4-4" />}
    </svg>
  );
}

const MODES: { mode: ApprovalMode; label: string; summary: string }[] = [
  { mode: "auto", label: "Auto", summary: "A reviewer checks risky steps; asks you before destructive ones." },
  { mode: "custom", label: "Custom", summary: "Asks you before each change." },
  { mode: "yolo", label: "YOLO", summary: "No prompts, no sandbox; asks you before destructive steps." },
];

const entry = (mode: ApprovalMode) => MODES.find(item => item.mode === mode) ?? MODES[1];

const STEP: Record<string, number> = { ArrowDown: 1, ArrowRight: 1, ArrowUp: -1, ArrowLeft: -1 };

interface ComposerShieldProps {
  mode?: ApprovalMode;
  saving: boolean;
  /** A run is going: a change applies to the next one. */
  running: boolean;
  /** Blocks the configuration wrote that YOLO keeps (network, Keychain, deny rules). */
  yoloKeeps?: string[];
  onChange?: (mode: ApprovalMode) => void;
}

/**
 * The composer's permission shield: the chat's mode (icon, name, colour) and a list of the three modes.
 * Arrows, Space, Enter and click choose; typed letters never change the mode, and a mouse press on the
 * shield does not move focus to it.
 */
export function ComposerShield({ mode = "custom", saving, running, yoloKeeps, onChange }: ComposerShieldProps) {
  const current = entry(mode);
  const items = useRef<(HTMLButtonElement | null)[]>([]);
  const choose = (next: ApprovalMode) => { if (!saving && next !== mode) onChange?.(next); };
  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    const step = STEP[event.key];
    if (step === undefined) return;
    event.preventDefault();
    const index = (MODES.findIndex(item => item.mode === mode) + step + MODES.length) % MODES.length;
    items.current[index]?.focus();
    choose(MODES[index].mode);
  }
  return (
    <details className="themis-composer-permissions" data-popover data-mode={mode}>
      <summary onMouseDown={event => event.preventDefault()} aria-label={`Permissions: ${current.label}`} title={current.summary}>
        <ModeIcon mode={mode} />
        <span className="themis-composer-label">{saving ? "Saving…" : current.label}</span>
      </summary>
      <div className="themis-permissions-panel">
        <div role="radiogroup" aria-label="Approval mode" aria-disabled={saving || undefined} onKeyDown={onKeyDown}>
          {MODES.map((item, index) => {
            const checked = item.mode === mode;
            let extra: ReactNode = null;
            if (item.mode === "yolo" && yoloKeeps?.length) extra = <small>Still applies: {yoloKeeps.join(", ")}</small>;
            return (
              <button key={item.mode} ref={node => { items.current[index] = node; }} type="button" role="radio" aria-checked={checked}
                tabIndex={checked ? 0 : -1} data-mode={item.mode} className="themis-permissions-mode" onClick={() => choose(item.mode)}>
                <ModeIcon mode={item.mode} />
                <span><strong>{item.label}</strong><small>{item.summary}</small>{extra}</span>
              </button>
            );
          })}
        </div>
        {running && <p>Changes apply to the next run.</p>}
      </div>
    </details>
  );
}
