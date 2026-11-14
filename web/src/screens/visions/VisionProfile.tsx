// A Vision's profile: Pause, Delete, its model, its soul when it leads a channel, and the channels
// it works in (autonomy and budget are its channel's). Atlas, the built-in admin, has no Delete
// and works only in `# administration`; it has a soul.
import { useEffect, useState } from "react";
import { listGoModels, listModelProviders } from "../../lib/tauri";
import type { GoModel, ModelProvider } from "../../lib/types";
import { ADMIN_VISION_ID, deleteVision, pauseVision, saveVision, type VisionInfo, type VisionProfile as Profile } from "../../lib/visions";
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

/** The soul, read as text; the pencil edits it in place. */
function SoulBlock({ soul, save }: { soul: string; save: Save }) {
  const [draft, setDraft] = useState<string | null>(null);
  const edit = <PanelButton label="Edit soul" onClick={() => setDraft(soul)}><EditIcon /></PanelButton>;
  return <PanelBlock title="Soul" action={draft === null ? edit : undefined}>
    {draft === null
      ? soul && <p className="themis-visions-soul">{soul}</p>
      : <form className="themis-visions-soul-form" onSubmit={event => { event.preventDefault(); void save({ soul: draft }).then(() => setDraft(null), () => undefined); }}>
        <textarea aria-label="Soul" rows={6} maxLength={16384} value={draft} autoFocus onChange={event => setDraft(event.target.value)} />
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
  const { id, name, role, shape, color, accessory, model, soul, paused, revision } = vision;
  return { id, name, role, shape, color, accessory, model, soul, paused, revision };
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
      {(admin || chief) && <SoulBlock soul={vision.soul} save={save} />}
      {!admin && <ChannelsBlock visionId={vision.id} />}
    </SidePanel>
  );
}
