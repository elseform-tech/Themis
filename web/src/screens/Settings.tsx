import { useEffect, useState } from "react";
import { Badge, EmptyState, Switch } from "../components";
import type { Settings, UpdateStatus } from "../lib/types";
import { checkForUpdates, updateSettings } from "../lib/tauri";
import { describeError, useApp } from "../state/store";
import "./Settings.css";
import { AppearanceSettings } from "./Appearance";
import { PermissionsSettings } from "./PermissionsView";
import { BrowserSettingsSection } from "./BrowserSettings";
import { WebSettingsSection } from "./WebSettings";
import { SidebarIcon } from "./Sidebar";
import { SecretsSettings } from "./SecretsSettings";
import { ModelsSettings } from "./ModelsSettings";
import { UsageSettingsSection } from "./UsageSettings";
import { ResetIcon, SectionIcon, SettingsGroup, SettingsIconButton } from "./SettingsGroup";

const SECTIONS = ["General", "Appearance", "Models", "Usage", "Secrets", "Permissions", "Browser", "Web", "Advanced"];
/** Backend field names in validation messages, as the labels the person sees. */
const FIELD_LABELS: Array<[string, string]> = [["max_total_turns", "Turn limit"], ["context_token_budget", "Context tokens"], ["context_messages", "Recent messages"], ["concurrency_limit", "Parallel runs"], ["approval_timeout_seconds", "Approval timeout"]];
type NumberSetting = "max_total_turns" | "context_token_budget" | "context_messages" | "concurrency_limit" | "approval_timeout_seconds";

/** Pure rendering of one update status (null = never checked). */
export function UpdateStatusView({ status }: { status: UpdateStatus | null }) {
  if (status === null) return null;
  switch (status.state) {
    case "disabled":
      return (
        <div className="themis-settings-update">
          <p className="themis-settings-hint">
            Updates are disabled
            {status.message !== undefined && status.message !== ""
              ? `: ${status.message}`
              : ""}
            . See <span className="themis-settings-mono">docs/user-guide.md</span> →
            Updates for setup.
          </p>
        </div>
      );
    case "up-to-date":
      return (
        <div className="themis-settings-update">
          <p className="themis-settings-update-line">
            <Badge tone="success">up to date</Badge>
          </p>
          {status.message !== undefined && status.message !== "" && (
            <p className="themis-settings-hint">{status.message}</p>
          )}
        </div>
      );
    case "available":
      return (
        <div className="themis-settings-update">
          <p className="themis-settings-update-line">
            <Badge tone="warning">update available</Badge>
            {status.version !== undefined && status.version !== "" && (
              <span className="themis-settings-mono">{status.version}</span>
            )}
          </p>
          {status.notes !== undefined && status.notes !== "" && (
            <p className="themis-settings-hint">{status.notes}</p>
          )}
        </div>
      );
    case "error":
      return (
        <div className="themis-settings-update">
          <p className="themis-settings-update-line">
            <Badge tone="danger">check failed</Badge>
          </p>
          {status.message !== undefined && status.message !== "" && (
            <p className="themis-settings-hint">{status.message}</p>
          )}
        </div>
      );
  }
}

