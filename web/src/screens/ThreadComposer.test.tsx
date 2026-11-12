// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup } from "@testing-library/react";
import samples from "../lib/secretSamples.json";
import type { ThreadInfo } from "../lib/types";
import { ThreadComposer } from "./ThreadComposer";
import { usePopoverDismissal } from "../lib/dismiss";
import type { ComponentProps } from "react";

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
  const props={thread,draft,composerDisabled,running,stopping:false,sendError:undefined,secretConfigured:true,onDraftChange:vi.fn(),onSend,onStop,onNewThread:vi.fn(),onOpenSettings:vi.fn(),skills:[{id:"review",name:"Review",description:"Review",plugin:"review"}],plugins:[{id:"g--drive",name:"Drive",description:"Drive",origin:"Global",enabled:true}]};
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
  it("waits for an open question box: no typing or sending, a quiet placeholder, and Stop still works", () => {
    const view = composer(true, "", true);
    view.rerender(<ThreadComposer {...view.props} locked />);
    const input = screen.getByRole("textbox", {name: "Message"});
    expect(input).toHaveAttribute("aria-disabled", "true");
    expect(input).toHaveAttribute("contenteditable", "false");
    expect(input).toHaveAttribute("data-placeholder", "Answer the question above to continue");
    fireEvent.click(screen.getByRole("button", {name: "Stop"}));
    expect(view.onStop).toHaveBeenCalledOnce();
    // Not running: Send is there but disabled.
    view.rerender(<ThreadComposer {...view.props} running={false} draft="hi" locked />);
    expect(screen.getByRole("button", {name: "Send"})).toBeDisabled();
    // The box moved into its turn: the composer unlocks.
    view.rerender(<ThreadComposer {...view.props} running={false} composerDisabled={false} draft="hi" locked={false} />);
    expect(screen.getByRole("button", {name: "Send"})).toBeEnabled();
    expect(screen.getByRole("textbox", {name: "Message"})).toHaveAttribute("data-placeholder", "Ask Themis… @ for plugins, / for skills");
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
  it("shows the mode on the shield with its own icon and colour, and a one-line summary per mode",()=>{
    const view=composer(false);
    view.rerender(<ThreadComposer {...view.props} thread={{...thread,approval_mode:"yolo"}} />);
    expect(screen.getByLabelText("Permissions: YOLO").closest("details")).toHaveAttribute("data-mode","yolo");
    expect(screen.getByText("Unrestricted, including network; destructive actions require approval.")).toBeInTheDocument();
    expect(screen.queryByText(/Still applies/)).toBeNull();
    view.rerender(<ThreadComposer {...view.props} thread={{...thread,approval_mode:"yolo"}} yoloKeeps={["Keychain off","2 deny rules"]} />);
    expect(screen.getByText("Still applies: Keychain off, 2 deny rules")).toBeInTheDocument();
    // A blocked network is said once, under the list, in any mode.
    expect(screen.queryByText("Network blocked by your configuration")).toBeNull();
    view.rerender(<ThreadComposer {...view.props} networkBlocked />);
    expect(screen.getByText("Network blocked by your configuration")).toBeInTheDocument();
    view.rerender(<ThreadComposer {...view.props} thread={{...thread,approval_mode:"auto"}} />);
    expect(screen.getByLabelText("Permissions: Auto").closest("details")).toHaveAttribute("data-mode","auto");
    expect(screen.getByText("Actions are reviewed automatically; destructive actions require approval.")).toBeInTheDocument();
    view.rerender(<ThreadComposer {...view.props} />);
    expect(screen.getByLabelText("Permissions: Custom").closest("details")).toHaveAttribute("data-mode","custom");
    expect(screen.getByText("Changes and network access require approval.")).toBeInTheDocument();
    // Three distinct icons: Auto's shield has a check, YOLO is a bolt, Custom a plain shield.
    const icons=screen.getAllByRole("radio",{hidden:true}).map(radio=>radio.querySelector("svg")!.innerHTML);
    expect(new Set(icons).size).toBe(3);
  });
  it("shows only Auto, Custom and YOLO and permits next-run changes while running",()=>{
    const view=composer(true),onApprovalModeChange=vi.fn();
    view.rerender(<ThreadComposer {...view.props} onApprovalModeChange={onApprovalModeChange} />);
    expect(screen.getByText("Custom",{selector:"summary span"})).toBeInTheDocument();
    const group=screen.getByRole("radiogroup",{name:"Approval mode",hidden:true});
    const radios=screen.getAllByRole("radio",{hidden:true});
    expect(radios.map(radio=>radio.querySelector("strong")!.textContent)).toEqual(["Auto","Custom","YOLO"]);
    expect(radios.map(radio=>radio.getAttribute("aria-checked"))).toEqual(["false","true","false"]);
    expect(group).not.toHaveAttribute("aria-disabled");
    fireEvent.click(screen.getByRole("radio",{name:/^YOLO/,hidden:true}));
    expect(onApprovalModeChange).toHaveBeenCalledWith("yolo");
    fireEvent.click(screen.getByRole("radio",{name:/^Auto/,hidden:true}));
    expect(onApprovalModeChange).toHaveBeenCalledWith("auto");
    // Choosing the current mode again saves nothing.
    fireEvent.click(screen.getByRole("radio",{name:/^Custom/,hidden:true}));
    expect(onApprovalModeChange).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("button", {name:"Edit configuration"})).toBeNull();
    expect(screen.getByText("Changes apply to the next run.")).toBeInTheDocument();
    view.rerender(<ThreadComposer {...view.props} running={false} />);
    expect(screen.queryByText("Changes apply to the next run.")).toBeNull();
  });
  it("chooses with the arrow keys and ignores input while a change is saving",()=>{
    const view=composer(false),onApprovalModeChange=vi.fn();
    view.rerender(<ThreadComposer {...view.props} onApprovalModeChange={onApprovalModeChange} />);
    const custom=screen.getByRole("radio",{name:/^Custom/,hidden:true});
    expect(custom).toHaveAttribute("tabindex","0");
    expect(screen.getByRole("radio",{name:/^YOLO/,hidden:true})).toHaveAttribute("tabindex","-1");
    fireEvent.keyDown(custom,{key:"ArrowDown"});
    expect(onApprovalModeChange).toHaveBeenLastCalledWith("yolo");
    expect(document.activeElement).toBe(screen.getByRole("radio",{name:/^YOLO/,hidden:true}));
    fireEvent.keyDown(custom,{key:"ArrowUp"});
    expect(onApprovalModeChange).toHaveBeenLastCalledWith("auto");
    view.rerender(<ThreadComposer {...view.props} onApprovalModeChange={onApprovalModeChange} thread={{...thread,approval_mode:"yolo"}} />);
    fireEvent.keyDown(screen.getByRole("radio",{name:/^YOLO/,hidden:true}),{key:"ArrowRight"});
    expect(onApprovalModeChange).toHaveBeenLastCalledWith("auto");
    onApprovalModeChange.mockClear();
    view.rerender(<ThreadComposer {...view.props} onApprovalModeChange={onApprovalModeChange} permissionSaving />);
    expect(screen.getByRole("radiogroup",{name:"Approval mode",hidden:true})).toHaveAttribute("aria-disabled","true");
    expect(screen.getByText("Saving…",{selector:"summary span"})).toBeInTheDocument();
    fireEvent.click(screen.getByRole("radio",{name:/^YOLO/,hidden:true}));
    fireEvent.keyDown(screen.getByRole("radio",{name:/^Custom/,hidden:true}),{key:"ArrowDown"});
    expect(onApprovalModeChange).not.toHaveBeenCalled();
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
    const group = screen.getByRole("radiogroup", { name: "Approval mode", hidden: true });
    const custom = screen.getByRole("radio", { name: /^Custom/, hidden: true });
    for (const key of ["y", "c", "a", "Y", "C", "A", "o"]) {
      fireEvent.keyDown(custom, { key });
      fireEvent.keyDown(group, { key });
    }
    expect(fireEvent.keyDown(custom, { key: "y", isComposing: true })).toBe(true);
    expect(onApprovalModeChange).not.toHaveBeenCalled();
    expect(custom).toHaveAttribute("aria-checked", "true");
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
    // Seen live: a click on the approval box focused the text area.
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
    fireEvent.click(summary.closest("details")!.querySelector(".themis-permissions-panel")!);
    expect(document.activeElement).not.toBe(editor);
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(document.activeElement).not.toBe(editor);
  });
});

