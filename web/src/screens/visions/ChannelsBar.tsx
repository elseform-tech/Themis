// The channels bar: in Visions mode it takes the chat list's place. The project switcher, `# hq`,
// the project's channels (each with what waits on the person there) and the Visions with their
// live clouds and moods.
import { useState } from "react";
import { MovingText } from "../../components/MovingText";
import { useApp } from "../../state/store";
import { channelNeeds, MOOD_LABEL, visionMoods } from "../../state/visions";
import { SidebarIcon } from "../Sidebar";
import { CloudIcon } from "./CloudIcon";
import { VisionsSheets, type Sheet } from "./NewChannelSheet";

function Plus() {
  return <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14" /></svg>;
}

/** Projects to switch between; Visions stays open on the new project's `# hq`. */
function ProjectSwitcher() {
  const { state, dispatch } = useApp();
  const current = state.projects.find(project => project.root === state.activeProjectRoot);
  function choose(root: string, menu: HTMLDetailsElement | null) {
    menu?.removeAttribute("open");
    dispatch({ type: "project/selected", root });
    dispatch({ type: "ui/view", view: "visions" });
    dispatch({ type: "visions/open", channelId: null });
  }
  return (
    <details className="themis-project-switcher" data-popover>
      <summary aria-label={`Project: ${current?.name ?? "none"}`} title={current?.root}>
        <SidebarIcon name="project" /><MovingText>{current?.name ?? "Choose a project"}</MovingText>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true"><path d="m6 9 6 6 6-6" /></svg>
      </summary>
      <div className="themis-project-menu" role="group" aria-label="Projects">
        {state.projects.map(project => (
          <button key={project.root} type="button" className="themis-project-option" title={project.root} aria-current={project.root === current?.root ? "true" : undefined}
            onClick={event => choose(project.root, event.currentTarget.closest("details"))}>
            <MovingText>{project.name}</MovingText>
          </button>
        ))}
      </div>
    </details>
  );
}

function SectionHead({ title, add, onAdd }: { title: string; add: string; onAdd: () => void }) {
  return <h2 className="themis-sidebar-section">{title}<button type="button" className="themis-visions-add" aria-label={add} onClick={onAdd}><Plus /></button></h2>;
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

export function ChannelsBar({ collapsed }: { collapsed: boolean }) {
  const { state, dispatch } = useApp();
  const [sheet, setSheet] = useState<Sheet>(null);
  const root = state.activeProjectRoot;
  const channels = root ? state.visions.channels[root] ?? [] : [];
  const shown = state.visions.open.channelId;
  const current = channels.some(channel => channel.id === shown) ? shown : null;
  const moods = visionMoods(state);
  const openChannel = (channelId: string | null) => dispatch({ type: "visions/open", channelId });
  return <>
    <aside id="themis-sidebar" className={`themis-sidebar themis-channels-bar ${collapsed ? "" : "is-visible"}`} inert={collapsed} aria-hidden={collapsed} aria-label="Visions navigation">
      <div className="themis-sidebar-top"><ProjectSwitcher /></div>
      <div className="themis-sidebar-projects">
        <ChannelRow id={null} name="hq" needs={0} active={current === null} onOpen={openChannel} />
        <SectionHead title="Channels" add="New channel" onAdd={() => setSheet("channel")} />
        <ul className="themis-sidebar-list">
          {channels.map(channel => <li key={channel.id}><ChannelRow id={channel.id} name={channel.name} needs={channelNeeds(state, channel.id)} active={current === channel.id} onOpen={openChannel} /></li>)}
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
    </aside>
    <VisionsSheets sheet={sheet} onClose={() => setSheet(null)} />
  </>;
}
