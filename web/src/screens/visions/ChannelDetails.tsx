// A channel's details (canvas VisionChannel): its topic, the approval mode its Visions' chats run
// in (the person's to set; a Vision never changes it), its daily budget (empty: Settings'), its members (Visions only) and Delete. A
// member opens the chief's profile, or a worker's thread with the chief; each can be removed from
// the channel or deleted. The Members block's + makes a new Vision by hand (the sidebar has no
// Visions list). `# administration` is built in: Atlas is its only member, and neither can be deleted.
import { useState } from "react";
import { ADMIN_VISION_ID, deleteChannel, deleteVision, type ApprovalModeName, type ChannelInfo, type VisionInfo, ADMINISTRATION } from "../../lib/visions";
import { describeError, useApp } from "../../state/store";
import { visionMoods } from "../../state/visions";
import { CharacterAvatar } from "./CharacterAvatar";
import { lookOf } from "./cast";
import { saveChannelChange } from "./channels";
import { NewVisionSheet } from "./NewVisionSheet";
import { CloseIcon, DeleteButton, PanelBlock, PanelButton, PlusIcon, SidePanel } from "./SidePanel";

const MODES: Array<[ApprovalModeName, string]> = [["custom", "Custom"], ["auto", "Auto"], ["yolo", "YOLO"]];

/** The channel's daily budget: empty goes back to Settings › Visions' budget, whose total still
 *  caps what all channels spend. */
function BudgetBlock({ channel, save, onError }: { channel: ChannelInfo; save: (change: Partial<ChannelInfo>) => void; onError: (message: string) => void }) {
  const [budget, setBudget] = useState<string | null>(null);
  const saveBudget = () => {
    if (budget === null) return;
    setBudget(null);
    const dollars = budget.trim() === "" ? null : Number(budget);
    if (dollars !== null && (!Number.isFinite(dollars) || dollars < 0)) { onError("Enter an amount in dollars, or leave it empty"); return; }
    if (dollars !== (channel.daily_budget_usd ?? null)) save({ daily_budget_usd: dollars });
  };
  return <PanelBlock title="Daily budget">
    <input type="number" className="themis-visions-field" aria-label="Daily budget" min={0} step={0.5} value={budget ?? String(channel.daily_budget_usd ?? "")}
      onChange={event => setBudget(event.target.value)} onBlur={saveBudget} onKeyDown={event => { if (event.key === "Enter") event.currentTarget.blur(); }} />
  </PanelBlock>;
}

/** New Vision: makes one by hand, from the channel's members. */
function NewVisionButton() {
  const [making, setMaking] = useState(false);
  return <>
    <PanelButton label="New Vision" onClick={() => setMaking(true)}><PlusIcon /></PanelButton>
    {making && <NewVisionSheet onClose={() => setMaking(false)} />}
  </>;
}

/** One member: it opens the chief's profile or a worker's thread; Remove takes it out of the
 *  channel (not in `# administration`), Delete deletes it after asking (never Atlas). */
function MemberRow({ channel, vision, save }: { channel: ChannelInfo; vision: VisionInfo; save: (change: Partial<ChannelInfo>) => void }) {
  const { state, dispatch } = useApp();
  const mood = visionMoods(state)[vision.id] ?? "calm";
  const hq = channel.name === ADMINISTRATION;
  const remove = async () => {
    await deleteVision(vision.id);
    const panel = state.visions.open.panel;
    if (panel && "visionId" in panel && panel.visionId === vision.id) dispatch({ type: "visions/panel", panel: { kind: "channel" } });
    dispatch({ type: "visions/loaded", visions: state.visions.list.filter(each => each.id !== vision.id) });
  };
  const open = () => dispatch({ type: "visions/panel", panel: { kind: vision.id === channel.chief ? "vision" : "worker", visionId: vision.id } });
  return <li>
    <button type="button" className="themis-visions-member" aria-label={vision.name} onClick={open}>
      <CharacterAvatar look={lookOf(vision)} mood={mood} size={20} />
      <span className="themis-visions-member-name">{vision.name}</span>
      {vision.role && <small>{vision.role}</small>}
    </button>
    {!hq && <PanelButton label={`Remove ${vision.name}`} onClick={() => save({ members: channel.members.filter(id => id !== vision.id) })}><CloseIcon /></PanelButton>}
    {vision.id !== ADMIN_VISION_ID && <DeleteButton name={vision.name} consequence="Its chats stop and become ordinary chats." onDelete={remove} />}
  </li>;
}

/** The channel's Visions; others can be added. */
function Members({ channel, save }: { channel: ChannelInfo; save: (change: Partial<ChannelInfo>) => void }) {
  const { state } = useApp();
  const hq = channel.name === ADMINISTRATION;
  const members = channel.members.flatMap(id => state.visions.list.find(vision => vision.id === id) ?? []);
  // The admin works only in # administration.
  const addable = state.visions.list.filter(vision => vision.id !== ADMIN_VISION_ID && !channel.members.includes(vision.id));
  return <PanelBlock title="Members" action={hq ? undefined : <NewVisionButton />}>
    <ul className="themis-visions-members">
      {members.map(vision => <MemberRow key={vision.id} channel={channel} vision={vision} save={save} />)}
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
      <BudgetBlock channel={channel} save={change => void save(change)} onError={setError} />
      <Members channel={channel} save={change => void save(change)} />
    </SidePanel>
  );
}
