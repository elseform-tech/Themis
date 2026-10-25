import { useEffect, useRef, useState } from "react";
import { usePopoverDismissal } from "./lib/dismiss";
import { Button, Toasts } from "./components";
import {
  AppProvider,
  describeError,
  toast,
  useApp,
} from "./state/store";
import { readSession, writeSession } from "./state/session";
import {
  Automations,
  Palette,
  ReviewQueue,
  SettingsScreen,
  Sidebar,
  Skills,
  ThreadView,
} from "./screens";
import { SidebarIcon } from "./screens/Sidebar";
import { isTauri } from "@tauri-apps/api/core";
import { ResourceMeter } from "./screens/ResourceMeter";
import "./screens/shell.css";
import { applyAppearance } from "./theme/appearance";
import { KnightCompanion, KnightControl, KnightLoading, readCompanionPreferences, type CompanionPreferences, type KnightMood } from "./components/KnightCompanion";
import { useDesktopCompanion } from "./components/DesktopCompanion";

function Shell() {
  const { state, startup, dispatch } = useApp();
  usePopoverDismissal();
  const [companion, setCompanion] = useState(readCompanionPreferences);
  function updateCompanion(next: CompanionPreferences) { setCompanion(next); writeSession("companion", next); }
  const [collapsed, setCollapsed] = useState(() => readSession("sidebar-collapsed", false));
  function toggleSidebar() { autoFolded.current = false; setCollapsed(value => { writeSession("sidebar-collapsed", !value); return !value; }); }
  // Opening the workspace on a narrower window folds the sidebar to its rail so the
  // conversation keeps room; closing the workspace restores it.
  const autoFolded = useRef(false);
  useEffect(() => {
    if (state.diffPanelOpen && !collapsed && window.innerWidth < 1500) { autoFolded.current = true; setCollapsed(true); }
    else if (!state.diffPanelOpen && autoFolded.current) { autoFolded.current = false; setCollapsed(readSession("sidebar-collapsed", false)); }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- react only to the panel opening or closing
  }, [state.diffPanelOpen]);
  useEffect(() => { applyAppearance(state.settings); }, [state.settings]);

  // Global keys: Cmd/Ctrl+K toggles the palette, Esc closes the topmost layer.
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (
        (event.metaKey || event.ctrlKey) &&
        event.key.toLowerCase() === "k"
      ) {
        event.preventDefault();
        dispatch({ type: "ui/palette", open: !state.paletteOpen });
      } else if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "b") {
        event.preventDefault(); toggleSidebar();
      } else if (event.key === "Escape") {
        if (state.paletteOpen) {
          dispatch({ type: "ui/palette", open: false });
        } else if (state.projectDialogOpen) {
          dispatch({ type: "ui/project-dialog", open: false });
        } else if (state.diffPanelOpen) {
          dispatch({ type: "ui/diff-panel", open: false });
        }
      }
    }
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [dispatch, state.paletteOpen, state.projectDialogOpen, state.diffPanelOpen]);

  const threadId = state.activeThreadId;
  const messages = threadId ? state.messages[threadId] ?? [] : [];
  const last = messages[messages.length - 1];
  const selectedThread = state.activeProjectRoot ? state.threadsByProject[state.activeProjectRoot]?.find(thread => thread.id === threadId) : undefined;
  const working = threadId ? state.running[threadId] ?? selectedThread?.running : false;
  const mood: KnightMood = (threadId && (state.sendErrors[threadId] || state.approvals.some(approval => approval.thread_id === threadId))) || (last?.role === "system" && /^(Run failed:|Send failed:|Send rejected:)/.test(last.text)) ? "attention"
    : working ? "working" : last?.role === "assistant" && !last.incomplete ? "complete" : "idle";
  const desktopCompanion = useDesktopCompanion(companion, mood, startup === null, error => toast(dispatch, `Companion unavailable: ${describeError(error)}`, "warning"));
  if (startup !== null) return <KnightLoading variant={companion.variant} status={startup} />;
  return (
    <div className={`themis-shell ${isTauri() && /Mac/.test(navigator.platform) ? "themis-shell--mac" : ""}`}>
        <header className="themis-nav" data-tauri-drag-region>
          <div className="themis-nav-context">
            <Button id="sidebar-toggle" variant="ghost" size="small" aria-label={collapsed ? "Show sidebar" : "Collapse sidebar"} aria-controls="themis-sidebar" aria-expanded={!collapsed} onClick={toggleSidebar}><SidebarIcon name="panel" /></Button>
            <span>{state.mainView === "thread" ? (state.projects.find(p => p.root === state.activeProjectRoot)?.name ?? "Workspace") : ({ skills: "Integrations", automations: "Automations", settings: "Settings", queue: "Review queue" }[state.mainView])}</span>
          </div>
          {isTauri() && <ResourceMeter />}
        </header>
        <div className="themis-shell-body">
          <Sidebar collapsed={collapsed} companionControl={<KnightControl preferences={companion} onChange={updateCompanion} />} />
          <main className="themis-main">
        <div className="themis-view">
          {state.mainView === "thread" && <ThreadView />}
          {state.mainView === "skills" && <Skills />}
          {state.mainView === "automations" && <Automations />}
          {state.mainView === "settings" && <SettingsScreen />}
          {state.mainView === "queue" && <ReviewQueue />}
        </div>
        {companion.enabled && !desktopCompanion && <KnightCompanion preferences={companion} onChange={updateCompanion} mood={mood} />}
      </main>
      </div>
      <Palette />
      <Toasts
        toasts={state.toasts}
        onDismiss={(id) => dispatch({ type: "toast/dismiss", id })}
      />
    </div>
  );
}

export default function App() {
  return (
    <AppProvider>
      <Shell />
    </AppProvider>
  );
}
