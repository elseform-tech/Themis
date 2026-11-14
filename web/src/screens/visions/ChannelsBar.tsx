// The channels bar (board TeamChannel): in Visions mode it takes the chat list's place under the
// project switcher. Channels: `# administration` pinned first, then the project's channels, each
// with its chief's small cloud and what waits on the person there. There is no list of Visions:
// a chief is reached from its channel's header, a worker from its chip.
import { useState } from "react";
import { ADMINISTRATION, type ChannelInfo } from "../../lib/visions";
import { useApp } from "../../state/store";
import { channelNeeds } from "../../state/visions";
import { CharacterAvatar } from "./CharacterAvatar";
import { lookOf } from "./cast";
import { projectChannels } from "./channels";
import { NewChannelSheet } from "./NewChannelSheet";

function ChannelRow({ channel, active, onOpen }: { channel: ChannelInfo; active: boolean; onOpen: (id: string | null) => void }) {
  const { state } = useApp();
  const needs = channelNeeds(channel);
  const chief = state.visions.list.find(vision => vision.id === channel.chief);
  const label = needs > 0 ? `# ${channel.name}, ${needs} ${needs === 1 ? "needs" : "need"} you` : `# ${channel.name}`;
  return (
    <button type="button" className={`themis-sidebar-row themis-channel-row ${active ? "themis-sidebar-row--active" : ""} ${needs > 0 ? "is-waiting" : ""}`} aria-label={label} aria-current={active ? "page" : undefined} onClick={() => onOpen(channel.id || null)}>
      {chief ? <CharacterAvatar look={lookOf(chief)} /> : <span className="themis-channel-hash" aria-hidden="true">#</span>}
      <span className="themis-sidebar-name">{channel.name}</span>
      {needs > 0 && <span className="themis-thread-tag" aria-hidden="true">{needs}</span>}
    </button>
  );
}

/** What the bar shows under the project switcher (Sidebar keeps the switcher and the frame). */
export function ChannelsBar() {
  const { state, dispatch } = useApp();
  const [making, setMaking] = useState(false);
  const { hq, channels, current } = projectChannels(state);
  const openChannel = (channelId: string | null) => dispatch({ type: "visions/open", channelId });
  return <>
    <div className="themis-sidebar-projects themis-channels-bar">
      <h2 className="themis-sidebar-section">Channels
        <button type="button" className="themis-visions-add" aria-label="New channel" onClick={() => setMaking(true)}>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14" /></svg>
        </button>
      </h2>
      <ul className="themis-sidebar-list">
        {[hq, ...channels].map(channel => <li key={channel.id || ADMINISTRATION}><ChannelRow channel={channel} active={current === channel} onOpen={openChannel} /></li>)}
      </ul>
    </div>
    {making && <NewChannelSheet onClose={() => setMaking(false)} />}
  </>;
}
