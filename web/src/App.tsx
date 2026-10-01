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

function Shell() {
  const { state, dispatch } = useApp();
  const [collapsed, setCollapsed] = useState(() => readSession("sidebar-collapsed", false));
  function toggleSidebar() { setCollapsed(value => { writeSession("sidebar-collapsed", !value); return !value; }); }
  useEffect(() => { document.documentElement.style.fontSize = `${16 * state.settings.text_size / 14}px`; }, [state.settings.text_size]);
  useEffect(() => {
    document.documentElement.dataset.palette = state.settings.theme_palette ?? "system";
    document.documentElement.dataset.font = state.settings.font_family ?? "system";
  }, [state.settings.theme_palette, state.settings.font_family]);
  const headApproval = state.approvals[0];

  // Theme: settings value, resolving `system` via matchMedia.
  useEffect(() => {
    const mode = state.settings.theme;
    const query = window.matchMedia("(prefers-color-scheme: light)");
    const apply = () => {
      const resolved =
        mode === "system" ? (query.matches ? "light" : "dark") : mode;
      document.documentElement.dataset.theme = resolved;
    };
    apply();
    query.addEventListener("change", apply);
    return () => {
      query.removeEventListener("change", apply);
    };
  }, [state.settings.theme]);

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

  return (
    <div className={`themis-shell ${isTauri() && /Mac/.test(navigator.platform) ? "themis-shell--mac" : ""}`}>
        <header className="themis-nav" data-tauri-drag-region>
          <div className="themis-nav-context">
            <Button id="sidebar-toggle" variant="ghost" size="small" aria-label={collapsed ? "Show sidebar" : "Collapse sidebar"} aria-controls="themis-sidebar" aria-expanded={!collapsed} onClick={toggleSidebar}><SidebarIcon name="panel" /></Button>
            <span>{state.mainView === "thread" ? (state.projects.find(p => p.root === state.activeProjectRoot)?.name ?? "Workspace") : ({ skills: "Skills", automations: "Automations", settings: "Settings", queue: "Review queue" }[state.mainView])}</span>
          </div>
          {state.reviews.some(r => r.status === "pending") && <Button variant="ghost" size="small" onClick={() => dispatch({ type: "ui/view", view: "queue" })}>Review queue · {state.reviews.filter(r => r.status === "pending").length}</Button>}
        </header>
        <div className="themis-shell-body">
          <Sidebar collapsed={collapsed} onToggle={toggleSidebar} />
          <main className="themis-main">
        <div className="themis-view">
          {state.mainView === "thread" && <ThreadView />}
          {state.mainView === "skills" && <Skills />}
          {state.mainView === "automations" && <Automations />}
          {state.mainView === "settings" && <SettingsScreen />}
          {state.mainView === "queue" && <ReviewQueue />}
        </div>
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
