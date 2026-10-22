import { useEffect, useState } from "react";
import { checkSearchServer, getWebSettings, updateWebSettings, type SearchServerStatus } from "../lib/tauri";
import { describeError } from "../state/store";
import { SettingsGroup } from "./SettingsGroup";

const STATE_LABEL: Record<SearchServerStatus["state"], string> = {
  working: "Working",
  degraded: "Some engines failing",
  json_off: "JSON output off",
  rate_limited: "Rate limited",
  unreachable: "Not reachable",
  invalid: "Not an http(s) URL",
  error: "Error",
};

/** What the status chip shows: the state, with the result count when it answered. */
function statusText(status: SearchServerStatus) {
  const label = STATE_LABEL[status.state];
  return status.results === undefined ? label : `${label} · ${status.results} result${status.results === 1 ? "" : "s"}`;
}

/** Settings › Web: the SearXNG server web_search asks, and whether it answers. */
export function WebSettingsSection() {
  const [saved, setSaved] = useState<string | null | undefined>(undefined);
  const [draft, setDraft] = useState("");
  const [status, setStatus] = useState<SearchServerStatus | null>(null);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState("");
  const check = async (url?: string) => {
    setChecking(true);
    try { setStatus(await checkSearchServer(url)); } catch (reason) { setStatus({ state: "error", message: describeError(reason) }); } finally { setChecking(false); }
  };
  useEffect(() => {
    void getWebSettings().then(web => {
      setSaved(web.search_url);
      setDraft(web.search_url ?? "");
      if (web.search_url) void check();
    }).catch(reason => setError(describeError(reason)));
  }, []);
  if (saved === undefined) return error ? <p role="alert" className="themis-form-error">{error}</p> : null;
  const save = async () => {
    if (draft.trim() === (saved ?? "")) return;
    setError("");
    try {
      const web = await updateWebSettings(draft.trim());
      setSaved(web.search_url);
      setStatus(null);
      if (web.search_url) void check();
    } catch (reason) { setError(describeError(reason)); }
  };
  const tone = !status ? "off" : status.state === "working" ? "ok" : status.state === "degraded" ? "warn" : "bad";
  return <>
    {error && <p role="alert" className="themis-form-error">{error}</p>}
    <SettingsGroup title="Search" hint="web_search asks your own SearXNG instance (JSON output on). Without one the agent still reads pages with fetch_url.">
      <label className="themis-settings-control">Search server
        <input className="themis-settings-url" aria-label="Search server" placeholder="http://127.0.0.1:8888" value={draft} onChange={event => setDraft(event.target.value)} onBlur={() => void save()} onKeyDown={event => { if (event.key === "Enter") { event.preventDefault(); void save(); } }} />
      </label>
      <div className="themis-settings-control">
        <span>Status</span>
        <span className="themis-browser-pick">
          {saved
            ? <span className={`themis-settings-state is-${tone}`} title={status?.message}>{checking ? "Checking…" : status ? statusText(status) : "Not checked"}</span>
            : <span className="themis-settings-state is-off">Not set · web_search is off</span>}
          {saved && <button type="button" className="themis-text-action" disabled={checking} onClick={() => void check()}>Check</button>}
        </span>
      </div>
      {status?.failed_engines && status.failed_engines.length > 0 && <p className="themis-settings-hint">{status.failed_engines.map(engine => engine.reason ? `${engine.engine}: ${engine.reason}` : engine.engine).join(" · ")}</p>}
      {/* Only a failure whose message says how to fix it; "Not reachable" needs no repeat. */}
      {status && ["json_off", "error", "invalid"].includes(status.state) && status.message && <p className="themis-permissions-error">{status.message}</p>}
    </SettingsGroup>
  </>;
}
