import { Tooltip } from "../components";
import { useApp } from "../state/store";
import { SectionIcon } from "./SettingsGroup";

export const SETTINGS_SECTIONS = ["General", "Appearance", "Models", "Usage", "Secrets", "Permissions", "Browser", "Web", "Advanced"];

/** The section shown on the right; the sidebar's list and the pane share it through the store. */
export function useSettingsSection() {
  const { state } = useApp();
  return state.settingsFocus?.section ?? "General";
}

/**
 * While Settings is open its section list takes over the whole sidebar: one column,
 * Back to app first. Folded, it shows icons only, like the rail it replaces.
 */
export function SettingsNav({ id }: { id?: string }) {
  const { dispatch } = useApp();
  const section = useSettingsSection();
  const row = (label: string, icon: import("react").ReactNode, onClick: () => void, current = false, className = "") =>
    <Tooltip key={label} content={label}>
      <button type="button" className={`themis-sidebar-row ${current ? "themis-sidebar-row--active" : ""} ${className}`} aria-label={label} aria-current={current ? "page" : undefined} onClick={onClick}>
        {icon}<span className="themis-sidebar-name">{label}</span>
      </button>
    </Tooltip>;
  return <nav id={id} className="themis-settings-nav" aria-label="Settings sections">
    {row("Back to app", <BackIcon />, () => dispatch({ type: "ui/view", view: "thread" }), false, "themis-settings-back")}
    {SETTINGS_SECTIONS.map(label => row(label, <SectionIcon section={label} />, () => dispatch({ type: "ui/settings-section", section: label }), section === label))}
  </nav>;
}

function BackIcon() {
  return <svg className="themis-sidebar-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false"><path d="M19 12H5M12 19l-7-7 7-7" /></svg>;
}
