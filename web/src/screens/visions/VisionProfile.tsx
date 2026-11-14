// A Vision's profile (canvas VisionProfile): Pause, Delete, its model, an autonomy override that
// may only lower Settings › Visions, its daily budget, its soul and the channels it works in.
// Atlas, the built-in admin, has no Delete and shows only what applies to it: it drives no chats
// (no autonomy), its runs do not read a soul, and it works only in `# administration`.
import { useEffect, useState } from "react";
import { listGoModels, listModelProviders } from "../../lib/tauri";
import type { GoModel, ModelProvider } from "../../lib/types";
import { ADMIN_VISION_ID, deleteVision, getSoul, openSoul, pauseVision, saveSoul, saveVision, type SoulView, type VisionInfo, type VisionProfile as Profile } from "../../lib/visions";
import { describeError, useApp } from "../../state/store";
import { visionMoods } from "../../state/visions";
import { ModelOptions } from "../ModelsSettings";
import { CharacterAvatar } from "./CharacterAvatar";
import { lookOf } from "./cast";
import { DeleteButton, EditIcon, PanelBlock, PanelButton, PauseIcon, PlayIcon, SidePanel } from "./SidePanel";

type Save = (change: Partial<Profile>) => Promise<void>;

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

/** The chief's or Atlas's `soul.md`, read as text: the pencil edits it in place, and "Open
 *  soul.md" opens the file in the person's editor for longer edits. */
function SoulBlock({ visionId, onError }: { visionId: string; onError: (message: string) => void }) {
  const [soul, setSoul] = useState<SoulView | null>(null);
  const [draft, setDraft] = useState<string | null>(null);
  useEffect(() => { void getSoul(visionId).then(setSoul).catch(reason => onError(describeError(reason))); }, [visionId, onError]);
  if (!soul) return null;
  const save = async (text: string) => {
    try {
      setSoul(await saveSoul(visionId, text, soul.revision));
      setDraft(null);
    } catch (reason) { onError(describeError(reason)); }
  };
  const actions = draft === null && <>
    <PanelButton label="Open soul.md" onClick={() => void openSoul(visionId).catch(reason => onError(describeError(reason)))}><OpenIcon /></PanelButton>
    <PanelButton label="Edit soul" onClick={() => setDraft(soul.text)}><EditIcon /></PanelButton>
  </>;
  return <PanelBlock title="Soul" action={actions || undefined}>
    {soul.problem && <p role="alert" className="themis-form-error">{soul.problem}</p>}
    {draft === null
      ? soul.text && <p className="themis-visions-soul">{soul.text}</p>
      : <form className="themis-visions-soul-form" onSubmit={event => { event.preventDefault(); void save(draft); }}>
        <textarea aria-label="Soul" rows={6} maxLength={16384} value={draft} autoFocus onChange={event => setDraft(event.target.value)} />
        <span className="themis-visions-soul-actions">
          <button type="button" className="themis-text-action" onClick={() => setDraft(null)}>Cancel</button>
          <button type="submit" className="themis-text-action">Save</button>
        </span>
      </form>}
  </PanelBlock>;
}

/** An arrow out of a box: opens a file elsewhere. */
function OpenIcon() {
  return <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M14 4h6v6M20 4l-9 9M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5" /></svg>;
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

/** The Vision's cloud in its mood, its name and its role. */
function ProfileTitle({ vision }: { vision: VisionInfo }) {
  const { state } = useApp();
  return <>
    <CharacterAvatar look={lookOf(vision)} mood={visionMoods(state)[vision.id] ?? "calm"} size={40} live />
    <div className="themis-visions-panel-title"><div>{vision.name}</div>{vision.role && <div className="themis-visions-meta">{vision.role}</div>}</div>
  </>;
}

/** The saved profile inside a listed Vision. */
function profileOf(vision: VisionInfo): Profile {
  const { id, name, role, shape, color, accessory, model, paused, revision } = vision;
  return { id, name, role, shape, color, accessory, model, paused, revision };
}

export function VisionProfile({ visionId }: { visionId: string }) {
  const { state, dispatch } = useApp();
  const [error, setError] = useState("");
  const vision = state.visions.list.find(each => each.id === visionId);
  if (!vision) return null;
  const admin = vision.id === ADMIN_VISION_ID;
  // Only a channel's chief of staff (and Atlas) has a soul; its workers have a role.
  const chief = Object.values(state.visions.channels).some(channels => channels.some(channel => channel.chief === vision.id));
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
      </PanelBlock>
      {(admin || chief) && <SoulBlock visionId={vision.id} onError={setError} />}
      {!admin && <ChannelsBlock visionId={vision.id} />}
    </SidePanel>
  );
}
