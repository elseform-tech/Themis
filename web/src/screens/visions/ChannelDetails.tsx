// A channel's details (canvas VisionChannel): its topic, the approval mode its Visions' chats run
// in (the person's to set; a Vision never changes it), its members (Visions only) and Delete. A
// member opens the chief's profile, or a worker's thread with the chief; New Vision makes one by hand.
// `# administration` is built in: Atlas is its only member and it cannot be deleted.
import { useState } from "react";
import { ADMIN_VISION_ID, deleteChannel, type ApprovalModeName, type ChannelInfo, ADMINISTRATION } from "../../lib/visions";
import { describeError, useApp } from "../../state/store";
import { visionMoods } from "../../state/visions";
import { CharacterAvatar } from "./CharacterAvatar";
import { lookOf } from "./cast";
import { saveChannelChange } from "./channels";
import { NewVisionSheet } from "./NewVisionSheet";
import { CloseIcon, DeleteButton, PanelBlock, PanelButton, PlusIcon, SidePanel } from "./SidePanel";

const MODES: Array<[ApprovalModeName, string]> = [["custom", "Custom"], ["auto", "Auto"], ["yolo", "YOLO"]];

/** New Vision: makes one by hand, from the channel's members. */
function NewVisionButton() {
  const [making, setMaking] = useState(false);
  return <>
    <PanelButton label="New Vision" onClick={() => setMaking(true)}><PlusIcon /></PanelButton>
    {making && <NewVisionSheet onClose={() => setMaking(false)} />}
  </>;
}

/** The channel's Visions, each opening the chief's profile or a worker's thread; others can be
 *  added, and any removed. */
function Members({ channel, save }: { channel: ChannelInfo; save: (change: Partial<ChannelInfo>) => void }) {
  const { state, dispatch } = useApp();
  const moods = visionMoods(state);
  const hq = channel.name === ADMINISTRATION;
  const members = channel.members.flatMap(id => state.visions.list.find(vision => vision.id === id) ?? []);
  // The admin works only in # administration.
  const addable = state.visions.list.filter(vision => vision.id !== ADMIN_VISION_ID && !channel.members.includes(vision.id));
  const open = (id: string) => dispatch({ type: "visions/panel", panel: { kind: id === channel.chief ? "vision" : "worker", visionId: id } });
  return <PanelBlock title="Members" action={hq ? undefined : <NewVisionButton />}>
    <ul className="themis-visions-members">
      {members.map(vision => <li key={vision.id}>
        <button type="button" className="themis-visions-member" aria-label={vision.name} onClick={() => open(vision.id)}>
          <CharacterAvatar look={lookOf(vision)} mood={moods[vision.id] ?? "calm"} size={20} />
          <span className="themis-visions-member-name">{vision.name}</span>
          {vision.role && <small>{vision.role}</small>}
        </button>
        {!hq && <PanelButton label={`Remove ${vision.name}`} onClick={() => save({ members: channel.members.filter(id => id !== vision.id) })}><CloseIcon /></PanelButton>}
      </li>)}
    </ul>
    {!hq && addable.length > 0 && <select className="themis-visions-field" aria-label="Add a member" value="" onChange={event => { if (event.target.value) save({ members: [...channel.members, event.target.value] }); }}>
      <option value="">Add a member</option>
      {addable.map(vision => <option key={vision.id} value={vision.id}>{vision.name}</option>)}
    </select>}
  </PanelBlock>;
}

export function ChannelDetails({ channel }: { channel: ChannelInfo }) {
  const { state, dispatch } = useApp();
  const [topic, setTopic] = useState<string | null>(null);
  const [error, setError] = useState("");
  const hq = channel.name === ADMINISTRATION;
  const root = channel.project_root;
  const listed = () => state.visions.channels[root] ?? [];
  async function save(change: Partial<ChannelInfo>) {
    setError("");
    try { await saveChannelChange(state, dispatch, channel, change); } catch (reason) { setError(describeError(reason)); }
  }
  async function remove() {
    await deleteChannel(channel.id);
    dispatch({ type: "visions/panel", panel: null });
    dispatch({ type: "visions/loaded", projectRoot: root, channels: listed().filter(each => each.id !== channel.id) });
    dispatch({ type: "visions/open", channelId: null });
  }
  const saveTopic = () => {
    if (topic === null) return;
    setTopic(null);
    if (topic.trim() !== channel.topic) void save({ topic: topic.trim() });
  };
  return (
    <SidePanel label="Channel details" error={error}
      title={<span className="themis-visions-panel-title"># {channel.name}</span>}
      actions={!hq && <DeleteButton name={`# ${channel.name}`} consequence="Its Visions' chats stop and become ordinary chats." onDelete={remove} />}>
      <PanelBlock title="Topic">
        <input type="text" className="themis-visions-field" aria-label="Topic" maxLength={500} value={topic ?? channel.topic}
          onChange={event => setTopic(event.target.value)} onBlur={saveTopic} onKeyDown={event => { if (event.key === "Enter") event.currentTarget.blur(); }} />
      </PanelBlock>
      {!hq && <PanelBlock title="Approval mode">
        <select className="themis-visions-field" aria-label="Approval mode" value={channel.approval_mode} onChange={event => void save({ approval_mode: event.target.value as ApprovalModeName })}>
          {MODES.map(([mode, label]) => <option key={mode} value={mode}>{label}</option>)}
        </select>
      </PanelBlock>}
      <Members channel={channel} save={change => void save(change)} />
    </SidePanel>
  );
}
