// The rail's Visions entry: a cloud whose face is the most urgent mood of every Vision, with a
// badge counting what waits on the person. It hops once for each new thing that needs them.
import { useEffect, useRef, useState } from "react";
import { Badge, Tooltip } from "../../components";
import { useApp } from "../../state/store";
import { needsCount, railMood, visionMoods, type Mood } from "../../state/visions";
import { RailCloud } from "./CloudIcon";

/** What the rail cloud says aloud. */
export function railLabel(mood: Mood, needs: number): string {
  if (mood === "needs") return `Visions, ${needs} ${needs === 1 ? "needs" : "need"} you`;
  const said: Record<Exclude<Mood, "needs">, string> = { sorry: ", a chat failed", work: ", working", think: ", thinking", calm: "", sleep: ", all asleep" };
  return `Visions${said[mood]}`;
}

/** A new element each time the count grows, so the hop plays once per new item. */
function useHop(count: number): { key: number; hop: boolean } {
  const seen = useRef(count);
  const [hop, setHop] = useState({ key: 0, hop: false });
  useEffect(() => {
    if (count > seen.current) setHop(current => ({ key: current.key + 1, hop: true }));
    seen.current = count;
  }, [count]);
  return hop;
}

export function VisionsButton() {
  const { state, dispatch } = useApp();
  const needs = needsCount(state);
  const mood = railMood(Object.values(visionMoods(state)));
  const hop = useHop(needs);
  const active = state.mainView === "visions";
  return (
    <Tooltip content="Visions">
      <button type="button" className={`themis-sidebar-row themis-visions-button ${active ? "themis-sidebar-row--active" : ""}`} aria-label={railLabel(mood, needs)} aria-current={active ? "page" : undefined} onClick={() => dispatch({ type: "ui/view", view: "visions" })}>
        <RailCloud key={hop.key} mood={mood} hop={hop.hop} />
        {needs > 0 && <span aria-hidden="true"><Badge tone="warning">{needs}</Badge></span>}
      </button>
    </Tooltip>
  );
}
