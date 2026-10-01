import { useEffect, useState } from "react";
import { Badge, Button, EmptyState, Input } from "../components";
import opencodeLogoDark from "../assets/opencode-logo-dark.svg";
import opencodeLogoLight from "../assets/opencode-logo-light.svg";
import type {
  Diagnostics,
  GoModel,
  SecretStatus,
  Settings,
  ThemeMode,
  UpdateStatus,
} from "../lib/types";
import {
  checkForUpdates,
  clearSecret,
  getDiagnostics,
  getSecretStatus,
  setSecret,
  updateSettings,
  listGoModels,
} from "../lib/tauri";
import { describeError, toast, useApp } from "../state/store";
import { pickProjectDirectory } from "./projectPick";
import { copyDiagnostics } from "./diagnostics";
import "./Settings.css";

const THEMES: ThemeMode[] = ["dark", "light", "system"];
const SECRET_PROVIDERS: Array<keyof SecretStatus> = ["go"];

/** Pure rendering of one update status (null = never checked). */
export function UpdateStatusView({ status }: { status: UpdateStatus | null }) {
  if (status === null) {
    return (
      <p className="themis-settings-hint">
        Not checked yet — press &ldquo;Check for updates&rdquo;.
      </p>
    );
  }
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
  const [models, setModels] = useState<GoModel[]>([]);
  const [connecting, setConnecting] = useState(false);
  const [connection, setConnection] = useState("");
  const [modelRefresh, setModelRefresh] = useState(0);
  const [saving, setSaving] = useState(false);
  const [keys, setKeys] = useState<Record<string, string>>({});
  const [keyBusy, setKeyBusy] = useState<string | null>(null);
  const [section, setSection] = useState("General");
  const [saved, setSaved] = useState("");
  const [error, setError] = useState("");
  const [update, setUpdate] = useState<UpdateStatus | null>(null);
  const [checking, setChecking] = useState(false);
  const [diagnostics, setDiagnostics] = useState<Diagnostics | null>(null);
  const [copying, setCopying] = useState(false);

  useEffect(() => {
    setForm(state.settings);
  }, [state.settings]);

  useEffect(() => {
    if (section !== "Models & connections" || !state.secretStatus.go) return;
    let active = true;
    setConnecting(true); setConnection(""); setError(""); setModels([]);
    void listGoModels().then(models => {
      if (active) { setModels(models); setConnection(`Connected · ${models.length} models available`); }
    }).catch((error: unknown) => {
      if (active) setError(describeError(error));
    }).finally(() => {
      if (active) setConnecting(false);
    });
    return () => { active = false; };
  }, [section, state.secretStatus.go, modelRefresh]);

  async function save(patch: Partial<Settings>) {
    if (saving) return;
    setSaving(true); setError(""); setSaved("");
    try {
      const settings = await updateSettings(patch);
      dispatch({ type: "settings/patched", settings });
      setSaved("Saved");
    } catch (error: unknown) {
      setError(describeError(error));
      setForm(state.settings);
    } finally { setSaving(false); }
  }
  async function chooseFolder() {
    const picked = await pickProjectDirectory();
    if (picked.kind === "path") await save({ projects_directory: picked.path });
  }

  async function refreshSecrets() {
    try {
      const status = await getSecretStatus();
      dispatch({ type: "secrets/loaded", status });
    } catch (error: unknown) {
      toast(dispatch, `Secret status failed: ${describeError(error)}`, "warning");
    }
  }

  async function saveKey(provider: (typeof SECRET_PROVIDERS)[number]) {
    const value = (keys[provider] ?? "").trim();
    if (value === "") return;
    setKeyBusy(provider);
    try {
      await setSecret(provider, value);
      setKeys((prev) => ({ ...prev, [provider]: "" }));
      await refreshSecrets();
      toast(dispatch, `Saved ${provider} key in Keychain`, "success");
    } catch (error: unknown) {
      toast(dispatch, `Save ${provider} key failed: ${describeError(error)}`, "danger");
    } finally {
      setKeyBusy(null);
    }
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

  async function copyDiagnosticsToClipboard() {
    if (copying) return;
    setCopying(true);
    try {
      const loaded = await copyDiagnostics({
        load: getDiagnostics,
        writeClipboard: (text) => navigator.clipboard.writeText(text),
        notify: (message, tone) => toast(dispatch, message, tone),
      });
      if (loaded !== null) setDiagnostics(loaded);
    } finally {
      setCopying(false);
    }
  }

  async function clearKey(provider: (typeof SECRET_PROVIDERS)[number]) {
    setKeyBusy(provider);
    try {
      await clearSecret(provider);
      await refreshSecrets();
      toast(dispatch, `Cleared ${provider} key`, "success");
    } catch (error: unknown) {
      toast(dispatch, `Clear ${provider} key failed: ${describeError(error)}`, "danger");
    } finally {
      setKeyBusy(null);
    }
  }

  if (!state.settingsLoaded) {
    return (
      <div className="themis-settings">
        <EmptyState title="Loading settings…" hint="Fetching stored preferences." />
      </div>
    );
  }

  return (
    <div className="themis-settings">
      <div className="themis-settings-heading"><div><h2 className="themis-settings-title">Settings</h2><p className="themis-settings-hint">App defaults · Changes save automatically.</p></div><span role="status" className="themis-settings-hint">{saving ? "Saving…" : saved}</span></div>
      <nav className="themis-settings-tabs" aria-label="Settings sections">{["General", "Appearance", "Models & connections", "Permissions", "Advanced"].map(label => <button key={label} aria-current={section === label ? "page" : undefined} onClick={() => { setSection(label); setSaved(""); }}>{label}</button>)}</nav>
      {error && <p role="alert" className="themis-form-error">{error}</p>}
      <fieldset disabled={saving} className="themis-settings-fields">
      {section === "General" && <>
        <section className="themis-settings-section" aria-label="Projects folder"><h3 className="themis-settings-subtitle">Projects folder</h3>
          <Input id="projects-directory" label="Default projects folder" value={form.projects_directory} onChange={event => setForm(f => ({ ...f, projects_directory: event.target.value }))} onBlur={() => { if (form.projects_directory !== state.settings.projects_directory) void save({ projects_directory: form.projects_directory }); }} />
          <div><Button variant="ghost" size="small" onClick={() => void chooseFolder()}>Choose folder…</Button></div>
          <p className="themis-settings-hint">New projects are created here. Existing projects stay where they are.</p>
          <p className="themis-settings-hint">Your selected project, thread and drafts are restored when you return.</p>
        </section>
        <section className="themis-settings-section" aria-label="Automations"><h3 className="themis-settings-subtitle">Automations</h3><label className="themis-settings-check"><input type="checkbox" checked={form.automations_enabled} onChange={event => void save({ automations_enabled: event.target.checked })} />Enable scheduled runs while Themis is open</label><p className="themis-settings-hint">Manage schedules in Automations. You can still run an automation manually when scheduling is off.</p></section>
        <details className="themis-settings-section"><summary>Updates & diagnostics</summary><UpdateStatusView status={update} /><div><Button variant="ghost" size="small" disabled={checking} onClick={() => void checkUpdates()}>{checking ? "Checking…" : "Check for updates"}</Button><Button variant="ghost" size="small" disabled={copying} onClick={() => void copyDiagnosticsToClipboard()}>{copying ? "Copying…" : "Copy diagnostics"}</Button></div><p className="themis-settings-hint">Updates are checked only when you ask. Diagnostics include settings and recent errors, never API keys.</p>{diagnostics && <p className="themis-settings-hint">{diagnostics.app_version} · {diagnostics.os}</p>}</details>
      </>}
      {section === "Appearance" && <section className="themis-settings-section" aria-label="Appearance preferences">
        <h3 className="themis-settings-subtitle">Appearance</h3>
        <label className="themis-settings-control">Theme<select value={form.theme} onChange={event => void save({ theme: event.target.value as ThemeMode })}>{THEMES.map(theme => <option key={theme} value={theme}>{theme[0]?.toUpperCase()}{theme.slice(1)}</option>)}</select></label>
        <label className="themis-settings-control">Color palette<select value={form.theme_palette ?? "system"} onChange={event => void save({ theme_palette: event.target.value as Settings["theme_palette"] })}>{[["system", "Silver & charcoal"], ["warm", "Warm stone"], ["ocean", "Ocean"], ["forest", "Forest"], ["violet", "Violet"]].map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select></label>
        <label className="themis-settings-control">Font<select value={form.font_family ?? "system"} onChange={event => void save({ font_family: event.target.value as Settings["font_family"] })}>{[["system", "System"], ["sans", "Sans serif"], ["serif", "Serif"], ["mono", "Monospace"], ["rounded", "Rounded"]].map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select></label>
        <label className="themis-settings-control">Text size<select value={form.text_size} onChange={event => void save({ text_size: Number(event.target.value) })}>{[12, 13, 14, 15, 16, 17, 18, 20, 22].map(size => <option key={size} value={size}>{size}px{size === 13 ? " · Default" : ""}</option>)}</select></label>
        <p className="themis-settings-hint">System font · Compact, clear text. Code keeps its monospace font.</p>
        <label className="themis-settings-check"><input type="checkbox" checked={form.sidebar_hover} onChange={event => void save({ sidebar_hover: event.target.checked })} />Reveal the collapsed sidebar on hover</label><p className="themis-settings-hint">Hover at the left edge to reveal it. Use the arrow to keep it open, or ⌘ / Ctrl + B to toggle it.</p>
        <h3 className="themis-settings-subtitle">Completion feedback</h3>
        <label className="themis-settings-check"><input type="checkbox" checked={form.completion_sound} onChange={event => void save({ completion_sound: event.target.checked })} />Play a click when my response finishes</label>
        {navigator.platform.startsWith("Mac") && <label className="themis-settings-check"><input type="checkbox" checked={form.completion_haptic} onChange={event => void save({ completion_haptic: event.target.checked })} />Use trackpad haptic feedback when available</label>}
        <p className="themis-settings-hint">Only for responses you start; scheduled runs stay quiet.</p>
        <div><Button variant="ghost" size="small" onClick={() => void save({ theme: "system", theme_palette: "system", font_family: "system", text_size: 13, sidebar_hover: true })}>Reset appearance</Button></div>
      </section>}
      {section === "Models & connections" && <>
        <section className="themis-settings-section" aria-label="Model defaults"><h3 className="themis-settings-subtitle">Model defaults</h3>
          <label className="themis-settings-control">Default model<span className="themis-settings-model-picker"><span className="themis-provider-logo" role="img" aria-label="OpenCode Go"><img className="themis-provider-logo-dark" src={opencodeLogoDark} alt="" /><img className="themis-provider-logo-light" src={opencodeLogoLight} alt="" /></span><select aria-label="Default model" value={models.some(model => model.id === form.default_model) ? form.default_model : ""} disabled={connecting || !state.secretStatus.go || models.length === 0} onChange={event => void save({ default_model: event.target.value })}><option value="" disabled>{connecting ? "Loading models…" : !state.secretStatus.go ? "Connect OpenCode Go to load models" : models.length === 0 ? "No models available" : models.some(model => model.id === form.default_model) ? "Choose a model" : `Current model unavailable: ${form.default_model}`}</option>{models.map(model => <option key={model.id} value={model.id}>{model.id}</option>)}</select></span></label>
          <div><Button variant="ghost" size="small" disabled={connecting || !state.secretStatus.go} onClick={() => setModelRefresh(value => value + 1)}>Refresh models</Button></div>{connection && <p role="status" className="themis-settings-hint">{connection}</p>}
          <p className="themis-settings-hint">Used for new threads. Existing threads keep their model.</p>
          <div><Button variant="ghost" size="small" onClick={() => void save({ default_provider: "go", default_model: "muse-spark-1.3-contributor" })}>Reset model defaults</Button></div>
        </section>
        <section className="themis-settings-section" aria-label="Connections"><h3 className="themis-settings-subtitle">Connections</h3><p className="themis-settings-hint">Keys entered here are saved in macOS Keychain and remain available after restart. OpenCode Go can also use OPENCODE_KEY from the launch environment.</p>
          {SECRET_PROVIDERS.map(provider => <div key={provider} className="themis-settings-key"><div className="themis-settings-key-head"><span className="themis-provider-logo" role="img" aria-label="OpenCode Go"><img className="themis-provider-logo-dark" src={opencodeLogoDark} alt="" /><img className="themis-provider-logo-light" src={opencodeLogoLight} alt="" /></span><Badge tone={state.secretStatus[provider] ? "success" : "default"}>{state.secretStatus[provider] ? "Key available" : "Not configured"}</Badge></div>
          <details><summary>Manage key</summary><div className="themis-settings-key-form"><Input id={`themis-key-${provider}`} label={`${provider} API key`} type="password" autoComplete="off" value={keys[provider] ?? ""} disabled={keyBusy !== null} onChange={event => setKeys(prev => ({ ...prev, [provider]: event.target.value }))} /><Button variant="primary" size="small" disabled={keyBusy !== null || !(keys[provider] ?? "").trim()} onClick={() => void saveKey(provider)}>Save key</Button><Button variant="ghost" size="small" disabled={keyBusy !== null || !state.secretStatus[provider]} onClick={() => void clearKey(provider)}>Forget saved key</Button></div>{provider === "go" && <p className="themis-settings-hint">Forgetting a saved key does not remove OPENCODE_KEY from your environment.</p>}</details></div>)}
        </section>
      </>}
      {section === "Permissions" && <section className="themis-settings-section" aria-label="Permissions"><h3 className="themis-settings-subtitle">Approval preferences</h3><label className="themis-settings-check"><input type="checkbox" checked={form.confirm_reads} onChange={event => void save({ confirm_reads: event.target.checked })} />Ask before read tools</label><dl className="themis-permissions"><dt>File access</dt><dd>File tools stay inside the thread’s project or worktree.</dd><dt>Changes and commands</dt><dd>Writes, shell commands, network and destructive tool calls pause for approval.</dd><dt>Approval duration</dt><dd>“Always” remembers a tool decision for the current run only.</dd></dl></section>}
      {section === "Advanced" && <section className="themis-settings-section" aria-label="Advanced settings">
        <h3 className="themis-settings-subtitle">Run and context limits</h3>
        <Input id="themis-max-turns" label="Turns between automatic checkpoints (1–200)" type="number" min={1} max={200} value={form.max_turns} onChange={event => setForm(f => ({ ...f, max_turns: Number(event.target.value) }))} onBlur={() => { if (form.max_turns !== state.settings.max_turns) void save({ max_turns: form.max_turns }); }} />
        <Input id="themis-total-turns" label="Hard turn limit per request (1–2000)" type="number" min={1} max={2000} value={form.max_total_turns} onChange={event => setForm(f => ({ ...f, max_total_turns: Number(event.target.value) }))} onBlur={() => { if (form.max_total_turns !== state.settings.max_total_turns) void save({ max_total_turns: form.max_total_turns }); }} />
        <Input id="themis-context-budget" label="Approximate model context budget in tokens (2000–200000)" type="number" min={2000} max={200000} value={form.context_token_budget} onChange={event => setForm(f => ({ ...f, context_token_budget: Number(event.target.value) }))} onBlur={() => { if (form.context_token_budget !== state.settings.context_token_budget) void save({ context_token_budget: form.context_token_budget }); }} />
        <Input id="themis-context-messages" label="Recent full messages to keep (1–100)" type="number" min={1} max={100} value={form.context_messages} onChange={event => setForm(f => ({ ...f, context_messages: Number(event.target.value) }))} onBlur={() => { if (form.context_messages !== state.settings.context_messages) void save({ context_messages: form.context_messages }); }} />
        <Input id="themis-concurrency" label="Parallel runs (1–16)" type="number" min={1} max={16} value={form.concurrency_limit} onChange={event => setForm(f => ({ ...f, concurrency_limit: Number(event.target.value) }))} onBlur={() => { if (form.concurrency_limit !== state.settings.concurrency_limit) void save({ concurrency_limit: form.concurrency_limit }); }} />
        <Input id="themis-approval-timeout" label="Approval timeout in seconds (30–600)" type="number" min={30} max={600} value={form.approval_timeout_seconds} onChange={event => setForm(f => ({ ...f, approval_timeout_seconds: Number(event.target.value) }))} onBlur={() => { if (form.approval_timeout_seconds !== state.settings.approval_timeout_seconds) void save({ approval_timeout_seconds: form.approval_timeout_seconds }); }} />
        <p className="themis-settings-hint">The current thread model summarizes older context at checkpoints. The run continues automatically at the checkpoint interval. The hard limit stops it after saving a handoff for the next message. Token counts are estimates. Changes apply to new runs.</p>
        <div><Button variant="ghost" size="small" onClick={() => void save({ max_turns: 20, max_total_turns: 200, context_token_budget: 16000, context_messages: 20, concurrency_limit: 3, approval_timeout_seconds: 300 })}>Reset advanced settings</Button></div>
      </section>}
      </fieldset>
    </div>
  );
}
