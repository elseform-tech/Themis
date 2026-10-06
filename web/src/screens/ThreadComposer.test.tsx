// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup } from "@testing-library/react";
import type { ThreadInfo } from "../lib/types";
import { ThreadComposer } from "./ThreadComposer";

const thread: ThreadInfo = {
  id: "thread-1",
  title: "Test thread",
  provider: "go",
  model: "gpt-5",
  running: false,
  worktree_path: null,
  branch: null,
  base_branch: null,
  recovered: false,
  skill_ids: [],
};

afterEach(()=>{cleanup();vi.restoreAllMocks();});
function composer(running:boolean,draft="",composerDisabled=running) {
  const onStop=vi.fn(),onSend=vi.fn();
  const props={thread,draft,models:[],effortLevels:[],effort:"",composerDisabled,composerBusy:running,readOnly:false,running,stopping:false,switching:false,sendError:undefined,secretConfigured:true,onDraftChange:vi.fn(),onSend,onStop,onNewThread:vi.fn(),onProviderChange:vi.fn(),onEffortChange:vi.fn(),onOpenSettings:vi.fn(),skills:[{id:"review",name:"Review",description:"Review",plugin:"review"}],plugins:[{id:"g--drive",name:"Drive",description:"Drive",origin:"Global",enabled:true}]};
  return {...render(<ThreadComposer {...props} />),onStop,onSend,props};
}

describe("ThreadComposer", () => {
  it("attaches any file type, removes pending files and permits attachment-only sends", () => {
    const view = composer(false);
    const onAttach = vi.fn(), onRemoveAttachment = vi.fn();
    view.rerender(<ThreadComposer {...view.props}
      attachments={[{name:"music.mp3",path:"/project/.themis/attachments/music.mp3",size:12}]}
      onAttach={onAttach} onRemoveAttachment={onRemoveAttachment} />);
    fireEvent.click(screen.getByRole("button", {name:"Attach files"}));
    expect(onAttach).toHaveBeenCalledOnce();
    expect(screen.getByRole("list", {name:"Attached files"}).compareDocumentPosition(screen.getByRole("textbox", {name:"Message"})) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.getByRole("button", {name:"Send"})).toBeEnabled();
    fireEvent.click(screen.getByRole("button", {name:"Remove music.mp3"}));
    expect(onRemoveAttachment).toHaveBeenCalledWith("/project/.themis/attachments/music.mp3");
  });

  it("stops once on double Escape without requiring focus in the disabled running composer",()=>{
    const {onStop,onSend}=composer(true);
    fireEvent.keyDown(window,{key:"Escape"});
    expect(onStop).not.toHaveBeenCalled();
    fireEvent.keyDown(window,{key:"Escape"});
    fireEvent.keyDown(window,{key:"Escape"});
    fireEvent.keyDown(window,{key:"Escape"});
    expect(onStop).toHaveBeenCalledTimes(1);
    expect(onSend).not.toHaveBeenCalled();
  });
  it("requires consecutive Escape presses within 500ms and ignores held-key repeats",()=>{
    const now=vi.spyOn(Date,"now").mockReturnValue(1000);
    const {onStop}=composer(true);
    fireEvent.keyDown(window,{key:"Escape"});
    fireEvent.keyDown(window,{key:"Escape",repeat:true});
    expect(onStop).not.toHaveBeenCalled();
    now.mockReturnValue(1600);
    fireEvent.keyDown(window,{key:"Escape"});
    expect(onStop).not.toHaveBeenCalled();
    fireEvent.keyDown(window,{key:"ArrowDown"});
    fireEvent.keyDown(window,{key:"Escape"});
    expect(onStop).not.toHaveBeenCalled();
    fireEvent.keyDown(window,{key:"Escape"});
    expect(onStop).toHaveBeenCalledTimes(1);
  });
  it("does not stop an idle chat and removes the shortcut on unmount",()=>{
    const idle=composer(false);
    fireEvent.keyDown(window,{key:"Escape"});fireEvent.keyDown(window,{key:"Escape"});
    expect(idle.onStop).not.toHaveBeenCalled();idle.unmount();
    const running=composer(true);running.unmount();
    fireEvent.keyDown(window,{key:"Escape"});fireEvent.keyDown(window,{key:"Escape"});
    expect(running.onStop).not.toHaveBeenCalled();
  });
  it("keeps the shortcut across rerenders, uses the current stop action and disarms while stopping",()=>{
    const view=composer(true),latestStop=vi.fn();
    fireEvent.keyDown(window,{key:"Escape"});
    view.rerender(<ThreadComposer {...view.props} onStop={latestStop} />);
    fireEvent.keyDown(window,{key:"Escape"});
    expect(latestStop).toHaveBeenCalledTimes(1);expect(view.onStop).not.toHaveBeenCalled();
    view.rerender(<ThreadComposer {...view.props} onStop={latestStop} stopping />);
    fireEvent.keyDown(window,{key:"Escape"});fireEvent.keyDown(window,{key:"Escape"});
    expect(latestStop).toHaveBeenCalledTimes(1);
  });
  it.each(["@","/"])("dismisses %s autocomplete first and stops on the second Escape",value=>{
    const {onStop,onSend}=composer(true,value,false);
    const input=screen.getByRole("textbox",{name:"Message"});
    const range=document.createRange();range.selectNodeContents(input);range.collapse(false);
    window.getSelection()!.removeAllRanges();window.getSelection()!.addRange(range);
    fireEvent.focus(input);expect(screen.getByRole("listbox")).toBeInTheDocument();
    fireEvent.keyDown(input,{key:"Escape"});
    expect(screen.queryByRole("listbox")).toBeNull();expect(onStop).not.toHaveBeenCalled();
    fireEvent.keyDown(input,{key:"Escape"});
    expect(onStop).toHaveBeenCalledTimes(1);expect(onSend).not.toHaveBeenCalled();
  });
  it("sends on Enter but preserves newlines and IME composition", () => {
    const onSend = vi.fn();
    render(
      <ThreadComposer
        thread={thread}
        draft="hello"
        models={[]}
        effortLevels={["low", "medium", "high"]}
        effort=""
        composerDisabled={false}
        composerBusy={false}
        readOnly={false}
        running={false}
        stopping={false}
        switching={false}
        sendError={undefined}
        secretConfigured
        onDraftChange={vi.fn()}
        onSend={onSend}
        onStop={vi.fn()}
        onNewThread={vi.fn()}
        onProviderChange={vi.fn()}
        onEffortChange={vi.fn()}
        onOpenSettings={vi.fn()}
      />,
    );

    const input = screen.getByRole("textbox", { name: "Message" });
    fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
    fireEvent.keyDown(input, { key: "Enter", isComposing: true });
    expect(onSend).not.toHaveBeenCalled();

    fireEvent.keyDown(input, { key: "Enter" });
    expect(onSend).toHaveBeenCalledTimes(1);
  });
});
