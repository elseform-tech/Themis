// @vitest-environment jsdom
import { act, fireEvent, render, screen, within, waitFor } from "@testing-library/react";
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
  onThreadEvent: vi.fn(async () => () => {}), onApprovalRequest: vi.fn(async () => () => {}), onApprovalExpired: vi.fn(async () => () => {}), onReviewItemAdded: vi.fn(async () => () => {}), onThreadChanged: vi.fn(async () => () => {}),
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
  it("starts MCP connections without waiting for them before showing the workspace", async () => {
    let finish!: (value: unknown) => void;
    vi.mocked(bridge.pluginAction).mockImplementation(async args => args.action === "mcp_status" ? await new Promise<unknown>(resolve => { finish = resolve; }) as never : [] as never);
    await mount();
    expect(screen.getByRole("button", { name: "Integrations" })).toBeVisible();
    expect(bridge.pluginAction).toHaveBeenCalledWith(expect.objectContaining({ action: "mcp_status", connect: true }));
    await act(async () => finish({ "global:test:mcp:broken": { status: "failed", reason: "Could not connect" } }));
    expect(screen.getByText(/MCP broken: Could not connect/).closest(".themis-toast")).not.toBeNull();
    vi.mocked(bridge.pluginAction).mockImplementation(async () => [] as never);
  });
  it("shows chats created, renamed and removed through the shared server without a reload", async () => {
    let receive!: Parameters<typeof bridge.onThreadChanged>[0];
    vi.mocked(bridge.onThreadChanged).mockImplementationOnce(async callback => { receive = callback; return () => {}; });
    const alpha = { name: "Alpha", root: "/tmp/alpha", is_git: true };
    const first = { id: "first", title: "First chat", provider: "go" as const, model: "test", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [], last_activity_seq: 10 };
    vi.mocked(bridge.getDefaultProject).mockResolvedValueOnce(alpha);
    vi.mocked(bridge.getSettings).mockResolvedValueOnce({ ...settings, recent_roots: [alpha.root] });
    vi.mocked(bridge.openProject).mockResolvedValue(alpha);
    vi.mocked(bridge.listThreads).mockResolvedValue([first]);
    vi.mocked(bridge.getThread).mockResolvedValue(first);
    await mount();
    const sidebar = screen.getByRole("complementary", { name: "Workspace navigation" });
    const chats = () => [...within(sidebar).getByRole("region", { name: "Chats" }).querySelectorAll(".themis-thread-row")].map(button => button.getAttribute("aria-label"));
    expect(chats()).toEqual(["First chat"]);
    const made = { ...first, id: "made-by-cli", title: "Made by CLI", last_activity_seq: 20 };
    await act(async () => receive({ kind: "created", project_root: alpha.root, thread_id: made.id, thread: made }));
    expect(chats()).toContain("Made by CLI");
    await act(async () => receive({ kind: "updated", project_root: alpha.root, thread_id: made.id, thread: { ...made, title: "Renamed by CLI", approval_mode: "yolo" } }));
    expect(chats()).toContain("Renamed by CLI");
    expect(chats()).not.toContain("Made by CLI");
    await act(async () => receive({ kind: "removed", project_root: alpha.root, thread_id: made.id }));
    expect(chats()).toEqual(["First chat"]);
  });
  it("lists only the chosen project's chats, most recent first, and switches projects", async () => {
    let receive!: Parameters<typeof bridge.onThreadEvent>[0];
    vi.mocked(bridge.onThreadEvent).mockImplementationOnce(async callback => { receive = callback; return () => {}; });
    const alpha = { name: "Alpha", root: "/tmp/alpha", is_git: true };
    const beta = { name: "Beta", root: "/tmp/beta", is_git: true };
    const older = { id: "older", title: "Earlier chat", provider: "go" as const, model: "test", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [], last_activity_seq: 10 };
    const newer = { ...older, id: "newer", title: "Latest chat", last_activity_seq: 20 };
    const betaChat = { ...older, id: "beta-chat", title: "Beta chat", last_activity_seq: 30 };
    vi.mocked(bridge.getDefaultProject).mockResolvedValueOnce(alpha);
    vi.mocked(bridge.getSettings).mockResolvedValueOnce({ ...settings, recent_roots: [alpha.root, beta.root] });
    vi.mocked(bridge.openProject).mockImplementation(async root => root === alpha.root ? alpha : beta);
    vi.mocked(bridge.listThreads).mockImplementation(async root => root === alpha.root ? [older, newer] : [betaChat]);
    vi.mocked(bridge.getThread).mockImplementation(async id => [older, newer, betaChat].find(thread => thread.id === id)!);
    await mount();
    const sidebar = screen.getByRole("complementary", { name: "Workspace navigation" });
    const chats = () => [...within(sidebar).getByRole("region", { name: "Chats" }).querySelectorAll(".themis-thread-row")].map(button => button.getAttribute("aria-label"));
    expect(within(sidebar).getByLabelText("Project: Alpha")).toBeInTheDocument();
    expect(chats()).toEqual(["Latest chat", "Earlier chat"]);
    expect(within(sidebar).queryByRole("button", { name: "Beta chat" })).toBeNull();
    expect(within(sidebar).queryByRole("region", { name: "Recent" })).toBeNull();
    fireEvent.click(within(sidebar).getByRole("button", { name: /^Beta/ }));
    expect(within(sidebar).getByLabelText("Project: Beta")).toBeInTheDocument();
    expect(chats()).toEqual(["Beta chat"]);
    expect(within(sidebar).getByRole("button", { name: "Beta chat" })).toHaveAttribute("aria-current", "true");
    fireEvent.click(within(sidebar).getByRole("button", { name: /^Alpha/ }));
    fireEvent.click(screen.getByRole("button", { name: "Collapse sidebar" }));
    expect(document.getElementById("themis-sidebar")).toHaveAttribute("inert");
    fireEvent.click(screen.getByRole("button", { name: "Show sidebar" }));
    older.last_activity_seq = 40;
    await act(async () => receive({ thread_id: older.id, run_id: "latest-run", event: { kind: "finished", result: "Done" } }));
    expect(chats()).toEqual(["Earlier chat", "Latest chat"]);
  });

  it("resizes the sidebar and restores its saved width after remount", async () => {
    let mounted!: ReturnType<typeof render>;
    await act(async () => { mounted = render(<App />); });
    const handle = screen.getByRole("separator", { name: "Resize sidebar" });
    expect(handle).toHaveAttribute("aria-valuenow", "280");
    fireEvent.keyDown(handle, { key: "ArrowRight" });
    expect(handle).toHaveAttribute("aria-valuenow", "300");
    expect(localStorage.getItem("themis:sidebarWidth")).toBe("300");
    vi.stubGlobal("PointerEvent", MouseEvent);
    handle.setPointerCapture = vi.fn();
    handle.hasPointerCapture = vi.fn(() => true);
    handle.releasePointerCapture = vi.fn();
    vi.spyOn(handle.parentElement!, "getBoundingClientRect").mockReturnValue({ width: 300 } as DOMRect);
    fireEvent.pointerDown(handle, { pointerId: 1, button: 0, clientX: 300 });
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: 420 });
    // Dragging only moves the CSS width; the value commits on release.
    expect(handle.parentElement!.style.getPropertyValue("--themis-sidebar-width")).toBe("420px");
    fireEvent.pointerUp(handle, { pointerId: 1 });
    expect(handle).toHaveAttribute("aria-valuenow", "420");
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: 450 });
    expect(localStorage.getItem("themis:sidebarWidth")).toBe("420");
    mounted.unmount();
    await mount();
    const restored = screen.getByRole("separator", { name: "Resize sidebar" });
    expect(restored).toHaveAttribute("aria-valuenow", "420");
    fireEvent.keyDown(restored, { key: "End" });
    expect(restored).toHaveAttribute("aria-valuenow", "560");
    fireEvent.keyDown(restored, { key: "ArrowRight" });
    expect(restored).toHaveAttribute("aria-valuenow", "560");
    fireEvent.keyDown(restored, { key: "Home" });
    expect(restored).toHaveAttribute("aria-valuenow", "280");
  });

  it("restores an active approval when the app is refreshed", async () => {
    const thread = { id: "waiting", title: "Waiting for approval", provider: "go" as const, model: "test", running: true, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] };
    vi.mocked(bridge.listThreads).mockResolvedValue([thread]);
    vi.mocked(bridge.getThread).mockResolvedValue(thread);
    vi.mocked(bridge.getThreadHistory).mockResolvedValueOnce([{ kind: "event", envelope: { thread_id: thread.id, run_id: "active", event: { kind: "started", task: "Read skill", max_turns: 5 } } }]);
    vi.mocked(bridge.getPendingApprovals).mockResolvedValueOnce([{ thread_id: thread.id, approval_id: "pending", tool: "read_file", risk: "read", summary: "Read skill instructions" }]);
    await mount();
    // The chat says it waits; the approval itself sits in that chat.
    await act(async () => fireEvent.click(await screen.findByRole("button", { name: /^Waiting for approval/ })));
    expect(await screen.findByRole("region", { name: "Approval needed" })).toHaveTextContent("Read skill instructions");
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

  it("does not reload an idle chat's history again while another chat keeps running", async () => {
    const base = { provider: "go" as const, model: "test", worktree_path: null, base_branch: null, branch: null, approval_mode: "custom" as const, last_activity_seq: 5 };
    const idle = { ...base, id: "idle", title: "Idle chat", running: false };
    const busy = { ...base, id: "busy", title: "Busy chat", running: true };
    vi.mocked(bridge.listThreads).mockResolvedValue([idle, busy] as never);
    vi.mocked(bridge.getThread).mockResolvedValue(idle as never);
    vi.mocked(bridge.getThreadHistory).mockClear();
    await mount();
    const loads = () => vi.mocked(bridge.getThreadHistory).mock.calls.filter(call => call[0] === "idle").length;
    const passes = () => vi.mocked(bridge.listThreads).mock.calls.length;
    await waitFor(() => expect(loads()).toBeGreaterThan(0));
    await waitFor(() => expect(passes()).toBeGreaterThanOrEqual(2), { timeout: 5000 });
    const settled = loads();
    const seen = passes();
    await waitFor(() => expect(passes()).toBeGreaterThanOrEqual(seen + 2), { timeout: 7000 });
    expect(loads()).toBe(settled);
  }, 15000);

  it("reloads an idle chat's history once it has new activity", async () => {
    const base = { provider: "go" as const, model: "test", worktree_path: null, base_branch: null, branch: null, approval_mode: "custom" as const };
    const idle = { ...base, id: "idle", title: "Idle chat", running: false, last_activity_seq: 5 };
    const busy = { ...base, id: "busy", title: "Busy chat", running: true, last_activity_seq: 5 };
    vi.mocked(bridge.listThreads).mockResolvedValue([idle, busy] as never);
    vi.mocked(bridge.getThread).mockResolvedValue(idle as never);
    vi.mocked(bridge.getThreadHistory).mockClear();
    await mount();
    await waitFor(() => expect(vi.mocked(bridge.getThreadHistory)).toHaveBeenCalledWith("idle"));
    const changed = { ...idle, last_activity_seq: 6 };
    vi.mocked(bridge.listThreads).mockResolvedValue([changed, busy] as never);
    vi.mocked(bridge.getThread).mockResolvedValue(changed as never);
    await waitFor(() => expect(vi.mocked(bridge.getThreadHistory).mock.calls.filter(call => call[0] === "idle").length).toBe(2), { timeout: 5000 });
  }, 10000);

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
    // Settings stands in the sidebar's place; Back to app returns to the chats.
    expect(screen.queryByRole("button", {name: /^Attachment test/})).toBeNull();
    await act(async () => fireEvent.click(screen.getByRole("button", {name: "Back to app"})));
    await act(async () => fireEvent.click(screen.getByRole("button", {name: /^Attachment test/})));
    await act(async () => uploaded([{name:"music.mp3",path:"/tmp/fixed-themis/.themis/attachments/music.mp3",size:12}]));
    expect(await screen.findByRole("button", {name: "Remove music.mp3"})).toBeInTheDocument();
    let sent!: (handle: {run_id: string}) => void;
    vi.mocked(bridge.sendMessage).mockImplementationOnce(() => new Promise(resolve => { sent = resolve; }));
    await act(async () => fireEvent.click(screen.getByRole("button", {name: "Send"})));
    await act(async () => fireEvent.click(screen.getByRole("button", {name: "Settings"})));
    await act(async () => fireEvent.click(screen.getByRole("button", {name: "Back to app"})));
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
    await screen.findByRole("button", { name: "Bubbles companion" });
    const utilities = screen.getByRole("navigation", { name: "Utilities" });
    fireEvent.click(within(utilities).getByRole("button", { name: "Bubbles companion" }));
    const options = screen.getByRole("dialog", { name: "Bubbles companion" });
    fireEvent.click(within(options).getByRole("button", { name: "Ink" }));
    expect(screen.getByRole("button", { name: "Move Ink companion" })).toBeInTheDocument();
    fireEvent.click(within(options).getByRole("checkbox", { name: "Companion" }));
    expect(screen.queryByRole("button", { name: "Move Ink companion" })).toBeNull();
    fireEvent.keyDown(document, { key: "Escape" });
    fireEvent.click(screen.getByRole("button", { name: "Collapse sidebar" }));
    expect(within(utilities).getByRole("button", { name: "Bubbles companion" })).toBeInTheDocument();
    app.unmount();
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Bubbles companion" }));
    const restored = screen.getByRole("dialog", { name: "Bubbles companion" });
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
    await screen.findByRole("button", { name: "Bubbles companion" });
    fireEvent.click(screen.getByRole("button", { name: "Bubbles companion" }));
    fireEvent.change(screen.getByRole("slider", { name: "Size" }), { target: { value: "144" } });
    expect(screen.getByRole("button", { name: "Move Honey companion" }).style.width).toBe("144px");
    app.unmount(); await mount();
    expect(screen.getByRole("button", { name: "Move Honey companion" }).style.width).toBe("144px");
  });
  it("exits startup on failure and keeps companion position within bounds using the keyboard", async () => {
    vi.mocked(bridge.getSettings).mockRejectedValueOnce(new Error("offline"));
    await mount();
    expect(screen.getByRole("button", { name: "Bubbles companion" })).toBeInTheDocument();
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
    fireEvent.click(screen.getByRole("button", { name: "Back to app" }));
    fireEvent.click(screen.getByRole("button", { name: "New project" }));
    expect(screen.queryByText("Change location")).toBeNull();
    expect(screen.queryByLabelText("Parent folder")).toBeNull();
  });
  it("renames any listed project from its own row without switching to it, including Themis", async () => {
    const root = "/tmp/Themis/Projects/Research";
    vi.mocked(bridge.getSettings).mockResolvedValue({ ...settings, recent_roots: [root] });
    vi.mocked(bridge.openProject).mockResolvedValue({ root, name: "Research", is_git: true });
    vi.mocked(bridge.renameProject).mockResolvedValue({ root, name: "Research notes", is_git: true });
    await mount();
    fireEvent.click(screen.getByRole("button", { name: /^Themis/ }));
    expect(screen.getByRole("button", { name: "Rename Themis" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Rename Research" }));
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Research notes" } });
    await act(async () => fireEvent.submit(screen.getByLabelText("Name").closest("form")!));
    expect(bridge.renameProject).toHaveBeenCalledWith(root, "Research notes");
    expect(screen.getByLabelText("Project: Themis")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Rename Research notes" })).toBeInTheDocument();
  });
  it("deletes a non-active project from its row after confirmation and never offers it for the built-in project", async () => {
    const root = "/tmp/Themis/Projects/Scratch";
    vi.mocked(bridge.getSettings).mockResolvedValue({ ...settings, recent_roots: [root] });
    vi.mocked(bridge.openProject).mockResolvedValue({ root, name: "Scratch", is_git: true });
    const remove = vi.spyOn(bridge, "deleteProject").mockResolvedValue(undefined);
    await mount();
    fireEvent.click(screen.getByRole("button", { name: /^Themis/ }));
    expect(screen.queryByRole("button", { name: "Delete Themis" })).toBeNull();
    expect(screen.getByLabelText("Project: Themis")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Delete Scratch" }));
    const dialog = screen.getByRole("dialog", { name: "Delete project?" });
    expect(dialog).toHaveTextContent("moves to the Trash");
    await act(async () => fireEvent.click(within(dialog).getByRole("button", { name: "Delete project" })));
    expect(remove).toHaveBeenCalledWith(root);
    expect(screen.queryByRole("button", { name: /^Scratch/ })).toBeNull();
    expect(screen.getByLabelText("Project: Themis")).toBeInTheDocument();
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
  it("shows a transient plugin load error in a dismissible timed toast", async () => {
    await mount();
    // Fail the Integrations visit after the chat's metadata discovery has finished.
    vi.mocked(bridge.pluginAction).mockRejectedValueOnce(new Error("Marketplace store is busy; retry"));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Integrations" })));
    const notification = screen.getByText("Marketplace store is busy; retry").closest(".themis-toast")!;
    expect(notification.querySelector(".themis-toast-timer")).not.toBeNull();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    fireEvent.click(within(notification as HTMLElement).getByRole("button", { name: "Dismiss notification" }));
    expect(screen.queryByText("Marketplace store is busy; retry")).toBeNull();
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
    // The sound is off by default; this person turned it on.
    vi.mocked(bridge.getSettings).mockResolvedValue({ ...settings, recent_roots: [root], completion_sound: true });
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
    fireEvent.click(within(sidebar).getByRole("button", { name: /^Automation project/ }));
    expect(within(within(sidebar).getByRole("button", { name: "Scheduled check" })).queryByRole("img")).toBeNull();
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
    // Scrolled up, the status shrinks to "…", so it doesn't cover what's being read.
    const stream = document.querySelector(".themis-thread-stream") as HTMLElement;
    const scrollTo = (top: number) => {
      Object.defineProperties(stream, {
        scrollHeight: { configurable: true, value: 2000 },
        clientHeight: { configurable: true, value: 500 },
        scrollTop: { configurable: true, writable: true, value: top },
      });
      fireEvent.scroll(stream);
    };
    await act(async () => scrollTo(100));
    const away = document.querySelector(".themis-run-progress") as HTMLElement;
    expect(away).toHaveClass("is-away");
    expect(away).toHaveTextContent(/^…$/);
    expect(screen.getByLabelText("Thinking…")).toBeInTheDocument();
    await act(async () => scrollTo(1500));
    expect(document.querySelector(".themis-run-progress")).not.toHaveClass("is-away");
    expect(screen.getByText("Thinking…")).toBeInTheDocument();
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
    // The progress sentence stays on screen; the tool call folds into one row that says
    // what it did.
    expect(screen.queryByText("Show thinking")).toBeNull();
    expect(screen.getByText("I will read the file.")).toBeVisible();
    expect(screen.queryByText("Inspect current files")).toBeNull();
    const toolSummary = screen.getByText("Read a file").closest("summary")!;
    expect(screen.getByText("Read a file")).toHaveAttribute("title", "1 tool call");
    const tools = toolSummary.parentElement as HTMLDetailsElement;
    expect(tools.open).toBe(false);
    fireEvent.click(toolSummary);
    expect(tools.open).toBe(true);
    expect(screen.getByText("read file")).toBeInTheDocument();
    expect(screen.getByText("ORBIT-17")).toBeInTheDocument();
    // The run's duration sits with the final answer's model.
    expect(screen.getByText("Verified.").closest(".themis-thread-msg")!.querySelector(".themis-response-model")).toHaveTextContent(/· 12s$/);
    expect(screen.queryByText("Task progress")).toBeNull();
    await act(async () => {
      receive({ thread_id: "thread1", run_id: "r2", event: { kind: "started", task: "follow up", max_turns: 3 } });
      receive({ thread_id: "thread1", run_id: "r2", event: { kind: "assistant_text", text: "Milestone: Follow-up\nI will check one more thing." } });
      receive({ thread_id: "thread1", run_id: "r2", event: { kind: "tool_started", tool: "list_files", summary: "workspace" } });
    });
    expect(screen.getByText("Verified.").closest(".themis-thread-msg")).not.toHaveClass("themis-thread-msg--action-waiting");
    expect(screen.getByText("I will check one more thing.").closest(".themis-thread-msg")).toHaveClass("themis-thread-msg--action-waiting");
    // A steered message is a plain bubble in place; no label repeats where it came from.
    await act(async () => receive({ thread_id: "thread1", run_id: "r2", event: { kind: "steered", text: "Use tabs." } }));
    expect(screen.getByText("Use tabs.")).toBeInTheDocument();
    expect(screen.queryByText(/Steered in/)).toBeNull();
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
  it("switches projects from the switcher without disturbing the composer", async () => {
    const first = { id: "a", title: "First thread", provider: "go" as const, model: "muse-spark-1.3-contributor", running: false, worktree_path: "/tmp/a", branch: "test", base_branch: "main", recovered: false, skill_ids: [] };
    vi.mocked(bridge.getSettings).mockResolvedValue({ ...settings, recent_roots: ["/tmp/one", "/tmp/two"] });
    vi.mocked(bridge.openProject).mockImplementation(async root => ({ root, name: root.endsWith("one") ? "One" : "Two", is_git: true }));
    vi.mocked(bridge.listThreads).mockImplementation(async root => root === "/tmp/fixed-themis" ? [] : [root.endsWith("one") ? first : { ...first, id: "b", title: "Second thread" }]);
    await act(async () => render(<App />));
    const sidebar = screen.getByRole("complementary", { name: "Workspace navigation" });
    fireEvent.click(within(sidebar).getByRole("button", { name: /^One/ }));
    expect(within(sidebar).getByRole("button", { name: "First thread" })).toBeInTheDocument();
    expect(within(sidebar).queryByRole("button", { name: "Second thread" })).toBeNull();
    fireEvent.click(within(sidebar).getByRole("button", { name: /^Two/ }));
    expect(within(sidebar).getByRole("button", { name: "Second thread" })).toHaveAttribute("aria-current", "true");
    expect(within(sidebar).queryByRole("button", { name: "First thread" })).toBeNull();
    expect(screen.getByLabelText("Message")).toBeEnabled();
  });
  it("shows restored running, completed, failed, and stopped thread status", async () => {
    const thread = { id: "status", title: "Status thread", provider: "go" as const, model: "muse-spark-1.3-contributor", running: true, worktree_path: "/tmp/a", branch: "test", base_branch: "main", recovered: false, skill_ids: [] };
    vi.mocked(bridge.getSettings).mockResolvedValue({ ...settings, recent_roots: ["/tmp/one"] });
    vi.mocked(bridge.openProject).mockResolvedValue({ root: "/tmp/one", name: "One", is_git: true });
    vi.mocked(bridge.listThreads).mockImplementation(async root => root === "/tmp/fixed-themis" ? [] : [thread]);
    await mount();
    const sidebar = screen.getByRole("complementary", { name: "Workspace navigation" });
    fireEvent.click(within(sidebar).getByRole("button", { name: /^One/ }));
    expect(within(within(sidebar).getByRole("region", { name: "Chats" })).getByRole("img", { name: "Working" })).toBeInTheDocument();
    expect(within(sidebar).queryByRole("region", { name: "Needs you" })).toBeNull();
    expect(within(sidebar).getByRole("button", { name: "Edit Status thread" })).toBeDisabled();
    const receive = vi.mocked(bridge.onThreadEvent).mock.calls[0]![0];
    await act(async () => receive({ thread_id: "status", run_id: "r", event: { kind: "finished", result: "Done" } }));
    // The finished thread is the one on screen, so there is no unseen-completion check.
    expect(within(sidebar).queryByRole("img")).toBeNull();
    await act(async () => receive({ thread_id: "status", run_id: "old", event: { kind: "incomplete", result: "I completed the draft. Next, verify it." } }));
    expect(within(sidebar).queryByRole("img")).toBeNull();
    expect(screen.queryByText(/Run activity · Incomplete/)).not.toBeInTheDocument();
    await act(async () => receive({ thread_id: "status", run_id: "r2", event: { kind: "started", task: "retry", max_turns: 3 } }));
    expect(within(sidebar).getByRole("img", { name: "Working" })).toBeInTheDocument();
    await act(async () => receive({ thread_id: "status", run_id: "r2", event: { kind: "failed", error: "HTTP 503" } }));
    expect(within(sidebar).getByRole("img", { name: "Failed" })).toBeInTheDocument();
    await act(async () => receive({ thread_id: "status", run_id: "r3", event: { kind: "failed", error: "Stopped by you." } }));
    expect(within(sidebar).queryByRole("img")).toBeNull();
  });
  it("marks a background run that finished as done until the chat is opened", async () => {
    const base = { provider: "go" as const, model: "muse-spark-1.3-contributor", worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] };
    const front = { ...base, id: "front", title: "Front chat", running: false, last_activity_seq: 2 };
    const back = { ...base, id: "back", title: "Background chat", running: true, last_activity_seq: 1 };
    vi.mocked(bridge.getSettings).mockResolvedValue({ ...settings, recent_roots: ["/tmp/one"] });
    vi.mocked(bridge.openProject).mockResolvedValue({ root: "/tmp/one", name: "One", is_git: true });
    vi.mocked(bridge.listThreads).mockImplementation(async root => root === "/tmp/fixed-themis" ? [] : [front, back]);
    await mount();
    const sidebar = screen.getByRole("complementary", { name: "Workspace navigation" });
    fireEvent.click(within(sidebar).getByRole("button", { name: /^One/ }));
    expect(within(sidebar).getByRole("button", { name: "Front chat" })).toHaveAttribute("aria-current", "true");
    const receive = vi.mocked(bridge.onThreadEvent).mock.calls[0]![0];
    await act(async () => receive({ thread_id: "back", run_id: "r", event: { kind: "finished", result: "Done" } }));
    expect(within(sidebar).getByRole("img", { name: "Done" })).toBeInTheDocument();
    await act(async () => fireEvent.click(within(sidebar).getByRole("button", { name: "Background chat" })));
    expect(within(sidebar).queryByRole("img", { name: "Done" })).toBeNull();
  });
  it("lists the files a turn edited under its reply and opens the workspace from them", async () => {
    const base = { provider: "go" as const, model: "muse-spark-1.3-contributor", worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] };
    vi.mocked(bridge.getSettings).mockResolvedValue({ ...settings, recent_roots: ["/tmp/one"] });
    vi.mocked(bridge.openProject).mockResolvedValue({ root: "/tmp/one", name: "One", is_git: true });
    vi.mocked(bridge.listThreads).mockImplementation(async root => root === "/tmp/fixed-themis" ? [] : [{ ...base, id: "edit", title: "Edit chat", running: false }]);
    await mount();
    const sidebar = screen.getByRole("complementary", { name: "Workspace navigation" });
    fireEvent.click(within(sidebar).getByRole("button", { name: /^One/ }));
    const receive = vi.mocked(bridge.onThreadEvent).mock.calls[0]![0];
    await act(async () => receive({ thread_id: "edit", run_id: "r", event: { kind: "started", task: "fix", max_turns: 3 } }));
    await act(async () => receive({ thread_id: "edit", run_id: "r", event: { kind: "files_changed", files: [{ path: "src/stats.py", status: "modified", added: 7, removed: 2 }, { path: "tests/test_stats.py", status: "added", added: 10, removed: 0 }] } }));
    await act(async () => receive({ thread_id: "edit", run_id: "r", event: { kind: "finished", result: "Fixed the median." } }));
    const edited = screen.getByRole("region", { name: "Files edited in this turn" });
    expect(edited).toHaveTextContent("Edited 2 files");
    expect(edited).toHaveTextContent("src/stats.py");
    expect(within(edited).getByLabelText("17 added, 2 removed")).toHaveTextContent("+17 -2");
    expect(within(edited).getByLabelText("7 added, 2 removed")).toHaveTextContent("+7 -2");
    expect(within(edited).getByLabelText("added")).toHaveTextContent("A");
    await act(async () => fireEvent.click(within(edited).getAllByRole("button")[0]!));
    expect(screen.getByRole("complementary", { name: "Workspace (read-only)" })).toBeInTheDocument();
    expect(screen.getByText("Edited 2 files")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Show all/ })).toBeNull();
    const many = Array.from({ length: 12 }, (_, index) => ({ path: `src/module_${index}.py`, status: "modified" as const }));
    await act(async () => receive({ thread_id: "edit", run_id: "r2", event: { kind: "started", task: "big", max_turns: 3 } }));
    await act(async () => receive({ thread_id: "edit", run_id: "r2", event: { kind: "files_changed", files: many } }));
    await act(async () => receive({ thread_id: "edit", run_id: "r2", event: { kind: "finished", result: "Refactored." } }));
    const regions = screen.getAllByRole("region", { name: "Files edited in this turn" });
    const big = regions[regions.length - 1]!;
    // Older chats saved no counts, and show none rather than a made-up zero.
    expect(big).not.toHaveTextContent("+0");
    expect(within(big).getAllByRole("listitem")).toHaveLength(5);
    fireEvent.click(within(big).getByRole("button", { name: "Show all 12" }));
    expect(within(big).getAllByRole("listitem")).toHaveLength(12);
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
    // Settings keeps the folded state, and the header toggle stays where it was.
    expect(document.querySelector(".themis-sidebar-slot")).toHaveClass("is-collapsed");
    expect(screen.getByRole("button", { name: "Show sidebar" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Back to app" }));
    fireEvent.click(screen.getByRole("button", { name: "Show sidebar" }));
    expect(document.getElementById("themis-sidebar")).not.toHaveAttribute("inert");
  });
  it("gives Settings the whole sidebar: no rail, one way back, and the same header toggle", async () => {
    await mount();
    const slot = document.querySelector<HTMLElement>(".themis-sidebar-slot")!;
    const width = slot.style.getPropertyValue("--themis-sidebar-width");
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    // One column at the sidebar's width: the rail and the chats panel are gone.
    expect(screen.queryByRole("navigation", { name: "Utilities" })).toBeNull();
    expect(screen.queryByRole("complementary", { name: "Workspace navigation" })).toBeNull();
    const nav = screen.getByRole("navigation", { name: "Settings sections" });
    expect(slot).toContainElement(nav);
    expect(nav.id).toBe("themis-sidebar");
    expect(slot).not.toHaveClass("is-collapsed");
    expect(slot.style.getPropertyValue("--themis-sidebar-width")).toBe(width);
    expect(screen.getAllByRole("button", { name: /^(Back to app|Home)$/ }).map(button => button.getAttribute("aria-label"))).toEqual(["Back to app"]);
    expect(within(nav).getByRole("button", { name: "General" })).toHaveFocus();
    // The header toggle folds the Settings list to icons, which keep their names.
    const toggle = screen.getByRole("button", { name: "Collapse sidebar" });
    expect(toggle).toHaveAttribute("aria-controls", "themis-sidebar");
    fireEvent.click(toggle);
    expect(slot).toHaveClass("is-collapsed");
    fireEvent.click(within(nav).getByRole("button", { name: "Advanced" }));
    expect(document.querySelector(".themis-settings-pane-title")).toHaveTextContent("Advanced");
    expect(within(nav).getByRole("button", { name: "Advanced" })).toHaveAttribute("aria-current", "page");
    fireEvent.click(screen.getByRole("button", { name: "Show sidebar" }));
    expect(slot).not.toHaveClass("is-collapsed");
    fireEvent.click(within(nav).getByRole("button", { name: "Back to app" }));
    expect(screen.queryByRole("navigation", { name: "Settings sections" })).toBeNull();
    expect(screen.getByRole("button", { name: "Home" })).toHaveFocus();
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
    fireEvent.click(screen.getByRole("button", { name: "Back to app" }));
    expect(document.documentElement.dataset.palette).toBe("ink");
  });
  it("shows budget-driven compaction settings without the legacy checkpoint interval", async () => {
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    fireEvent.click(screen.getByRole("button", { name: "Advanced" }));
    expect(screen.queryByLabelText("Checkpoint turns")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Working memory")).toHaveValue("auto");
    expect(screen.queryByLabelText("Working memory tokens")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Recent messages")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Steps per reply")).toHaveValue(settings.max_total_turns);
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Reset limits" })));
    expect(bridge.updateSettings).toHaveBeenLastCalledWith({ max_total_turns: 200, context_token_cap: 0, concurrency_limit: 3 });
  });
  it("keeps utilities separate from the expanded project dock and pins Themis", async () => {
    await mount();
    const rail = screen.getByRole("navigation", { name: "Utilities" });
    const sidebar = screen.getByRole("complementary", { name: "Workspace navigation" });
    expect(within(sidebar).getByRole("button", { name: "New chat" })).toBeEnabled();
    expect(within(sidebar).queryByRole("button", { name: "Settings" })).not.toBeInTheDocument();
    expect(within(sidebar).queryByRole("button", { name: "Integrations" })).not.toBeInTheDocument();
    expect(within(sidebar).getByText("Themis", { selector: ".themis-wordmark" })).toBeInTheDocument();
    expect(within(sidebar).getByLabelText("Project: Themis")).toHaveAttribute("title", "/tmp/fixed-themis");
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
  it("shows a model Themis cannot call as unavailable and never lets it be chosen", async () => {
    vi.mocked(bridge.listGoModels).mockResolvedValueOnce([
      { id: "muse-spark-1.3-contributor", effort_levels: [] },
      { id: "odd-model", effort_levels: [], unavailable_reason: "Needs @ai-sdk/xyz, which Themis does not support yet" },
    ]);
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Models" })));
    const option = screen.getByRole("option", { name: "odd-model (unavailable)" });
    expect(option).toBeDisabled();
    expect(option).toHaveAttribute("title", "Needs @ai-sdk/xyz, which Themis does not support yet");
    expect(screen.getByRole("option", { name: "muse-spark-1.3-contributor" })).toBeEnabled();
  });
});