describe("approval mode menu closes", () => {
  // Seen live: the menu stayed open after a pick and on Esc, and the next click meant for the
  // text box landed on it and switched the chat to YOLO.
  // The app closes `details[data-popover]` menus on a press outside or an Esc elsewhere.
  function InApp(props: ComponentProps<typeof ThreadComposer>) {
    usePopoverDismissal();
    return <ThreadComposer {...props} />;
  }
  function openShield() {
    const onApprovalModeChange = vi.fn();
    const view = composer(false);
    view.rerender(<InApp {...view.props} onApprovalModeChange={onApprovalModeChange} />);
    const summary = screen.getByText("Custom", { selector: "summary span" }).closest("summary")!;
    fireEvent.click(summary);
    const details = summary.closest("details")!;
    expect(details.open).toBe(true);
    return { onApprovalModeChange, summary, details };
  }
  it("closes when a mode is picked", () => {
    const { onApprovalModeChange, summary, details } = openShield();
    const auto = screen.getByRole("radio", { name: /^Auto/, hidden: true });
    auto.focus();
    fireEvent.click(auto);
    expect(onApprovalModeChange).toHaveBeenCalledWith("auto");
    expect(details.open).toBe(false);
    // Focus does not fall to the page once the list hides.
    expect(document.activeElement).toBe(summary);
    // Picking the current mode closes it too.
    fireEvent.click(summary);
    fireEvent.click(screen.getByRole("radio", { name: /^Custom/, hidden: true }));
    expect(details.open).toBe(false);
    expect(onApprovalModeChange).toHaveBeenCalledTimes(1);
  });
  it("closes on Esc and puts focus back on the shield", () => {
    const { onApprovalModeChange, summary, details } = openShield();
    const custom = screen.getByRole("radio", { name: /^Custom/, hidden: true });
    custom.focus();
    fireEvent.keyDown(custom, { key: "Escape" });
    expect(details.open).toBe(false);
    expect(document.activeElement).toBe(summary);
    fireEvent.click(summary);
    fireEvent.keyDown(document, { key: "Escape" });
    expect(details.open).toBe(false);
    expect(onApprovalModeChange).not.toHaveBeenCalled();
  });
  it("closes on a click outside without changing the mode", () => {
    const { onApprovalModeChange, summary, details } = openShield();
    const editor = screen.getByRole("textbox", { name: "Message" });
    fireEvent.pointerDown(editor);
    fireEvent.click(editor);
    expect(details.open).toBe(false);
    expect(onApprovalModeChange).not.toHaveBeenCalled();
    // A press inside the open list is not outside.
    fireEvent.click(summary);
    fireEvent.pointerDown(screen.getByRole("radiogroup", { name: "Approval mode", hidden: true }));
    expect(details.open).toBe(true);
  });
});

