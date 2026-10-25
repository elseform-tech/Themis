import { useEffect, useState } from "react";
import { Badge, EmptyState, Switch } from "../components";
import type { Settings, UpdateStatus } from "../lib/types";
import { checkForUpdates, notificationPermission, updateSettings } from "../lib/tauri";
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
const FIELD_LABELS: Array<[string, string]> = [["max_total_turns", "Turn limit"], ["context_token_cap", "Context tokens"], ["concurrency_limit", "Parallel runs"], ["approval_timeout_seconds", "Approval timeout"]];
type NumberSetting = "max_total_turns" | "context_token_cap" | "concurrency_limit" | "approval_timeout_seconds";
/** The cap Custom starts from: the Auto ceiling, so switching changes nothing until edited. */
const CUSTOM_CONTEXT_START = 200000;

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

/** Settings › General › Notifications. Goal notifications are macOS-only, so
 *  their switch (like Haptics) shows only there; permission is asked when the
 *  switch is turned on, and a refusal leaves one short hint. */
function NotificationSettings({ form, save }: { form: Settings; save: (patch: Partial<Settings>) => Promise<boolean> }) {
  const mac = navigator.platform.startsWith("Mac");
  const [denied, setDenied] = useState(false);
  const goals = form.goal_notifications;
  useEffect(() => {
    if (!mac || !goals) { setDenied(false); return; }
    let current = true;
    void Promise.resolve().then(() => notificationPermission(false))
      .then(permission => { if (current) setDenied(permission === "denied"); })
      .catch(() => {});
    return () => { current = false; };
  }, [mac, goals]);
  async function toggleGoals(enabled: boolean) {
    if (!await save({ goal_notifications: enabled }) || !enabled) return;
    try { setDenied(await notificationPermission(true) === "denied"); } catch { /* not in the desktop app */ }
  }
  return <SettingsGroup title="Notifications">
    <label className="themis-settings-check" title="Play a click when a response you started finishes">Sound when a reply finishes<Switch checked={form.completion_sound} onChange={event => void save({ completion_sound: event.target.checked })} /></label>
    {mac && <label className="themis-settings-check" title="Use trackpad feedback when available">Haptics<Switch checked={form.completion_haptic} onChange={event => void save({ completion_haptic: event.target.checked })} /></label>}
    {mac && <label className="themis-settings-check" title="Only while Themis is in the background">Notify when a goal finishes or pauses<Switch checked={goals} onChange={event => void toggleGoals(event.target.checked)} /></label>}
    {mac && goals && denied && <p className="themis-settings-hint">Notifications are off for Themis in System Settings › Notifications.</p>}
  </SettingsGroup>;
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

  const numberInput = (key: NumberSetting, label: string, min: number, max: number) =>
    <input className="themis-settings-number" aria-label={label} type="number" min={min} max={max} value={form[key]} onChange={event => setForm(f => ({ ...f, [key]: Number(event.target.value) }))} onBlur={() => { if (form[key] !== state.settings[key]) void save({ [key]: form[key] }); }} />;
  const number = (key: NumberSetting, label: string, min: number, max: number) => <label className="themis-settings-control">{label}
    {numberInput(key, label, min, max)}
  </label>;
  // Auto (0) follows the model's context window; Custom caps it.
  const customContext = state.settings.context_token_cap !== 0;
  const contextTokens = <div className="themis-settings-control">
    <span title="Auto uses the model's context window">Context tokens</span>
    <span className="themis-settings-pair">
      {customContext && numberInput("context_token_cap", "Context token cap", 2000, 1000000)}
      <select aria-label="Context tokens" value={customContext ? "custom" : "auto"} onChange={event => void save({ context_token_cap: event.target.value === "auto" ? 0 : CUSTOM_CONTEXT_START })}>
        <option value="auto">Auto</option>
        <option value="custom">Custom</option>
      </select>
    </span>
  </div>;

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
      {section === "General" && <NotificationSettings form={form} save={save} />}
      {section === "Appearance" && <AppearanceSettings settings={state.settings} save={save} />}
      {section === "Models" && <ModelsSettings save={save} saving={saving} />}
      {section === "Usage" && <UsageSettingsSection />}
      {section === "Permissions" && <PermissionsSettings editor={state.settings.config_editor ?? ""} onEditor={editor => void save({ config_editor: editor })} prefs={{ default_approval_mode: state.settings.default_approval_mode, approval_timeout_seconds: state.settings.approval_timeout_seconds }} onPrefs={patch => void save(patch)} />}
      {section === "Secrets" && <SecretsSettings focusKey={state.settingsFocus?.key} />}
      {section === "Browser" && <BrowserSettingsSection />}
      {section === "Web" && <WebSettingsSection />}
      {section === "Advanced" && <SettingsGroup title="Run limits" hint="Limits apply to new runs." action={<SettingsIconButton label="Reset limits" onClick={() => void save({ max_total_turns: 200, context_token_cap: 0, concurrency_limit: 3 })}><ResetIcon /></SettingsIconButton>}>
        {number("max_total_turns", "Turn limit", 1, 2000)}
        {contextTokens}
        {number("concurrency_limit", "Parallel runs", 1, 16)}
      </SettingsGroup>}
      </fieldset>
      </div>
    </div>
  );
}
