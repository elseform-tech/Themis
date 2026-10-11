// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { ThreadEventEnvelope } from "../lib/types";
import { AppProvider, useApp } from "./store";

const backend = vi.hoisted(() => ({
  status: vi.fn(),
  event: null as ((event: ThreadEventEnvelope) => void) | null,
  reconnect: null as (() => void) | null,
}));

vi.mock("../lib/tauri", () => ({
  getSecretStatus: backend.status,
  getSettings: async () => ({ recent_roots: [], completion_sound: false }),
  getDefaultProject: async () => null,
  listThreads: async () => [],
  getThreadHistory: async () => [],
  importLegacyHistory: async () => {},
  openProject: async (root: string) => ({ root, name: "project", is_git: false }),
  listSkills: async () => [],
  listAutomations: async () => [],
  listReviewItems: async () => [],
  getPendingApprovals: async () => [],
  onThreadEvent: async (callback: typeof backend.event) => {
    backend.event = callback;
    return () => { backend.event = null; };
  },
  onBackendResync: async (callback: typeof backend.reconnect) => {
    backend.reconnect = callback;
    return () => { backend.reconnect = null; };
  },
  onApprovalRequest: async () => () => {},
  onReviewItemAdded: async () => () => {},
}));

function Readiness() {
  const { state } = useApp();
  return <><output>{state.secretStatus.go ? "available" : "unavailable"}</output>
    {state.toasts.map(toast => <p key={toast.id}>{toast.message}</p>)}</>;
}

beforeEach(() => {
  vi.stubGlobal("localStorage", window.localStorage);
  backend.status.mockReset().mockResolvedValue({ go: false });
  sessionStorage.clear();
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

it("refreshes mounted readiness on returning to the app after credential unlock", async () => {
  render(<AppProvider><Readiness /></AppProvider>);
  await waitFor(() => expect(backend.status).toHaveBeenCalledOnce());
  backend.status.mockResolvedValue({ go: true });
  fireEvent.focus(window);
  await screen.findByText("available");
  backend.status.mockResolvedValue({ go: false });
  fireEvent.focus(window);
  await screen.findByText("unavailable");
});

it("preserves confirmed readiness when a later lookup fails", async () => {
  backend.status.mockResolvedValue({ go: true });
  render(<AppProvider><Readiness /></AppProvider>);
  await screen.findByText("available");
  backend.status.mockRejectedValue(new Error("Credential lookup timed out"));
  fireEvent.focus(window);
  await screen.findByText(/Failed to load secret status/);
  expect(screen.getByText("available")).toBeInTheDocument();
});

it("coalesces readiness refresh bursts and refreshes after reconnect or completion", async () => {
  let complete!: (status: {go: boolean}) => void;
  backend.status.mockImplementationOnce(() => new Promise(resolve => { complete = resolve; }))
    .mockResolvedValue({ go: true });
  render(<AppProvider><Readiness /></AppProvider>);
  await waitFor(() => expect(backend.reconnect).toBeTypeOf("function"));
  fireEvent.focus(window);
  fireEvent(document, new Event("visibilitychange"));
  act(() => {
    backend.reconnect!();
    backend.event!({thread_id: "chat", run_id: "run", event: {kind: "finished", result: "done"}});
  });
  expect(backend.status).toHaveBeenCalledOnce();
  await act(async () => { complete({go: false}); });
  await screen.findByText("available");
  expect(backend.status).toHaveBeenCalledTimes(2);
});