export function SettingsScreen() {
  const { state, dispatch } = useApp();
  const [form, setForm] = useState<Settings>(state.settings);
  const [saving, setSaving] = useState(false);
  const [section, setSection] = useState(state.settingsFocus?.section ?? "General");
  const [error, setError] = useState("");
  const [update, setUpdate] = useState<UpdateStatus | null>(null);
  const [checking, setChecking] = useState(false);

  useEffect(() => {
    setForm(state.settings);
  }, [state.settings]);

  async function save(patch: Partial<Settings>) {
    if (saving) return false;
    setSaving(true); setError("");
    try {
      const settings = await updateSettings(patch);
      dispatch({ type: "settings/patched", settings });
      return true;
    } catch (error: unknown) {
      setError(FIELD_LABELS.reduce((text, [field, label]) => text.replace(field, label), describeError(error)));
      setForm(state.settings);
      return false;
    } finally { setSaving(false); }
  }

  // Manual-only: Themis never checks for updates on boot.
  async function checkUpdates() {
    if (checking) return;
    setChecking(true);
    try {
      setUpdate(await checkForUpdates());
    } catch (error: unknown) {
      setUpdate({ state: "error", message: describeError(error) });
    } finally {
      setChecking(false);
    }
  }

  if (!state.settingsLoaded) {
    return (
      <div className="themis-settings">
        <EmptyState title="Loading settings…" hint="Fetching stored preferences." />
      </div>
    );
  }

  const number = (key: NumberSetting, label: string, min: number, max: number) => <label className="themis-settings-control">{label}
    <input className="themis-settings-number" aria-label={label} type="number" min={min} max={max} value={form[key]} onChange={event => setForm(f => ({ ...f, [key]: Number(event.target.value) }))} onBlur={() => { if (form[key] !== state.settings[key]) void save({ [key]: form[key] }); }} />
  </label>;

  return (
    <div className="themis-settings">
      <nav className="themis-settings-nav" aria-label="Settings sections">
        <button type="button" className="themis-settings-back" onClick={() => dispatch({ type: "ui/view", view: "thread" })}><SidebarIcon name="back" />Back to app</button>
        {SECTIONS.map(label => <button key={label} type="button" aria-current={section === label ? "page" : undefined} onClick={() => { setSection(label); setError(""); }}><SectionIcon section={label} />{label}</button>)}
      </nav>
      <div className="themis-settings-pane">
      <fieldset disabled={saving} className="themis-settings-fields">
      <h2 className="themis-settings-pane-title">{section}</h2>
      {error && <p role="alert" className="themis-form-error">{error}</p>}
      {section === "General" && <SettingsGroup>
        <div className="themis-settings-control"><span>Projects folder</span><span className="themis-settings-root" title={state.settings.projects_directory}>{state.settings.projects_directory}</span></div>
        <label className="themis-settings-check">Scheduled runs<Switch checked={form.automations_enabled} onChange={event => void save({ automations_enabled: event.target.checked })} /></label>
        <div className="themis-settings-control"><span>Updates</span><button type="button" className="themis-text-action" disabled={checking} onClick={() => void checkUpdates()}>{checking ? "Checking…" : "Check for updates"}</button></div>
        {update && <UpdateStatusView status={update} />}
      </SettingsGroup>}
      {section === "Appearance" && <>
        <AppearanceSettings settings={state.settings} save={save} />
        <SettingsGroup title="Feedback">
          <label className="themis-settings-check" title="Play a click when a response you started finishes">Sound when a reply finishes<Switch checked={form.completion_sound} onChange={event => void save({ completion_sound: event.target.checked })} /></label>
          {navigator.platform.startsWith("Mac") && <label className="themis-settings-check" title="Use trackpad feedback when available">Haptics<Switch checked={form.completion_haptic} onChange={event => void save({ completion_haptic: event.target.checked })} /></label>}
        </SettingsGroup>
      </>}
      {section === "Models" && <ModelsSettings save={save} saving={saving} />}
      {section === "Usage" && <UsageSettingsSection />}
      {section === "Permissions" && <PermissionsSettings editor={state.settings.config_editor ?? ""} onEditor={editor => void save({ config_editor: editor })} prefs={{ default_approval_mode: state.settings.default_approval_mode, approval_timeout_seconds: state.settings.approval_timeout_seconds }} onPrefs={patch => void save(patch)} />}
      {section === "Secrets" && <SecretsSettings focusKey={state.settingsFocus?.key} />}
      {section === "Browser" && <BrowserSettingsSection />}
      {section === "Web" && <WebSettingsSection />}
      {section === "Advanced" && <SettingsGroup title="Run limits" hint="Limits apply to new runs." action={<SettingsIconButton label="Reset limits" onClick={() => void save({ max_total_turns: 200, context_token_budget: 200000, context_messages: 20, concurrency_limit: 3 })}><ResetIcon /></SettingsIconButton>}>
        {number("max_total_turns", "Turn limit", 1, 2000)}
        {number("context_token_budget", "Context tokens", 2000, 256000)}
        {number("context_messages", "Recent messages", 1, 100)}
        {number("concurrency_limit", "Parallel runs", 1, 16)}
      </SettingsGroup>}
      </fieldset>
      </div>
    </div>
  );
}
