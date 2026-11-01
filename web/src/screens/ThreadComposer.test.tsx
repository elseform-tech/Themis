// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup } from "@testing-library/react";
import samples from "../lib/secretSamples.json";
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
  const props={thread,draft,composerDisabled,readOnly:false,running,stopping:false,sendError:undefined,secretConfigured:true,onDraftChange:vi.fn(),onSend,onStop,onNewThread:vi.fn(),onOpenSettings:vi.fn(),skills:[{id:"review",name:"Review",description:"Review",plugin:"review"}],plugins:[{id:"g--drive",name:"Drive",description:"Drive",origin:"Global",enabled:true}]};
  return {...render(<ThreadComposer {...props} />),onStop,onSend,props};
}

describe("ThreadComposer secret warning", () => {
  const key = samples.secrets[4].join("");
  const warning = /This looks like a secret\./;

  it("says so above the composer and still lets the person send", () => {
    const view = composer(false, `use ${key}`);
    expect(screen.getByRole("status")).toHaveTextContent("This looks like a secret. It'd be sent to the model provider. If it's real, use Settings › Secrets, and rotate it if already shared.");
    const send = screen.getByRole("button", { name: "Send" });
    expect(send).toBeEnabled();
    fireEvent.click(send);
    expect(view.onSend).toHaveBeenCalledTimes(1);
  });

  it("stays quiet for ordinary text, code and hashes", () => {
    for (const text of samples.ordinary) {
      const view = composer(false, text);
      expect(screen.queryByText(warning)).toBeNull();
      view.unmount();
    }
  });

  it("dismisses once for this text and comes back for a new secret", () => {
    const view = composer(false, `use ${key}`);
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByText(warning)).toBeNull();
    view.rerender(<ThreadComposer {...view.props} draft={`use ${key} please`} />);
    expect(screen.queryByText(warning)).toBeNull();
    view.rerender(<ThreadComposer {...view.props} draft="something else" />);
    expect(screen.queryByText(warning)).toBeNull();
    view.rerender(<ThreadComposer {...view.props} draft={`again ${key}`} />);
    expect(screen.getByText(warning)).toBeInTheDocument();
  });

  it("never shows or logs the value", () => {
    const logged = (["log", "info", "warn", "error", "debug"] as const).map(method => vi.spyOn(console, method));
    composer(false, `use ${key}`);
    expect(screen.getByRole("status").textContent).not.toContain(key);
    for (const spy of logged) expect(spy).not.toHaveBeenCalled();
  });
});

