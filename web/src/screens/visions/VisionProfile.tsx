// A chief of staff's profile (board ChiefProfile), opened from its channel's header in the
// channel's place: "← # channel", the chief in 3D with its name, job and state, and Pause; its
// soul, saved in place; its model; the channel's autonomy and budget; its team; its routines.
// Atlas, the built-in admin, shows its soul and model only. Workers have no profile.
import { useEffect, useState } from "react";
import { listGoModels, listModelProviders } from "../../lib/tauri";
import type { GoModel, ModelProvider } from "../../lib/types";
import { ADMIN_VISION_ID, getVisionsSettings, pauseVision, saveVision, type Autonomy, type ChannelInfo, type VisionInfo, type VisionProfile as Profile, type VisionsSettings } from "../../lib/visions";
import { describeError, useApp } from "../../state/store";
import { ModelOptions } from "../ModelsSettings";
import { AUTONOMY_STOPS, LEVELS } from "./autonomy";
import { CharacterAvatar } from "./CharacterAvatar";
import { lookOf } from "./cast";
import { projectChannels, saveChannelChange } from "./channels";
import { EXPRESSION_LABEL } from "./Expression";
import { Routines, type RoutineControls } from "./Routines";
import { PauseIcon, PlayIcon } from "./SidePanel";
import { ChiefScene } from "./three/ChiefScene";
import { chiefExpression, statusOf, teamOf, workerStatus } from "./team";
import "./VisionsPanels.css";

type Save = (change: Partial<Profile>) => Promise<void>;
type AppState = ReturnType<typeof useApp>["state"];

/** The models the app offers, with their providers' names. */
function useModels() {
  const [models, setModels] = useState<GoModel[]>([]);
  const [providers, setProviders] = useState<ModelProvider[]>([]);
  useEffect(() => {
    void listGoModels().then(setModels).catch(() => setModels([]));
    void listModelProviders().then(setProviders).catch(() => setProviders([]));
  }, []);
  const providerName = (id: string | null) => id === null ? "OpenCode Go" : providers.find(provider => provider.id === id)?.name ?? id;
  return { models, providerName };
}

/** Settings › Visions, for the channel's autonomy and budget when it sets none of its own. */
function useVisionsSettings(): VisionsSettings | null {
  const [settings, setSettings] = useState<VisionsSettings | null>(null);
  useEffect(() => { void getVisionsSettings().then(view => setSettings(view.settings)).catch(() => setSettings(null)); }, []);
  return settings;
}

/** The model picker with its provider's mark (none for the chat model). */
function ModelField({ vision, save }: { vision: VisionInfo; save: Save }) {
  const { models, providerName } = useModels();
  const model = vision.model ?? "";
  const known = model === "" || models.some(entry => entry.id === model);
  const listed = models.find(entry => entry.id === model);
  const mark = listed ? (listed.provider ? providerName(listed.provider).slice(0, 2) : "Go") : "";
  return <label className="themis-profile-field">
    {mark && <span className="themis-provider-mark" aria-hidden="true">{mark}</span>}
    <select aria-label="Model" value={model} onChange={event => void save({ model: event.target.value || undefined })}>
      <option value="">Chat model</option>
      {!known && <option value={model}>{model} (unavailable)</option>}
      <ModelOptions models={models} providerName={providerName} />
    </select>
  </label>;
}

/** The soul as a text box, saved when the person leaves it. */
function SoulField({ soul, save }: { soul: string; save: Save }) {
  const [draft, setDraft] = useState<string | null>(null);
  const commit = () => {
    if (draft === null) return;
    if (draft !== soul) void save({ soul: draft }).then(() => setDraft(null), () => undefined);
    else setDraft(null);
  };
  return <textarea className="themis-profile-box themis-profile-soul" aria-label="Soul" rows={5} maxLength={16384} value={draft ?? soul}
    onChange={event => setDraft(event.target.value)} onBlur={commit} />;
}

/** The channel's autonomy (saved on the channel) and today's budget. `spentToday` is unknown
 *  until the server reports a channel's spend. */
function ChannelLimits({ channel, settings, spentToday, onError }: { channel: ChannelInfo; settings: VisionsSettings | null; spentToday?: number; onError: (reason: unknown) => void }) {
  const { state, dispatch } = useApp();
  const level = channel.autonomy ?? settings?.autonomy;
  const limit = channel.daily_budget_usd ?? settings?.daily_budget_usd;
  const choices = AUTONOMY_STOPS.filter(stop => stop !== "custom" || level === "custom");
  return <div className="themis-profile-pair">
    <section className="themis-profile-block">
      <h2 className="themis-visions-label">Autonomy</h2>
      <label className="themis-profile-field">
        <select aria-label="Autonomy" value={level ?? ""} onChange={event => void saveChannelChange(state, dispatch, channel, { autonomy: event.target.value as Autonomy }).catch(onError)}>
          {level === undefined && <option value="" />}
          {choices.map(stop => <option key={stop} value={stop}>{LEVELS[stop]}</option>)}
        </select>
      </label>
    </section>
    {limit !== undefined && <section className="themis-profile-block">
      <h2 className="themis-visions-label">Budget</h2>
      {spentToday === undefined
        ? <p className="themis-profile-budget">${limit} a day</p>
        : <div className="themis-profile-budget">
          <span>${spentToday.toFixed(2)} today</span><span className="themis-profile-of">of ${limit}</span>
          <span className="themis-profile-bar" role="meter" aria-label="Today's spend" aria-valuemin={0} aria-valuemax={limit} aria-valuenow={Math.min(spentToday, limit)}><i style={{ width: `${Math.min(100, (spentToday / Math.max(limit, 0.01)) * 100)}%` }} /></span>
        </div>}
    </section>}
  </div>;
}

