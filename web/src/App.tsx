import { useEffect, useState } from "react";
import { ApprovalDialog, Button, Toasts } from "./components";
import type { ApprovalDecision } from "./lib/types";
import { approveAction } from "./lib/tauri";
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
import "./screens/shell.css";
import { applyAppearance } from "./theme/appearance";
import { KnightCompanion, KnightControl, KnightLoading, readCompanionPreferences, type CompanionPreferences, type KnightMood } from "./components/KnightCompanion";

function Shell() {
  const { state, startup, dispatch } = useApp();
  const [companion, setCompanion] = useState(readCompanionPreferences);
  function updateCompanion(next: CompanionPreferences) { setCompanion(next); writeSession("companion", next); }
  const [collapsed, setCollapsed] = useState(() => readSession("sidebar-collapsed", false));
  function toggleSidebar() { setCollapsed(value => { writeSession("sidebar-collapsed", !value); return !value; }); }
  useEffect(() => { applyAppearance(state.settings); }, [state.settings]);
  const headApproval = state.approvals[0];

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

  async function decideApproval(decision: ApprovalDecision) {
    if (headApproval === undefined) return;
    const { thread_id, approval_id } = headApproval;
    try {
      await approveAction(thread_id, approval_id, decision);
      dispatch({ type: "approval/dequeued", approvalId: approval_id });
    } catch (error: unknown) {
      toast(dispatch, `Approval failed: ${describeError(error)}`, "danger");
    }

  }

  if (startup !== null) return <KnightLoading variant={companion.variant} status={startup} />;
  const threadId = state.activeThreadId;
  const messages = threadId ? state.messages[threadId] ?? [] : [];
  const last = messages[messages.length - 1];
  const selectedThread = state.activeProjectRoot ? state.threadsByProject[state.activeProjectRoot]?.find(thread => thread.id === threadId) : undefined;
  const working = threadId ? state.running[threadId] ?? selectedThread?.running : false;
  const mood: KnightMood = (threadId && (state.sendErrors[threadId] || state.approvals.some(approval => approval.thread_id === threadId))) || (last?.role === "system" && /^(Run failed:|Send failed:|Send rejected:)/.test(last.text)) ? "attention"
    : working ? "working" : last?.role === "assistant" && !last.incomplete ? "complete" : "idle";
  return (
    <div className={`themis-shell ${isTauri() && /Mac/.test(navigator.platform) ? "themis-shell--mac" : ""}`}>
        <header className="themis-nav" data-tauri-drag-region>
          <div className="themis-nav-context">
            <Button id="sidebar-toggle" variant="ghost" size="small" aria-label={collapsed ? "Show sidebar" : "Collapse sidebar"} aria-controls="themis-sidebar" aria-expanded={!collapsed} onClick={toggleSidebar}><SidebarIcon name="panel" /></Button>
            <span>{state.mainView === "thread" ? (state.projects.find(p => p.root === state.activeProjectRoot)?.name ?? "Workspace") : ({ skills: "Plugins", automations: "Automations", settings: "Settings", queue: "Review queue" }[state.mainView])}</span>
          </div>
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
        {companion.enabled && <KnightCompanion preferences={companion} onChange={updateCompanion} mood={mood} />}
      </main>
      </div>
      {headApproval !== undefined && (
        <ApprovalDialog
          request={headApproval}
          open
          onDecide={(decision) => void decideApproval(decision)}
        />
      )}
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
