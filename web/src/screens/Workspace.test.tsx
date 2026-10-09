// @vitest-environment jsdom
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_SETTINGS } from "../state/reducer";
import App from "../App";
import { open } from "@tauri-apps/plugin-dialog";
import * as bridge from "../lib/tauri";

vi.mock("@tauri-apps/plugin-dialog", () => ({open: vi.fn()}));
vi.mock("../lib/tauri", async importOriginal => ({
  ...await importOriginal<typeof import("../lib/tauri")>(),
  attachFiles: vi.fn(), attachmentFile: vi.fn(async (_id, path) => `asset://localhost${path}`),
  pluginAction: vi.fn(async () => []), listPromptSkills: vi.fn(async () => []),
  getDefaultProject: vi.fn(async () => ({ name: "Themis", root: "/tmp/fixed-themis", is_git: true, is_default: true })), getSettings: vi.fn(), getSecretStatus: vi.fn(), updateSettings: vi.fn(),
  onBackendResync: vi.fn(async () => () => {}), getPendingApprovals: vi.fn(async () => []),
  onThreadEvent: vi.fn(async () => () => {}), onApprovalRequest: vi.fn(async () => () => {}), onReviewItemAdded: vi.fn(async () => () => {}),
  listSkills: vi.fn(async () => []), listAutomations: vi.fn(async () => []), listReviewItems: vi.fn(async () => []),
  renameThread: vi.fn(), setThreadApprovalMode: vi.fn(), setProvider: vi.fn(), setThreadEffort: vi.fn(), getThread: vi.fn(), getThreadHistory: vi.fn(async () => []), sendMessage: vi.fn(async () => ({ run_id: "test-run" })),
  openProject: vi.fn(), listThreads: vi.fn(), createProject: vi.fn(), renameProject: vi.fn(), createThread: vi.fn(), listGoModels: vi.fn(async () => [{ id: "muse-spark-1.3-contributor", effort_levels: [] }, { id: "gpt-5.6-luna", effort_levels: ["low", "medium", "high"] }]),
}));
const settings = { ...DEFAULT_SETTINGS, projects_directory: "/tmp/Themis/Projects" };
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(bridge.getThreadHistory).mockResolvedValue([]);
  const storage = new Map<string, string>();
  vi.stubGlobal("localStorage", { getItem: (key: string) => storage.get(key) ?? null, setItem: (key: string, value: string) => storage.set(key, value), clear: () => storage.clear() });
  vi.mocked(bridge.getSettings).mockResolvedValue(settings);
  vi.mocked(bridge.listThreads).mockResolvedValue([]);
  vi.mocked(bridge.getSecretStatus).mockResolvedValue({ go: true });
  vi.mocked(bridge.updateSettings).mockImplementation(async patch => ({ ...settings, ...patch }));
  window.matchMedia = vi.fn().mockReturnValue({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() });
  HTMLElement.prototype.scrollIntoView = vi.fn();
});
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });
async function mount() { await act(async () => { render(<App />); }); }

