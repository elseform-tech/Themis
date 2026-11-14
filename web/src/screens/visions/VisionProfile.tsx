// A Vision's profile (canvas VisionProfile): Pause, Delete, its model, an autonomy override that
// may only lower Settings › Visions, its daily budget, its soul and the channels it works in.
// Atlas, the built-in admin, has no Delete and shows only what applies to it: it drives no chats
// (no autonomy), its runs do not read a soul, and it works only in `# hq`.
import { useEffect, useState } from "react";
import { listGoModels, listModelProviders } from "../../lib/tauri";
import type { GoModel, ModelProvider } from "../../lib/types";
import { ADMIN_VISION_ID, deleteVision, getVisionsSettings, pauseVision, saveVision, type Autonomy, type VisionInfo, type VisionProfile as Profile, type VisionsSettings } from "../../lib/visions";
import { describeError, useApp } from "../../state/store";
import { visionMoods } from "../../state/visions";
import { ModelOptions } from "../ModelsSettings";
import { AUTONOMY_STOPS, LEVELS, rankOf } from "./autonomy";
import { CloudIcon } from "./CloudIcon";
import { DeleteButton, EditIcon, PanelBlock, PanelButton, PauseIcon, PlayIcon, SidePanel } from "./SidePanel";

type Save = (change: Partial<Profile>) => Promise<void>;

/** The most a Vision's own daily budget may be, as for all Visions together. */
const MAX_BUDGET = 100;

/** The levels a profile may choose: those below Settings' own (any preset under Custom, whose
 *  rows still bound it). */
function lowerLevels(settings: Autonomy): Autonomy[] {
  const rank = rankOf(settings);
  return AUTONOMY_STOPS.filter(level => level !== "custom" && (rank === null || (rankOf(level) ?? 0) < rank));
}

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

/** The Vision's own model, or the chat model. */
function ModelRow({ vision, save }: { vision: VisionInfo; save: Save }) {
  const { models, providerName } = useModels();
  const model = vision.model ?? "";
  const known = model === "" || models.some(entry => entry.id === model);
  return <label className="themis-visions-kv">Model
    <select aria-label="Model" value={model} onChange={event => void save({ model: event.target.value || undefined })}>
      <option value="">Default model</option>
      {!known && <option value={model}>{model} (unavailable)</option>}
      <ModelOptions models={models} providerName={providerName} />
    </select>
  </label>;
}

/** Settings' level, or a lower one; a level saved before Settings went lower still shows. */
function AutonomyRow({ vision, settings, save }: { vision: VisionInfo; settings: VisionsSettings; save: Save }) {
  const lower = lowerLevels(settings.autonomy);
  const kept = vision.autonomy && !lower.includes(vision.autonomy) ? [vision.autonomy] : [];
  return <label className="themis-visions-kv">Autonomy
    <select aria-label="Autonomy" value={vision.autonomy ?? ""} onChange={event => void save({ autonomy: (event.target.value || undefined) as Autonomy | undefined })}>
      <option value="">Same as Settings ({LEVELS[settings.autonomy]})</option>
      {[...kept, ...lower].map(level => <option key={level} value={level}>{LEVELS[level]}</option>)}
    </select>
  </label>;
}

/** The Vision's own daily cap in dollars, $0 to $100 (0 stops it); empty is none (the cap for
 *  all Visions still holds). */
function BudgetRow({ vision, save, onError }: { vision: VisionInfo; save: Save; onError: (message: string) => void }) {
  const [budget, setBudget] = useState<string | null>(null);
  const saveBudget = () => {
    if (budget === null) return;
    setBudget(null);
    const dollars = budget.trim() === "" ? undefined : Number(budget);
    if (dollars !== undefined && (!Number.isFinite(dollars) || dollars < 0 || dollars > MAX_BUDGET)) {
      onError("Enter an amount from $0 to $100");
      return;
    }
    if (dollars !== vision.daily_budget_usd) void save({ daily_budget_usd: dollars });
  };
  return <label className="themis-visions-kv">Daily budget
    <span className="themis-visions-dollars">$
      <input type="number" min={0} max={MAX_BUDGET} step={0.5} aria-label="Daily budget" placeholder="Not set" value={budget ?? (vision.daily_budget_usd ?? "").toString()}
        onChange={event => setBudget(event.target.value)} onBlur={saveBudget} onKeyDown={event => { if (event.key === "Enter") event.currentTarget.blur(); }} />
    </span>
  </label>;
}

