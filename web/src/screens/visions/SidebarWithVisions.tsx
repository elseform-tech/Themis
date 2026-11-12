// The sidebar with Visions: the rail gains the Visions cloud, a Vision's chats are marked with its
// cloud, and in Visions mode the channels bar takes the chat list's place under the same project
// switcher.
import { useApp } from "../../state/store";
import { Sidebar } from "../Sidebar";
import { ChannelsBar } from "./ChannelsBar";
import { VisionMark } from "./VisionMark";
import { VisionsButton } from "./VisionsButton";

export function SidebarWithVisions({ collapsed }: { collapsed: boolean }) {
  const { state, dispatch } = useApp();
  const panel = state.mainView === "visions"
    ? { label: "Visions navigation", content: <ChannelsBar />, onProject: (root: string) => dispatch({ type: "visions/project", root }) }
    : undefined;
  return <Sidebar collapsed={collapsed} railExtra={<VisionsButton />} panel={panel} rowMark={thread => <VisionMark thread={thread} />} />;
}
