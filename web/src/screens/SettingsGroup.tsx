import type { ReactNode } from "react";

/** One titled container of settings rows; `hint` shows on hovering the title. */
export function SettingsGroup({ title, hint, action, children }: { title?: string; hint?: string; action?: ReactNode; children: ReactNode }) {
  return <section className="themis-settings-group" aria-label={title}>
    {(title || action) && <div className="themis-settings-group-head">
      {title && <h3 className="themis-settings-subtitle" title={hint}>{title}</h3>}
      {action}
    </div>}
    <div className="themis-settings-card">{children}</div>
  </section>;
}

/** A small icon-only action with its name as tooltip and accessible label. */
export function SettingsIconButton({ label, onClick, disabled, children }: { label: string; onClick: () => void; disabled?: boolean; children: ReactNode }) {
  return <button type="button" className="themis-settings-icon-button" aria-label={label} title={label} disabled={disabled} onClick={onClick}>{children}</button>;
}

const svg = (children: ReactNode) => <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{children}</svg>;

/** The icon beside each Settings section name. */
export function SectionIcon({ section }: { section: string }) {
  switch (section) {
    case "General": return svg(<><circle cx="12" cy="12" r="3" /><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9 7 7M17 17l2.1 2.1M4.9 19.1 7 17M17 7l2.1-2.1" /></>);
    case "Appearance": return svg(<><circle cx="12" cy="12" r="9" /><path d="M12 3a9 9 0 0 1 0 18Z" fill="currentColor" /></>);
    case "Models": return svg(<path d="m12 3 1.9 5.1L19 10l-5.1 1.9L12 17l-1.9-5.1L5 10l5.1-1.9L12 3Zm6 12 .8 2.2L21 18l-2.2.8L18 21l-.8-2.2L15 18l2.2-.8L18 15Z" />);
    case "Usage": return svg(<path d="M4 20h16M7 16v-4M12 16V6M17 16v-7" />);
    case "Secrets": return svg(<><circle cx="8" cy="15" r="4" /><path d="m11 12 9-9M17 6l3 3M14 9l2 2" /></>);
    case "Permissions": return svg(<path d="M12 3 5 6v5c0 4.5 3 8.2 7 10 4-1.8 7-5.5 7-10V6l-7-3Z" />);
    case "Web": return svg(<><circle cx="10.5" cy="10.5" r="6.5" /><path d="m20 20-4.8-4.8" /></>);
    case "Subagents": return svg(<><circle cx="8" cy="9" r="3" /><circle cx="16" cy="9" r="3" /><path d="M3 19c.8-3 2.7-4.5 5-4.5s4.2 1.5 5 4.5M11 19c.8-3 2.7-4.5 5-4.5s4.2 1.5 5 4.5" /></>);
    case "Browser": return svg(<><circle cx="12" cy="12" r="9" /><path d="M3 12h18M12 3c2.5 2.6 3.8 5.6 3.8 9s-1.3 6.4-3.8 9c-2.5-2.6-3.8-5.6-3.8-9S9.5 5.6 12 3Z" /></>);
    default: return svg(<path d="M4 6h10M18 6h2M4 12h4M12 12h8M4 18h12M20 18h0M14 4v4M8 10v4M16 16v4" />);
  }
}

export const RefreshIcon = () => svg(<path d="M20 11a8 8 0 1 0-2.3 5.7M20 4v7h-7" />);
export const ResetIcon = () => svg(<path d="M4 12a8 8 0 1 0 2.3-5.7M4 4v5h5" />);
