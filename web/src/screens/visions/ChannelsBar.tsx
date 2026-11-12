// The channels bar: in Visions mode it takes the chat list's place under the project switcher: `# hq`,
// the project's channels (each with what waits on the person there) and the Visions with their
// live clouds and moods.
import { useState } from "react";
import { useApp } from "../../state/store";
import { channelNeeds, MOOD_LABEL, visionMoods } from "../../state/visions";
import { CloudIcon } from "./CloudIcon";
import { projectChannels } from "./channels";
import { VisionsSheets, type Sheet } from "./NewChannelSheet";

function SectionHead({ title, add, onAdd }: { title: string; add: string; onAdd: () => void }) {
  return (
    <h2 className="themis-sidebar-section">{title}
      <button type="button" className="themis-visions-add" aria-label={add} onClick={onAdd}>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14" /></svg>
      </button>
    </h2>
  );
}

function ChannelRow({ id, name, needs, active, onOpen }: { id: string | null; name: string; needs: number; active: boolean; onOpen: (id: string | null) => void }) {
  const label = needs > 0 ? `# ${name}, ${needs} ${needs === 1 ? "needs" : "need"} you` : `# ${name}`;
  return (
    <button type="button" className={`themis-sidebar-row themis-channel-row ${active ? "themis-sidebar-row--active" : ""} ${needs > 0 ? "is-waiting" : ""}`} aria-label={label} aria-current={active ? "page" : undefined} onClick={() => onOpen(id)}>
      <span className="themis-channel-hash" aria-hidden="true">#</span><span className="themis-sidebar-name">{name}</span>
      {needs > 0 && <span className="themis-thread-tag" aria-hidden="true">{needs}</span>}
    </button>
  );
}

/** What the bar shows under the project switcher (Sidebar keeps the switcher and the frame). */
export function ChannelsBar() {
  const { state, dispatch } = useApp();
  const [sheet, setSheet] = useState<Sheet>(null);
  const { hq, channels, current } = projectChannels(state);
  const moods = visionMoods(state);
  const openChannel = (channelId: string | null) => dispatch({ type: "visions/open", channelId });
  return <>
    <div className="themis-sidebar-projects themis-channels-bar">
        <ChannelRow id={hq.id || null} name="hq" needs={channelNeeds(hq)} active={current === hq} onOpen={openChannel} />
        <SectionHead title="Channels" add="New channel" onAdd={() => setSheet("channel")} />
        <ul className="themis-sidebar-list">
          {channels.map(channel => <li key={channel.id}><ChannelRow id={channel.id} name={channel.name} needs={channelNeeds(channel)} active={current === channel} onOpen={openChannel} /></li>)}
        </ul>
        <SectionHead title="Visions" add="New Vision" onAdd={() => setSheet("vision")} />
        <ul className="themis-sidebar-list">
          {state.visions.list.map(vision => {
            const mood = moods[vision.id] ?? "calm";
            return <li key={vision.id} className="themis-sidebar-row themis-vision-row">
              <CloudIcon shape={vision.shape} color={vision.color} mood={mood} live />
              <span className="themis-sidebar-name">{vision.name}</span>
              {MOOD_LABEL[mood] && <span className={`themis-thread-tag themis-mood-tag--${mood}`}>{MOOD_LABEL[mood]}</span>}
            </li>;
          })}
        </ul>
    </div>
    <VisionsSheets sheet={sheet} onClose={() => setSheet(null)} />
  </>;
}
