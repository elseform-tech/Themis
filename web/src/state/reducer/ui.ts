import type { AppAction, AppState } from "../reducer";

export function reduceUiActions(
  state: AppState,
  action: AppAction,
): AppState | null {
  switch (action.type) {
    case "toast/push":
      return { ...state, toasts: [...state.toasts, action.toast] };
    case "toast/dismiss":
      return {
        ...state,
        toasts: state.toasts.filter((toast) => toast.id !== action.id),
      };
    case "ui/view":
      return {
        ...state,
        mainView: action.view,
        settingsFocus: action.settings ?? null,
        diffPanelOpen: action.view === "thread" && state.diffPanelOpen,
      };
    case "ui/settings-section":
      return { ...state, settingsFocus: { section: action.section } };
    case "ui/diff-panel":
      return {
        ...state,
        diffPanelOpen: action.open,
        ...(action.focus ? { diffFocus: { path: action.focus, at: Date.now() } } : {}),
        ...(action.open && action.tab ? { workspaceTabRequest: { tab: action.tab, at: Date.now() } } : {}),
        ...(action.subagent ? { subagentFocus: action.subagent } : {}),
      };
    case "ui/subagent-focus":
      return { ...state, subagentFocus: action.id };
    case "ui/tab-request-consumed":
      return state.workspaceTabRequest ? { ...state, workspaceTabRequest: null } : state;
    case "tasks/loaded":
      return { ...state, tasks: { ...state.tasks, [action.threadId]: action.tasks } };
    case "ui/palette":
      return { ...state, paletteOpen: action.open };
    case "ui/project-dialog":
      return { ...state, projectDialogOpen: action.open };
    default:
      return null;
  }
}
