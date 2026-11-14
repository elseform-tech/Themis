// @vitest-environment jsdom
import { act, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as bridge from "../lib/tauri";
import type { ChannelInfo } from "../lib/visions";
import { AppProvider, DEFAULT_SETTINGS, useApp, type AppAction } from "./store";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("../lib/tauri", async importOriginal => ({
  ...await importOriginal<typeof import("../lib/tauri")>(),
  pluginAction: vi.fn(async () => []), getDefaultProject: vi.fn(async () => null), getSettings: vi.fn(), getSecretStatus: vi.fn(async () => ({ go: false })),
  onBackendResync: vi.fn(async () => () => {}), getPendingApprovals: vi.fn(async () => []), onNotificationOpened: vi.fn(async () => () => {}),
  onThreadEvent: vi.fn(async () => () => {}), onApprovalRequest: vi.fn(async () => () => {}), onApprovalExpired: vi.fn(async () => () => {}), onReviewItemAdded: vi.fn(async () => () => {}), onThreadChanged: vi.fn(async () => () => {}), onSettingsChanged: vi.fn(async () => () => {}), onVisionsChanged: vi.fn(async () => () => {}),
  listSkills: vi.fn(async () => []), listAutomations: vi.fn(async () => []), listReviewItems: vi.fn(async () => []), getBrowserSettings: vi.fn(async () => ({ settings: { open_workspace: true } })),
  openProject: vi.fn(), listThreads: vi.fn(async () => []), getThreadHistory: vi.fn(async () => []),
}));
vi.mock("../lib/visions", async importOriginal => ({
  ...await importOriginal<typeof import("../lib/visions")>(),
  listVisions: vi.fn(async () => []), listChannels: vi.fn(async () => []), getChannelPage: vi.fn(async () => ({ entries: [], next_before: null })), listChannelThreads: vi.fn(async () => []), listQuestions: vi.fn(async () => []),
}));

const sent = vi.hoisted(() => ({ dispatch: null as null | ((action: AppAction) => void) }));

function Probe() {
  const { state, dispatch } = useApp();
  sent.dispatch = dispatch;
  return <div data-testid="open">{String(state.visions.open.channelId)}</div>;
}

const channel = (id: string, root: string): ChannelInfo => ({ id, name: id, topic: "", project_root: root, approval_mode: "custom", members: [], revision: "r" });

beforeEach(() => {
  vi.mocked(bridge.getSettings).mockResolvedValue({ ...DEFAULT_SETTINGS, recent_roots: [] });
  window.matchMedia = vi.fn().mockReturnValue({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() });
});

describe("the app's store", () => {
  it("lands Visions on the new project's # hq after a project change from anywhere", async () => {
    await act(async () => { render(<AppProvider><Probe /></AppProvider>); });
    const send = (action: AppAction) => act(() => { sent.dispatch!(action); });
    send({ type: "project/opened", project: { root: "/p", name: "P", is_git: true } });
    send({ type: "project/opened", project: { root: "/q", name: "Q", is_git: true }, select: false });
    send({ type: "visions/loaded", projectRoot: "/p", channels: [channel("c1", "/p")] });
    send({ type: "visions/loaded", projectRoot: "/q", channels: [channel("c2", "/q")] });
    send({ type: "visions/open", channelId: "c1" });
    expect(screen.getByTestId("open")).toHaveTextContent("c1");
    send({ type: "project/selected", root: "/q" });
    expect(screen.getByTestId("open")).toHaveTextContent("null");
  });
});
