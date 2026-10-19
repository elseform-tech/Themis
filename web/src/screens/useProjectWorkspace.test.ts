// @vitest-environment jsdom
import { renderHook, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import type { WorkspaceInfo } from "../lib/types";
import { useProjectWorkspace } from "./useProjectWorkspace";

const info = (root: string) => ({ root, branch: "main" }) as unknown as WorkspaceInfo;

it("loads a newly selected project even while its chat runs, and never shows the old one", async () => {
  const load = vi.fn(async (root: string) => info(root));
  const { result, rerender } = renderHook(
    ({ root, running }) => useProjectWorkspace(root, running, load),
    { initialProps: { root: "/p/NightSky", running: false } },
  );
  await waitFor(() => expect(result.current?.root).toBe("/p/NightSky"));
  rerender({ root: "/p/Planetarium", running: true });
  // The previous project's info is never shown for the new one.
  expect(result.current).toBeNull();
  await waitFor(() => expect(result.current?.root).toBe("/p/Planetarium"));
  expect(load).toHaveBeenLastCalledWith("/p/Planetarium");
});

it("does not reload the same project mid-run, and refreshes when the run ends", async () => {
  const load = vi.fn(async (root: string) => info(root));
  const { result, rerender } = renderHook(
    ({ running }) => useProjectWorkspace("/p/Planetarium", running, load),
    { initialProps: { running: false } },
  );
  await waitFor(() => expect(result.current).not.toBeNull());
  rerender({ running: true });
  expect(load).toHaveBeenCalledTimes(1);
  rerender({ running: false });
  await waitFor(() => expect(load).toHaveBeenCalledTimes(2));
});
