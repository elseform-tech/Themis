// The sidebar with Visions: the rail gains the Visions cloud, and in Visions mode the channels bar
// takes the chat list's place.
import { useApp } from "../../state/store";
import { Sidebar } from "../Sidebar";
import { ChannelsBar } from "./ChannelsBar";
import { VisionsButton } from "./VisionsButton";

export function SidebarWithVisions({ collapsed }: { collapsed: boolean }) {
  const { state } = useApp();
  const panel = state.mainView === "visions" ? <ChannelsBar collapsed={collapsed} /> : undefined;
  return <Sidebar collapsed={collapsed} railExtra={<VisionsButton />} panel={panel} />;
}
