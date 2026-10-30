import { useEffect, useRef, useState } from "react";
import type { WorkspaceInfo } from "../lib/types";

/** The project's directory, branch and changes for the composer and
 *  Workspace: loaded whenever the project changes, even while its chat runs,
 *  and refreshed when a run ends. Never returns another project's info. */
const WORKSPACE_REREAD_MS = 5000;

export function useProjectWorkspace(
  projectRoot: string | undefined,
  running: boolean,
  load: (projectRoot: string) => Promise<WorkspaceInfo>,
  version = 0,
): WorkspaceInfo | null {
  const [loaded, setLoaded] = useState<WorkspaceInfo | null>(null);
  const loadedRoot = useRef<string | null>(null);
  const loadedVersion = useRef(version);
  useEffect(() => {
    if (projectRoot === undefined || (running && loadedRoot.current === projectRoot && loadedVersion.current === version)) return;
    let cancelled = false;
    load(projectRoot).then(info => {
      if (cancelled) return;
      loadedRoot.current = projectRoot;
      loadedVersion.current = version;
      setLoaded(info);
    }).catch(() => { if (!cancelled) setLoaded(null); });
    return () => { cancelled = true; };
  }, [projectRoot, running, load, version]);
  // Branches also change outside the app: keep them current, and reread on coming back.
  useEffect(() => {
    if (projectRoot === undefined) return;
    let stopped = false;
    const reread = () => {
      if (document.hidden) return;
      void load(projectRoot).then(info => {
        if (!stopped) setLoaded(previous => JSON.stringify(previous) === JSON.stringify(info) ? previous : info);
      }).catch(() => {});
    };
    const timer = setInterval(reread, WORKSPACE_REREAD_MS);
    window.addEventListener("focus", reread);
    return () => { stopped = true; clearInterval(timer); window.removeEventListener("focus", reread); };
  }, [projectRoot, load]);
  return loaded?.root === projectRoot ? loaded : null;
}
