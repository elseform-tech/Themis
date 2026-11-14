// Server reads behind the team screens: a worker's thread with its chief, the channel's routines
// and its spend today. Each reloads when the channel's log moves, and keeps the last good value
// when a read fails (the screens show what they have).
import { useCallback, useEffect, useState } from "react";
import { deleteRoutine, getChannelSpend, getTeamLog, listRoutines, setRoutineEnabled, type Routine } from "../../lib/visions";
import { useApp } from "../../state/store";
import type { RoutineControls } from "./Routines";
import type { TeamLine } from "./team";

/** A change counter for the channel: its log's length, which grows with every line. */
function useChannelTick(channelId: string): number {
  const { state } = useApp();
  return state.visions.lines[channelId]?.length ?? 0;
}

export function useTeamLog(channelId: string, workerId: string): TeamLine[] | null {
  const tick = useChannelTick(channelId);
  const [log, setLog] = useState<TeamLine[] | null>(null);
  useEffect(() => {
    let live = true;
    getTeamLog(channelId, workerId)
      .then(page => { if (live) setLog(page.entries as TeamLine[]); })
      .catch(() => undefined);
    return () => { live = false; };
  }, [channelId, workerId, tick]);
  return log;
}

export function useRoutines(channelId: string | undefined): RoutineControls | undefined {
  const tick = useChannelTick(channelId ?? "");
  const [list, setList] = useState<Routine[] | null>(null);
  const reload = useCallback(() => {
    if (!channelId) return;
    void listRoutines(channelId).then(setList).catch(() => undefined);
  }, [channelId]);
  useEffect(reload, [reload, tick]);
  if (!channelId || !list) return undefined;
  return {
    list,
    setEnabled: async (routineId, enabled) => { await setRoutineEnabled(routineId, enabled); reload(); },
    remove: async routineId => { await deleteRoutine(routineId); reload(); },
  };
}

export function useChannelSpend(channelId: string | undefined): number | undefined {
  const tick = useChannelTick(channelId ?? "");
  const [spent, setSpent] = useState<number | undefined>(undefined);
  useEffect(() => {
    if (!channelId) return;
    let live = true;
    getChannelSpend(channelId)
      .then(read => { if (live) setSpent(read.spent_usd); })
      .catch(() => undefined);
    return () => { live = false; };
  }, [channelId, tick]);
  return spent;
}