describe("ThreadComposer layout (canvas: input, then one row of controls)", () => {
  it("puts + · Auto · branch · context · send in one row under the input, with no project line", () => {
    const onSwitchBranch = vi.fn();
    const props = { thread, draft: "", composerDisabled: false, running: false, stopping: false, sendError: undefined, secretConfigured: true, onDraftChange: vi.fn(), onSend: vi.fn(), onStop: vi.fn(), onNewThread: vi.fn(), onOpenSettings: vi.fn(), skills: [], plugins: [],
      location: { name: "ledger-api", root: "/p/ledger-api", branch: "codex/date-rollover", branches: ["main", "codex/date-rollover"], onSwitchBranch } };
    const { container } = render(<ThreadComposer {...props} />);
    const box = container.querySelector(".themis-thread-composer")!;
    expect(box.querySelector(".themis-composer-location")).toBeNull();
    expect(screen.queryByText("ledger-api")).toBeNull();
    const row = box.querySelector(".themis-thread-composer-actions")!;
    const picker = screen.getByLabelText("Branch codex/date-rollover. Switch branch");
    expect(row).toContainElement(picker);
    // In order: +, the shield, the branch, then (after the flexible gap) send.
    const order = [box.querySelector(".themis-composer-add"), box.querySelector(".themis-composer-permissions"), picker.closest(".themis-branch-picker"), screen.getByRole("button", { name: "Send" })];
    for (let i = 1; i < order.length; i++) expect(order[i - 1]!.compareDocumentPosition(order[i]!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // The project and its path stay on the branch's tooltip; switching works as before.
    expect(picker.closest("summary")).toHaveAttribute("title", "Switch branch · ledger-api\n/p/ledger-api");
    fireEvent.click(screen.getByRole("menuitem", { name: "main", hidden: true }));
    expect(onSwitchBranch).toHaveBeenCalledWith("main");
  });

  it("names the shield Auto in the row and hides labels only in an extremely narrow composer", async () => {
    const { readFileSync } = await import("node:fs");
    const css = readFileSync("src/screens/ThreadView.css", "utf8");
    expect(css).toMatch(/@container themis-composer \(max-width: 260px\)\s*\{\s*\.themis-thread-composer \.themis-composer-label\s*\{\s*display:\s*none;/);
    expect(css).not.toMatch(/themis-composer-location/);
  });

  // A Workspace beside the chat leaves a composer about 280px wide (canvas Workspace board). What gives way, in order:
  // the meter's percent text (its ring and tooltip stay), then the gaps, then a branch name past ten characters
  // (shorter names such as main or feature-x show whole); the branch's icon alone is the last resort.
  it("gives way in a narrow composer in order: meter text, then gaps, then a long branch name", async () => {
    const { readFileSync } = await import("node:fs");
    const css = readFileSync("src/screens/ThreadView.css", "utf8");
    const rule = (selector: string) => { const at = css.indexOf(`\n${selector} {`); expect(at, selector).toBeGreaterThanOrEqual(0); return css.slice(at, css.indexOf("}", at)); };
    const step = (width: number, body: RegExp) => expect(css, `step at ${width}px`).toMatch(new RegExp(`@container themis-composer \\(max-width: ${width}px\\)\\s*\\{\\s*${body.source}`));
    step(420, /\.themis-thread-composer \.themis-context-value\s*\{\s*display:\s*none;/);
    step(380, /\.themis-thread-composer-actions\s*\{\s*gap:\s*8px;/);
    step(260, /\.themis-thread-composer \.themis-composer-label\s*\{\s*display:\s*none;/);
    // +, Auto, the meter and send never shrink; the branch shrinks only to its content's floor.
    expect(rule(".themis-thread-composer-actions > *")).toContain("flex: none");
    expect(rule(".themis-thread-composer-actions > .themis-branch-picker, .themis-thread-composer-actions > .themis-composer-branch-static")).toMatch(/flex: 0 1 auto;[^}]*min-width: auto/);
    expect(rule(".themis-composer-branch")).toMatch(/min-width: 10ch;[^}]*overflow: hidden;[^}]*text-overflow: ellipsis/);
    expect(rule(".themis-composer-branch.is-short")).toContain("min-width: auto");
  });

  it("keeps a short branch name whole and lets only a long one truncate", () => {
    const props = { thread, draft: "", composerDisabled: false, running: false, stopping: false, sendError: undefined, secretConfigured: true, onDraftChange: vi.fn(), onSend: vi.fn(), onStop: vi.fn(), onNewThread: vi.fn(), onOpenSettings: vi.fn(), skills: [], plugins: [] };
    const view = render(<ThreadComposer {...props} location={{ name: "p", root: "/p", branch: "main", branches: ["main", "dev"], onSwitchBranch: vi.fn() }} />);
    expect(screen.getByText("main")).toHaveClass("is-short");
    view.rerender(<ThreadComposer {...props} location={{ name: "p", root: "/p", branch: "codex/date-rollover", branches: ["main", "codex/date-rollover"], onSwitchBranch: vi.fn() }} />);
    expect(screen.getByText("codex/date-rollover")).not.toHaveClass("is-short");
  });
});
