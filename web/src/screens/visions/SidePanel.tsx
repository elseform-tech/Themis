// The side panel beside a channel in Visions (canvas VisionProfile, VisionChannel): a Vision's
// profile or a channel's details, with its title, its actions and Close, and the parts both use.
import { useState, type ReactNode } from "react";
import { Button, Dialog } from "../../components";
import { describeError, useApp } from "../../state/store";
import "./VisionsPanels.css";

const icon = (children: ReactNode) => <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{children}</svg>;
export const CloseIcon = () => icon(<path d="M6 6l12 12M18 6 6 18" />);
export const TrashIcon = () => icon(<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13M10 11v6M14 11v6" />);
export const PauseIcon = () => icon(<path d="M9 5v14M15 5v14" />);
export const PlayIcon = () => icon(<path d="M7 5l12 7-12 7Z" />);
export const EditIcon = () => icon(<path d="M4 20h4L19 9l-4-4L4 16Z" />);

/** A small icon-only action in the panel's head or a block's title. */
export function PanelButton({ label, onClick, disabled, children }: { label: string; onClick: () => void; disabled?: boolean; children: ReactNode }) {
  return <button type="button" className="themis-visions-panel-icon" aria-label={label} title={label} disabled={disabled} onClick={onClick}>{children}</button>;
}

/** The panel: `title` (with what stands before it) and its actions on top, then its blocks. */
export function SidePanel({ label, title, actions, error, children }: { label: string; title: ReactNode; actions?: ReactNode; error?: string; children: ReactNode }) {
  const { dispatch } = useApp();
  return (
    <aside className="themis-visions-panel" aria-label={label}>
      <div className="themis-visions-panel-head">
        {title}
        {actions}
        <PanelButton label="Close" onClick={() => dispatch({ type: "visions/panel", panel: null })}><CloseIcon /></PanelButton>
      </div>
      {error && <p role="alert" className="themis-form-error">{error}</p>}
      {children}
    </aside>
  );
}

/** One titled block of the panel. */
export function PanelBlock({ title, action, children }: { title?: string; action?: ReactNode; children: ReactNode }) {
  return <div className="themis-visions-block">
    {title && <h3 className="themis-visions-label">{title}{action}</h3>}
    {children}
  </div>;
}

/** Delete with a confirmation that says what happens to the work; `onDelete` throws its error. */
export function DeleteButton({ name, consequence, onDelete }: { name: string; consequence: string; onDelete: () => Promise<void> }) {
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function confirm() {
    setBusy(true); setError("");
    try { await onDelete(); setAsking(false); } catch (reason) { setError(describeError(reason)); } finally { setBusy(false); }
  }
  return <>
    <PanelButton label={`Delete ${name}`} onClick={() => setAsking(true)}><TrashIcon /></PanelButton>
    <Dialog open={asking} title={`Delete ${name}?`} closeOnOutsideClick={false} onClose={() => { if (!busy) setAsking(false); }}>
      <p className="themis-settings-hint">{consequence}</p>
      {error && <p role="alert" className="themis-form-error">{error}</p>}
      <div className="themis-sidebar-dialog-actions"><Button variant="ghost" disabled={busy} onClick={() => setAsking(false)}>Cancel</Button><Button variant="danger" disabled={busy} onClick={() => void confirm()}>Delete</Button></div>
    </Dialog>
  </>;
}
