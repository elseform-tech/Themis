import { useEffect, useRef, useState } from "react";
import type { WorkspaceInfo } from "../lib/types";

/** The project's directory, branch and changes for the composer and
 *  Workspace: loaded whenever the project changes, even while its chat runs,
 *  and refreshed when a run ends. Never returns another project's info. */
export function useProjectWorkspace(
  projectRoot: string | undefined,
  running: boolean,
  load: (projectRoot: string) => Promise<WorkspaceInfo>,
): WorkspaceInfo | null {
  const [loaded, setLoaded] = useState<WorkspaceInfo | null>(null);
  const loadedRoot = useRef<string | null>(null);
  useEffect(() => {
    if (projectRoot === undefined || (running && loadedRoot.current === projectRoot)) return;
    let cancelled = false;
    load(projectRoot).then(info => {
      if (cancelled) return;
      loadedRoot.current = projectRoot;
      setLoaded(info);
    }).catch(() => { if (!cancelled) setLoaded(null); });
    return () => { cancelled = true; };
  }, [projectRoot, running, load]);
  return loaded?.root === projectRoot ? loaded : null;
}
