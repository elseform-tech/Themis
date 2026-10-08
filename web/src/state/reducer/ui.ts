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
        diffPanelOpen: action.view === "thread" && state.diffPanelOpen,
      };
    case "ui/diff-panel":
      return { ...state, diffPanelOpen: action.open };
    case "ui/palette":
      return { ...state, paletteOpen: action.open };
    case "ui/project-dialog":
      return { ...state, projectDialogOpen: action.open };
    default:
      return null;
  }
}