describe("Workspace journey", () => {
  it("shows recent activity across projects and opens the matching chat", async () => {
    let receive!: Parameters<typeof bridge.onThreadEvent>[0];
    vi.mocked(bridge.onThreadEvent).mockImplementationOnce(async callback => { receive = callback; return () => {}; });
    const alpha = { name: "Alpha", root: "/tmp/alpha", is_git: true };
    const beta = { name: "Beta", root: "/tmp/beta", is_git: true };
    const older = { id: "older", title: "Earlier chat", provider: "go" as const, model: "test", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [], last_activity_seq: 10 };
    const newer = { ...older, id: "newer", title: "Latest chat", last_activity_seq: 20 };
    vi.mocked(bridge.getDefaultProject).mockResolvedValueOnce(alpha);
    vi.mocked(bridge.getSettings).mockResolvedValueOnce({ ...settings, recent_roots: [alpha.root, beta.root] });
    vi.mocked(bridge.openProject).mockImplementation(async root => root === alpha.root ? alpha : beta);
    vi.mocked(bridge.listThreads).mockImplementation(async root => root === alpha.root ? [older] : [newer]);
    vi.mocked(bridge.getThread).mockImplementation(async id => id === older.id ? older : newer);
    await mount();
    const recent = screen.getByRole("region", { name: "Recent" });
    expect(within(recent).queryByText("Beta")).toBeNull();
    expect(screen.getByRole("heading", { name: "Projects" }).compareDocumentPosition(screen.getByRole("heading", { name: "Recent" })) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(within(recent).getAllByRole("button").map(button => button.textContent)).toEqual([expect.stringContaining("Latest chat"), expect.stringContaining("Earlier chat")]);
    await act(async () => fireEvent.click(within(recent).getByRole("button", { name: /Latest chat/ })));
    expect(within(recent).getByRole("button", { name: /Latest chat/ })).toHaveAttribute("aria-current", "true");
    fireEvent.click(screen.getByRole("button", { name: "Collapse sidebar" }));
    expect(screen.queryByRole("region", { name: "Recent" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Show sidebar" }));
    expect(screen.getByRole("region", { name: "Recent" })).toBeInTheDocument();
    older.last_activity_seq = 30;
    await act(async () => receive({ thread_id: older.id, run_id: "latest-run", event: { kind: "finished", result: "Done" } }));
    expect(within(recent).getAllByRole("button")[0]).toHaveAccessibleName(/Earlier chat/);
  });

  it("resizes the sidebar and restores its saved width after remount", async () => {
    let mounted!: ReturnType<typeof render>;
    await act(async () => { mounted = render(<App />); });
    const handle = screen.getByRole("separator", { name: "Resize sidebar" });
    expect(handle).toHaveAttribute("aria-valuenow", "292");
    fireEvent.keyDown(handle, { key: "ArrowRight" });
    expect(handle).toHaveAttribute("aria-valuenow", "312");
    expect(localStorage.getItem("themis:sidebarWidth")).toBe("312");
    vi.stubGlobal("PointerEvent", MouseEvent);
    handle.setPointerCapture = vi.fn();
    handle.hasPointerCapture = vi.fn(() => true);
    handle.releasePointerCapture = vi.fn();
    vi.spyOn(handle.parentElement!, "getBoundingClientRect").mockReturnValue({ width: 312 } as DOMRect);
    fireEvent.pointerDown(handle, { pointerId: 1, button: 0, clientX: 300 });
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: 420 });
    expect(handle).toHaveAttribute("aria-valuenow", "432");
    fireEvent.pointerUp(handle, { pointerId: 1 });
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: 450 });
    expect(localStorage.getItem("themis:sidebarWidth")).toBe("432");
    mounted.unmount();
    await mount();
    const restored = screen.getByRole("separator", { name: "Resize sidebar" });
    expect(restored).toHaveAttribute("aria-valuenow", "432");
    fireEvent.keyDown(restored, { key: "End" });
    expect(restored).toHaveAttribute("aria-valuenow", "560");
    fireEvent.keyDown(restored, { key: "ArrowRight" });
    expect(restored).toHaveAttribute("aria-valuenow", "560");
    fireEvent.keyDown(restored, { key: "Home" });
    expect(restored).toHaveAttribute("aria-valuenow", "240");
  });

  it("restores an active approval when the app is refreshed", async () => {
    const thread = { id: "waiting", title: "Waiting for approval", provider: "go" as const, model: "test", running: true, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] };
    vi.mocked(bridge.listThreads).mockResolvedValue([thread]);
    vi.mocked(bridge.getThread).mockResolvedValue(thread);
    vi.mocked(bridge.getThreadHistory).mockResolvedValueOnce([{ kind: "event", envelope: { thread_id: thread.id, run_id: "active", event: { kind: "started", task: "Read skill", max_turns: 5 } } }]);
    vi.mocked(bridge.getPendingApprovals).mockResolvedValueOnce([{ thread_id: thread.id, approval_id: "pending", tool: "read_file", risk: "read", summary: "Read skill instructions" }]);
    await mount();
    expect(await screen.findByRole("dialog", { name: "Approval required" })).toHaveTextContent("Read skill instructions");
    expect(screen.queryByText(/Run interrupted/)).toBeNull();
  });

  it("restores a completed conversation when the backend reports an event gap", async () => {
    let recover!: () => void;
    vi.mocked(bridge.onBackendResync).mockImplementationOnce(async callback => { recover = callback; return () => {}; });
    const thread = { id: "recovery", title: "Recovery test", provider: "go" as const, model: "test", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] };
    vi.mocked(bridge.listThreads).mockResolvedValue([thread]);
    vi.mocked(bridge.getThread).mockResolvedValue(thread);
    await mount();
    vi.mocked(bridge.getThreadHistory).mockResolvedValueOnce([{ kind: "event", envelope: { thread_id: thread.id, run_id: "missed", event: { kind: "finished", result: "Recovered completion" } } }]);
    await act(async () => recover());
    fireEvent.click(screen.getByRole("button", { name: "Recovery test" }));
    expect(await screen.findByText("Recovered completion")).toBeInTheDocument();
  });

  it("permits YOLO selection and sending in a non-Git project",async()=>{
    const project={name:"NonGit QA",root:"/tmp/non-git-qa",is_git:false};
    const thread={id:"non-git-thread",title:"NonGit thread",provider:"go" as const,model:"test-model",running:false,worktree_path:null,branch:null,base_branch:null,recovered:false,skill_ids:[]};
    vi.mocked(bridge.getDefaultProject).mockResolvedValueOnce(project);
    vi.mocked(bridge.listThreads).mockResolvedValue([thread]);vi.mocked(bridge.getThread).mockResolvedValue(thread);
    vi.mocked(bridge.setThreadApprovalMode).mockResolvedValueOnce({...thread,approval_mode:"yolo"});
    await mount();await act(async()=>fireEvent.click(screen.getByRole("button",{name:/^NonGit thread/})));
    expect(screen.getByLabelText("Message")).toHaveAttribute("aria-disabled","true");
    fireEvent.click(screen.getByLabelText("Permissions: Custom"));
    expect(screen.getByRole("combobox",{name:"Approval mode"})).toBeEnabled();
    await act(async()=>fireEvent.change(screen.getByRole("combobox",{name:"Approval mode"}),{target:{value:"yolo"}}));
    expect(screen.getByLabelText("Message")).toHaveAttribute("aria-disabled","false");
    fireEvent.input(screen.getByLabelText("Message"),{target:{textContent:"Inspect project"}});
    expect(screen.getByRole("button",{name:"Send"})).toBeEnabled();
    await act(async()=>fireEvent.click(screen.getByRole("button",{name:"Send"})));
    expect(bridge.sendMessage).toHaveBeenCalledWith(thread.id,"Inspect project","",[]);
  });

  it("waits for permission persistence before permitting a send",async()=>{
    const thread={id:"permission-thread",title:"Permission test",provider:"go" as const,model:"test-model",running:false,worktree_path:null,branch:null,base_branch:null,recovered:false,skill_ids:[]};
    vi.mocked(bridge.listThreads).mockResolvedValue([thread]);vi.mocked(bridge.getThread).mockResolvedValue(thread);
    let saved!:(value:typeof thread)=>void;
    vi.mocked(bridge.setThreadApprovalMode).mockImplementationOnce(()=>new Promise(resolve=>{saved=resolve;}));
    await mount();await act(async()=>fireEvent.click(screen.getByRole("button",{name:/^Permission test/})));
    fireEvent.input(screen.getByLabelText("Message"),{target:{textContent:"Hello"}});
    fireEvent.click(screen.getByLabelText("Permissions: Custom"));
    fireEvent.change(screen.getByRole("combobox",{name:"Approval mode"}),{target:{value:"yolo"}});
    expect(screen.getByRole("combobox",{name:"Approval mode"})).toBeDisabled();
    expect(screen.getByRole("button",{name:"Send"})).toBeDisabled();
    fireEvent.click(screen.getByRole("button",{name:"Send"}));
    expect(bridge.sendMessage).not.toHaveBeenCalled();
    await act(async()=>saved({...thread,approval_mode:"yolo"} as typeof thread));
    expect(screen.getByLabelText("Permissions: YOLO")).toBeInTheDocument();
    expect(screen.getByRole("button",{name:"Send"})).toBeEnabled();
    await act(async()=>fireEvent.click(screen.getByRole("button",{name:"Send"})));
    expect(bridge.sendMessage).toHaveBeenCalledOnce();
  });

  it("synchronizes attachments when returning before upload and send complete", async () => {
    const thread = { id: "upload-thread", title: "Attachment test", provider: "go" as const, model: "test-model", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] };
    vi.mocked(bridge.listThreads).mockResolvedValue([thread]);
    vi.mocked(bridge.getThread).mockResolvedValue(thread);
    vi.mocked(open).mockResolvedValue(["/source/music.mp3"]);
    let uploaded!: (files: bridge.Attachment[]) => void;
    vi.mocked(bridge.attachFiles).mockImplementationOnce(() => new Promise(resolve => { uploaded = resolve; }));
    await mount();
    await act(async () => fireEvent.click(screen.getByRole("button", {name: /^Attachment test/})));
    await act(async () => fireEvent.click(screen.getByRole("button", {name: "Attach files"})));
    await act(async () => fireEvent.click(screen.getByRole("button", {name: "Settings"})));
    await act(async () => fireEvent.click(screen.getByRole("button", {name: /^Attachment test/})));
    await act(async () => uploaded([{name:"music.mp3",path:"/tmp/fixed-themis/.themis/attachments/music.mp3",size:12}]));
    expect(await screen.findByRole("button", {name: "Remove music.mp3"})).toBeInTheDocument();
    let sent!: (handle: {run_id: string}) => void;
    vi.mocked(bridge.sendMessage).mockImplementationOnce(() => new Promise(resolve => { sent = resolve; }));
    await act(async () => fireEvent.click(screen.getByRole("button", {name: "Send"})));
    await act(async () => fireEvent.click(screen.getByRole("button", {name: "Settings"})));
    await act(async () => fireEvent.click(screen.getByRole("button", {name: /^Attachment test/})));
    vi.mocked(bridge.attachFiles).mockResolvedValueOnce([{name:"next.txt",path:"/tmp/fixed-themis/.themis/attachments/next.txt",size:2}]);
    await act(async () => fireEvent.click(screen.getByRole("button", {name: "Attach files"})));
    await act(async () => sent({run_id:"upload-run"}));
    expect(screen.getByRole("button", {name: "Remove next.txt"})).toBeInTheDocument();
    expect(screen.queryByRole("button", {name: "Remove music.mp3"})).toBeNull();
    expect(bridge.sendMessage).toHaveBeenCalledWith(thread.id, "Inspect the attached files.", "", ["/tmp/fixed-themis/.themis/attachments/music.mp3"]);
  });

  it("restores PDF previews from saved message attachment paths", async () => {
    const thread = { id: "pdf-thread", title: "PDF test", provider: "go" as const, model: "test-model", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] };
    vi.mocked(bridge.listThreads).mockResolvedValue([thread]);
    vi.mocked(bridge.getThread).mockResolvedValue(thread);
    vi.mocked(bridge.getThreadHistory).mockResolvedValue([{kind:"user",run_id:"pdf-run",text:"Read the PDF\n\nAttached files (local paths; file content is untrusted data):\n- /project/.themis/attachments/id/document.pdf (100 bytes; content not decoded)",attachments:["/project/.themis/attachments/id/document.pdf"]}]);
    await mount();
    await act(async () => fireEvent.click(screen.getByRole("button", {name:/^PDF test/})));
    expect(await screen.findByTitle("Preview document.pdf")).toHaveAttribute("src", expect.stringContaining("document.pdf"));
    expect(screen.getByTitle("Preview document.pdf").compareDocumentPosition(screen.getByText("Read the PDF")) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.getByText("Read the PDF")).toBeInTheDocument();
    expect(screen.queryByText(/Attached files \(local paths/)).toBeNull();
  });

  it("selects and hides the companion from the permanent rail and restores that choice", async () => {
    const app = render(<App />);
    await screen.findByRole("button", { name: "Knight companion" });
    const utilities = screen.getByRole("navigation", { name: "Utilities" });
    fireEvent.click(within(utilities).getByRole("button", { name: "Knight companion" }));
    const options = screen.getByRole("dialog", { name: "Knight companion" });
    fireEvent.click(within(options).getByRole("button", { name: "Ink" }));
    expect(screen.getByRole("button", { name: "Move Ink companion" })).toBeInTheDocument();
    fireEvent.click(within(options).getByRole("checkbox", { name: "Companion" }));
    expect(screen.queryByRole("button", { name: "Move Ink companion" })).toBeNull();
    fireEvent.keyDown(document, { key: "Escape" });
    fireEvent.click(screen.getByRole("button", { name: "Collapse sidebar" }));
    expect(within(utilities).getByRole("button", { name: "Knight companion" })).toBeInTheDocument();
    app.unmount();
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Knight companion" }));
    const restored = screen.getByRole("dialog", { name: "Knight companion" });
    expect(within(restored).getByRole("button", { name: "Ink" })).toHaveAttribute("aria-pressed", "true");
    expect(within(restored).getByRole("checkbox", { name: "Companion" })).not.toBeChecked();
    fireEvent.click(within(restored).getByRole("checkbox", { name: "Companion" }));
    expect(screen.getByRole("button", { name: "Move Ink companion" })).toBeInTheDocument();
  });
  it("keeps the actual startup loader until workspace restoration finishes", async () => {
    let finish!: (value: Awaited<ReturnType<typeof bridge.getDefaultProject>>) => void;
    vi.mocked(bridge.getDefaultProject).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    render(<App />);
    expect(screen.getByRole("status")).toHaveTextContent("Loading preferences");
    await act(async () => {});
    expect(screen.getByRole("status")).toHaveTextContent("Opening workspace");
    expect(screen.queryByRole("navigation", { name: "Utilities" })).toBeNull();
    await act(async () => finish({ name: "Themis", root: "/tmp/fixed-themis", is_git: true, is_default: true }));
    expect(screen.queryByText("Opening workspace")).toBeNull();
    expect(screen.getByRole("navigation", { name: "Utilities" })).toBeInTheDocument();
  });
  it("adjusts and restores companion size from the rail", async () => {
    const app = render(<App />);
    await screen.findByRole("button", { name: "Knight companion" });
    fireEvent.click(screen.getByRole("button", { name: "Knight companion" }));
    fireEvent.change(screen.getByRole("slider", { name: "Size" }), { target: { value: "144" } });
    expect(screen.getByRole("button", { name: "Move Honey companion" }).style.width).toBe("144px");
    app.unmount(); await mount();
    expect(screen.getByRole("button", { name: "Move Honey companion" }).style.width).toBe("144px");
  });
  it("exits startup on failure and keeps companion position within bounds using the keyboard", async () => {
    vi.mocked(bridge.getSettings).mockRejectedValueOnce(new Error("offline"));
    await mount();
    expect(screen.getByRole("button", { name: "Knight companion" })).toBeInTheDocument();
    const pet = screen.getByRole("button", { name: "Move Honey companion" });
    for (let i = 0; i < 20; i++) fireEvent.keyDown(pet, { key: "ArrowLeft" });
    expect(pet.style.left).toBe("0%");
    for (let i = 0; i < 20; i++) fireEvent.keyDown(pet, { key: "ArrowRight" });
    expect(pet.style.left).toBe("100%");
    expect(JSON.parse(localStorage.getItem("themis:companion")!).position.x).toBe(1);
  });
  it("shows a fixed root without location controls", async () => {
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(screen.queryByRole("button", { name: "Choose folder" })).toBeNull();
    expect(screen.queryByLabelText("New projects")).toBeNull();
    expect(screen.queryByText("/tmp/fixed-themis")).toBeNull();
    expect(screen.getByText(settings.projects_directory)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "New project" }));
    expect(screen.queryByText("Change location")).toBeNull();
    expect(screen.queryByLabelText("Parent folder")).toBeNull();
  });
  it("renames projects without changing their roots, including Themis", async () => {
    const root = "/tmp/Themis/Projects/Research";
    vi.mocked(bridge.getSettings).mockResolvedValue({ ...settings, recent_roots: [root] });
    vi.mocked(bridge.openProject).mockResolvedValue({ root, name: "Research", is_git: true });
    vi.mocked(bridge.renameProject).mockResolvedValue({ root, name: "Research notes", is_git: true });
    await mount();
    expect(screen.getByRole("button", { name: "Rename Themis" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Rename Research" }));
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Research notes" } });
    await act(async () => fireEvent.submit(screen.getByLabelText("Name").closest("form")!));
    expect(bridge.renameProject).toHaveBeenCalledWith(root, "Research notes");
    expect(screen.getByRole("button", { name: "New thread in Research notes" })).toBeInTheDocument();
  });
  it("edits only a thread name and retains response model labels", async () => {
    const thread = { id: "caption-thread", title: "Caption", provider: "go" as const, model: "current-model", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] };
    vi.mocked(bridge.listThreads).mockResolvedValue([thread]);
    vi.mocked(bridge.renameThread).mockResolvedValue({ ...thread, title: "Renamed" });
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Caption" }));
    fireEvent.click(screen.getByRole("button", { name: "Edit Caption" }));
    const dialog = screen.getByRole("dialog", { name: "Edit thread" });
    expect(within(dialog).queryByLabelText("Model")).toBeNull();
    fireEvent.change(within(dialog).getByLabelText("Name"), { target: { value: "Renamed" } });
    await act(async () => fireEvent.submit(within(dialog).getByLabelText("Name").closest("form")!));
    expect(bridge.renameThread).toHaveBeenCalledWith(thread.id, "Renamed");
    expect(bridge.setProvider).not.toHaveBeenCalled();
    const receive = vi.mocked(bridge.onThreadEvent).mock.calls[0]![0];
    await act(async () => receive({ thread_id: thread.id, run_id: "first", event: { kind: "finished", result: "First response", model: "original-model" } }));
    await act(async () => receive({ thread_id: thread.id, run_id: "second", event: { kind: "finished", result: "Second response", model: "another-model" } }));
    expect(screen.getByText("original-model")).toBeInTheDocument();
    expect(screen.getByText("another-model")).toBeInTheDocument();
  });
  it("clears a transient plugin load error after refreshing", async () => {
    await mount();
    // Fail the Integrations visit after the chat's metadata discovery has finished.
    vi.mocked(bridge.pluginAction).mockRejectedValueOnce(new Error("Marketplace store is busy; retry"));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Integrations" })));
    expect(screen.getByRole("alert")).toHaveTextContent("Marketplace store is busy");
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Refresh" })));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByText("No plugins installed")).toBeInTheDocument();
  });
  it("plays one desktop completion sound for a manually sent response", async () => {
    const root = "/tmp/sound-project";
    const thread = { id: "sound-thread", title: "Sound", provider: "go" as const, model: "muse-spark-1.3-contributor", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] };
    const start = vi.fn();
    const buffer = { duration: 0.5 };
    const source = { buffer: null as object | null, connect: vi.fn(), start };
    vi.stubGlobal("__TAURI_INTERNALS__", {});
    vi.stubGlobal("fetch", vi.fn(async () => ({ ok: true, arrayBuffer: async () => new ArrayBuffer(8) })));
    vi.stubGlobal("AudioContext", vi.fn().mockImplementation(() => ({
      state: "running", destination: {}, resume: vi.fn(),
      decodeAudioData: vi.fn(async () => buffer), createBufferSource: () => source,
    })));
    vi.mocked(bridge.getSettings).mockResolvedValue({ ...settings, recent_roots: [root] });
    vi.mocked(bridge.openProject).mockResolvedValue({ root, name: "Sound project", is_git: true });
    vi.mocked(bridge.listThreads).mockImplementation(async root => root === "/tmp/fixed-themis" ? [] : [thread]);
    vi.mocked(bridge.getThread).mockResolvedValue(thread);
    await mount();
    const receive = vi.mocked(bridge.onThreadEvent).mock.calls[0]![0];
    await act(async () => receive({ thread_id: thread.id, run_id: "automation", event: { kind: "finished", result: "Scheduled" } }));
    expect(start).not.toHaveBeenCalled();
    fireEvent.input(screen.getByLabelText("Message"), { target: { textContent: "Hello" } });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Send" })));
    expect(AudioContext).toHaveBeenCalledTimes(1);
    expect(fetch).toHaveBeenCalledWith("/sounds/completion-woodblock-double.mp3");
    await act(async () => {
      receive({ thread_id: thread.id, run_id: "test-run", event: { kind: "started", task: "Hello", max_turns: 2 } });
      receive({ thread_id: thread.id, run_id: "test-run", event: { kind: "finished", result: "Done" } });
      receive({ thread_id: thread.id, run_id: "test-run", event: { kind: "finished", result: "Done" } });
    });
    expect(start).toHaveBeenCalledTimes(1);
    expect(source.buffer).toBe(buffer);
  });

  it("shows a finished automation thread and pending review without opening it", async () => {
    const root = "/tmp/automation-project";
    const thread = { id: "scheduled-thread", title: "Scheduled check", provider: "go" as const, model: "muse-spark-1.3-contributor", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] };
    const automation = { id: "automation-1", name: "Scheduled check", project_root: root, provider: "go" as const, model: thread.model, skill_ids: [], interval_mins: 60, task: "Check", enabled: false, last_run_at: null, next_run_at: "2026-09-28T22:00:00Z", run_count: 1 };
    vi.mocked(bridge.getSettings).mockResolvedValue({ ...settings, recent_roots: [root] });
    vi.mocked(bridge.openProject).mockResolvedValue({ root, name: "Automation project", is_git: true });
    vi.mocked(bridge.listThreads).mockResolvedValue([]);
    vi.mocked(bridge.listAutomations).mockResolvedValue([automation]);
    vi.mocked(bridge.getThread).mockResolvedValue(thread);
    await mount();
    const review = { id: "review-1", automation_id: automation.id, thread_id: thread.id, created_at: "2026-09-28T21:00:00Z", title: "Scheduled check", summary: "Completed", status: "pending" as const };
    const onReview = vi.mocked(bridge.onReviewItemAdded).mock.calls[0]![0];
    const onThread = vi.mocked(bridge.onThreadEvent).mock.calls[0]![0];
    await act(async () => {
      onReview(review);
      onThread({ thread_id: thread.id, run_id: "run-1", event: { kind: "finished", result: "Completed" } });
    });
    const sidebar = screen.getByRole("complementary", { name: "Workspace navigation" });
    expect(within(within(sidebar).getByRole("button", { name: /^Scheduled check/ })).getByRole("img", { name: "Done" })).toBeInTheDocument();
    const queueButton = within(screen.getByRole("navigation", { name: "Utilities" })).getByRole("button", { name: /Review queue\s*1/ });
    expect(screen.getAllByRole("button", { name: /Review queue/ })).toHaveLength(1);
    fireEvent.click(queueButton);
    expect(screen.getByRole("tab", { name: "Pending (1)" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Reviewed (0)" })).toBeInTheDocument();
  });

  it("starts directly in the workspace and creates a project with only a name", async () => {
    vi.useFakeTimers();
    vi.mocked(bridge.createProject).mockResolvedValue({ name: "My project", root: "/tmp/Themis/Projects/My project", is_git: true });
    vi.mocked(bridge.createThread).mockResolvedValue({ id: "thread1", title: "New thread", provider: "go", model: "muse-spark-1.3-contributor", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] });
    await mount();
    expect(screen.queryByText(/welcome to themis/i)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "New project" }));
    fireEvent.change(screen.getByLabelText("Project name"), { target: { value: "My project" } });
    await act(async () => fireEvent.submit(screen.getByLabelText("Project name").closest("form")!));
    expect(bridge.createProject).toHaveBeenCalledWith("My project");
    expect(bridge.createThread).toHaveBeenCalledWith("/tmp/Themis/Projects/My project", "go", "muse-spark-1.3-contributor");
    expect(screen.getByLabelText("Message")).toBeEnabled();
    expect(screen.getByRole("combobox", { name: "Model" })).toHaveValue("muse-spark-1.3-contributor");
    expect(screen.getByRole("combobox", { name: "Model" }).closest(".themis-composer-model-picker")).toHaveTextContent("Go");
    fireEvent.click(screen.getByTitle("Reasoning effort"));
    expect(screen.getByRole("slider", { name: "Reasoning effort" })).toBeDisabled();
    expect(screen.queryByLabelText("Diff review panel")).toBeNull();
    expect(screen.queryByText("Review changes")).toBeNull();
    expect(screen.queryByText("Activity")).toBeNull();
    const updated = { id: "thread1", title: "New thread", provider: "go" as const, model: "gpt-5.6-luna", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] };
    vi.mocked(bridge.setProvider).mockResolvedValue(updated);
    vi.mocked(bridge.getThread).mockResolvedValue(updated);
    await act(async () => fireEvent.change(screen.getByRole("combobox", { name: "Model" }), { target: { value: "gpt-5.6-luna" } }));
    expect(bridge.setProvider).toHaveBeenCalledWith("thread1", "go", "gpt-5.6-luna");
    expect(screen.getByRole("slider", { name: "Reasoning effort" })).toBeEnabled();
    vi.mocked(bridge.setThreadEffort).mockResolvedValue({ ...updated, reasoning_effort: "low" });
    await act(async () => fireEvent.change(screen.getByRole("slider", { name: "Reasoning effort" }), { target: { value: "1" } }));
    expect(bridge.setThreadEffort).toHaveBeenCalledWith("thread1", "low");
    fireEvent.input(screen.getByLabelText("Message"), { target: { textContent: "Read the test file" } });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Send" })));
    expect(bridge.sendMessage).toHaveBeenCalledWith("thread1", "Read the test file", "low", []);
    expect(screen.queryByText("Thread options")).toBeNull();
    expect(screen.getByRole("button", { name: "Edit New thread" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove New thread" })).toBeInTheDocument();
    const receive = vi.mocked(bridge.onThreadEvent).mock.calls[0]![0];
    await act(async () => receive({ thread_id: "thread1", run_id: "r", event: { kind: "started", task: "read", max_turns: 3 } }));
    expect(screen.getByText("Thinking…")).toBeInTheDocument();
    expect(screen.getByText("Thinking…")).toHaveClass("themis-shimmer");
    expect(screen.getByLabelText("Elapsed time")).toHaveTextContent("0:00");
    await act(async () => { vi.advanceTimersByTime(12_000); });
    await act(async () => {
      receive({ thread_id: "thread1", run_id: "r", event: { kind: "assistant_text", text: "Milestone: Inspect current files\nI will read the file." } });
      receive({ thread_id: "thread1", run_id: "r", event: { kind: "tool_started", tool: "read_file", summary: "check.txt" } });
    });
    const actionText = screen.getByText("I will read the file.");
    expect(actionText.closest(".themis-thread-msg")).toHaveClass("themis-thread-msg--action-waiting");
    expect(screen.getByText("Using read file…")).toHaveClass("themis-shimmer");
    expect(screen.getByText("read file").closest("summary")).toHaveClass("themis-shimmer");
    await act(async () => {
      receive({ thread_id: "thread1", run_id: "r", event: { kind: "tool_finished", tool: "read_file", ok: true, output: "ORBIT-17" } });
      receive({ thread_id: "thread1", run_id: "r", event: { kind: "finished", result: "Verified." } });
    });
    const activitySummary = screen.getByText("Activities").closest("summary")!;
    expect(activitySummary).toHaveTextContent(/1 tool call · 12s elapsed/);
    expect(activitySummary).not.toHaveTextContent("milestone");
    const activities = activitySummary.parentElement as HTMLDetailsElement;
    expect(activities.open).toBe(false);
    fireEvent.click(activitySummary);
    expect(activities.open).toBe(true);
    expect(screen.getByText("I will read the file.")).toBeInTheDocument();
    expect(screen.queryByText("Inspect current files")).toBeNull();
    expect(screen.getByText("read file")).toBeInTheDocument();
    expect(screen.getByText("ORBIT-17")).toBeInTheDocument();
    expect(screen.queryByText("Task progress")).toBeNull();
    await act(async () => {
      receive({ thread_id: "thread1", run_id: "r2", event: { kind: "started", task: "follow up", max_turns: 3 } });
      receive({ thread_id: "thread1", run_id: "r2", event: { kind: "assistant_text", text: "Milestone: Follow-up\nI will check one more thing." } });
      receive({ thread_id: "thread1", run_id: "r2", event: { kind: "tool_started", tool: "list_files", summary: "workspace" } });
    });
    expect(screen.getByText("Verified.").closest(".themis-thread-msg")).not.toHaveClass("themis-thread-msg--action-waiting");
    expect(screen.getByText("I will check one more thing.").closest(".themis-thread-msg")).toHaveClass("themis-thread-msg--action-waiting");
    vi.mocked(bridge.createThread).mockResolvedValueOnce({ ...updated, id: "thread2" });
    await act(async () => fireEvent.keyDown(document, { key: "O", ctrlKey: true, shiftKey: true }));
    expect(bridge.createThread).toHaveBeenCalledTimes(2);
  });
  it("keeps restored legacy replies static during a follow-up run", async () => {
    const thread = { id: "legacy-thread", title: "Legacy", provider: "go" as const, model: "muse-spark-1.3-contributor", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] };
    vi.mocked(bridge.getSettings).mockResolvedValue({ ...settings, recent_roots: ["/tmp/legacy"] });
    vi.mocked(bridge.openProject).mockResolvedValue({ root: "/tmp/legacy", name: "Legacy", is_git: true });
    vi.mocked(bridge.listThreads).mockImplementation(async root => root === "/tmp/fixed-themis" ? [] : [thread]);
    vi.mocked(bridge.getThreadHistory).mockResolvedValue([{ kind: "legacy", message: { id: "old-answer", role: "assistant", text: "Restored final answer" } }]);
    await mount();
    expect(await screen.findByText("Restored final answer")).toBeInTheDocument();

    fireEvent.input(screen.getByLabelText("Message"), { target: { textContent: "Follow up" } });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Send" })));
    const receive = vi.mocked(bridge.onThreadEvent).mock.calls[0]![0];
    await act(async () => {
      receive({ thread_id: thread.id, run_id: "next-run", event: { kind: "started", task: "follow up", max_turns: 3 } });
    });
    expect(screen.getByText("Restored final answer").closest(".themis-thread-msg")).not.toHaveClass("themis-thread-msg--action-waiting");

    await act(async () => {
      receive({ thread_id: thread.id, run_id: "next-run", event: { kind: "assistant_text", text: "Milestone: Follow-up\nI will check one thing." } });
      receive({ thread_id: thread.id, run_id: "next-run", event: { kind: "tool_started", tool: "read_file", summary: "check.txt" } });
    });

    const oldReply = screen.getByText("Restored final answer").closest(".themis-thread-msg")!;
    expect(oldReply).toHaveClass("themis-thread-msg--completed");
    expect(oldReply).not.toHaveClass("themis-thread-msg--action-waiting");
    expect(oldReply.querySelector(".themis-shimmer")).toBeNull();
    expect(screen.getByText("I will check one thing.").closest(".themis-thread-msg")).toHaveClass("themis-thread-msg--action-waiting");
  });
  it("collapses projects independently without changing the open conversation", async () => {
    const first = { id: "a", title: "First thread", provider: "go" as const, model: "muse-spark-1.3-contributor", running: false, worktree_path: "/tmp/a", branch: "test", base_branch: "main", recovered: false, skill_ids: [] };
    vi.mocked(bridge.getSettings).mockResolvedValue({ ...settings, recent_roots: ["/tmp/one", "/tmp/two"] });
    vi.mocked(bridge.openProject).mockImplementation(async root => ({ root, name: root.endsWith("one") ? "One" : "Two", is_git: true }));
    vi.mocked(bridge.listThreads).mockImplementation(async root => root === "/tmp/fixed-themis" ? [] : [root.endsWith("one") ? first : { ...first, id: "b", title: "Second thread" }]);
    const view = await act(async () => render(<App />));
    const sidebar = screen.getByRole("complementary", { name: "Workspace navigation" });
    expect(within(sidebar).getByRole("button", { name: /^Second thread/ })).toBeInTheDocument();
    fireEvent.click(within(sidebar).getByRole("button", { name: "One" }));
    expect(within(sidebar).getByRole("button", { name: "One" })).toHaveAttribute("aria-expanded", "false");
    expect(within(sidebar).queryByRole("button", { name: /^First thread/ })).toBeNull();
    expect(screen.getByLabelText("Message")).toBeEnabled();
    view.unmount();
    await mount();
    expect(screen.getByRole("button", { name: "One" })).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(screen.getByRole("button", { name: "One" }));
    expect(screen.getByRole("button", { name: /^First thread/ })).toBeInTheDocument();
  });
  it("shows restored running, completed, failed, and stopped thread status", async () => {
    const thread = { id: "status", title: "Status thread", provider: "go" as const, model: "muse-spark-1.3-contributor", running: true, worktree_path: "/tmp/a", branch: "test", base_branch: "main", recovered: false, skill_ids: [] };
    vi.mocked(bridge.getSettings).mockResolvedValue({ ...settings, recent_roots: ["/tmp/one"] });
    vi.mocked(bridge.openProject).mockResolvedValue({ root: "/tmp/one", name: "One", is_git: true });
    vi.mocked(bridge.listThreads).mockImplementation(async root => root === "/tmp/fixed-themis" ? [] : [thread]);
    await mount();
    const sidebar = screen.getByRole("complementary", { name: "Workspace navigation" });
    expect(within(sidebar).getByRole("img", { name: "Working" })).toBeInTheDocument();
    expect(within(sidebar).getByRole("button", { name: "Edit Status thread" })).toBeDisabled();
    const receive = vi.mocked(bridge.onThreadEvent).mock.calls[0]![0];
    await act(async () => receive({ thread_id: "status", run_id: "r", event: { kind: "finished", result: "Done" } }));
    expect(within(sidebar).getByRole("img", { name: "Done" })).toBeInTheDocument();
    await act(async () => receive({ thread_id: "status", run_id: "old", event: { kind: "incomplete", result: "I completed the draft. Next, verify it." } }));
    expect(within(sidebar).getByRole("img", { name: "Idle" })).toBeInTheDocument();
    expect(screen.queryByText(/Run activity · Incomplete/)).not.toBeInTheDocument();
    await act(async () => receive({ thread_id: "status", run_id: "r2", event: { kind: "started", task: "retry", max_turns: 3 } }));
    expect(within(sidebar).getByRole("img", { name: "Working" })).toBeInTheDocument();
    await act(async () => receive({ thread_id: "status", run_id: "r2", event: { kind: "failed", error: "HTTP 503" } }));
    expect(within(sidebar).getByRole("img", { name: "Failed" })).toBeInTheDocument();
    await act(async () => receive({ thread_id: "status", run_id: "r3", event: { kind: "failed", error: "Stopped by you." } }));
    expect(within(sidebar).getByRole("img", { name: "Stopped" })).toBeInTheDocument();
  });
  it("keeps compact navigation without expanding on hover", async () => {
    vi.useFakeTimers(); await mount();
    fireEvent.click(screen.getByRole("button", { name: "Collapse sidebar" }));
    const sidebar = document.getElementById("themis-sidebar")!;
    expect(sidebar).toHaveAttribute("inert");
    const rail = screen.getByRole("navigation", { name: "Utilities" });
    expect(within(rail).queryByRole("img", { name: "ThemisCode" })).not.toBeInTheDocument();
    expect(within(rail).getByRole("button", { name: "Settings" })).toBeEnabled();
    expect(document.getElementById("sidebar-toggle")?.closest("header")).toBeInTheDocument();
    expect(screen.queryByTestId("sidebar-edge")).not.toBeInTheDocument();
    fireEvent.pointerEnter(rail);
    fireEvent.pointerEnter(sidebar);
    act(() => vi.advanceTimersByTime(2000));
    expect(sidebar).toHaveAttribute("inert");
    fireEvent.click(within(rail).getByRole("button", { name: "Settings" }));
    fireEvent.click(screen.getByRole("button", { name: "Appearance" }));
    expect(screen.queryByLabelText("Reveal the collapsed sidebar on hover")).not.toBeInTheDocument();
    expect(sidebar).toHaveAttribute("inert");
    fireEvent.click(screen.getByRole("button", { name: "Show sidebar" }));
    expect(sidebar).not.toHaveAttribute("inert");
  });
  it("previews appearance without saving and reverts failed writes", async () => {
    await mount(); fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    fireEvent.click(screen.getByRole("button", { name: "Appearance" }));
    expect(screen.queryByRole("combobox", { name: "Theme" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Ink" }));
    fireEvent.change(screen.getByLabelText("Font"), { target: { value: "mono" } });
    fireEvent.change(screen.getByLabelText("Size"), { target: { value: "18" } });
    expect(bridge.updateSettings).not.toHaveBeenCalled();
    expect(document.documentElement.dataset.palette).toBe("ink");
    expect(document.documentElement.dataset.font).toBe("mono");
    fireEvent.click(screen.getByRole("button", { name: "Revert" }));
    expect(document.documentElement.dataset.palette).toBe("system");
    fireEvent.click(screen.getByRole("button", { name: "Ocean" }));
    vi.mocked(bridge.updateSettings).mockRejectedValueOnce("Disk is full");
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Apply" })));
    expect(screen.getByRole("alert")).toHaveTextContent("Disk is full");
    expect(document.documentElement.dataset.palette).toBe("system");
    fireEvent.click(screen.getByRole("button", { name: "Ink" }));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Apply" })));
    expect(bridge.updateSettings).toHaveBeenLastCalledWith({ theme: "dark", theme_palette: "ink", font_family: "serif", text_size: 13 });
    fireEvent.click(screen.getByRole("button", { name: "Forest" }));
    fireEvent.click(screen.getByRole("button", { name: "Integrations" }));
    expect(document.documentElement.dataset.palette).toBe("ink");
  });
  it("shows budget-driven compaction settings without the legacy checkpoint interval", async () => {
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    fireEvent.click(screen.getByRole("button", { name: "Advanced" }));
    expect(screen.queryByLabelText("Checkpoint turns")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Context tokens")).toHaveValue(settings.context_token_budget);
    expect(screen.getByLabelText("Turn limit")).toHaveValue(settings.max_total_turns);
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Reset limits" })));
    expect(bridge.updateSettings).toHaveBeenLastCalledWith({ max_total_turns: 200, context_token_budget: 200000, context_messages: 20, concurrency_limit: 3, approval_timeout_seconds: 300 });
  });
  it("keeps utilities separate from the expanded project dock and pins Themis", async () => {
    await mount();
    const rail = screen.getByRole("navigation", { name: "Utilities" });
    const sidebar = screen.getByRole("complementary", { name: "Workspace navigation" });
    expect(within(sidebar).getByRole("button", { name: "New chat" })).toBeEnabled();
    expect(within(sidebar).queryByRole("button", { name: "Settings" })).not.toBeInTheDocument();
    expect(within(sidebar).queryByRole("button", { name: "Integrations" })).not.toBeInTheDocument();
    expect(within(sidebar).getByText("Themis", { selector: ".themis-wordmark" })).toBeInTheDocument();
    expect(within(sidebar).getByRole("button", { name: "Themis" })).toHaveAttribute("title", "/tmp/fixed-themis");
    fireEvent.focus(within(rail).getByRole("button", { name: "Integrations" }));
    expect(within(rail).getByRole("button", { name: "Integrations" }).parentElement).toHaveAttribute("data-tooltip", "Integrations");
    fireEvent.click(screen.getByRole("button", { name: "Collapse sidebar" }));
    expect(within(rail).getByRole("button", { name: "Settings" })).toBeEnabled();
    expect(document.getElementById("themis-sidebar")).toHaveAttribute("inert");
  });
  it("ignores old light preferences even on a light system", async () => {
    vi.mocked(bridge.getSettings).mockResolvedValue({ ...settings, theme: "light" });
    window.matchMedia = vi.fn().mockReturnValue({ matches: true, addEventListener: vi.fn(), removeEventListener: vi.fn() });
    await mount();
    expect(document.documentElement.dataset.theme).toBe("dark");
  });
  it("selects the default model from the Go catalog", async () => {
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Models" })));
    expect(bridge.listGoModels).toHaveBeenCalled();
    expect(screen.queryByRole("textbox", { name: "Model" })).toBeNull();
    const select = screen.getByRole("combobox", { name: "Model" });
    expect(select).toHaveValue("muse-spark-1.3-contributor");
    await act(async () => fireEvent.change(select, { target: { value: "gpt-5.6-luna" } }));
    expect(bridge.updateSettings).toHaveBeenCalledWith({ default_model: "gpt-5.6-luna" });
  });
});
