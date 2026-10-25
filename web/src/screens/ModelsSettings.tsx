import { useEffect, useState } from "react";
import opencodeLogoDark from "../assets/opencode-logo-dark.svg";
import opencodeLogoLight from "../assets/opencode-logo-light.svg";
import { checkGoKey, clearSecret, getSecretStatus, listGoModels, listKeys, listModelProviders, setSecret } from "../lib/tauri";
import type { GoModel, ModelProvider, Settings } from "../lib/types";
import { DEFAULT_MODEL, groupByProvider, modelLabel } from "../lib/models";
import { goUsable } from "../state/reducer";
import { describeError, useApp } from "../state/store";
import { RefreshIcon, ResetIcon, SettingsGroup, SettingsIconButton } from "./SettingsGroup";

const STATUS = {
  works: ["ok", "Connected"],
  rejected: ["bad", "Key rejected"],
  unavailable: ["off", "Couldn't reach OpenCode"],
  missing: ["off", "Not set"],
} as const;

/** A runtime.jsonc provider's status, in the same words as OpenCode Go's. */
const PROVIDER_STATUS: Record<ModelProvider["status"], readonly [string, string]> = {
  works: ["ok", "Connected"],
  unchecked: ["ok", "Key set"],
  rejected: ["bad", "Key rejected"],
  unavailable: ["off", "Couldn't reach it"],
  missing: ["off", "Not set"],
};

/**
 * Settings › Models: the providers (OpenCode Go, built in: which secret it reads, whether
 * OpenCode accepts it, and its value; then any from runtime.jsonc with their secret and
 * status), then the default model from every connected provider.
 */