/** The chief's workers: look, name, role, model and status; each opens its thread. */
function Team({ channel }: { channel: ChannelInfo }) {
  const { state, dispatch } = useApp();
  const { workers } = teamOf(state, channel);
  if (workers.length === 0) return null;
  return <section className="themis-profile-block">
    <h2 className="themis-visions-label">Team</h2>
    <ul className="themis-profile-box themis-profile-team" aria-label="Team">
      {workers.map(worker => {
        const status = workerStatus(state, channel, worker);
        return <li key={worker.id}>
          <button type="button" aria-label={`${worker.name}, ${status.label}`} onClick={() => { dispatch({ type: "visions/open", channelId: channel.id }); dispatch({ type: "visions/panel", panel: { kind: "worker", visionId: worker.id } }); }}>
            <CharacterAvatar look={lookOf(worker)} size={24} />
            <b>{worker.name}</b>
            <span className="themis-profile-role">{worker.role}</span>
            {worker.model && <span className="themis-profile-model">{worker.model}</span>}
            <span className="themis-state-dot" style={{ background: status.dot }} aria-hidden="true" />
          </button>
        </li>;
      })}
    </ul>
  </section>;
}

/** The channel the Vision leads: the open one when it leads that, else the first in the project. */
function ledChannel(state: AppState, visionId: string): ChannelInfo | undefined {
  const { hq, channels, current } = projectChannels(state);
  if (current?.chief === visionId) return current;
  return [hq, ...channels].find(channel => channel.chief === visionId);
}

/** The chief in 3D, its name, its job in its channel, its state and Pause. */
function ProfileHero({ vision, channel, onError }: { vision: VisionInfo; channel: ChannelInfo | undefined; onError: (reason: unknown) => void }) {
  const { state } = useApp();
  const expression = channel ? chiefExpression(state, channel, vision) : "calm";
  const status = statusOf(expression);
  const job = vision.id === ADMIN_VISION_ID ? "Admin" : "Chief of staff";
  return <div className="themis-profile-hero">
    <ChiefScene look={lookOf(vision)} expression={expression} name={vision.name} size={150} />
    <div className="themis-profile-who">
      <h1 className="themis-team-gradient themis-profile-name">{vision.name}</h1>
      <div className="themis-team-chief">
        <span>{job}{channel ? `, # ${channel.name}` : ""}</span>
        <span className="themis-state-dot" style={{ background: status.dot }} aria-hidden="true" />
        <span>{EXPRESSION_LABEL[expression]}</span>
      </div>
      <button type="button" className="themis-visions-quiet-button themis-profile-pause" aria-label={`${vision.paused ? "Resume" : "Pause"} ${vision.name}`}
        onClick={() => void pauseVision(vision.id, !vision.paused).catch(onError)}>
        {vision.paused ? <PlayIcon /> : <PauseIcon />}{vision.paused ? "Resume" : "Pause"}
      </button>
    </div>
  </div>;
}

/** The saved profile inside a listed Vision. */
function profileOf(vision: VisionInfo): Profile {
  const { id, name, role, shape, color, accessory, model, soul, paused, revision } = vision;
  return { id, name, role, shape, color, accessory, model, soul, paused, revision };
}

export function VisionProfile({ visionId, routines, spentToday }: { visionId: string; routines?: RoutineControls; spentToday?: number }) {
  const { state, dispatch } = useApp();
  const [error, setError] = useState("");
  const settings = useVisionsSettings();
  const vision = state.visions.list.find(each => each.id === visionId);
  if (!vision) return null;
  const admin = vision.id === ADMIN_VISION_ID;
  const channel = ledChannel(state, vision.id);
  const shown = (reason: unknown) => setError(describeError(reason));
  const save: Save = async change => {
    setError("");
    try {
      const saved = await saveVision({ ...profileOf(vision), ...change }, vision.revision);
      dispatch({ type: "visions/loaded", visions: state.visions.list.map(each => each.id === saved.id ? { ...each, ...saved } : each) });
    } catch (reason) { shown(reason); throw reason; }
  };
  const guarded: Save = change => save(change).catch(() => undefined);
  return (
    <section className="themis-profile" aria-label={`${vision.name}, profile`}>
      {channel && <button type="button" className="themis-profile-back" onClick={() => dispatch({ type: "visions/panel", panel: null })}>
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M15 6l-6 6 6 6" /></svg># {channel.name}
      </button>}
      <ProfileHero vision={vision} channel={channel} onError={shown} />
      {error && <p role="alert" className="themis-form-error">{error}</p>}
      <div className="themis-profile-grid">
        <div className="themis-profile-column">
          <section className="themis-profile-block"><h2 className="themis-visions-label">Soul</h2><SoulField key={vision.revision} soul={vision.soul} save={save} /></section>
          <section className="themis-profile-block"><h2 className="themis-visions-label">Model</h2><ModelField vision={vision} save={guarded} /></section>
          {channel && !admin && <ChannelLimits channel={channel} settings={settings} spentToday={spentToday} onError={shown} />}
        </div>
        {channel && !admin && <div className="themis-profile-column">
          <Team channel={channel} />
          {routines && <Routines {...routines} />}
        </div>}
      </div>
    </section>
  );
}