/** The soul, read as text; the pencil edits it in place. */
function SoulBlock({ soul, save }: { soul: string; save: Save }) {
  const [draft, setDraft] = useState<string | null>(null);
  const edit = <PanelButton label="Edit soul" onClick={() => setDraft(soul)}><EditIcon /></PanelButton>;
  return <PanelBlock title="Soul" action={draft === null ? edit : undefined}>
    {draft === null
      ? soul && <p className="themis-visions-soul">{soul}</p>
      : <form className="themis-visions-soul-form" onSubmit={event => { event.preventDefault(); void save({ soul: draft }).then(() => setDraft(null), () => undefined); }}>
        <textarea aria-label="Soul" rows={6} maxLength={4096} value={draft} autoFocus onChange={event => setDraft(event.target.value)} />
        <span className="themis-visions-soul-actions">
          <button type="button" className="themis-text-action" onClick={() => setDraft(null)}>Cancel</button>
          <button type="submit" className="themis-text-action">Save</button>
        </span>
      </form>}
  </PanelBlock>;
}

/** The channels the Vision is a member of, each opening its channel. */
function ChannelsBlock({ visionId }: { visionId: string }) {
  const { state, dispatch } = useApp();
  const channels = (state.visions.channels[state.activeProjectRoot ?? ""] ?? []).filter(channel => channel.members.includes(visionId));
  if (channels.length === 0) return null;
  return <PanelBlock title="Channels">
    <div className="themis-visions-pills" role="group" aria-label="Channels">
      {channels.map(channel => <button key={channel.id} type="button" className="themis-mention" onClick={() => dispatch({ type: "visions/open", channelId: channel.id })}># {channel.name}</button>)}
    </div>
  </PanelBlock>;
}

/** Pause or Resume, and Delete (none for Atlas). */
function ProfileActions({ vision, onPause, onDelete }: { vision: VisionInfo; onPause: () => void; onDelete: (() => Promise<void>) | null }) {
  return <>
    <PanelButton label={`${vision.paused ? "Resume" : "Pause"} ${vision.name}`} onClick={onPause}>{vision.paused ? <PlayIcon /> : <PauseIcon />}</PanelButton>
    {onDelete && <DeleteButton name={vision.name} consequence="Its work chats become ordinary chats, and it leaves every channel." onDelete={onDelete} />}
  </>;
}

/** Settings › Visions as last read, read again when runtime.jsonc changes. */
function useVisionsSettings(changes: number): VisionsSettings | null {
  const [settings, setSettings] = useState<VisionsSettings | null>(null);
  useEffect(() => { void getVisionsSettings().then(view => setSettings(view.settings)).catch(() => setSettings(null)); }, [changes]);
  return settings;
}

/** The Vision's cloud in its mood, its name and its role. */
function ProfileTitle({ vision }: { vision: VisionInfo }) {
  const { state } = useApp();
  return <>
    <CloudIcon shape={vision.shape} color={vision.color} mood={visionMoods(state)[vision.id] ?? "calm"} size={40} live crown={vision.id === ADMIN_VISION_ID} />
    <div className="themis-visions-panel-title"><div>{vision.name}</div>{vision.role && <div className="themis-visions-meta">{vision.role}</div>}</div>
  </>;
}

/** The saved profile inside a listed Vision. */
function profileOf(vision: VisionInfo): Profile {
  const { id, name, role, shape, color, model, autonomy, custom, daily_budget_usd, soul, paused, revision } = vision;
  return { id, name, role, shape, color, model, autonomy, custom, daily_budget_usd, soul, paused, revision };
}

export function VisionProfile({ visionId }: { visionId: string }) {
  const { state, dispatch } = useApp();
  const settings = useVisionsSettings(state.settingsChanges.runtime);
  const [error, setError] = useState("");
  const vision = state.visions.list.find(each => each.id === visionId);
  if (!vision) return null;
  const admin = vision.id === ADMIN_VISION_ID;
  const save: Save = async change => {
    setError("");
    try {
      const saved = await saveVision({ ...profileOf(vision), ...change }, vision.revision);
      dispatch({ type: "visions/loaded", visions: state.visions.list.map(each => each.id === saved.id ? { ...each, ...saved } : each) });
    } catch (reason) { setError(describeError(reason)); throw reason; }
  };
  const pause = () => void pauseVision(vision.id, !vision.paused).catch(reason => setError(describeError(reason)));
  const remove = async () => {
    await deleteVision(vision.id);
    dispatch({ type: "visions/panel", panel: null });
    dispatch({ type: "visions/loaded", visions: state.visions.list.filter(each => each.id !== vision.id) });
  };
  const guarded: Save = change => save(change).catch(() => undefined);
  return (
    <SidePanel label={`${vision.name}, profile`} error={error}
      title={<ProfileTitle vision={vision} />}
      actions={<ProfileActions vision={vision} onPause={pause} onDelete={admin ? null : remove} />}>
      <PanelBlock>
        <ModelRow vision={vision} save={guarded} />
        {!admin && settings && <AutonomyRow vision={vision} settings={settings} save={guarded} />}
        <BudgetRow vision={vision} save={guarded} onError={setError} />
      </PanelBlock>
      {!admin && <><SoulBlock soul={vision.soul} save={save} /><ChannelsBlock visionId={vision.id} /></>}
    </SidePanel>
  );
}