export function ModelsSettings({ save, saving }: { save: (patch: Partial<Settings>) => Promise<boolean>; saving: boolean }) {
  const { state, dispatch } = useApp();
  const secret = state.settings.go_secret ?? "OPENCODE_KEY";
  const goReady = goUsable(state);
  const [checking, setChecking] = useState(false);
  const [names, setNames] = useState<string[]>([]);
  const [editing, setEditing] = useState(false);
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState("");
  const [error, setError] = useState("");
  const [models, setModels] = useState<GoModel[]>([]);
  const [providers, setProviders] = useState<ModelProvider[]>([]);
  const [loadingModels, setLoadingModels] = useState(false);
  const [modelRefresh, setModelRefresh] = useState(0);
  const [managing, setManaging] = useState(false);
  const otherReady = providers.some(provider => provider.status === "works" || provider.status === "unchecked");
  const usable = goReady || otherReady;

  // Whether OpenCode accepts the key (its model list accepts any key, so ask the chat API).
  async function check() {
    setChecking(true);
    try {
      dispatch({ type: "secrets/loaded", status: await getSecretStatus() });
      dispatch({ type: "provider/checked", state: (await checkGoKey()).state });
    } catch {
      dispatch({ type: "provider/checked", state: "unavailable" });
    } finally { setChecking(false); }
  }
  useEffect(() => { void check(); }, [secret]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    let active = true;
    void listKeys(state.activeProjectRoot).then(entries => { if (active) setNames(entries.map(entry => entry.name)); }).catch(() => undefined);
    return () => { active = false; };
  }, [state.activeProjectRoot]);

  // The providers runtime.jsonc adds, checked when the section opens and on Refresh.
  useEffect(() => {
    let active = true;
    void listModelProviders().then(list => { if (active) setProviders(list); }).catch(() => undefined);
    return () => { active = false; };
  }, [modelRefresh]);

  useEffect(() => {
    if (!usable) { setModels([]); return; }
    let active = true;
    setLoadingModels(true); setError("");
    void listGoModels().then(list => { if (active) setModels(list); })
      .catch(reason => { if (active) setError(describeError(reason)); })
      .finally(() => { if (active) setLoadingModels(false); });
    return () => { active = false; };
  }, [usable, modelRefresh]);

  async function choose(name: string) {
    setNote(""); setEditing(false);
    await save({ go_secret: name });
  }
  async function saveValue() {
    setBusy(true); setError("");
    try {
      await setSecret("go", value.trim());
      setValue(""); setEditing(false); setNote(`Saved ${secret}`);
      await check();
    } catch (reason) { setError(describeError(reason)); } finally { setBusy(false); }
  }
  async function removeValue() {
    setBusy(true); setError("");
    try {
      await clearSecret("go");
      setNote(`Removed ${secret}`);
      await check();
    } catch (reason) { setError(describeError(reason)); } finally { setBusy(false); }
  }

  // The secret and its value are shown on demand, and whenever the key needs fixing.
  const open = managing || editing || state.goKeyCheck === "missing" || state.goKeyCheck === "rejected";
  const [tone, label] = checking || state.goKeyCheck === null ? ["off", "Checking…"] : STATUS[state.goKeyCheck];
  const choices = [...new Set([secret, "OPENCODE_KEY", ...names])].sort();
  const current = models.some(model => model.id === state.settings.default_model);
  const placeholder = !usable ? "Fix the connection first" : loadingModels ? "Loading models…" : models.length === 0 ? "No models available" : current ? "Choose a model" : `Unavailable: ${state.settings.default_model}`;
  const groups = groupByProvider(models);
  const providerName = (id: string | null) => id === null ? "OpenCode Go" : providers.find(provider => provider.id === id)?.name ?? id;

  return <>
    {error && <p role="alert" className="themis-form-error">{error}</p>}
    <SettingsGroup title="Providers" hint={`OpenCode Go reads the secret ${secret} from Settings › Secrets, or an environment variable of that name, and is checked without using credits. Add OpenAI-compatible providers under "providers" in runtime.jsonc (Settings › Permissions opens it).`}>
      <div className="themis-settings-control">
        <span className="themis-settings-key-head"><span className="themis-provider-logo" role="img" aria-label="OpenCode Go"><img className="themis-provider-logo-dark" src={opencodeLogoDark} alt="" /><img className="themis-provider-logo-light" src={opencodeLogoLight} alt="" /></span></span>
        <span className="themis-browser-pick">
          <span role="status" className={`themis-settings-state is-${tone}`}>{label}</span>
          <button type="button" className="themis-text-action" disabled={checking} onClick={() => void check()}>Check</button>
          <button type="button" className="themis-text-action" aria-expanded={open} onClick={() => setManaging(!open)}>{open ? "Hide" : "Manage"}</button>
        </span>
      </div>
      {open && <>
      <label className="themis-settings-control">Secret
        <select aria-label="OpenCode Go secret" value={secret} disabled={saving || busy} onChange={event => void choose(event.target.value)}>
          {choices.map(name => <option key={name} value={name}>{name}</option>)}
        </select>
      </label>
      {!editing && <div className="themis-settings-control">
        <span>Value</span>
        <span className="themis-browser-pick">
          {note && <span role="status" className="themis-settings-hint">{note}</span>}
          <button type="button" className="themis-text-action" disabled={busy} onClick={() => { setNote(""); setEditing(true); }}>{state.secretStatus.go ? "Replace" : "Set"}</button>
          {state.secretStatus.go && <button type="button" className="themis-text-action" disabled={busy} onClick={() => void removeValue()}>Remove</button>}
        </span>
      </div>}
      {editing && <form className="themis-settings-control themis-settings-key-form" onSubmit={event => { event.preventDefault(); if (value.trim()) void saveValue(); }}>
        <input className="themis-settings-url" type="password" autoComplete="off" aria-label={`Value of ${secret}`} placeholder={`Paste the value of ${secret}`} value={value} disabled={busy} autoFocus onChange={event => setValue(event.target.value)} />
        <span className="themis-browser-pick">
          <button type="submit" className="themis-text-action" disabled={busy || !value.trim()}>{busy ? "Saving…" : "Save"}</button>
          <button type="button" className="themis-text-action" disabled={busy} onClick={() => { setValue(""); setEditing(false); }}>Cancel</button>
        </span>
      </form>}
      </>}
      {providers.map(provider => {
        const [providerTone, providerLabel] = PROVIDER_STATUS[provider.status];
        return <div key={provider.id} className="themis-settings-control" aria-label={provider.name} role="group">
          <span>{provider.name}</span>
          <span className="themis-browser-pick">
            <span className="themis-settings-hint" title="Its key, from Settings › Secrets">{provider.secret}</span>
            <span role="status" className={`themis-settings-state is-${providerTone}`}>{providerLabel}</span>
          </span>
        </div>;
      })}
    </SettingsGroup>
    <SettingsGroup title="Default model" action={<span className="themis-settings-group-actions">
      <SettingsIconButton label="Refresh models" disabled={loadingModels} onClick={() => setModelRefresh(count => count + 1)}><RefreshIcon /></SettingsIconButton>
      <SettingsIconButton label="Reset model" onClick={() => void save({ default_provider: "go", default_model: DEFAULT_MODEL })}><ResetIcon /></SettingsIconButton>
    </span>}>
      <label className="themis-settings-control">New chats use<span className="themis-settings-model-picker">
        <select aria-label="Model" value={current ? state.settings.default_model : ""} disabled={saving || loadingModels || !usable || models.length === 0} onChange={event => void save({ default_model: event.target.value })}>
          <option value="" disabled>{placeholder}</option>
          {groups.length === 1 && groups[0].provider === null
            ? groups[0].models.map(model => <ModelOption key={model.id} model={model} />)
            : groups.map(group => <optgroup key={group.provider ?? "go"} label={providerName(group.provider)}>
              {group.models.map(model => <ModelOption key={model.id} model={model} />)}
            </optgroup>)}
        </select>
      </span></label>
    </SettingsGroup>
  </>;
}

/** A model in its provider's group, named as that provider names it. */
function ModelOption({ model }: { model: GoModel }) {
  const label = modelLabel(model);
  return <option value={model.id} disabled={!!model.unavailable_reason} title={model.unavailable_reason ?? undefined}>{model.provider ? label.slice(model.provider.length + 1) : label}</option>;
}
