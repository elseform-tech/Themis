import { useEffect, useState } from "react";

function formatWait(ms: number): string {
  const seconds = Math.ceil(ms / 1000);
  return seconds < 60 ? `${seconds} s` : `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

/** What a goal is waiting on between turns, where "Thinking…" sits during one. It leaves nothing
 *  in the chat: a countdown would be out of date as soon as it stopped. */
export function GoalWait({ checking, retryAt }: { checking?: boolean; retryAt?: number }) {
  const [now, setNow] = useState(() => Date.now());
  const waiting = !checking && retryAt !== undefined && retryAt > now;
  useEffect(() => {
    if (!waiting) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [waiting]);
  if (!checking && !waiting) return null;
  return (
    <div className="themis-run-progress" role="status">
      <div className="themis-run-status">
        <span className={checking ? "themis-shimmer" : undefined}>{checking ? "Checking…" : `Trying again in ${formatWait(retryAt! - now)}`}</span>
      </div>
    </div>
  );
}
