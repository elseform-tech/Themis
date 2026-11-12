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
  { mode: "auto", label: "Auto", summary: "Actions are reviewed automatically; destructive actions require approval." },
  { mode: "custom", label: "Custom", summary: "Changes and network access require approval." },
  { mode: "yolo", label: "YOLO", summary: "Unrestricted, including network; destructive actions require approval." },
];

const entry = (mode: ApprovalMode) => MODES.find(item => item.mode === mode) ?? MODES[1];

const STEP: Record<string, number> = { ArrowDown: 1, ArrowRight: 1, ArrowUp: -1, ArrowLeft: -1 };

interface ComposerShieldProps {
  mode?: ApprovalMode;
  saving: boolean;
  /** Blocks the configuration wrote that YOLO keeps (network, Keychain, deny rules). */
  yoloKeeps?: string[];
  /** The configuration blocks the network, in every mode. */
  networkBlocked?: boolean;
  onChange?: (mode: ApprovalMode) => void;
}

/** Closes the shield's list; focus in it goes back to the shield, not to the page (the text box keeps its own). */
function closeList(menu: HTMLDetailsElement | null) {
  if (!menu?.open) return;
  const focusInside = menu.contains(document.activeElement);
  menu.open = false;
  if (focusInside) menu.querySelector("summary")?.focus();
}

/**
 * The composer's permission shield: the chat's mode (icon, name, colour) and a list of the three modes.
 * Arrows, Space, Enter and click choose; typed letters never change the mode, and a mouse press on the
 * shield does not move focus to it. A pick (not an arrow step) and Esc close the list, so a later click
 * meant for the text box cannot land on it; the app closes it on a press outside.
 */
export function ComposerShield({ mode = "custom", saving, yoloKeeps, networkBlocked = false, onChange }: ComposerShieldProps) {
  const current = entry(mode);
  const menu = useRef<HTMLDetailsElement>(null);
  const items = useRef<(HTMLButtonElement | null)[]>([]);
  const choose = (next: ApprovalMode) => { if (!saving && next !== mode) onChange?.(next); };
  const pick = (next: ApprovalMode) => { closeList(menu.current); choose(next); };
  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    const step = STEP[event.key];
    if (step === undefined) return;
    event.preventDefault();
    const index = (MODES.findIndex(item => item.mode === mode) + step + MODES.length) % MODES.length;
    items.current[index]?.focus();
    choose(MODES[index].mode);
  }
  return (
    <details ref={menu} className="themis-composer-permissions" data-popover data-mode={mode} onKeyDown={event => { if (event.key === "Escape") closeList(menu.current); }}>
      <summary onMouseDown={event => event.preventDefault()} aria-label={`Permissions: ${current.label}`} title={current.summary}>
        <ModeIcon mode={mode} />
        <span className="themis-composer-label">{saving ? "Saving…" : current.label}</span>
      </summary>
      <div className="themis-permissions-panel">
        <div role="radiogroup" aria-label="Approval mode" aria-disabled={saving || undefined} onKeyDown={onKeyDown}>
          {MODES.map((item, index) => {
            const checked = item.mode === mode;
            let extra: ReactNode = null;
            if (item.mode === "yolo" && yoloKeeps?.length) extra = <small className="themis-permissions-keeps" title={`Still applies: ${yoloKeeps.join(", ")}`}>Still applies: {yoloKeeps.join(", ")}</small>;
            return (
              <button key={item.mode} ref={node => { items.current[index] = node; }} type="button" role="radio" aria-checked={checked}
                tabIndex={checked ? 0 : -1} data-mode={item.mode} className="themis-permissions-mode" onClick={() => pick(item.mode)}>
                <ModeIcon mode={item.mode} />
                <span><strong>{item.label}</strong><small className="themis-permissions-hint">{item.summary}</small>{extra}</span>
              </button>
            );
          })}
        </div>
        {networkBlocked && <p>Network blocked by your configuration</p>}
      </div>
    </details>
  );
}
