// New Vision: a name, a cloud (shape and colour) and a role. Model, autonomy, budget and soul
// start from Settings and are changed in its profile.
import { useState, type FormEvent } from "react";
import { Button, Dialog, Input } from "../../components";
import { saveVision, type VisionInfo } from "../../lib/visions";
import { describeError, useApp } from "../../state/store";
import { CloudIcon, COLORS, SHAPES } from "./CloudIcon";

export function NewVisionSheet({ onClose }: { onClose: () => void }) {
  const { state, dispatch } = useApp();
  const [name, setName] = useState("");
  const [role, setRole] = useState("");
  const [shape, setShape] = useState("fluffy");
  const [color, setColor] = useState("indigo");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function create(event: FormEvent) {
    event.preventDefault();
    if (!name.trim() || busy) return;
    setBusy(true); setError("");
    try {
      const saved = await saveVision({ name: name.trim(), role: role.trim(), shape, color }, null);
      const listed: VisionInfo = { ...saved, over_budget: false, vision_chats: [], needs: 0, sorry: false };
      dispatch({ type: "visions/loaded", visions: [...state.visions.list.filter(each => each.id !== saved.id), listed] });
      onClose();
    } catch (reason) { setError(describeError(reason)); } finally { setBusy(false); }
  }
  return (
    <Dialog open title="New Vision" onClose={() => { if (!busy) onClose(); }}>
      <form className="themis-sidebar-dialog themis-visions-sheet" onSubmit={event => void create(event)}>
        <div className="themis-visions-sheet-name">
          <CloudIcon shape={shape} color={color} mood="calm" size={44} live />
          <Input id="visions-vision-name" label="Name" placeholder="Echo" value={name} maxLength={40} disabled={busy} onChange={event => { setName(event.target.value); setError(""); }} />
        </div>
        <div className="themis-visions-shapes" role="group" aria-label="Shape">
          {Object.entries(SHAPES).map(([id, { label }]) => (
            <button key={id} type="button" aria-label={label} aria-pressed={shape === id} onClick={() => setShape(id)}><CloudIcon shape={id} color={color} mood="calm" size={26} /></button>
          ))}
        </div>
        <div className="themis-visions-colors" role="group" aria-label="Colour">
          {Object.entries(COLORS).map(([id, label]) => (
            <button key={id} type="button" className={`themis-cloud--${id}`} aria-label={label} aria-pressed={color === id} onClick={() => setColor(id)} />
          ))}
        </div>
        <Input id="visions-vision-role" label="Role" placeholder="Release captain" value={role} maxLength={80} disabled={busy} onChange={event => setRole(event.target.value)} />
        {error && <p role="alert" className="themis-form-error">{error}</p>}
        <div className="themis-sidebar-dialog-actions"><Button variant="ghost" disabled={busy} onClick={onClose}>Cancel</Button><Button type="submit" disabled={busy || !name.trim()}>{busy ? "Creating…" : "Create"}</Button></div>
      </form>
    </Dialog>
  );
}
