// A channel's autonomy in its details (board ChannelAutonomy): the level on a field that opens the
// five levels, each with the approval mode it sets for the team's chats (Full access aside, which
// runs them in YOLO). Arrow keys move, Enter or Space chooses, Escape or a click outside closes.
import { useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import type { Autonomy } from "../../lib/visions";
import { AUTONOMY_STOPS, LEVELS } from "./autonomy";

/** The mode a level sets: Always ask keeps the chats asking (Custom), the rest run in Auto. */
export const LEVEL_MODE: Record<Autonomy, string> = { ask: "Asks before changes", small: "Auto", most: "Auto", nearly_all: "Auto", custom: "Auto" };

const CHEVRON = <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="m7 10 5 5 5-5" /></svg>;

/** Where a key moves the active option, or null for a key the list does not use. */
function moved(key: string, active: number): number | null {
  const last = AUTONOMY_STOPS.length - 1;
  if (key === "ArrowDown") return active === last ? 0 : active + 1;
  if (key === "ArrowUp") return active === 0 ? last : active - 1;
  if (key === "Home") return 0;
  if (key === "End") return last;
  return null;
}

/** `level` is the channel's, else Settings'; undefined while Settings is not read. */
export function AutonomyField({ level, onChange }: { level: Autonomy | undefined; onChange: (level: Autonomy) => void }) {
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const root = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLUListElement>(null);
  const id = useId();
  useEffect(() => {
    if (!open) return;
    list.current?.focus();
    const away = (event: MouseEvent) => { if (!root.current?.contains(event.target as Node)) setOpen(false); };
    document.addEventListener("mousedown", away);
    return () => document.removeEventListener("mousedown", away);
  }, [open]);
  const close = () => { setOpen(false); field.current?.focus(); };
  const choose = (stop: Autonomy) => { close(); if (stop !== level) onChange(stop); };
  const show = () => { setActive(Math.max(0, AUTONOMY_STOPS.indexOf(level ?? "ask"))); setOpen(true); };
  function onKeyDown(event: KeyboardEvent<HTMLUListElement>) {
    const next = moved(event.key, active);
    if (next !== null) setActive(next);
    else if (event.key === "Enter" || event.key === " ") choose(AUTONOMY_STOPS[active]!);
    else if (event.key === "Escape") close();
    else { if (event.key === "Tab") setOpen(false); return; }
    event.preventDefault();
  }
  return <div className="themis-autonomy" ref={root}>
    <button ref={field} type="button" className="themis-visions-field themis-autonomy-field" aria-label="Autonomy" aria-haspopup="listbox" aria-expanded={open} onClick={() => (open ? close() : show())}>
      <span>{level ? LEVELS[level] : ""}</span>{CHEVRON}
    </button>
    {open && <ul ref={list} className="themis-autonomy-menu" role="listbox" aria-label="Autonomy" tabIndex={-1} aria-activedescendant={`${id}-${AUTONOMY_STOPS[active]}`} onKeyDown={onKeyDown}>
      {AUTONOMY_STOPS.map((stop, index) => (
        <li key={stop} id={`${id}-${stop}`} role="option" aria-selected={stop === level} className={index === active ? "is-active" : undefined}
          onMouseEnter={() => setActive(index)} onClick={() => choose(stop)}>
          <span>{LEVELS[stop]}</span><span className="themis-autonomy-mode">{LEVEL_MODE[stop]}</span>
        </li>
      ))}
    </ul>}
  </div>;
}
