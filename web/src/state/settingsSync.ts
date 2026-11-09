// Follows `settings-changed`: settings written through the shared server (the `themis` CLI,
// another client or this window) reach this window without a reload. Pure apart from the
// injected reads, so tests drive it without the Tauri bridge.
import type { Dispatch } from "react";
import type { Settings, SettingsChange } from "../lib/types";
import type { AppAction } from "./reducer";

export interface SettingsSyncApi {
  getSettings: () => Promise<Settings>;
  getBrowserSettings: () => Promise<{ settings: { open_workspace: boolean } }>;
  refreshSecrets: () => void;
}

/** Returns the handler for one `settings-changed` event. `app` reads settings.json again and
 *  replaces the store's copy (only the latest read lands, so quick writes cannot arrive out of
 *  order); `runtime` and `secrets` bump a counter the open Settings sections reload on. A read
 *  that fails keeps what is shown. Fields being edited keep their drafts (see `useDraft`). */
export function settingsSync(api: SettingsSyncApi, dispatch: Dispatch<AppAction>, isCancelled: () => boolean) {
  let latest = 0;
  return (change: SettingsChange) => {
    if (isCancelled()) return;
    if (change.scope === "app") {
      const request = ++latest;
      void api.getSettings()
        .then(settings => { if (!isCancelled() && request === latest) dispatch({ type: "settings/loaded", settings }); })
        .catch(() => { /* the next change or reload reads them again */ });
      return;
    }
    dispatch({ type: "settings/changed", scope: change.scope });
    if (change.scope === "secrets") { api.refreshSecrets(); return; }
    void api.getBrowserSettings()
      .then(view => { if (!isCancelled()) dispatch({ type: "browser/opens-workspace", value: view.settings.open_workspace }); })
      .catch(() => { /* the default stays */ });
  };
}