describe("ThreadComposer", () => {
  it("keeps preparation stoppable without showing a stale connect hint", () => {
    const view = composer(true);
    view.rerender(<ThreadComposer {...view.props} secretConfigured={false} />);
    expect(screen.queryByText(/Connect OpenCode Go/)).toBeNull();
    fireEvent.click(screen.getByRole("button", {name: "Stop"}));
    expect(view.onStop).toHaveBeenCalledOnce();
    view.rerender(<ThreadComposer {...view.props} running={false} secretConfigured={false} />);
    expect(screen.getByText(/Connect OpenCode Go/)).toBeInTheDocument();
  });
  it("explains a rejected key and does not send until it is fixed", () => {
    const view = composer(false, "Build the chess app");
    view.rerender(<ThreadComposer {...view.props} keyRejected="OPENCODE_KEY" />);
    expect(screen.getByRole("alert")).toHaveTextContent("OpenCode Go rejected the key in OPENCODE_KEY.");
    expect(screen.getByRole("button", {name: "Send"})).toBeDisabled();
    fireEvent.click(screen.getByRole("button", {name: "Model settings"}));
    expect(view.props.onOpenSettings).toHaveBeenCalledOnce();
    expect(view.onSend).not.toHaveBeenCalled();
    view.rerender(<ThreadComposer {...view.props} keyRejected={undefined} />);
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByRole("button", {name: "Send"})).toBeEnabled();
  });
  it("says plainly that YOLO has no approvals and no sandbox, and marks it as a warning",()=>{
    const view=composer(false);
    view.rerender(<ThreadComposer {...view.props} thread={{...thread,approval_mode:"yolo"}} />);
    expect(screen.getByText("YOLO",{selector:"summary span"})).toHaveClass("is-yolo");
    expect(screen.getByText(/No approvals, no sandbox: the agent can change anything on this computer\. Only Themis settings are protected\./)).toBeInTheDocument();
    view.rerender(<ThreadComposer {...view.props} thread={{...thread,approval_mode:"yolo"}} yoloKeeps={["network blocked","2 deny rules"]} />);
    expect(screen.getByText(/Only Themis settings are protected; still applies: network blocked, 2 deny rules\./)).toBeInTheDocument();
    view.rerender(<ThreadComposer {...view.props} />);
    expect(screen.getByText("Custom",{selector:"summary span"})).not.toHaveClass("is-yolo");
  });
  it("shows only YOLO and Custom and permits next-run changes while running",()=>{
    const view=composer(true),onApprovalModeChange=vi.fn();
    view.rerender(<ThreadComposer {...view.props} onApprovalModeChange={onApprovalModeChange} />);
    expect(screen.getByText("Custom",{selector:"summary span"})).toBeInTheDocument();
    const select=screen.getByRole("combobox",{name:"Approval mode",hidden:true});
    expect(Array.from(select.querySelectorAll("option")).map(option=>option.textContent)).toEqual(["YOLO","Custom"]);
    expect(select).toBeEnabled();fireEvent.change(select,{target:{value:"yolo"}});
    expect(onApprovalModeChange).toHaveBeenCalledWith("yolo");
    expect(screen.queryByRole("button", {name:"Edit configuration"})).toBeNull();
    expect(screen.getByText(/Changes apply to the next run/)).toBeInTheDocument();
  });
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
  it("requires consecutive Escape presses within 1.5s and ignores held-key repeats",()=>{
    const now=vi.spyOn(Date,"now").mockReturnValue(1000);
    const {onStop}=composer(true);
    fireEvent.keyDown(window,{key:"Escape"});
    fireEvent.keyDown(window,{key:"Escape",repeat:true});
    expect(onStop).not.toHaveBeenCalled();
    now.mockReturnValue(2600);
    fireEvent.keyDown(window,{key:"Escape"});
    expect(onStop).not.toHaveBeenCalled();
    fireEvent.keyDown(window,{key:"ArrowDown"});
    fireEvent.keyDown(window,{key:"Escape"});
    expect(onStop).not.toHaveBeenCalled();
    fireEvent.keyDown(window,{key:"Escape"});
    expect(onStop).toHaveBeenCalledTimes(1);
  });
  it("shows a hint after the first Escape and hides it when the window passes",()=>{
    vi.useFakeTimers();
    try {
      composer(true);
      expect(screen.queryByText("Press Esc again to stop")).toBeNull();
      act(()=>{fireEvent.keyDown(window,{key:"Escape"});});
      expect(screen.getByText("Press Esc again to stop")).toHaveAttribute("role","status");
      act(()=>{vi.advanceTimersByTime(1600);});
      expect(screen.queryByText("Press Esc again to stop")).toBeNull();
    } finally { vi.useRealTimers(); }
  });
  it("never counts an Escape that closes a menu or panel toward stopping",()=>{
    const {onStop}=composer(true);
    const panel=document.createElement("div");panel.setAttribute("role","dialog");document.body.appendChild(panel);
    fireEvent.keyDown(window,{key:"Escape"});
    panel.remove();
    fireEvent.keyDown(window,{key:"Escape"});
    expect(onStop).not.toHaveBeenCalled();
    expect(screen.getByText("Press Esc again to stop")).toBeInTheDocument();
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
  it.each(["@","/"])("dismisses %s autocomplete first, then Escape twice stops",value=>{
    const {onStop,onSend}=composer(true,value,false);
    const input=screen.getByRole("textbox",{name:"Message"});
    const range=document.createRange();range.selectNodeContents(input);range.collapse(false);
    window.getSelection()!.removeAllRanges();window.getSelection()!.addRange(range);
    fireEvent.focus(input);expect(screen.getByRole("listbox")).toBeInTheDocument();
    fireEvent.keyDown(input,{key:"Escape"});
    expect(screen.queryByRole("listbox")).toBeNull();expect(onStop).not.toHaveBeenCalled();
    // The Escape that closed the list was for the list; two more stop the run.
    fireEvent.keyDown(input,{key:"Escape"});
    expect(onStop).not.toHaveBeenCalled();
    fireEvent.keyDown(input,{key:"Escape"});
    expect(onStop).toHaveBeenCalledTimes(1);expect(onSend).not.toHaveBeenCalled();
  });
  it("sends on Enter but preserves newlines and IME composition", () => {
    const onSend = vi.fn();
    render(
      <ThreadComposer
        thread={thread}
        draft="hello"
        composerDisabled={false}
        readOnly={false}
        running={false}
        stopping={false}
        sendError={undefined}
        secretConfigured
        onDraftChange={vi.fn()}
        onSend={onSend}
        onStop={vi.fn()}
        onNewThread={vi.fn()}
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

describe("composer focus and shortcuts", () => {
  it("never lets typed letters change the approval mode or reach the shield", () => {
    const onApprovalModeChange = vi.fn();
    const view = composer(false);
    view.rerender(<ThreadComposer {...view.props} onApprovalModeChange={onApprovalModeChange} />);
    const select = screen.getByRole("combobox", { name: "Approval mode", hidden: true });
    for (const key of ["y", "c", "Y", "C"]) expect(fireEvent.keyDown(select, { key })).toBe(false);
    // Other keys, IME composition and modified keys keep their native behaviour.
    expect(fireEvent.keyDown(select, { key: "o" })).toBe(true);
    expect(fireEvent.keyDown(select, { key: "Process" })).toBe(true);
    expect(fireEvent.keyDown(select, { key: "y", isComposing: true })).toBe(true);
    expect(fireEvent.keyDown(select, { key: "c", metaKey: true })).toBe(true);
    expect(onApprovalModeChange).not.toHaveBeenCalled();
    // Arrow keys and Space stay available for explicit keyboard use.
    expect(fireEvent.keyDown(select, { key: "ArrowDown" })).toBe(true);
    expect(fireEvent.keyDown(select, { key: " " })).toBe(true);
  });
  it("does not move focus to the shield on a mouse press, but still toggles it on click", () => {
    composer(false);
    const summary = screen.getByText("Custom", { selector: "summary span" }).closest("summary")!;
    expect(fireEvent.mouseDown(summary)).toBe(false);
    const details = summary.closest("details")!;
    expect(details.open).toBe(false);
    fireEvent.click(summary);
    expect(details.open).toBe(true);
    fireEvent.click(summary);
    expect(details.open).toBe(false);
  });
  it("leaves focus to the approval and question boxes stacked on top", () => {
    // Seen live: a click on the approval box focused the text area, so 1–3 typed there.
    const { props } = composer(false);
    cleanup();
    render(<ThreadComposer {...props} top={<>
      <section className="themis-approval-box" tabIndex={-1}><p>Approval needed</p></section>
      <section className="themis-question"><p>Which one?</p></section>
    </>} />);
    const editor = screen.getByRole("textbox", { name: "Message" });
    fireEvent.click(screen.getByText("Approval needed"));
    expect(document.activeElement).not.toBe(editor);
    fireEvent.click(screen.getByText("Which one?"));
    expect(document.activeElement).not.toBe(editor);
  });
  it("focuses the text area when the composer box is clicked outside a control", () => {
    composer(false);
    const editor = screen.getByRole("textbox", { name: "Message" });
    const box = editor.closest(".themis-thread-composer")!;
    expect(document.activeElement).not.toBe(editor);
    fireEvent.click(box);
    expect(document.activeElement).toBe(editor);
    (editor as HTMLElement).blur();
    const summary = screen.getByText("Custom", { selector: "summary span" }).closest("summary")!;
    fireEvent.click(summary);
    fireEvent.click(summary.closest("details")!.querySelector(".themis-permissions-panel p")!);
    expect(document.activeElement).not.toBe(editor);
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(document.activeElement).not.toBe(editor);
  });
});
