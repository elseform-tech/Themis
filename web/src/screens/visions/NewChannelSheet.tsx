// New channel: a name, a topic and the Visions that work there. The channel starts in the
// person's default approval mode (changed later in channel details, by the person only).
import { useState, type FormEvent } from "react";
import { Button, Dialog, Input } from "../../components";
import { ADMIN_VISION_ID, saveChannel } from "../../lib/visions";
import { describeError, useApp } from "../../state/store";
import { CloudIcon } from "./CloudIcon";
import { NewVisionSheet } from "./NewVisionSheet";

export type Sheet = "channel" | "vision" | null;

/** The two ways to add something in Visions, shared by the channels bar and `# hq`. */
export function VisionsSheets({ sheet, onClose }: { sheet: Sheet; onClose: () => void }) {
  return <>
    {sheet === "channel" && <NewChannelSheet onClose={onClose} />}
    {sheet === "vision" && <NewVisionSheet onClose={onClose} />}
  </>;
}

/** A channel name as the store takes it: lowercase letters, digits and dashes. */
function channelName(text: string): string {
  return text.toLowerCase().replace(/[\s_]+/g, "-").replace(/[^a-z0-9-]/g, "").slice(0, 40);
}

export function NewChannelSheet({ onClose }: { onClose: () => void }) {
  const { state, dispatch } = useApp();
  const [name, setName] = useState("");
  const [topic, setTopic] = useState("");
  const [members, setMembers] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const root = state.activeProjectRoot;
  // The admin works only in # hq.
  const choices = state.visions.list.filter(vision => vision.id !== ADMIN_VISION_ID);
  const toggle = (id: string) => setMembers(current => current.includes(id) ? current.filter(each => each !== id) : [...current, id]);
  async function create(event: FormEvent) {
    event.preventDefault();
    if (!root || !name || busy) return;
    setBusy(true); setError("");
    try {
      const channel = await saveChannel({ name, topic: topic.trim(), project_root: root, approval_mode: state.settings.default_approval_mode ?? "custom", members }, null);
      dispatch({ type: "visions/loaded", projectRoot: root, channels: [...(state.visions.channels[root] ?? []).filter(each => each.id !== channel.id), channel] });
      dispatch({ type: "visions/open", channelId: channel.id });
      onClose();
    } catch (reason) { setError(describeError(reason)); } finally { setBusy(false); }
  }
  return (
    <Dialog open title="New channel" onClose={() => { if (!busy) onClose(); }}>
      <form className="themis-sidebar-dialog themis-visions-sheet" onSubmit={event => void create(event)}>
        <Input id="visions-channel-name" label="Channel name" placeholder="release" value={name} maxLength={40} disabled={busy} onChange={event => { setName(channelName(event.target.value)); setError(""); }} />
        <Input id="visions-channel-topic" label="Topic" value={topic} maxLength={200} disabled={busy} onChange={event => setTopic(event.target.value)} />
        {choices.length > 0 && <fieldset className="themis-visions-picks">
          <legend className="themis-input-label">Members</legend>
          <div>{choices.map(vision => (
            <button key={vision.id} type="button" className="themis-visions-pick" aria-pressed={members.includes(vision.id)} onClick={() => toggle(vision.id)}>
              <CloudIcon shape={vision.shape} color={vision.color} mood="calm" size={22} /><span>{vision.name}</span>
            </button>
          ))}</div>
        </fieldset>}
        {error && <p role="alert" className="themis-form-error">{error}</p>}
        <div className="themis-sidebar-dialog-actions"><Button variant="ghost" disabled={busy} onClick={onClose}>Cancel</Button><Button type="submit" disabled={busy || !name || !root}>{busy ? "Creating…" : "Create"}</Button></div>
      </form>
    </Dialog>
  );
}
