import { useEffect, useState, type ReactNode } from "react";
import { Badge, Tooltip } from "../components";
import { pluginAction } from "../lib/tauri";
import { unseenOff } from "../lib/integrations";
import { useApp, type MainView } from "../state/store";
import type { Plugin } from "../lib/types";
import { SidebarIcon, type SidebarIconName } from "./Sidebar";

const views: Array<[MainView, string]> = [["skills", "Integrations"], ["automations", "Automations"], ["queue", "Review queue"]];
const viewIcons: Record<MainView, SidebarIconName> = { thread: "chat", skills: "skills", automations: "automations", queue: "queue", settings: "settings" };

/** The utility rail: Chats, the optional `extra` slot, the main views, Settings, Help and Search. */
export function Rail({ onHelp, extra }: { onHelp: () => void; extra?: ReactNode }) {
  const { state, dispatch } = useApp();
  const activeRoot = state.activeProjectRoot;
  const pending = state.reviews.filter(r => r.status === "pending").length;
  // Installed parts still off: the mark stays until they've been seen in the Installed list.
  const [partsOff, setPartsOff] = useState(false);
  useEffect(() => {
    let live = true;
    const check = () => void pluginAction<Plugin[]>({ action: "list", projectRoot: activeRoot }).then(list => { if (live && Array.isArray(list)) setPartsOff(unseenOff(list)); }).catch(() => undefined);
    check();
    window.addEventListener("themis-plugins-changed", check);
    window.addEventListener("themis-off-seen", check);
    return () => { live = false; window.removeEventListener("themis-plugins-changed", check); window.removeEventListener("themis-off-seen", check); };
  }, [activeRoot]);
  function nav(view: MainView, label: string) {
    return <Tooltip key={view} content={label}><button type="button" className={`themis-sidebar-row ${state.mainView === view ? "themis-sidebar-row--active" : ""}`} aria-label={view === "queue" && pending > 0 ? `${label} ${pending}` : label} aria-current={state.mainView === view ? "page" : undefined} onClick={() => dispatch({ type: "ui/view", view })}>
      <SidebarIcon name={viewIcons[view]} /><span className="themis-sidebar-name">{label}</span>{view === "queue" && pending > 0 && <Badge tone="warning">{pending}</Badge>}{view === "skills" && partsOff && <i className="themis-off-dot themis-sidebar-off" aria-hidden="true" data-testid="parts-off" />}
    </button></Tooltip>;
  }
  return <nav className="themis-sidebar-rail" aria-label="Utilities">
    {nav("thread", "Chats")}
    {extra}
    {views.map(([view, label]) => nav(view, label))}
    <div className="themis-rail-footer">{nav("settings", "Settings")}<Tooltip content="Help"><button type="button" className="themis-sidebar-row" aria-label="Help" onClick={onHelp}><SidebarIcon name="help" /></button></Tooltip><Tooltip content="Search"><button type="button" className="themis-sidebar-row" aria-label="Search commands" onClick={() => dispatch({ type: "ui/palette", open: true })}><SidebarIcon name="search" /></button></Tooltip></div>
  </nav>;
}
