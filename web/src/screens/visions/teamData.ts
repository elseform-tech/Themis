// Server reads behind the team screens: a worker's thread with its chief, the channel's routines
// and its spend today. Each reads again when the channel moves (a new line, or the server saying
// the channel changed: a report, a pause, a routine), drops an answer that arrives after the
// channel or worker changed, and keeps the last good value when a read fails.
import { useEffect, useState } from "react";
import { onVisionsChanged } from "../../lib/tauri";
import { deleteRoutine, getChannelSpend, getTeamLog, getVisionsSettings, listRoutines, setRoutineEnabled, type Routine, type VisionsSettings } from "../../lib/visions";
import { useApp } from "../../state/store";
import type { RoutineControls } from "./Routines";
import type { TeamLine } from "./team";

/** A value that moves whenever the channel does: its newest line, and the server's change
 *  events naming it. */
function useChannelTick(channelId: string): string {
  const { state } = useApp();
  const lines = state.visions.lines[channelId] ?? [];
  const newest = lines.length ? lines[lines.length - 1]!.seq : 0;
  const [changes, setChanges] = useState(0);
  useEffect(() => {
    let off: (() => void) | undefined;
    let live = true;
    void onVisionsChanged(change => {
      if (live && change.channel_id === channelId) setChanges(count => count + 1);
    }).then(unlisten => { if (live) off = unlisten; else unlisten(); }).catch(() => undefined);
    return () => { live = false; off?.(); };
  }, [channelId]);
  return `${newest}:${changes}`;
}

/** Reads `read` for `key` whenever `tick` moves (or `reload` is called); a stale answer (for an
 *  older key) is dropped. */
function useRead<T>(key: string | undefined, tick: string, read: (key: string) => Promise<T>): [T | undefined, () => void] {
  const [value, setValue] = useState<{ key: string; value: T } | undefined>(undefined);
  const [version, setVersion] = useState(0);
  useEffect(() => {
    if (!key) return;
    let live = true;
    read(key).then(answer => { if (live) setValue({ key, value: answer }); }).catch(() => undefined);
    return () => { live = false; };
    // `read` is a module function at each call site.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, tick, version]);
  return [value && value.key === key ? value.value : undefined, () => setVersion(count => count + 1)];
}

export function useTeamLog(channelId: string, workerId: string): TeamLine[] | null {
  const tick = useChannelTick(channelId);
  const [page] = useRead(`${channelId}\n${workerId}`, tick, key => {
    const [channel, worker] = key.split("\n") as [string, string];
    return getTeamLog(channel, worker);
  });
  return page ? (page.entries as TeamLine[]) : null;
}

export function useRoutines(channelId: string | undefined): RoutineControls | undefined {
  const tick = useChannelTick(channelId ?? "");
  const [list, again] = useRead<Routine[]>(channelId, tick, listRoutines);
  if (!channelId || !list) return undefined;
  return {
    list,
    setEnabled: async (routineId, enabled) => { await setRoutineEnabled(routineId, enabled); again(); },
    remove: async routineId => { await deleteRoutine(routineId); again(); },
  };
}

/** Today's spend, or undefined while it is unknown (the profile then shows the limit only). */
export function useChannelSpend(channelId: string | undefined): number | undefined {
  const tick = useChannelTick(channelId ?? "");
  const [read] = useRead(channelId, tick, getChannelSpend);
  return read?.spent_usd ?? undefined;
}

/** Settings › Visions, for a channel's autonomy and budget when it sets none of its own. */
export function useVisionsSettings(): VisionsSettings | null {
  const [settings, setSettings] = useState<VisionsSettings | null>(null);
  useEffect(() => { void getVisionsSettings().then(view => setSettings(view.settings)).catch(() => setSettings(null)); }, []);
  return settings;
}
