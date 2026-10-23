import { useEffect, useState } from "react";
import { listKeys, removeKey, setKey, type KeyEntry } from "../lib/tauri";
import { describeError, useApp } from "../state/store";
import { SettingsGroup } from "./SettingsGroup";

const NAME = /^[A-Za-z0-9_]{1,128}$/;

function KeyRow({ entry, editing, onEdit, onSaved, onRemoved }: { entry: KeyEntry; editing: boolean; onEdit: (open: boolean) => void; onSaved: () => void; onRemoved: () => void }) {
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const save = async () => {
    setBusy(true); setError("");
    try { await setKey(entry.name, value.trim()); setValue(""); onSaved(); } catch (reason) { setError(describeError(reason)); } finally { setBusy(false); }
  };
  const remove = async () => {
    setBusy(true); setError("");
    try { await removeKey(entry.name); onRemoved(); } catch (reason) { setError(describeError(reason)); } finally { setBusy(false); }
  };
  const state = entry.saved ? ["ok", "Saved"] : entry.environment ? ["ok", "From environment"] : ["bad", "Missing"];
  return <div className="themis-settings-key">
    <div className="themis-settings-control">
      <span className="themis-key-name"><code>{entry.name}</code>{entry.used_by.length > 0 && <small className="themis-settings-hint">{entry.used_by.join(", ")}</small>}</span>
      <span className="themis-browser-pick">
        <span className={`themis-settings-state is-${state[0]}`}>{state[1]}</span>
        {!editing && <button type="button" className="themis-text-action" onClick={() => onEdit(true)}>{entry.saved ? "Replace" : "Add"}</button>}
        {!editing && entry.saved && <button type="button" className="themis-text-action" disabled={busy} onClick={() => void remove()}>{busy ? "Removing…" : "Remove"}</button>}
      </span>
    </div>
    {editing && <form className="themis-settings-control themis-settings-key-form" onSubmit={event => { event.preventDefault(); if (value.trim()) void save(); }}>
      <input className="themis-settings-url" type="password" autoComplete="off" aria-label={`Value of ${entry.name}`} placeholder="Paste the secret" value={value} disabled={busy} autoFocus onChange={event => setValue(event.target.value)} />
      <span className="themis-browser-pick">
        <button type="submit" className="themis-text-action" disabled={busy || !value.trim()}>{busy ? "Saving…" : "Save"}</button>
        <button type="button" className="themis-text-action" disabled={busy} onClick={() => { setValue(""); onEdit(false); }}>Cancel</button>
      </span>
    </form>}
    {error && <p className="themis-permissions-error">{error}</p>}
  </div>;
}

/** Settings › Secrets: secrets providers and MCP servers read by name (${NAME} in MCP),
 * kept in the system's credential store. */
export function SecretsSettings({ focusKey }: { focusKey?: string }) {
  const { state } = useApp();
  const [entries, setEntries] = useState<KeyEntry[] | null>(null);
  const [editing, setEditing] = useState<string | null>(focusKey ?? null);
  const [newName, setNewName] = useState("");
  const [error, setError] = useState("");
  const load = () => listKeys(state.activeProjectRoot).then(setEntries).catch(reason => setError(describeError(reason)));
  useEffect(() => { void load(); }, []); // eslint-disable-line react-hooks/exhaustive-deps
  if (!entries) return error ? <p role="alert" className="themis-form-error">{error}</p> : null;
  // A key asked for by name (from a server's card) shows even before anything uses it.
  const shown = focusKey && !entries.some(entry => entry.name === focusKey)
    ? [...entries, { name: focusKey, saved: false, environment: false, used_by: [] }]
    : entries;
  const draftName = newName.trim();
  const addNamed = () => {
    if (!NAME.test(draftName)) { setError("Secret names use letters, digits and _"); return; }
    setError("");
    if (!shown.some(entry => entry.name === draftName)) setEntries([...entries, { name: draftName, saved: false, environment: false, used_by: [] }]);
    setEditing(draftName); setNewName("");
  };
  const done = () => { setEditing(null); void load(); };
  return <>
    <SettingsGroup hint="Models › Connections and MCP servers (as ${NAME}) read a secret by its name. Secrets stay in this computer's credential store and are never shown again.">
      {shown.length === 0 && <p className="themis-settings-hint">No secrets yet.</p>}
      {shown.map(entry => <KeyRow key={entry.name} entry={entry} editing={editing === entry.name} onEdit={open => setEditing(open ? entry.name : null)} onSaved={done} onRemoved={done} />)}
      <form className="themis-settings-control" onSubmit={event => { event.preventDefault(); if (draftName) addNamed(); }}>
        <input className="themis-settings-url" aria-label="New secret name" placeholder="New secret name, such as GITHUB_TOKEN" value={newName} onChange={event => setNewName(event.target.value)} />
        <button type="submit" className="themis-text-action" disabled={!draftName}>Add</button>
      </form>
    </SettingsGroup>
    {error && <p role="alert" className="themis-form-error">{error}</p>}
  </>;
}
